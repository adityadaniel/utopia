//! MC-5, MC-13, MC-14 (PRD §13.8): `utopia mcp`, end to end.
//!
//! These start the real binary with pipes on `stdin` and `stdout` and speak JSON-RPC to it,
//! because what is under test is the process: it opens no store before the first call, it
//! takes `--graph` and nothing else, it answers in the two protocol eras, and it exits when
//! its input closes.
//!
//! Every child gets `UTOPIA_GRAPH`, `UTOPIA_CONFIG_DIR` and `HOME` inside the temp
//! directory (the `Scratch` pattern of `owner_lock.rs`), so nothing can reach the user's
//! own graph or config. `UTOPIA_GRAPH` is set even where `--graph` is missing, so that a
//! test proves the command never falls back to the app's start-up order.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use utopia_api::access::Level;
use utopia_api::Access;
use utopia_store::{OwnerLock, Store};

/// The two `_meta` keys of the 2026-07-28 era (`docs/mcp-design.md` §3.2).
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const CLIENT_KEY: &str = "io.modelcontextprotocol/clientInfo";

struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    graph: PathBuf,
}

impl Scratch {
    /// A graph with one page and one block that `search` finds.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let graph = root.join("g.utopia");
        let mut store = Store::open(&graph).unwrap();
        let page = store.create_page("Zebra").unwrap();
        let first = store.ensure_first_block(&page).unwrap();
        store
            .set_block_text(&page, &first, "zebrasearch the quick brown fox", None, None)
            .unwrap();
        store.commit_block(&page, &first).unwrap();
        drop(store);
        Self {
            _dir: dir,
            root,
            graph,
        }
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// A graph path that nothing has created yet.
    fn unopened(root: &Path) -> PathBuf {
        root.join("later.utopia")
    }

    /// The binary, confined to this scratch directory.
    fn utopia(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_utopia"));
        cmd.env("UTOPIA_GRAPH", &self.graph)
            .env("UTOPIA_CONFIG_DIR", self.config())
            .env("HOME", self.root.join("home"))
            .env_remove("UTOPIA_API")
            .env_remove("UTOPIA_DB_PASSPHRASE")
            .env_remove("UTOPIA_SHOT_SCRIPT")
            .env_remove("UTOPIA_SYNC_URL")
            .env_remove("UTOPIA_SYNC_FOLDER")
            .env_remove("UTOPIA_LAN_SERVE")
            .stdin(Stdio::null());
        cmd
    }

    /// Write this graph's access setting (MC-1, MC-5).
    fn access(&self, level: Level, when_closed: bool) {
        self.access_for(&self.graph, level, when_closed);
    }

    fn access_for(&self, graph: &Path, level: Level, when_closed: bool) {
        Access { level, when_closed }
            .save(&utopia_api::access::access_file(&self.config(), graph))
            .unwrap();
    }

    /// `utopia mcp` with the graph named on the command line (MC-13).
    fn mcp(&self, extra: &[&str]) -> Server {
        let mut cmd = self.utopia();
        cmd.arg("mcp");
        if !extra.contains(&"--graph") {
            cmd.arg("--graph").arg(&self.graph);
        }
        cmd.args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Server::start(cmd)
    }

    /// Start `serve-api` as the graph's owner and wait until it serves (the bridge's other
    /// end).
    fn serve_api(&self) -> Child {
        let mut child = self
            .utopia()
            .args([
                "serve-api",
                &self.graph.display().to_string(),
                "127.0.0.1:0",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.contains("automation API on"), "serve-api said: {line}");
        child
    }
}

/// A running `utopia mcp` with its pipes. Dropping it kills the process, so a failed
/// assertion never leaves one behind.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    out: BufReader<ChildStdout>,
    err: Option<ChildStderr>,
}

struct Finished {
    status: ExitStatus,
    stderr: String,
}

impl Server {
    fn start(mut cmd: Command) -> Self {
        let mut child = cmd.spawn().unwrap();
        Self {
            stdin: child.stdin.take(),
            out: BufReader::new(child.stdout.take().unwrap()),
            err: child.stderr.take(),
            child,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("input is still open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// One answer. Every line of `stdout` is one JSON-RPC 2.0 message (MC-6): anything else
    /// on that stream would break the client, so the check lives here for every test.
    fn read(&mut self) -> Value {
        let mut line = String::new();
        let n = self.out.read_line(&mut line).unwrap();
        assert!(n > 0, "utopia mcp closed its output without an answer");
        let message: Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|e| panic!("stdout carried {line:?}, which is not JSON-RPC: {e}"));
        assert_eq!(message["jsonrpc"], "2.0", "line: {line:?}");
        assert!(
            message.get("id").is_some() && message.get("result").is_some()
                || message.get("error").is_some(),
            "a response carries an id and a result or an error: {line:?}"
        );
        message
    }

    /// Close the input: the server must exit (MC-6).
    fn finish(mut self) -> Finished {
        drop(self.stdin.take());
        let status = self
            .child
            .wait()
            .expect("utopia mcp exits when its input closes");
        let mut stderr = String::new();
        if let Some(mut err) = self.err.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        Finished { status, stderr }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start `utopia mcp` for `graph`, send one request, and time the answer from the moment
/// the process is up (page-in of a freshly built binary is the launcher's cost).
fn one_shot(s: &Scratch, graph: &Path, request: Value) -> (Value, Duration) {
    let mut cmd = s.utopia();
    cmd.args(["mcp", "--graph"]).arg(graph);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut server = Server::start(cmd);
    let started = Instant::now();
    server.send(request);
    let answer = server.read();
    let answered = started.elapsed();
    assert!(server.finish().status.success());
    (answer, answered)
}

fn modern_meta() -> Value {
    json!({
        VERSION_KEY: "2026-07-28",
        CAPABILITIES_KEY: {},
        CLIENT_KEY: {"name": "process-test", "version": "1"},
    })
}

fn tool_call(id: u32, name: &str, arguments: Value, meta: Option<Value>) -> Value {
    let mut params = json!({"name": name, "arguments": arguments});
    if let Some(meta) = meta {
        params["_meta"] = meta;
    }
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params})
}

/// The text of a tool result, and whether the server marked it as an error.
fn text_of(answer: &Value) -> (String, bool) {
    assert!(
        answer.get("error").is_none(),
        "expected a tool result, got {answer}"
    );
    (
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string(),
        answer["result"]["isError"].as_bool().unwrap(),
    )
}

/// MC-6, MC-14: a 2026-07-28 client discovers the server and reaches one `search`; the
/// first answer comes in under a second, and end of input ends the process.
#[test]
fn mc_6_a_modern_client_discovers_and_searches() {
    let s = Scratch::new();
    s.access(Level::Read, true);
    let mut server = s.mcp(&[]);

    server.send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
               "params": {"_meta": modern_meta()}}),
    );
    let discover = server.read();
    assert_eq!(discover["id"], 1);
    assert_eq!(discover["result"]["resultType"], "complete");
    let versions = discover["result"]["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!("2026-07-28")), "{versions:?}");

    server.send(tool_call(
        2,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let answer = server.read();
    let (text, is_error) = text_of(&answer);
    assert!(!is_error, "{text}");
    assert!(text.contains("Zebra"), "{text}");
    assert!(text.contains("zebrasearch"), "{text}");

    let done = server.finish();
    assert!(done.status.success(), "exit: {:?}", done.status);
    assert!(
        done.stderr.contains("era=modern") && done.stderr.contains("process-test"),
        "the era line PR 4 records: {:?}",
        done.stderr
    );
}

/// MC-6, MC-14: a handshake client gets `initialize`, then one `search`, in the legacy
/// shape (no `resultType`, no `ttlMs`). End of input ends the process.
#[test]
fn mc_6_a_legacy_client_initializes_and_searches() {
    let s = Scratch::new();
    s.access(Level::Read, true);
    let mut server = s.mcp(&[]);

    server.send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "process-test", "version": "1"},
        },
    }));
    let init = server.read();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"].get("resultType").is_none(), "legacy shape");

    // A notification never gets an answer (MC-6); the next line is the answer to `search`.
    server.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    server.send(tool_call(
        2,
        "search",
        json!({"query": "zebrasearch"}),
        None,
    ));
    let answer = server.read();
    assert_eq!(answer["id"], 2);
    let (text, is_error) = text_of(&answer);
    assert!(!is_error, "{text}");
    assert!(text.contains("Zebra"), "{text}");

    let done = server.finish();
    assert!(done.status.success(), "exit: {:?}", done.status);
    assert!(done.stderr.contains("era=legacy"), "{:?}", done.stderr);
}

/// MC-1: without an access file every call answers the Off text, and `tools/list` still
/// answers so the client shows the server as connected.
#[test]
fn mc_1_access_off_answers_the_off_text() {
    let s = Scratch::new();
    let mut server = s.mcp(&[]);
    server.send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list",
                       "params": {"_meta": modern_meta()}}));
    let list = server.read();
    assert!(!list["result"]["tools"].as_array().unwrap().is_empty());

    server.send(tool_call(
        2,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(is_error);
    assert!(text.contains("Settings → Assistants"), "{text}");
    assert!(text.contains("Nothing was read or written"), "{text}");
    assert!(server.finish().status.success());
}

/// MC-5: the app is closed and `when_closed` is off, so the call stops without opening the
/// store; the graph is exactly as it was afterwards.
#[test]
fn mc_5_a_closed_app_without_when_closed_asks_to_open_utopia() {
    let s = Scratch::new();
    s.access(Level::Read, false);
    let mut server = s.mcp(&[]);
    server.send(tool_call(
        1,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(is_error);
    assert!(text.contains("Open Utopia"), "{text}");
    assert!(server.finish().status.success());

    // The lock is free again and the graph is clean: nothing was opened for a write.
    assert!(OwnerLock::try_acquire(&s.graph).unwrap().is_some());
    let doctor = s
        .utopia()
        .args(["doctor", &s.graph.display().to_string()])
        .output()
        .unwrap();
    assert!(
        doctor.status.success(),
        "doctor: {}",
        String::from_utf8_lossy(&doctor.stderr)
    );
}

/// MC-5: `when_closed` is on, so the call runs with no app and no prompt, and `doctor` is
/// clean afterwards.
#[test]
fn mc_5_when_closed_reads_without_the_app() {
    let s = Scratch::new();
    s.access(Level::Read, true);
    let mut server = s.mcp(&[]);
    server.send(tool_call(
        1,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(!is_error, "{text}");
    assert!(text.contains("zebrasearch"), "{text}");
    assert!(server.finish().status.success());

    let doctor = s
        .utopia()
        .args(["doctor", &s.graph.display().to_string()])
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&doctor.stdout);
    assert!(doctor.status.success(), "doctor: {out}");
    assert!(out.contains("ok: all invariants hold"), "{out}");
}

/// MC-4, MC-2: a `serve-api` child owns the graph, so the call goes through `/v1/op` and
/// the owner runs it. `when_closed` is off here: a headless call would have said "Open
/// Utopia", so a result can only have come from the owner.
#[test]
fn mc_4_a_call_goes_to_the_open_owner_through_its_api() {
    let s = Scratch::new();
    s.access(Level::Read, false);
    let mut owner = s.serve_api();

    let mut server = s.mcp(&[]);
    server.send(tool_call(
        1,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(!is_error, "{text}");
    assert!(text.contains("Zebra"), "{text}");
    assert!(server.finish().status.success());

    // Stop the owner before the graph is opened to check.
    owner.kill().unwrap();
    owner.wait().unwrap();
    assert!(OwnerLock::try_acquire(&s.graph).unwrap().is_some());
}

/// The live block ids of one page, read from a fresh store (the owner commits and flushes
/// each write, so its work is visible here).
fn live_ids(graph: &Path, title: &str) -> Vec<String> {
    let mut store = Store::open(graph).unwrap();
    let page = store.find_page(title).unwrap().expect("the page");
    let st = store.page_state(&page).unwrap().clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, _)| id.as_str().to_string())
        .collect()
}

/// The last live block of `title`, polling for up to 10 s: a write the owner (`serve-api`)
/// runs is committed by its writer after the answer, so a reader of the file can be early.
fn wait_for_live_id(graph: &Path, title: &str) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(id) = live_ids(graph, title).pop() {
            return Some(id);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// MC-9, MC-5: the delete answer says who can undo it. A headless call drops its store (and
/// the undo stack with it), so it must not offer a `Cmd+Z`; a call the owner runs is one step
/// on the app's stack and says so. PR 9's review: the answer promised the undo either way.
#[test]
fn mc_9_the_delete_answer_says_who_can_undo_it() {
    let s = Scratch::new();
    s.access(Level::Write, true);
    let headless_target = live_ids(&s.graph, "Zebra")[0].clone();

    // The app closed: `utopia mcp` opens the store, deletes, and closes it.
    let mut server = s.mcp(&[]);
    server.send(tool_call(
        1,
        "delete_block",
        json!({"id": headless_target}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(!is_error, "{text}");
    assert!(text.contains("tombstone"), "{text}");
    assert!(
        text.contains("left no undo step"),
        "a headless delete says the app cannot undo it: {text}"
    );
    assert!(
        !text.contains("Cmd+Z in Utopia restores"),
        "and does not offer a recovery that is not there: {text}"
    );
    assert!(server.finish().status.success());

    // The owner runs the next delete: `serve-api` holds the graph and its store, so the
    // group is on its undo stack.
    let mut owner = s.serve_api();
    let mut server = s.mcp(&[]);
    server.send(tool_call(
        2,
        "append_blocks",
        json!({"page": "Zebra", "markdown": "- a block the owner will delete\n"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(!is_error, "{text}");
    // The owner writes in the background: wait for the block to reach the graph file rather
    // than read it at once (CI machines are slower than the append's answer).
    let owner_target = wait_for_live_id(&s.graph, "Zebra")
        .expect("the block the owner just added reaches the graph file");
    server.send(tool_call(
        3,
        "delete_block",
        json!({"id": owner_target}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(!is_error, "{text}");
    assert!(
        text.contains("Cmd+Z in Utopia restores"),
        "the owner keeps the undo group, and the answer says so: {text}"
    );
    assert!(!text.contains("left no undo step"), "{text}");
    assert!(server.finish().status.success());

    owner.kill().unwrap();
    owner.wait().unwrap();
}

/// MC-5, step 4 of the rule: a process owns the graph but runs no API (what a level turned
/// on while the app runs looks like until Settings can change it live). The command retries
/// the lock for 2 s and then tells the owner what to do.
#[test]
fn mc_5_an_owner_without_an_api_asks_to_open_utopia_again() {
    let s = Scratch::new();
    s.access(Level::Read, true);
    let owner = OwnerLock::try_acquire(&s.graph)
        .unwrap()
        .expect("free lock");

    let started = Instant::now();
    let mut server = s.mcp(&[]);
    server.send(tool_call(
        1,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(is_error);
    assert!(
        text.contains("does not answer calls from assistants"),
        "{text}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(1900),
        "it tried the lock for 2 s first: {:?}",
        started.elapsed()
    );
    assert!(server.finish().status.success());
    drop(owner);
}

/// MC-2: `--read-only` lowers the level for the process and cannot raise it. It serves the
/// six read tools, and with access Off it still gives the Off text.
#[test]
fn mc_2_read_only_serves_reads_and_cannot_raise_access() {
    let s = Scratch::new();
    s.access(Level::Read, true);
    let mut server = s.mcp(&["--read-only"]);
    server.send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list",
                       "params": {"_meta": modern_meta()}}));
    let list = server.read();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6, "the six read tools");
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool}");
    }
    server.send(tool_call(
        2,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(!is_error, "{text}");
    assert!(server.finish().status.success());

    // It cannot raise a level that is Off.
    let s = Scratch::new();
    let mut server = s.mcp(&["--read-only"]);
    server.send(tool_call(
        1,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(is_error);
    assert!(text.contains("Settings → Assistants"), "{text}");
    assert!(server.finish().status.success());
}

/// MC-13: without `--graph` the command stops with a usage error. `UTOPIA_GRAPH` names a
/// graph, and it is still ignored: an agent cannot reach a graph by the app's start-up
/// order.
#[test]
fn mc_13_mcp_without_graph_stops() {
    let s = Scratch::new();
    let out = s.utopia().arg("mcp").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "mcp without --graph must fail");
    assert!(stderr.contains("--graph"), "stderr: {stderr}");
    assert!(
        out.stdout.is_empty(),
        "nothing on stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// MC-13, D6 A: `utopia mcp config` prints the entry with this binary's path and the graph.
#[test]
fn mc_13_the_config_entry_names_this_binary_and_the_graph() {
    use utopia_mcp::SetupClient;
    let s = Scratch::new();
    let out = s
        .utopia()
        .args(["mcp", "config", "codex", "--graph"])
        .arg(&s.graph)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "config: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdin = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_utopia"));
    assert_eq!(stdin, SetupClient::Codex.entry(&bin, Some(&s.graph)));
    assert!(stdin.starts_with("codex mcp add utopia -- "), "{stdin}");
    assert!(stdin.contains(&s.graph.display().to_string()), "{stdin}");

    let out = s.utopia().args(["mcp", "config", "nope"]).output().unwrap();
    assert!(!out.status.success(), "an unknown client is refused");
}

/// MC-13: the entry pins the graph the owner named, so the path it carries cannot mean a
/// different file later. A relative `--graph` is resolved against the directory the owner
/// typed it in, not against the directory the AI client happens to start the server in.
#[test]
fn mc_13_the_config_entry_pins_an_absolute_graph() {
    use utopia_mcp::SetupClient;
    let s = Scratch::new();
    let out = s
        .utopia()
        .current_dir(&s.root)
        .args(["mcp", "config", "codex", "--graph", "Notes.utopia"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "config: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let entry = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // The child resolves the path from its own working directory, which macOS reports
    // without the `/var` → `/private/var` symlink the temp dir was created under.
    let root = std::fs::canonicalize(&s.root).unwrap();
    let expected = SetupClient::Codex.entry(
        &PathBuf::from(env!("CARGO_BIN_EXE_utopia")),
        Some(&root.join("Notes.utopia")),
    );
    assert_eq!(entry, expected, "the entry carries the absolute path");
    assert!(
        !entry.contains("--graph Notes.utopia"),
        "a relative path would follow the client's working directory: {entry}"
    );
}

/// MC-14: start-up opens no store, and `server/discover` and `initialize` answer in under a
/// second. A graph path that does not exist is the sharpest check of "no store": opening it
/// would create the file.
///
/// The clock starts once the process is up, because page-in of a freshly built binary is
/// the launcher's cost, not the server's (AGENTS.md, known limits: ~1 s on a first launch).
/// The first launch below warms the binary; the two timed ones follow it.
#[test]
fn mc_14_discover_and_initialize_answer_without_a_store() {
    let s = Scratch::new();
    let graph = Scratch::unopened(&s.root);
    let discover = json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover",
                          "params": {"_meta": modern_meta()}});
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "process-test", "version": "1"},
        },
    });

    // The first launch of a fresh build pages the binary in; it still must not open (and so
    // create) the graph.
    let (answer, _) = one_shot(&s, &graph, discover.clone());
    assert_eq!(answer["id"], 1);
    assert!(
        !graph.exists(),
        "start-up must not open (and so create) the store: {} exists",
        graph.display()
    );

    for request in [discover, initialize] {
        let method = request["method"].as_str().unwrap().to_string();
        let (answer, answered) = one_shot(&s, &graph, request);
        assert_eq!(answer["id"], 1);
        assert!(
            answered < Duration::from_secs(1),
            "{method} answered in {answered:?} (MC-14)"
        );
        assert!(!graph.exists(), "{method} opened no store");
    }

    // The first call is what opens the store, and a graph that is not there is a text.
    s.access_for(&graph, Level::Read, true);
    let mut cmd = s.utopia();
    cmd.args(["mcp", "--graph"]).arg(&graph);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut server = Server::start(cmd);
    server.send(tool_call(
        1,
        "search",
        json!({"query": "anything"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(is_error, "no store, so no result: {text}");
    assert!(text.contains("no graph at"), "{text}");
    assert!(!graph.exists(), "the call did not create it");
    assert!(server.finish().status.success());
}

/// Encrypt a scratch graph that this test just closed. `set_encryption` needs the graph's
/// lock exclusively and answers `Busy` while anyone holds it. Other tests in this binary
/// spawn processes all the time, and a child holds a copy of every descriptor the parent had
/// open (this store's lock included) from `fork` until its `exec` closes them. Under a busy
/// `make check` that window is long enough to see. Wait it out instead of failing.
fn encrypt_graph(graph: &Path, passphrase: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match utopia_store::set_encryption(graph, None, Some(passphrase)) {
            Ok(()) => return,
            Err(utopia_store::StoreError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("encrypt the scratch graph: {e:?}"),
        }
    }
}

/// MC-5: the opener never prompts. An encrypted graph with no `UTOPIA_DB_PASSPHRASE` and no
/// Keychain item fails with a text, and the process does not hang on a read of `stdin`.
#[test]
fn mc_5_an_encrypted_graph_without_a_passphrase_is_a_text_not_a_prompt() {
    if !utopia_store::encryption_available() {
        return;
    }
    let s = Scratch::new();
    s.access(Level::Read, true);
    encrypt_graph(&s.graph, "secret");

    let started = Instant::now();
    let mut server = s.mcp(&[]);
    server.send(tool_call(
        1,
        "search",
        json!({"query": "zebrasearch"}),
        Some(modern_meta()),
    ));
    let (text, is_error) = text_of(&server.read());
    assert!(is_error);
    assert!(text.contains("encrypted at rest"), "{text}");
    assert!(text.contains("UTOPIA_DB_PASSPHRASE"), "{text}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "it did not wait for input: {:?}",
        started.elapsed()
    );
    assert!(server.finish().status.success());
}
