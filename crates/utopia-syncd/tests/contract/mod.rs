//! The relay contract (PRD §12.3, §12.8): the client-level scenarios every relay that speaks
//! the v1 protocol must satisfy, written once against [`RelayUnderTest`].
//!
//! - `tests/http.rs` runs the suite against an in-process `utopia-syncd`, always, as part of
//!   `cargo test --workspace`.
//! - `tests/cloudflare.rs` runs the same list against the Cloudflare Worker
//!   (`relay-cloudflare/`, under `wrangler dev --local`) when `UTOPIA_RELAY_URL` names one.
//!
//! A scenario added to [`contract_suite!`] runs against both, with nothing to copy. Scenarios
//! that only one relay can stage (a wiped relay database, a relay that is down, the folder
//! transport, LAN discovery) stay in `tests/http.rs`.
//!
//! Every scenario that needs a small quota has `quota` in its name and every other one does
//! not: `relay-cloudflare/scripts/test-against-rust-client.sh` runs the Worker twice (a
//! default and a small quota) and selects by that word. [`start`] enforces the rule, so a
//! misnamed scenario fails here rather than being skipped there.

// Each test binary that includes this module uses a different part of it.
#![allow(dead_code)]

pub mod targets;

use std::sync::atomic::{AtomicU64, Ordering};

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::{BlockId, PageId};
use utopia_store::Store;
use utopia_sync::http::HttpTransport;
use utopia_sync::{PushRequest, SyncClient, SyncError, SyncStatus, Transport, UpdateIn};

/// The quota of a small-quota relay: the in-process one and the script's second Worker run.
pub const SMALL_QUOTA: u64 = 4096;

/// What a scenario needs from the relay it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The default quota (1 GiB on both relays).
    Default,
    /// A per-graph quota small enough to fill in a test ([`RelayUnderTest::quota`]).
    SmallQuota,
}

/// What a storage scan found: how many stored items it read, and which held the needle.
pub struct Scan {
    pub inspected: usize,
    pub hits: Vec<String>,
}

/// One relay the suite runs against.
pub trait RelayUnderTest {
    /// The relay's origin, `http://127.0.0.1:<port>`.
    fn url(&self) -> &str;
    /// The bearer token it accepts; `None` when it was started without auth.
    fn token(&self) -> Option<&str>;
    /// The per-graph quota, when it is a small one ([`Need::SmallQuota`]).
    fn quota(&self) -> Option<u64>;
    /// Every byte the relay stored (logs, snapshots, attachments), searched for `needle`.
    /// `None` when this relay's storage cannot be read from the test.
    fn scan_storage(&self, needle: &[u8]) -> Option<Scan>;
    /// Log rows stored for a page; `None` when the relay cannot report it.
    fn log_rows(&self, _graph: &str, _page: &str) -> Option<usize> {
        None
    }
    /// Whether a hook this relay cannot serve is a failure rather than a printed skip
    /// (`UTOPIA_RELAY_REQUIRED=1`, and always for the in-process relay).
    fn required(&self) -> bool;
}

/// A relay for one scenario: `factory` starts or finds it, `None` meaning skipped (the
/// factory printed why, or panicked when skipping is not allowed).
pub fn start(
    factory: fn(Need) -> Option<Box<dyn RelayUnderTest>>,
    need: Need,
    scenario: &str,
) -> Option<Box<dyn RelayUnderTest>> {
    assert_eq!(
        scenario.contains("quota"),
        need == Need::SmallQuota,
        "{scenario}: a scenario needs a small quota exactly when its name contains \"quota\" \
         (relay-cloudflare/scripts/test-against-rust-client.sh selects the small-quota run \
         by that word)"
    );
    factory(need)
}

/// The suite: one `#[test]` per scenario against the relay `factory` returns. Usage, after
/// `mod contract;`: `contract::contract_suite!(contract::targets::in_process);`.
#[allow(unused_macros)] // tests/differential.rs includes the module but not the suite
macro_rules! contract_suite {
    ($factory:path) => {
        $crate::contract::contract_suite!(@each $factory;
            two_machines_converge_over_http: Default,
            manifest_is_conditional_and_pushes_are_idempotent: Default,
            auth_is_enforced: Default,
            ac_12_3_a_change_id_repeated_in_one_batch_is_a_duplicate: Default,
            ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota: SmallQuota,
            ac_12_6_a_lost_ack_is_replayed_not_rejected_by_quota: SmallQuota,
            quota_rejects_and_keeps_the_outbox: SmallQuota,
            snapshots_compact_the_log_and_bootstrap_new_devices: Default,
            e2e_encrypted_sync_relay_sees_only_ciphertext: Default,
            realtime_notice_on_push: Default,
            attachments_sync_encrypted_and_verified: Default,
            tampered_blob_is_rejected: Default,
        );
    };
    (@each $factory:path; $($name:ident: $need:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let Some(relay) = $crate::contract::start(
                    $factory,
                    $crate::contract::Need::$need,
                    stringify!($name),
                ) else {
                    return;
                };
                $crate::contract::$name(&*relay);
            }
        )*
    };
}
#[allow(unused_imports)]
pub(crate) use contract_suite;

// -- helpers -------------------------------------------------------------------------

/// A graph id no other test or run can share: `[A-Za-z0-9_-]`, well under 128 bytes.
pub fn unique(tag: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{tag}-{nanos:x}-{n:x}-{}", std::process::id())
}

fn token(relay: &dyn RelayUnderTest) -> Option<String> {
    relay.token().map(str::to_string)
}

fn transport(relay: &dyn RelayUnderTest, graph: &str) -> HttpTransport {
    HttpTransport::new(relay.url(), graph, token(relay)).unwrap()
}

fn store() -> Store {
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    s
}

pub fn device(relay: &dyn RelayUnderTest, graph: &str) -> (Store, SyncClient<HttpTransport>) {
    let s = store();
    let c = SyncClient::new(transport(relay, graph), &s).unwrap();
    (s, c)
}

pub fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = s.execute_plan(page, plan, None).unwrap().unwrap().block;
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

pub fn texts(s: &mut Store, page: &PageId) -> Vec<String> {
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

pub fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Text that no compressor shrinks: a quota scenario sizes its payload by it.
fn incompressible(len: usize) -> String {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            alphabet[(x % alphabet.len() as u64) as usize] as char
        })
        .collect()
}

fn small_quota(relay: &dyn RelayUnderTest) -> u64 {
    relay
        .quota()
        .expect("a SmallQuota scenario was given a relay without a small quota")
}

fn update(id: String, client_seq: i64, payload: Vec<u8>) -> UpdateIn {
    UpdateIn {
        id,
        client_seq,
        frontier: String::new(),
        payload,
    }
}

fn is_413(err: &SyncError) -> bool {
    matches!(err, SyncError::Transport(m) if m.contains("413") && m.contains("quota exceeded"))
}

/// The ciphertext check the encrypted scenarios share: the marker must be on the device
/// (or the scan proves nothing) and in no byte the relay stored.
fn assert_only_ciphertext(relay: &dyn RelayUnderTest, marker: &str, plaintext_exists: bool) {
    assert!(
        plaintext_exists,
        "the test must first prove the plaintext exists on the device"
    );
    let Some(scan) = relay.scan_storage(marker.as_bytes()) else {
        assert!(
            !relay.required(),
            "UTOPIA_RELAY_REQUIRED=1, but the relay's storage cannot be scanned: set \
             UTOPIA_RELAY_STATE_DIR to wrangler's --persist-to directory"
        );
        eprintln!(
            "skipped the ciphertext scan: set UTOPIA_RELAY_STATE_DIR to wrangler's \
             --persist-to directory"
        );
        return;
    };
    assert!(scan.inspected > 0, "the storage scan read nothing");
    assert!(
        scan.hits.is_empty(),
        "the relay stored plaintext {marker:?} in {} of {} stored item(s): {:?}",
        scan.hits.len(),
        scan.inspected,
        scan.hits
    );
}

/// A client that hands the push to the relay and then loses the response, as a dropped
/// connection would.
pub struct LostAck {
    pub inner: HttpTransport,
    pub lost: bool,
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
        req: PushRequest,
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

fn manifest_page(t: &mut HttpTransport, page: &PageId) -> utopia_sync::wire::ManifestPage {
    t.manifest(None)
        .unwrap()
        .unwrap()
        .pages
        .into_iter()
        .find(|p| p.page_id == page.as_str())
        .expect("the page is in the manifest")
}

// -- scenarios -----------------------------------------------------------------------

/// M3 exit criteria: two stores converge through the relay, tags and dailies included.
pub fn two_machines_converge_over_http(relay: &dyn RelayUnderTest) {
    let graph = unique("converge");
    let (mut a, mut ca) = device(relay, &graph);
    let (mut b, mut cb) = device(relay, &graph);
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

/// §12.3: `If-None-Match` on the manifest is a 304, and a re-pushed change id is a duplicate.
pub fn manifest_is_conditional_and_pushes_are_idempotent(relay: &dyn RelayUnderTest) {
    let graph = unique("manifest");
    let (mut a, mut ca) = device(relay, &graph);
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
    let dup = PushRequest {
        updates: vec![update(
            pull.updates[0].id.clone(),
            1,
            pull.updates[0].payload.clone(),
        )],
    };
    let r = t.push(&p, dup).unwrap();
    assert_eq!((r.accepted, r.duplicate), (0, 1));
}

/// §12.8: a wrong token is 401, the client says `NeedsAuth` and keeps its outbox; the
/// right token syncs.
pub fn auth_is_enforced(relay: &dyn RelayUnderTest) {
    let Some(right) = relay.token() else {
        assert!(
            !relay.required(),
            "UTOPIA_RELAY_REQUIRED=1, but UTOPIA_RELAY_TOKEN is unset"
        );
        eprintln!("skipped: the relay has no token, so it has no auth to enforce");
        return;
    };
    let graph = unique("auth");
    let mut a = store();
    let wrong = HttpTransport::new(relay.url(), &graph, Some(format!("{right}-wrong"))).unwrap();
    let mut ca = SyncClient::new(wrong, &a).unwrap();
    a.create_page("Private").unwrap();
    assert!(matches!(ca.sync_once(&mut a), Err(SyncError::Unauthorized)));
    assert_eq!(ca.sync_status(&mut a), SyncStatus::NeedsAuth);
    assert!(
        SyncClient::<HttpTransport>::queued(&mut a).unwrap() > 0,
        "edits stay queued"
    );
    let (mut b, mut cb) = device(relay, &graph);
    b.create_page("Allowed").unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(SyncClient::<HttpTransport>::queued(&mut b).unwrap(), 0);
}

/// §12.3 step 2: the same new change id twice in one request is accepted once, counted as
/// a duplicate and charged to the quota once — a second insert would break the UNIQUE
/// index and fail the whole request.
pub fn ac_12_3_a_change_id_repeated_in_one_batch_is_a_duplicate(relay: &dyn RelayUnderTest) {
    let graph = unique("repeat");
    let (mut a, mut ca) = device(relay, &graph);
    let p = a.create_page("Repeated").unwrap();
    let one = update(format!("{}:1", unique("client")), 1, vec![7u8; 100]);
    let t = ca.transport_mut();
    let r = t
        .push(
            &p,
            PushRequest {
                updates: vec![one.clone(), one],
            },
        )
        .expect("a repeated change id is a duplicate, not a constraint violation");
    assert_eq!((r.accepted, r.duplicate), (1, 1));
    assert_eq!(
        t.pull(&p, 0).unwrap().updates.len(),
        1,
        "the payload is stored once"
    );
    assert_eq!(
        manifest_page(t, &p).tail_bytes,
        100,
        "the payload is charged once"
    );
}

/// §12.6: a batch the relay stored but whose response was lost is re-pushed. Dedupe runs
/// before quota, so its own bytes cannot make it a permanent 413.
pub fn ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota(relay: &dyn RelayUnderTest) {
    let quota = small_quota(relay);
    let graph = unique("replay");
    let mut t = transport(relay, &graph);
    let p = PageId::fresh();
    let client = unique("client");
    let first = update(format!("{client}:1"), 1, vec![7u8; (quota / 2) as usize]);
    let r = t
        .push(
            &p,
            PushRequest {
                updates: vec![first.clone()],
            },
        )
        .unwrap();
    assert_eq!(r.accepted, 1);
    // Nothing like enough room for a second batch of this size.
    let new = update(
        format!("{client}:2"),
        2,
        vec![7u8; (quota * 3 / 4) as usize],
    );
    let err = t.push(&p, PushRequest { updates: vec![new] }).unwrap_err();
    assert!(is_413(&err), "{err}");
    // The replay of the batch already stored needs no new space: it is a duplicate, not a
    // permanent 413 (§12.6).
    let again = t
        .push(
            &p,
            PushRequest {
                updates: vec![first],
            },
        )
        .expect("a duplicate batch needs no new space");
    assert_eq!((again.accepted, again.duplicate), (0, 1));
    // Genuinely new bytes are still refused.
    let more = update(
        format!("{client}:3"),
        3,
        vec![7u8; (quota / 2) as usize + 1],
    );
    let err = t
        .push(
            &p,
            PushRequest {
                updates: vec![more],
            },
        )
        .unwrap_err();
    assert!(is_413(&err), "{err}");
}

/// §12.6, end to end: a batch the relay stored whose response was lost is re-pushed by the
/// real client; the replay is a duplicate, so a quota with room for one copy of the batch
/// and not two cannot turn it into a permanent 413, and the relay stores it once.
pub fn ac_12_6_a_lost_ack_is_replayed_not_rejected_by_quota(relay: &dyn RelayUnderTest) {
    let quota = small_quota(relay);
    let graph = unique("lostack");
    // Size the batch from the quota: more than half of it, so a second copy cannot fit,
    // and the overhead of the CRDT update measured rather than guessed.
    let (mut a, page, text, payload_bytes) = {
        let mut len = (quota as usize) * 3 / 4;
        let mut attempt = 0;
        loop {
            let mut a = store();
            let page = a.create_page("Lost ack").unwrap();
            let text = incompressible(len);
            write(&mut a, &page, &text);
            let bytes: usize = a
                .outbox_pending(1000)
                .unwrap()
                .iter()
                .map(|row| row.3.len())
                .sum();
            if (bytes as u64) * 2 > quota && bytes as u64 + 256 <= quota {
                break (a, page, text, bytes);
            }
            attempt += 1;
            assert!(
                attempt < 3,
                "cannot size a batch between half and all of a {quota}-byte quota \
                 ({len} characters made {bytes} bytes)"
            );
            len = (quota as usize * 3 / 4).saturating_sub(bytes - len);
        }
    };
    let queued_updates = a.outbox_pending(1000).unwrap().len();
    let mut ca = SyncClient::new(
        LostAck {
            inner: transport(relay, &graph),
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
    assert!(replayed.failed.is_empty(), "{replayed:?}");
    assert!(matches!(ca.sync_status(&mut a), SyncStatus::Synced { .. }));
    assert_eq!(
        SyncClient::<LostAck>::queued(&mut a).unwrap(),
        0,
        "the replay was acknowledged"
    );
    let mut t = transport(relay, &graph);
    assert_eq!(
        t.pull(&page, 0).unwrap().updates.len(),
        queued_updates,
        "the relay stored the batch once, not once per attempt"
    );
    assert_eq!(
        manifest_page(&mut t, &page).tail_bytes as usize,
        payload_bytes,
        "the relay charged the batch once"
    );
    // A device that pulled the batch sees the block once.
    let (mut b, mut cb) = device(relay, &graph);
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &page), vec![text]);
}

/// §12.6: a push over the quota is 413, and the client keeps its outbox.
pub fn quota_rejects_and_keeps_the_outbox(relay: &dyn RelayUnderTest) {
    let quota = small_quota(relay);
    let graph = unique("quota");
    let (mut a, mut ca) = device(relay, &graph);
    let p = a.create_page("Big").unwrap();
    write(&mut a, &p, &incompressible(quota as usize * 2));
    let err = ca.sync_once(&mut a).unwrap_err();
    assert!(is_413(&err), "{err}");
    assert!(matches!(ca.sync_status(&mut a), SyncStatus::Offline { queued } if queued > 0));
}

/// §11.7: the client compacts with a snapshot, the relay truncates the covered log, and a
/// new device bootstraps from the snapshot.
pub fn snapshots_compact_the_log_and_bootstrap_new_devices(relay: &dyn RelayUnderTest) {
    let graph = unique("compact");
    let (mut a, mut ca) = device(relay, &graph);
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
    // What a client sees: a snapshot, and `tail_bytes` (the bytes left in the log, not in
    // the snapshot) small.
    let entry = manifest_page(ca.transport_mut(), &p);
    assert!(entry.snapshot_seq > 0, "{entry:?}");
    assert!(
        entry.tail_bytes < 2048,
        "the covered log rows are truncated: {entry:?}"
    );
    if let Some(rows) = relay.log_rows(&graph, p.as_str()) {
        assert!(rows < 50, "log truncated: {rows} rows");
    }
    let (mut fresh, mut cf) = device(relay, &graph);
    cf.sync_once(&mut fresh).unwrap();
    assert_eq!(texts(&mut fresh, &p), vec!["rev 219"]);
}

/// §11.7, §15.6: with E2E on the relay holds only ciphertext, and a wrong passphrase fails
/// with a message rather than garbage.
pub fn e2e_encrypted_sync_relay_sees_only_ciphertext(relay: &dyn RelayUnderTest) {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let graph = unique("e2e");
    let marker = format!("canary-{}", unique("plaintext"));
    let enc = |s: &Store, pass: &str| {
        let key = GraphKey::derive(pass, &graph).unwrap();
        SyncClient::new(Encrypted::new(transport(relay, &graph), key, &graph), s).unwrap()
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
        texts(&mut b, &p),
        vec![format!("a very secret sentence {marker}")],
        "the other device decrypted the page"
    );
    // A device with the wrong passphrase cannot read, and says so.
    let mut c = Store::open_in_memory().unwrap();
    let mut cc = enc(&c, "wrong passphrase");
    let err = cc.sync_once(&mut c).unwrap_err();
    assert!(err.to_string().contains("decryption failed"), "{err}");
    let seen = texts(&mut b, &p).iter().any(|t| t.contains(&marker));
    assert_only_ciphertext(relay, &marker, seen);
}

/// §12.3 "Realtime": an accepted push sends `{"page_id","head_seq"}` to subscribers.
pub fn realtime_notice_on_push(relay: &dyn RelayUnderTest) {
    let graph = unique("realtime");
    let (mut a, mut ca) = device(relay, &graph);
    let p = a.create_page("Live").unwrap();
    let base = relay.url().to_string();
    let listen_graph = graph.clone();
    let listen_token = token(relay);
    let (tx, rx) = std::sync::mpsc::channel();
    let listener = std::thread::spawn(move || {
        utopia_sync::realtime::listen(&base, &listen_graph, listen_token.as_deref(), |n| {
            tx.send(n).unwrap();
            false
        })
    });
    // No subscribe acknowledgement exists, so a push confirms the subscription: the socket
    // may still be connecting (or the Worker's Durable Object spinning up) at the first.
    let t = ca.transport_mut();
    let client = unique("client");
    let mut notice = None;
    for attempt in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(if attempt == 0 {
            200
        } else {
            700
        }));
        t.push(
            &p,
            PushRequest {
                updates: vec![update(
                    format!("{client}:{attempt}"),
                    attempt + 1,
                    vec![1u8; 16],
                )],
            },
        )
        .unwrap();
        if let Ok(n) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
            notice = Some(n);
            break;
        }
    }
    let notice = notice.expect("the relay sends a notice for an accepted push");
    assert_eq!(notice.page_id, p.as_str());
    assert!(notice.head_seq >= 1);
    listener.join().unwrap().unwrap();
}

/// §10.3 rule 12: attachments travel encrypted, arrive verified, and the relay holds only
/// ciphertext.
pub fn attachments_sync_encrypted_and_verified(relay: &dyn RelayUnderTest) {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let graph = unique("attach");
    let marker = format!("pixels-{}", unique("secret"));
    let dir = tempfile::tempdir().unwrap();
    let enc = |s: &Store| {
        let key = GraphKey::derive("pass phrase", &graph).unwrap();
        SyncClient::new(Encrypted::new(transport(relay, &graph), key, &graph), s).unwrap()
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
    assert_only_ciphertext(relay, &marker, contains(&photo, marker.as_bytes()));
    // Nothing more to move.
    let r = ca.sync_once(&mut a).unwrap();
    assert_eq!((r.blobs_up, r.blobs_down), (0, 0));
}

/// §10.3 rule 12: a blob whose bytes do not hash to its name is never stored by a client.
/// The relay does not check hashes (blobs may be ciphertext), and write-once means bytes
/// someone with relay access uploaded first are what a download returns.
pub fn tampered_blob_is_rejected(relay: &dyn RelayUnderTest) {
    let graph = unique("tamper");
    let (mut a, mut ca) = device(relay, &graph);
    let (mut b, mut cb) = device(relay, &graph);
    let att = a.add_blob("notes.txt", b"the real notes").unwrap();
    let p = a.create_page("Notes").unwrap();
    write(&mut a, &p, &format!("[notes.txt](blob:{})", att.hash));
    // Someone with relay access stores forged bytes under the hash before A uploads.
    transport(relay, &graph)
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
