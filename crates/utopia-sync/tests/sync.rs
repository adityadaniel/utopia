//! Multi-device scenarios from §12.4 and AC-4.8, through the reference relay.

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::{BlockId, PageId};
use utopia_store::Store;
use utopia_sync::relay::{Relay, RelayTransport};
use utopia_sync::{SyncClient, SyncStatus};

fn device(relay: &std::sync::Arc<Relay>) -> (Store, SyncClient<RelayTransport>) {
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let c = SyncClient::new(RelayTransport::new(relay.clone(), "g1"), &s).unwrap();
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

fn graph(s: &mut Store) -> Vec<(String, Vec<String>)> {
    s.list_pages(true)
        .unwrap()
        .into_iter()
        .map(|p| (p.title.clone(), texts(s, &p.id)))
        .collect()
}

fn settle(devs: &mut [(&mut Store, &mut SyncClient<RelayTransport>)]) {
    for _ in 0..2 {
        for (s, c) in devs.iter_mut() {
            c.sync_once(s).unwrap();
        }
    }
}

/// §12.3 step 4: the folder cursor is a revision, not a file count. A peer that drops two
/// files and adds two others leaves the count unchanged but must still be pulled.
#[test]
fn ac_12_3_a_folder_revision_with_an_unchanged_file_count_still_pulls() {
    use utopia_sync::folder::FolderTransport;
    let dir = tempfile::tempdir().unwrap();
    let mut a = Store::open_in_memory().unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let mut ca = SyncClient::new(FolderTransport::new(dir.path(), "a").unwrap(), &a).unwrap();
    let mut cb = SyncClient::new(FolderTransport::new(dir.path(), "b").unwrap(), &b).unwrap();
    let page = a.create_page("Shared").unwrap();
    for i in 0..6 {
        write(&mut a, &page, &format!("first {i}"));
    }
    ca.sync_once(&mut a).unwrap();
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &page).len(), 6);

    // Two more local updates, not yet pushed, plus the same number of already-imported
    // files removed: the file count stays the same and only the contents change.
    write(&mut a, &page, "later one");
    write(&mut a, &page, "later two");
    let pending = a.outbox_pending(100).unwrap();
    let page_dir = dir.path().join("pages").join(page.as_str()).join("a");
    let mut existing: Vec<_> = std::fs::read_dir(&page_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "upd"))
        .collect();
    existing.sort();
    assert!(existing.len() >= pending.len());
    for f in existing.iter().take(pending.len()) {
        std::fs::remove_file(f).unwrap();
    }
    for (_, _, client_seq, payload) in &pending {
        std::fs::write(
            page_dir.join(format!("{:012}.upd", client_seq.max(&0))),
            payload,
        )
        .unwrap();
    }

    cb.sync_once(&mut b).unwrap();
    let seen = texts(&mut b, &page);
    assert!(seen.contains(&"later one".to_string()), "{seen:?}");
    assert!(seen.contains(&"later two".to_string()), "{seen:?}");
}

/// §12.3 step 4: a cursor measured on one transport is not compared with another's
/// `head_seq`. Switching from a folder (a file count) to a fresh relay must pull.
#[test]
fn ac_12_3_switching_transport_pulls_the_new_transport_from_zero() {
    use utopia_sync::folder::FolderTransport;
    let relay = Relay::new();
    let dir = tempfile::tempdir().unwrap();
    let mut a = Store::open_in_memory().unwrap();
    let mut ca = SyncClient::new(FolderTransport::new(dir.path(), "a").unwrap(), &a).unwrap();
    let page = a.create_page("Shared").unwrap();
    for i in 0..10 {
        write(&mut a, &page, &format!("folder {i}"));
    }
    ca.sync_once(&mut a).unwrap();

    // The same page on a fresh relay: its sequence starts at 1, far below the folder's
    // file count.
    let (mut c, mut cc) = device(&relay);
    let same = c.create_page("Shared").unwrap();
    assert_eq!(same, page, "titles make the same page id");
    write(&mut c, &same, "from the relay");
    cc.sync_once(&mut c).unwrap();

    let mut ca = SyncClient::new(RelayTransport::new(relay.clone(), "g1"), &a).unwrap();
    ca.sync_once(&mut a).unwrap();
    let seen = texts(&mut a, &page);
    assert!(seen.contains(&"from the relay".to_string()), "{seen:?}");
}

/// A relay whose reported protocol version can change in place, as a binary upgrade that
/// keeps the database (and its revision) would.
struct UpgradedRelay {
    inner: RelayTransport,
    version: u32,
    pushes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl utopia_sync::Transport for UpgradedRelay {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }
    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        match self.inner.manifest(if_none_match)? {
            Some(mut m) => {
                m.protocol_version = self.version;
                Ok(Some(m))
            }
            None => Ok(None),
        }
    }
    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        self.pushes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.push(page, req)
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

/// §12.6: an in-place protocol upgrade keeps the relay's rev, so a conditional manifest
/// answers 304 and skips the version check. A round with something to push must see the
/// new version before it writes.
#[test]
fn ac_12_6_a_protocol_upgrade_is_seen_before_pushing() {
    let relay = Relay::new();
    let mut a = Store::open_in_memory().unwrap();
    a.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = a.create_page("Protocol").unwrap();
    write(&mut a, &page, "first");
    let pushes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ca = SyncClient::new(
        UpgradedRelay {
            inner: RelayTransport::new(relay.clone(), "g1"),
            version: 1,
            pushes: pushes.clone(),
        },
        &a,
    )
    .unwrap();
    ca.sync_once(&mut a).unwrap();
    let pushed = pushes.load(std::sync::atomic::Ordering::SeqCst);
    assert!(pushed > 0);
    let rev = relay.manifest("g1").rev;

    // The relay is upgraded in place: same database and revision, another protocol.
    ca.transport_mut().version = 2;
    assert_eq!(relay.manifest("g1").rev, rev);
    write(&mut a, &page, "second");
    let result = ca.sync_once(&mut a);
    assert!(
        matches!(result, Err(utopia_sync::SyncError::ProtocolVersion(2))),
        "the new protocol is seen before writing: {result:?}"
    );
    assert_eq!(
        pushes.load(std::sync::atomic::Ordering::SeqCst),
        pushed,
        "nothing was pushed to the unsupported relay"
    );
}

/// A relay that refuses the first push for one page and accepts any later one.
struct RefuseFirstBatch {
    inner: RelayTransport,
    page: PageId,
    pushes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    refused: bool,
}

impl utopia_sync::Transport for RefuseFirstBatch {
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
        if page == &self.page {
            self.pushes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !self.refused {
                self.refused = true;
                return Err(utopia_sync::SyncError::Transport(
                    "HTTP 413: quota exceeded".into(),
                ));
            }
        }
        self.inner.push(page, req)
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

/// §12.3 step 2: a page whose outbox spans several batches must not send batch 2 after
/// batch 1 was refused — the relay would hold later updates without their predecessors,
/// and a peer could advance its cursor over an update it only holds pending.
#[test]
fn ac_12_3_a_refused_batch_stops_the_pages_later_batches() {
    let relay = Relay::new();
    let (mut a, _) = device(&relay);
    let page = a.create_page("Long").unwrap();
    write(&mut a, &page, &"x".repeat(600_000));
    write(&mut a, &page, &"y".repeat(600_000));
    let pending = a.outbox_pending(100).unwrap();
    let bytes: usize = pending.iter().map(|row| row.3.len()).sum();
    assert!(
        bytes > utopia_sync::MAX_PUSH_BYTES,
        "the page needs several push batches: {bytes}"
    );

    let pushes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ca = SyncClient::new(
        RefuseFirstBatch {
            inner: RelayTransport::new(relay.clone(), "g1"),
            page: page.clone(),
            pushes: pushes.clone(),
            refused: false,
        },
        &a,
    )
    .unwrap();
    let result = ca.sync_once(&mut a);
    assert!(
        matches!(result, Err(utopia_sync::SyncError::Transport(_))),
        "nothing moved: {result:?}"
    );
    assert_eq!(
        pushes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "only the refused batch was offered"
    );
    assert_eq!(
        relay.log_len("g1", page.as_str()),
        0,
        "no later batch reached the relay"
    );
    assert_eq!(
        a.outbox_pending(100).unwrap().len(),
        pending.len(),
        "every row is still queued"
    );
}

/// A relay that takes the pushes but times out on the manifest read that follows them.
struct NoSecondManifest {
    inner: RelayTransport,
    calls: usize,
}

impl utopia_sync::Transport for NoSecondManifest {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }
    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.calls += 1;
        if self.calls > 1 {
            return Err(utopia_sync::SyncError::Transport(
                "manifest timed out".into(),
            ));
        }
        self.inner.manifest(if_none_match)
    }
    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        self.inner.push(page, req)
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

/// §12.3 steps 2–3: a manifest read that fails after the relay accepted the pushes must not
/// lose their acks; only the pulls are skipped.
#[test]
fn ac_12_3_a_failed_second_manifest_keeps_the_acks() {
    let relay = Relay::new();
    let (mut a, _) = device(&relay);
    let page = a.create_page("Pushed").unwrap();
    write(&mut a, &page, "the relay takes this");
    let mut ca = SyncClient::new(
        NoSecondManifest {
            inner: RelayTransport::new(relay.clone(), "g1"),
            calls: 0,
        },
        &a,
    )
    .unwrap();
    let report = ca
        .sync_once(&mut a)
        .expect("the pushes count even without the second manifest");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(report.failed[0].page.is_none());
    assert!(report.pushed > 0, "{report:?}");
    assert_eq!(
        a.outbox_pending(100).unwrap().len(),
        0,
        "the acknowledged rows are marked sent"
    );
    assert_eq!(
        a.sync_get("rev").unwrap(),
        None,
        "no manifest was applied, so none is remembered"
    );
}

/// A relay that refuses one page (a 413, a poison batch) and serves every other page.
struct Flaky {
    inner: RelayTransport,
    refuse: PageId,
}

impl utopia_sync::Transport for Flaky {
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
        if page == &self.refuse {
            return Err(utopia_sync::SyncError::Transport(
                "HTTP 413: quota exceeded".into(),
            ));
        }
        self.inner.push(page, req)
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

/// Records how the manifest and pulls were asked for, to prove the conditional request.
struct Conditional {
    inner: RelayTransport,
    asked: std::sync::Arc<std::sync::Mutex<Vec<Option<u64>>>>,
    pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl utopia_sync::Transport for Conditional {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }
    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.asked.lock().unwrap().push(if_none_match);
        self.inner.manifest(if_none_match)
    }
    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        self.inner.push(page, req)
    }
    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        self.pulls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

/// §12.3 step 1: an idle round asks with `If-None-Match: <applied rev>` and does no pulls
/// when the relay has not moved.
#[test]
fn ac_12_3_an_unchanged_manifest_is_asked_for_conditionally() {
    let relay = Relay::new();
    let mut a = Store::open_in_memory().unwrap();
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let pulls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ca = SyncClient::new(
        Conditional {
            inner: RelayTransport::new(relay.clone(), "g1"),
            asked: asked.clone(),
            pulls: pulls.clone(),
        },
        &a,
    )
    .unwrap();
    let page = a.create_page("Once").unwrap();
    write(&mut a, &page, "hello");
    ca.sync_once(&mut a).unwrap();
    let rev = utopia_sync::ManifestRev::decode(&a.sync_get("rev").unwrap().unwrap())
        .expect("the applied rev is tagged with its space")
        .rev;

    let before = asked.lock().unwrap().len();
    ca.sync_once(&mut a).unwrap();
    let asked_all = asked.lock().unwrap().clone();
    assert_eq!(
        asked_all[before],
        Some(rev),
        "the second round sends the applied rev: {asked_all:?}"
    );
    assert_eq!(
        pulls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "only the first round pulled"
    );
}

/// Records snapshot uploads and can refuse every pull, so a round's compaction wants can
/// be observed without letting a fresh pull make them legitimate.
struct Counting {
    inner: RelayTransport,
    puts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    fail_pulls: bool,
}

impl utopia_sync::Transport for Counting {
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
        self.inner.push(page, req)
    }
    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        if self.fail_pulls {
            return Err(utopia_sync::SyncError::Transport("unreachable".into()));
        }
        self.inner.pull(page, since_seq)
    }
    fn put_snapshot(
        &mut self,
        page: &PageId,
        covers_seq: u64,
        payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        self.puts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.put_snapshot(page, covers_seq, payload)
    }
}

/// §12.6 (rule 5): a compaction snapshot may only cover a head measured on the same
/// transport. A cursor left behind by another relay, with the pull failing, must not
/// truncate relay entries this client never imported.
#[test]
fn ac_12_6_a_foreign_cursor_cannot_compact_the_relay() {
    use utopia_sync::{PushRequest, Transport, UpdateIn};
    let relay = Relay::new();
    let mut a = Store::open_in_memory().unwrap();
    let page = a.create_page("Compaction").unwrap();
    // Another device left a long log for this page (the relay never parses payloads).
    let mut t = RelayTransport::new(relay.clone(), "g1");
    for i in 0..201 {
        t.push(
            &page,
            PushRequest {
                updates: vec![UpdateIn {
                    id: format!("other:{i}"),
                    client_seq: i,
                    frontier: String::new(),
                    payload: vec![1, 2, 3],
                }],
            },
        )
        .unwrap();
    }
    let head = relay.manifest("g1").pages[0].head_seq;
    assert!(head > 200, "the tail crosses the snapshot bound");
    // A cursor from a relay this graph used before claims to cover that head.
    a.sync_set(
        &format!("cursor:{page}"),
        &utopia_sync::Cursor {
            space: utopia_sync::CursorSpace::new(
                "relay:another-store#g1",
                utopia_sync::CursorKind::Seq,
            ),
            pos: head + 1000,
        }
        .encode(),
    )
    .unwrap();

    let puts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ca = SyncClient::new(
        Counting {
            inner: RelayTransport::new(relay.clone(), "g1"),
            puts: puts.clone(),
            fail_pulls: true,
        },
        &a,
    )
    .unwrap();
    // Both rounds fail every pull, so no pull can justify a snapshot.
    let _ = ca.sync_once(&mut a);
    let _ = ca.sync_once(&mut a);
    assert_eq!(
        puts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no snapshot was queued from another transport's cursor"
    );
    assert!(
        relay.log_len("g1", page.as_str()) > 200,
        "the relay log is untouched"
    );
}

/// §12.3 step 1: the stored manifest rev belongs to the relay that issued it. A second
/// relay that happens to be at the same revision must not answer 304 and hide its pages.
#[test]
fn ac_12_3_switching_relay_at_the_same_revision_still_pulls() {
    let relay_a = Relay::new();
    let relay_b = Relay::new();
    let (mut a, mut ca) = device(&relay_a);
    let page = a.create_page("Shared").unwrap();
    write(&mut a, &page, "written while on relay a");
    ca.sync_once(&mut a).unwrap();

    let (mut b, mut cb) = device(&relay_b);
    let same = b.create_page("Shared").unwrap();
    assert_eq!(same, page);
    write(&mut b, &same, "written on relay b");
    cb.sync_once(&mut b).unwrap();
    assert_eq!(
        relay_a.manifest("g1").rev,
        relay_b.manifest("g1").rev,
        "the two relays are at the same revision"
    );

    let mut ca = SyncClient::new(RelayTransport::new(relay_b.clone(), "g1"), &a).unwrap();
    ca.sync_once(&mut a).unwrap();
    let seen = texts(&mut a, &page);
    assert!(seen.contains(&"written on relay b".to_string()), "{seen:?}");
}

/// A relay whose pull payloads for one page are corrupt: a shard the relay accepted but
/// this device cannot import.
struct Corrupt {
    inner: RelayTransport,
    page: PageId,
}

impl utopia_sync::Transport for Corrupt {
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
        self.inner.push(page, req)
    }
    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        let mut resp = self.inner.pull(page, since_seq)?;
        if page == &self.page {
            for u in &mut resp.updates {
                u.payload = b"not a loro update".to_vec();
            }
        }
        Ok(resp)
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

/// §12.3 steps 3–4: an import that fails must leave that page queued and its cursor
/// unmoved, instead of recording the acks and losing the position.
#[test]
fn ac_12_3_a_failed_import_does_not_mark_the_page_synced() {
    let relay = Relay::new();
    let (mut a, _) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let broken = a.create_page("Broken").unwrap();
    write(&mut a, &broken, "the relay's copy will not import");
    let readable = b.create_page("Readable").unwrap();
    write(&mut b, &readable, "written on b");
    cb.sync_once(&mut b).unwrap();

    let mut ca = SyncClient::new(
        Corrupt {
            inner: RelayTransport::new(relay.clone(), "g1"),
            page: broken.clone(),
        },
        &a,
    )
    .unwrap();
    let report = ca
        .sync_once(&mut a)
        .expect("the readable page still imports");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].page.as_ref(), Some(&broken));
    assert_eq!(texts(&mut a, &readable), vec!["written on b"]);
    // The broken page's push was accepted, but its import failed: its rows stay queued.
    let queued = a.outbox_pending(100).unwrap();
    assert!(!queued.is_empty(), "the failed page is still queued");
    assert!(
        queued.iter().all(|(_, p, _, _)| p == &broken),
        "the readable page's acks were recorded: {queued:?}"
    );
    assert_eq!(
        a.sync_get("rev").unwrap(),
        None,
        "an unfinished manifest is fetched again next round"
    );
}

/// §12.3 steps 2–4: one page can be pulled successfully and still have its push refused.
/// The pull's cursor advances; the refused rows stay queued.
#[test]
fn ac_12_3_a_page_can_be_pulled_and_still_queued_for_its_push() {
    let relay = Relay::new();
    let (mut a, _) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let page = a.create_page("Shared").unwrap();
    write(&mut a, &page, "this push will be refused");
    let same = b.create_page("Shared").unwrap();
    assert_eq!(same, page);
    write(&mut b, &same, "written on b");
    cb.sync_once(&mut b).unwrap();

    let mut ca = SyncClient::new(
        Flaky {
            inner: RelayTransport::new(relay.clone(), "g1"),
            refuse: page.clone(),
        },
        &a,
    )
    .unwrap();
    let report = ca.sync_once(&mut a).expect("the pull still runs");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert!(
        texts(&mut a, &page).contains(&"written on b".to_string()),
        "the pull was applied"
    );
    let cursor = utopia_sync::Cursor::decode(
        &a.sync_get(&format!("cursor:{page}"))
            .unwrap()
            .expect("the pulled page has a cursor"),
    )
    .expect("cursor is JSON");
    assert_eq!(
        cursor.pos,
        relay.manifest("g1").pages[0].head_seq,
        "the cursor advanced to the pulled head"
    );
    let queued = a.outbox_pending(100).unwrap();
    assert!(
        !queued.is_empty() && queued.iter().all(|(_, p, _, _)| p == &page),
        "the refused push's rows are still queued: {queued:?}"
    );
}

/// §12.3 steps 3–4: when the batch import fails on one page, every other page must still
/// be committed and reported. The corrupt page is tried both first and last in `PageId`
/// order: when it is last, the readable page's shard is already in memory when the batch
/// fails, so a naive per-page retry would skip it as "already imported".
#[test]
fn ac_12_3_a_failed_batch_import_still_stores_the_readable_pages() {
    let relay = Relay::new();
    let (mut b, mut cb) = device(&relay);
    let alpha = b.create_page("Alpha").unwrap();
    let bravo = b.create_page("Bravo").unwrap();
    write(&mut b, &alpha, "alpha from b");
    write(&mut b, &bravo, "bravo from b");
    cb.sync_once(&mut b).unwrap();
    let (low, high) = if alpha < bravo {
        (alpha.clone(), bravo.clone())
    } else {
        (bravo.clone(), alpha.clone())
    };
    for broken in [low, high] {
        let readable = if broken == alpha {
            bravo.clone()
        } else {
            alpha.clone()
        };
        let want = if readable == alpha {
            "alpha from b"
        } else {
            "bravo from b"
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.utopia");
        let mut a = Store::open(&path).unwrap();
        a.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let mut ca = SyncClient::new(
            Corrupt {
                inner: RelayTransport::new(relay.clone(), "g1"),
                page: broken.clone(),
            },
            &a,
        )
        .unwrap();
        let report = ca.sync_once(&mut a).expect("the readable page imports");
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].page.as_ref(), Some(&broken));
        assert!(
            report.imported.contains(&readable),
            "the readable page is reported: {report:?}"
        );
        assert_eq!(texts(&mut a, &readable), vec![want]);
        // Reopen from disk: the readable page must have been committed, not just left in
        // the in-memory shard the failed batch import touched.
        drop(ca);
        drop(a);
        let mut reopened = Store::open(&path).unwrap();
        assert_eq!(
            texts(&mut reopened, &readable),
            vec![want],
            "committed to disk (corrupt page {broken:?})"
        );
    }
}

/// An encrypted device: same relay, payloads sealed with the graph key.
fn encrypted_device(
    relay: &std::sync::Arc<Relay>,
    passphrase: &str,
) -> (
    Store,
    SyncClient<utopia_sync::e2e::Encrypted<RelayTransport>>,
) {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    let mut s = Store::open_in_memory().unwrap();
    s.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let key = GraphKey::derive(passphrase, "g1").unwrap();
    let t = Encrypted::new(RelayTransport::new(relay.clone(), "g1"), key, "g1");
    let c = SyncClient::new(t, &s).unwrap();
    (s, c)
}

/// §12.3 step 4: the pre-`cursor:` positions are retired on the first engine open, so a
/// legacy graph re-pulls once instead of keeping dead keys forever.
#[test]
fn ac_12_3_legacy_seq_positions_are_removed() {
    let s = Store::open_in_memory().unwrap();
    s.sync_set("seq:page-1", "42").unwrap();
    s.sync_set("seq:page-2", "7").unwrap();
    s.sync_set("cursor:page-3", "keep").unwrap();
    let _engine = utopia_sync::SyncEngine::new(&s).unwrap();
    assert!(s.sync_prefixed("seq").unwrap().is_empty());
    assert_eq!(
        s.sync_get("cursor:page-3").unwrap().as_deref(),
        Some("keep")
    );
}

/// §9.3/§12.6: a reachable relay that could not take one page is a partial round, not
/// "Offline with 0 changes queued" and not a plain success.
#[test]
fn ac_12_6_a_partial_round_has_its_own_status() {
    let store = Store::open_in_memory().unwrap();
    let report = utopia_sync::SyncReport {
        pushed: 2,
        failed: vec![utopia_sync::Failure {
            page: None,
            kind: utopia_sync::FailureKind::Transport("HTTP 400".into()),
        }],
        ..Default::default()
    };
    assert_eq!(
        SyncStatus::from_result(&Ok(report), &store),
        SyncStatus::Partial {
            failed: 1,
            queued: 0
        }
    );
}

/// §12.6: a page this device cannot read (here: plaintext left by a device without the
/// passphrase) must not stop every other page's pull, nor lose their cursors.
#[test]
fn ac_12_6_one_unreadable_page_does_not_halt_the_others() {
    let relay = Relay::new();
    let (mut plain, mut cp) = device(&relay);
    let poison = plain.create_page("Plaintext").unwrap();
    write(&mut plain, &poison, "not for encrypted readers");
    cp.sync_once(&mut plain).unwrap();

    let (mut a, mut ca) = encrypted_device(&relay, "correct horse");
    let sealed = a.create_page("Sealed").unwrap();
    write(&mut a, &sealed, "readable by both encrypted devices");
    let report = ca
        .sync_once(&mut a)
        .expect("the unreadable page does not fail the round");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].page.as_ref(), Some(&poison));

    let (mut b, mut cb) = encrypted_device(&relay, "correct horse");
    let report = cb
        .sync_once(&mut b)
        .expect("the second device gets the readable page");
    assert_eq!(
        texts(&mut b, &sealed),
        vec!["readable by both encrypted devices"]
    );
    assert_eq!(
        report.failed.len(),
        1,
        "only the poison page failed: {report:?}"
    );
}

/// §12.3 steps 2–5: a page the relay refuses must not cost the other pages their acks,
/// nor stop the pull of what the relay does have.
#[test]
fn ac_12_3_a_refused_page_keeps_the_other_acks_and_the_pulls() {
    let relay = Relay::new();
    let (mut a, _) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let refused = a.create_page("Refused").unwrap();
    let accepted = a.create_page("Accepted").unwrap();
    write(&mut a, &refused, "this batch is too big");
    write(&mut a, &accepted, "this one fits");
    let remote = b.create_page("Remote").unwrap();
    write(&mut b, &remote, "written on b");
    cb.sync_once(&mut b).unwrap();

    let mut ca = SyncClient::new(
        Flaky {
            inner: RelayTransport::new(relay.clone(), "g1"),
            refuse: refused.clone(),
        },
        &a,
    )
    .unwrap();
    let report = ca
        .sync_once(&mut a)
        .expect("the round carries on past the refused page");
    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].page.as_ref(), Some(&refused));
    assert!(
        report.pushed > 0,
        "the other page was acknowledged: {report:?}"
    );
    assert_eq!(
        texts(&mut a, &remote),
        vec!["written on b"],
        "the pull still ran"
    );
    // The accepted page's outbox rows are marked sent; only the refused page's remain.
    let queued = a.outbox_pending(100).unwrap();
    assert!(!queued.is_empty());
    assert!(
        queued.iter().all(|(_, p, _, _)| p == &refused),
        "only the refused page is queued: {queued:?}"
    );
}

#[test]
fn two_devices_converge_on_different_blocks_and_same_block() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let (page, first) = a.open_daily(a.today()).unwrap();
    a.set_block_text(&page, &first, "hello world", None, None)
        .unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(texts(&mut b, &page), vec!["hello world"]);
    // Offline: both edit the same block and add different blocks.
    a.set_block_text(&page, &first, "hello big world", None, None)
        .unwrap();
    b.set_block_text(&page, &first, "hello world!", None, None)
        .unwrap();
    write(&mut a, &page, "from a");
    write(&mut b, &page, "from b");
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(graph(&mut a), graph(&mut b));
    let t = texts(&mut a, &page);
    assert_eq!(t[0], "hello big world!", "both inserts survive");
    assert!(t.contains(&"from a".to_string()) && t.contains(&"from b".to_string()));
    for s in [&mut a, &mut b] {
        assert!(s.doctor().unwrap().is_ok());
    }
}

#[test]
fn same_daily_note_offline_is_one_page() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let (pa, ba) = a.open_daily(a.today()).unwrap();
    let (pb, bb) = b.open_daily(b.today()).unwrap();
    assert_eq!(pa, pb, "daily ids are deterministic");
    a.set_block_text(&pa, &ba, "a's morning", None, None)
        .unwrap();
    b.set_block_text(&pb, &bb, "b's morning", None, None)
        .unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(graph(&mut a), graph(&mut b));
    assert_eq!(a.list_pages(true).unwrap().len(), 1);
    assert_eq!(texts(&mut a, &pa).len(), 2);
}

#[test]
fn offline_edits_queue_and_flush() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    ca.transport_mut().offline = true;
    let p = a.create_page("Queued").unwrap();
    write(&mut a, &p, "one");
    write(&mut a, &p, "two");
    let queued = SyncClient::<RelayTransport>::queued(&mut a).unwrap();
    assert!(queued > 0);
    assert_eq!(ca.sync_status(&mut a), SyncStatus::Offline { queued });
    assert_eq!(
        SyncClient::<RelayTransport>::queued(&mut a).unwrap(),
        queued,
        "outbox retained"
    );
    ca.transport_mut().offline = false;
    ca.sync_once(&mut a).unwrap();
    assert_eq!(SyncClient::<RelayTransport>::queued(&mut a).unwrap(), 0);
    cb.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut b, &p), vec!["one", "two"]);
}

/// §9.3/§12.3: the "N changes queued" count must not lag behind a debounced edit.
#[test]
fn ac_12_3_queued_counts_edits_still_waiting_for_the_writer() {
    let mut s = Store::open_in_memory().unwrap();
    s.set_debounced(true).unwrap();
    let page = s.create_page("Debounced").unwrap();
    let block = write(&mut s, &page, "first");
    let before = SyncClient::<RelayTransport>::queued(&mut s).unwrap();
    // A plain text edit stays in memory until a flush: still unsent, so still queued.
    s.set_block_text(&page, &block, "first, edited", None, None)
        .unwrap();
    assert!(s.has_pending(), "the edit is debounced");
    let after = SyncClient::<RelayTransport>::queued(&mut s).unwrap();
    assert_eq!(after, before + 1, "the waiting edit is counted");
    assert!(!s.has_pending(), "counting flushed it");
}

#[test]
fn ac_4_8_merge_while_peer_offline_converges() {
    let relay = Relay::new();
    let (mut d1, mut c1) = device(&relay);
    let (mut d2, mut c2) = device(&relay);
    let a = d1.create_page("Alpha").unwrap();
    write(&mut d1, &a, "a0");
    let b = d1.create_page("Bravo").unwrap();
    write(&mut d1, &b, "b0");
    settle(&mut [(&mut d1, &mut c1), (&mut d2, &mut c2)]);
    d1.merge_pages(&b, &a).unwrap();
    for i in 0..3 {
        write(&mut d2, &b, &format!("offline {i}"));
    }
    settle(&mut [(&mut d1, &mut c1), (&mut d2, &mut c2)]);
    assert_eq!(graph(&mut d1), graph(&mut d2));
    let merged = texts(&mut d1, &a);
    assert_eq!(merged.len(), 5, "{merged:?}");
    for s in [&mut d1, &mut d2] {
        assert!(s.page_row(&b).unwrap().unwrap().deleted_at.is_some());
        assert!(s.doctor().unwrap().is_ok());
    }
}

#[test]
fn concurrent_renames_keep_both_titles_resolvable() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let (mut b, mut cb) = device(&relay);
    let p = a.create_page("Alpha").unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    a.rename_page(&p, "Alpha One").unwrap();
    b.rename_page(&p, "Alpha Two").unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(graph(&mut a), graph(&mut b));
    for t in ["Alpha", "Alpha One", "Alpha Two"] {
        assert_eq!(a.find_page(t).unwrap(), Some(p.clone()), "{t}");
    }
}

#[test]
fn snapshot_compaction_bootstraps_new_device() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let p = a.create_page("Long").unwrap();
    let blk = write(&mut a, &p, "");
    for i in 0..250 {
        a.set_block_text(&p, &blk, &format!("rev {i}"), None, None)
            .unwrap();
        // Each edit is its own sync so the relay log grows past the threshold.
        if i % 10 == 0 {
            ca.sync_once(&mut a).unwrap();
        }
    }
    ca.sync_once(&mut a).unwrap();
    let r = ca.sync_once(&mut a).unwrap();
    assert!(
        relay.log_len("g1", p.as_str()) < 200,
        "log truncated (snapshots: {})",
        r.snapshots
    );
    let (mut fresh, mut cf) = device(&relay);
    cf.sync_once(&mut fresh).unwrap();
    assert_eq!(texts(&mut fresh, &p), vec!["rev 249"]);
}

#[test]
fn pushes_are_idempotent() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let p = a.create_page("Once").unwrap();
    write(&mut a, &p, "x");
    let pending = a.outbox_pending(100).unwrap();
    ca.sync_once(&mut a).unwrap();
    // Replay the same outbox rows (e.g. a lost ack): the relay reports duplicates.
    let mut t = RelayTransport::new(relay.clone(), "g1");
    let client_id = a.sync_get("client_id").unwrap().unwrap();
    use utopia_sync::Transport;
    let (_, page, client_seq, payload) = pending[0].clone();
    let resp = t
        .push(
            &page,
            utopia_sync::PushRequest {
                updates: vec![utopia_sync::UpdateIn {
                    id: format!("{client_id}:{client_seq}"),
                    client_seq,
                    frontier: String::new(),
                    payload,
                }],
            },
        )
        .unwrap();
    assert_eq!((resp.accepted, resp.duplicate), (0, 1));
}

/// §9.2/§15.1: the offline status shows how many changes are queued, and computing it on
/// the UI thread must not read the payloads. Counting has to survive a row whose payload
/// cannot be decoded as bytes; loading the whole outbox does not.
#[test]
fn the_offline_status_counts_the_outbox_without_reading_payloads() {
    let relay = Relay::new();
    let (mut s, _c) = device(&relay);
    let p = s.create_page("Queued").unwrap();
    write(&mut s, &p, "one");
    write(&mut s, &p, "two");
    let pending = s.outbox_pending(usize::MAX >> 1).unwrap().len();
    assert!(pending >= 2, "the edits are queued");
    let offline =
        SyncStatus::from_result(&Err(utopia_sync::SyncError::Transport("down".into())), &s);
    assert_eq!(offline, SyncStatus::Offline { queued: pending });
    // An undecodable payload is still a queued change; the count must not depend on
    // deserialising payload bytes (rule 10: no outbox read on the UI thread).
    s.conn()
        .execute(
            "INSERT INTO outbox (page_id, payload, client_seq, created_at) VALUES ('p', 3, 99, 0)",
            [],
        )
        .unwrap();
    assert_eq!(
        SyncStatus::from_result(&Err(utopia_sync::SyncError::Transport("down".into())), &s),
        SyncStatus::Offline {
            queued: pending + 1
        }
    );
}

/// §12.6 (rule 5): a queued compaction snapshot belongs to the relay that measured the head
/// it covers. Retaining a [`utopia_sync::SyncEngine`] across calls (the mobile FFI keeps one
/// for the graph, docs/state-machines/ffi-mobile-sync.md) must not upload it to a different
/// destination, whose log it would truncate past updates this device never imported.
#[test]
fn ac_12_6_a_snapshot_measured_on_another_relay_is_not_uploaded() {
    use std::sync::atomic::Ordering;
    use utopia_sync::{
        exchange, CursorKind, CursorSpace, Outgoing, PushRequest, QueuedSnapshot, Transport,
        UpdateIn,
    };

    let relay = Relay::new();
    let page = PageId("elsewhere".into());
    // The destination already holds history from another device.
    let mut other = RelayTransport::new(relay.clone(), "g1");
    for i in 0..5u64 {
        other
            .push(
                &page,
                PushRequest {
                    updates: vec![UpdateIn {
                        id: format!("other:{i}"),
                        client_seq: i as i64,
                        frontier: String::new(),
                        payload: vec![i as u8, 1, 2],
                    }],
                },
            )
            .unwrap();
    }
    let head = relay.manifest("g1").pages[0].head_seq;
    let here = other.cursor_space();
    let elsewhere = CursorSpace::new("relay:not-this-one#g1", CursorKind::Seq);
    let queued = |space: CursorSpace| Outgoing {
        snapshots: vec![QueuedSnapshot {
            page: page.clone(),
            covers_seq: head,
            space,
            payload: vec![9, 9, 9],
        }],
        ..Default::default()
    };
    let counting = || Counting {
        inner: RelayTransport::new(relay.clone(), "g1"),
        puts: Default::default(),
        fail_pulls: true,
    };

    // Measured on another relay: dropped, and this relay's entries survive.
    let mut t = counting();
    let inc = exchange(&mut t, queued(elsewhere)).unwrap();
    assert_eq!(inc.snapshots_sent, 0);
    assert_eq!(
        t.puts.load(Ordering::SeqCst),
        0,
        "another relay's snapshot was uploaded"
    );
    assert_eq!(
        relay.log_len("g1", page.as_str()),
        5,
        "the destination's log was truncated by another relay's snapshot"
    );

    // Measured here: the same snapshot is legitimate and goes out.
    let mut t = counting();
    let inc = exchange(&mut t, queued(here)).unwrap();
    assert_eq!(inc.snapshots_sent, 1);
    assert_eq!(t.puts.load(Ordering::SeqCst), 1);
}

/// §12.3: the store is not locked while a round is on the wire, so an update can be queued
/// after `prepare` gathered nothing. The round must make another pass instead of reporting
/// a clean sync that left it behind (docs/state-machines/ffi-mobile-sync.md).
#[test]
fn ac_12_3_a_round_picks_up_an_edit_queued_while_it_was_on_the_wire() {
    use utopia_sync::{sync_round, StoreDone, StoreStep, SyncEngine};

    let relay = Relay::new();
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Round").unwrap();
    // Send the page creation first, so the round under test starts with an empty outbox.
    SyncClient::new(RelayTransport::new(relay.clone(), "g1"), &store)
        .unwrap()
        .sync_once(&mut store)
        .unwrap();
    assert_eq!(store.outbox_count().unwrap(), 0);
    let head_before = relay.manifest("g1").pages[0].head_seq;

    let mut engine = SyncEngine::new(&store).unwrap();
    let mut transport = RelayTransport::new(relay.clone(), "g1");
    let mut captured = false;
    let report = sync_round(&mut engine, &mut transport, |engine, step| match step {
        StoreStep::Prepare => engine.prepare(&mut store).map(StoreDone::Prepared),
        StoreStep::Apply(inc) => {
            let pass = engine.apply(&mut store, *inc)?;
            if !captured {
                // A capture lands between the round's two store steps.
                captured = true;
                write(&mut store, &page, "typed during the round");
            }
            Ok(StoreDone::Applied(pass))
        }
        StoreStep::Queued => store
            .outbox_count()
            .map(|n| StoreDone::Queued(n > 0))
            .map_err(Into::into),
    })
    .unwrap();
    assert!(
        report.pushed >= 1,
        "the edit made during the exchange was left unsent: {report:?}"
    );
    assert_eq!(store.outbox_count().unwrap(), 0, "everything was sent");
    assert!(
        relay.manifest("g1").pages[0].head_seq > head_before,
        "the capture reached the relay"
    );
}

/// §12.7: a new target needs acknowledged and imported content, not only pending edits.
#[test]
fn ac_12_7_setup_switch_republishes_acked_and_imported_pages_without_duplicates() {
    use utopia_store::Destination;
    let old = Relay::new();
    let (mut a, mut ca) = device(&old);
    let (mut b, mut cb) = device(&old);
    let page = a.create_page("Keep every block").unwrap();
    write(&mut a, &page, "written on A");
    // Setup records the destination this graph publishes to, before it is switched.
    let old_space = utopia_sync::Transport::cursor_space(ca.transport_mut()).transport;
    a.prepare_sync_destination(&Destination::new(&old_space, None))
        .unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    write(&mut b, &page, "written on B");
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    let before = texts(&mut a, &page);
    let new = Relay::new();
    let mut moved = SyncClient::new(RelayTransport::new(new.clone(), "g1"), &a).unwrap();
    let new_space = utopia_sync::Transport::cursor_space(moved.transport_mut()).transport;
    a.prepare_sync_destination(&Destination::new(new_space, None))
        .unwrap();
    moved.sync_once(&mut a).unwrap();
    let (mut c, mut cc) = device(&new);
    cc.sync_once(&mut c).unwrap();
    assert_eq!(texts(&mut c, &page), before);
    a.prepare_sync_destination(&Destination::new(&old_space, None))
        .unwrap();
    settle(&mut [(&mut a, &mut ca), (&mut b, &mut cb)]);
    assert_eq!(texts(&mut b, &page), before);
    assert_eq!(texts(&mut a, &page), before);
    assert!(a.doctor().unwrap().is_ok());
}

/// §12.6: a second device's wrong key must not mix new ciphertext into an existing graph.
#[test]
fn ac_12_6_pairing_validates_existing_payloads_before_pushing() {
    use utopia_sync::{
        e2e::{Encrypted, GraphKey},
        setup::verified_target::VerifiedTarget,
        Transport,
    };
    let relay = Relay::new();
    let mut a = Store::open_in_memory().unwrap();
    a.create_page("Encrypted original").unwrap();
    let transport = || RelayTransport::new(relay.clone(), "g");
    let key = |pass| GraphKey::derive(pass, "g").unwrap();
    let mut original =
        SyncClient::new(Encrypted::new(transport(), key("original"), "g"), &a).unwrap();
    original.sync_once(&mut a).unwrap();
    let before = relay.manifest("g").rev;
    let mut b = Store::open_in_memory().unwrap();
    b.create_page("Keep this local until paired").unwrap();
    let mut wrong = SyncClient::new(
        VerifiedTarget::new(Encrypted::new(transport(), key("wrong"), "g")),
        &b,
    )
    .unwrap();
    assert!(wrong.sync_once(&mut b).is_err());
    assert_eq!(relay.manifest("g").rev, before);
    let mut unencrypted = VerifiedTarget::new(transport());
    assert!(unencrypted.manifest(None).is_err());
    assert_eq!(relay.manifest("g").rev, before);
    let mut correct = SyncClient::new(
        VerifiedTarget::new(Encrypted::new(transport(), key("original"), "g")),
        &b,
    )
    .unwrap();
    correct.sync_once(&mut b).unwrap();
    assert!(relay.manifest("g").rev > before);
    assert!(b.find_page("Encrypted original").unwrap().is_some());
}

/// §12.7 / rule 12: publication acknowledgements belong to a destination, including blobs.
#[test]
fn ac_12_7_setup_switch_republishes_uploaded_blobs_to_a_folder() {
    use utopia_sync::folder::FolderTransport;
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let page = a.create_page("Attachments survive switching").unwrap();
    let bytes = b"mock attachment for sync setup";
    let blob = a.add_blob("example.txt", bytes).unwrap();
    write(&mut a, &page, &format!("[example.txt](blob:{})", blob.hash));
    ca.sync_once(&mut a).unwrap();
    assert_eq!(
        a.attachment_state(&blob.hash).unwrap(),
        Some(utopia_store::BlobState::Uploaded)
    );
    // The relay is the destination this graph recorded, so the folder is a real switch.
    let relay_space = utopia_sync::Transport::cursor_space(ca.transport_mut()).transport;
    a.prepare_sync_destination(&utopia_store::Destination::new(relay_space, None))
        .unwrap();
    let folder = tempfile::tempdir().unwrap();
    let shared =
        utopia_store::Destination::new(format!("folder:{}", folder.path().display()), None);
    a.prepare_sync_destination(&shared).unwrap();
    let queued = a.outbox_count().unwrap();
    a.prepare_sync_destination(&shared).unwrap();
    assert_eq!(
        a.outbox_count().unwrap(),
        queued,
        "repeated Apply must not enqueue again"
    );
    let mut moved = SyncClient::new(FolderTransport::new(folder.path(), "a").unwrap(), &a).unwrap();
    moved.sync_once(&mut a).unwrap();
    let mut b = Store::open_in_memory().unwrap();
    let mut reader =
        SyncClient::new(FolderTransport::new(folder.path(), "b").unwrap(), &b).unwrap();
    reader.sync_once(&mut b).unwrap();
    assert_eq!(texts(&mut a, &page), texts(&mut b, &page));
    assert_eq!(
        b.blob(&blob.hash).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
}

/// §12.7: the identity a launch observes is not the same online and offline. A relay is
/// only identified by its own store id once its manifest has been read; an offline launch
/// knows just the URL, and that difference must not look like a new destination — every
/// flip would export the whole graph into the outbox again.
#[test]
fn ac_12_7_an_offline_launch_does_not_republish_the_graph() {
    let mut a = Store::open_in_memory().unwrap();
    let page = a.create_page("Offline launches").unwrap();
    write(&mut a, &page, "one block");
    let origin = utopia_sync::http::destination_origin("http://relay.example", "g1");
    let online = utopia_store::Destination::new(origin.clone(), Some("store-1".into()));
    let offline = utopia_store::Destination::new(origin, None);
    a.prepare_sync_destination(&online).unwrap();
    let queued = a.outbox_pending(usize::MAX >> 1).unwrap().len();
    a.prepare_sync_destination(&offline).unwrap();
    assert_eq!(
        a.outbox_pending(usize::MAX >> 1).unwrap().len(),
        queued,
        "the URL-only identity of an offline launch republished the whole graph"
    );
}

/// §12.7: a graph that was already syncing when this bookkeeping was introduced has no
/// recorded destination. The first launch must adopt the current one instead of exporting
/// every shard to the target it is already in sync with.
#[test]
fn ac_12_7_the_first_recorded_destination_does_not_republish_an_existing_graph() {
    let relay = Relay::new();
    let (mut a, mut ca) = device(&relay);
    let page = a.create_page("Already synced").unwrap();
    write(&mut a, &page, "old content");
    ca.sync_once(&mut a).unwrap();
    assert_eq!(a.sync_get("setup_destination").unwrap(), None);
    let queued = a.outbox_pending(usize::MAX >> 1).unwrap().len();
    assert_eq!(queued, 0, "the previous round acknowledged everything");
    let instance = utopia_sync::setup::probe::probe(ca.transport_mut()).instance;
    let origin = utopia_sync::http::destination_origin("http://relay.example", "g1");
    a.prepare_sync_destination(&utopia_store::Destination::new(origin, instance))
        .unwrap();
    assert_eq!(
        a.outbox_pending(usize::MAX >> 1).unwrap().len(),
        queued,
        "the upgrade republished the whole graph into its existing target"
    );
}
/// §12.6: an empty target has nothing to read back, so it proves no key. A device that
/// proved its passphrase there must re-check before its first push once foreign payloads
/// exist — otherwise it mixes ciphertext into a graph encrypted with another key.
#[test]
fn ac_12_6_an_empty_target_never_proves_a_key() {
    use utopia_sync::{
        e2e::{Encrypted, GraphKey},
        setup::verified_target::VerifiedTarget,
    };
    let relay = Relay::new();
    let transport = || RelayTransport::new(relay.clone(), "g");
    let key = |pass| GraphKey::derive(pass, "g").unwrap();
    // A joins while the relay is empty and publishes nothing: it learned no key.
    let mut a = Store::open_in_memory().unwrap();
    let mut ca = SyncClient::new(
        VerifiedTarget::new(Encrypted::new(transport(), key("original"), "g")),
        &a,
    )
    .unwrap();
    ca.sync_once(&mut a).unwrap();
    // B's wrong key arrives at the still-empty relay and publishes.
    let mut b = Store::open_in_memory().unwrap();
    let page_b = b.create_page("B's notebook").unwrap();
    write(&mut b, &page_b, "encrypted with the wrong key");
    let mut cb = SyncClient::new(
        VerifiedTarget::new(Encrypted::new(transport(), key("wrong"), "g")),
        &b,
    )
    .unwrap();
    cb.sync_once(&mut b).unwrap();
    let pushed = relay.manifest("g");
    assert_eq!(pushed.pages.len(), 1);
    // A now edits and syncs: it must notice it cannot read the target before adding its
    // own ciphertext to it.
    let page_a = a.create_page("A's notebook").unwrap();
    write(&mut a, &page_a, "encrypted with the original key");
    assert!(
        ca.sync_once(&mut a).is_err(),
        "the round must refuse a target it cannot read"
    );
    assert_eq!(
        relay.manifest("g").pages.len(),
        pushed.pages.len(),
        "A pushed ciphertext into a graph it cannot read"
    );
}
