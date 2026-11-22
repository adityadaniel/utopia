//! The two relays side by side (PRD §12.3, §12.8): one scripted sequence of raw HTTP
//! requests goes to an in-process `utopia-syncd` and to the Cloudflare Worker, and their
//! answers are compared — the status, the JSON body after masking what legitimately
//! differs (`instance`, `updated_at`), the text body, and the `ETag` and `Content-Type`
//! headers. Request bodies are zstd, as the real client sends them.
//!
//! Every difference must be written down in [`EXCEPTIONS`], with the values each relay
//! answers: a new difference fails until someone records it, and a recorded one that stops
//! happening fails until it is removed. An exception is either one of the deliberate
//! differences in `relay-cloudflare/README.md` (the test checks the README still says so),
//! or a known failure: a real difference nobody has decided on yet, printed on every run.
//!
//! Needs the Worker (`relay-cloudflare/scripts/test-against-rust-client.sh`, which sets
//! `UTOPIA_RELAY_URL`, `UTOPIA_RELAY_TOKEN` and, in its small-quota run,
//! `UTOPIA_RELAY_QUOTA`); without it the tests print a skip, or fail under
//! `UTOPIA_RELAY_REQUIRED=1`.

mod contract;

use std::time::Duration;

use serde_json::{json, Value};
use utopia_syncd::{Config, DEFAULT_QUOTA_BYTES};

use contract::targets::{self, InProcess, WorkerEnv};

/// How the answers of one request are compared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Compare {
    /// Status, `Content-Type`, `ETag` and body.
    Exact,
    /// Status and `Content-Type` only: the message comes from a different parser on each
    /// side (serde vs `JSON.parse`, zstd vs fzstd, axum's query extractor vs a regex).
    Status,
}

enum Body {
    None,
    /// JSON, zstd-compressed with `Content-Encoding: zstd`, as `HttpTransport` sends it.
    Json(Value),
    /// JSON without compression (a body a client may send; both relays accept it).
    PlainJson(Value),
    /// Bytes sent as they are, with `Content-Encoding: zstd` (a malformed frame).
    BadZstd(Vec<u8>),
    /// Text compressed with zstd, for bodies that are not JSON.
    ZstdText(&'static str),
    /// An attachment.
    Octets(Vec<u8>),
    /// An attachment sent chunked, without a `Content-Length`.
    Chunked(Vec<u8>),
}

#[derive(Clone, Copy)]
enum Auth {
    Token,
    Missing,
    Wrong,
    /// `Authorization: bearer <token>`: the scheme is case-sensitive on both relays.
    LowercaseScheme,
}

struct Case {
    name: &'static str,
    method: &'static str,
    /// `{g}` and `{g2}` are replaced with this run's graph ids.
    path: String,
    auth: Auth,
    headers: Vec<(&'static str, &'static str)>,
    body: Body,
    compare: Compare,
}

fn case(name: &'static str, method: &'static str, path: &str) -> Case {
    Case {
        name,
        method,
        path: path.to_string(),
        auth: Auth::Token,
        headers: Vec::new(),
        body: Body::None,
        compare: Compare::Exact,
    }
}

impl Case {
    fn auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }
    fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }
    fn json(mut self, body: Value) -> Self {
        self.body = Body::Json(body);
        self
    }
    fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }
    fn status_only(mut self) -> Self {
        self.compare = Compare::Status;
        self
    }
}

fn b64(bytes: &[u8]) -> String {
    utopia_proto::encode(bytes)
}

/// One update for a push body; `fill` repeated `len` times is the payload.
fn upd(id: &str, client_seq: i64, fill: u8, len: usize) -> Value {
    json!({
        "id": id,
        "client_seq": client_seq,
        "frontier": format!("f-{id}"),
        "payload": b64(&vec![fill; len]),
    })
}

fn push(updates: Vec<Value>) -> Value {
    json!({ "updates": updates })
}

fn snapshot(covers_seq: i64, fill: u8, len: usize) -> Value {
    json!({ "covers_seq": covers_seq, "payload": b64(&vec![fill; len]) })
}

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const HASH_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

/// The default-quota script: every route, its refusals, and the state they leave behind.
fn protocol_script() -> Vec<Case> {
    let upd_path = "/v1/graphs/{g}/pages/page-a/updates";
    let snap_path = "/v1/graphs/{g}/pages/page-a/snapshot";
    let long_id = "g".repeat(129);
    vec![
        case("healthz", "GET", "/healthz").auth(Auth::Missing),
        // Manifest and its ETag.
        case(
            "manifest of an empty graph",
            "GET",
            "/v1/graphs/{g}/manifest",
        ),
        case(
            "manifest, If-None-Match current",
            "GET",
            "/v1/graphs/{g}/manifest",
        )
        .header("If-None-Match", "\"0\""),
        case(
            "manifest, If-None-Match stale",
            "GET",
            "/v1/graphs/{g}/manifest",
        )
        .header("If-None-Match", "\"7\""),
        case("manifest, HEAD", "HEAD", "/v1/graphs/{g}/manifest"),
        // Push: new, duplicate, repeated in one batch, mixed, empty, uncompressed.
        case("push new", "POST", upd_path).json(push(vec![upd("dev:1", 1, 1, 40)])),
        case("push duplicate", "POST", upd_path).json(push(vec![upd("dev:1", 1, 1, 40)])),
        case("push, a change id repeated in one batch", "POST", upd_path)
            .json(push(vec![upd("dev:2", 2, 2, 30), upd("dev:2", 2, 2, 30)])),
        case("push, new and duplicate mixed", "POST", upd_path).json(push(vec![
            upd("dev:3", 3, 3, 20),
            upd("dev:1", 1, 1, 40),
            upd("dev:4", 4, 4, 10),
        ])),
        case("push, no updates", "POST", upd_path).json(push(vec![])),
        case(
            "push, uncompressed body",
            "POST",
            "/v1/graphs/{g}/pages/page-b/updates",
        )
        .body(Body::PlainJson(push(vec![upd("other:1", -5, 9, 8)]))),
        case(
            "push, the same change id to another page",
            "POST",
            "/v1/graphs/{g}/pages/page-b/updates",
        )
        .json(push(vec![upd("dev:4", 4, 4, 10)])),
        case("manifest after pushes", "GET", "/v1/graphs/{g}/manifest"),
        case(
            "manifest, If-None-Match after pushes",
            "GET",
            "/v1/graphs/{g}/manifest",
        )
        .header("If-None-Match", "\"4\""),
        // Pull.
        case("pull since=0", "GET", &format!("{upd_path}?since=0")),
        case("pull without since", "GET", upd_path),
        case("pull since=2", "GET", &format!("{upd_path}?since=2")),
        case(
            "pull since past head",
            "GET",
            &format!("{upd_path}?since=99"),
        ),
        case(
            "pull a page with no log",
            "GET",
            "/v1/graphs/{g}/pages/page-z/updates?since=0",
        ),
        // Snapshots.
        case("snapshot accepted", "POST", snap_path).json(snapshot(2, 0x5a, 64)),
        case("snapshot not newer (no-op)", "POST", snap_path).json(snapshot(2, 0x5b, 64)),
        case("snapshot older (no-op)", "POST", snap_path).json(snapshot(1, 0x5c, 64)),
        case("snapshot covering unknown updates", "POST", snap_path).json(snapshot(99, 1, 4)),
        case(
            "snapshot of a page with no log",
            "POST",
            "/v1/graphs/{g}/pages/page-c/snapshot",
        )
        .json(snapshot(0, 0x5d, 16)),
        case(
            "pull since=0, before the snapshot",
            "GET",
            &format!("{upd_path}?since=0"),
        ),
        case(
            "pull since=1, before the snapshot",
            "GET",
            &format!("{upd_path}?since=1"),
        ),
        case(
            "pull since=2, at the snapshot",
            "GET",
            &format!("{upd_path}?since=2"),
        ),
        case(
            "pull since=3, after the snapshot",
            "GET",
            &format!("{upd_path}?since=3"),
        ),
        case(
            "pull the snapshot-only page",
            "GET",
            "/v1/graphs/{g}/pages/page-c/updates",
        ),
        case("manifest after snapshots", "GET", "/v1/graphs/{g}/manifest"),
        case("push after the snapshot", "POST", upd_path).json(push(vec![upd("dev:5", 5, 5, 12)])),
        case(
            "pull after the snapshot and a push",
            "GET",
            &format!("{upd_path}?since=3"),
        ),
        // A change id a snapshot truncated is forgotten (both relays dedupe on the log).
        case("push a change id the snapshot truncated", "POST", upd_path)
            .json(push(vec![upd("dev:1", 1, 1, 40)])),
        // Dedupe across graphs: a change id another graph still holds in its log.
        case(
            "push to a second graph, a change id the first graph stored",
            "POST",
            "/v1/graphs/{g2}/pages/page-a/updates",
        )
        .json(push(vec![upd("dev:4", 4, 4, 10)])),
        case(
            "manifest of the second graph",
            "GET",
            "/v1/graphs/{g2}/manifest",
        ),
        // Blobs.
        case(
            "blob PUT new",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_A}"),
        )
        .body(Body::Octets(b"attachment bytes".to_vec())),
        case(
            "blob PUT repeated",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_A}"),
        )
        .body(Body::Octets(b"attachment bytes".to_vec())),
        case(
            "blob PUT, other bytes under a stored hash",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_A}"),
        )
        .body(Body::Octets(b"forged bytes".to_vec())),
        case(
            "blob PUT empty",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_C}"),
        )
        .body(Body::Octets(Vec::new())),
        case(
            "blob PUT, chunked",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_D}"),
        )
        .body(Body::Chunked(b"chunked attachment".repeat(100))),
        case(
            "blob GET, uploaded chunked",
            "GET",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_D}"),
        ),
        case(
            "blob GET present",
            "GET",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_A}"),
        ),
        case(
            "blob GET empty",
            "GET",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_C}"),
        ),
        case(
            "blob GET missing",
            "GET",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_B}"),
        ),
        case("manifest after blobs", "GET", "/v1/graphs/{g}/manifest"),
        // Validation.
        case("bad graph id", "GET", "/v1/graphs/bad.id/manifest"),
        case(
            "graph id of 129 characters",
            "GET",
            &format!("/v1/graphs/{long_id}/manifest"),
        ),
        case("empty graph id", "GET", "/v1/graphs//manifest"),
        case(
            "bad page id, pull",
            "GET",
            "/v1/graphs/{g}/pages/bad.page/updates",
        ),
        case(
            "bad page id, push",
            "POST",
            "/v1/graphs/{g}/pages/bad.page/updates",
        )
        .json(push(vec![upd("dev:9", 9, 9, 4)])),
        case(
            "bad page id, snapshot",
            "POST",
            "/v1/graphs/{g}/pages/bad.page/snapshot",
        )
        .json(snapshot(0, 1, 4)),
        case(
            "bad hash, uppercase",
            "GET",
            &format!("/v1/graphs/{{g}}/blobs/{}", HASH_B.to_uppercase()),
        ),
        case("bad hash, short", "PUT", "/v1/graphs/{g}/blobs/abc123")
            .body(Body::Octets(b"x".to_vec())),
        // Auth.
        case("missing token", "GET", "/v1/graphs/{g}/manifest").auth(Auth::Missing),
        case("wrong token", "GET", "/v1/graphs/{g}/manifest").auth(Auth::Wrong),
        case("lowercase bearer scheme", "GET", "/v1/graphs/{g}/manifest")
            .auth(Auth::LowercaseScheme),
        case("missing token, push", "POST", upd_path)
            .auth(Auth::Missing)
            .json(push(vec![upd("dev:9", 9, 9, 4)])),
        case(
            "wrong token, blob GET",
            "GET",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_A}"),
        )
        .auth(Auth::Wrong),
        case(
            "missing token, bad graph id",
            "GET",
            "/v1/graphs/bad.id/manifest",
        )
        .auth(Auth::Missing),
        // Methods and routes.
        case(
            "wrong method, DELETE manifest",
            "DELETE",
            "/v1/graphs/{g}/manifest",
        ),
        case("wrong method, GET snapshot", "GET", snap_path),
        case("wrong method, PUT updates", "PUT", upd_path).json(push(vec![])),
        case(
            "wrong method, POST blob",
            "POST",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_A}"),
        )
        .body(Body::Octets(b"x".to_vec())),
        case(
            "unknown route under a graph",
            "GET",
            "/v1/graphs/{g}/nothing",
        ),
        case("unknown version", "GET", "/v2/graphs/{g}/manifest"),
        case("root", "GET", "/"),
        case("trailing slash", "GET", "/v1/graphs/{g}/manifest/"),
        // Bodies the relays refuse; the messages come from different parsers.
        case("malformed zstd", "POST", upd_path)
            .body(Body::BadZstd(b"definitely not a zstd frame".to_vec()))
            .status_only(),
        case("malformed JSON", "POST", upd_path)
            .body(Body::ZstdText("{\"updates\": ["))
            .status_only(),
        case("push, a missing field", "POST", upd_path)
            .json(json!({ "updates": [{ "id": "dev:9", "client_seq": 9, "frontier": "" }] }))
            .status_only(),
        case("push, updates not an array", "POST", upd_path)
            .json(json!({ "updates": "nope" }))
            .status_only(),
        case("push, payload not base64", "POST", upd_path)
            .json(json!({ "updates": [{
                "id": "dev:9", "client_seq": 9, "frontier": "", "payload": "!!!"
            }] }))
            .status_only(),
        case("push, client_seq not an integer", "POST", upd_path)
            .json(json!({ "updates": [{
                "id": "dev:9", "client_seq": 1.5, "frontier": "", "payload": ""
            }] }))
            .status_only(),
        case("snapshot, a missing field", "POST", snap_path)
            .json(json!({ "covers_seq": 1 }))
            .status_only(),
        case("snapshot, negative covers_seq", "POST", snap_path)
            .json(json!({ "covers_seq": -1, "payload": "" }))
            .status_only(),
        // `since` is a u64 in Rust.
        case(
            "since not a number",
            "GET",
            &format!("{upd_path}?since=abc"),
        )
        .status_only(),
        case("since negative", "GET", &format!("{upd_path}?since=-1")).status_only(),
        case("since empty", "GET", &format!("{upd_path}?since=")).status_only(),
        case(
            "since above 2^53",
            "GET",
            &format!("{upd_path}?since=9007199254740993"),
        ),
        case(
            "since above i64::MAX",
            "GET",
            &format!("{upd_path}?since=9223372036854775808"),
        ),
        // The realtime route, without the upgrade.
        case("events, not an upgrade", "GET", "/v1/graphs/{g}/events").status_only(),
        case(
            "events, not an upgrade, no token",
            "GET",
            "/v1/graphs/{g}/events",
        )
        .auth(Auth::Missing)
        .status_only(),
        // Nothing above changed the state the two relays end in.
        case("final manifest", "GET", "/v1/graphs/{g}/manifest"),
        case("final pull", "GET", &format!("{upd_path}?since=0")),
    ]
}

/// The small-quota script: run against both relays with the Worker's quota. Sizes are
/// fractions of the quota, so any small quota works.
fn quota_script(quota: usize) -> Vec<Case> {
    let upd_path = "/v1/graphs/{g}/pages/page-a/updates";
    let big = quota * 3 / 4;
    vec![
        case("push most of the quota", "POST", upd_path).json(push(vec![upd("q:1", 1, 1, big)])),
        case("push over the quota", "POST", upd_path).json(push(vec![upd("q:2", 2, 2, quota / 2)])),
        case("replay at the quota", "POST", upd_path).json(push(vec![upd("q:1", 1, 1, big)])),
        case("replay plus a small new update", "POST", upd_path).json(push(vec![
            upd("q:1", 1, 1, big),
            upd("q:3", 3, 3, quota / 32),
        ])),
        case(
            "blob over the quota",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_A}"),
        )
        .body(Body::Octets(vec![7; quota / 2])),
        case(
            "blob within the quota",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_B}"),
        )
        .body(Body::Octets(vec![8; quota / 16])),
        case(
            "blob replay at the quota",
            "PUT",
            &format!("/v1/graphs/{{g}}/blobs/{HASH_B}"),
        )
        .body(Body::Octets(vec![8; quota / 16])),
        case("manifest at the quota", "GET", "/v1/graphs/{g}/manifest"),
        // Snapshots are not quota-checked, so a client can always compact.
        case(
            "snapshot larger than the room left",
            "POST",
            "/v1/graphs/{g}/pages/page-a/snapshot",
        )
        .json(snapshot(2, 9, big)),
        case("push after compaction, over", "POST", upd_path).json(push(vec![upd(
            "q:4",
            4,
            4,
            quota / 2,
        )])),
        case("push after compaction, within", "POST", upd_path).json(push(vec![upd(
            "q:5",
            5,
            5,
            quota / 32,
        )])),
        case("final manifest", "GET", "/v1/graphs/{g}/manifest"),
    ]
}

/// One relay's answer, as compared.
#[derive(Debug)]
struct Answer {
    status: u16,
    content_type: Option<String>,
    etag: Option<String>,
    body: Vec<u8>,
}

struct Target<'a> {
    base: &'a str,
    token: &'a str,
    graphs: (&'a str, &'a str),
}

fn send(agent: &ureq::Agent, target: &Target, c: &Case) -> Answer {
    let path = c
        .path
        .replace("{g2}", target.graphs.1)
        .replace("{g}", target.graphs.0);
    let url = format!("{}{path}", target.base);
    let mut req = agent.request(c.method, &url);
    req = match c.auth {
        Auth::Token => req.set("Authorization", &format!("Bearer {}", target.token)),
        Auth::Missing => req,
        Auth::Wrong => req.set("Authorization", &format!("Bearer {}-wrong", target.token)),
        Auth::LowercaseScheme => req.set("Authorization", &format!("bearer {}", target.token)),
    };
    for (name, value) in &c.headers {
        req = req.set(name, value);
    }
    let zstd = |bytes: &[u8]| zstd::encode_all(bytes, 3).unwrap();
    let result = match &c.body {
        Body::None => req.call(),
        Body::Json(v) => req
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "zstd")
            .send_bytes(&zstd(&serde_json::to_vec(v).unwrap())),
        Body::PlainJson(v) => req
            .set("Content-Type", "application/json")
            .send_bytes(&serde_json::to_vec(v).unwrap()),
        Body::BadZstd(bytes) => req
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "zstd")
            .send_bytes(bytes),
        Body::ZstdText(text) => req
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "zstd")
            .send_bytes(&zstd(text.as_bytes())),
        Body::Octets(bytes) => req
            .set("Content-Type", "application/octet-stream")
            .send_bytes(bytes),
        Body::Chunked(bytes) => req
            .set("Content-Type", "application/octet-stream")
            .set("Transfer-Encoding", "chunked")
            .send(std::io::Cursor::new(bytes.clone())),
    };
    let resp = match result {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(e)) => panic!("{} {url}: {e}", c.method),
    };
    let header = |name: &str| resp.header(name).map(str::to_string);
    let (status, content_type, etag) = (resp.status(), header("Content-Type"), header("ETag"));
    let mut body = Vec::new();
    // A HEAD answer announces the GET's length and carries no body, and neither do 204 and
    // 304 (RFC 9110 §6.4.1); ureq 2 would wait for one that never comes.
    if c.method != "HEAD" && status != 204 && status != 304 {
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut body)
            .unwrap_or_else(|e| panic!("{:?}: reading the body of {url}: {e}", c.name));
    }
    Answer {
        status,
        content_type,
        etag,
        body,
    }
}

/// What may differ between two correct relays: the store's random id and the receive
/// times. Both must be there on both sides, so a missing one is still a difference.
fn mask(v: &mut Value) {
    if let Some(obj) = v.as_object_mut() {
        if let Some(Value::String(s)) = obj.get_mut("instance") {
            let ok = s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit());
            *s = if ok {
                "<instance>"
            } else {
                "<malformed instance>"
            }
            .into();
        }
        if let Some(Value::Array(pages)) = obj.get_mut("pages") {
            for page in pages {
                if let Some(at) = page.get_mut("updated_at") {
                    if at.as_i64().is_some_and(|t| t > 0) {
                        *at = json!("<ms>");
                    }
                }
            }
        }
    }
}

fn render_body(a: &Answer) -> String {
    let json = a
        .content_type
        .as_deref()
        .is_some_and(|t| t.starts_with("application/json"));
    if json {
        if let Ok(mut v) = serde_json::from_slice::<Value>(&a.body) {
            mask(&mut v);
            return v.to_string();
        }
    }
    match std::str::from_utf8(&a.body) {
        Ok(text) => format!("{text:?}"),
        Err(_) => format!("<{} bytes>", a.body.len()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Aspect {
    Status,
    ContentType,
    ETag,
    Body,
}

#[derive(Debug)]
struct Difference {
    script: &'static str,
    case: &'static str,
    aspect: Aspect,
    rust: String,
    worker: String,
}

fn compare(script: &'static str, c: &Case, rust: &Answer, worker: &Answer) -> Vec<Difference> {
    let opt = |s: &Option<String>| s.clone().unwrap_or_else(|| "<none>".into());
    let mut aspects = vec![
        (
            Aspect::Status,
            rust.status.to_string(),
            worker.status.to_string(),
        ),
        (
            Aspect::ContentType,
            opt(&rust.content_type),
            opt(&worker.content_type),
        ),
    ];
    if c.compare == Compare::Exact {
        aspects.push((Aspect::ETag, opt(&rust.etag), opt(&worker.etag)));
        aspects.push((Aspect::Body, render_body(rust), render_body(worker)));
    }
    aspects
        .into_iter()
        .filter(|(_, r, w)| r != w)
        .map(|(aspect, rust, worker)| Difference {
            script,
            case: c.name,
            aspect,
            rust,
            worker,
        })
        .collect()
}

/// The deliberate differences `relay-cloudflare/README.md` lists ("Differences from
/// `utopia-syncd`, and why"), each by the start of its bold title, with what this test makes
/// of it. A README item missing here, or one here the README no longer has, fails
/// [`every_readme_difference_is_classified`], so a difference written down is one the test
/// knows about.
const README_DIFFERENCES: &[(&str, &str)] = &[
    (
        "An empty token list means",
        "configuration, not wire: both relays run with a token here; \
         tests/http.rs an_empty_token_list_disables_auth and the Worker's unit tests pin each side",
    ),
    (
        "No `graph_id` columns",
        "storage layout; but see the known failure on dedupe across graphs",
    ),
    (
        "Attachments live in R2",
        "storage; the blob cases answer the same",
    ),
    (
        "Payloads are chunked",
        "storage; invisible on the wire (test/large.test.ts)",
    ),
    (
        "Snapshots are not quota-checked",
        "the same on both relays: \"snapshot larger than the room left\"",
    ),
    (
        "No hash check on blobs",
        "the same on both relays: \"blob PUT, other bytes under a stored hash\"",
    ),
    (
        "The token is neither forwarded nor stored",
        "storage and the Worker's internal request; not on the wire",
    ),
    (
        "Attachment uploads stream",
        "the same answers with and without a Content-Length (\"blob PUT, chunked\"); a body \
         that is not the length it declares cannot be sent with ureq",
    ),
];

/// Why a difference is allowed.
enum Why {
    /// One of [`README_DIFFERENCES`], by its title.
    #[allow(dead_code)] // none of them shows on the wire today
    Readme(&'static str),
    /// A real difference nobody has decided on: printed on every run, kept honest by the
    /// exact values, and failing once it disappears (fixed, so remove the entry).
    KnownFailure(&'static str),
}

/// A difference allowed for one case: each `(aspect, utopia-syncd, Worker)` as rendered.
struct Exception {
    script: &'static str,
    case: &'static str,
    diffs: &'static [(Aspect, &'static str, &'static str)],
    why: Why,
}

const P: &str = "differential_protocol";
const TEXT: &str = "text/plain; charset=utf-8";

const ETAG_ON_304: Why = Why::KnownFailure(
    "utopia-syncd's 304 carries no ETag; the Worker repeats it, as RFC 9110 §15.4.5 asks. \
     The client reads `rev` from a 200's body and ignores the header, so nothing breaks.",
);
const AXUM_DEFAULT_BODIES: Why = Why::KnownFailure(
    "axum's own 404 and 405 (an unknown route, a wrong method) and get_blob's bare 404 have \
     an empty body and no Content-Type; the Worker writes `not found` / `method not allowed` \
     as text/plain. The status codes agree, and the client reads only the status.",
);
const NOT_FOUND: &[(Aspect, &str, &str)] = &[
    (Aspect::ContentType, "<none>", TEXT),
    (Aspect::Body, "\"\"", "\"not found\""),
];
const NOT_ALLOWED: &[(Aspect, &str, &str)] = &[
    (Aspect::ContentType, "<none>", TEXT),
    (Aspect::Body, "\"\"", "\"method not allowed\""),
];
const SINCE_ABOVE_2_53: Why = Why::KnownFailure(
    "the Worker refuses a `since` a JavaScript number cannot hold exactly (above 2^53 - 1) \
     with 400; utopia-syncd takes any u64. No real seq comes near 2^53.",
);
const SINCE_WRAPS: Why = Why::KnownFailure(
    "utopia-syncd binds `since as i64`, so a `since` above i64::MAX wraps negative and the \
     pull answers every update in the log (and no snapshot); the Worker answers 400. A \
     utopia-syncd bug the real client cannot reach.",
);
const EVENTS_AUTH_ORDER: Why = Why::KnownFailure(
    "utopia-syncd's WebSocketUpgrade extractor refuses a plain GET (400) before the handler \
     checks the token; the Worker checks the token first (401). Only the order of two \
     refusals differs.",
);
const DEDUPE_ACROSS_GRAPHS: Why = Why::KnownFailure(
    "utopia-syncd dedupes change ids across every graph it holds (`page_log.change_id` is \
     UNIQUE and the existence check has no graph_id), so a push to graph B of an id graph A \
     still has in its log is answered `duplicate` and not stored; the Worker dedupes per graph \
     (one Durable Object each) and stores it. Change ids are `<client_id>:<client_seq>` and \
     the client id travels with a copied graph file, so a copy synced under a new graph id \
     to the same utopia-syncd can have edits dropped this way.",
);

/// Every difference the scripts are allowed to find, with the exact values both relays
/// answer. `UTOPIA_DIFF_TRACE=1` prints every answer when one needs updating.
const EXCEPTIONS: &[Exception] = &[
    Exception {
        script: P,
        case: "manifest, If-None-Match current",
        diffs: &[(Aspect::ETag, "<none>", "\"0\"")],
        why: ETAG_ON_304,
    },
    Exception {
        script: P,
        case: "manifest, If-None-Match after pushes",
        diffs: &[(Aspect::ETag, "<none>", "\"4\"")],
        why: ETAG_ON_304,
    },
    Exception {
        script: P,
        case: "push to a second graph, a change id the first graph stored",
        diffs: &[(
            Aspect::Body,
            r#"{"accepted":0,"duplicate":1,"head_seq":0}"#,
            r#"{"accepted":1,"duplicate":0,"head_seq":1}"#,
        )],
        why: DEDUPE_ACROSS_GRAPHS,
    },
    Exception {
        script: P,
        case: "manifest of the second graph",
        diffs: &[
            (Aspect::ETag, "\"0\"", "\"1\""),
            (
                Aspect::Body,
                r#"{"instance":"<instance>","pages":[],"protocol_version":1,"rev":0}"#,
                r#"{"instance":"<instance>","pages":[{"deleted":false,"frontier":"f-dev:4","head_seq":1,"page_id":"page-a","snapshot_seq":0,"tail_bytes":10,"updated_at":"<ms>"}],"protocol_version":1,"rev":1}"#,
            ),
        ],
        why: DEDUPE_ACROSS_GRAPHS,
    },
    Exception {
        script: P,
        case: "blob GET missing",
        diffs: NOT_FOUND,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "wrong method, DELETE manifest",
        diffs: NOT_ALLOWED,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "wrong method, GET snapshot",
        diffs: NOT_ALLOWED,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "wrong method, PUT updates",
        diffs: NOT_ALLOWED,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "wrong method, POST blob",
        diffs: NOT_ALLOWED,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "unknown route under a graph",
        diffs: NOT_FOUND,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "unknown version",
        diffs: NOT_FOUND,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "root",
        diffs: NOT_FOUND,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "trailing slash",
        diffs: NOT_FOUND,
        why: AXUM_DEFAULT_BODIES,
    },
    Exception {
        script: P,
        case: "since above 2^53",
        diffs: &[
            (Aspect::Status, "200", "400"),
            (Aspect::ContentType, "application/json", TEXT),
            (
                Aspect::Body,
                r#"{"head_seq":6,"updates":[]}"#,
                r#""invalid since \"9007199254740993\"""#,
            ),
        ],
        why: SINCE_ABOVE_2_53,
    },
    Exception {
        script: P,
        case: "since above i64::MAX",
        diffs: &[
            (Aspect::Status, "200", "400"),
            (Aspect::ContentType, "application/json", TEXT),
            (
                Aspect::Body,
                r#"{"head_seq":6,"updates":[{"frontier":"f-dev:3","id":"dev:3","payload":"AwMDAwMDAwMDAwMDAwMDAwMDAwM=","seq":3},{"frontier":"f-dev:4","id":"dev:4","payload":"BAQEBAQEBAQEBA==","seq":4},{"frontier":"f-dev:5","id":"dev:5","payload":"BQUFBQUFBQUFBQUF","seq":5},{"frontier":"f-dev:1","id":"dev:1","payload":"AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQ==","seq":6}]}"#,
                r#""invalid since \"9223372036854775808\"""#,
            ),
        ],
        why: SINCE_WRAPS,
    },
    Exception {
        script: P,
        case: "events, not an upgrade, no token",
        diffs: &[(Aspect::Status, "400", "401")],
        why: EVENTS_AUTH_ORDER,
    },
];

/// The numbered items' bold titles under the README's "Differences from" heading.
fn readme_differences() -> Vec<String> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../relay-cloudflare/README.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut titles = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("## ") {
            inside = line.starts_with("## Differences from");
            continue;
        }
        let item = line.trim_start_matches(|c: char| c.is_ascii_digit());
        if inside && item.len() < line.len() {
            if let Some(rest) = item.strip_prefix(". **") {
                titles.push(rest.split("**").next().unwrap_or_default().to_string());
            }
        }
    }
    titles
}

/// Runs in every `cargo test`: the README's list of deliberate differences and
/// [`README_DIFFERENCES`] name the same items.
#[test]
fn every_readme_difference_is_classified() {
    let titles = readme_differences();
    assert!(
        !titles.is_empty(),
        "relay-cloudflare/README.md has no numbered list under \"## Differences from\""
    );
    let unclassified: Vec<_> = titles
        .iter()
        .filter(|t| !README_DIFFERENCES.iter().any(|(key, _)| t.starts_with(key)))
        .collect();
    assert!(
        unclassified.is_empty(),
        "relay-cloudflare/README.md lists differences crates/utopia-syncd/tests/differential.rs \
         does not classify; add them to README_DIFFERENCES (and to EXCEPTIONS if they show on \
         the wire): {unclassified:?}"
    );
    let gone: Vec<_> = README_DIFFERENCES
        .iter()
        .filter(|(key, _)| !titles.iter().any(|t| t.starts_with(key)))
        .map(|(key, _)| key)
        .collect();
    assert!(
        gone.is_empty(),
        "README_DIFFERENCES in crates/utopia-syncd/tests/differential.rs names differences \
         relay-cloudflare/README.md no longer lists: {gone:?}"
    );
    for e in EXCEPTIONS {
        if let Why::Readme(key) = e.why {
            assert!(
                README_DIFFERENCES.iter().any(|(k, _)| *k == key),
                "{:?} cites README difference {key:?}, which README_DIFFERENCES does not have",
                e.case
            );
        }
    }
}

/// Run `script` against both relays and hold the differences to [`EXCEPTIONS`].
fn run(name: &'static str, script: Vec<Case>, worker: &WorkerEnv, quota: u64) {
    let token = worker.token.clone().or_else(|| {
        targets::unavailable::<String>("UTOPIA_RELAY_TOKEN is unset, and both relays need one")
    });
    let Some(token) = token else { return };
    let rust = InProcess::start(Config {
        tokens: vec![token.clone()],
        quota_bytes: quota,
    });
    let graphs = (contract::unique("diff"), contract::unique("diff2"));
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .redirects(0)
        .build();
    let rust_target = Target {
        base: &rust.base,
        token: &token,
        graphs: (&graphs.0, &graphs.1),
    };
    let worker_target = Target {
        base: &worker.url,
        token: &token,
        graphs: (&graphs.0, &graphs.1),
    };
    // `UTOPIA_DIFF_TRACE=1` prints both answers to every request.
    let trace = std::env::var("UTOPIA_DIFF_TRACE").is_ok_and(|v| v == "1");
    let mut found = Vec::new();
    for c in &script {
        let r = send(&agent, &rust_target, c);
        let w = send(&agent, &worker_target, c);
        if trace {
            eprintln!(
                "[{name}] {:?}\n  utopia-syncd {} {}\n  Worker       {} {}",
                c.name,
                r.status,
                render_body(&r),
                w.status,
                render_body(&w)
            );
        }
        found.extend(compare(name, c, &r, &w));
    }

    let mut seen = std::collections::HashSet::new();
    let mut noted = std::collections::HashSet::new();
    let mut unexplained = Vec::new();
    for d in &found {
        let hit = EXCEPTIONS.iter().enumerate().find_map(|(i, e)| {
            (e.script == d.script && e.case == d.case)
                .then(|| {
                    e.diffs.iter().position(|(aspect, rust, worker)| {
                        *aspect == d.aspect && *rust == d.rust && *worker == d.worker
                    })
                })
                .flatten()
                .map(|j| (i, j))
        });
        match hit {
            Some((i, j)) => {
                seen.insert((i, j));
                if let Why::KnownFailure(note) = EXCEPTIONS[i].why {
                    if noted.insert(i) {
                        eprintln!("KNOWN FAILURE [{name}] {:?}: {note}", d.case);
                    }
                    eprintln!(
                        "  {:?}: utopia-syncd {} / Worker {}",
                        d.aspect, d.rust, d.worker
                    );
                }
            }
            None => unexplained.push(format!(
                "  {:?} {:?}: utopia-syncd {} / Worker {}",
                d.case, d.aspect, d.rust, d.worker
            )),
        }
    }
    let stale: Vec<String> = EXCEPTIONS
        .iter()
        .enumerate()
        .filter(|(_, e)| e.script == name)
        .flat_map(|(i, e)| {
            let seen = &seen;
            e.diffs
                .iter()
                .enumerate()
                .filter(move |(j, _)| !seen.contains(&(i, *j)))
                .map(move |(_, (aspect, _, _))| format!("{:?} {aspect:?}", e.case))
        })
        .collect();
    assert!(
        unexplained.is_empty(),
        "[{name}] the relays answer differently, and EXCEPTIONS in \
         crates/utopia-syncd/tests/differential.rs does not record it (fix a relay, or \
         record the difference there and, if it is deliberate, in relay-cloudflare/README.md):\n{}",
        unexplained.join("\n")
    );
    assert!(
        stale.is_empty(),
        "[{name}] recorded differences no longer happen (or answer other values); update \
         EXCEPTIONS: {stale:?}"
    );
}

#[test]
fn differential_protocol() {
    let Some(worker) = targets::worker_env() else {
        return;
    };
    if let Some(q) = worker.quota {
        targets::unavailable::<()>(&format!(
            "the relay runs with a {q}-byte quota (UTOPIA_RELAY_QUOTA); this script needs \
             the default"
        ));
        return;
    }
    run(
        "differential_protocol",
        protocol_script(),
        &worker,
        DEFAULT_QUOTA_BYTES,
    );
}

#[test]
fn differential_quota() {
    let Some(worker) = targets::worker_env() else {
        return;
    };
    let Some(quota) = worker.quota else {
        targets::unavailable::<()>(
            "this script needs a small-quota relay, and UTOPIA_RELAY_QUOTA is unset (the \
             script's second run sets it)",
        );
        return;
    };
    run(
        "differential_quota",
        quota_script(quota as usize),
        &worker,
        quota,
    );
}
