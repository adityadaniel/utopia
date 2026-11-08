//! Utopia local store (PRD §10, §11): one SQLite file per graph.
//!
//! Page shards (`page_doc` + `page_update`) are the system of record. Every commit writes
//! the shard delta, the outbox entry and all projections (`page`, `alias`, `block`, `ref`,
//! `block_fts`) in **one transaction**, so the index never disagrees with the document.
//!
//! Commands apply to the in-memory shards immediately; commits are handed to a writer
//! thread (§13.3). In *strict* mode (the default: tests, CLI) every commit is shipped at
//! once and every index read waits for the writer, so reads always see prior writes. In
//! *debounced* mode (the desktop UI) commits accumulate until [`Store::flush`] and index
//! reads never wait; the UI refreshes when a [`WriteTicket`] completes.

mod attach;
mod blockquery;
mod crypt;
mod doctor;
mod export;
pub use export::{write_export, ExportSnapshot};
mod extras;
mod pagegraph;
mod pages;
mod project;
mod query;
mod schema;
mod writer;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;
use utopia_core::outline::Plan;
use utopia_core::undo::{Caret, UndoGroup, UndoStack, UndoStep};
use utopia_core::{BlockId, Command, DateFormat, DocDelta, DomainError, PageId, PageState};
use utopia_doc::{DocError, LoroPageDoc, PageDoc};

pub use attach::{sha256_hex, Attachment, BlobState, PdfHighlight, MAX_BLOB_BYTES};
pub use blockquery::{QueryHit, QueryResult};
pub use crypt::{encryption_available, is_encrypted, set_encryption};
pub use doctor::DoctorReport;
pub use extras::{ImportReport, Mention, TEMPLATE_PREFIX};
pub use pagegraph::{GraphNode, GraphOptions, PageGraph};
pub use pages::PageFate;
pub use project::{display_title, resolve_page_key};
pub use query::*;
pub use utopia_doc::DocCursor;
pub use writer::WriteTicket;

use writer::{Job, PageJob, Persister, TrashOp};

/// Fold `page_update` rows into the snapshot after this many updates.
const COMPACT_AFTER: i64 = 128;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Doc(#[from] DocError),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("commit failed: {0}")]
    Writer(String),
    /// The graph is encrypted at rest and the passphrase is missing or wrong.
    #[error("this graph is encrypted; the passphrase is missing or wrong")]
    Encrypted,
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

/// A pending outbox row: `(seq, page, client_seq, payload)`.
pub type OutboxRow = (i64, PageId, i64, Vec<u8>);

struct DocEntry {
    doc: LoroPageDoc,
    /// Version confirmed durable in `page_doc` + `page_update`. Only a committed job may
    /// advance it, never the act of submitting one (store-commit-pipeline issue 1).
    durable: utopia_doc::Frontier,
    has_snapshot: bool,
    pending_updates: i64,
    /// The last job submitted for this shard, and the frontier its update log will reach.
    /// Promoted into `durable` on completion, discarded on failure.
    in_flight: Option<InFlight>,
    /// A draft day whose first edit was queued (FR-1.2). Until that commit is durable the
    /// shard is still a draft, so a repair must keep trying to write it, while a day that
    /// was only visited must never be written.
    promoted: bool,
}

/// A submitted (not yet confirmed) commit for one page (store-commit-pipeline).
struct InFlight {
    seq: u64,
    frontier: utopia_doc::Frontier,
    /// The job writes a full snapshot: it replaces `page_doc` and clears the update log.
    snapshot: bool,
    /// The job appends one `page_update` row.
    update: bool,
    /// The queued entry wanted an outbox payload (local ops only; remote imports do not).
    enqueue: bool,
}

/// One page's share of a commit, prepared before anything is taken from `pending`.
struct PreparedPage {
    page: PageId,
    frontier: utopia_doc::Frontier,
    writes_snapshot: bool,
    writes_update: bool,
    enqueue: bool,
    job: PageJob,
}

impl DocEntry {
    fn doc_state(&self) -> PageState {
        self.doc.state().clone()
    }
}

/// A calendar day that has been looked at but never written (FR-1.2): shown in the Daily
/// Notes feed, editable in place, but not projected, synced or seen by `doctor` until its
/// first character is committed.
///
/// The shard lives inside the entry rather than in `Store::docs`, so "a draft whose shard
/// is gone" cannot be expressed: `is_draft`, `day`-lookups and `page_state` all read this
/// one value, and promotion is the only transition that moves it into `docs`.
struct DraftDay {
    date: NaiveDate,
    entry: DocEntry,
}

/// Options for recording an edit in the undo history.
#[derive(Debug, Clone, Default)]
pub struct EditOpts {
    /// Coalesce with the previous group when both are text edits of this block (FR-2.12).
    pub coalesce: Option<BlockId>,
    pub caret_before: Option<Caret>,
    pub caret_after: Option<Caret>,
    /// Skip the undo stack (automatic maintenance such as re-merges or first blocks).
    pub no_undo: bool,
}

pub struct Store {
    conn: Connection,
    path: Option<PathBuf>,
    docs: HashMap<PageId, DocEntry>,
    peer: u64,
    date_format: DateFormat,
    undo: UndoStack,
    collapsed: HashSet<BlockId>,
    clock: Option<i64>,
    today: Option<NaiveDate>,
    persister: Persister,
    /// Pages with in-memory changes not yet handed to the writer.
    pending: Vec<(PageId, Option<DocDelta>, bool)>,
    /// Daily pages shown but never written (FR-1.2), by page id: in memory only until
    /// their first edit.
    drafts: HashMap<PageId, DraftDay>,
    /// Draft ids whose promotion merged them into the page that already owned the day
    /// (FR-1.2). The ids were never durable, so this is a session alias for views and the
    /// editor that still hold one.
    promotions: HashMap<PageId, PageId>,
    /// Pages this device has put in the trash: it owns the local `trash` row for them, so
    /// every job for the page keeps that row in step with its tombstone. A page another
    /// device tombstoned is not in here and gets no row (docs/state-machines/
    /// page-lifecycle-trash.md). Seeded from the table at open, so a restart keeps it.
    own_trash: HashSet<PageId>,
    /// Replacements not yet announced to the views ([`Store::take_replacements`]): a stale
    /// draft retired for the day's page (FR-1.2) can be shown by a view the caller did not
    /// name.
    replacements: Vec<(PageId, PageId)>,
    debounced: bool,
    /// Attachment bytes for in-memory graphs (file graphs use the blob directory).
    mem_blobs: HashMap<String, Vec<u8>>,
    /// Full tag-page classifications this store's own connection has run (SB-2). The
    /// commit path publishes the set instead, so the UI never runs one; tests read this to
    /// keep it that way.
    tag_page_scans: std::sync::atomic::AtomicU64,
    /// Unlinked-reference scans this store has run (LR-5). The count is cached per index
    /// version; tests read this to check the scan happens off the paint path.
    unlinked_scans: std::sync::atomic::AtomicU64,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_key(path, None)
    }

    /// Open a graph encrypted at rest (SQLCipher, P2). `None` opens a plain graph; a
    /// missing or wrong passphrase fails with [`StoreError::Encrypted`].
    pub fn open_with_key(path: impl AsRef<Path>, key: Option<&str>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let conn = crypt::open_keyed(path, key)?;
        let mut store = Self::init(conn, Some(path.to_path_buf()))?;
        store.persister = Persister::spawn(path, key)?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None)
    }

    fn init(mut conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        schema::configure(&conn)?;
        schema::migrate(&mut conn)?;
        let meta = |key: &str| -> rusqlite::Result<Option<String>> {
            conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
                .optional()
        };
        let previous: Vec<String> = meta("date_format_previous")?
            .map(|v| v.lines().map(str::to_string).collect())
            .unwrap_or_default();
        let date_format = meta("date_format")?
            .map(|p| DateFormat::with_previous(p, previous))
            .unwrap_or_default();
        let collapsed = {
            let mut stmt = conn
                .prepare("SELECT substr(key, 11) FROM local_state WHERE key LIKE 'collapsed:%'")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(BlockId).collect()
        };
        let own_trash = {
            let mut stmt = conn.prepare("SELECT id FROM trash")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(PageId).collect()
        };
        // A fresh CRDT peer per session: copies of the file on two machines can never
        // reuse a peer id (Loro requires unique peers per writer).
        let peer = (ulid::Ulid::new().0 as u64) & ((1 << 53) - 1);
        Ok(Self {
            conn,
            path,
            docs: HashMap::new(),
            peer,
            date_format,
            undo: UndoStack::new(),
            collapsed,
            clock: None,
            today: None,
            persister: Persister::inline(),
            pending: Vec::new(),
            drafts: HashMap::new(),
            promotions: HashMap::new(),
            own_trash,
            replacements: Vec::new(),
            debounced: false,
            mem_blobs: HashMap::new(),
            tag_page_scans: std::sync::atomic::AtomicU64::new(0),
            unlinked_scans: std::sync::atomic::AtomicU64::new(0),
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The read connection (after waiting for in-flight commits in strict mode).
    pub fn conn(&self) -> &Connection {
        let _ = self.read_barrier();
        &self.conn
    }

    /// Debounced mode (desktop UI): commits wait for [`Store::flush`] and index reads
    /// never block on the writer.
    pub fn set_debounced(&mut self, on: bool) -> Result<()> {
        self.debounced = on;
        if !on {
            self.sync_index()?;
        }
        Ok(())
    }

    /// Whether in-memory changes are waiting to be committed.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// A handle that completes once everything flushed so far is durable.
    pub fn write_ticket(&self) -> WriteTicket {
        self.persister.ticket()
    }

    /// The tag-page classification as the last commit published it (SB-2). `None` until
    /// the first commit; [`Store::hashtag_only_pages`] computes and returns it directly.
    pub fn tag_pages(&self) -> Option<HashSet<PageId>> {
        self.persister.tag_pages()
    }

    /// How many whole-graph tag classifications the commit path has run (tests, SB-2). One
    /// is enough: later commits update only the pages they can affect.
    pub fn tag_page_full_scans(&self) -> u64 {
        self.persister.tag_scans()
    }

    /// How many per-page classifications the commit path has run (tests, SB-2). A commit
    /// classifies only the pages it wrote and the destinations whose ref edges changed.
    pub fn tag_page_classifications(&self) -> u64 {
        self.persister.tag_classifications()
    }

    /// How many pages the commit path's tag publisher looked at (tests, SB-2). One commit
    /// touches only the pages it wrote and the destinations whose ref edges it changed.
    pub fn tag_pages_visited(&self) -> u64 {
        self.persister.tag_pages_visited()
    }

    /// How many full tag-page classifications this store's own connection has run. The
    /// commit path classifies on the writer, so this stays put while the UI edits.
    pub fn tag_page_scans(&self) -> u64 {
        self.tag_page_scans
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many unlinked-reference scans this store has run (LR-5). Views cache the answer
    /// per index version, so this only moves when the index changed.
    pub fn unlinked_scans(&self) -> u64 {
        self.unlinked_scans
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Hand every pending change to the writer (non-blocking with a writer thread).
    ///
    /// A failed writer is a state: this refuses new jobs, keeps `pending` for the repair,
    /// and leaves every in-memory shard alone (rule 5). `sync_index` is the repair path.
    pub fn flush(&mut self) -> Result<()> {
        self.reconcile_writer();
        self.settle_drafts();
        if let Some(e) = self.persister.failure() {
            return Err(e);
        }
        if self.pending.is_empty() {
            return Ok(());
        }
        // Prepare before taking `pending`: a failing snapshot/export must not strand the
        // queue (issue 9).
        let pending = std::mem::take(&mut self.pending);
        let prepared = {
            let docs = &mut self.docs;
            let drafts = &mut self.drafts;
            let own_trash = &self.own_trash;
            let fmt = &self.date_format;
            let mut prepared = Vec::with_capacity(pending.len());
            let mut failed = None;
            for (page, delta, enqueue) in &pending {
                // A draft is queued only by the repair path, for a day whose first commit
                // failed: its shard lives in the draft entry (FR-1.2).
                let Some(entry) = docs
                    .get_mut(page)
                    .or_else(|| drafts.get_mut(page).map(|d| &mut d.entry))
                else {
                    continue;
                };
                // An earlier submitted job's update lands first, so later updates build on
                // it; a snapshot is only skipped when one is confirmed durable.
                let frontier = entry.doc.frontier();
                let base = entry
                    .in_flight
                    .as_ref()
                    .map_or(&entry.durable, |f| &f.frontier);
                let snapshot = if !entry.has_snapshot || entry.pending_updates >= COMPACT_AFTER {
                    match entry.doc.snapshot() {
                        Ok(snap) => Some((snap, frontier.0.clone())),
                        Err(e) => {
                            failed = Some(e.into());
                            break;
                        }
                    }
                } else {
                    None
                };
                let update = if frontier != *base {
                    match entry.doc.export_since(base) {
                        Ok(update) => Some(update),
                        Err(e) => {
                            failed = Some(e.into());
                            break;
                        }
                    }
                } else {
                    None
                };
                // A snapshot in the same job already covers the update.
                let log_update = if snapshot.is_none() {
                    update.clone()
                } else {
                    None
                };
                let state = entry.doc.state().clone();
                prepared.push(PreparedPage {
                    page: page.clone(),
                    frontier,
                    writes_snapshot: snapshot.is_some(),
                    writes_update: log_update.is_some(),
                    enqueue: *enqueue,
                    job: PageJob {
                        page: page.clone(),
                        trash: own_trash.contains(page).then(|| trash_op(&state, fmt)),
                        state,
                        delta: delta.clone(),
                        snapshot,
                        update: log_update,
                        outbox: if *enqueue { update } else { None },
                    },
                });
            }
            match failed {
                Some(e) => Err(e),
                None => Ok(prepared),
            }
        };
        let prepared = match prepared {
            Ok(p) => p,
            Err(e) => {
                self.pending = pending;
                return Err(e);
            }
        };
        let mut pages = Vec::with_capacity(prepared.len());
        let mut bookkeeping = Vec::with_capacity(prepared.len());
        for p in prepared {
            pages.push(p.job);
            bookkeeping.push((
                p.page,
                p.frontier,
                p.writes_snapshot,
                p.writes_update,
                p.enqueue,
            ));
        }
        if pages.is_empty() {
            return Ok(());
        }
        let job = Job {
            pages,
            fmt: self.date_format.clone(),
            now: self.now(),
        };
        let seq = match self.persister.submit(&mut self.conn, job) {
            Ok(seq) => seq,
            Err(e) => {
                // Nothing was submitted: keep the queue so the repair can ship it.
                self.pending = pending;
                return Err(e);
            }
        };
        for (page, frontier, snapshot, update, enqueue) in bookkeeping {
            // A repair retries a failed first commit from the draft entry (FR-1.2).
            let entry = self
                .docs
                .get_mut(&page)
                .or_else(|| self.drafts.get_mut(&page).map(|d| &mut d.entry));
            if let Some(entry) = entry {
                // `reconcile_writer` promoted every confirmed job at the top of this flush.
                // A job still in flight below will also append its update row before this
                // one, so count it now: overwriting `in_flight` must not lose it.
                if let Some(prev) = entry.in_flight.take() {
                    if prev.snapshot {
                        entry.pending_updates = 0;
                    } else if prev.update && !snapshot {
                        entry.pending_updates += 1;
                    }
                }
                entry.in_flight = Some(InFlight {
                    seq,
                    frontier,
                    snapshot,
                    update,
                    enqueue,
                });
            }
        }
        // Inline graphs commit inside `submit`; promote them at once.
        self.reconcile_writer();
        Ok(())
    }

    /// Promote the shards of jobs the writer confirmed, and no others. `completed` never
    /// counts a failed job, so a failed submission stays `in_flight` until it is repaired.
    fn reconcile_writer(&mut self) {
        let completed = self.persister.completed();
        // A draft's shard lives in its own entry (FR-1.2); a failed promotion puts it back
        // there, so the repair's retry must be reconciled there too.
        let drafts = self.drafts.values_mut().map(|d| &mut d.entry);
        for entry in self.docs.values_mut().chain(drafts) {
            let Some(in_flight) = entry.in_flight.take() else {
                continue;
            };
            if in_flight.seq > completed {
                entry.in_flight = Some(in_flight);
                continue;
            }
            entry.durable = in_flight.frontier;
            entry.has_snapshot = true;
            if in_flight.snapshot {
                entry.pending_updates = 0;
            } else if in_flight.update {
                entry.pending_updates += 1;
            }
        }
    }

    /// A draft whose first commit is durable is a page (FR-1.2): move its shard out of the
    /// draft entry, so the feed stops treating the day as unwritten and a later repair sees
    /// a live shard.
    fn settle_drafts(&mut self) {
        let durable: Vec<PageId> = self
            .drafts
            .iter()
            .filter(|(_, d)| {
                d.entry.promoted
                    && d.entry.has_snapshot
                    && d.entry.in_flight.is_none()
                    && d.entry.doc.frontier() == d.entry.durable
            })
            .map(|(page, _)| page.clone())
            .collect();
        for page in durable {
            self.promote_draft(&page);
            if let Some(entry) = self.docs.get_mut(&page) {
                entry.promoted = false;
            }
        }
    }

    /// Reconcile a failed writer without losing a character (rule 5): promote the jobs that
    /// did commit, then queue every shard whose durable copy is behind as a full snapshot.
    /// Returns the pages that were requeued (used to retire repaired drafts).
    fn repair_writer(&mut self) -> Vec<PageId> {
        self.reconcile_writer();
        // Days that were only visited are drafts (FR-1.2): the feed renders them from
        // memory and a repair must not write them. Only a draft whose first commit was
        // queued (`promoted`) is rewritten.
        let mut requeued: Vec<(PageId, bool)> = Vec::new();
        let mut redraft: Vec<(NaiveDate, PageId)> = Vec::new();
        for (page, entry) in self.docs.iter_mut() {
            // A submitted job that was not confirmed cannot be trusted: the writer either
            // rolled it back or dropped it. The shard is rewritten whole below.
            let was_durable = entry.has_snapshot;
            // Keep the job's outbox intent: a page queued from `import_batch` must not be
            // echoed to the relay by the repair.
            let enqueue = entry.in_flight.as_ref().is_some_and(|f| f.enqueue);
            entry.in_flight = None;
            if entry.has_snapshot && entry.doc.frontier() == entry.durable {
                continue;
            }
            if entry.promoted && !was_durable {
                if let Some(date) = entry.doc.state().meta.daily_date {
                    redraft.push((date, page.clone()));
                }
            }
            entry.has_snapshot = false;
            requeued.push((page.clone(), enqueue));
        }
        // A draft's shard lives in its own entry: a retry of its failed first commit is
        // queued from there.
        for (page, draft) in self.drafts.iter_mut() {
            let entry = &mut draft.entry;
            let enqueue = entry.in_flight.as_ref().is_some_and(|f| f.enqueue);
            entry.in_flight = None;
            if !entry.promoted || (entry.has_snapshot && entry.doc.frontier() == entry.durable) {
                continue;
            }
            entry.has_snapshot = false;
            requeued.push((page.clone(), enqueue));
        }
        // A draft whose first commit failed is still a draft (FR-1.2): the feed keeps its
        // page id, and the next edit does not mint a second page for the same day. Its shard
        // goes into the draft entry, where `flush` still finds it for the repair.
        for (date, page) in redraft {
            if self.drafts.contains_key(&page) {
                continue;
            }
            if let Some(entry) = self.docs.remove(&page) {
                self.drafts.insert(page, DraftDay { date, entry });
            }
        }
        for (page, enqueue) in &requeued {
            match self.pending.iter_mut().find(|(p, _, _)| p == page) {
                Some((_, delta, _)) => {
                    // Keep the queued entry's own flag: it is newer than the in-flight job.
                    *delta = None;
                }
                None => self.pending.push((page.clone(), None, *enqueue)),
            }
        }
        self.persister.clear_failure();
        requeued.into_iter().map(|(page, _)| page).collect()
    }

    /// Wait until the index reflects every change (flush + wait). Required before
    /// operations that read the index to decide what to write (create, rename, merge).
    ///
    /// A writer failure is repaired here by rewriting the affected shards as snapshots,
    /// and still reported to this caller: the commit it asked for did not land.
    pub fn sync_index(&mut self) -> Result<()> {
        let flushed = self.flush();
        let waited = self.persister.wait();
        self.reconcile_writer();
        self.settle_drafts();
        if let Some(failure) = self.persister.failure() {
            self.repair_writer();
            let retried = self.flush().and_then(|()| self.persister.wait());
            self.reconcile_writer();
            // A retried draft whose commit is durable now is a page (FR-1.2).
            self.settle_drafts();
            return match retried {
                Ok(()) => Err(failure),
                Err(e) => Err(e),
            };
        }
        flushed.and(waited)
    }

    /// Strict mode waits for in-flight commits before an index read.
    pub(crate) fn read_barrier(&self) -> Result<()> {
        if self.debounced {
            Ok(())
        } else {
            self.persister.wait()
        }
    }

    /// Queue shard commits; strict mode ships them immediately.
    fn queue(&mut self, pages: &[(PageId, Option<DocDelta>)], enqueue: bool) -> Result<()> {
        let mut promoted = false;
        // Pages a promoted draft had to be merged into (§8.6); their shards changed too.
        let mut merged_into: Vec<PageId> = Vec::new();
        for (page, delta) in pages {
            // A draft's first edit writes the whole page (meta, first block, the edit).
            let delta = match self.promote_draft(page) {
                Some(date) => {
                    promoted = true;
                    // The first commit of a promoted draft is still in flight: until it is
                    // durable the day is still a draft for a repair (store-commit-pipeline).
                    if let Some(entry) = self.docs.get_mut(page) {
                        entry.promoted = true;
                    }
                    // One page per date (rule 4): if a remote copy of the day already
                    // owns it under another id, the draft's blocks move into that page
                    // and its own shard is dropped (nothing to write).
                    if let Some(live) = self.resolve_promotion(page, date)? {
                        merged_into.push(live);
                        continue;
                    }
                    None
                }
                None => delta.clone(),
            };
            self.push_pending(page, delta, enqueue);
        }
        for page in merged_into {
            self.push_pending(&page, None, enqueue);
        }
        if self.debounced && !promoted {
            Ok(())
        } else {
            self.flush()
        }
    }

    /// Append a page to the pending commit, merging its delta with an earlier one.
    fn push_pending(&mut self, page: &PageId, delta: Option<DocDelta>, enqueue: bool) {
        match self.pending.iter_mut().find(|(p, _, _)| p == page) {
            Some((_, d, e)) => {
                *d = match (d.take(), delta) {
                    (Some(mut a), Some(b)) => {
                        a.merge(b);
                        Some(a)
                    }
                    _ => None,
                };
                *e |= enqueue;
            }
            None => self.pending.push((page.clone(), delta, enqueue)),
        }
    }

    /// Fix the clock (tests). `None` restores the wall clock.
    pub fn set_clock(&mut self, now_ms: Option<i64>) {
        self.clock = now_ms;
    }

    /// Fix "today" (tests, AC-1.2). `None` follows the machine's local date (FR-1.7).
    pub fn set_today(&mut self, today: Option<NaiveDate>) {
        self.today = today;
    }

    pub fn now(&self) -> i64 {
        self.clock.unwrap_or_else(utopia_core::now_ms)
    }

    /// Today's calendar date in the machine's current local time zone (§5.5).
    pub fn today(&self) -> NaiveDate {
        self.today
            .unwrap_or_else(|| chrono::Local::now().date_naive())
    }

    pub fn date_format(&self) -> &DateFormat {
        &self.date_format
    }

    /// Change the date rendering (AC-1.8): titles re-render, identities are untouched,
    /// and links typed in earlier formats keep resolving.
    pub fn set_date_format(&mut self, pattern: &str) -> Result<()> {
        self.sync_index()?;
        let fmt = self.date_format.changed_to(pattern);
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (key, value) in [
            ("date_format", pattern.to_string()),
            ("date_format_previous", fmt.previous().join("\n")),
        ] {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )?;
        }
        let dailies: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT id, daily_date FROM page WHERE kind='daily'")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for (id, date) in dailies {
            if let Some(d) = utopia_core::date::parse_iso(&date) {
                let title = fmt.format(d);
                tx.execute(
                    "UPDATE page SET title=?1, title_key=?2 WHERE id=?3",
                    params![title, utopia_core::title::normalize_title(&title), id],
                )?;
            }
        }
        // Re-resolve every reference: forms that parse as dates may have changed.
        let keys: HashSet<String> = {
            let mut stmt =
                tx.prepare("SELECT DISTINCT dst_title_key FROM ref WHERE dst_kind='page'")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            rows
        };
        let projector = project::Projector {
            conn: &tx,
            fmt: &fmt,
            now: self.clock.unwrap_or_else(utopia_core::now_ms),
        };
        projector.reresolve(&mut project::ResolutionWork {
            keys,
            pages: HashSet::new(),
            changed: HashSet::new(),
        })?;
        tx.commit()?;
        self.date_format = fmt;
        Ok(())
    }

    // ---------------------------------------------------------------- shards

    pub fn page_exists(&self, page: &PageId) -> Result<bool> {
        // Deliberately not redirected: an incoming shard for a draft's old id (FR-1.2) is a
        // different page and must be imported under its own id, not folded into the day.
        if self.docs.contains_key(page) || self.drafts.contains_key(page) {
            return Ok(true);
        }
        self.read_barrier()?;
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM page_doc WHERE page_id=?1)",
            [page.as_str()],
            |r| r.get(0),
        )? || self.docs.contains_key(page))
    }

    fn load_doc(&mut self, page: &PageId) -> Result<&mut DocEntry> {
        // A promoted draft that merged into the day's page (FR-1.2) leaves its id behind as
        // a session alias: reads follow the page that owns the day, so a view or an editor
        // that still holds the old id keeps working.
        let page = &self.redirect(page).unwrap_or_else(|| page.clone());
        // A draft day has no `page_doc` row yet: its shard is inside the draft entry, and
        // every reader must find it there rather than look for a row (FR-1.2).
        if self.drafts.contains_key(page) {
            return Ok(&mut self.drafts.get_mut(page).expect("checked above").entry);
        }
        if !self.docs.contains_key(page) {
            let snap: Option<Vec<u8>> = self
                .conn
                .query_row(
                    "SELECT snapshot FROM page_doc WHERE page_id=?1",
                    [page.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(snap) = snap else {
                return Err(StoreError::NotFound(format!("page {page}")));
            };
            let updates: Vec<Vec<u8>> = {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT payload FROM page_update WHERE page_id=?1 ORDER BY seq",
                )?;
                let rows = stmt
                    .query_map([page.as_str()], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let doc = LoroPageDoc::load(page.clone(), &snap, &updates, self.peer)?;
            let durable = doc.frontier();
            self.docs.insert(
                page.clone(),
                DocEntry {
                    doc,
                    durable,
                    has_snapshot: true,
                    pending_updates: updates.len() as i64,
                    in_flight: None,
                    promoted: false,
                },
            );
        }
        Ok(self.docs.get_mut(page).expect("inserted above"))
    }

    fn new_doc(&mut self, page: &PageId) -> &mut DocEntry {
        if !self.docs.contains_key(page) {
            let entry = self.fresh_entry(page);
            self.docs.insert(page.clone(), entry);
        }
        self.docs.get_mut(page).expect("inserted above")
    }

    /// A new, empty shard for `page` that has never been written.
    fn fresh_entry(&self, page: &PageId) -> DocEntry {
        DocEntry {
            doc: LoroPageDoc::new(page.clone(), self.peer),
            durable: utopia_doc::Frontier::default(),
            has_snapshot: false,
            pending_updates: 0,
            in_flight: None,
            promoted: false,
        }
    }

    /// Anchor a caret in a block so it survives remote merges (§6.5).
    pub fn caret_cursor(
        &mut self,
        page: &PageId,
        block: &BlockId,
        offset: usize,
    ) -> Option<DocCursor> {
        self.load_doc(page).ok()?.doc.cursor_for(block, offset)
    }

    /// Where an anchored caret is now (byte offset in the block's text).
    pub fn resolve_caret(&mut self, page: &PageId, cursor: &DocCursor) -> Option<usize> {
        self.load_doc(page).ok()?.doc.resolve_cursor(cursor)
    }

    /// The current tree of a page (loads its shard on first use).
    pub fn page_state(&mut self, page: &PageId) -> Result<&PageState> {
        Ok(self.load_doc(page)?.doc.state())
    }

    /// Apply commands to a shard in memory, returning the merged delta and the inverse
    /// commands (in application order).
    fn apply_cmds(
        &mut self,
        page: &PageId,
        cmds: &[Command],
        now: i64,
        want_inverse: bool,
    ) -> Result<(DocDelta, Vec<Command>)> {
        let entry = self.load_doc(page)?;
        let mut delta = DocDelta::default();
        let mut inverse: Vec<Vec<Command>> = Vec::new();
        for cmd in cmds {
            let inv = if want_inverse {
                Some(cmd.invert(entry.doc.state())?)
            } else {
                None
            };
            delta.merge(entry.doc.apply(cmd, now)?);
            if let Some(inv) = inv {
                inverse.push(inv);
            }
        }
        inverse.reverse();
        Ok((delta, inverse.into_iter().flatten().collect()))
    }

    /// Rolling snapshot (§15.3): `VACUUM INTO` a timestamped copy in `dir`, keeping the
    /// newest `keep` generations. Returns the new file.
    pub fn backup(&mut self, dir: &Path, keep: usize) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        self.sync_index()?;
        self.compact()?;
        let name = format!("graph-{}.db", self.now());
        let target = dir.join(&name);
        self.conn
            .execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
        let mut existing: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("graph-") && n.ends_with(".db"))
            })
            .collect();
        existing.sort();
        while existing.len() > keep {
            std::fs::remove_file(existing.remove(0))?;
        }
        Ok(target)
    }

    /// Fold every pending update into its snapshot (on close; §15.3).
    pub fn compact(&mut self) -> Result<()> {
        self.sync_index()?;
        let pages: Vec<PageId> = self
            .docs
            .iter()
            .filter(|(_, e)| e.pending_updates > 0)
            .map(|(p, _)| p.clone())
            .collect();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for p in &pages {
            let e = &self.docs[p];
            let snap = e.doc.snapshot()?;
            tx.execute(
                "UPDATE page_doc SET snapshot=?1, frontier=?2, bytes=?3 WHERE page_id=?4",
                params![snap, e.doc.frontier().0, snap.len() as i64, p.as_str()],
            )?;
            tx.execute("DELETE FROM page_update WHERE page_id=?1", [p.as_str()])?;
        }
        tx.commit()?;
        for p in pages {
            if let Some(e) = self.docs.get_mut(&p) {
                e.pending_updates = 0;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- editing

    /// Drop an in-memory shard a failed command group may have left half-applied. A live
    /// page reloads from its last durable state. A draft has none, so the day goes back to
    /// unwritten: its shard and its draft entry are one value and go together (FR-1.2),
    /// rather than leaving a draft `page_state` cannot find.
    fn drop_shard(&mut self, page: &PageId) {
        self.docs.remove(page);
        self.drafts.remove(page);
    }

    /// Apply commands to one page as a single undo group.
    pub fn execute(
        &mut self,
        page: &PageId,
        cmds: Vec<Command>,
        opts: EditOpts,
    ) -> Result<DocDelta> {
        let mut deltas = self.execute_multi(vec![(page.clone(), cmds)], opts)?;
        Ok(deltas.pop().map(|(_, d)| d).unwrap_or_default())
    }

    /// Apply commands to several pages as one transaction and one undo group (merge,
    /// reference rewrites).
    pub fn execute_multi(
        &mut self,
        steps: Vec<(PageId, Vec<Command>)>,
        opts: EditOpts,
    ) -> Result<Vec<(PageId, DocDelta)>> {
        // A promoted draft that merged into the day (FR-1.2) is an alias: writes to its id
        // apply to the page that owns the day, so an editor holding the old id keeps
        // typing into the same block instead of a shard that no longer exists (rule 5).
        let steps: Vec<(PageId, Vec<Command>)> = steps
            .into_iter()
            .map(|(page, cmds)| (self.redirect(&page).unwrap_or(page), cmds))
            .collect();
        let now = self.now();
        let mut group = UndoGroup::new(now);
        group.coalesce = opts.coalesce.clone();
        group.caret_before = opts.caret_before.clone();
        group.caret_after = opts.caret_after.clone();
        let mut deltas: Vec<(PageId, DocDelta)> = Vec::new();
        for (page, cmds) in steps {
            if cmds.is_empty() {
                continue;
            }
            let (delta, inverse) = match self.apply_cmds(&page, &cmds, now, !opts.no_undo) {
                Ok(v) => v,
                Err(e) => {
                    // Roll back what this group already applied in memory.
                    for (p, _) in &deltas {
                        self.drop_shard(p);
                    }
                    self.drop_shard(&page);
                    return Err(e);
                }
            };
            group.steps.push(UndoStep {
                page: page.clone(),
                forward: cmds,
                inverse,
            });
            match deltas.iter_mut().find(|(p, _)| *p == page) {
                Some((_, d)) => d.merge(delta),
                None => deltas.push((page, delta)),
            }
        }
        let pages: Vec<(PageId, Option<DocDelta>)> = deltas
            .iter()
            .map(|(p, d)| (p.clone(), Some(d.clone())))
            .collect();
        self.queue(&pages, true)?;
        // Only plain text edits are debounced; structure and identity reach the writer
        // at once so resolution and autocomplete see them.
        if deltas.iter().any(|(_, d)| d.structural || d.meta_changed) {
            self.flush()?;
        }
        // A draft that promotion merged into the day (see `resolve_promotion`) has this
        // group's commands in the survivor's shard now. Point the undo step *and the two
        // saved carets* there too: undo/redo restore the caret into the page it names, and a
        // discarded id would put the editor on a row the view does not draw (rule 2, FR-1.2).
        for step in &mut group.steps {
            if let Some(to) = self.redirect(&step.page) {
                step.page = to;
            }
        }
        for caret in [&mut group.caret_before, &mut group.caret_after] {
            let Some(caret) = caret else { continue };
            if let Some(to) = self.redirect(&caret.page) {
                caret.page = to;
            }
        }
        if !opts.no_undo {
            self.undo.push(group);
        }
        Ok(deltas)
    }

    /// Execute an outliner plan on `page`; returns where the caret should go.
    pub fn execute_plan(
        &mut self,
        page: &PageId,
        plan: Plan,
        caret_before: Option<Caret>,
    ) -> Result<Option<utopia_core::outline::Focus>> {
        let caret_after = plan.focus.as_ref().map(|f| Caret {
            page: page.clone(),
            block: f.block.clone(),
            offset: f.offset,
        });
        self.undo.seal();
        self.execute(
            page,
            plan.cmds,
            EditOpts {
                caret_before,
                caret_after,
                ..Default::default()
            },
        )?;
        self.undo.seal();
        Ok(plan.focus)
    }

    /// Set a block's full text (the editor's keystroke path). Coalesces with adjacent
    /// edits of the same block.
    pub fn set_block_text(
        &mut self,
        page: &PageId,
        block: &BlockId,
        text: &str,
        caret_before: Option<usize>,
        caret_after: Option<usize>,
    ) -> Result<bool> {
        let st = self.page_state(page)?;
        let Some(edit) = st
            .block(block)
            .and_then(|b| utopia_core::text::diff(&b.text, text))
        else {
            return Ok(false);
        };
        let caret = |o: Option<usize>| {
            o.map(|offset| Caret {
                page: page.clone(),
                block: block.clone(),
                offset,
            })
        };
        self.execute(
            page,
            vec![Command::SetBlockText {
                id: block.clone(),
                edit,
            }],
            EditOpts {
                coalesce: Some(block.clone()),
                caret_before: caret(caret_before),
                caret_after: caret(caret_after),
                no_undo: false,
            },
        )?;
        Ok(true)
    }

    /// Close the current undo group so the next edit starts a new one.
    pub fn seal_undo(&mut self) {
        self.undo.seal();
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Cmd+Z. Returns the caret to restore.
    pub fn undo(&mut self) -> Result<Option<Caret>> {
        let Some(group) = self.undo.pop_undo() else {
            return Ok(None);
        };
        let now = self.now();
        let mut touched: Vec<(PageId, Option<DocDelta>)> = Vec::new();
        for step in group.steps.iter().rev() {
            let entry = self.load_doc(&step.page)?;
            let mut delta = DocDelta::default();
            for cmd in &step.inverse {
                // A remote edit can make an inverse inapplicable; skip it rather than
                // abort the whole undo (never lose a character, §2.3).
                if let Ok(d) = entry.doc.apply(cmd, now) {
                    delta.merge(d);
                }
            }
            push_delta(&mut touched, &step.page, delta);
        }
        self.queue(&touched, true)?;
        let caret = group.caret_before.clone();
        self.undo.push_redo(group);
        Ok(caret)
    }

    /// Cmd+Shift+Z.
    pub fn redo(&mut self) -> Result<Option<Caret>> {
        let Some(group) = self.undo.pop_redo() else {
            return Ok(None);
        };
        let now = self.now();
        let mut touched: Vec<(PageId, Option<DocDelta>)> = Vec::new();
        for step in &group.steps {
            let entry = self.load_doc(&step.page)?;
            let mut delta = DocDelta::default();
            for cmd in &step.forward {
                if let Ok(d) = entry.doc.apply(cmd, now) {
                    delta.merge(d);
                }
            }
            push_delta(&mut touched, &step.page, delta);
        }
        self.queue(&touched, true)?;
        let caret = group.caret_after.clone();
        self.undo.push_undone(group);
        Ok(caret)
    }

    // ---------------------------------------------------------------- local state

    pub fn is_collapsed(&self, block: &BlockId) -> bool {
        self.collapsed.contains(block)
    }

    pub fn collapsed_set(&self) -> &HashSet<BlockId> {
        &self.collapsed
    }

    /// Collapse state is local to this device and never enters a shard (FR-2.8, §12.5).
    pub fn set_collapsed(&mut self, block: &BlockId, collapsed: bool) -> Result<()> {
        let key = format!("collapsed:{block}");
        if collapsed {
            self.conn.execute(
                "INSERT OR REPLACE INTO local_state(key, value) VALUES (?1, '1')",
                [&key],
            )?;
            self.collapsed.insert(block.clone());
        } else {
            self.conn
                .execute("DELETE FROM local_state WHERE key=?1", [&key])?;
            self.collapsed.remove(block);
        }
        Ok(())
    }

    pub fn local_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM local_state WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn local_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO local_state(key, value) VALUES (?1, ?2)",
            [key, value],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- sync surface

    /// Updates for `page` since `frontier` (empty frontier = everything).
    pub fn export_since(
        &mut self,
        page: &PageId,
        frontier: &utopia_doc::Frontier,
    ) -> Result<Vec<u8>> {
        Ok(self.load_doc(page)?.doc.export_since(frontier)?)
    }

    pub fn frontier(&mut self, page: &PageId) -> Result<utopia_doc::Frontier> {
        Ok(self.load_doc(page)?.doc.frontier())
    }

    /// Import remote updates for one page; see [`Store::import_batch`].
    pub fn import(&mut self, page: &PageId, bytes: &[u8]) -> Result<bool> {
        Ok(!self
            .import_batch(&[(page.clone(), bytes.to_vec())])?
            .is_empty())
    }

    /// Pending outbox rows: `(seq, page, client_seq, payload)`.
    pub fn outbox_pending(&self, limit: usize) -> Result<Vec<OutboxRow>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare(
            "SELECT seq, page_id, client_seq, payload FROM outbox WHERE sent_at IS NULL
             ORDER BY seq LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit as i64], |r| {
                Ok((r.get(0)?, PageId(r.get(1)?), r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// How many changes are waiting for the relay, without loading their payloads (the
    /// sidebar shows this count on the UI thread, §9.2, rule 10).
    pub fn outbox_count(&self) -> Result<usize> {
        self.read_barrier()?;
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM outbox WHERE sent_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn outbox_mark_sent(&self, seqs: &[i64]) -> Result<()> {
        let now = self.now();
        for s in seqs {
            self.conn
                .execute("UPDATE outbox SET sent_at=?1 WHERE seq=?2", params![now, s])?;
        }
        Ok(())
    }

    pub fn sync_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM sync_state WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// `sync_state` entries whose key starts with `prefix:` as `(suffix, value)`. Cursors
    /// live under `cursor:<page>`; the value is opaque here (utopia-sync owns its shape).
    pub fn sync_prefixed(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let like = format!("{prefix}:%");
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM sync_state WHERE key LIKE ?1")?;
        let rows = stmt.query_map([&like], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (key, value) = row?;
            if let Some(suffix) = key.strip_prefix(&format!("{prefix}:")) {
                out.push((suffix.to_string(), value));
            }
        }
        Ok(out)
    }

    /// Delete every `sync_state` entry whose key starts with `prefix:`; returns how many.
    pub fn sync_delete_prefixed(&self, prefix: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM sync_state WHERE key LIKE ?1",
            [format!("{prefix}:%")],
        )?)
    }

    pub fn sync_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sync_state(key, value) VALUES (?1, ?2)",
            [key, value],
        )?;
        Ok(())
    }

    /// Every page with a shard (live or tombstoned).
    pub fn all_page_ids(&self) -> Result<Vec<PageId>> {
        self.read_barrier()?;
        let mut stmt = self
            .conn
            .prepare("SELECT page_id FROM page_doc ORDER BY page_id")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.into_iter().map(PageId).collect())
    }
}

fn push_delta(touched: &mut Vec<(PageId, Option<DocDelta>)>, page: &PageId, delta: DocDelta) {
    match touched.iter_mut().find(|(p, _)| p == page) {
        Some((_, Some(d))) => d.merge(delta),
        Some((_, d)) => *d = Some(delta),
        None => touched.push((page.clone(), Some(delta))),
    }
}

/// The local trash row a job for this page must carry, when this device owns one for it: the
/// row follows the shard's fate (§6.5), so a live or merged page loses it and a trashed page
/// (re)creates it in the same transaction as its tombstone.
fn trash_op(st: &PageState, fmt: &DateFormat) -> TrashOp {
    match PageFate::of(st) {
        PageFate::Trashed { at } => TrashOp::Put {
            title: display_title(st, fmt),
            at,
        },
        _ => TrashOp::Clear,
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.sync_index();
        self.persister.stop();
        self.persister = Persister::inline();
        let _ = self.compact();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    /// Review P3: with a slow writer two jobs for one page can be in flight at once. Each
    /// appends a `page_update` row, but the second `flush` replaces `in_flight`; the count
    /// that drives `COMPACT_AFTER` must keep both or compaction drifts.
    #[test]
    fn overlapping_jobs_keep_the_update_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(dir.path().join("g.utopia")).unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        s.set_today(Some(day));
        let (page, block) = s.open_daily(day).unwrap();
        s.set_block_text(&page, &block, "a", None, None).unwrap();
        s.sync_index().unwrap();
        s.set_debounced(true).unwrap();
        let before = s.docs[&page].pending_updates;

        // Hold the write lock so both flushes queue while the first job is blocked.
        s.conn().execute_batch("BEGIN IMMEDIATE").unwrap();
        s.set_block_text(&page, &block, "ab", None, None).unwrap();
        s.flush().unwrap();
        s.set_block_text(&page, &block, "abc", None, None).unwrap();
        s.flush().unwrap();
        s.conn().execute_batch("ROLLBACK").unwrap();
        s.sync_index().unwrap();

        let rows: i64 = s
            .conn()
            .query_row(
                "SELECT count(*) FROM page_update WHERE page_id=?1",
                [page.as_str()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            s.docs[&page].pending_updates, rows,
            "the compaction count matches the rows that landed"
        );
        assert_eq!(rows, before + 2, "both jobs appended an update row");
    }
}
