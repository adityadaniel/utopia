//! The real Utopia sync client against the Cloudflare Worker relay (PRD §12.8,
//! `relay-cloudflare/`): the client-level scenarios of `tests/http.rs` re-run against an
//! external relay, so the Worker is proven by the app's own transport rather than by the
//! TypeScript tests that mirror it.
//!
//! `relay-cloudflare/scripts/test-against-rust-client.sh` starts `wrangler dev --local`
//! and sets:
//!
//! - `UTOPIA_RELAY_URL` — the Worker's origin (`http://127.0.0.1:<port>`);
//! - `UTOPIA_RELAY_TOKEN` — the bearer token the script gave `RELAY_TOKENS`;
//! - `UTOPIA_RELAY_STATE_DIR` — wrangler's local state (DO SQLite + R2), scanned for
//!   plaintext in the encrypted tests;
//! - `UTOPIA_RELAY_QUOTA` — the byte quota of a second, small-quota run (the quota tests
//!   print a skip in the first run).
//!
//! With `UTOPIA_RELAY_URL` unset every test prints a skip and returns, so
//! `cargo test --workspace` (`make check`) stays green without wrangler.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::{BlockId, PageId};
use utopia_store::Store;
use utopia_sync::http::HttpTransport;
use utopia_sync::{SyncClient, SyncError, SyncStatus, Transport};

struct Relay {
    url: String,
    token: Option<String>,
}

/// The relay under test, or `None` with the reason printed (see the module docs).
fn relay() -> Option<Relay> {
    let url = std::env::var("UTOPIA_RELAY_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())?;
    let token = std::env::var("UTOPIA_RELAY_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    Some(Relay { url, token })
}

macro_rules! relay_or_skip {
    () => {
        match relay() {
            Some(relay) => relay,
            None => {
                eprintln!(
                    "skipped: UTOPIA_RELAY_URL is unset; run \
                     relay-cloudflare/scripts/test-against-rust-client.sh"
                );
                return;
            }
        }
    };
}

/// A graph id no other test or run can share: `[A-Za-z0-9_-]`, well under 128 bytes.
/// (No rand dependency: the tests may only use this crate's existing dev-dependencies.)
fn unique(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{tag}-{nanos:x}-{n:x}-{}", std::process::id())
}

fn device(relay: &Relay, graph: &str) -> (Store, SyncClient<HttpTransport>) {
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let t = HttpTransport::new(&relay.url, graph, relay.token.clone()).unwrap();
    let c = SyncClient::new(t, &s).unwrap();
    (s, c)
}

fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = s.execute_plan(page, plan, None).unwrap().unwrap().block;
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

fn texts(s: &mut Store, page: &PageId) -> Vec<String> {
    let st = s.page_state(page).unwrap().clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, _)| st.block(&id).unwrap().text.clone())
        .collect()
}

fn pages_of(s: &mut Store) -> Vec<(String, Vec<String>)> {
    s.list_pages(true)
        .unwrap()
        .into_iter()
        .map(|p| (p.title.clone(), texts(s, &p.id)))
        .collect()
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Where wrangler keeps the local relay's state: `UTOPIA_RELAY_STATE_DIR`, or the
/// repository's `relay-cloudflare/.wrangler/state` (what `wrangler dev --local` writes by
/// default). `None` for a relay elsewhere, where there is no local state to read.
fn state_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("UTOPIA_RELAY_STATE_DIR") {
        if !dir.trim().is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let default =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../relay-cloudflare/.wrangler/state");
    default.is_dir().then_some(default)
}

/// Every file under wrangler's local state, and how many hold `needle`. A small recursive
/// walk: the tests may not add a dependency for it.
fn scan_state(dir: &Path, needle: &[u8]) -> (usize, Vec<PathBuf>) {
    fn walk(dir: &Path, needle: &[u8], files: &mut usize, hits: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, needle, files, hits);
            } else if let Ok(bytes) = std::fs::read(&path) {
                *files += 1;
                if contains(&bytes, needle) {
                    hits.push(path);
                }
            }
        }
    }
    let mut files = 0;
    let mut hits = Vec::new();
    walk(dir, needle, &mut files, &mut hits);
    (files, hits)
}

/// The state scan the encrypted tests share: the marker must be on the device (or the test
/// would prove nothing) and in no byte wrangler wrote (DO SQLite, its WAL, R2 objects, the
/// local trace store).
fn assert_only_ciphertext(dir: &Path, marker: &str, plaintext_exists: bool) {
    assert!(
        plaintext_exists,
        "the test must first prove the plaintext exists on the device"
    );
    let (files, hits) = scan_state(dir, marker.as_bytes());
    assert!(
        files > 0,
        "no files under {}: wrong state dir?",
        dir.display()
    );
    assert!(
        hits.is_empty(),
        "the relay stored plaintext {marker:?} in {} of {files} file(s): {hits:?}",
        hits.len()
    );
}

#[test]
fn two_machines_converge_over_http() {
    let relay = relay_or_skip!();
    let graph = unique("converge");
    let (mut a, mut ca) = device(&relay, &graph);
    let (mut b, mut cb) = device(&relay, &graph);
    let (page, first) = a
        .open_daily(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap())
        .unwrap();
    a.set_block_text(&page, &first, "shared line", None, None)
        .unwrap();
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &page), vec!["shared line"]);
    // Both edit the same page offline.
    a.set_block_text(&page, &first, "shared line, edited on A", None, None)
        .unwrap();
    write(&mut b, &page, "added on B with [[Topic]]");
    for _ in 0..2 {
        ca.sync_once(&mut a).unwrap();
        cb.sync_once(&mut b).unwrap();
    }
    assert_eq!(pages_of(&mut a), pages_of(&mut b));
    assert_eq!(
        texts(&mut a, &page),
        vec!["shared line, edited on A", "added on B with [[Topic]]"]
    );
    assert!(a.find_page("Topic").unwrap().is_some());
    assert!(a.doctor().unwrap().is_ok() && b.doctor().unwrap().is_ok());
}

#[test]
fn manifest_is_conditional_and_pushes_are_idempotent() {
    let relay = relay_or_skip!();
    let graph = unique("manifest");
    let (mut a, mut ca) = device(&relay, &graph);
    let p = a.create_page("Once").unwrap();
    write(&mut a, &p, "x");
    ca.sync_once(&mut a).unwrap();
    let t = ca.transport_mut();
    let m = t.manifest(None).unwrap().unwrap();
    assert!(
        t.manifest(Some(m.rev)).unwrap().is_none(),
        "304 Not Modified"
    );
    let pull = t.pull(&p, 0).unwrap();
    let dup = utopia_sync::PushRequest {
        updates: vec![utopia_sync::UpdateIn {
            id: pull.updates[0].id.clone(),
            client_seq: 1,
            frontier: String::new(),
            payload: pull.updates[0].payload.clone(),
        }],
    };
    let r = t.push(&p, dup).unwrap();
    assert_eq!((r.accepted, r.duplicate), (0, 1));
}

#[test]
fn auth_is_enforced() {
    let Some(relay) = relay() else {
        eprintln!("skipped: UTOPIA_RELAY_URL is unset");
        return;
    };
    let Some(right) = relay.token.clone() else {
        eprintln!("skipped: UTOPIA_RELAY_TOKEN is unset, so the relay has no auth to enforce");
        return;
    };
    let graph = unique("auth");
    let wrong = Relay {
        url: relay.url.clone(),
        token: Some(format!("{right}-wrong")),
    };
    let (mut a, mut ca) = device(&wrong, &graph);
    a.create_page("Private").unwrap();
    assert!(matches!(ca.sync_once(&mut a), Err(SyncError::Unauthorized)));
    assert_eq!(ca.sync_status(&mut a), SyncStatus::NeedsAuth);
    assert!(
        SyncClient::<HttpTransport>::queued(&mut a).unwrap() > 0,
        "edits stay queued"
    );
    let (mut b, mut cb) = device(&relay, &graph);
    b.create_page("Allowed").unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(SyncClient::<HttpTransport>::queued(&mut b).unwrap(), 0);
}

/// §12.3 step 2: the same new change id twice in one request is accepted once, counted as
/// a duplicate and charged to the quota once.
#[test]
fn ac_12_3_a_change_id_repeated_in_one_batch_is_a_duplicate() {
    let relay = relay_or_skip!();
    let graph = unique("repeat");
    let (mut a, mut ca) = device(&relay, &graph);
    let p = a.create_page("Repeated").unwrap();
    let one = utopia_sync::UpdateIn {
        id: format!("{}:1", unique("client")),
        client_seq: 1,
        frontier: String::new(),
        payload: vec![7u8; 100],
    };
    let t = ca.transport_mut();
    let r = t
        .push(
            &p,
            utopia_sync::PushRequest {
                updates: vec![one.clone(), one],
            },
        )
        .unwrap();
    assert_eq!((r.accepted, r.duplicate), (1, 1));
    assert_eq!(
        t.pull(&p, 0).unwrap().updates.len(),
        1,
        "the payload is stored once"
    );
}

/// §12.6: a batch the relay stored but whose response was lost is re-pushed. Dedupe runs
/// before quota, so its own bytes cannot make it a permanent 413. The quota half of that
/// rule needs a graph whose quota is nearly full, which is the small-quota run
/// (`ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota`); here it is the replay
/// itself, over the Worker and the real client.
#[test]
fn ac_12_6_a_lost_ack_is_replayed_not_rejected() {
    let relay = relay_or_skip!();
    let graph = unique("lostack");
    let mut a = Store::open_in_memory().unwrap();
    a.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = a.create_page("Lost ack").unwrap();
    write(&mut a, &page, &"x".repeat(2000));
    let queued_updates = a.outbox_pending(1000).unwrap().len();
    assert!(queued_updates > 0);
    let mut ca = SyncClient::new(
        LostAck {
            inner: HttpTransport::new(&relay.url, &graph, relay.token.clone()).unwrap(),
            lost: true,
        },
        &a,
    )
    .unwrap();
    let first = ca.sync_once(&mut a);
    assert!(
        first.is_err(),
        "the lost response fails that round: {first:?}"
    );
    // The next round re-pushes the very same batch; the relay answers duplicate.
    let replayed = ca.sync_once(&mut a).unwrap();
    assert!(replayed.failed.is_empty());
    assert_eq!(
        SyncClient::<LostAck>::queued(&mut a).unwrap(),
        0,
        "the replay was acknowledged"
    );
    let mut t = HttpTransport::new(&relay.url, &graph, relay.token.clone()).unwrap();
    assert_eq!(
        t.pull(&page, 0).unwrap().updates.len(),
        queued_updates,
        "the relay stored the batch once, not once per attempt"
    );
    // A device that pulled the batch sees the block once.
    let (mut b, mut cb) = device(&relay, &graph);
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &page), vec!["x".repeat(2000)]);
}

/// A client that hands the push to the relay and then loses the response, as a dropped
/// connection would.
struct LostAck {
    inner: HttpTransport,
    lost: bool,
}

impl Transport for LostAck {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }
    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.inner.manifest(if_none_match)
    }
    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        let resp = self.inner.push(page, req);
        if self.lost {
            self.lost = false;
            return Err(SyncError::Transport(
                "connection reset after the push".into(),
            ));
        }
        resp
    }
    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        self.inner.pull(page, since_seq)
    }
    fn put_snapshot(
        &mut self,
        page: &PageId,
        covers_seq: u64,
        payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        self.inner.put_snapshot(page, covers_seq, payload)
    }
}

/// The quota of the script's small-quota run, and `None` in the default run.
fn small_quota() -> Option<u64> {
    std::env::var("UTOPIA_RELAY_QUOTA")
        .ok()
        .and_then(|q| q.parse().ok())
}

#[test]
fn ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota() {
    let relay = relay_or_skip!();
    let Some(quota) = small_quota() else {
        eprintln!("skipped: UTOPIA_RELAY_QUOTA is unset (the small-quota run sets it)");
        return;
    };
    let graph = unique("replay");
    let (mut a, mut ca) = device(&relay, &graph);
    let p = a.create_page("At the quota").unwrap();
    let first = utopia_sync::UpdateIn {
        id: format!("{}:1", unique("client")),
        client_seq: 1,
        frontier: String::new(),
        payload: vec![7u8; (quota / 2) as usize],
    };
    let t = ca.transport_mut();
    assert_eq!(
        t.push(
            &p,
            utopia_sync::PushRequest {
                updates: vec![first.clone()]
            }
        )
        .unwrap()
        .accepted,
        1
    );
    // Nothing like enough room for a second batch of this size.
    let new = utopia_sync::UpdateIn {
        id: format!("{}:2", unique("client")),
        client_seq: 2,
        frontier: String::new(),
        payload: vec![7u8; (quota * 3 / 4) as usize],
    };
    let err = t
        .push(&p, utopia_sync::PushRequest { updates: vec![new] })
        .unwrap_err();
    assert!(
        matches!(&err, SyncError::Transport(m) if m.contains("413")),
        "{err}"
    );
    // The replay of the batch already stored needs no new space: it is a duplicate, not a
    // permanent 413 (§12.6).
    let again = t
        .push(
            &p,
            utopia_sync::PushRequest {
                updates: vec![first],
            },
        )
        .unwrap();
    assert_eq!((again.accepted, again.duplicate), (0, 1));
}

#[test]
fn quota_rejects_and_keeps_the_outbox() {
    let relay = relay_or_skip!();
    let Some(quota) = small_quota() else {
        eprintln!("skipped: UTOPIA_RELAY_QUOTA is unset (the small-quota run sets it)");
        return;
    };
    let graph = unique("quota");
    let (mut a, mut ca) = device(&relay, &graph);
    let p = a.create_page("Big").unwrap();
    write(&mut a, &p, &"x".repeat(quota as usize * 2));
    let err = ca.sync_once(&mut a).unwrap_err();
    assert!(
        matches!(&err, SyncError::Transport(m) if m.contains("413")),
        "{err}"
    );
    assert!(matches!(ca.sync_status(&mut a), SyncStatus::Offline { queued } if queued > 0));
}

#[test]
fn snapshots_compact_the_log_and_bootstrap_new_devices() {
    let relay = relay_or_skip!();
    let graph = unique("compact");
    let (mut a, mut ca) = device(&relay, &graph);
    let p = a.create_page("Long").unwrap();
    let blk = write(&mut a, &p, "");
    for i in 0..220 {
        a.set_block_text(&p, &blk, &format!("rev {i}"), None, None)
            .unwrap();
        if i % 20 == 0 {
            ca.sync_once(&mut a).unwrap();
        }
    }
    ca.sync_once(&mut a).unwrap();
    ca.sync_once(&mut a).unwrap();
    // The Rust relay asserts the row count through `Db::log_len`; over the Worker the
    // manifest's `tail_bytes` (the bytes left in the log, not the snapshot) is the same
    // proof, and it is what a client sees.
    let m = ca.transport_mut().manifest(None).unwrap().unwrap();
    let entry = m
        .pages
        .iter()
        .find(|page| page.page_id == p.as_str())
        .expect("the page is in the manifest");
    assert!(entry.snapshot_seq > 0, "{entry:?}");
    assert!(
        entry.tail_bytes < 2048,
        "the covered log rows are truncated: {entry:?}"
    );
    let (mut fresh, mut cf) = device(&relay, &graph);
    cf.sync_once(&mut fresh).unwrap();
    assert_eq!(texts(&mut fresh, &p), vec!["rev 219"]);
}

#[test]
fn e2e_encrypted_sync_relay_sees_only_ciphertext() {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let relay = relay_or_skip!();
    let graph = unique("e2e");
    let marker = format!("canary-{}", unique("plaintext"));
    let enc = |s: &Store, pass: &str| {
        let t = HttpTransport::new(&relay.url, &graph, relay.token.clone()).unwrap();
        let key = GraphKey::derive(pass, &graph).unwrap();
        SyncClient::new(Encrypted::new(t, key, &graph), s).unwrap()
    };
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let mut ca = enc(&a, "correct horse battery");
    let mut cb = enc(&b, "correct horse battery");
    let p = a.create_page(&format!("Diary {marker}")).unwrap();
    write(&mut a, &p, &format!("a very secret sentence {marker}"));
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(
        texts(&mut b, &p).len(),
        1,
        "the other device decrypted the page"
    );
    // A device with the wrong passphrase cannot read, and says so.
    let mut c = Store::open_in_memory().unwrap();
    let mut cc = enc(&c, "wrong passphrase");
    let err = cc.sync_once(&mut c).unwrap_err();
    assert!(err.to_string().contains("decryption failed"), "{err}");
    // The relay's own state (DO SQLite, its WAL, R2 objects, the trace store) holds the
    // ciphertext, which is what §11.7 and §15.6 promise a relay operator can see.
    match state_dir() {
        Some(dir) => {
            let seen = texts(&mut b, &p).iter().any(|t| t.contains(&marker));
            assert_only_ciphertext(&dir, &marker, seen);
        }
        None => eprintln!(
            "skipped the ciphertext scan: set UTOPIA_RELAY_STATE_DIR to wrangler's \
             --persist-to directory"
        ),
    }
}

#[test]
fn realtime_notice_on_push() {
    let relay = relay_or_skip!();
    let graph = unique("realtime");
    let (mut a, mut ca) = device(&relay, &graph);
    let p = a.create_page("Live").unwrap();
    let base = relay.url.clone();
    let listen_graph = graph.clone();
    let token = relay.token.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let listener = std::thread::spawn(move || {
        utopia_sync::realtime::listen(&base, &listen_graph, token.as_deref(), |n| {
            tx.send(n).unwrap();
            false
        })
    });
    // No subscribe acknowledgement exists, so a push confirms the subscription: the
    // Durable Object may still have been spinning up when the socket connected.
    let t = ca.transport_mut();
    let mut notice = None;
    for attempt in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(700));
        t.push(
            &p,
            utopia_sync::PushRequest {
                updates: vec![utopia_sync::UpdateIn {
                    id: format!("{}:{attempt}", unique("client")),
                    client_seq: attempt as i64 + 1,
                    frontier: String::new(),
                    payload: vec![1u8; 16],
                }],
            },
        )
        .unwrap();
        match rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(n) => {
                notice = Some(n);
                break;
            }
            Err(_) => continue,
        }
    }
    let notice = notice.expect("the Worker sends a notice for an accepted push");
    assert_eq!(notice.page_id, p.as_str());
    assert!(notice.head_seq >= 1);
    listener.join().unwrap().unwrap();
}

#[test]
fn attachments_sync_encrypted_and_verified() {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let relay = relay_or_skip!();
    let graph = unique("attach");
    let marker = format!("pixels-{}", unique("secret"));
    let dir = tempfile::tempdir().unwrap();
    let enc = |s: &Store| {
        let t = HttpTransport::new(&relay.url, &graph, relay.token.clone()).unwrap();
        let key = GraphKey::derive("pass phrase", &graph).unwrap();
        SyncClient::new(Encrypted::new(t, key, &graph), s).unwrap()
    };
    let mut a = Store::open(dir.path().join("a.utopia")).unwrap();
    let mut b = Store::open(dir.path().join("b.utopia")).unwrap();
    let (mut ca, mut cb) = (enc(&a), enc(&b));
    let photo = format!("\u{89}PNG\r\n\x1a\n{marker}")
        .repeat(500)
        .into_bytes();
    let att = a.add_blob("holiday.png", &photo).unwrap();
    let p = a.create_page("Trip").unwrap();
    write(
        &mut a,
        &p,
        &utopia_core::attach::markup(&att.name, &att.hash, &att.mime),
    );
    let r = ca.sync_once(&mut a).unwrap();
    assert_eq!(r.blobs_up, 1);
    assert!(b.missing_blobs().unwrap().is_empty());
    let r = cb.sync_once(&mut b).unwrap();
    assert_eq!(r.blobs_down, 1, "{r:?}");
    assert_eq!(b.blob(&att.hash).unwrap().unwrap(), photo);
    let got = b.attachment(&att.hash).unwrap().unwrap();
    assert_eq!(
        (got.name.as_str(), got.mime.as_str()),
        ("holiday.png", "image/png")
    );
    assert!(b
        .blob_path(&att.hash)
        .unwrap()
        .starts_with(dir.path().join("b.blobs")));
    // The relay holds ciphertext only (R2 objects included).
    match state_dir() {
        Some(state) => {
            let seen = contains(&photo, marker.as_bytes());
            assert_only_ciphertext(&state, &marker, seen);
        }
        None => eprintln!(
            "skipped the ciphertext scan: set UTOPIA_RELAY_STATE_DIR to wrangler's \
             --persist-to directory"
        ),
    }
    // Nothing more to move.
    let r = ca.sync_once(&mut a).unwrap();
    assert_eq!((r.blobs_up, r.blobs_down), (0, 0));
}

#[test]
fn tampered_blob_is_rejected() {
    let relay = relay_or_skip!();
    let graph = unique("tamper");
    let (mut a, mut ca) = device(&relay, &graph);
    let (mut b, mut cb) = device(&relay, &graph);
    let att = a.add_blob("notes.txt", b"the real notes").unwrap();
    let p = a.create_page("Notes").unwrap();
    write(&mut a, &p, &format!("[notes.txt](blob:{})", att.hash));
    // Someone with relay access swaps the stored bytes before A uploads: write-once means
    // the forged object is what a download returns.
    let mut attacker = HttpTransport::new(&relay.url, &graph, relay.token.clone()).unwrap();
    attacker
        .put_blob(&att.hash, b"forged notes".to_vec())
        .unwrap();
    ca.sync_once(&mut a).unwrap();
    let r = cb.sync_once(&mut b).unwrap();
    assert_eq!(r.blobs_down, 0);
    assert!(
        !b.has_blob(&att.hash),
        "a blob that fails its hash is never stored"
    );
    assert_eq!(b.missing_blobs().unwrap(), vec![att.hash.clone()]);
}
