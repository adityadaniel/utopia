//! The wire: both protocol eras, the error codes, cancellation and the end of input
//! (PRD §13.8 MC-6, MC-14; docs/mcp-design.md §3.1, §3.2).
//!
//! Every test drives the real `serve` through an in-memory pipe: one thread reads what the
//! test writes, the other runs the calls, exactly as with `stdin`.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use support::{initialize, meta, modern, request, start, text_of, tool_call, Graph, Shared};
use utopia_api::access::Level;
use utopia_api::Op;
use utopia_mcp::backend::BackendError;
use utopia_mcp::{Backend, Options};

fn ready() -> (Graph, Options) {
    let graph = Graph::new();
    graph.set_access(Level::Read, true);
    (graph, Options::new())
}

/// MC-6: the handshake era, from its first message to a tool call. Legacy results keep the
/// legacy shape: no `resultType`, no `ttlMs`, no `cacheScope`.
#[test]
fn mc_6_a_legacy_script_from_initialize_to_a_tool_call() {
    let (graph, options) = ready();
    let reply = support::run(
        vec![
            initialize(1, "2025-06-18"),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            request(2, "tools/list", json!({})),
            tool_call(3, "list_pages", json!({})),
            request(4, "ping", json!({})),
        ],
        graph.backend(),
        options,
    );
    assert_eq!(
        reply.len(),
        4,
        "the notification is not answered: {reply:?}"
    );

    let init = &reply[0]["result"];
    assert_eq!(
        init["protocolVersion"], "2025-06-18",
        "the client's version"
    );
    assert_eq!(init["capabilities"], json!({"tools": {}}));
    assert_eq!(init["serverInfo"]["name"], "utopia");
    let instructions = init["instructions"].as_str().unwrap();
    for syntax in ["[[Page]]", "#tag", "((block-id))", "key:: value", "{id:"] {
        assert!(instructions.contains(syntax), "instructions name {syntax}");
    }
    assert!(init.get("resultType").is_none());
    assert!(init.get("ttlMs").is_none());
    assert!(init.get("cacheScope").is_none());

    let list = &reply[1]["result"];
    let names: Vec<&str> = list["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "search",
            "list_pages",
            "read_page",
            "read_block",
            "get_backlinks",
            "run_query",
            "append_blocks",
            "update_block"
        ]
    );
    assert!(
        list.get("ttlMs").is_none(),
        "legacy lists are not cacheable"
    );
    assert!(list.get("cacheScope").is_none());
    let tool = &list["tools"][0];
    assert_eq!(tool["inputSchema"]["type"], "object");
    assert_eq!(
        tool["annotations"],
        json!({"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true,
               "openWorldHint": false})
    );

    let (text, is_error) = text_of(&reply[2]);
    assert!(!is_error, "{text}");
    assert!(text.contains("- Project X"), "{text}");

    assert_eq!(
        reply[3]["result"],
        json!({}),
        "ping answers an empty result"
    );
    assert!(reply[3]["result"].get("resultType").is_none());
}

/// MC-6: a legacy version the server does not speak gets the newest one it does.
#[test]
fn mc_6_an_unknown_legacy_version_gets_the_newest_legacy_one() {
    let (graph, options) = ready();
    let reply = support::run(vec![initialize(1, "1999-01-01")], graph.backend(), options);
    assert_eq!(reply[0]["result"]["protocolVersion"], "2025-11-25");
}

/// MC-6: the 2026-07-28 era, from `server/discover` to a tool call. Every result has
/// `resultType` and the server's `_meta`, and `tools/list` is cacheable.
#[test]
fn mc_6_a_modern_script_from_discover_to_a_tool_call() {
    let (graph, options) = ready();
    let reply = support::run(
        vec![
            modern(1, "server/discover", json!({})),
            modern(2, "tools/list", json!({})),
            modern(
                3,
                "tools/call",
                json!({"name": "read_page", "arguments": {"page": "Project X"}}),
            ),
        ],
        graph.backend(),
        options,
    );
    assert_eq!(reply.len(), 3, "{reply:?}");

    let discover = &reply[0]["result"];
    assert_eq!(discover["resultType"], "complete");
    let versions: Vec<&str> = discover["supportedVersions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        versions,
        [
            "2026-07-28",
            "2025-11-25",
            "2025-06-18",
            "2025-03-26",
            "2024-11-05"
        ]
    );
    assert_eq!(discover["capabilities"], json!({"tools": {}}));
    assert!(!discover["instructions"].as_str().unwrap().is_empty());
    assert_eq!(discover["ttlMs"], 3_600_000);
    assert_eq!(discover["cacheScope"], "public");
    assert_eq!(
        discover["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "utopia"
    );

    let list = &reply[1]["result"];
    assert_eq!(list["resultType"], "complete");
    assert_eq!(list["ttlMs"], 3_600_000);
    assert_eq!(list["cacheScope"], "public");
    assert_eq!(list["tools"].as_array().unwrap().len(), 8);
    assert_eq!(
        list["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "utopia"
    );

    let call = &reply[2]["result"];
    assert_eq!(call["resultType"], "complete");
    assert_eq!(
        call["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "utopia"
    );
    let (text, is_error) = text_of(&reply[2]);
    assert!(!is_error, "{text}");
    assert!(text.contains("- Plan the launch {id:"), "{text}");
}

/// MC-6: an `initialize` selects the legacy rules for the life of the process.
#[test]
fn mc_6_initialize_selects_legacy_for_the_rest_of_the_process() {
    let (graph, options) = ready();
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            request(2, "tools/list", json!({})),
            request(3, "server/discover", json!({})),
        ],
        graph.backend(),
        options,
    );
    assert!(reply[1]["result"].get("resultType").is_none());
    assert!(reply[1]["result"].get("ttlMs").is_none());
    assert_eq!(
        reply[2]["error"]["code"], -32601,
        "server/discover is 2026-07-28 only"
    );
}

/// MC-6: a modern request without the two `_meta` keys is `-32602` (design §3.2).
#[test]
fn mc_6_a_modern_request_without_the_meta_keys_is_invalid() {
    let (graph, options) = ready();
    let reply = support::run(
        vec![
            request(1, "tools/list", json!({})),
            // Only half of the pair is not enough either.
            request(
                2,
                "tools/list",
                json!({"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}),
            ),
        ],
        graph.backend(),
        options,
    );
    assert_eq!(reply[0]["error"]["code"], -32602, "{:?}", reply[0]);
    assert_eq!(reply[1]["error"]["code"], -32602, "{:?}", reply[1]);
}

/// MC-6: a version the server does not speak gets `-32022` with its list (design §3.2).
#[test]
fn mc_6_an_unsupported_modern_version_is_refused() {
    let (graph, options) = ready();
    let mut params = json!({});
    params["_meta"] = meta("2027-01-01", "test-client");
    let reply = support::run(
        vec![request(1, "tools/list", params)],
        graph.backend(),
        options,
    );
    let error = &reply[0]["error"];
    assert_eq!(error["code"], -32022);
    assert_eq!(error["data"]["requested"], "2027-01-01");
    assert_eq!(error["data"]["supported"], json!(["2026-07-28"]));
}

/// MC-6: a method the server does not know gets `-32601`.
#[test]
fn mc_6_an_unknown_method_is_not_found() {
    let (graph, options) = ready();
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            request(2, "resources/list", json!({})),
        ],
        graph.backend(),
        options,
    );
    assert_eq!(reply[1]["error"]["code"], -32601);
}

/// MC-6: a tool that is not in the catalog is the protocol error, not a result for the
/// model (the order in the brief: `-32602` before every other check).
#[test]
fn mc_6_an_unknown_tool_is_invalid_params() {
    let (graph, options) = ready();
    let reply = support::run(
        vec![
            initialize(1, "2025-11-25"),
            tool_call(2, "rm_rf", json!({})),
        ],
        graph.backend(),
        options,
    );
    assert_eq!(reply[1]["error"]["code"], -32602);
    assert!(reply[1].get("result").is_none());
}

/// MC-6: no answer to a notification, ever.
#[test]
fn mc_6_a_notification_never_gets_an_answer() {
    let (graph, options) = ready();
    let reply = support::run(
        vec![
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                   "params": {"requestId": 1, "reason": "test"}}),
        ],
        graph.backend(),
        options,
    );
    assert!(reply.is_empty(), "{reply:?}");
}

/// MC-6: a line that is not JSON is a parse error, and the next line is still answered.
#[test]
fn mc_6_a_bad_line_does_not_end_the_server() {
    let (graph, options) = ready();
    let server = start(graph.backend(), options);
    server.wire.raw("this is not json\n");
    server.wire.send(&initialize(1, "2025-11-25"));
    let messages = server.out.wait_for(2);
    assert_eq!(messages[0]["error"]["code"], -32700);
    assert_eq!(messages[1]["result"]["protocolVersion"], "2025-11-25");
    drop(server);
}

/// A backend that runs slowly, so a cancel can arrive while a call runs.
struct Slow {
    delay: Duration,
    calls: Arc<AtomicUsize>,
    started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl Slow {
    fn new(delay: Duration) -> (Self, Arc<AtomicUsize>, std::sync::mpsc::Receiver<()>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        (
            Self {
                delay,
                calls: calls.clone(),
                started: Mutex::new(Some(tx)),
            },
            calls,
            rx,
        )
    }
}

impl Backend for Slow {
    fn call(&self, op: Op) -> Result<Value, BackendError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(tx) = self.started.lock().expect("started").take() {
            let _ = tx.send(());
        }
        std::thread::sleep(self.delay);
        Ok(json!({"pages": [], "blocks": [], "op": op.kind() == utopia_api::OpKind::Read}))
    }
}

/// MC-6: a call cancelled while it was still in the queue never runs.
#[test]
fn mc_6_a_cancelled_call_in_the_queue_never_runs() {
    let (slow, calls, _started) = Slow::new(Duration::from_millis(150));
    let server = start(Arc::new(slow), Options::new());
    server.send(initialize(1, "2025-11-25"));
    server.send(tool_call(2, "search", json!({"query": "first"})));
    server.send(tool_call(3, "search", json!({"query": "second"})));
    server.send(
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                       "params": {"requestId": 3}}),
    );
    let messages = server.out.wait_for(2);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the queued call never ran");
    assert_eq!(messages[0]["id"], 1);
    assert_eq!(messages[1]["id"], 2);
    let out = server.out.clone();
    server.finish().expect("serve returns at end of input");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "and it never ran after the first call finished"
    );
    assert_eq!(out.messages().len(), 2, "the cancelled call got no answer");
}

/// MC-6: a cancelled call that is already running gets no answer (it is not stopped: a
/// cancel cannot reverse what the owner started).
#[test]
fn mc_6_a_cancelled_call_that_runs_gets_no_answer() {
    let (slow, calls, started) = Slow::new(Duration::from_millis(150));
    let server = start(Arc::new(slow), Options::new());
    server.send(initialize(1, "2025-11-25"));
    server.send(tool_call(2, "search", json!({"query": "first"})));
    // Wait until the call is inside the backend, then cancel it.
    started
        .recv_timeout(Duration::from_secs(5))
        .expect("the call started");
    server.send(
        json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                       "params": {"requestId": 2}}),
    );
    let out = server.out.clone();
    server.finish().expect("serve returns at end of input");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the call ran");
    let messages = out.messages();
    assert_eq!(
        messages.len(),
        1,
        "only the initialize is answered: {messages:?}"
    );
    assert_eq!(messages[0]["id"], 1);
}

/// MC-6: `serve` returns when the input ends, and no store was opened on the way.
#[test]
fn mc_6_serve_returns_at_end_of_input() {
    let (graph, options) = ready();
    let server = start(graph.backend(), options);
    server.send(initialize(1, "2025-11-25"));
    let messages = server.out.wait_for(1);
    assert_eq!(messages[0]["id"], 1);
    server.finish().expect("serve returns at end of input");
}

/// MC-14: `initialize` and `server/discover` open no store, and answer at once even when
/// the opener would fail.
#[test]
fn mc_14_discover_opens_no_store() {
    let graph = Graph::new();
    graph.set_access(Level::Write, true);
    let d = utopia_mcp::Direct::new(
        &graph.graph,
        &graph.config,
        Box::new(|| panic!("a handshake must not open the store")),
    );
    let started = std::time::Instant::now();
    let reply = support::run(
        vec![
            modern(1, "server/discover", json!({})),
            initialize(2, "2025-11-25"),
        ],
        Arc::new(d),
        Options::new(),
    );
    assert_eq!(reply.len(), 2);
    assert!(reply.iter().all(|m| m.get("result").is_some()));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "MC-14: under a second, took {:?}",
        started.elapsed()
    );
}

/// MC-14: the first request writes one line to the log: the era and the client. PR 4 reads
/// it to record which era each real client uses.
#[test]
fn mc_14_the_first_request_names_the_era_and_the_client() {
    let (graph, _) = ready();
    let log = Shared::default();
    let options = Options {
        read_only: false,
        log: Some(Box::new(log.clone())),
    };
    support::run(
        vec![
            modern(1, "server/discover", json!({})),
            modern(2, "tools/list", json!({})),
        ],
        graph.backend(),
        options,
    );
    let line = log.text();
    assert_eq!(line.lines().count(), 1, "one line: {line:?}");
    assert!(line.contains("era=modern"), "{line}");
    assert!(line.contains("protocol=2026-07-28"), "{line}");
    assert!(line.contains("client=\"test-client\""), "{line}");
}

#[test]
fn mc_14_a_legacy_client_is_logged_as_legacy() {
    let (graph, _) = ready();
    let log = Shared::default();
    let options = Options {
        read_only: false,
        log: Some(Box::new(log.clone())),
    };
    support::run(vec![initialize(1, "2025-03-26")], graph.backend(), options);
    let line = log.text();
    assert!(line.contains("era=legacy"), "{line}");
    assert!(line.contains("protocol=2025-03-26"), "{line}");
    assert!(line.contains("client=\"test-client\""), "{line}");
}
