//! JSON-RPC 2.0 on stdio for the two MCP eras (PRD §13.8 MC-6, MC-14;
//! `docs/mcp-design.md` §3.1, §3.2).
//!
//! One JSON-RPC message for each line of `stdout`. The server answers the handshake era
//! (`initialize`, 2025-11-25 and earlier) and the handshake-free 2026-07-28 era from one
//! process, because the clients are a mix of the two:
//!
//! | Era | How a request is recognised | What it changes |
//! |---|---|---|
//! | legacy | `initialize` selects it for the life of the process | results have no `resultType`, `ttlMs` or `cacheScope` |
//! | modern | `params._meta` has the protocol version and the client capabilities | `server/discover` exists, each result has `resultType` and the server's `_meta`, and the version is checked per request |
//!
//! Two threads: one reads the input, one runs the calls. That is what lets a
//! `notifications/cancelled` reach the server while a tool call is running (design §3.1).
//! A cancel is recorded by the reader the moment it arrives, so a call still in the queue
//! never runs and a call already running gets no answer. A cancel cannot undo a write the
//! owner already started.
//!
//! `stdout` carries protocol messages only: no code here calls `println!` (MC-6). The one
//! line about the first request goes to `stderr` (or to [`Options::log`]), for PR 4 to
//! record the era each real client uses.

use std::collections::HashSet;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};

use crate::backend::Backend;
use crate::tools;

/// The revision with no `initialize` handshake (design §3.2).
pub const MODERN: &str = "2026-07-28";

/// The handshake revisions the server answers, newest first (design §3.1).
pub const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Every revision the server speaks, newest first: `server/discover`'s `supportedVersions`.
pub const SUPPORTED: [&str; 5] = [MODERN, LEGACY[0], LEGACY[1], LEGACY[2], LEGACY[3]];

/// How `serverInfo` and `tools/list`'s cache values name the server (§3.2, §4.2).
pub const SERVER_NAME: &str = "utopia";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The tool list cannot change while the process runs (design §5), so it is cacheable.
pub const CACHE_TTL_MS: u64 = 3_600_000;
pub const CACHE_SCOPE: &str = "public";

/// `_meta` keys of 2026-07-28 (§3.2).
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";
const CLIENT_KEY: &str = "io.modelcontextprotocol/clientInfo";
const SERVER_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// The error for a protocol version the server does not speak (§3.2).
pub const UNSUPPORTED_VERSION: i64 = -32022;
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// How this process behaves, independent of the graph.
#[derive(Default)]
pub struct Options {
    /// `utopia mcp --read-only` (MC-2): the level cannot go above read, and the write tools
    /// leave `tools/list` for the life of this process. It never raises the access file's
    /// level.
    pub read_only: bool,
    /// Where the one-line note about the first request goes. `None` writes it to the
    /// process's `stderr`; tests pass a buffer.
    pub log: Option<Box<dyn Write + Send>>,
}

impl Options {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Serve one MCP client until its input closes (MC-6).
///
/// `reader` is read on its own thread; the calls run on this one. The function returns when
/// the input ends, which is how a client stops the server.
pub fn serve<R, W>(reader: R, writer: W, backend: &dyn Backend, options: Options) -> io::Result<()>
where
    R: Read + Send + 'static,
    W: Write,
{
    let cancelled: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let (tx, rx) = mpsc::channel::<Incoming>();
    let read_cancelled = Arc::clone(&cancelled);
    let reader_thread = thread::spawn(move || read_lines(reader, tx, read_cancelled));

    let mut server = Server {
        backend,
        read_only: options.read_only,
        legacy: false,
        logged: false,
        log: options.log,
    };
    let mut out = writer;
    for incoming in rx {
        match incoming {
            Incoming::Message(message) => {
                // A notification never gets an answer (design §3.2); `notifications/
                // cancelled` was already recorded by the reader.
                let Some(id) = message.get("id").cloned().filter(|id| !id.is_null()) else {
                    continue;
                };
                if is_cancelled(&cancelled, &id) {
                    continue;
                }
                let request = match Request::parse(&message) {
                    Ok(request) => request,
                    Err(e) => {
                        write_message(&mut out, error_response(id, e))?;
                        continue;
                    }
                };
                let answer = server.handle(&request);
                // The client may have cancelled while the call ran: the answer is kept back.
                if is_cancelled(&cancelled, &id) {
                    continue;
                }
                let message = match answer {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err(e) => error_response(id, e),
                };
                write_message(&mut out, message)?;
            }
            Incoming::BadLine => {
                write_message(
                    &mut out,
                    error_response(Value::Null, RpcError::new(PARSE_ERROR, "parse error")),
                )?;
            }
        }
    }
    // The reader ended, so the input closed: exit (MC-6).
    let _ = reader_thread.join();
    Ok(())
}

enum Incoming {
    /// One line that parsed as JSON.
    Message(Value),
    /// One line that did not.
    BadLine,
}

/// Read the input line by line and hand each message to the calls thread. Cancellations are
/// recorded here rather than on the calls thread, so a call that is running sees the cancel
/// before it answers.
fn read_lines<R: Read>(
    reader: R,
    tx: mpsc::Sender<Incoming>,
    cancelled: Arc<Mutex<HashSet<String>>>,
) {
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => return,
            Ok(_) => {}
            Err(_) => return,
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            if tx.send(Incoming::BadLine).is_err() {
                return;
            }
            continue;
        };
        if message.get("method").and_then(Value::as_str) == Some("notifications/cancelled") {
            if let Some(id) = message.pointer("/params/requestId") {
                if let Ok(mut set) = cancelled.lock() {
                    set.insert(id_key(id));
                }
            }
            continue;
        }
        if tx.send(Incoming::Message(message)).is_err() {
            return;
        }
    }
}

fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    }
}

fn is_cancelled(cancelled: &Arc<Mutex<HashSet<String>>>, id: &Value) -> bool {
    cancelled
        .lock()
        .map(|set| set.contains(&id_key(id)))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------
// Requests and answers

struct Request {
    method: String,
    params: Value,
}

impl Request {
    fn parse(message: &Value) -> Result<Self, RpcError> {
        let Some(object) = message.as_object() else {
            return Err(RpcError::new(
                INVALID_REQUEST,
                "a message must be one JSON-RPC object",
            ));
        };
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return Err(RpcError::new(
                INVALID_REQUEST,
                "a message must have a method",
            ));
        };
        Ok(Self {
            method: method.to_string(),
            params: object.get("params").cloned().unwrap_or(Value::Null),
        })
    }
}

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

type Answer = Result<Value, RpcError>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Era {
    Legacy,
    Modern,
}

struct Server<'a> {
    backend: &'a dyn Backend,
    read_only: bool,
    /// `initialize` has been seen: the legacy rules hold for the rest of the process (§3.1).
    legacy: bool,
    logged: bool,
    log: Option<Box<dyn Write + Send>>,
}

impl Server<'_> {
    fn handle(&mut self, request: &Request) -> Answer {
        let meta = request.params.get("_meta").and_then(Value::as_object);
        let modern_meta =
            meta.is_some_and(|m| m.contains_key(VERSION_KEY) && m.contains_key(CAPABILITIES_KEY));

        // `initialize` selects the legacy era for the life of the process (§3.1).
        if request.method == "initialize" {
            self.legacy = true;
            self.log_first(Some(Era::Legacy), &request.params);
            let requested = request
                .params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(LEGACY[0]);
            let version = if LEGACY.contains(&requested) {
                requested
            } else {
                // A version the server does not speak gets its newest legacy revision.
                LEGACY[0]
            };
            return Ok(json!({
                "protocolVersion": version,
                "capabilities": capabilities(),
                "serverInfo": server_info(),
                "instructions": tools::INSTRUCTIONS,
            }));
        }

        let era = if modern_meta {
            Era::Modern
        } else if self.legacy {
            Era::Legacy
        } else {
            // Neither an `initialize` nor the modern `_meta`: the client has told the
            // server nothing about how to answer it (§3.2).
            self.log_first(None, &request.params);
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!(
                    "this request has no params._meta[\"{VERSION_KEY}\"] or no \
                     params._meta[\"{CAPABILITIES_KEY}\"], and no initialize came before it, \
                     so the server cannot tell which MCP revision to answer with: send the \
                     _meta keys (MCP {MODERN}) or an initialize request (MCP {}).",
                    LEGACY[0]
                ),
            ));
        };
        self.log_first(Some(era), &request.params);

        if era == Era::Modern {
            let requested = meta
                .and_then(|m| m.get(VERSION_KEY))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if requested != MODERN {
                return Err(RpcError {
                    code: UNSUPPORTED_VERSION,
                    message: format!(
                        "unsupported protocol version {requested:?}; this server speaks \
                         {MODERN} in this request shape"
                    ),
                    data: Some(json!({
                        "supported": [MODERN],
                        "requested": requested,
                    })),
                });
            }
        }

        let answer = match request.method.as_str() {
            "ping" => Ok(json!({})),
            "tools/list" => {
                let mut body = json!({ "tools": tools::list(self.read_only) });
                if era == Era::Modern {
                    // The list cannot change while the process runs (design §5), so a
                    // 2026-07-28 client may keep it for an hour (§3.2).
                    body["ttlMs"] = json!(CACHE_TTL_MS);
                    body["cacheScope"] = json!(CACHE_SCOPE);
                }
                Ok(body)
            }
            "tools/call" => self.call_tool(&request.params),
            // `server/discover` is how a 2026-07-28 client asks what this server is. It
            // answers at once: no store is open (MC-14).
            "server/discover" if era == Era::Modern => Ok(json!({
                "supportedVersions": SUPPORTED,
                "capabilities": capabilities(),
                "instructions": tools::INSTRUCTIONS,
                "ttlMs": CACHE_TTL_MS,
                "cacheScope": CACHE_SCOPE,
            })),
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        };
        answer.map(|body| decorate(era, body))
    }

    fn call_tool(&mut self, params: &Value) -> Answer {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
        match tools::call(self.backend, self.read_only, name, &arguments) {
            Ok(text) => Ok(tool_result(&text, false)),
            Err(tools::ToolError::Unknown) => Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown tool {name:?}: tools/list names the tools this server has"),
            )),
            Err(tools::ToolError::Failed(text)) => Ok(tool_result(&text, true)),
        }
    }

    /// One line for the first request: the era and the client (PR 4 records which era each
    /// real client uses). It goes to `stderr`, never `stdout` (MC-6).
    fn log_first(&mut self, era: Option<Era>, params: &Value) {
        if self.logged {
            return;
        }
        self.logged = true;
        let meta = params.get("_meta");
        let version = meta
            .and_then(|m| m.get(VERSION_KEY))
            .and_then(Value::as_str)
            .or_else(|| params.get("protocolVersion").and_then(Value::as_str))
            .unwrap_or("-");
        let client = meta
            .and_then(|m| m.get(CLIENT_KEY))
            .or_else(|| params.get("clientInfo"))
            .and_then(Value::as_object);
        let (name, version_of_client) = match client {
            Some(c) => (
                c.get("name").and_then(Value::as_str).unwrap_or("unknown"),
                c.get("version").and_then(Value::as_str).unwrap_or("-"),
            ),
            None => ("unknown", "-"),
        };
        let era = match era {
            Some(Era::Modern) => "modern",
            Some(Era::Legacy) => "legacy",
            None => "unknown",
        };
        let line = format!(
            "utopia mcp: era={era} protocol={version} client=\"{}\" version=\"{version_of_client}\"\n",
            name.replace('"', "'")
        );
        match &mut self.log {
            Some(w) => {
                let _ = w.write_all(line.as_bytes());
                let _ = w.flush();
            }
            None => {
                let _ = io::stderr().write_all(line.as_bytes());
            }
        }
    }
}

fn capabilities() -> Value {
    json!({ "tools": {} })
}

fn server_info() -> Value {
    json!({ "name": SERVER_NAME, "version": SERVER_VERSION })
}

/// A 2026-07-28 result: `resultType` and the server's name in `_meta` (§3.2).
fn decorate(era: Era, mut body: Value) -> Value {
    if era == Era::Modern {
        if let Some(object) = body.as_object_mut() {
            object.insert("resultType".into(), json!("complete"));
            object.insert("_meta".into(), json!({ SERVER_KEY: server_info() }));
        }
    }
    body
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

fn error_response(id: Value, error: RpcError) -> Value {
    let mut body = json!({ "code": error.code, "message": error.message });
    if let Some(data) = error.data {
        body["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": body })
}

fn write_message<W: Write>(out: &mut W, message: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *out, &message).map_err(io::Error::other)?;
    out.write_all(b"\n")?;
    out.flush()
}
