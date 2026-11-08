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
