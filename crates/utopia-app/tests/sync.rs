//! `utopia sync` (§12), end to end: the real binary against a scratch relay, two graphs,
//! and scratch `UTOPIA_CONFIG_DIR` / `HOME` directories (the `Scratch` pattern of
//! `owner_lock.rs` and `mcp.rs`).
//!
//! No test touches a real Keychain item (§15.6): every child gets `UTOPIA_SYNC_TOKEN` and
//! `UTOPIA_SYNC_PASSPHRASE` in its environment, and `utopia sync` reads or writes the
//! Keychain only for a credential the environment did not provide. The relay is the real
//! `utopia-syncd` router served in-process on `127.0.0.1`, from the crate the app already
//! depends on.
//!
//! Every child names its graph with `--graph` (or `UTOPIA_GRAPH` for `capture`), so nothing
//! can fall back to the user's own graph or config directory.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use utopia_store::{OwnerLock, Store};
use utopia_sync::setup::{Mode, Preferences};

const TOKEN: &str = "headless-relay-token-314159";
const PASSPHRASE: &str = "headless-e2e-passphrase-271828";
const GRAPH_ID: &str = "headless-graph";
const ALREADY_OPEN: &str = "This graph is already open in Utopia";

struct Scratch {
    /// Served before the graphs so it stops before the temp directory is removed.
    _relay: utopia_syncd::lan::LanRelay,
    url: String,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        // Inside `target/`, like every scratch process in this repository, and unique per
        // test so two tests can run at once.
        let dir = tempfile::Builder::new()
            .prefix("utopia-headless-sync-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let relay = utopia_syncd::lan::serve_lan(
            &root.join("relay.db"),
            "127.0.0.1:0",
            GRAPH_ID,
            utopia_syncd::Config {
                tokens: vec![TOKEN.into()],
                ..Default::default()
            },
            false,
        )
        .unwrap();
        let url = format!("http://{}", relay.addr);
        Self {
            _relay: relay,
            url,
            root,
            _dir: dir,
        }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    fn graph(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.utopia"))
    }

    /// The per-graph settings file (`<config>/sync/<graph key>.json`).
    fn settings_file(&self, graph: &Path) -> PathBuf {
        self.config()
            .join("sync")
            .join(format!("{}.json", utopia_store::graph_key(graph)))
    }

    /// The binary, confined to this scratch directory, with credentials from the
    /// environment (so no Keychain item is read or written).
    fn utopia(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_utopia"));
        cmd.env("UTOPIA_CONFIG_DIR", self.config())
            .env("HOME", self.root.join("home"))
            .env("UTOPIA_SYNC_TOKEN", TOKEN)
            .env("UTOPIA_SYNC_PASSPHRASE", PASSPHRASE)
            .env_remove("UTOPIA_GRAPH")
            .env_remove("UTOPIA_API")
            .env_remove("UTOPIA_DB_PASSPHRASE")
            .env_remove("UTOPIA_SHOT_SCRIPT")
            .env_remove("UTOPIA_SYNC_URL")
            .env_remove("UTOPIA_SYNC_FOLDER")
            .env_remove("UTOPIA_SYNC_GRAPH")
            .env_remove("UTOPIA_LAN_SERVE")
            .env_remove("UTOPIA_SYNC_KEYCHAIN_PREFIX")
            .stdin(std::process::Stdio::null());
        cmd
    }

    fn setup(&self, graph: &Path, extra: &[&str]) -> Output {
        self.utopia()
            .args(["sync", "setup", "--graph"])
            .arg(graph)
            .args([
                "--mode",
                "relay",
                "--url",
                self.url.as_str(),
                "--graph-id",
                GRAPH_ID,
            ])
            .args(extra)
            .output()
            .unwrap()
    }

    fn sync(&self, graph: &Path) -> Output {
        self.utopia()
            .args(["sync", "--graph"])
            .arg(graph)
            .output()
            .unwrap()
    }

    fn sync_json(&self, graph: &Path) -> (Output, Value) {
        let out = self
            .utopia()
            .args(["sync", "--graph"])
            .arg(graph)
            .arg("--json")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "sync --json must print one object (stderr: {}): {stdout} ({e})",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out, value)
    }

    fn status_json(&self, graph: &Path) -> (Output, Value) {
        let out = self
            .utopia()
            .args(["sync", "status", "--graph"])
            .arg(graph)
            .arg("--json")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "sync status --json must print one object (stderr: {}): {stdout} ({e})",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out, value)
    }

    /// The settings the app's loader reads back.
    fn preferences(&self, graph: &Path) -> (Preferences, Option<String>) {
        Preferences::load(&self.settings_file(graph))
    }

    /// Seed a page with one block, the way a user's edit (or `utopia capture`) would.
    fn seed(&self, graph: &Path, page: &str, text: &str) {
        let mut store = Store::open(graph).unwrap();
        let id = store.create_page(page).unwrap();
        let block = store.ensure_first_block(&id).unwrap();
        store.set_block_text(&id, &block, text, None, None).unwrap();
        store.commit_block(&id, &block).unwrap();
    }
}

fn hits(graph: &Path, word: &str) -> usize {
    Store::open(graph)
        .unwrap()
        .search_blocks(word, 10)
        .unwrap()
        .len()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// §15.6: none of the run's output or its settings file may carry a credential.
fn assert_no_secret(out: &Output, secrets: &[&str]) {
    let all = format!("{}\n{}", stdout(out), stderr(out));
    for secret in secrets {
        assert!(!all.contains(secret), "output carries a secret: {all}");
    }
}

#[test]
fn headless_sync_moves_blocks_both_ways_and_a_quiet_run_moves_nothing() {
    let s = Scratch::new();
    let a = s.graph("a");
    let b = s.graph("b");
    s.seed(&a, "Alpha", "zebrasync-a");

    for graph in [&a, &b] {
        let out = s.setup(graph, &[]);
        assert!(
            out.status.success(),
            "setup {}: {}",
            graph.display(),
            stderr(&out)
        );
        assert!(stdout(&out).contains("Sync is set up"), "{}", stdout(&out));
        assert_no_secret(&out, &[TOKEN, PASSPHRASE]);
    }
    // The missing graph was created by setup, so the first sync can pull everything.
    assert!(a.exists() && b.exists(), "setup created the empty graph");
    let (preferences, notice) = s.preferences(&b);
    assert_eq!(notice, None);
    assert_eq!(preferences.mode, Mode::Relay);
    assert_eq!(preferences.url, s.url);
    assert_eq!(
        Store::open(&b).unwrap().sync_get("setup_graph").unwrap(),
        Some(GRAPH_ID.to_string()),
        "the graph id is frozen into the graph"
    );
    // The settings file is never a place for credentials.
    let raw = std::fs::read_to_string(s.settings_file(&b)).unwrap();
    for secret in [TOKEN, PASSPHRASE] {
        assert!(!raw.contains(secret), "settings carry a secret: {raw}");
    }

    // A → relay → B.
    let out = s.sync(&a);
    assert!(out.status.success(), "sync a: {}", stderr(&out));
    assert_no_secret(&out, &[TOKEN, PASSPHRASE]);
    let out = s.sync(&b);
    assert!(out.status.success(), "sync b: {}", stderr(&out));
    assert_eq!(hits(&b, "zebrasync-a"), 1, "B pulled A's block");

    // B → relay → A.
    s.seed(&b, "Beta", "zebrasync-b");
    assert!(s.sync(&b).status.success());
    assert!(s.sync(&a).status.success());
    assert_eq!(hits(&a, "zebrasync-b"), 1, "the reverse direction works");

    // Nothing changed since: the run pushes, pulls and leaves nothing queued.
    for graph in [&a, &b] {
        let (out, report) = s.sync_json(graph);
        assert!(out.status.success(), "quiet sync: {}", stderr(&out));
        assert_eq!(report["ok"], true, "{report}");
        assert_eq!(report["pushed"], 0, "{report}");
        assert_eq!(report["pulled"], 0, "{report}");
        assert_eq!(report["pending"], 0, "{report}");
        assert_eq!(report["errors"], serde_json::json!([]), "{report}");
        assert_eq!(report["graph_id"], GRAPH_ID, "{report}");
        assert_no_secret(&out, &[TOKEN, PASSPHRASE]);
    }
    assert!(Store::open(&a).unwrap().doctor().unwrap().is_ok());
    assert!(Store::open(&b).unwrap().doctor().unwrap().is_ok());
}

#[test]
fn headless_status_reports_mode_target_secrets_and_the_owner() {
    let s = Scratch::new();
    let a = s.graph("a");
    s.seed(&a, "Alpha", "zebrastatus");
    assert!(s.setup(&a, &[]).status.success());

    let (out, status) = s.status_json(&a);
    assert!(out.status.success(), "status: {}", stderr(&out));
    assert_no_secret(&out, &[TOKEN, PASSPHRASE]);
    assert_eq!(status["mode"], "relay", "{status}");
    assert_eq!(status["target"], s.url, "{status}");
    assert_eq!(status["graph_id"], GRAPH_ID, "{status}");
    assert_eq!(status["secrets"]["token"], true, "{status}");
    assert_eq!(status["secrets"]["passphrase"], true, "{status}");
    assert_eq!(status["environment_override"], true, "{status}");
    assert_eq!(status["last_synced_at"], Value::Null, "{status}");
    assert!(
        status["pending"]
            .as_u64()
            .is_some_and(|pending| pending > 0),
        "the seed's edits are queued for the relay: {status}"
    );
    assert_eq!(status["owned"], false, "{status}");
    assert_eq!(status["notice"], Value::Null, "{status}");

    // Status only looks at the lock: the open app still owns the graph.
    let held = OwnerLock::try_acquire(&a).unwrap().expect("free lock");
    let (out, status) = s.status_json(&a);
    assert!(out.status.success(), "status while owned: {}", stderr(&out));
    assert_eq!(status["owned"], true, "{status}");
    drop(held);

    assert!(s.sync(&a).status.success());
    let (_, status) = s.status_json(&a);
    assert!(
        status["last_synced_at"].as_i64().is_some_and(|at| at > 0),
        "a successful sync is recorded: {status}"
    );
    assert_eq!(status["pending"], 0, "{status}");
}

#[test]
fn headless_config_turns_sync_off_and_a_broken_settings_file_means_off() {
    let s = Scratch::new();
    let a = s.graph("a");
    s.seed(&a, "Alpha", "zebraconfig");
    assert!(s.setup(&a, &[]).status.success());

    let out = s
        .utopia()
        .args(["sync", "config", "--graph"])
        .arg(&a)
        .arg("--off")
        .output()
        .unwrap();
    assert!(out.status.success(), "config --off: {}", stderr(&out));
    assert_eq!(
        s.preferences(&a),
        (Preferences::default(), None),
        "turning sync off is the window's fresh default"
    );
    let (out, report) = s.sync_json(&a);
    assert_eq!(out.status.code(), Some(2), "Sync is Off has its own code");
    assert_eq!(report["mode"], "off", "{report}");
    assert_eq!(report["ok"], false, "{report}");
    assert!(
        report["notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("Off")),
        "{report}"
    );

    // Rule 11: a malformed settings file is Off with a notice, never a guess or a panic.
    let file = s.settings_file(&a);
    std::fs::write(&file, "{\"version\":1,\"mode\":\"future\"}").unwrap();
    let (out, report) = s.sync_json(&a);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(report["mode"], "off", "{report}");
    assert!(
        report["notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("malformed")),
        "{report}"
    );
    let (out, status) = s.status_json(&a);
    assert!(
        out.status.success(),
        "status reads a broken file: {}",
        stderr(&out)
    );
    assert_eq!(status["mode"], "off", "{status}");
    assert!(
        status["notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("malformed")),
        "{status}"
    );
    assert_eq!(status["target"], Value::Null, "{status}");
}

#[test]
fn headless_sync_refuses_a_wrong_passphrase_on_an_encrypted_graph() {
    let s = Scratch::new();
    let a = s.graph("a");
    s.seed(&a, "Alpha", "zebrapassphrase");
    assert!(s.setup(&a, &[]).status.success());
    // The first run publishes into an empty target; the second reads its own payloads
    // back, which is what proves the key and freezes the marker.
    assert!(s.sync(&a).status.success());
    assert!(s.sync(&a).status.success());
    assert!(
        Store::open(&a)
            .unwrap()
            .sync_get("setup_e2e_check")
            .unwrap()
            .is_some(),
        "a proven key is frozen into the graph"
    );
    let before = Store::open(&a).unwrap().outbox_count().unwrap();

    let wrong = "a-different-passphrase-161803";
    let out = s
        .utopia()
        .args(["sync", "--graph"])
        .arg(&a)
        .env("UTOPIA_SYNC_PASSPHRASE", wrong)
        .output()
        .unwrap();
    assert!(!out.status.success(), "a wrong passphrase must fail");
    assert!(
        stderr(&out).contains("original passphrase"),
        "the message names the rule, not the key: {}",
        stderr(&out)
    );
    assert_no_secret(&out, &[TOKEN, PASSPHRASE, wrong]);
    assert_eq!(hits(&a, "zebrapassphrase"), 1, "the graph is untouched");
    assert_eq!(
        Store::open(&a).unwrap().outbox_count().unwrap(),
        before,
        "nothing was queued by the refused run"
    );
}

#[test]
fn headless_sync_stops_while_another_process_owns_the_graph() {
    let s = Scratch::new();
    let a = s.graph("a");
    s.seed(&a, "Alpha", "zebraowned");
    assert!(s.setup(&a, &[]).status.success());
    let before = Store::open(&a).unwrap().outbox_count().unwrap();

    let held = OwnerLock::try_acquire(&a).unwrap().expect("free lock");
    let out = s.sync(&a);
    assert_eq!(
        out.status.code(),
        Some(3),
        "another owner has its own exit code: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains(ALREADY_OPEN), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("already syncs"),
        "the message says the owner already syncs it: {}",
        stderr(&out)
    );
    assert_eq!(hits(&a, "zebraowned"), 1, "the graph is untouched");
    assert_eq!(
        Store::open(&a).unwrap().outbox_count().unwrap(),
        before,
        "nothing was pushed behind the owner's back"
    );
    drop(held);

    let out = s.sync(&a);
    assert!(
        out.status.success(),
        "after the owner releases: {}",
        stderr(&out)
    );
}

#[test]
fn headless_setup_refuses_to_save_while_the_environment_overrides_the_transport() {
    let s = Scratch::new();
    let a = s.graph("a");
    let out = s
        .utopia()
        .args(["sync", "setup", "--graph"])
        .arg(&a)
        .args([
            "--mode",
            "relay",
            "--url",
            s.url.as_str(),
            "--graph-id",
            GRAPH_ID,
        ])
        .env("UTOPIA_SYNC_URL", "https://elsewhere.example")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("UTOPIA_SYNC_URL"),
        "the refusal names the override: {}",
        stderr(&out)
    );
    assert!(
        !s.settings_file(&a).exists(),
        "no settings were saved for a run whose transport the environment owns"
    );
}

/// §15.6: an environment-derived target goes through the same validation as a saved one, so
/// a URL with credentials never reaches the transport, the report, the status output or the
/// graph's `setup_destination` record.
#[test]
fn headless_sync_refuses_an_environment_url_with_credentials() {
    let s = Scratch::new();
    let a = s.graph("a");
    s.seed(&a, "Alpha", "zebraenvurl");
    assert!(s.setup(&a, &[]).status.success());

    let secret = "password-secret-913";
    let bad = format!("http://user:{secret}@127.0.0.1:9");
    let destination = Store::open(&a)
        .unwrap()
        .sync_get("setup_destination")
        .unwrap();
    assert!(
        destination.is_some(),
        "setup recorded the valid destination first"
    );
    let out = s
        .utopia()
        .args(["sync", "--graph"])
        .arg(&a)
        .arg("--json")
        .env("UTOPIA_SYNC_URL", &bad)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an environment target with credentials must not sync"
    );
    assert_no_secret(&out, &[secret]);
    let store = Store::open(&a).unwrap();
    assert_eq!(
        store.sync_get("setup_destination").unwrap(),
        destination,
        "the invalid target replaced the graph's destination record"
    );

    let out = s
        .utopia()
        .args(["sync", "status", "--graph"])
        .arg(&a)
        .arg("--json")
        .env("UTOPIA_SYNC_URL", &bad)
        .output()
        .unwrap();
    assert!(out.status.success(), "status: {}", stderr(&out));
    assert_no_secret(&out, &[secret]);
    let status: Value = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(status["mode"], "off", "{status}");
    assert!(
        status["notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("credentials")),
        "{status}"
    );
}

/// One-shot `sync` must drain the attachment queue too: `sync_once` moves at most
/// `BLOBS_PER_ROUND` per pass, so a backlog needs more than its inner loop. Success means
/// the queue is empty, not that the last pass happened to move pages.
#[test]
fn headless_sync_uploads_every_queued_attachment() {
    let s = Scratch::new();
    let a = s.graph("a");
    s.seed(&a, "Alpha", "zebrablobs");
    let mut hashes = Vec::new();
    {
        let mut store = Store::open(&a).unwrap();
        for i in 0..120 {
            let bytes = format!("attachment-{i}-{}", "x".repeat(32));
            let attachment = store
                .add_blob(&format!("file-{i}.txt"), bytes.as_bytes())
                .unwrap();
            hashes.push(attachment.hash);
        }
    }
    assert!(s.setup(&a, &[]).status.success());
    let (out, report) = s.sync_json(&a);
    assert!(out.status.success(), "sync: {}", stderr(&out));
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(
        report["blobs_up"], 120,
        "every queued attachment was uploaded: {report}"
    );
    let store = Store::open(&a).unwrap();
    let left = hashes
        .iter()
        .filter(|hash| {
            store.attachment_state(hash).unwrap() != Some(utopia_store::BlobState::Uploaded)
        })
        .count();
    assert_eq!(left, 0, "{left} attachment(s) were still queued");
}

/// A configuration flag is not a shortcut for the configuration command. The one-shot and
/// `status` forms reject a flag they do not take before opening the graph, instead of
/// silently ignoring it (and, for `--off`, syncing anyway).
#[test]
fn headless_sync_rejects_options_its_form_does_not_take() {
    let s = Scratch::new();
    let a = s.graph("a");
    s.seed(&a, "Alpha", "zebraflags");
    assert!(s.setup(&a, &[]).status.success());
    let before = Store::open(&a).unwrap().outbox_count().unwrap();

    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["sync", "--off"], "--off"),
        (vec!["sync", "--graph-id", "other"], "--graph-id"),
        (vec!["sync", "--mode", "folder"], "--mode"),
        (vec!["sync", "status", "--off"], "--off"),
        (
            vec!["sync", "status", "--json", "--mode", "relay"],
            "--mode",
        ),
    ];
    for (args, flag) in cases {
        let out = s
            .utopia()
            .args(&args)
            .args(["--graph"])
            .arg(&a)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "`utopia {} --graph … {flag}` must be refused",
            args.join(" ")
        );
        assert!(
            stderr(&out).contains(flag),
            "the refusal names {flag}: {}",
            stderr(&out)
        );
    }

    let store = Store::open(&a).unwrap();
    assert_eq!(
        store.sync_get("synced_at").unwrap(),
        None,
        "a rejected command must not sync"
    );
    assert_eq!(
        store.outbox_count().unwrap(),
        before,
        "a rejected command must not push"
    );
    assert_eq!(
        store.sync_get("setup_graph").unwrap().as_deref(),
        Some(GRAPH_ID),
        "a rejected command must not change the graph id"
    );
}
