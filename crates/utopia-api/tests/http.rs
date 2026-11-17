//! The automation API over real HTTP, with the store owned by another thread as in the app.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use serde_json::{json, Value};
use utopia_api::{serve, serve_with, ApiServer, Limits};
use utopia_store::Store;

struct Running {
    server: Arc<ApiServer>,
    stop: Arc<AtomicBool>,
    owner: Option<std::thread::JoinHandle<Store>>,
    _dir: tempfile::TempDir,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.owner.take() {
            let _ = t.join();
        }
    }
}

fn start() -> Running {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let server = Arc::new(serve("127.0.0.1:0", "secret-token".into()).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2) = (server.clone(), stop.clone());
    let owner = std::thread::spawn(move || {
        let mut store = Store::open(path).unwrap();
        store.set_today(NaiveDate::from_ymd_opt(2026, 11, 2));
        while !st2.load(Ordering::SeqCst) {
            if let Some(call) = s2.recv_timeout(Duration::from_millis(20)) {
                call.run(&mut store);
            }
        }
        store
    });
    Running {
        server,
        stop,
        owner: Some(owner),
        _dir: dir,
    }
}

fn req(r: &Running, method: &str, path: &str) -> ureq::Request {
    ureq::request(method, &format!("{}{path}", r.server.url()))
        .set("Authorization", "Bearer secret-token")
}

// Mirrors ureq's own `send_string`, whose error type is large.
#[allow(clippy::result_large_err)]
trait SendJson {
    fn send_string_json(self, v: Value) -> Result<ureq::Response, ureq::Error>;
}

impl SendJson for ureq::Request {
    fn send_string_json(self, v: Value) -> Result<ureq::Response, ureq::Error> {
        self.set("Content-Type", "application/json")
            .send_string(&v.to_string())
    }
}

fn ok(resp: Result<ureq::Response, ureq::Error>) -> Value {
    match resp {
        Ok(r) => serde_json::from_str(&r.into_string().unwrap()).unwrap(),
        Err(ureq::Error::Status(code, r)) => panic!("{code}: {}", r.into_string().unwrap()),
        Err(e) => panic!("{e}"),
    }
}

fn status(resp: Result<ureq::Response, ureq::Error>) -> u16 {
    match resp {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn write_then_read_over_http() {
    let r = start();
    // Capture lands in today's (empty) first block.
    let b =
        ok(req(&r, "POST", "/v1/capture").send_string_json(json!({"text": "met [[Dana]] #crm"})));
    assert_eq!(b["text"], "met [[Dana]] #crm");
    let day = ok(req(&r, "GET", "/v1/journal/today").call());
    assert_eq!(day["title"], "November 2nd, 2026");
    assert_eq!(day["blocks"].as_array().unwrap().len(), 1);
    // Append under a block, creating the page on demand.
    let parent = ok(req(&r, "POST", "/v1/pages/Reading%20list/blocks")
        .send_string_json(json!({"text": "Books", "create": true})));
    let pid = parent["id"].as_str().unwrap().to_string();
    ok(req(&r, "POST", "/v1/pages/reading%20LIST/blocks")
        .send_string_json(json!({"text": "Dune #toread", "parent": pid})));
    let page = ok(req(&r, "GET", "/v1/pages/Reading%20list").call());
    assert_eq!(page["blocks"][0]["text"], "Books");
    assert_eq!(page["blocks"][0]["children"][0]["text"], "Dune #toread");
    // Edit a block; references follow.
    let child = page["blocks"][0]["children"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(req(&r, "PATCH", &format!("/v1/blocks/{child}"))
        .send_string_json(json!({"text": "Dune #toread [[Dana]]"})));
    let back = ok(req(&r, "GET", "/v1/pages/Dana/backlinks").call());
    assert_eq!(back["total"], 2);
    let got = ok(req(&r, "GET", &format!("/v1/blocks/{child}")).call());
    assert_eq!(got["text"], "Dune #toread [[Dana]]");
    // Search and query.
    let s = ok(req(&r, "GET", "/v1/search?q=dune").call());
    assert_eq!(s["blocks"].as_array().unwrap().len(), 1);
    let q =
        ok(req(&r, "POST", "/v1/query")
            .send_string_json(json!({"query": "{and: [[Dana]] #toread}"})));
    assert_eq!(q["total"], 1);
    assert_eq!(q["hits"][0]["page_title"], "Reading list");
    let pages = ok(req(&r, "GET", "/v1/pages").call());
    let titles: Vec<&str> = pages["pages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Reading list") && titles.contains(&"Dana"));
}

#[test]
fn errors_are_json_with_status() {
    let r = start();
    assert_eq!(status(req(&r, "GET", "/v1/pages/Nowhere").call()), 404);
    assert_eq!(
        status(req(&r, "POST", "/v1/pages/Nowhere/blocks").send_string_json(json!({"text": "x"}))),
        404,
        "no implicit page creation"
    );
    assert_eq!(
        status(req(&r, "POST", "/v1/query").send_string_json(json!({"query": "{and: [[A]]"}))),
        400
    );
    assert_eq!(status(req(&r, "GET", "/v1/journal/someday").call()), 400);
    assert_eq!(status(req(&r, "GET", "/v1/blocks/nope").call()), 404);
    assert_eq!(
        status(req(&r, "POST", "/v1/capture").send_string_json(json!({"text": "  "}))),
        400
    );
}

#[test]
fn only_authorized_local_non_browser_requests() {
    let r = start();
    let url = format!("{}/v1/pages", r.server.url());
    assert_eq!(status(ureq::get(&url).call()), 401, "no token");
    assert_eq!(
        status(
            ureq::get(&url)
                .set("Authorization", "Bearer wrong-token!")
                .call()
        ),
        401
    );
    assert_eq!(
        status(
            req(&r, "GET", "/v1/pages")
                .set("Origin", "https://evil.example")
                .call()
        ),
        403,
        "browsers cannot call it"
    );
    assert_eq!(
        status(
            req(&r, "GET", "/v1/pages")
                .set("Host", "evil.example")
                .call()
        ),
        403,
        "DNS rebinding"
    );
    assert!(
        serve("0.0.0.0:0", "t".into()).is_err(),
        "never binds publicly"
    );
    // Discovery file is private to the user.
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("api.json");
    r.server.write_discovery(&f).unwrap();
    let v: Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
    assert_eq!(v["token"], "secret-token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(utopia_api::new_token().len(), 64);
}

/// AC §4.1 (local automation API): a request that gets a 504 has been cancelled, so the
/// retry cannot apply the same capture twice (docs/state-machines/api-call-inbox.md).
#[test]
fn ac_4_1_a_timed_out_capture_is_not_written_when_the_owner_gets_to_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let server = Arc::new(
        serve_with(
            "127.0.0.1:0",
            "secret-token".into(),
            Limits {
                wait: Duration::from_millis(150),
                ..Limits::default()
            },
        )
        .unwrap(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2, p2) = (server.clone(), stop.clone(), path.clone());
    let owner = std::thread::spawn(move || {
        let mut store = Store::open(p2).unwrap();
        store.set_today(NaiveDate::from_ymd_opt(2026, 11, 2));
        // Sit on the first call past the client's wait, then drain like the app does.
        let stalled = s2
            .recv_timeout(Duration::from_secs(5))
            .expect("a call arrived");
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            stalled.run(&mut store).is_empty(),
            "the call whose client timed out must not run"
        );
        while !st2.load(Ordering::SeqCst) {
            if let Some(call) = s2.recv_timeout(Duration::from_millis(20)) {
                call.run(&mut store);
            }
        }
        store
    });
    #[allow(clippy::result_large_err)]
    let capture = || {
        ureq::post(&format!("{}/v1/capture", server.url()))
            .set("Authorization", "Bearer secret-token")
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(5))
            .send_string(&json!({ "text": "one" }).to_string())
    };
    match capture() {
        Err(ureq::Error::Status(504, r)) => {
            let body: Value = serde_json::from_str(&r.into_string().unwrap()).unwrap();
            assert!(
                body["error"]
                    .as_str()
                    .unwrap()
                    .contains("nothing was written"),
                "a 504 says whether the write happened: {body}"
            );
        }
        Ok(r) => panic!("expected 504, got {}", r.status()),
        Err(e) => panic!("{e}"),
    }
    // The retry is the first call the owner actually runs (once it is awake again).
    std::thread::sleep(Duration::from_millis(500));
    ok(capture());
    stop.store(true, Ordering::SeqCst);
    let mut store = owner.join().unwrap();
    let pages = store.list_pages(true).unwrap();
    assert_eq!(pages.len(), 1, "one page: today");
    let st = store.page_state(&pages[0].id).unwrap().clone();
    let texts: Vec<String> = st
        .live_preorder()
        .into_iter()
        .map(|(id, _)| st.block(&id).unwrap().text.clone())
        .collect();
    assert_eq!(texts, ["one"], "the timed-out call wrote nothing");
}

/// AC §4.1 (local automation API): a stalled owner must not let the queue grow without
/// bound. A call that does not fit is refused with 429, which — unlike one that was queued
/// and then abandoned — never runs (docs/state-machines/api-call-inbox.md).
#[test]
fn ac_4_1_a_stalled_owner_refuses_calls_instead_of_queueing_them_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let server = Arc::new(serve("127.0.0.1:0", "secret-token".into()).unwrap());
    let queue = Limits::default().queue.max(1);
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2, p2) = (server.clone(), stop.clone(), path.clone());
    let owner = std::thread::spawn(move || {
        let mut store = Store::open(p2).unwrap();
        store.set_today(NaiveDate::from_ymd_opt(2026, 11, 2));
        // Let every request arrive before draining a single call.
        std::thread::sleep(Duration::from_millis(600));
        while !st2.load(Ordering::SeqCst) {
            if let Some(call) = s2.recv_timeout(Duration::from_millis(20)) {
                call.run(&mut store);
            }
        }
        store
    });
    let mut threads = Vec::new();
    for i in 0..queue + 36 {
        let url = format!("{}/v1/capture", server.url());
        threads.push(std::thread::spawn(move || {
            let body = json!({ "text": format!("n{i}") }).to_string();
            let req = ureq::post(&url)
                .set("Authorization", "Bearer secret-token")
                .set("Content-Type", "application/json")
                .timeout(Duration::from_millis(150));
            match req.send_string(&body) {
                Ok(_) | Err(ureq::Error::Status(504, _)) => None, // queued, then gave up
                Err(ureq::Error::Status(code, r)) => Some((code, r.into_string().unwrap())),
                Err(ureq::Error::Transport(_)) => None, // gave up before an answer
            }
        }));
    }
    let mut refused = Vec::new();
    for t in threads {
        if let Some(r) = t.join().unwrap() {
            refused.push(r);
        }
    }
    assert!(
        !refused.is_empty(),
        "the queue accepted all {} calls",
        queue + 36
    );
    for (code, body) in &refused {
        assert_eq!(*code, 429, "{body}");
        let body: Value = serde_json::from_str(body).unwrap();
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("nothing was written"),
            "{body}"
        );
    }
    stop.store(true, Ordering::SeqCst);
    let mut store = owner.join().unwrap();
    let pages = store.list_pages(true).unwrap();
    let wrote = pages
        .iter()
        .map(|p| store.page_state(&p.id).unwrap().live_preorder().len())
        .sum::<usize>();
    assert!(wrote <= queue, "{wrote} writes from a queue of {queue}");
}

/// AC §4.1: a call the app accepted and then dropped (it stopped before running it) answers
/// the same 503 as one that never got queued: "nothing was written", so a retry is safe.
/// Two readings of "graph is closed" under one status was doc issue 3.
#[test]
fn ac_4_1_a_call_dropped_before_it_ran_answers_not_running() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.utopia");
    let server = Arc::new(serve("127.0.0.1:0", "secret-token".into()).unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let (s2, st2, p2) = (server.clone(), stop.clone(), path.clone());
    let owner = std::thread::spawn(move || {
        let mut store = Store::open(p2).unwrap();
        store.set_today(NaiveDate::from_ymd_opt(2026, 11, 2));
        // A shutting-down owner: take the call, never run it.
        if let Some(call) = s2.recv_timeout(Duration::from_secs(5)) {
            drop(call);
        }
        while !st2.load(Ordering::SeqCst) {
            if let Some(call) = s2.recv_timeout(Duration::from_millis(20)) {
                call.run(&mut store);
            }
        }
        store
    });
    let resp = ureq::post(&format!("{}/v1/capture", server.url()))
        .set("Authorization", "Bearer secret-token")
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(5))
        .send_string(&json!({ "text": "one" }).to_string());
    match resp {
        Err(ureq::Error::Status(503, r)) => {
            let body: Value = serde_json::from_str(&r.into_string().unwrap()).unwrap();
            assert_eq!(body["error"], "the app is not running; nothing was written");
        }
        Ok(r) => panic!("expected 503, got {}", r.status()),
        Err(e) => panic!("{e}"),
    }
    stop.store(true, Ordering::SeqCst);
    let store = owner.join().unwrap();
    assert!(
        store.list_pages(true).unwrap().is_empty(),
        "the dropped call wrote nothing"
    );
}

impl Running {
    /// Stop the owner and hand back its store, to look at what the calls did.
    fn finish(mut self) -> Store {
        self.stop.store(true, Ordering::SeqCst);
        self.owner.take().unwrap().join().unwrap()
    }
}

/// `POST /v1/op` with one op as JSON.
#[allow(clippy::result_large_err)]
fn op(r: &Running, body: Value) -> Result<ureq::Response, ureq::Error> {
    req(r, "POST", "/v1/op").send_string_json(body)
}

/// The status and the `error` text of a refused call.
fn refused(resp: Result<ureq::Response, ureq::Error>) -> (u16, String) {
    match resp {
        Ok(r) => panic!("expected a refusal, got {}", r.status()),
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = serde_json::from_str(&r.into_string().unwrap()).unwrap();
            (code, body["error"].as_str().unwrap_or_default().to_string())
        }
        Err(e) => panic!("{e}"),
    }
}

const OFF: &str = "Assistant access is off for this graph. Turn it on in Utopia: Settings → Assistants. Nothing was read or written.";
const READ_ONLY: &str = "This graph gives assistants read access only. Change it in Utopia: Settings → Assistants. Nothing was written.";

/// The body of a written block, without what differs between two writes of it.
fn shape(mut v: Value) -> Value {
    let o = v.as_object_mut().unwrap();
    o.remove("id");
    o.remove("text");
    v
}

/// MC-3 (docs/mcp-design.md §2.4): every op sent to `/v1/op` gives what its REST route
/// gives, because both run the one `handle` through the one inbox.
#[test]
fn mc_3_each_op_through_v1_op_answers_as_its_rest_route() {
    let r = start();
    r.server.set_level(utopia_api::Level::Write);

    // Writes: the REST route, then the same op through /v1/op on the same graph.
    let a = ok(req(&r, "POST", "/v1/capture").send_string_json(json!({"text": "met [[Dana]]"})));
    let b = ok(op(
        &r,
        json!({"op": "capture", "text": "met [[Dana]] again"}),
    ));
    assert_eq!(b["text"], "met [[Dana]] again");
    assert_eq!(shape(a), shape(b), "capture");

    let a = ok(req(&r, "POST", "/v1/pages/Reading%20list/blocks")
        .send_string_json(json!({"text": "Books", "create": true})));
    let b = ok(op(
        &r,
        json!({"op": "append_block", "title": "Reading list", "text": "Films", "create": true}),
    ));
    assert_eq!(b["text"], "Films");
    let books = a["id"].as_str().unwrap().to_string();
    assert_eq!(shape(a), shape(b), "append_block");

    let a = ok(req(&r, "POST", "/v1/pages/Reading%20list/blocks")
        .send_string_json(json!({"text": "Dune", "parent": books})));
    let b = ok(op(
        &r,
        json!({"op": "append_block", "title": "Reading list", "text": "Emma", "parent": books}),
    ));
    let dune = a["id"].as_str().unwrap().to_string();
    assert_eq!(shape(a), shape(b), "append_block under a parent");

    let a = ok(req(&r, "PATCH", &format!("/v1/blocks/{dune}"))
        .send_string_json(json!({"text": "Dune #toread [[Dana]]"})));
    let b = ok(op(
        &r,
        json!({"op": "update_block", "id": dune, "text": "Dune #toread [[Dana]] #scifi"}),
    ));
    assert_eq!(a["id"], b["id"]);
    assert_eq!(b["text"], "Dune #toread [[Dana]] #scifi");
    assert_eq!(shape(a), shape(b), "update_block");

    // Reads: the same graph state, so the bodies are equal.
    let reads: Vec<(Value, Value)> = vec![
        (
            json!({"op": "list_pages"}),
            ok(req(&r, "GET", "/v1/pages").call()),
        ),
        (
            json!({"op": "list_pages", "daily": true}),
            ok(req(&r, "GET", "/v1/pages?daily=true").call()),
        ),
        (
            json!({"op": "get_page", "title": "Reading list"}),
            ok(req(&r, "GET", "/v1/pages/Reading%20list").call()),
        ),
        (
            json!({"op": "backlinks", "title": "Dana"}),
            ok(req(&r, "GET", "/v1/pages/Dana/backlinks").call()),
        ),
        (
            json!({"op": "get_block", "id": dune}),
            ok(req(&r, "GET", &format!("/v1/blocks/{dune}")).call()),
        ),
        (
            json!({"op": "journal", "date": "today"}),
            ok(req(&r, "GET", "/v1/journal/today").call()),
        ),
        (
            json!({"op": "search", "q": "dune", "limit": 20}),
            ok(req(&r, "GET", "/v1/search?q=dune").call()),
        ),
        (
            json!({"op": "query", "query": "{and: [[Dana]] #toread}", "limit": 100}),
            ok(req(&r, "POST", "/v1/query")
                .send_string_json(json!({"query": "{and: [[Dana]] #toread}"}))),
        ),
    ];
    for (body, want) in reads {
        let got = ok(op(&r, body.clone()));
        assert_eq!(got, want, "{body}");
    }
    // Errors from the op itself keep their status.
    assert_eq!(
        status(op(&r, json!({"op": "get_page", "title": "Nowhere"}))),
        404
    );
    assert_eq!(status(op(&r, json!({"op": "capture", "text": " "}))), 400);
    let mut store = r.finish();
    assert!(store.doctor().unwrap().is_ok());
}

/// MC-1: `/v1/op` passes the same request checks as every other route.
#[test]
fn mc_1_v1_op_needs_the_token_a_loopback_host_and_no_origin() {
    let r = start();
    r.server.set_level(utopia_api::Level::Write);
    let body = json!({"op": "capture", "text": "zebra"}).to_string();
    let url = format!("{}/v1/op", r.server.url());
    let post = || ureq::post(&url).set("Content-Type", "application/json");
    assert_eq!(status(post().send_string(&body)), 401, "no token");
    assert_eq!(
        status(
            post()
                .set("Authorization", "Bearer wrong-token!")
                .send_string(&body)
        ),
        401
    );
    assert_eq!(
        status(
            post()
                .set("Authorization", "Bearer secret-token")
                .set("Origin", "https://evil.example")
                .send_string(&body)
        ),
        403,
        "browsers cannot call it"
    );
    assert_eq!(
        status(
            post()
                .set("Authorization", "Bearer secret-token")
                .set("Host", "evil.example")
                .send_string(&body)
        ),
        403,
        "DNS rebinding"
    );
    // A bad body is still refused for the token first: nothing is parsed before it.
    assert_eq!(status(post().send_string("not json")), 401);
    let store = r.finish();
    assert_eq!(store.search_blocks("zebra", 10).unwrap().len(), 0);
}

/// MC-2: the level decides, by `Op::kind`, which ops `/v1/op` runs.
#[test]
fn mc_2_the_level_permits_ops_by_kind() {
    use utopia_api::Level;
    let r = start();
    let read = json!({"op": "search", "q": "zebra"});
    let write = json!({"op": "capture", "text": "zebra"});
    // (level, read op, write op): Some(text) is a 403 with that text.
    let table = [
        (Level::Off, Some(OFF), Some(OFF)),
        (Level::Read, None, Some(READ_ONLY)),
        (Level::Write, None, None),
    ];
    for (level, read_refusal, write_refusal) in table {
        r.server.set_level(level);
        assert_eq!(r.server.level(), level);
        for (body, refusal) in [(&read, read_refusal), (&write, write_refusal)] {
            match refusal {
                Some(text) => assert_eq!(
                    refused(op(&r, body.clone())),
                    (403, text.to_string()),
                    "{level:?} {body}"
                ),
                None => {
                    ok(op(&r, body.clone()));
                }
            }
        }
    }
    // The REST routes do not follow the level.
    r.server.set_level(Level::Off);
    ok(req(&r, "GET", "/v1/search?q=zebra").call());
    ok(req(&r, "POST", "/v1/capture").send_string_json(json!({"text": "zebra rest"})));
    let store = r.finish();
    // One capture at Write, one through REST at Off.
    assert_eq!(store.search_blocks("zebra", 10).unwrap().len(), 2);
}

/// MC-1: a new server is at Off, so nothing reaches the store through `/v1/op` until the
/// owner sets the level from the access file.
#[test]
fn mc_1_a_new_server_is_off() {
    let r = start();
    assert_eq!(r.server.level(), utopia_api::Level::Off);
    assert_eq!(
        refused(op(&r, json!({"op": "list_pages"}))),
        (403, OFF.to_string())
    );
}

/// MC-2, MC-3: at `read`, a refused write is never queued: the graph is unchanged and
/// `doctor` is clean.
#[test]
fn mc_2_a_refused_write_leaves_the_graph_unchanged() {
    let r = start();
    r.server.set_level(utopia_api::Level::Write);
    let page = ok(op(
        &r,
        json!({"op": "append_block", "title": "Kept", "text": "zebra one", "create": true}),
    ));
    let id = page["id"].as_str().unwrap().to_string();
    r.server.set_level(utopia_api::Level::Read);
    let before = ok(op(&r, json!({"op": "get_page", "title": "Kept"})));
    let pages_before = ok(op(&r, json!({"op": "list_pages", "daily": true})));
    for body in [
        json!({"op": "capture", "text": "zebra two"}),
        json!({"op": "append_block", "title": "Kept", "text": "zebra three"}),
        json!({"op": "append_block", "title": "New page", "text": "zebra four", "create": true}),
        json!({"op": "update_block", "id": id, "text": "zebra changed"}),
    ] {
        assert_eq!(
            refused(op(&r, body.clone())),
            (403, READ_ONLY.to_string()),
            "{body}"
        );
    }
    assert_eq!(
        ok(op(&r, json!({"op": "get_page", "title": "Kept"}))),
        before
    );
    assert_eq!(
        ok(op(&r, json!({"op": "list_pages", "daily": true}))),
        pages_before
    );
    let mut store = r.finish();
    assert_eq!(store.search_blocks("zebra", 10).unwrap().len(), 1);
    assert!(store.find_page("New page").unwrap().is_none());
    assert!(store.doctor().unwrap().is_ok());
}

/// A body that is not an op answers 400 and runs nothing.
#[test]
fn mc_3_a_body_that_is_not_an_op_is_400() {
    let r = start();
    r.server.set_level(utopia_api::Level::Write);
    for body in [
        "not json",
        "",
        "[]",
        r#"{"op": "drop_everything"}"#,
        r#"{"op": "search"}"#,
        r#"{"q": "x"}"#,
        r#"{"op": "capture", "text": 5}"#,
    ] {
        let resp = req(&r, "POST", "/v1/op")
            .set("Content-Type", "application/json")
            .send_string(body);
        assert_eq!(status(resp), 400, "{body:?}");
    }
    let store = r.finish();
    assert!(store.list_pages(true).unwrap().is_empty());
}

/// docs/mcp-design.md §2.2: the discovery file of a graph, written by the owner and read by
/// a client in another process, which then calls the owner.
#[test]
fn mc_4_the_client_calls_the_owner_named_in_the_discovery_file() {
    use utopia_api::{discovery_file, read_discovery, Client, ClientError, Discovery, Op};
    let r = start();
    let dir = tempfile::tempdir().unwrap();
    let graph = dir.path().join("g.utopia");
    std::fs::write(&graph, b"").unwrap();
    let config = dir.path().join("config");
    let f = discovery_file(&config, &graph);
    assert_eq!(
        f,
        config
            .join("api")
            .join(format!("{}.json", utopia_store::graph_key(&graph)))
    );
    r.server.write_discovery(&f).unwrap();
    assert_eq!(
        read_discovery(&f).unwrap(),
        Discovery {
            url: r.server.url(),
            token: "secret-token".into()
        }
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let client = Client::from_discovery(&f).unwrap();
    let b = client.capture("zebra by client").unwrap();
    assert_eq!(b["text"], "zebra by client");
    // The server's status and text arrive unchanged.
    assert_eq!(
        client.op(&Op::ListPages { daily: false }),
        Err(ClientError::Status {
            status: 403,
            message: OFF.into()
        })
    );
    r.server.set_level(utopia_api::Level::Read);
    let s = client
        .op(&Op::Search {
            q: "zebra".into(),
            limit: 5,
        })
        .unwrap();
    assert_eq!(s["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(
        client.capture(" "),
        Err(ClientError::Status {
            status: 400,
            message: "text is empty".into()
        })
    );
    // A stale token is refused before anything is queued.
    let stale = Client::new(Discovery {
        url: r.server.url(),
        token: "old-token".into(),
    });
    assert!(matches!(
        stale.capture("zebra stale"),
        Err(ClientError::Status { status: 401, .. })
    ));
    // Nothing listens at a stale address.
    let addr = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let gone = Client::new(Discovery {
        url: format!("http://{addr}"),
        token: "secret-token".into(),
    });
    assert!(matches!(
        gone.capture("zebra gone"),
        Err(ClientError::NotListening(_))
    ));
    let store = r.finish();
    assert_eq!(store.search_blocks("zebra", 10).unwrap().len(), 1);
}
