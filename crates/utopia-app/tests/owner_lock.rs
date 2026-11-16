//! MC-4 (PRD §13.8): one process owns a graph for writes. These run the real `utopia`
//! binary against a temporary graph.
//!
//! Every child gets `UTOPIA_GRAPH`, `UTOPIA_CONFIG_DIR` and `HOME` pointing into the temp
//! directory: `capture` otherwise falls back to the last graph in the real config directory
//! or the only graph in `~/Documents/Utopia`, and would write into the user's own notes.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use utopia_store::{OwnerLock, Store};

const ALREADY_OPEN: &str = "This graph is already open in Utopia";

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    graph: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let graph = root.join("g.utopia");
        drop(Store::open(&graph).unwrap());
        Self {
            _dir: dir,
            root,
            graph,
        }
    }

    /// The binary, confined to this scratch directory.
    fn utopia(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_utopia"));
        cmd.env("UTOPIA_GRAPH", &self.graph)
            .env("UTOPIA_CONFIG_DIR", self.root.join("config"))
            .env("HOME", self.root.join("home"))
            .env_remove("UTOPIA_API")
            .env_remove("UTOPIA_SHOT_SCRIPT")
            .env_remove("UTOPIA_SYNC_URL")
            .env_remove("UTOPIA_SYNC_FOLDER")
            .env_remove("UTOPIA_LAN_SERVE")
            .stdin(Stdio::null());
        cmd
    }
}

fn hits(graph: &Path, word: &str) -> usize {
    Store::open(graph)
        .unwrap()
        .search_blocks(word, 10)
        .unwrap()
        .len()
}

#[test]
fn mc_4_capture_stops_while_another_process_owns_the_graph() {
    let s = Scratch::new();
    let held = OwnerLock::try_acquire(&s.graph)
        .unwrap()
        .expect("free lock");

    let started = Instant::now();
    let out = s
        .utopia()
        .args(["capture", "zebracapture refused"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "capture must fail: {stderr}");
    assert!(stderr.contains(ALREADY_OPEN), "stderr: {stderr}");
    assert!(
        started.elapsed() >= Duration::from_millis(1900),
        "it tried for the 2 s retry first"
    );
    assert_eq!(hits(&s.graph, "zebracapture"), 0, "nothing was written");

    drop(held);
    let out = s
        .utopia()
        .args(["capture", "zebracapture accepted"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "capture after release: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(hits(&s.graph, "zebracapture"), 1);
    assert!(Store::open(&s.graph).unwrap().doctor().unwrap().is_ok());
}

#[test]
fn mc_4_import_roam_and_doctor_rebuild_stop_while_owned() {
    let s = Scratch::new();
    let roam = s.root.join("roam.json");
    std::fs::write(
        &roam,
        r#"[{"title":"Imported","children":[{"string":"zebraimport","uid":"u1"}]}]"#,
    )
    .unwrap();
    // A graph that does not exist yet: import-roam would create it.
    let fresh = s.root.join("fresh.utopia");
    let _held = OwnerLock::try_acquire(&s.graph)
        .unwrap()
        .expect("free lock");
    let _fresh = OwnerLock::try_acquire(&fresh).unwrap().expect("free lock");

    for args in [
        vec![
            "import-roam".to_string(),
            s.graph.display().to_string(),
            roam.display().to_string(),
        ],
        vec![
            "import-roam".to_string(),
            fresh.display().to_string(),
            roam.display().to_string(),
        ],
        vec![
            "doctor".to_string(),
            s.graph.display().to_string(),
            "--rebuild".to_string(),
        ],
    ] {
        let out = s.utopia().args(&args).output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?} must fail: {stderr}");
        assert!(stderr.contains(ALREADY_OPEN), "{args:?} stderr: {stderr}");
    }
    assert_eq!(hits(&s.graph, "zebraimport"), 0, "nothing was imported");
    assert!(!fresh.exists(), "the graph was not created");

    // Reading needs no lock.
    let out = s
        .utopia()
        .args(["doctor", &s.graph.display().to_string()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "doctor reads while owned: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn mc_4_a_killed_owner_leaves_no_lock() {
    let s = Scratch::new();
    let mut child = s
        .utopia()
        .args(["serve-api", &s.graph.display().to_string(), "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // `serve-api` prints its address once it has the lock and the store.
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("automation API on"), "serve-api said: {line}");
    assert!(
        OwnerLock::try_acquire(&s.graph).unwrap().is_none(),
        "serve-api owns the graph"
    );

    // A second owner is refused while the first lives.
    let out = s
        .utopia()
        .args(["serve-api", &s.graph.display().to_string(), "127.0.0.1:0"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains(ALREADY_OPEN));

    // SIGKILL: no destructor runs, so only the kernel can release the lock.
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        OwnerLock::try_acquire(&s.graph).unwrap().is_some(),
        "a killed owner leaves no lock"
    );
}
