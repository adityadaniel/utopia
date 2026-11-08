//! Sync (PRD §12): a deliberately dumb relay that stores opaque CRDT updates per page,
//! and a client loop that pushes the outbox and pulls what it has not seen.
//!
//! - [`wire`]: the v1 protocol shapes (§12.3), JSON with base64 payloads.
//! - [`Transport`]: what a client needs from a relay. HTTP, folder and LAN transports
//!   (§12.7) implement the same trait over the same update format.
//! - [`relay::Relay`]: the reference server semantics (§12.8) in memory: dedupe by
//!   change id, monotonic per-page `seq`, snapshot truncation, manifest with `rev`.
//! - [`SyncClient`]: the client loop (§12.3 steps 1–5).
//!
//! The relay never parses payloads, so it needs no CRDT dependency and works unchanged
//! once payloads are end-to-end encrypted (§11.7). Because the relay cannot interpret a
//! Loro frontier, pulls are addressed by the relay's own `seq`; frontiers are carried
//! opaquely for diagnostics.

pub mod e2e;
pub mod folder;
pub mod http;
pub mod realtime;
pub mod relay;

/// The v1 wire format, shared with the relay server.
pub use utopia_proto as wire;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use utopia_core::PageId;
use utopia_doc::Frontier;
use utopia_store::{Store, StoreError};

pub use wire::{
    Manifest, ManifestPage, PullResponse, PushRequest, PushResponse, UpdateIn, UpdateOut,
};

pub use utopia_proto::PROTOCOL_VERSION;
/// Max bytes per push batch (§12.3 step 2).
pub const MAX_PUSH_BYTES: usize = 1 << 20;
/// Upload a fresh snapshot when a page's server tail exceeds either bound (§12.3 step 5).
pub const SNAPSHOT_TAIL_BYTES: u64 = 256 * 1024;
/// Attachments moved per direction per round (memory stays bounded; the loop continues).
pub const BLOBS_PER_ROUND: usize = 8;
pub const SNAPSHOT_TAIL_UPDATES: u64 = 200;

#[derive(Debug, Error)]
pub enum SyncError {
    /// 401/403: halt sync and re-authenticate; local editing continues (§12.6).
    #[error("unauthorized")]
    Unauthorized,
    /// Network failure or 5xx: retry with backoff; the outbox keeps every payload.
    #[error("transport: {0}")]
    Transport(String),
    /// The relay speaks a protocol this client does not; never push (§12.6).
    #[error("unsupported protocol version {0}")]
    ProtocolVersion(u32),
    #[error(transparent)]
    Store(#[from] StoreError),
}

pub type Result<T, E = SyncError> = std::result::Result<T, E>;

/// A failure that stopped one page in a round that kept running (§12.3). The page's
/// outbox rows stay queued and the next round retries them; the other pages are
/// already done and stay done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The page that failed; `None` for the manifest or an attachment.
    pub page: Option<PageId>,
    pub kind: FailureKind,
}

/// Why one page failed, flattened so a report can be compared and shown without
/// carrying a whole [`SyncError`] (which may wrap a store error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureKind {
    Unauthorized,
    ProtocolVersion(u32),
    /// Retryable: transport, store or payload problem.
    Transport(String),
    Store(String),
}

impl FailureKind {
    pub fn of(e: &SyncError) -> Self {
        match e {
            SyncError::Unauthorized => FailureKind::Unauthorized,
            SyncError::ProtocolVersion(v) => FailureKind::ProtocolVersion(*v),
            SyncError::Transport(m) => FailureKind::Transport(m.clone()),
            SyncError::Store(e) => FailureKind::Store(e.to_string()),
        }
    }
}

impl Failure {
    pub fn page(page: &PageId, e: &SyncError) -> Self {
        Self {
            page: Some(page.clone()),
            kind: FailureKind::of(e),
        }
    }

    pub fn object(e: &SyncError) -> Self {
        Self {
            page: None,
            kind: FailureKind::of(e),
        }
    }

    /// Bad credentials and an unsupported protocol will fail every later call too, so the
    /// round stops instead of marking the rest of the pages failed (§12.6).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.kind,
            FailureKind::Unauthorized | FailureKind::ProtocolVersion(_)
        )
    }

    /// The failure as an error, for callers that need a round's error: a round that moved
    /// nothing is still an [`Err`] (Offline, NeedsAuth, …) even when it was per page.
    pub fn to_error(&self) -> SyncError {
        match &self.kind {
            FailureKind::Unauthorized => SyncError::Unauthorized,
            FailureKind::ProtocolVersion(v) => SyncError::ProtocolVersion(*v),
            FailureKind::Transport(m) => SyncError::Transport(m.clone()),
            // A store failure is local, but it is still retryable and still leaves the
            // outbox queued; the message says where it came from.
            FailureKind::Store(m) => SyncError::Transport(format!("store: {m}")),
        }
    }
}

/// How a transport's `head_seq` values are compared with a stored [`Cursor`]
/// (§12.3 step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorKind {
    /// A monotonic relay sequence: pull when `head > local`.
    Seq,
    /// A content revision (the folder transport's file digest): pull when `head != local`.
    Revision,
}

/// Which transport measured a `head_seq`, and how to compare it. A cursor is only
/// compared with a `head_seq` from the same space, so switching from a folder to a relay
/// (or to another relay) pulls everything once instead of comparing a file count with a
/// relay sequence (or a fresh relay's `3` with an old one's `500`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorSpace {
    /// This transport's identity, e.g. `relay:https://host#graph` or `folder:/path`.
    pub transport: String,
    pub kind: CursorKind,
}

impl CursorSpace {
    pub fn new(transport: impl Into<String>, kind: CursorKind) -> Self {
        Self {
            transport: transport.into(),
            kind,
        }
    }
}

/// The pull position of one page (§12.3 step 4), tagged with the space it lives in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub space: CursorSpace,
    pub pos: u64,
}

/// The manifest revision last fully applied, tagged with the cursor space that issued it
/// (§12.3 step 1). A rev from another relay or folder must never answer "not modified":
/// two relays can be at the same counter while holding different pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRev {
    pub space: CursorSpace,
    pub rev: u64,
}

impl ManifestRev {
    pub fn encode(&self) -> String {
        // A plain struct of a string, an enum tag and a u64 cannot fail to serialize.
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn decode(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }
}

impl Cursor {
    pub fn encode(&self) -> String {
        // A plain struct of a string, an enum tag and a u64 cannot fail to serialize.
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn decode(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    /// Whether `head` in `space` is already covered by this cursor.
    pub fn covers(&self, space: &CursorSpace, head: u64) -> bool {
        &self.space == space
            && match space.kind {
                CursorKind::Seq => head <= self.pos,
                CursorKind::Revision => head == self.pos,
            }
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>> {
        (**self).manifest(if_none_match)
    }
    fn cursor_space(&self) -> CursorSpace {
        (**self).cursor_space()
    }
    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
        (**self).push(page, req)
    }
    fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse> {
        (**self).pull(page, since_seq)
    }
    fn put_snapshot(&mut self, page: &PageId, covers_seq: u64, payload: Vec<u8>) -> Result<()> {
        (**self).put_snapshot(page, covers_seq, payload)
    }
    fn put_blob(&mut self, hash: &str, bytes: Vec<u8>) -> Result<bool> {
        (**self).put_blob(hash, bytes)
    }
    fn get_blob(&mut self, hash: &str) -> Result<Option<Vec<u8>>> {
        (**self).get_blob(hash)
    }
}

/// What a client needs from a relay (§12.3). One instance talks to one graph.
pub trait Transport {
    /// `GET manifest` with `If-None-Match: rev`; `None` means 304 Not Modified.
    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>>;
    /// The space this transport's `head_seq` values are measured in. Two transports of
    /// different kinds (or different relays) never share a cursor.
    fn cursor_space(&self) -> CursorSpace;
    /// `POST pages/{pid}/updates`.
    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse>;
    /// `GET pages/{pid}/updates?since={seq}`.
    fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse>;
    /// `POST pages/{pid}/snapshot`: replaces the log up to `covers_seq`.
    fn put_snapshot(&mut self, page: &PageId, covers_seq: u64, payload: Vec<u8>) -> Result<()>;
    /// `PUT blobs/{sha256}` (P2 attachments). `false`: this transport keeps no blobs.
    fn put_blob(&mut self, _hash: &str, _bytes: Vec<u8>) -> Result<bool> {
        Ok(false)
    }
    /// `GET blobs/{sha256}`; `None` when the relay does not have it (yet).
    fn get_blob(&mut self, _hash: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub pushed: usize,
    pub duplicates: usize,
    pub pulled_pages: usize,
    pub imported: Vec<PageId>,
    pub snapshots: usize,
    pub blobs_up: usize,
    pub blobs_down: usize,
    /// Pages that failed while the rest of the round carried on; empty on a clean round.
    pub failed: Vec<Failure>,
}

impl SyncReport {
    /// Fold one pass of a round into the whole, keeping one entry per failed page.
    fn absorb(&mut self, other: SyncReport) {
        self.pushed += other.pushed;
        self.duplicates += other.duplicates;
        self.pulled_pages += other.pulled_pages;
        self.snapshots += other.snapshots;
        self.blobs_up += other.blobs_up;
        self.blobs_down += other.blobs_down;
        for p in other.imported {
            if !self.imported.contains(&p) {
                self.imported.push(p);
            }
        }
        self.failed.extend(other.failed);
    }

    /// Whether anything moved in either direction.
    pub fn is_quiet(&self) -> bool {
        self.pushed == 0 && self.imported.is_empty() && self.snapshots == 0
    }
}

/// Status shown in the sidebar (§9.2, §9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    Synced {
        at_ms: i64,
    },
    /// The relay was reachable and the round applied what it could, but some pages failed:
    /// not "Offline" (the network is up), and not plain "Synced" either (§12.6).
    Partial {
        failed: usize,
        queued: usize,
    },
    /// The relay was not reached, or answered with a transport failure: everything is
    /// still queued locally (§12.6).
    Offline {
        queued: usize,
    },
    /// The round failed on this device (the store, a shard, a projection). The network was
    /// never tried, so saying "offline" would blame the relay for a local fault.
    LocalError {
        message: String,
    },
    NeedsAuth,
    Unsupported,
}

impl SyncStatus {
    pub fn from_result(r: &Result<SyncReport>, store: &Store) -> Self {
        // A count, not the rows: this runs on the UI thread on every backoff round with no
        // network, and payloads can be large (rule 10).
        let queued = || store.outbox_count().unwrap_or(0);
        match r {
            Ok(r) if r.failed.is_empty() => SyncStatus::Synced { at_ms: store.now() },
            Ok(r) => {
                let failed = r
                    .failed
                    .iter()
                    .map(|f| f.page.clone())
                    .collect::<BTreeSet<_>>()
                    .len();
                SyncStatus::Partial {
                    failed,
                    queued: queued(),
                }
            }
            Err(SyncError::Unauthorized) => SyncStatus::NeedsAuth,
            Err(SyncError::ProtocolVersion(_)) => SyncStatus::Unsupported,
            Err(SyncError::Store(e)) => SyncStatus::LocalError {
                message: e.to_string(),
            },
            Err(_) => SyncStatus::Offline { queued: queued() },
        }
    }
}

/// A compaction snapshot queued for upload (§12.3 step 5), bound to the relay that asked
/// for it.
///
/// The snapshot is this device's whole page, so it may only truncate a log whose head this
/// device actually covered. Uploading it to another relay — or to the same graph id on
/// another server — would delete that relay's entries up to `covers_seq`, which this
/// snapshot may not contain (AGENTS.md rule 5). [`exchange`] therefore drops a snapshot
/// whose space is not the destination's; a destination that still wants compaction asks for
/// it on its own manifest and gets a fresh snapshot on a later round.
#[derive(Debug, Clone)]
pub struct QueuedSnapshot {
    pub page: PageId,
    pub covers_seq: u64,
    /// The cursor space whose head `covers_seq` was measured in.
    pub space: CursorSpace,
    pub payload: Vec<u8>,
}

/// Everything a round needs from the store, gathered on the store's thread.
#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    /// `(page, outbox seqs, request)`, each request at most [`MAX_PUSH_BYTES`].
    pub pushes: Vec<(PageId, Vec<i64>, PushRequest)>,
    /// Where each page was pulled to, tagged with the transport that measured it.
    pub cursors: BTreeMap<PageId, Cursor>,
    /// The last manifest this client fully applied, in the space that issued it.
    pub rev: Option<ManifestRev>,
    pub snapshots: Vec<QueuedSnapshot>,
    /// Attachments to upload: `(sha256, bytes)`.
    pub blob_puts: Vec<(String, Vec<u8>)>,
    /// Attachments referenced here but missing locally.
    pub blob_wants: Vec<String>,
}

/// What one exchange did to one page (§12.3 steps 2–4): the one place the round's
/// per-page outcome lives, instead of three loose lists on [`Incoming`].
#[derive(Debug, Clone, Default)]
pub struct PageExchange {
    /// Outbox rows this page's pushes were acknowledged for (accepted or duplicate).
    pub sent: Vec<i64>,
    pub accepted: usize,
    pub duplicates: usize,
    /// This page's pull result when the relay was ahead of the local cursor.
    pub pull: Option<PullResponse>,
}

/// What came back from the relay; applied on the store's thread.
#[derive(Debug, Clone, Default)]
pub struct Incoming {
    /// Per-page outcome, keyed by page.
    pub pages: BTreeMap<PageId, PageExchange>,
    pub snapshots_sent: usize,
    pub manifest: Option<Manifest>,
    /// The space the `head_seq` values above were measured in.
    pub cursor_space: Option<CursorSpace>,
    /// Pages whose server tail is long enough to compact.
    pub compact: Vec<(PageId, u64)>,
    /// Uploaded attachment hashes.
    pub blobs_put: Vec<String>,
    /// Downloaded attachments, not yet verified.
    pub blobs_got: Vec<(String, Vec<u8>)>,
    /// Retryable failures that did not stop the round (one page, or one attachment).
    /// Their outbox rows and cursors are left untouched, so the next round retries only
    /// them.
    pub failed: Vec<Failure>,
}

fn cursor_key(page: &PageId) -> String {
    format!("cursor:{page}")
}

/// Refuse a relay that speaks another protocol before this client writes to it (§12.6).
/// `None` is a 304: there was no manifest to check.
fn check_protocol(m: &Option<Manifest>) -> Result<()> {
    match m {
        Some(m) if m.protocol_version != PROTOCOL_VERSION => {
            Err(SyncError::ProtocolVersion(m.protocol_version))
        }
        _ => Ok(()),
    }
}

/// The network half of a round (§12.3 steps 1–2, 5): touches only the transport, so it
/// can run on a background thread while the UI keeps editing.
pub fn exchange<T: Transport + ?Sized>(t: &mut T, out: Outgoing) -> Result<Incoming> {
    let mut inc = Incoming::default();
    // The transport's identity can come from its own manifest (a relay store id), so ask
    // first and take the cursor space afterwards. A conditional request is only safe when
    // the stored rev belongs to the space this transport already has; otherwise it is
    // another relay's (or folder's) counter and must not answer 304.
    //
    // It is also only safe on a read-only round: a relay's 304 compares the rev alone, so
    // an in-place protocol upgrade that keeps the rev would answer 304, skip the version
    // check below, and let this client push to an unsupported relay. A round with pushes
    // or snapshots therefore fetches the manifest unconditionally first (§12.6).
    let writes = !out.pushes.is_empty() || !out.snapshots.is_empty();
    let known = t.cursor_space();
    let if_none_match = if writes {
        None
    } else {
        out.rev.as_ref().filter(|r| r.space == known).map(|r| r.rev)
    };
    let first = t.manifest(if_none_match)?;
    check_protocol(&first)?;
    let space = t.cursor_space();
    // Batches of one page are causally ordered (`prepare` splits a page's outbox by
    // `MAX_PUSH_BYTES`). Once one is refused, a later batch must not reach the relay
    // without its predecessors: a peer could pull it, advance its cursor over an update
    // Loro only holds pending, and lose it on restart. Other pages still go through.
    let mut refused: BTreeSet<PageId> = BTreeSet::new();
    for (page, seqs, req) in out.pushes {
        if refused.contains(&page) {
            continue;
        }
        match t.push(&page, req) {
            Ok(resp) => {
                // Duplicates are success: the relay already has them (§12.6, 409).
                let done = inc.pages.entry(page).or_default();
                done.sent.extend(seqs);
                done.accepted += resp.accepted;
                done.duplicates += resp.duplicate;
            }
            Err(e) => {
                // One page the relay refuses must not cost the other pages their acks, nor
                // the pulls below their chance to run (§12.3).
                let f = Failure::page(&page, &e);
                if f.is_terminal() {
                    return Err(e);
                }
                refused.insert(page);
                inc.failed.push(f);
            }
        }
    }
    for snap in out.snapshots {
        if snap.space != space {
            // The destination changed since this snapshot was measured (another relay, or
            // the same graph id on another server): sending it would truncate a log this
            // device never read, to a head it may not have (rule 5). Drop it; the
            // destination that wants compaction re-queues one.
            continue;
        }
        let page = snap.page;
        match t.put_snapshot(&page, snap.covers_seq, snap.payload) {
            Ok(()) => inc.snapshots_sent += 1,
            Err(e) => {
                let f = Failure::page(&page, &e);
                if f.is_terminal() {
                    return Err(e);
                }
                inc.failed.push(f);
            }
        }
    }
    for (hash, bytes) in out.blob_puts {
        match t.put_blob(&hash, bytes) {
            Ok(true) => inc.blobs_put.push(hash),
            Ok(false) => {}
            Err(e) => {
                let f = Failure::object(&e);
                if f.is_terminal() {
                    return Err(e);
                }
                inc.failed.push(f);
            }
        }
    }
    for hash in out.blob_wants {
        match t.get_blob(&hash) {
            Ok(Some(bytes)) => inc.blobs_got.push((hash, bytes)),
            Ok(None) => {}
            Err(e) => {
                let f = Failure::object(&e);
                if f.is_terminal() {
                    return Err(e);
                }
                inc.failed.push(f);
            }
        }
    }
    let manifest = if inc.pages.values().all(|p| p.sent.is_empty()) && inc.snapshots_sent == 0 {
        // `first` is already `None` when the manifest was unchanged, and carries the newer
        // rev otherwise.
        first
    } else {
        // What was pushed may have changed the manifest, so ask again unconditionally.
        // Failing to re-read it must not throw away the acks this round just earned: the
        // rows stay marked sent, no cursor moves, and the next round pulls normally.
        match t.manifest(None) {
            Ok(m) => {
                check_protocol(&m)?;
                m
            }
            Err(e) => {
                let f = Failure::object(&e);
                if f.is_terminal() {
                    return Err(e);
                }
                inc.failed.push(f);
                None
            }
        }
    };
    if let Some(m) = &manifest {
        for p in &m.pages {
            let page = PageId(p.page_id.clone());
            // A cursor from another transport (or another relay) is not comparable with
            // this manifest's `head_seq`: treat the page as never pulled (§12.3 step 4).
            let local = out.cursors.get(&page).filter(|c| c.space == space);
            let up_to_date = local.is_some_and(|c| c.covers(&space, p.head_seq));
            if !up_to_date {
                let since = local.map_or(0, |c| c.pos);
                match t.pull(&page, since) {
                    Ok(resp) => {
                        inc.pages.entry(page.clone()).or_default().pull = Some(resp);
                    }
                    Err(e) => {
                        // One page the relay serves but this device cannot read (an E2E
                        // payload from a device without the passphrase, a corrupt update)
                        // must not cost every other page its pull (§12.6).
                        let f = Failure::page(&page, &e);
                        if f.is_terminal() {
                            return Err(e);
                        }
                        inc.failed.push(f);
                    }
                }
            }
            let tail = p.head_seq.saturating_sub(p.snapshot_seq);
            if p.tail_bytes > SNAPSHOT_TAIL_BYTES || tail > SNAPSHOT_TAIL_UPDATES {
                inc.compact.push((page, p.head_seq));
            }
        }
    }
    inc.cursor_space = Some(space);
    inc.manifest = manifest;
    Ok(inc)
}

/// The store half of the client (§12.3 steps 3–5), independent of any transport.
pub struct SyncEngine {
    client_id: String,
    pending_snapshots: Vec<QueuedSnapshot>,
}

impl SyncEngine {
    pub fn new(store: &Store) -> Result<Self> {
        let client_id = match store.sync_get("client_id")? {
            Some(id) => id,
            None => {
                let id = ulid::Ulid::new().to_string();
                store.sync_set("client_id", &id)?;
                id
            }
        };
        // Retire the pre-`cursor:` positions once: a bare `seq:<page>` is a relay sequence
        // with no transport tag and no file-count meaning, so it is not comparable with
        // any head. Dropping it makes the first round pull every page once, idempotently
        // (§12.3 step 4).
        let stale = store.sync_delete_prefixed("seq")?;
        if stale > 0 {
            eprintln!("utopia-sync: dropped {stale} pre-cursor sync position(s)");
        }
        Ok(Self {
            client_id,
            pending_snapshots: Vec::new(),
        })
    }

    /// This device's sync client id (stable per graph file).
    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    /// Gather pushes (outbox), local pull positions and queued snapshot uploads.
    pub fn prepare(&mut self, store: &mut Store) -> Result<Outgoing> {
        store.sync_index()?;
        let mut out = Outgoing {
            rev: store
                .sync_get("rev")?
                .and_then(|raw| ManifestRev::decode(&raw)),
            snapshots: std::mem::take(&mut self.pending_snapshots),
            ..Default::default()
        };
        let mut by_page: BTreeMap<PageId, Vec<(i64, i64, Vec<u8>)>> = BTreeMap::new();
        for (seq, page, client_seq, payload) in store.outbox_pending(10_000)? {
            by_page
                .entry(page)
                .or_default()
                .push((seq, client_seq, payload));
        }
        for (page, rows) in by_page {
            let frontier = store.frontier(&page).unwrap_or_default();
            let mut seqs = Vec::new();
            let mut updates = Vec::new();
            let mut bytes = 0;
            for (seq, client_seq, payload) in rows {
                if bytes + payload.len() > MAX_PUSH_BYTES && !updates.is_empty() {
                    out.pushes.push((
                        page.clone(),
                        std::mem::take(&mut seqs),
                        PushRequest {
                            updates: std::mem::take(&mut updates),
                        },
                    ));
                    bytes = 0;
                }
                bytes += payload.len();
                seqs.push(seq);
                updates.push(UpdateIn {
                    id: format!("{}:{client_seq}", self.client_id),
                    client_seq,
                    frontier: frontier.0.clone(),
                    payload,
                });
            }
            if !updates.is_empty() {
                out.pushes.push((page, seqs, PushRequest { updates }));
            }
        }
        for (k, raw) in store.sync_prefixed("cursor")? {
            match Cursor::decode(&raw) {
                Some(c) => {
                    out.cursors.insert(PageId(k), c);
                }
                // A hand-edited or older value is ignored: the page is pulled again, which
                // imports idempotently (§12.3 step 4).
                None => eprintln!("utopia-sync: ignoring unreadable cursor for page {k}"),
            }
        }
        out.blob_puts = store.blobs_to_upload(BLOBS_PER_ROUND)?;
        out.blob_wants = store.missing_blobs()?;
        out.blob_wants.truncate(BLOBS_PER_ROUND);
        Ok(out)
    }

    /// Mark pushes sent, import pulled updates (idempotent), record positions, and queue
    /// compaction snapshots for pages this client is fully caught up on.
    ///
    /// A page's acks and its cursor are recorded only after its payload is stored: an
    /// import that fails leaves that page queued and unadvanced, so the next round retries
    /// it instead of believing it synced (§12.3 steps 3–4).
    pub fn apply(&mut self, store: &mut Store, inc: Incoming) -> Result<SyncReport> {
        store.mark_blobs_uploaded(&inc.blobs_put)?;
        let mut blobs_in = 0;
        for (hash, bytes) in &inc.blobs_got {
            // A blob that does not match its hash is dropped, never stored (§15.6).
            match store.put_synced_blob(hash, bytes) {
                Ok(()) => blobs_in += 1,
                Err(StoreError::Invalid(e)) => eprintln!("utopia-sync: {e}"),
                Err(e) => return Err(e.into()),
            }
        }
        let mut report = SyncReport {
            pushed: inc.pages.values().map(|p| p.accepted).sum(),
            duplicates: inc.pages.values().map(|p| p.duplicates).sum(),
            snapshots: inc.snapshots_sent,
            pulled_pages: inc.pages.values().filter(|p| p.pull.is_some()).count(),
            blobs_up: inc.blobs_put.len(),
            blobs_down: blobs_in,
            failed: inc.failed,
            ..Default::default()
        };
        // One batch on the happy path, so a round keeps the merge ordering it had before
        // per-page outcomes existed: every remote update is in memory before a page is
        // deduped.
        let mut batch: Vec<(PageId, Vec<u8>)> = Vec::new();
        let mut pulled: BTreeMap<PageId, Vec<Vec<u8>>> = BTreeMap::new();
        for (page, done) in &inc.pages {
            let Some(resp) = &done.pull else { continue };
            let mut payloads: Vec<Vec<u8>> = Vec::new();
            if let Some(snap) = &resp.snapshot {
                payloads.push(snap.clone());
            }
            payloads.extend(resp.updates.iter().map(|u| u.payload.clone()));
            batch.extend(payloads.iter().map(|b| (page.clone(), b.clone())));
            pulled.insert(page.clone(), payloads);
        }
        // Pages whose pulled payload could not be stored.
        let mut unstored: Vec<PageId> = Vec::new();
        if !batch.is_empty() {
            match store.import_batch(&batch) {
                Ok(imported) => report.imported = imported,
                Err(_) => {
                    // Isolate the page that cannot be stored; every other page's payload is
                    // still applied (imports are idempotent, so a few are read twice).
                    for (page, payloads) in &pulled {
                        let one: Vec<(PageId, Vec<u8>)> =
                            payloads.iter().map(|b| (page.clone(), b.clone())).collect();
                        match store.import_batch(&one) {
                            Ok(imported) => {
                                for p in imported {
                                    if !report.imported.contains(&p) {
                                        report.imported.push(p);
                                    }
                                }
                            }
                            Err(e) => {
                                unstored.push(page.clone());
                                report.failed.push(Failure {
                                    page: Some(page.clone()),
                                    kind: FailureKind::Store(e.to_string()),
                                });
                            }
                        }
                    }
                }
            }
        }
        for (page, done) in inc.pages {
            if unstored.contains(&page) {
                continue;
            }
            store.outbox_mark_sent(&done.sent)?;
            if let (Some(resp), Some(space)) = (&done.pull, &inc.cursor_space) {
                let cursor = Cursor {
                    space: space.clone(),
                    pos: resp.head_seq,
                };
                store.sync_set(&cursor_key(&page), &cursor.encode())?;
            }
        }
        // Only a manifest whose pulls were all applied may be remembered: with a page
        // still failed, the next round must fetch the manifest again and retry it.
        if let (Some(m), Some(space)) = (&inc.manifest, &inc.cursor_space) {
            if report.failed.is_empty() {
                let rev = ManifestRev {
                    space: space.clone(),
                    rev: m.rev,
                };
                store.sync_set("rev", &rev.encode())?;
            }
        }
        store.sync_set("synced_at", &store.now().to_string())?;
        for (page, head) in inc.compact {
            let local = store
                .sync_get(&cursor_key(&page))?
                .and_then(|raw| Cursor::decode(&raw));
            // Only a cursor written by this very transport may cover a head it measured:
            // another space's position (a previous relay, a folder, a wiped store) says
            // nothing about what this client pulled, and a snapshot over unseen updates
            // would delete them from the relay (§12.6, rule 5).
            let covered = match (&local, &inc.cursor_space) {
                (Some(c), Some(space)) if c.covers(space, head) => Some(space),
                _ => None,
            };
            if let Some(space) = covered {
                if store.page_exists(&page)? {
                    let full = store.export_since(&page, &Frontier::default())?;
                    self.pending_snapshots.push(QueuedSnapshot {
                        page,
                        covers_seq: head,
                        space: space.clone(),
                        payload: full,
                    });
                }
            }
        }
        Ok(report)
    }

    /// Prepare → exchange → apply until nothing moves (imports may create local ops,
    /// e.g. re-merges, which must be pushed in turn).
    ///
    /// A retryable failure on one page does not fail the round: its rows stay queued and
    /// the rest of the round is applied. A round in which nothing at all moved is still an
    /// [`Err`], so "offline" keeps meaning offline (§12.6).
    pub fn sync_once<T: Transport + ?Sized>(
        &mut self,
        store: &mut Store,
        t: &mut T,
    ) -> Result<SyncReport> {
        sync_round(self, t, |engine, step| match step {
            StoreStep::Prepare => engine.prepare(store).map(StoreDone::Prepared),
            StoreStep::Apply(inc) => engine.apply(store, *inc).map(StoreDone::Applied),
            StoreStep::Queued => store
                .outbox_count()
                .map(|n| StoreDone::Queued(n > 0))
                .map_err(SyncError::from),
        })
    }
}

/// One store-side step of a round: the calls that need the store, so a caller that keeps
/// its store behind a lock can take that lock for each and release it for the network
/// between them (§12.3 steps 3–5, §14.3).
#[derive(Debug)]
pub enum StoreStep {
    /// Gather pushes, cursors, queued snapshots and attachment work.
    Prepare,
    /// Store what the relay returned: acks, imports, cursors, snapshots to upload.
    /// Boxed so the step stays one word (it is passed by value once per round, and
    /// `Prepare` carries nothing).
    Apply(Box<Incoming>),
    /// "Is anything still waiting to be sent?" Asked when the round would otherwise stop:
    /// the store is not locked between the steps, so an edit made on another thread while
    /// the exchange ran is not in the pass that was just applied.
    Queued,
}

/// What the store did with a [`StoreStep`].
#[derive(Debug)]
pub enum StoreDone {
    Prepared(Outgoing),
    Applied(SyncReport),
    /// The answer to [`StoreStep::Queued`]: text or structural updates are still unsent.
    Queued(bool),
}

/// The [`SyncEngine::sync_once`] loop, with the store reached through `store` instead of
/// borrowed for the whole round.
///
/// The mobile FFI keeps its store behind a mutex it must not hold across the network
/// (§14.3), so it runs this with a closure that locks per step; [`SyncEngine::sync_once`]
/// runs it with a closure that just calls the engine. One loop, so the two cannot disagree
/// about when a round stops.
///
pub fn sync_round<T, F>(engine: &mut SyncEngine, t: &mut T, mut store: F) -> Result<SyncReport>
where
    T: Transport + ?Sized,
    F: FnMut(&mut SyncEngine, StoreStep) -> Result<StoreDone>,
{
    let mut report = SyncReport::default();
    for _ in 0..6 {
        let out = match store(engine, StoreStep::Prepare)? {
            StoreDone::Prepared(out) => out,
            // A programmer error, not user data: the closure answered the wrong step.
            StoreDone::Applied(_) => return Err(wrong_step("apply", "prepare")),
            StoreDone::Queued(_) => return Err(wrong_step("queued", "prepare")),
        };
        let had_work = !out.pushes.is_empty() || !out.snapshots.is_empty();
        let inc = exchange(t, out)?;
        let failed = !inc.failed.is_empty();
        let pass = match store(engine, StoreStep::Apply(Box::new(inc)))? {
            StoreDone::Applied(pass) => pass,
            StoreDone::Prepared(_) => return Err(wrong_step("prepare", "apply")),
            StoreDone::Queued(_) => return Err(wrong_step("queued", "apply")),
        };
        let nothing_moved = pass.imported.is_empty()
            && !had_work
            && engine.pending_snapshots.is_empty()
            && pass.blobs_up == 0
            && pass.blobs_down == 0;
        let stopped = failed || !pass.failed.is_empty();
        report.absorb(pass);
        // Retrying a page that just failed inside the same round only re-spends the
        // same bandwidth; the next round owns it.
        if stopped {
            break;
        }
        // `prepare` ran before the exchange and the store is not locked in between, so an
        // edit made meanwhile is not in the pass above. A round that stopped here would
        // answer "synced" with that edit still queued (§12.3, §14.3).
        let quiet = nothing_moved
            && (match store(engine, StoreStep::Queued)? {
                StoreDone::Queued(pending) => !pending,
                StoreDone::Prepared(_) => return Err(wrong_step("prepare", "queued")),
                StoreDone::Applied(_) => return Err(wrong_step("apply", "queued")),
            });
        if quiet {
            break;
        }
    }
    if let Some(f) = report.failed.first() {
        let moved = report.pushed + report.duplicates > 0
            || !report.imported.is_empty()
            || report.snapshots > 0
            || report.blobs_up + report.blobs_down > 0;
        if !moved {
            return Err(f.to_error());
        }
    }
    Ok(report)
}

/// The answer to a step that was never asked. A local fault, not a network one, so it is
/// worded as a store error and never as "offline".
fn wrong_step(answered: &str, asked: &str) -> SyncError {
    StoreError::Invalid(format!(
        "sync round: the store answered `{answered}` to `{asked}`"
    ))
    .into()
}

/// A transport plus an engine: the simple synchronous client used by tests and tools.
pub struct SyncClient<T: Transport> {
    transport: T,
    engine: SyncEngine,
}

impl<T: Transport> SyncClient<T> {
    pub fn new(transport: T, store: &Store) -> Result<Self> {
        Ok(Self {
            transport,
            engine: SyncEngine::new(store)?,
        })
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Number of local updates not yet accepted by the relay.
    ///
    /// Flushes first: in the UI's debounced mode an edit lives in memory for up to 250 ms,
    /// and a count read straight from the index would say "nothing queued" while the
    /// keystrokes are still unsent (§9.3). `prepare` does the same before pushing.
    pub fn queued(store: &mut Store) -> Result<usize> {
        store.sync_index()?;
        Ok(store.outbox_pending(usize::MAX >> 1)?.len())
    }

    pub fn sync_once(&mut self, store: &mut Store) -> Result<SyncReport> {
        self.engine.sync_once(store, &mut self.transport)
    }

    /// Run a round and translate failures into a status for the UI (§9.3).
    pub fn sync_status(&mut self, store: &mut Store) -> SyncStatus {
        let r = self.sync_once(store);
        SyncStatus::from_result(&r, store)
    }
}
