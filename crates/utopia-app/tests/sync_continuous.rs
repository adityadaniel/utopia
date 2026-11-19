//! `utopia sync --continuous`, end to end (§12, PR 2): a headless owner that keeps a graph
//! in sync and serves the automation API to `utopia mcp` and `utopia capture`.
//!
//! Every child runs with `--graph`, `UTOPIA_CONFIG_DIR` and `HOME` inside a scratch
//! directory under `target/` (the `Scratch` pattern of `sync.rs` and `owner_lock.rs`). The
//! relay is the real `utopia-syncd` served in-process on `127.0.0.1`; no test reads or
//! writes a Keychain item: the relay token comes from `UTOPIA_SYNC_TOKEN`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use utopia_api::access::Level;
use utopia_api::Access;
use utopia_store::Store;
use utopia_sync::setup::{Mode, Preferences};

const TOKEN: &str = "continuous-relay-token-314159";
const GRAPH_ID: &str = "headless-continuous";

struct Scratch {
    _relay: utopia_syncd::lan::LanRelay,
    url: String,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("utopia-sync-continuous-")
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

    fn settings_file(&self, graph: &Path) -> PathBuf {
        self.config()
            .join("sync")
            .join(format!("{}.json", utopia_store::graph_key(graph)))
    }

    fn utopia(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_utopia"));
        cmd.env("UTOPIA_CONFIG_DIR", self.config())
            .env("HOME", self.root.join("home"))
            .env("UTOPIA_SYNC_TOKEN", TOKEN)
            .env_remove("UTOPIA_GRAPH")
            .env_remove("UTOPIA_API")
            .env_remove("UTOPIA_DB_PASSPHRASE")
            .env_remove("UTOPIA_SHOT_SCRIPT")
            .env_remove("UTOPIA_SYNC_URL")
            .env_remove("UTOPIA_SYNC_FOLDER")
            .env_remove("UTOPIA_SYNC_GRAPH")
            .env_remove("UTOPIA_LAN_SERVE")
            .env_remove("UTOPIA_SYNC_KEYCHAIN_PREFIX")
            .env_remove("UTOPIA_SYNC_PASSPHRASE")
            .stdin(Stdio::null());
        cmd
    }

    fn setup(&self, graph: &Path) {
        self.setup_at(graph, &self.url);
    }

    fn setup_at(&self, graph: &Path, url: &str) {
        let out = self
            .utopia()
            .args(["sync", "setup", "--graph"])
            .arg(graph)
            .args(["--mode", "relay", "--url", url, "--graph-id", GRAPH_ID])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "setup {}: {}",
            graph.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        let (preferences, notice) = Preferences::load(&self.settings_file(graph));
        assert_eq!(notice, None);
        assert_eq!(preferences.mode, Mode::Relay);
    }

    fn seed(&self, graph: &Path, page: &str, text: &str) {
        let mut store = Store::open(graph).unwrap();
        let id = store.create_page(page).unwrap();
        let block = store.ensure_first_block(&id).unwrap();
        store.set_block_text(&id, &block, text, None, None).unwrap();
        store.commit_block(&id, &block).unwrap();
    }

    fn sync(&self, graph: &Path) -> std::process::Output {
        self.utopia()
            .args(["sync", "--graph"])
            .arg(graph)
            .output()
            .unwrap()
    }

    fn access(&self, graph: &Path, level: Level) {
        Access {
            level,
            when_closed: false,
        }
        .save(&utopia_api::access::access_file(&self.config(), graph))
        .unwrap();
    }

    fn discovery(&self, graph: &Path) -> PathBuf {
        utopia_api::discovery_file(&self.config(), graph)
    }

    /// Start the daemon and wait until its API wrote the discovery file: from then on the
    /// loop is running and a client can reach it.
    fn daemon(&self, graph: &Path) -> Daemon {
        let child = self
            .utopia()
            .args(["sync", "--continuous", "--graph"])
            .arg(graph)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut daemon = Daemon { child: Some(child) };
        daemon.wait_ready(&self.discovery(graph), graph);
        daemon
    }
}

struct Daemon {
    child: Option<Child>,
}

impl Daemon {
    fn wait_ready(&mut self, discovery: &Path, graph: &Path) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if discovery.is_file() {
                return;
            }
            if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
                let mut stderr = String::new();
                if let Some(err) = self.child.as_mut().unwrap().stderr.as_mut() {
                    let _ = err.read_to_string(&mut stderr);
                }
                panic!(
                    "the daemon for {} exited {status} before serving:\n{stderr}",
                    graph.display()
                );
            }
            assert!(
                Instant::now() < deadline,
                "the daemon for {} did not write its discovery file",
                graph.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// SIGTERM, then the process's exit status and everything it logged.
    fn stop(mut self) -> (ExitStatus, String) {
        let mut child = self.child.take().expect("running");
        let pid = child.id() as libc::pid_t;
        // SAFETY: the process is this test's child and still running.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
        let status = child.wait().unwrap();
        let mut stderr = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        (status, stderr)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn hits(graph: &Path, word: &str) -> usize {
    Store::open(graph)
        .unwrap()
        .search_blocks(word, 10)
        .unwrap()
        .len()
}

fn wait_for<F: FnMut() -> bool>(what: &str, mut ready: F) -> bool {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if ready() {
            return true;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A running `utopia mcp` with pipes, so a write goes through the Remote path (MC-4).
struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
}

impl Mcp {
    fn start(s: &Scratch, graph: &Path) -> Self {
        let mut cmd = s.utopia();
        cmd.args(["mcp", "--graph"]).arg(graph);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        Self {
            stdin: child.stdin.take(),
            out: BufReader::new(child.stdout.take().unwrap()),
            child,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("input open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.out.read_line(&mut line).unwrap() > 0, "mcp closed");
        serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("stdout carried {line:?}, not JSON-RPC: {e}"))
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The two `_meta` keys of the 2026-07-28 era (`docs/mcp-design.md` §3.2).
fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "continuous-test", "version": "1"},
    })
}

fn tool_call(id: u32, name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments, "_meta": modern_meta()},
    })
}

fn text_of(answer: &Value) -> (String, bool) {
    assert!(answer.get("error").is_none(), "expected a result: {answer}");
    (
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
        answer["result"]["isError"].as_bool().unwrap(),
    )
}

/// The daemon is the graph's only writer on B and keeps pulling: a write on A, pushed by a
/// one-shot `utopia sync` on A, reaches B's store within the realtime round.
#[test]
fn a_write_on_a_reaches_b_through_the_continuous_daemon() {
    let s = Scratch::new();
    let a = s.graph("a");
    let b = s.graph("b");
    s.seed(&a, "Alpha", "zebracontinuous-a");
    s.setup(&a);
    s.setup(&b);

    let daemon = s.daemon(&b);
    let out = s.sync(&a);
    assert!(
        out.status.success(),
        "sync a: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    wait_for("B to pull A's write", || hits(&b, "zebracontinuous-a") == 1);

    let (status, stderr) = daemon.stop();
    assert!(status.success(), "daemon exit: {status}\n{stderr}");
    assert!(stderr.contains("continuous sync stopped"), "{stderr}");
    // §12.3, §13.3: one poll loop, one realtime listener — never a second of either.
    assert_eq!(
        stderr.matches("continuous sync loop started").count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        stderr.matches("realtime listener started").count(),
        1,
        "{stderr}"
    );
    assert!(
        !stderr.contains(TOKEN),
        "the log carries the relay token: {stderr}"
    );
    assert!(Store::open(&b).unwrap().doctor().unwrap().is_ok());
}

/// An agent's `append_blocks` on B goes through the daemon's API (the daemon owns the graph)
/// and the next daemon round pushes it; A gets it on its own sync.
#[test]
fn an_mcp_write_on_b_goes_through_the_daemon_and_reaches_a() {
    let s = Scratch::new();
    let a = s.graph("a");
    let b = s.graph("b");
    s.seed(&a, "Alpha", "zebracontinuous-base");
    s.seed(&b, "Beta", "zebra-baseline-b");
    s.setup(&a);
    s.setup(&b);
    // A write-level access file makes the daemon answer `/v1/op`; `when_closed` is false,
    // so a result can only have come from the running owner (MC-4, MC-5).
    s.access(&b, Level::Write);
    let daemon = s.daemon(&b);

    let mut mcp = Mcp::start(&s, &b);
    mcp.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "continuous-test", "version": "1"},
        },
    }));
    let init = mcp.read();
    assert_eq!(init["id"], 1);
    mcp.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    mcp.send(tool_call(
        2,
        "append_blocks",
        json!({"page": "Beta", "markdown": "- zebracontinuous-mcp\n"}),
    ));
    let answer = mcp.read();
    let (text, is_error) = text_of(&answer);
    assert!(!is_error, "{text}");
    mcp.finish();
    assert_eq!(hits(&b, "zebracontinuous-mcp"), 1, "the owner wrote B");

    // The daemon nudges a round after the local write; A only has to ask.
    wait_for("A to pull the daemon's write", || {
        let out = s.sync(&a);
        assert!(
            out.status.success(),
            "sync a: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        hits(&a, "zebracontinuous-mcp") == 1
    });

    let (status, stderr) = daemon.stop();
    assert!(status.success(), "daemon exit: {status}\n{stderr}");
    assert!(Store::open(&a).unwrap().doctor().unwrap().is_ok());
    assert!(Store::open(&b).unwrap().doctor().unwrap().is_ok());
}

/// SIGTERM is a clean stop: the round in flight (if any) completes, the store flushes, the
/// discovery file goes, and the owner lock is free for the next process.
#[test]
fn sigterm_stops_the_daemon_and_releases_the_owner_lock() {
    let s = Scratch::new();
    let b = s.graph("b");
    s.seed(&b, "Beta", "zebracontinuous-term");
    s.setup(&b);
    let daemon = s.daemon(&b);
    let discovery = s.discovery(&b);
    assert!(discovery.is_file());

    let (status, stderr) = daemon.stop();
    assert!(status.success(), "daemon exit: {status}\n{stderr}");
    assert!(stderr.contains("continuous sync stopped"), "{stderr}");
    assert!(
        !discovery.exists(),
        "a clean stop removes the discovery file it wrote"
    );

    // The lock is free: a one-shot sync does not answer "already open" (exit 3).
    let out = s.sync(&b);
    assert!(
        out.status.success(),
        "after SIGTERM: {:?}\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(Store::open(&b).unwrap().doctor().unwrap().is_ok());
}

/// `utopia sync launchd-plist` only prints: absolute binary and graph paths, KeepAlive,
/// RunAtLoad, logs under `~/Library/Logs/Utopia`, and no file installed anywhere.
#[test]
fn launchd_plist_prints_an_agent_and_installs_nothing() {
    let s = Scratch::new();
    let b = s.graph("b");
    s.seed(&b, "Beta", "zebracontinuous-plist");
    let out = s
        .utopia()
        .args(["sync", "launchd-plist", "--graph"])
        .arg(&b)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "launchd-plist: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plist = String::from_utf8_lossy(&out.stdout);
    for expected in [
        "<key>Label</key>",
        "<key>KeepAlive</key>",
        "<key>RunAtLoad</key>",
        "<string>--continuous</string>",
        "<string>sync</string>",
        "Library/Logs/Utopia/",
        &b.canonicalize().unwrap().display().to_string(),
    ] {
        assert!(plist.contains(expected), "missing {expected:?}:\n{plist}");
    }
    assert!(plist.contains(env!("CARGO_BIN_EXE_utopia")), "{plist}");
    let agents = s.root.join("home/Library/LaunchAgents");
    assert!(
        !agents.exists() || std::fs::read_dir(&agents).unwrap().next().is_none(),
        "nothing was installed under {}",
        agents.display()
    );
}

/// `serve-api --sync` is the alias of `sync --continuous`, and while either owns the graph a
/// second continuous daemon gets the one-shot ownership answer (exit 3) instead of opening
/// a second writer.
#[test]
fn serve_api_sync_is_an_alias_and_a_second_daemon_exits_three() {
    let s = Scratch::new();
    let b = s.graph("b");
    s.seed(&b, "Beta", "zebracontinuous-alias");
    s.setup(&b);
    let child = s
        .utopia()
        .args(["serve-api"])
        .arg(&b)
        .arg("--sync")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut daemon = Daemon { child: Some(child) };
    daemon.wait_ready(&s.discovery(&b), &b);

    let out = s
        .utopia()
        .args(["sync", "--continuous", "--graph"])
        .arg(&b)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "a second daemon must not open the graph: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("This graph is already open in Utopia"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let (status, stderr) = daemon.stop();
    assert!(status.success(), "alias exit: {status}\n{stderr}");
    assert_eq!(
        stderr.matches("continuous sync loop started").count(),
        1,
        "the alias is the same one-loop daemon: {stderr}"
    );
}

/// A relay that answers manifests at once except one chosen manifest request, which it
/// holds open. Setup and the daemon's probe stay fast; the first round's exchange stalls.
struct SlowRelay {
    url: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl SlowRelay {
    /// `stalled` is the manifest request number (1 = setup's probe, 2 = the daemon's probe,
    /// 3 = its first round) that sleeps for `hold`; `started` is signalled when it begins.
    fn new(stalled: usize, hold: Duration, started: mpsc::Sender<()>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut manifests = 0usize;
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        let mut request = [0u8; 4096];
                        let mut read = Vec::new();
                        let mut socket = socket;
                        let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
                        loop {
                            match socket.read(&mut request) {
                                Ok(0) => break,
                                Ok(n) => {
                                    read.extend_from_slice(&request[..n]);
                                    if read.windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        let head = String::from_utf8_lossy(&read);
                        if head.contains("/manifest") {
                            manifests += 1;
                            let number = manifests;
                            let started = started.clone();
                            std::thread::spawn(move || {
                                if number == stalled {
                                    let _ = started.send(());
                                    std::thread::sleep(hold);
                                }
                                let body = format!(
                                    r#"{{"rev":0,"protocol_version":{},"pages":[]}}"#,
                                    utopia_sync::PROTOCOL_VERSION
                                );
                                let response = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                let _ = socket.write_all(response.as_bytes());
                            });
                        } else {
                            // The realtime listener's socket: no event stream here, so it
                            // reconnects at its own cadence without affecting the count.
                            let _ = socket.write_all(
                                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                                  Connection: close\r\n\r\n",
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            url,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for SlowRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// P1: the round's network exchange must not stop the only loop that drains the API. A
/// capture sent while the exchange is stalled runs at once; before the exchange moved to a
/// worker it sat in the queue until the relay answered (and would 504 after 30 s).
#[test]
fn api_calls_are_served_while_the_exchange_is_slow() {
    let s = Scratch::new();
    let b = s.graph("b");
    s.seed(&b, "Beta", "zebra-starved");
    let (started, exchange_started) = mpsc::channel();
    let relay = SlowRelay::new(3, Duration::from_secs(8), started);
    s.setup_at(&b, &relay.url);
    s.access(&b, Level::Write);
    let daemon = s.daemon(&b);
    exchange_started
        .recv_timeout(Duration::from_secs(15))
        .expect("the daemon reached the stalled exchange");

    let discovery = utopia_api::read_discovery(&s.discovery(&b)).unwrap();
    let (answered, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let op = utopia_api::Op::Capture {
            text: "zebra-starved-served".into(),
        };
        let _ = answered.send(utopia_api::Client::new(discovery).op(&op));
    });
    match answer.recv_timeout(Duration::from_secs(4)) {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => panic!("the capture failed during the exchange: {error}"),
        Err(_) => panic!("the capture did not run while the exchange was stalled"),
    }
    assert_eq!(
        hits(&b, "zebra-starved-served"),
        1,
        "the owner wrote the capture while the exchange was in flight"
    );

    let (status, stderr) = daemon.stop();
    assert!(status.success(), "daemon exit: {status}\n{stderr}");
}

/// P2: `serve-api graph [addr] --sync` keeps the address it was given.
#[test]
fn serve_api_sync_uses_the_explicit_address() {
    let s = Scratch::new();
    let b = s.graph("b");
    s.seed(&b, "Beta", "zebra-address");
    s.setup(&b);
    let port = {
        let free = TcpListener::bind("127.0.0.1:0").unwrap();
        free.local_addr().unwrap().port()
    };
    let addr = format!("127.0.0.1:{port}");
    let child = s
        .utopia()
        .args(["serve-api"])
        .arg(&b)
        .arg(&addr)
        .arg("--sync")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut daemon = Daemon { child: Some(child) };
    daemon.wait_ready(&s.discovery(&b), &b);
    let discovery = utopia_api::read_discovery(&s.discovery(&b)).unwrap();
    assert_eq!(
        discovery.url,
        format!("http://{addr}"),
        "the alias silently changed the requested address"
    );
    let (status, stderr) = daemon.stop();
    assert!(status.success(), "daemon exit: {status}\n{stderr}");
}
