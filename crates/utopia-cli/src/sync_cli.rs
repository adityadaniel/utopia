//! `utopia sync …` (§12): headless setup, one-shot sync, status and configuration.
//!
//! This is the no-window equivalent of Settings → Sync, for an agent, a launchd job, or a
//! machine with no app (`ut`, Linux included). Three rules shape it:
//!
//! - **One settings format.** The saved preferences are `utopia_sync::setup::Preferences`
//!   in the same per-graph file Settings → Sync writes (`paths::sync_config`), and the
//!   transport comes from `crate::sync_target`, the builder the window uses too.
//! - **Secrets never become arguments, files or output** (§15.6). They are read from a
//!   terminal without echo, or as `--token-stdin` / `--passphrase-stdin` lines, and go to
//!   the platform store through the `Secrets` trait (tests use its in-memory double).
//! - **One writer** (MC-4). `setup`, `config` and `sync` take the owner lock; `status` only
//!   looks at whether it is held. A graph the open app owns is synced by the app, and the
//!   command says so with [`EXIT_OWNED`] instead of writing.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::json;
use utopia_core::PageId;
use utopia_store::{OwnerLock, Store};
use utopia_sync::runner::{exchange_owned, BoxTransport, SyncRunner as SyncRound};
use utopia_sync::setup::probe::{probe, Connection};
use utopia_sync::setup::verified_target::{VerifiedTarget, UNREADABLE};
use utopia_sync::setup::{
    may_freeze_key, save_secrets, verify_passphrase, Environment, Keychain, Mode, Preferences,
    Secrets, PASSPHRASE_SERVICE, TOKEN_SERVICE,
};
use utopia_sync::{FailureKind, SyncEngine, SyncError, SyncReport, SyncStatus, Transport};

use crate::sync_target::{self, Discover};

pub const USAGE: &str = "\
usage:
  utopia sync setup --graph <file.utopia> --mode relay|nearby|folder [--url <relay-url>]
                    [--folder <path>] [--graph-id <id>] [--token-stdin] [--passphrase-stdin]
        one-time headless setup: write this graph's sync settings (the same file Settings →
        Sync uses), its graph id and, when given, the relay token and E2E passphrase to the
        Keychain (on Linux, the credentials file). Creates an empty graph when --graph does not exist, so the first
        `utopia sync` pulls everything.
  utopia sync --graph <file.utopia> [--json]
        run sync rounds until nothing is left to push or pull, then exit
  utopia sync --continuous --graph <file.utopia>
        keep syncing until SIGTERM/SIGINT: a round every 5 s while there is activity,
        one a minute when idle, on relay notices, and shortly after a local write; also
        serves the automation API so utopia mcp and utopia capture forward to this process
  utopia sync launchd-plist --graph <file.utopia>
        print a LaunchAgent plist for --continuous on stdout (nothing is installed)
  utopia sync status --graph <file.utopia> [--json]
        show the saved mode and target, the graph id, whether secrets are stored, whether
        the environment overrides the settings, the last successful sync, the queued
        changes and whether another process owns the graph
  utopia sync config --graph <file.utopia> (--off | --mode relay|nearby|folder [--url …] [--folder …])
        change the saved settings (same file, same validation); a blank secret keeps the
        stored one
  utopia sync inspect --graph <file.utopia> [--page <date|title|id>] [--deep] [--json]
        compare this device with the destination, read-only (works while the app is
        open): unsent changes, pages not pulled yet or not on the destination, and changes
        that wait for another device; --deep (or --page) reads each page's copy on the
        destination too (one full pull) and says which device has to act. Exit 1 when it
        finds something
  utopia sync repair --graph <file.utopia> [--json]
        forget the pull positions, so the next sync compares every page with the
        destination and sends what it lacks (for a page another device reports as waiting
        for changes from this one); costs one full pull, sends only what is missing

`serve-api --sync` is an alias for `sync --continuous`. The daemon takes the graph's owner
lock for its whole life (MC-4), so a second utopia process forwards writes to it instead of
opening the graph; stopping it releases the lock, flushes the store and removes its
discovery file.

--graph is necessary for every form: utopia sync never picks a graph by the app's start-up
order, so a script cannot reach a graph its author did not name. The token and passphrase
are read from stdin without echo on a terminal, or as one line each with --token-stdin /
--passphrase-stdin (token first when both are given). They are never arguments, settings
fields or output, and are stored only in the macOS Keychain (services utopia-sync and
utopia-sync-passphrase), or on Linux in <config>/credentials.json, a file only you can read
(mode 0600; one that others can read is refused).

environment: UTOPIA_SYNC_URL, UTOPIA_SYNC_FOLDER, UTOPIA_LAN_SERVE and UTOPIA_SYNC_GRAPH
override the saved transport for a run; setup and config refuse to save while they are set.
UTOPIA_SYNC_TOKEN and UTOPIA_SYNC_PASSPHRASE override the stored credentials without
being saved.

exit codes: 0 synced (or the command succeeded); 1 bad arguments, an unreachable target, a
refused credential, a wrong E2E passphrase, or a sync that left changes queued; 2 Sync is
Off for this graph; 3 another process owns the graph (the open app or serve-api already
syncs it).";

/// Synced (or the command changed the settings).
pub const EXIT_OK: i32 = 0;
/// Sync is Off for this graph: there is no transport to run.
pub const EXIT_OFF: i32 = 2;
/// Another process owns the graph and syncs it (MC-4).
pub const EXIT_OWNED: i32 = 3;

/// How many one-shot rounds to run before giving up with the outbox still full. One
/// `SyncEngine::sync_once` already loops until a round moves nothing; this outer bound only
/// covers an edit that keeps arriving between rounds (a store another process cannot use,
/// because this run owns the graph).
const MAX_ROUNDS: usize = 8;

/// `utopia sync …`.
pub fn run(args: &[String]) -> Result<i32> {
    if args
        .first()
        .is_some_and(|a| matches!(a.as_str(), "-h" | "--help" | "help"))
    {
        println!("{USAGE}");
        return Ok(EXIT_OK);
    }
    let (command, rest) = match args.first().map(String::as_str) {
        Some("setup") => ("setup", &args[1..]),
        Some("status") => ("status", &args[1..]),
        Some("config") => ("config", &args[1..]),
        Some("launchd-plist") => ("launchd-plist", &args[1..]),
        Some("repair") => ("repair", &args[1..]),
        Some("inspect") => ("inspect", &args[1..]),
        Some(other) if !other.starts_with('-') => bail!(
            "unknown utopia sync command {other:?}: use setup, status, config, inspect or \
             repair, or no command to sync once.\n\n{USAGE}"
        ),
        _ => ("sync", args),
    };
    let options = Options::parse(rest)?;
    let config = crate::paths::config_dir();
    match command {
        "setup" => {
            options.ensure_only(
                "setup",
                &[
                    "--graph",
                    "--mode",
                    "--url",
                    "--folder",
                    "--graph-id",
                    "--token-stdin",
                    "--passphrase-stdin",
                ],
            )?;
            apply_command(&options, &config, true)
        }
        "config" => {
            options.ensure_only(
                "config",
                &["--graph", "--mode", "--url", "--folder", "--off"],
            )?;
            apply_command(&options, &config, false)
        }
        "status" => {
            options.ensure_only("status", &["--graph", "--json"])?;
            status(&options, &config)
        }
        "launchd-plist" => {
            options.ensure_only("launchd-plist", &["--graph"])?;
            launchd_plist(&options)
        }
        "repair" => {
            options.ensure_only("repair", &["--graph", "--json"])?;
            repair(&options)
        }
        "inspect" => {
            options.ensure_only("inspect", &["--graph", "--json", "--page", "--deep"])?;
            inspect_command(&options, &config)
        }
        _ => {
            options.ensure_only("sync", &["--graph", "--json", "--continuous"])?;
            if options.continuous {
                let graph = options.graph()?;
                return continuous(graph, &config, "127.0.0.1:0");
            }
            match one_shot(&options, &config) {
                Ok(code) => Ok(code),
                Err(error) => {
                    // --json promises one object on stdout even when the run could not start;
                    // the human text still goes to stderr.
                    if options.json {
                        emit(&json!({
                            "version": 1,
                            "ok": false,
                            "errors": [error.to_string()],
                        }))?;
                    }
                    Err(error)
                }
            }
        }
    }
}

/// Every option the four forms take; a form rejects the ones it does not use rather than
/// silently ignoring them.
#[derive(Default)]
struct Options {
    graph: Option<PathBuf>,
    json: bool,
    mode: Option<Mode>,
    url: Option<String>,
    folder: Option<PathBuf>,
    graph_id: Option<String>,
    token_stdin: bool,
    passphrase_stdin: bool,
    off: bool,
    continuous: bool,
    page: Option<String>,
    deep: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self::default();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--graph" => options.graph = Some(take(args, &mut i, "--graph")?.into()),
                "--json" => options.json = true,
                "--mode" => options.mode = Some(parse_mode(&take(args, &mut i, "--mode")?)?),
                "--url" => options.url = Some(take(args, &mut i, "--url")?),
                "--folder" => options.folder = Some(take(args, &mut i, "--folder")?.into()),
                "--graph-id" => options.graph_id = Some(take(args, &mut i, "--graph-id")?),
                "--token-stdin" => options.token_stdin = true,
                "--passphrase-stdin" => options.passphrase_stdin = true,
                "--off" => options.off = true,
                "--continuous" => options.continuous = true,
                "--page" => options.page = Some(take(args, &mut i, "--page")?),
                "--deep" => options.deep = true,
                other => bail!("unknown option {other:?} for utopia sync.\n\n{USAGE}"),
            }
            i += 1;
        }
        Ok(options)
    }

    /// The flags this invocation supplied. `ensure_only` compares them with the ones a form
    /// takes, so a flag cannot be parsed and then silently ignored.
    fn given(&self) -> Vec<&'static str> {
        let mut given = Vec::new();
        if self.graph.is_some() {
            given.push("--graph");
        }
        if self.json {
            given.push("--json");
        }
        if self.mode.is_some() {
            given.push("--mode");
        }
        if self.url.is_some() {
            given.push("--url");
        }
        if self.folder.is_some() {
            given.push("--folder");
        }
        if self.graph_id.is_some() {
            given.push("--graph-id");
        }
        if self.token_stdin {
            given.push("--token-stdin");
        }
        if self.passphrase_stdin {
            given.push("--passphrase-stdin");
        }
        if self.off {
            given.push("--off");
        }
        if self.continuous {
            given.push("--continuous");
        }
        if self.page.is_some() {
            given.push("--page");
        }
        if self.deep {
            given.push("--deep");
        }
        given
    }

    /// Refuse an option the selected form does not take. Runs before the graph is opened:
    /// a malformed configuration command must not fall through to a one-shot sync.
    fn ensure_only(&self, command: &str, allowed: &[&str]) -> Result<()> {
        let unsupported: Vec<&str> = self
            .given()
            .into_iter()
            .filter(|flag| !allowed.contains(flag))
            .collect();
        if unsupported.is_empty() {
            return Ok(());
        }
        bail!(
            "utopia sync {command} does not take {}. Use `utopia sync setup` or `utopia sync \
             config` to change the saved settings.\n\n{USAGE}",
            unsupported.join(", ")
        );
    }

    /// `--graph` is necessary in every form (see [`USAGE`]).
    fn graph(&self) -> Result<&Path> {
        self.graph.as_deref().with_context(|| {
            format!(
                "utopia sync needs --graph <file.utopia>: it never picks a graph by the \
                 app's start-up order.\n\n{USAGE}"
            )
        })
    }
}

fn take(args: &[String], i: &mut usize, name: &str) -> Result<String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .with_context(|| format!("{name} needs a value.\n\n{USAGE}"))
}

fn parse_mode(name: &str) -> Result<Mode> {
    Ok(match name {
        "off" => Mode::Off,
        "relay" => Mode::Relay,
        "nearby" => Mode::Nearby,
        "folder" => Mode::Folder,
        other => bail!("unknown sync mode {other:?}: use relay, nearby or folder.\n\n{USAGE}"),
    })
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Off => "off",
        Mode::Relay => "relay",
        Mode::Nearby => "nearby",
        Mode::Folder => "folder",
    }
}

/// Where the settings point, as it may be printed: never a token or passphrase (§15.6).
fn target_text(preferences: &Preferences) -> Option<String> {
    match preferences.mode {
        Mode::Off => None,
        Mode::Relay => (!preferences.url.is_empty()).then(|| preferences.url.clone()),
        Mode::Nearby => Some(match &preferences.host {
            Some(bind) => format!("nearby host on {bind}"),
            None => "nearby (mDNS discovery)".into(),
        }),
        Mode::Folder => preferences
            .folder
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

fn keychain() -> Keychain {
    Keychain::new(std::env::var("UTOPIA_SYNC_KEYCHAIN_PREFIX").ok())
}

/// The run's credentials: the environment first, the Keychain only for what the environment
/// did not provide (§15.6).
///
/// The app's own resolver also compares an environment passphrase with the stored one; the
/// headless path gets that protection from the graph's key marker (`verify_passphrase`) and
/// from `VerifiedTarget`, which refuses to push into a target it cannot read. Reading the
/// Keychain only for the missing field is what lets a script hand both values in without
/// touching a stored credential.
fn credentials(
    env: &Environment,
    secrets: &dyn Secrets,
    graph: &str,
) -> Result<(Option<String>, Option<String>), String> {
    let token = match env.token.clone() {
        Some(token) => Some(token),
        None => secrets.get(TOKEN_SERVICE, graph)?,
    };
    let passphrase = match env.passphrase.clone() {
        Some(passphrase) => Some(passphrase),
        None => secrets.get(PASSPHRASE_SERVICE, graph)?,
    };
    Ok((token, passphrase))
}

/// The stored credentials for the fields this call does not supply. A typed token or
/// passphrase replaces the stored one, so the old value is not read (`save_secrets` still
/// refuses a passphrase that differs from one already saved).
fn stored_credentials(
    secrets: &dyn Secrets,
    graph: &str,
    typed: &Typed,
    env: &Environment,
) -> Result<(Option<String>, Option<String>), String> {
    let token = if env.token.is_none() && typed.token.is_empty() {
        secrets.get(TOKEN_SERVICE, graph)?
    } else {
        None
    };
    let passphrase = if env.passphrase.is_none() && typed.passphrase.is_empty() {
        secrets.get(PASSPHRASE_SERVICE, graph)?
    } else {
        None
    };
    Ok((token, passphrase))
}

/// Secrets typed for this call (`setup`). Blank keeps what is stored, as a blank secret
/// field in Settings does.
#[derive(Default)]
struct Typed {
    token: String,
    passphrase: String,
}

/// Read the secrets this call was asked for: no echo on a terminal, or one line each for a
/// script. A non-terminal stdin without either flag supplies nothing, so a command in a
/// pipeline never blocks waiting for a line it was not promised.
fn read_typed(options: &Options, mode: Mode) -> Result<Typed> {
    let terminal = std::io::stdin().is_terminal();
    let mut typed = Typed::default();
    if options.token_stdin {
        typed.token = read_secret_line("--token-stdin")?;
    } else if terminal && matches!(mode, Mode::Relay | Mode::Nearby) {
        typed.token = crate::cli::prompt_secret("relay token (blank for none): ")?;
    }
    if options.passphrase_stdin {
        typed.passphrase = read_secret_line("--passphrase-stdin")?;
    } else if terminal {
        typed.passphrase = crate::cli::prompt_secret("E2E passphrase (blank for none): ")?;
    }
    Ok(typed)
}

fn read_secret_line(flag: &str) -> Result<String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .with_context(|| format!("reading the secret for {flag} from stdin"))?;
    if line.is_empty() {
        bail!("{flag} needs one line on stdin.");
    }
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// The saved transport for a `setup` or `config` call. Setup starts from the defaults (the
/// mode must name its target); config starts from what is saved, so a field it does not name
/// is kept, exactly as the fields in Settings are.
/// The mode a `setup` or `config` call names, before any file is read: `setup` must name a
/// transport, and `config` may turn sync off.
fn requested_mode(options: &Options, setup: bool) -> Result<Mode, String> {
    let mode = match (options.off, options.mode) {
        (true, _) => Mode::Off,
        (false, Some(mode)) => mode,
        (false, None) => return Err("Choose --mode relay|nearby|folder, or --off.".into()),
    };
    if setup && mode == Mode::Off {
        return Err(
            "utopia sync setup sets a transport; use `utopia sync config --off` to turn \
             sync off."
                .into(),
        );
    }
    Ok(mode)
}

fn preferences_for(
    options: &Options,
    base: &Preferences,
    setup: bool,
) -> Result<Preferences, String> {
    let mode = requested_mode(options, setup)?;
    let mut preferences = if setup {
        Preferences::default()
    } else {
        base.clone()
    };
    preferences.version = 1;
    preferences.mode = mode;
    match mode {
        Mode::Off => preferences = Preferences::default(),
        Mode::Relay => {
            preferences.url = match &options.url {
                Some(url) => url.clone(),
                None if setup => String::new(),
                None => preferences.url.clone(),
            };
            preferences.folder = None;
            preferences.host = None;
        }
        Mode::Nearby => {
            preferences.url.clear();
            preferences.folder = None;
            // The CLI does not host (there is no --host): a Nearby client finds its host on
            // the LAN. A host recorded by Settings stays, because `config --mode nearby`
            // changes only the mode.
            if setup {
                preferences.host = None;
            }
        }
        Mode::Folder => {
            preferences.url.clear();
            preferences.host = None;
            preferences.folder = match &options.folder {
                Some(folder) => Some(folder.clone()),
                None if setup => None,
                None => preferences.folder.clone(),
            };
        }
    }
    preferences.validate()?;
    Ok(preferences)
}

/// The graph id this graph publishes under (§12.5): the environment's choice, else the id
/// frozen into the graph, else its file name made URL-safe.
fn graph_id(store: &Store, graph: &Path, env: &Environment) -> Result<String> {
    if let Some(id) = &env.graph {
        return Ok(id.clone());
    }
    if let Some(id) = store.sync_get("setup_graph")? {
        return Ok(id);
    }
    Ok(crate::paths::graph_id(graph))
}

/// `setup --graph-id` may name the id only before it names synced data; afterwards it is
/// frozen, like a graph id in the app (§12.5).
fn setup_graph_id(requested: Option<&str>, store: &Store, graph: &Path) -> Result<String> {
    let id = match (requested, store.sync_get("setup_graph")?) {
        (Some(given), Some(stored)) if given != stored => bail!(
            "This graph already syncs as {stored:?}. Its id is frozen once it names synced \
             data (§12.5); use the original id on every device."
        ),
        (Some(given), _) => given.to_string(),
        (None, Some(stored)) => stored,
        (None, None) => crate::paths::graph_id(graph),
    };
    if !utopia_sync::http::valid_graph_id(&id) {
        bail!("Enter a graph id: letters, digits, hyphens and underscores only.");
    }
    Ok(id)
}

/// Become the graph's owner, or answer with the exit code that says another process is
/// (MC-4). A command that cannot own the graph has nothing to do here: its owner syncs it.
/// `json` additionally emits the refusal as the one object `--json` promises, because a
/// launchd job or agent sees this outcome routinely.
fn owner_or_exit(graph: &Path, json: bool) -> Result<std::result::Result<OwnerLock, i32>> {
    match crate::cli::take_owner(graph) {
        Ok(lock) => Ok(Ok(lock)),
        Err(e) if e.downcast_ref::<crate::cli::AlreadyOpen>().is_some() => {
            let message = format!("{e}; the open process already syncs this graph.");
            eprintln!("{message}");
            if json {
                emit(&json!({
                    "version": 1,
                    "ok": false,
                    "owned": true,
                    "errors": [message],
                }))?;
            }
            Ok(Err(EXIT_OWNED))
        }
        Err(e) => Err(e),
    }
}

fn lan_discovery() -> impl Fn(&str) -> Option<String> {
    |graph| utopia_syncd::lan::discover(graph, Duration::from_secs(5))
}

/// `utopia sync setup` and `utopia sync config`: resolve the target once, apply the same
/// E2E rules as the app, and write the destination, the frozen id and the settings in the
/// app's order (destination first, settings last).
fn apply_command(options: &Options, config_dir: &Path, setup: bool) -> Result<i32> {
    let graph = options.graph()?.to_path_buf();
    if !setup && !graph.exists() {
        bail!(
            "no graph at {}; run `utopia sync setup --graph {} …` first.",
            graph.display(),
            graph.display()
        );
    }
    let env = Environment::read(|key| std::env::var(key).ok());
    let overrides = env.transport_names();
    if !overrides.is_empty() {
        bail!(
            "Sync settings cannot be saved while {} overrides the transport for this run. \
             Unset {} and run the command again.",
            overrides.join(", "),
            if overrides.len() == 1 { "it" } else { "them" }
        );
    }
    // Read a script's lines (or a terminal's prompts) before the lock and the store: a
    // prompt should not hold the graph, and the lines are consumed whatever happens next.
    let typed = if setup {
        read_typed(
            options,
            requested_mode(options, true).map_err(anyhow::Error::msg)?,
        )?
    } else {
        Typed::default()
    };
    let _owner = match owner_or_exit(&graph, false)? {
        Ok(lock) => lock,
        Err(code) => return Ok(code),
    };
    let store = if graph.exists() {
        crate::cli::open_store(&graph)?
    } else {
        Store::open(&graph).with_context(|| format!("creating {}", graph.display()))?
    };
    // The settings file is named by the graph's canonical path, so it is computed only now:
    // a graph setup just created has no canonical path until it exists.
    let file = crate::paths::sync_config(config_dir, &graph);
    let (saved, _notice) = Preferences::load(&file);
    let preferences = preferences_for(options, &saved, setup).map_err(anyhow::Error::msg)?;
    let summary = if preferences.mode == Mode::Off {
        // Nothing to probe: turning sync off only changes the settings file (Settings does
        // the same) and the next run reads it.
        preferences.save(&file).map_err(anyhow::Error::msg)?;
        format!("Sync is Off for {}.", graph.display())
    } else {
        let secrets = keychain();
        let discover = lan_discovery();
        save_and_apply(
            &graph,
            config_dir,
            Save {
                store,
                preferences,
                requested_id: options.graph_id.clone(),
                typed: &typed,
                secrets: &secrets,
                env: &env,
                discover: &discover,
            },
        )?
    };
    // The owner lock is bound before the store, so the store drops (and closes) first: the
    // sidecar stays held until every write is in.
    println!("{summary}");
    Ok(EXIT_OK)
}

/// What a setup/config run saves, and the services it does it through.
struct Save<'a> {
    store: Store,
    preferences: Preferences,
    /// The id `--graph-id` asked for, if any.
    requested_id: Option<String>,
    /// Secrets typed for this call; blank keeps the stored ones.
    typed: &'a Typed,
    secrets: &'a dyn Secrets,
    env: &'a Environment,
    discover: &'a Discover,
}

fn save_and_apply(graph: &Path, config_dir: &Path, run: Save<'_>) -> Result<String> {
    let Save {
        mut store,
        preferences,
        requested_id,
        typed,
        secrets,
        env,
        discover,
    } = run;
    let id = setup_graph_id(requested_id.as_deref(), &store, graph)?;
    let file = crate::paths::sync_config(config_dir, graph);
    let marker = store.sync_get("setup_e2e_check")?;
    let (stored_token, stored_passphrase) =
        stored_credentials(secrets, &id, typed, env).map_err(anyhow::Error::msg)?;
    let token = env
        .token
        .clone()
        .or_else(|| (!typed.token.is_empty()).then(|| typed.token.clone()))
        .or(stored_token);
    let passphrase = env
        .passphrase
        .clone()
        .or_else(|| (!typed.passphrase.is_empty()).then(|| typed.passphrase.clone()))
        .or(stored_passphrase);
    // §12.6: a graph that has used E2E refuses another key, before the target is touched.
    let key_check = verify_passphrase(passphrase.as_deref(), &id, marker.as_deref())
        .map_err(anyhow::Error::msg)?;
    let Some(target) = sync_target::resolve(
        &preferences,
        env,
        &id,
        sync_target::Credentials { token, passphrase },
        &file,
        None,
        discover,
    )
    .map_err(anyhow::Error::msg)?
    else {
        bail!("Sync is Off: choose --mode relay|nearby|folder.");
    };
    let sync_target::Target { config, lan } = target;
    let engine = SyncEngine::new(&store)?;
    let mut transport = VerifiedTarget::new(
        sync_target::transport(&config, engine.client_id()).map_err(anyhow::Error::msg)?,
    );
    // Learn the relay's identity and whether it holds payloads before choosing a
    // destination; an unreachable target still lets setup save (offline-first), except for
    // a first E2E passphrase, which needs a target that was read (§12.6).
    let connected = probe(&mut transport);
    if transport.unreadable {
        bail!("{UNREADABLE}");
    }
    if config.passphrase.is_some()
        && marker.is_none()
        && connected.connection() != Connection::Reachable
    {
        bail!(
            "Verify the target before first enabling E2E encryption. {}",
            connected.connection().message()
        );
    }
    // The first E2E use freezes the graph's key marker, but only against a target that was
    // read *and* held payloads: an empty target proves no key (`may_freeze_key`).
    let freeze_marker = if marker.is_none() && may_freeze_key(&connected) {
        key_check
    } else {
        None
    };
    // Only typed values are saved. Environment credentials are this run's, never the
    // graph's or the Keychain's (§15.6).
    save_secrets(secrets, &id, &typed.token, &typed.passphrase).map_err(anyhow::Error::msg)?;
    let destination =
        sync_target::destination(&config, transport.cursor_space(), connected.instance);
    store.sync_set("setup_graph", &id)?;
    if let Some(marker) = freeze_marker {
        store.sync_set("setup_e2e_check", &marker)?;
    }
    store.prepare_sync_destination(&destination)?;
    preferences.save(&file).map_err(anyhow::Error::msg)?;
    // A hosted Nearby relay, if any, stops here with the one-shot run.
    drop(lan);
    Ok(format!(
        "Sync is set up: {} · graph {id}. Settings in {}.",
        target_text(&preferences).unwrap_or_else(|| "Off".into()),
        file.display()
    ))
}

/// Everything a run needs after the saved settings named a transport: the frozen id, the
/// transport guarded by the first-use check, the engine, and a hosted Nearby relay that the
/// caller must keep alive for as long as it syncs.
struct Prepared {
    id: String,
    target: sync_target::Target,
    transport: VerifiedTarget<BoxTransport>,
    engine: SyncEngine,
}

/// Resolve this run's target exactly as the app launches sync: credentials from the
/// environment or the Keychain, the frozen graph id, the E2E marker, a target that was read
/// before anything is pushed, and the destination record. One-shot `utopia sync` and the
/// continuous daemon both come through here, so a difference between them cannot make one
/// publish where the other would not.
fn prepare_transport(
    store: &mut Store,
    graph: &Path,
    config_dir: &Path,
    env: &Environment,
    preferences: &Preferences,
) -> Result<Prepared> {
    let file = crate::paths::sync_config(config_dir, graph);
    let id = graph_id(store, graph, env)?;
    let secrets = keychain();
    let (token, passphrase) = credentials(env, &secrets, &id).map_err(anyhow::Error::msg)?;
    let marker = store.sync_get("setup_e2e_check")?;
    let key_check = verify_passphrase(passphrase.as_deref(), &id, marker.as_deref())
        .map_err(anyhow::Error::msg)?;
    let discover = lan_discovery();
    let Some(target) = sync_target::resolve(
        preferences,
        env,
        &id,
        sync_target::Credentials { token, passphrase },
        &file,
        None,
        &discover,
    )
    .map_err(anyhow::Error::msg)?
    else {
        bail!("Sync is Off: choose a transport with `utopia sync setup`.");
    };
    let sync_target::Target { config, lan } = target;
    let engine = SyncEngine::new(store)?;
    let mut transport = VerifiedTarget::new(
        sync_target::transport(&config, engine.client_id()).map_err(anyhow::Error::msg)?,
    );
    let connected = probe(&mut transport);
    if transport.unreadable {
        bail!("{UNREADABLE}");
    }
    // A launch that reached the target freezes what now names synced data: the id and,
    // once a payload proved the key, the E2E marker. Both are the app's launch rules.
    store.sync_set("setup_graph", &id)?;
    if marker.is_none() && config.passphrase.is_some() && may_freeze_key(&connected) {
        if let Some(marker) = key_check {
            store.sync_set("setup_e2e_check", &marker)?;
        }
    }
    let destination =
        sync_target::destination(&config, transport.cursor_space(), connected.instance);
    store.prepare_sync_destination(&destination)?;
    Ok(Prepared {
        id,
        target: sync_target::Target { config, lan },
        transport,
        engine,
    })
}

/// `utopia sync`: one run to completion, then exit.
fn one_shot(options: &Options, config_dir: &Path) -> Result<i32> {
    let graph = options.graph()?.to_path_buf();
    if !graph.exists() {
        bail!(
            "no graph at {}; run `utopia sync setup --graph {} …` first.",
            graph.display(),
            graph.display()
        );
    }
    let _owner = match owner_or_exit(&graph, options.json)? {
        Ok(lock) => lock,
        Err(code) => return Ok(code),
    };
    let mut store = crate::cli::open_store(&graph)?;
    let env = Environment::read(|key| std::env::var(key).ok());
    let file = crate::paths::sync_config(config_dir, &graph);
    let (saved, notice) = Preferences::load(&file);
    let preferences = env.apply(&saved);
    // §15.6: an environment override is a target too. Validate it exactly like a saved one
    // before it can be printed, used for a transport, or written into the graph's
    // destination record, so a URL with credentials never gets that far.
    preferences.validate().map_err(anyhow::Error::msg)?;
    if preferences.mode == Mode::Off {
        let message = notice.unwrap_or_else(|| {
            format!(
                "Sync is Off for {}; run `utopia sync setup --graph {} …` first.",
                graph.display(),
                graph.display()
            )
        });
        if options.json {
            emit(&json!({
                "version": 1,
                "ok": false,
                "mode": mode_name(Mode::Off),
                "notice": message,
            }))?;
        }
        eprintln!("{message}");
        return Ok(EXIT_OFF);
    }
    let Prepared {
        id,
        target,
        mut transport,
        mut engine,
    } = prepare_transport(&mut store, &graph, config_dir, &env, &preferences)?;
    let lan = target.lan;
    // §12.6: one line per round that did something, in `<config>/logs/sync.jsonl`.
    let sync_log = utopia_sync::log::SyncLog::new(
        config_dir.join("logs"),
        utopia_store::graph_key(&graph),
        "cli",
    );
    let destination = target_text(&preferences);

    let mut totals = Totals::default();
    let mut errors: Vec<String> = Vec::new();
    for _ in 0..MAX_ROUNDS {
        let result = engine.sync_once(&mut store, &mut transport);
        sync_log.record(&result, &store, destination.as_deref());
        match result {
            Ok(report) => {
                totals.add(&report);
                if !report.failed.is_empty() {
                    errors.extend(report.failed.iter().map(failure_text));
                    break;
                }
                // A round that moved attachments is not quiet even when it moved no pages:
                // `sync_once` sends at most `BLOBS_PER_ROUND` per pass, so an attachment
                // backlog needs more rounds before the queue is empty.
                let moved = !report.is_quiet() || report.blobs_up > 0 || report.blobs_down > 0;
                if !moved && store.outbox_count()? == 0 {
                    break;
                }
            }
            Err(error) => {
                errors.push(error_text(&error));
                break;
            }
        }
    }
    // Flush anything still in memory before the count and the exit; nothing is left
    // half-written when the process stops.
    store.sync_index()?;
    let pending = store.outbox_count()?;
    // Pages are not the whole queue: a destination switch (or an interrupted run) leaves
    // attachments pending, and `outbox_count` does not see them (§12.7). One row is enough
    // to know work remains; its bytes are not kept.
    let attachments_pending = !store.blobs_to_upload(1)?.is_empty();
    if errors.is_empty() && pending > 0 {
        errors.push(format!(
            "{pending} change(s) are still queued after {MAX_ROUNDS} rounds."
        ));
    }
    // §12.6, §2.3.7: another device's changes are held, not applied. The run is not a
    // clean sync, and the error says which side has to act.
    let blocked = store.waiting_pages()?;
    if !blocked.is_empty() {
        errors.push(format!(
            "{} page(s) hold changes from another device that cannot apply yet: that device \
             has not sent this relay the earlier changes they build on. They are kept, and \
             apply when it does.",
            blocked.len()
        ));
    }
    if errors.is_empty() && attachments_pending {
        errors.push(
            "Attachment(s) are still queued for upload; run `utopia sync` again to finish.".into(),
        );
    }
    drop(lan);
    let ok = errors.is_empty();
    if options.json {
        let report = json!({
            "version": 1,
            "ok": ok,
            "graph": graph.display().to_string(),
            "graph_id": id,
            "mode": mode_name(preferences.mode),
            "target": target_text(&preferences),
            "pushed": totals.pushed,
            "duplicates": totals.duplicates,
            "pulled": totals.imported.len(),
            "snapshots": totals.snapshots,
            "blobs_up": totals.blobs_up,
            "blobs_down": totals.blobs_down,
            "attachments_pending": attachments_pending,
            "pending": pending,
            "blocked_pages": blocked_json(&blocked),
            "errors": errors,
        });
        emit(&report)?;
    } else {
        eprintln!(
            "synced {id} ({}): pushed {} change(s), pulled {} page(s), {pending} queued",
            mode_name(preferences.mode),
            totals.pushed,
            totals.imported.len()
        );
        for error in &errors {
            eprintln!("error: {error}");
        }
    }
    Ok(if ok { EXIT_OK } else { 1 })
}

/// `utopia sync inspect`: compare this device with its destination (§12.6). Read-only on
/// the graph and the destination, so it takes no owner lock and runs while the app is open.
/// The token and passphrase come from the same places as a sync and are never printed.
fn inspect_command(options: &Options, config_dir: &Path) -> Result<i32> {
    let graph = options.graph()?.to_path_buf();
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let mut store = crate::cli::open_store(&graph)?;
    let env = Environment::read(|key| std::env::var(key).ok());
    let file = crate::paths::sync_config(config_dir, &graph);
    let (saved, _) = Preferences::load(&file);
    let preferences = env.apply(&saved);
    preferences.validate().map_err(anyhow::Error::msg)?;
    if preferences.mode == Mode::Off {
        eprintln!(
            "Sync is Off for {}: there is no destination to compare with.",
            graph.display()
        );
        return Ok(EXIT_OFF);
    }
    let id = graph_id(&store, &graph, &env)?;
    let secrets = keychain();
    let (token, passphrase) = credentials(&env, &secrets, &id).map_err(anyhow::Error::msg)?;
    let marker = store.sync_get("setup_e2e_check")?;
    verify_passphrase(passphrase.as_deref(), &id, marker.as_deref()).map_err(anyhow::Error::msg)?;
    let discover = lan_discovery();
    let Some(target) = sync_target::resolve(
        &preferences,
        &env,
        &id,
        sync_target::Credentials { token, passphrase },
        &file,
        None,
        &discover,
    )
    .map_err(anyhow::Error::msg)?
    else {
        bail!("Sync is Off: choose a transport with `utopia sync setup`.");
    };
    let sync_target::Target { config, lan } = target;
    let client = store
        .sync_get("client_id")?
        .unwrap_or_else(|| "inspect".into());
    let mut transport = sync_target::transport(&config, &client).map_err(anyhow::Error::msg)?;
    let only = match &options.page {
        Some(p) => Some(vec![store
            .find_page(p)?
            .unwrap_or_else(|| PageId(p.clone()))]),
        None => None,
    };
    let report =
        utopia_sync::inspect::inspect(&mut store, &mut transport, only.as_deref(), options.deep)
            .map_err(|e| anyhow::anyhow!(error_text(&e)))?;
    drop(lan);
    if options.json {
        let mut value = serde_json::to_value(&report)?;
        if let Some(pages) = value.get_mut("pages").and_then(|p| p.as_array_mut()) {
            for (page, r) in pages.iter_mut().zip(&report.pages) {
                page["advice"] = json!(r.findings.iter().map(|f| f.advice()).collect::<Vec<_>>());
            }
        }
        value["graph_id"] = json!(id);
        emit(&value)?;
    } else {
        print_inspection(&id, &report);
    }
    Ok(if report.has_findings() { 1 } else { EXIT_OK })
}

fn print_inspection(id: &str, r: &utopia_sync::inspect::Inspection) {
    let peers = |p: &[utopia_sync::inspect::PeerCount]| {
        if p.is_empty() {
            "none".to_string()
        } else {
            p.iter()
                .map(|c| format!("{}×{}", c.peer, c.changes))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    let waiting = |w: &[utopia_sync::inspect::Waiting]| {
        w.iter()
            .map(|w| format!("peer {} {}..{}", w.peer, w.start, w.end))
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!(
        "graph id {id} · client {} · {}",
        r.client_id.as_deref().unwrap_or("none yet"),
        r.destination
    );
    println!(
        "destination: rev {}, {} page(s) · this device: {} page(s), {} unsent change(s)",
        r.rev, r.destination_pages, r.local_pages, r.unsent
    );
    if r.deep {
        println!("compared each page's copy on the destination");
    } else {
        println!(
            "compared with the destination's page list; add --deep to read each page's copy too"
        );
    }
    for p in &r.pages {
        println!();
        match &p.title {
            Some(t) => println!("page {} \"{t}\"", p.page_id),
            None => println!("page {}", p.page_id),
        }
        let pulled = p.cursor.map_or("never pulled from here".into(), |c| {
            format!("pulled to {c}")
        });
        match &p.local {
            Some(local) => println!(
                "  this device:  {} · unsent {} · {pulled}",
                peers(local),
                p.unsent
            ),
            None => println!("  this device:  does not have this page"),
        }
        if !p.waiting_here.is_empty() {
            println!("                waiting: {}", waiting(&p.waiting_here));
        }
        match (p.head, &p.destination) {
            (None, _) => println!("  destination:  does not list this page"),
            (Some(head), Some(dest)) => {
                println!(
                    "  destination:  head {head} (snapshot {}) · {}",
                    p.snapshot_seq.unwrap_or(0),
                    peers(dest)
                );
                if !p.destination_waiting.is_empty() {
                    println!(
                        "                waiting: {}",
                        waiting(&p.destination_waiting)
                    );
                }
            }
            (Some(head), None) => println!(
                "  destination:  head {head} (snapshot {})",
                p.snapshot_seq.unwrap_or(0)
            ),
        }
        for f in &p.findings {
            let name = serde_json::to_value(f)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            println!("  ! {name}: {}", f.advice());
        }
    }
    if !r.has_findings() {
        println!();
        println!("no findings: this device and the destination agree");
    }
}

/// `utopia sync repair`: drop the pull positions so the next round compares every page
/// with its destination (§12.3 step 4). It writes `sync_state`, so it owns the graph for
/// the moment it takes (MC-4); it does not sync, and the next `utopia sync` or the app does.
fn repair(options: &Options) -> Result<i32> {
    let graph = options.graph()?.to_path_buf();
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let _lock = match owner_or_exit(&graph, options.json)? {
        Ok(lock) => lock,
        Err(code) => return Ok(code),
    };
    let store = crate::cli::open_store(&graph)?;
    let reset = utopia_sync::SyncEngine::reset_positions(&store)?;
    if options.json {
        emit(&json!({ "version": 1, "ok": true, "positions_reset": reset }))?;
    } else {
        eprintln!(
            "forgot {reset} pull position(s): the next sync compares every page with the \
             destination and sends what it lacks"
        );
    }
    Ok(EXIT_OK)
}

/// `utopia sync status`: read the saved settings, the graph and the owner sidecar without
/// writing anything.
fn status(options: &Options, config_dir: &Path) -> Result<i32> {
    let graph = options.graph()?.to_path_buf();
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let file = crate::paths::sync_config(config_dir, &graph);
    let (saved, notice) = Preferences::load(&file);
    let env = Environment::read(|key| std::env::var(key).ok());
    let mut notice = notice;
    let preferences = env.apply(&saved);
    // The same validation the run uses. An environment target that fails it is Off for this
    // report, with a notice that names the rule and never echoes the URL (§15.6).
    let preferences = match preferences.validate() {
        Ok(()) => preferences,
        Err(message) => {
            notice = Some(format!("Sync is Off: {message}"));
            Preferences::default()
        }
    };
    // Look, never keep: status must work while the app owns the graph (it reports that).
    let held = OwnerLock::try_acquire(&graph)
        .with_context(|| format!("checking the owner of {}", graph.display()))?;
    let owned = held.is_none();
    drop(held);
    let store = crate::cli::open_store(&graph)?;
    let id = graph_id(&store, &graph, &env)?;
    let secrets = keychain();
    let token = env.token.is_some()
        || secrets
            .get(TOKEN_SERVICE, &id)
            .map_err(anyhow::Error::msg)?
            .is_some();
    let passphrase = env.passphrase.is_some()
        || secrets
            .get(PASSPHRASE_SERVICE, &id)
            .map_err(anyhow::Error::msg)?
            .is_some();
    let pending = store.outbox_count()?;
    let blocked = store.waiting_pages()?;
    let last = store
        .sync_get("synced_at")?
        .and_then(|raw| raw.parse::<i64>().ok());
    let report = json!({
        "version": 1,
        "mode": mode_name(preferences.mode),
        "target": target_text(&preferences),
        "graph": graph.display().to_string(),
        "graph_id": id,
        "secrets": { "token": token, "passphrase": passphrase },
        "environment_override": !env.names.is_empty(),
        "environment": env.names,
        "last_synced_at": last,
        "pending": pending,
        "blocked_pages": blocked_json(&blocked),
        "owned": owned,
        "notice": notice,
    });
    if options.json {
        emit(&report)?;
    } else {
        println!("mode: {}", mode_name(preferences.mode));
        println!(
            "target: {}",
            target_text(&preferences).unwrap_or_else(|| "none".into())
        );
        println!("graph id: {id}");
        println!(
            "secrets: token {}, passphrase {}",
            if token { "stored" } else { "missing" },
            if passphrase { "stored" } else { "missing" }
        );
        println!(
            "environment: {}",
            if env.names.is_empty() {
                "none".to_string()
            } else {
                format!("{} override(s) this run", env.names.join(", "))
            }
        );
        println!(
            "last sync: {}",
            match last {
                Some(at) => ago(at, now_ms()),
                None => "never".into(),
            }
        );
        println!("pending: {pending} change(s)");
        if !blocked.is_empty() {
            println!(
                "waiting: {} page(s) hold changes from another device that cannot apply yet",
                blocked.len()
            );
        }
        println!(
            "owner: {}",
            if owned {
                "another process (it syncs this graph)"
            } else {
                "free"
            }
        );
        if let Some(notice) = &notice {
            println!("notice: {notice}");
        }
    }
    Ok(EXIT_OK)
}

#[derive(Default)]
struct Totals {
    pushed: usize,
    duplicates: usize,
    snapshots: usize,
    blobs_up: usize,
    blobs_down: usize,
    imported: Vec<PageId>,
}

impl Totals {
    fn add(&mut self, report: &SyncReport) {
        self.pushed += report.pushed;
        self.duplicates += report.duplicates;
        self.snapshots += report.snapshots;
        self.blobs_up += report.blobs_up;
        self.blobs_down += report.blobs_down;
        for page in &report.imported {
            if !self.imported.contains(page) {
                self.imported.push(page.clone());
            }
        }
    }
}

fn failure_text(failure: &utopia_sync::Failure) -> String {
    let page = failure
        .page
        .as_ref()
        .map(|page| format!("page {}: ", page.0))
        .unwrap_or_default();
    let why = match &failure.kind {
        FailureKind::Unauthorized => "the target refused the credentials (401/403)".into(),
        FailureKind::ProtocolVersion(version) => {
            format!("the target speaks protocol version {version}")
        }
        FailureKind::Transport(message) | FailureKind::Store(message) => message.clone(),
    };
    format!("{page}{why}")
}

fn error_text(error: &SyncError) -> String {
    match error {
        SyncError::Unauthorized => "the sync target refused this device's token (401/403)".into(),
        SyncError::ProtocolVersion(version) => {
            format!("the sync target speaks protocol version {version}; update Utopia or the relay")
        }
        other => other.to_string(),
    }
}

/// Pages that hold another device's changes which cannot apply yet (§12.6): the page id,
/// when it started to wait, and the peers and counters that wait. No page text (§15.6).
fn blocked_json(pages: &[utopia_store::WaitingPage]) -> serde_json::Value {
    serde_json::Value::Array(
        pages
            .iter()
            .map(|p| {
                json!({
                    "page_id": p.page.as_str(),
                    "since": p.since,
                    "waiting": p.ranges.iter().map(|r| json!({
                        "peer": r.peer.to_string(),
                        "start": r.start,
                        "end": r.end,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn emit(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn ago(at_ms: i64, now_ms: i64) -> String {
    let seconds = ((now_ms - at_ms) / 1000).max(0);
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86_400)
    }
}

/// The daemon's stop flag, set by SIGTERM/SIGINT. A signal handler may only touch atomics;
/// the loop notices it between steps and lets a round that already started finish.
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn request_stop(_: libc::c_int) {
    STOP.store(true, std::sync::atomic::Ordering::SeqCst);
}

fn install_stop_handlers() {
    STOP.store(false, std::sync::atomic::Ordering::SeqCst);
    // SAFETY: `request_stop` only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(
            libc::SIGTERM,
            request_stop as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            request_stop as *const () as libc::sighandler_t,
        );
    }
}

/// The network half of a round, running on its own thread while the main loop keeps
/// draining the API. The result is the exchange's answer and the transport coming home.
struct InFlight {
    result: std::sync::mpsc::Receiver<(Result<utopia_sync::Incoming, SyncError>, BoxTransport)>,
}

enum RoundStart {
    InFlight(InFlight),
    Finished(Result<SyncReport, SyncError>),
}

/// Prepare on the store-owning thread and send the network exchange to a worker. Only the
/// exchange leaves the thread; apply stays here, so a round seen by SQLite is still one
/// ordered transaction run by the owner.
fn start_round(runner: &mut SyncRound, store: &mut Store) -> RoundStart {
    let Some(transport) = runner.begin_round() else {
        // Only one loop drives this runner; a second round cannot be started.
        return RoundStart::Finished(Ok(SyncReport::default()));
    };
    match runner.engine.prepare(store) {
        Ok(out) => {
            runner.exchange();
            let (answers, result) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("utopia-sync-exchange".into())
                .spawn(move || {
                    let _ = answers.send(exchange_owned(transport, out));
                })
                .expect("spawn the sync exchange worker");
            RoundStart::InFlight(InFlight { result })
        }
        Err(error) => {
            let report: Result<SyncReport, SyncError> = Err(error);
            let status = SyncStatus::from_result(&report, store);
            runner.round_failed(transport, status);
            RoundStart::Finished(report)
        }
    }
}

/// Import an exchange's answer on the thread that owns the store, then put the transport
/// home and set the next cadence.
fn finish_round(
    runner: &mut SyncRound,
    store: &mut Store,
    exchanged: (Result<utopia_sync::Incoming, SyncError>, BoxTransport),
) -> Result<SyncReport, SyncError> {
    let (result, transport) = exchanged;
    runner.exchange_returned();
    let report = match result {
        Ok(incoming) => runner.engine.apply(store, incoming),
        Err(error) => Err(error),
    };
    let status = SyncStatus::from_result(&report, store);
    runner.end_round(transport, &report, status);
    report
}

/// Run every queued API call; a write nudges the next round. This runs between every step
/// of the daemon, including while an exchange is in flight, so an agent's call cannot wait
/// out the network (the app's UI thread does the same).
fn drain_api(
    api: &utopia_api::ApiServer,
    store: &mut Store,
    nudge: &std::sync::atomic::AtomicBool,
) {
    while let Some(call) = api.try_recv() {
        let changed = call.run(store);
        if !changed.is_empty() {
            nudge.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// Whether `error` is the one transient failure a daemon retries: a Nearby client whose
/// host is asleep or off the network. A bad credential or a first E2E passphrase that
/// cannot be proven is still fatal, as it is for the app.
fn nearby_discovery_failure(preferences: &Preferences, error: &anyhow::Error) -> bool {
    preferences.mode == Mode::Nearby
        && preferences.host.is_none()
        && error.to_string() == sync_target::NO_NEARBY_HOST
}

/// The discovery file names this process. It is removed on every exit, clean or fatal;
/// nothing else can own the graph while the lock is held.
struct DiscoveryGuard(PathBuf);

impl Drop for DiscoveryGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// One line per finished round, on stderr and without secrets (§15.6). A refused or failed
/// round still says what happened and what the next cadence is.
fn log_round(round: u64, result: &Result<SyncReport, SyncError>, runner: &SyncRound) {
    let next = runner.interval.as_secs();
    match result {
        Ok(report) if report.failed.is_empty() => {
            if !report.is_quiet() || report.blobs_up > 0 || report.blobs_down > 0 {
                eprintln!(
                    "utopia: sync round {round}: pushed {}, pulled {}, snapshots {}, blobs {}/{}, next in {next}s",
                    report.pushed,
                    report.imported.len(),
                    report.snapshots,
                    report.blobs_up,
                    report.blobs_down
                );
            } else {
                eprintln!("utopia: sync round {round}: quiet, next in {next}s");
            }
        }
        Ok(report) => {
            eprintln!(
                "utopia: sync round {round}: {} page(s) could not sync, next in {next}s",
                report.failed.len()
            );
        }
        Err(error) => {
            eprintln!(
                "utopia: sync round {round}: {}, next in {next}s",
                error_text(error)
            );
        }
    }
}

/// `utopia sync --continuous` (and the `utopia serve-api --sync` alias): keep this graph in
/// sync, and serve the loopback API while doing it, until SIGTERM/SIGINT. It owns the graph
/// for its whole life (MC-4), so `utopia mcp` (Remote) and `utopia capture` forward to it
/// instead of opening a second writer.
///
/// The API starts before the transport is resolved, so an agent keeps working while a
/// Nearby host is asleep; discovery is retried on the slow cadence. The network exchange of
/// each round runs on a worker (`utopia-sync-exchange`), while prepare/apply and every API
/// call stay on this thread with the store.
pub fn continuous(graph: &Path, config_dir: &Path, addr: &str) -> Result<i32> {
    if !graph.exists() {
        bail!(
            "no graph at {}; run `utopia sync setup --graph {} …` first.",
            graph.display(),
            graph.display()
        );
    }
    install_stop_handlers();
    // Another process owning the graph is a routine answer (the app or a daemon already
    // syncs it), so it gets the one-shot command's exit code instead of a generic failure.
    let owner = match owner_or_exit(graph, false)? {
        Ok(lock) => lock,
        Err(code) => return Ok(code),
    };
    let mut store = crate::cli::open_store(graph)?;
    let env = Environment::read(|key| std::env::var(key).ok());
    let file = crate::paths::sync_config(config_dir, graph);
    let (saved, notice) = Preferences::load(&file);
    let preferences = env.apply(&saved);
    preferences.validate().map_err(anyhow::Error::msg)?;
    if preferences.mode == Mode::Off {
        let message = notice.unwrap_or_else(|| {
            format!(
                "Sync is Off for {}; run `utopia sync setup --graph {} …` first.",
                graph.display(),
                graph.display()
            )
        });
        eprintln!("{message}");
        return Ok(EXIT_OFF);
    }
    let level = crate::api_launch::load_access(config_dir, graph).level;
    let _discovery = DiscoveryGuard(utopia_api::discovery_file(config_dir, graph));
    let api = crate::api_launch::serve(addr, level, config_dir, graph)?;
    eprintln!("utopia: automation API on {}", api.url());
    eprintln!("utopia: assistant access: {level:?}");

    let nudge = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut runner: Option<SyncRound> = None;
    let mut hosted: Option<utopia_syncd::lan::LanRelay> = None;
    let mut inflight: Option<InFlight> = None;
    let mut next_due = std::time::Instant::now();
    let mut next_prepare = std::time::Instant::now();
    let mut round = 0u64;
    let mut fatal: Option<anyhow::Error> = None;
    let sync_log = utopia_sync::log::SyncLog::new(
        config_dir.join("logs"),
        utopia_store::graph_key(graph),
        "daemon",
    );
    let destination = target_text(&preferences);

    loop {
        if !STOP.load(std::sync::atomic::Ordering::SeqCst) {
            drain_api(&api, &mut store, &nudge);
        }
        if let Some(current) = runner.as_mut() {
            if let Some(waiting) = inflight.as_ref() {
                match waiting.result.try_recv() {
                    Ok(exchanged) => {
                        inflight = None;
                        let result = finish_round(current, &mut store, exchanged);
                        round += 1;
                        log_round(round, &result, current);
                        sync_log.record(&result, &store, destination.as_deref());
                        next_due = std::time::Instant::now() + current.interval;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        inflight = None;
                        fatal = Some(anyhow::anyhow!(
                            "the sync exchange worker stopped without a result"
                        ));
                        break;
                    }
                }
            } else if !STOP.load(std::sync::atomic::Ordering::SeqCst) {
                let due = std::time::Instant::now() >= next_due;
                let nudged =
                    !current.is_refused() && nudge.swap(false, std::sync::atomic::Ordering::SeqCst);
                if due || nudged {
                    match start_round(current, &mut store) {
                        RoundStart::InFlight(waiting) => inflight = Some(waiting),
                        RoundStart::Finished(result) => {
                            round += 1;
                            log_round(round, &result, current);
                            sync_log.record(&result, &store, destination.as_deref());
                            next_due = std::time::Instant::now() + current.interval;
                        }
                    }
                }
            }
        } else if !STOP.load(std::sync::atomic::Ordering::SeqCst)
            && std::time::Instant::now() >= next_prepare
        {
            match prepare_transport(&mut store, graph, config_dir, &env, &preferences) {
                Ok(Prepared {
                    id,
                    target: sync_target::Target { config, lan: host },
                    transport,
                    engine,
                }) => {
                    let target_text = target_text(&preferences).unwrap_or_else(|| "unknown".into());
                    let mut started = SyncRound::new(engine, Box::new(transport) as BoxTransport);
                    started.nudge = nudge.clone();
                    hosted = host;
                    eprintln!(
                        "utopia: continuous sync for {} (graph {id}, {target_text})",
                        graph.display()
                    );
                    // The daemon always listens: Settings → Sync "Live updates" is about a
                    // user's presence, and there is no user here to follow (§12.3,
                    // `docs/sync-modes.md`).
                    if let utopia_sync::runner::SyncTarget::Relay { url, token } = &config.target {
                        started.realtime = Some(
                            utopia_sync::realtime::Listener::start(
                                url.clone(),
                                config.graph.clone(),
                                token.clone(),
                                nudge.clone(),
                            )
                            .map_err(|error| {
                                anyhow::anyhow!("starting the realtime listener: {error}")
                            })?,
                        );
                        eprintln!("utopia: realtime listener started for {url}");
                    } else {
                        eprintln!("utopia: no realtime listener for a folder target; rounds poll");
                    }
                    runner = Some(started);
                    next_due = std::time::Instant::now();
                    eprintln!("utopia: continuous sync loop started");
                }
                Err(error) if nearby_discovery_failure(&preferences, &error) => {
                    eprintln!(
                        "utopia: Nearby host not found for {}; retrying discovery in {}s",
                        graph.display(),
                        utopia_sync::runner::SLOW.as_secs()
                    );
                    next_prepare = std::time::Instant::now() + utopia_sync::runner::SLOW;
                }
                Err(error) => {
                    fatal = Some(error);
                    break;
                }
            }
        }
        if STOP.load(std::sync::atomic::Ordering::SeqCst) && inflight.is_none() {
            break;
        }
        let now = std::time::Instant::now();
        let until = if runner.is_some() {
            next_due.saturating_duration_since(now)
        } else {
            next_prepare.saturating_duration_since(now)
        };
        std::thread::sleep(
            until
                .min(Duration::from_millis(100))
                .max(Duration::from_millis(5)),
        );
    }

    // Stop the listener before the store: a notice that arrives during shutdown must not
    // wake a round the loop will never run. An exchange already in flight finished above,
    // so no round is left half-applied.
    if let Some(runner) = runner.as_mut() {
        if let Some(listener) = runner.realtime.take() {
            listener.stop();
        }
    }
    drop(inflight);
    drop(runner);
    drop(api);
    // Finish the writer's buffered work before the store and the lock go away (MC-4).
    store.sync_index()?;
    store.flush()?;
    drop(store);
    drop(owner);
    drop(hosted);
    if let Some(error) = fatal {
        return Err(error);
    }
    eprintln!("utopia: continuous sync stopped");
    Ok(EXIT_OK)
}

/// The `launchd` plist for `utopia sync --continuous`: printed to stdout, never installed.
/// `home` is the user whose LaunchAgent directory and log directory it names.
pub fn launchd_plist_text(binary: &Path, graph: &Path, home: &Path) -> String {
    let stem = graph
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("graph");
    let label: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let label = format!("local.utopia.sync.{label}");
    let logs = home.join("Library/Logs/Utopia");
    let out = logs.join(format!("sync-{stem}.out.log"));
    let err = logs.join(format!("sync-{stem}.err.log"));
    let xml = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>sync</string>
    <string>--continuous</string>
    <string>--graph</string>
    <string>{graph}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{out}</string>
  <key>StandardErrorPath</key>
  <string>{err}</string>
</dict>
</plist>
"#,
        label = xml(&label),
        binary = xml(&binary.display().to_string()),
        graph = xml(&graph.display().to_string()),
        out = xml(&out.display().to_string()),
        err = xml(&err.display().to_string()),
    )
}

fn launchd_plist(options: &Options) -> Result<i32> {
    let graph = options.graph()?;
    let graph = graph
        .canonicalize()
        .with_context(|| format!("resolving {}", graph.display()))?;
    let binary = std::env::current_exe().context("finding the utopia binary")?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .context("HOME must name an absolute home directory")?;
    print!("{}", launchd_plist_text(&binary, &graph, &home));
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The in-memory double: no test touches a real Keychain item (§15.6).
    #[derive(Default)]
    struct MemorySecrets(Mutex<Vec<(String, String, String)>>);
    impl Secrets for MemorySecrets {
        fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|(s, a, _)| s == service && a == account)
                .map(|(_, _, value)| value.clone()))
        }
        fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
            let mut all = self.0.lock().unwrap();
            all.retain(|(s, a, _)| !(s == service && a == account));
            all.push((service.into(), account.into(), value.into()));
            Ok(())
        }
    }

    struct EmptyConfig;
    impl Secrets for EmptyConfig {
        fn get(&self, _: &str, _: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            panic!("config/sync with environment credentials must not write the Keychain")
        }
    }

    fn options_for(graph: &Path, mode: Mode) -> Options {
        Options {
            graph: Some(graph.to_path_buf()),
            mode: Some(mode),
            ..Default::default()
        }
    }

    /// §12 / §15.6: setup writes the same per-graph file the window reads, stores the typed
    /// secrets only in `Secrets`, and never prints or serializes them.
    #[test]
    fn setup_writes_the_settings_the_window_reads_and_never_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("Agent.utopia");
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let config = dir.path().join("config");
        let store = Store::open(&graph).unwrap();
        let secrets = MemorySecrets::default();
        let typed = Typed {
            token: "setup-token-secret".into(),
            passphrase: "setup-passphrase-secret".into(),
        };
        let mut options = options_for(&graph, Mode::Folder);
        options.folder = Some(shared.clone());
        let preferences = preferences_for(&options, &Preferences::default(), true).unwrap();
        let discover = |_: &str| None;
        let summary = save_and_apply(
            &graph,
            &config,
            Save {
                store,
                preferences: preferences.clone(),
                requested_id: Some("agent-graph".into()),
                typed: &typed,
                secrets: &secrets,
                env: &Environment::default(),
                discover: &discover,
            },
        )
        .unwrap();

        // The app's loader reads back exactly what setup saved.
        let file = crate::paths::sync_config(&config, &graph);
        assert_eq!(Preferences::load(&file), (preferences, None));
        let raw = std::fs::read_to_string(&file).unwrap();
        for secret in [&typed.token, &typed.passphrase] {
            assert!(!raw.contains(secret), "the settings file holds a secret");
            assert!(!summary.contains(secret), "the summary holds a secret");
        }
        // The credentials are in the (double) Keychain, under the graph id.
        assert_eq!(
            secrets
                .get(TOKEN_SERVICE, "agent-graph")
                .unwrap()
                .as_deref(),
            Some(typed.token.as_str())
        );
        assert_eq!(
            secrets
                .get(PASSPHRASE_SERVICE, "agent-graph")
                .unwrap()
                .as_deref(),
            Some(typed.passphrase.as_str())
        );
        let store = Store::open(&graph).unwrap();
        assert_eq!(
            store.sync_get("setup_graph").unwrap().as_deref(),
            Some("agent-graph"),
            "the id is frozen into the graph"
        );
        assert!(
            store.sync_get("setup_e2e_check").unwrap().is_none(),
            "an empty target proves no key, so no marker is frozen"
        );
    }

    /// §12.6 / §15.6: a typed passphrase that differs from the saved one is refused; no
    /// silent re-key, no file written.
    #[test]
    fn setup_refuses_a_second_e2e_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("Agent.utopia");
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let config = dir.path().join("config");
        let secrets = MemorySecrets::default();
        let mut options = options_for(&graph, Mode::Folder);
        options.folder = Some(shared.clone());
        let preferences = preferences_for(&options, &Preferences::default(), true).unwrap();
        let discover = |_: &str| None;
        let original = Typed {
            token: String::new(),
            passphrase: "the-original-key".into(),
        };
        save_and_apply(
            &graph,
            &config,
            Save {
                store: Store::open(&graph).unwrap(),
                preferences: preferences.clone(),
                requested_id: None,
                typed: &original,
                secrets: &secrets,
                env: &Environment::default(),
                discover: &discover,
            },
        )
        .unwrap();
        let other = Typed {
            token: String::new(),
            passphrase: "a-different-key".into(),
        };
        let error = save_and_apply(
            &graph,
            &config,
            Save {
                store: Store::open(&graph).unwrap(),
                preferences,
                requested_id: None,
                typed: &other,
                secrets: &secrets,
                env: &Environment::default(),
                discover: &discover,
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("already has an E2E passphrase"),
            "{error}"
        );
        assert!(
            !error.to_string().contains("a-different-key"),
            "the error quotes the secret: {error}"
        );
        assert_eq!(
            secrets.get(PASSPHRASE_SERVICE, "Agent").unwrap().as_deref(),
            Some("the-original-key"),
            "the saved key is unchanged"
        );
    }

    /// §15.6: environment credentials are used without reading or writing the Keychain, so
    /// a script (or the process tests) can run both setup and sync with no stored item.
    #[test]
    fn environment_credentials_do_not_need_the_keychain() {
        let env = Environment {
            token: Some("env-token".into()),
            passphrase: Some("env-passphrase".into()),
            names: vec!["UTOPIA_SYNC_TOKEN", "UTOPIA_SYNC_PASSPHRASE"],
            ..Default::default()
        };
        assert_eq!(
            credentials(&env, &EmptyConfig, "Agent").unwrap(),
            (Some("env-token".into()), Some("env-passphrase".into()))
        );
        let typed = Typed {
            token: String::new(),
            passphrase: String::new(),
        };
        assert_eq!(
            stored_credentials(&EmptyConfig, "Agent", &typed, &env).unwrap(),
            (None, None),
            "both fields came from the environment"
        );
    }

    /// §12.5: a graph id is frozen once it names synced data; a different `--graph-id` is
    /// refused instead of publishing into another graph.
    #[test]
    fn a_graph_id_that_names_data_is_frozen() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("Agent.utopia");
        let store = Store::open(&graph).unwrap();
        assert_eq!(
            setup_graph_id(Some("first"), &store, &graph).unwrap(),
            "first"
        );
        store.sync_set("setup_graph", "first").unwrap();
        assert_eq!(setup_graph_id(None, &store, &graph).unwrap(), "first");
        let error = setup_graph_id(Some("second"), &store, &graph).unwrap_err();
        assert!(error.to_string().contains("frozen"), "{error}");
        let fresh_graph = dir.path().join("Fresh.utopia");
        let fresh = Store::open(&fresh_graph).unwrap();
        assert!(
            setup_graph_id(Some("not a graph id"), &fresh, &fresh_graph).is_err(),
            "an id outside the URL alphabet is refused before it is frozen"
        );
    }

    /// A command's target must name it; config keeps the fields it does not name, exactly
    /// as the prefilled fields in Settings do.
    #[test]
    fn preferences_name_their_target_and_config_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("Agent.utopia");
        let mut relay = options_for(&graph, Mode::Relay);
        assert!(
            preferences_for(&relay, &Preferences::default(), true).is_err(),
            "relay setup needs a URL"
        );
        relay.url = Some("https://relay.example".into());
        let saved = preferences_for(&relay, &Preferences::default(), true).unwrap();
        assert_eq!(saved.url, "https://relay.example");
        // config --mode folder keeps the URL it does not name? No: folder clears it, like
        // the window does, and names its own folder.
        let mut folder = options_for(&graph, Mode::Folder);
        assert!(preferences_for(&folder, &saved, false).is_err());
        folder.folder = Some(dir.path().to_path_buf());
        let changed = preferences_for(&folder, &saved, false).unwrap();
        assert_eq!(changed.mode, Mode::Folder);
        assert!(changed.url.is_empty());
        // The window's Off is a fresh default, not the old target with the mode changed.
        let off = Options {
            off: true,
            ..Default::default()
        };
        assert_eq!(
            preferences_for(&off, &saved, false).unwrap(),
            Preferences::default()
        );
        assert!(
            preferences_for(&off, &saved, true).is_err(),
            "setup names a transport; config --off is the way to turn sync off"
        );
    }
    /// `utopia sync launchd-plist` names absolute paths and the continuous command, and
    /// points its logs at the documented directory. The process test checks it installs
    /// nothing.
    #[test]
    fn launchd_plist_is_an_absolute_continuous_agent() {
        let plist = launchd_plist_text(
            Path::new("/Applications/Utopia.app/Contents/MacOS/utopia"),
            Path::new("/Users/me/Documents/Utopia/Notes & Ideas.utopia"),
            Path::new("/Users/me"),
        );
        for expected in [
            "<string>/Applications/Utopia.app/Contents/MacOS/utopia</string>",
            "<string>--continuous</string>",
            "<string>/Users/me/Documents/Utopia/Notes &amp; Ideas.utopia</string>",
            "<key>KeepAlive</key>",
            "<key>RunAtLoad</key>",
            "/Users/me/Library/Logs/Utopia/sync-Notes &amp; Ideas.out.log",
        ] {
            assert!(plist.contains(expected), "missing {expected:?}:\n{plist}");
        }
        assert!(plist.contains("local.utopia.sync.Notes---Ideas"), "{plist}");
    }
}
