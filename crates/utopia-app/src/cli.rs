//! Headless commands: `utopia doctor <graph>` (§10.4, §15.2) and
//! `utopia export <graph> <dir>` (G4).

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use utopia_store::{OwnerLock, Store};

pub const USAGE: &str = "\
usage:
  utopia [--graph <file.utopia>]        open the desktop app
  utopia doctor <file.utopia> [--rebuild]  check invariants (and rebuild the index)
  utopia export <file.utopia> <dir>     write Markdown files, graph.json and graph.opml to <dir>
  utopia import-roam <file.utopia> <roam.json>   import a Roam JSON export
  utopia capture <text…>                 append a line to today's note (bind to any OS shortcut);
                                         when Utopia has the graph open, the open app writes it
  utopia serve-api <file.utopia> [addr]  run the local automation API without the app
  utopia mcp --graph <file.utopia> [--read-only]
                                         MCP server on stdio for an AI client (§13.8); the
                                         graph is necessary, and --read-only lowers the
                                         assistant access level for that client
  utopia mcp config <claude-code|claude-desktop|codex> [--graph <file.utopia>]
                                         print the setup entry for that AI client
  utopia encrypt <file.utopia>          encrypt the graph at rest (or change its passphrase)
  utopia decrypt <file.utopia>          remove encryption at rest
  utopia utopia://page/<title>          open a utopia:// link (page, journal, block, search, capture)

encrypted graphs: the passphrase comes from UTOPIA_DB_PASSPHRASE, the macOS keychain
(service \"utopia-db\", account = graph name), or a prompt. It is never stored in the graph.

automation API: UTOPIA_API=1 (or UTOPIA_API=127.0.0.1:<port>) starts it with the app, and so
does assistant access read or write in mcp/<graph key>.json in the config directory
({\"version\": 1, \"access\": \"off\" | \"read\" | \"write\", \"when_closed\": false}; a missing
or malformed file means off, and no variable or flag raises it). The URL and a per-launch
bearer token are written to api/<graph key>.json in the config directory (graph key =
SHA-256 of the canonical graph path). POST /v1/op runs one op at the access level; the other
routes do not follow it.

sync (optional): set UTOPIA_SYNC_URL=<relay origin>, and optionally UTOPIA_SYNC_GRAPH=<id>
and UTOPIA_SYNC_TOKEN=<token> (or a macOS keychain item, service \"utopia-sync\").";

/// What a process that cannot become the graph's owner says (MC-4), in the window and on
/// stderr.
pub const ALREADY_OPEN: &str = "This graph is already open in Utopia";

/// How long the app and the write commands try for the owner lock (docs/mcp-design.md
/// §2.2): a headless call holds it for a short time.
pub const OWNER_WAIT: Duration = Duration::from_secs(2);

/// Become the graph's one writer (MC-4), or fail with [`ALREADY_OPEN`] after [`OWNER_WAIT`].
pub fn take_owner(graph: &Path) -> Result<OwnerLock> {
    match OwnerLock::acquire_within(graph, OWNER_WAIT)
        .with_context(|| format!("locking {}", graph.display()))?
    {
        Some(lock) => Ok(lock),
        None => bail!("{ALREADY_OPEN}: {}", graph.display()),
    }
}

/// A store opened by its owner. The fields drop in order, so the store has closed before
/// the lock is released.
pub struct Owned {
    pub store: Store,
    _owner: OwnerLock,
}

/// Open a graph for writes (MC-4): the graph must exist, then the owner lock, then the store.
/// Nothing is written when the lock stays held.
pub fn open_owned(graph: &Path) -> Result<Owned> {
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let owner = take_owner(graph)?;
    Ok(Owned {
        store: open_store(graph)?,
        _owner: owner,
    })
}

/// Read a passphrase from the terminal without echoing it.
fn prompt_secret(prompt: &str) -> Result<String> {
    use std::io::Write;
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let tty = std::fs::File::open("/dev/tty").ok();
    let stty = |arg: &str| {
        if let Some(t) = &tty {
            let _ = std::process::Command::new("/bin/stty")
                .arg(arg)
                .stdin(t.try_clone().expect("tty"))
                .status();
        }
    };
    stty("-echo");
    let mut line = String::new();
    let r = std::io::stdin().read_line(&mut line);
    stty("echo");
    eprintln!();
    r?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Open a graph for a command, unlocking it if it is encrypted at rest.
pub fn open_store(graph: &Path) -> Result<Store> {
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    if !utopia_store::is_encrypted(graph) {
        return Store::open(graph).with_context(|| format!("opening {}", graph.display()));
    }
    let pass = match crate::paths::db_passphrase(graph) {
        Some(p) => p,
        None => prompt_secret("passphrase: ")?,
    };
    Store::open_with_key(graph, Some(&pass)).with_context(|| format!("opening {}", graph.display()))
}

/// `utopia encrypt`: encrypt a plain graph, or change an encrypted graph's passphrase.
pub fn encrypt(graph: &Path) -> Result<()> {
    if !utopia_store::encryption_available() {
        bail!("this build has no encryption at rest (SQLCipher)");
    }
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    let current = if utopia_store::is_encrypted(graph) {
        Some(match crate::paths::db_passphrase(graph) {
            Some(p) => p,
            None => prompt_secret("current passphrase: ")?,
        })
    } else {
        None
    };
    let new = match std::env::var("UTOPIA_DB_NEW_PASSPHRASE")
        .ok()
        .filter(|v| !v.is_empty())
    {
        Some(p) => p,
        None => {
            let a = prompt_secret("new passphrase: ")?;
            let b = prompt_secret("again: ")?;
            if a != b {
                bail!("the passphrases differ");
            }
            a
        }
    };
    utopia_store::set_encryption(graph, current.as_deref(), Some(&new))?;
    println!(
        "{} is encrypted at rest. Keep the passphrase safe: without it the graph cannot be read.",
        graph.display()
    );
    Ok(())
}

/// `utopia decrypt`: back to a plain SQLite file.
pub fn decrypt(graph: &Path) -> Result<()> {
    if !utopia_store::is_encrypted(graph) {
        bail!("{} is not encrypted", graph.display());
    }
    let pass = match crate::paths::db_passphrase(graph) {
        Some(p) => p,
        None => prompt_secret("passphrase: ")?,
    };
    utopia_store::set_encryption(graph, Some(&pass), None)?;
    println!("{} is no longer encrypted", graph.display());
    Ok(())
}

/// `doctor` only reads; `doctor --rebuild` rewrites the projections, so it must own the
/// graph (MC-4).
pub fn doctor(graph: &Path, rebuild: bool) -> Result<bool> {
    let report = if rebuild {
        let mut owned = open_owned(graph)?;
        owned.store.rebuild_index()?;
        println!("index rebuilt from page shards");
        owned.store.doctor()?
    } else {
        open_store(graph)?.doctor()?
    };
    println!(
        "{} live pages, {} live blocks, {} references",
        report.pages, report.blocks, report.refs
    );
    if report.is_ok() {
        println!("ok: all invariants hold");
    } else {
        for p in &report.problems {
            println!("problem: {p}");
        }
    }
    Ok(report.is_ok())
}

pub fn export(graph: &Path, dir: &Path) -> Result<()> {
    let mut store = open_store(graph)?;
    let n = store.export_markdown(dir)?;
    store.export_json(&dir.join("graph.json"))?;
    store.export_opml(&dir.join("graph.opml"))?;
    println!("exported {n} pages to {}", dir.display());
    Ok(())
}

pub fn import_roam(graph: &Path, file: &Path) -> Result<()> {
    let json =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    // Importing may create the graph, so the lock is taken before the file exists (MC-4).
    let _owner = take_owner(graph)?;
    let mut store = if graph.exists() {
        open_store(graph)?
    } else {
        Store::open(graph).with_context(|| format!("opening {}", graph.display()))?
    };
    let r = store.import_roam_json(&json)?;
    println!("imported {} pages, {} blocks", r.pages, r.blocks);
    Ok(())
}

/// What became of a capture sent to the graph's owner.
enum Sent {
    /// The owner wrote it.
    Done,
    /// It was refused before it reached the owner's queue, so nothing was written: there is
    /// no discovery file, nothing listens at its address, or the token is not the owner's
    /// (a stale file, possibly another server on a reused port).
    NotDelivered,
}

/// D10 A: send a capture to the process that owns the graph, through its loopback API.
/// An answer that leaves the outcome open (the connection broke after the request, or the
/// owner answered with an error) stops here with the owner's own text: the capture is
/// never tried a second time, so it cannot be written twice.
fn send_to_owner(config: &Path, graph: &Path, text: &str) -> Result<Sent> {
    use utopia_api::{Client, ClientError};
    let file = utopia_api::discovery_file(config, graph);
    let Ok(client) = Client::from_discovery(&file) else {
        return Ok(Sent::NotDelivered);
    };
    match client.capture(text) {
        Ok(_) => Ok(Sent::Done),
        Err(ClientError::NotListening(_)) => Ok(Sent::NotDelivered),
        // The token, Host and Origin checks run before the call is queued.
        Err(ClientError::Status {
            status: 401 | 403, ..
        }) => Ok(Sent::NotDelivered),
        Err(ClientError::Status { status, message }) => {
            bail!("the open Utopia app did not take the capture ({status}): {message}")
        }
        Err(ClientError::Transport(e)) => bail!(
            "the open Utopia app did not answer ({e}); the capture may be in today's note, \
             look before you try again"
        ),
    }
}

/// `utopia capture` (MC-4, D10 A). When the graph is free it is written here. When another
/// process owns it, the text goes to that owner's API (`POST /v1/capture`, found through
/// `<config>/api/<graph_key>.json`). When that fails before anything was sent, it waits for
/// the lock as the other write commands do, and stops with [`ALREADY_OPEN`].
pub fn capture(graph: &Path, config: &Path, text: &str) -> Result<()> {
    let text = text.trim();
    if text.is_empty() {
        bail!("nothing to capture");
    }
    if !graph.exists() {
        bail!("no graph at {}", graph.display());
    }
    // First without waiting, so a capture for the open app does not sit out the wait.
    let owner = match OwnerLock::try_acquire(graph)
        .with_context(|| format!("locking {}", graph.display()))?
    {
        Some(lock) => lock,
        None => match send_to_owner(config, graph, text)? {
            Sent::Done => {
                println!("captured to today's note in the open Utopia app");
                return Ok(());
            }
            Sent::NotDelivered => take_owner(graph)?,
        },
    };
    let mut owned = Owned {
        store: open_store(graph)?,
        _owner: owner,
    };
    let store = &mut owned.store;
    let (page, _) = store.capture(text)?;
    let title = store.page_row(&page)?.map(|r| r.title).unwrap_or_default();
    println!("captured to {title}");
    Ok(())
}

/// `utopia serve-api`: the automation API over a graph with no window, for scripts and
/// servers. Runs until killed, and owns the graph for all of that time (MC-4). The level of
/// `/v1/op` comes from the graph's access file (MC-1), and the discovery file is the graph's
/// own, `<config>/api/<graph_key>.json`.
pub fn serve_api(graph: &Path, addr: &str, config: &Path) -> Result<()> {
    let mut owned = open_owned(graph)?;
    let level = crate::api_launch::load_access(config, graph).level;
    let server = crate::api_launch::serve(addr, level, config, graph)?;
    println!(
        "automation API on {} (assistant access: {level:?}; token in {})",
        server.url(),
        utopia_api::discovery_file(config, graph).display()
    );
    loop {
        if let Some(call) = server.recv_timeout(std::time::Duration::from_secs(1)) {
            call.run(&mut owned.store);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_and_export_on_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        {
            let mut s = Store::open(&graph).unwrap();
            let (p, b) = s.open_daily(s.today()).unwrap();
            s.set_block_text(&p, &b, "hello [[World]]", None, None)
                .unwrap();
            s.commit_block(&p, &b).unwrap();
        }
        assert!(doctor(&graph, false).unwrap());
        assert!(doctor(&graph, true).unwrap());
        let out = dir.path().join("out");
        export(&graph, &out).unwrap();
        assert!(out.join("graph.json").exists());
        assert!(out.join("graph.opml").exists());
        capture(&graph, &dir.path().join("config"), "from the command line").unwrap();
        let roam = dir.path().join("roam.json");
        std::fs::write(
            &roam,
            r#"[{"title":"Imported","children":[{"string":"hi","uid":"u1"}]}]"#,
        )
        .unwrap();
        import_roam(&graph, &roam).unwrap();
        let mut s = Store::open(&graph).unwrap();
        assert!(s.find_page("Imported").unwrap().is_some());
        assert!(s.doctor().unwrap().is_ok());
        assert!(out.join("World.md").exists());
        assert!(doctor(&dir.path().join("missing.utopia"), false).is_err());
    }
}
