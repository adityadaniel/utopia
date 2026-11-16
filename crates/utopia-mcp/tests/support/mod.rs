//! The in-memory pipes and the scratch graph the MCP tests drive `serve` through.
//!
//! Every process and every graph a test touches lives under a temporary directory: the
//! tests never see the real graph, config directory or Keychain.
#![allow(dead_code)]

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use utopia_api::access::{access_file, Access, Level};
use utopia_api::{handle, Op};
use utopia_mcp::{serve, Backend, Direct, Options};
use utopia_store::Store;

// ---------------------------------------------------------------------------------------
// A pipe the test writes to and the server reads from

/// The write end of the pipe: what a client sends.
pub struct Wire(Option<Sender<Vec<u8>>>);

impl Wire {
    /// One JSON message and its newline.
    pub fn send(&self, message: &Value) {
        self.raw(&format!("{message}\n"));
    }

    /// Raw bytes, for a line that is not JSON.
    pub fn raw(&self, text: &str) {
        if let Some(tx) = &self.0 {
            let _ = tx.send(text.as_bytes().to_vec());
        }
    }

    /// End the input: `serve` must return (MC-6).
    pub fn close(&mut self) {
        self.0 = None;
    }
}

/// The read end, handed to `serve` on its reader thread.
pub struct Pipe {
    rx: Receiver<Vec<u8>>,
    buf: Vec<u8>,
}

impl Read for Pipe {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.buf.is_empty() {
            match self.rx.recv() {
                Ok(chunk) => self.buf = chunk,
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf.drain(..n);
        Ok(n)
    }
}

pub fn pipe() -> (Wire, Pipe) {
    let (tx, rx) = mpsc::channel();
    (
        Wire(Some(tx)),
        Pipe {
            rx,
            buf: Vec::new(),
        },
    )
}

// ---------------------------------------------------------------------------------------
// A writer the test can read while the server runs

#[derive(Clone, Default)]
pub struct Shared(Arc<Mutex<Vec<u8>>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Shared {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer")).to_string()
    }

    /// Every complete line, parsed. A line still being written is not counted.
    pub fn messages(&self) -> Vec<Value> {
        self.text()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Wait until `n` messages have arrived (5 s, then panic: a stuck server is a bug).
    pub fn wait_for(&self, n: usize) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let messages = self.messages();
            if messages.len() >= n {
                return messages;
            }
            assert!(
                Instant::now() < deadline,
                "waited 5 s for {n} messages, have {}: {:?}",
                messages.len(),
                self.text()
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
}

// ---------------------------------------------------------------------------------------
// A running server

pub struct Running {
    pub wire: Wire,
    pub out: Shared,
    done: Receiver<io::Result<()>>,
    _join: JoinHandle<()>,
}

/// Start `serve` on a pipe with `backend`.
pub fn start(backend: Arc<dyn Backend>, options: Options) -> Running {
    let (wire, pipe) = pipe();
    let out = Shared::default();
    let writer = out.clone();
    let (tx, done) = mpsc::channel();
    let join = thread::spawn(move || {
        let result = serve(pipe, writer, backend.as_ref(), options);
        let _ = tx.send(result);
    });
    Running {
        wire,
        out,
        done,
        _join: join,
    }
}

impl Running {
    pub fn send(&self, message: Value) {
        self.wire.send(&message);
    }

    /// Messages so far (a running server may be about to write more).
    pub fn messages(&self) -> Vec<Value> {
        self.out.messages()
    }

    /// Close the input and wait for `serve` to return.
    pub fn finish(mut self) -> io::Result<()> {
        self.wire.close();
        self.done
            .recv_timeout(Duration::from_secs(5))
            .expect("serve returned within 5 s of its input closing")
    }
}

/// Send everything, close the input, and return every message the server wrote. `serve`
/// must return on its own (MC-6).
pub fn run(messages: Vec<Value>, backend: Arc<dyn Backend>, options: Options) -> Vec<Value> {
    let server = start(backend, options);
    for m in messages {
        server.send(m);
    }
    let out = server.out.clone();
    server.finish().expect("serve returns at end of input");
    out.messages()
}

// ---------------------------------------------------------------------------------------
// JSON-RPC messages

pub fn meta(version: &str, client: &str) -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": client, "version": "9.9.9"},
    })
}

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A 2026-07-28 request: the `_meta` keys are what makes it modern.
pub fn modern(id: u64, method: &str, mut params: Value) -> Value {
    params["_meta"] = meta("2026-07-28", "test-client");
    request(id, method, params)
}

pub fn tool_call(id: u64, name: &str, arguments: Value) -> Value {
    request(
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

pub fn initialize(id: u64, version: &str) -> Value {
    request(
        id,
        "initialize",
        json!({
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "9.9.9"},
        }),
    )
}

/// The text of a `tools/call` result, and whether it is the `isError` kind.
pub fn text_of(message: &Value) -> (String, bool) {
    let result = &message["result"];
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    (text, result["isError"] == json!(true))
}

// ---------------------------------------------------------------------------------------
// A scratch graph

/// A scratch graph, its config directory, and the ids of what was seeded into it.
pub struct Graph {
    dir: tempfile::TempDir,
    pub graph: PathBuf,
    pub config: PathBuf,
    /// The block that says `[[Ana]]`, the one a backlink search finds.
    pub ana_link: String,
    /// The root block of "Project X".
    pub root: String,
}

impl Graph {
    /// A graph with pages, a day, backlinks and one page bigger than a result.
    pub fn new() -> Self {
        Self::create(true)
    }

    /// A graph with nothing in it: no page, no day.
    pub fn empty() -> Self {
        Self::create(false)
    }

    fn create(seed: bool) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let graph = dir.path().join("g.utopia");
        let config = dir.path().join("config");
        let mut store = Store::open(&graph).expect("open the scratch graph");
        let mut ana_link = String::new();
        let mut root = String::new();
        if seed {
            let today = store.today();
            store.create_page("Project X").expect("a page");
            root = append(&mut store, "Project X", "Plan the launch", None);
            ana_link = append(
                &mut store,
                "Project X",
                "Ask [[Ana]] for the copy",
                Some(root.clone()),
            );
            append(&mut store, "Project X", "ship it", None);
            store.create_page("Ana").expect("a page");

            // Today's note, so a read of today has something to show.
            let (day, block) = store.open_daily(today).expect("a day");
            store
                .set_block_text(&day, &block, "Met [[Ana]] today", None, None)
                .expect("text");
            store.commit_block(&day, &block).expect("commit");

            // A page bigger than one result: 600 blocks of about 60 bytes.
            store.create_page("Big Page").expect("a page");
            let only = append(&mut store, "Big Page", "", None);
            let big = store
                .find_page("Big Page")
                .expect("the page")
                .expect("a page");
            let st = store.page_state(&big).expect("the page state").clone();
            let only = utopia_core::BlockId(only);
            let markdown: String = (0..600)
                .map(|i| format!("- block {i} {}\n", "x".repeat(50)))
                .collect();
            let plan = utopia_core::outline::paste(&st, &only, 0..0, &markdown).expect("a plan");
            store.execute_plan(&big, plan, None).expect("the paste");
        }
        store.sync_index().expect("index");
        drop(store);

        Self {
            dir,
            graph,
            config,
            ana_link,
            root,
        }
    }

    pub fn set_access(&self, level: Level, when_closed: bool) {
        Access { level, when_closed }
            .save(&self.access_path())
            .expect("the access file");
    }

    pub fn access_path(&self) -> PathBuf {
        access_file(&self.config, &self.graph)
    }

    pub fn open(&self) -> Store {
        Store::open(&self.graph).expect("open the scratch graph")
    }

    pub fn backend(&self) -> Arc<dyn Backend> {
        Arc::new(self.direct())
    }

    pub fn direct(&self) -> Direct {
        let graph = self.graph.clone();
        Direct::new(
            &self.graph,
            &self.config,
            Box::new(move || Store::open(&graph).map_err(|e| e.to_string())),
        )
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// The number of live pages, from a fresh store's `doctor` (§10.4).
    pub fn page_count(&self) -> usize {
        let mut store = self.open();
        store.doctor().expect("doctor runs").pages
    }
}

/// Append one block under `parent` on `title` and give its id back.
fn append(store: &mut Store, title: &str, text: &str, parent: Option<String>) -> String {
    handle(
        store,
        Op::AppendBlock {
            title: title.to_string(),
            text: text.to_string(),
            parent,
            create: true,
        },
    )
    .expect("append a block")
    .body["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// Today, as the tool layer and the store see it (§5.5: the machine's local date).
pub fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}
