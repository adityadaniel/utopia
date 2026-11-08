//! The SQLite writer (PRD §13.3): commits of shard deltas and projections run on a
//! dedicated thread with its own connection, so keystrokes never wait on disk. Jobs are
//! applied strictly in submission order; each one is a single transaction.
//!
//! In-memory graphs cannot share a database between connections, so they write inline
//! on the caller's thread with the same code path.

use std::collections::HashSet;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use rusqlite::{params, Connection, TransactionBehavior};
use utopia_core::{DateFormat, DocDelta, PageId, PageState};

use crate::project::{Projector, ResolutionWork};
use crate::{schema, Result, StoreError};

/// A change to the local `trash` table that must land in the same transaction as the shard
/// change it belongs to (docs/state-machines/page-lifecycle-trash.md issues 1 and 2).
///
/// The row is local bookkeeping, not a projection: a page tombstoned by another device gets
/// no row here, because only a page this device deleted is in this device's trash.
pub(crate) enum TrashOp {
    /// The page is trashed: the row that lists it (and lets it be restored, AC-1.5).
    Put { title: String, at: i64 },
    /// The page is live or merged: it owns no row.
    Clear,
}

/// One page's share of a commit.
pub(crate) struct PageJob {
    pub page: PageId,
    pub state: PageState,
    pub delta: Option<DocDelta>,
    /// Full snapshot (and its frontier) replacing `page_doc` and the update log.
    pub snapshot: Option<(Vec<u8>, String)>,
    /// Incremental update appended to `page_update`.
    pub update: Option<Vec<u8>>,
    /// Payload queued for sync.
    pub outbox: Option<Vec<u8>>,
    /// The local trash row for this page, when this device owns one for it.
    pub trash: Option<TrashOp>,
}

pub(crate) struct Job {
    pub pages: Vec<PageJob>,
    pub fmt: DateFormat,
    pub now: i64,
}

/// Write one job in one transaction (§11.2: shard, outbox and projections together).
///
/// Returns the destination pages whose reference edges this job added or removed. Only
/// those, plus the pages the job wrote, can change tag-page classification; a destination
/// whose edges are untouched (a widely used `#todo` on an edited page) needs no rescan
/// (SB-2).
pub(crate) fn write_job(conn: &mut Connection, job: &Job) -> Result<HashSet<PageId>> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut work = ResolutionWork::default();
    {
        let projector = Projector {
            conn: &tx,
            fmt: &job.fmt,
            now: job.now,
        };
        // Pages that become tombstoned project first so unique title/date indexes never
        // see two live rows mid-transaction (§8.6, import-time dedupe).
        let mut order: Vec<&PageJob> = job.pages.iter().collect();
        order.sort_by_key(|p| p.state.meta.deleted_at.is_none());
        let mut flips = Vec::with_capacity(order.len());
        for p in &order {
            flips.push(projector.project_page(&p.state, &mut work)?);
        }
        // Block rows of live pages first: blocks merged into a survivor are re-homed in
        // place before the tombstoned page cleans up what is left (§8.6).
        let mut blocks: Vec<(&PageJob, bool)> = order.iter().copied().zip(flips).collect();
        blocks.sort_by_key(|(p, _)| p.state.meta.deleted_at.is_some());
        for (p, flip) in &blocks {
            projector.project_blocks(&p.state, p.delta.as_ref(), *flip, &mut work)?;
        }
        for p in order {
            // The local trash row is written here, inside the tombstone's transaction, so the
            // two can never disagree: no undo, failed write or merge may leave a live page
            // with a row, or a tombstoned page with none (AC-1.5).
            match &p.trash {
                Some(TrashOp::Put { title, at }) => {
                    tx.execute(
                        "INSERT OR REPLACE INTO trash(id, kind, payload, deleted_at)
                         VALUES (?1, 'page', ?2, ?3)",
                        params![
                            p.page.as_str(),
                            serde_json::json!({ "title": title })
                                .to_string()
                                .into_bytes(),
                            at
                        ],
                    )?;
                }
                Some(TrashOp::Clear) => {
                    tx.execute("DELETE FROM trash WHERE id=?1", [p.page.as_str()])?;
                }
                None => {}
            }
            if let Some((snap, frontier)) = &p.snapshot {
                tx.execute(
                    "INSERT INTO page_doc(page_id, snapshot, frontier, bytes) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(page_id) DO UPDATE SET snapshot=excluded.snapshot,
                       frontier=excluded.frontier, bytes=excluded.bytes",
                    params![p.page.as_str(), snap, frontier, snap.len() as i64],
                )?;
                tx.execute(
                    "DELETE FROM page_update WHERE page_id=?1",
                    [p.page.as_str()],
                )?;
            }
            if let Some(update) = &p.update {
                tx.execute(
                    "INSERT INTO page_update(page_id, payload) VALUES (?1, ?2)",
                    params![p.page.as_str(), update],
                )?;
            }
            if let Some(payload) = &p.outbox {
                // max(seq) reads the rowid b-tree's last entry: O(1).
                let client_seq: i64 =
                    tx.query_row("SELECT ifnull(max(seq), 0) + 1 FROM outbox", [], |r| {
                        r.get(0)
                    })?;
                tx.execute(
                    "INSERT INTO outbox(page_id, payload, client_seq, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![p.page.as_str(), payload, client_seq, job.now],
                )?;
            }
        }
        projector.reresolve(&mut work)?;
    }
    tx.commit()?;
    Ok(work.changed)
}

/// The writer's health, explicit (store-commit-pipeline): a failed commit is a state the
/// store stays in until `Store::sync_index` has reconciled the shards and resubmitted
/// them. It is not a one-shot string that the next reader may take.
enum WriterState {
    Healthy,
    Failed { error: String },
}

struct Progress {
    /// Highest job sequence whose transaction committed. Never advances on failure, so a
    /// ticket that asks for an uncommitted job cannot mistake it for durable.
    completed: u64,
    /// Highest job sequence the writer has accounted for (committed, failed or dropped).
    /// Tickets wait on this: a job dropped after a failure must not leave a waiter hanging.
    processed: u64,
    /// Jobs at or below this sequence were submitted before a repair and are never written
    /// (their base may be the job that failed).
    stale_through: u64,
    state: WriterState,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            completed: 0,
            processed: 0,
            stale_through: 0,
            state: WriterState::Healthy,
        }
    }
}

#[derive(Default)]
pub(crate) struct Shared {
    progress: Mutex<Progress>,
    cv: Condvar,
    /// The tag-page classification as of the last committed job (SB-2). Published here,
    /// off the UI thread, so an index update never runs the whole-graph query.
    tag_pages: Mutex<Option<HashSet<PageId>>>,
    /// How many whole-graph classifications this writer has run. Only the first commit
    /// needs one; later commits update the pages they can affect (SB-2).
    tag_scans: std::sync::atomic::AtomicU64,
    /// How many per-page classifications the incremental path ran (tests, SB-2).
    tag_classifications: std::sync::atomic::AtomicU64,
    /// How many pages the incremental tag publisher looked at — classified or probed
    /// (tests, SB-2). A commit must never walk the published set, so this stays as small as
    /// the pages it wrote plus the destinations it changed.
    tag_pages_visited: std::sync::atomic::AtomicU64,
}

impl Shared {
    fn failure(&self) -> Option<String> {
        let p = self.progress.lock().ok()?;
        match &p.state {
            WriterState::Healthy => None,
            WriterState::Failed { error } => Some(error.clone()),
        }
    }

    fn completed(&self) -> u64 {
        self.progress.lock().map_or(0, |p| p.completed)
    }

    fn fail(&self, error: String) {
        if let Ok(mut p) = self.progress.lock() {
            p.state = WriterState::Failed { error };
        }
        self.cv.notify_all();
    }

    fn commit(&self, seq: u64) {
        if let Ok(mut p) = self.progress.lock() {
            p.completed = seq;
            p.processed = p.processed.max(seq);
        }
        self.cv.notify_all();
    }

    /// Decide one dequeued job: `true` means drop it unwritten. Jobs submitted before the
    /// last repair are stale by sequence; so is everything while the writer is failed.
    /// Either way the job counts as processed, so nobody waits for it forever.
    fn skip(&self, seq: u64) -> bool {
        let mut skipped = true;
        if let Ok(mut p) = self.progress.lock() {
            skipped = p.stale_through >= seq || matches!(p.state, WriterState::Failed { .. });
            if skipped {
                p.processed = p.processed.max(seq);
            }
        }
        if skipped {
            self.cv.notify_all();
        }
        skipped
    }

    /// Mark the writer healthy again after `Store` requeued the shards as snapshots.
    /// Everything submitted up to `through` is stale and must not be written: its base may
    /// be the job that failed. `processed` jumps to `through` so a repair that has nothing
    /// to requeue cannot leave a ticket waiting on a job that will never run.
    fn clear_failure(&self, through: u64) {
        if let Ok(mut p) = self.progress.lock() {
            p.state = WriterState::Healthy;
            p.stale_through = p.stale_through.max(through);
            p.processed = p.processed.max(through);
        }
        self.cv.notify_all();
    }

    /// Update the published tag-page classification after a commit (SB-2). Only the pages
    /// the commit wrote and the destinations whose reference edges it changed can flip:
    /// losing the last reference to a page removes that edge, so it is one of
    /// `changed_refs`. Everything else keeps its entry, so no commit walks the whole graph.
    /// The first commit (before there is a set) classifies once. A failure leaves the
    /// previous set in place.
    fn publish_tag_pages(&self, conn: &Connection, job: &Job, changed_refs: &HashSet<PageId>) {
        let mut slot = match self.tag_pages.lock() {
            Ok(slot) => slot,
            Err(_) => return,
        };
        let Some(mut set) = slot.clone() else {
            self.tag_scans
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match crate::query::hashtag_only_pages(conn) {
                Ok(pages) => *slot = Some(pages),
                Err(e) => eprintln!("utopia: tag pages: {e}"),
            }
            return;
        };
        let written: Vec<PageId> = job.pages.iter().map(|p| p.page.clone()).collect();
        let mut affected: HashSet<PageId> = written.iter().cloned().collect();
        // Only destinations whose reference edges changed can flip classification. The
        // pages a job wrote are included above; unchanged refs on them are not rescanned.
        affected.extend(changed_refs.iter().cloned());
        for page in affected {
            self.tag_pages_visited
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.tag_classifications
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            match crate::query::classify_tag_page(conn, &page) {
                Ok(Some(true)) => {
                    set.insert(page);
                }
                Ok(_) => {
                    set.remove(&page);
                }
                Err(e) => {
                    eprintln!("utopia: tag pages: {e}");
                    return;
                }
            }
        }
        *slot = Some(set);
    }
}

pub(crate) enum Msg {
    Job(u64, Job),
    Stop,
}

/// Lets a caller (e.g. a UI background task) wait until everything submitted so far is
/// durable, without holding the store.
#[derive(Clone)]
pub struct WriteTicket {
    shared: Option<Arc<Shared>>,
    target: u64,
}

impl WriteTicket {
    /// Whether there is nothing left to wait for: the job committed or the writer failed.
    pub fn is_done(&self) -> bool {
        self.shared.as_ref().is_none_or(|s| {
            s.progress.lock().map_or(true, |p| {
                p.completed >= self.target || matches!(p.state, WriterState::Failed { .. })
            })
        })
    }

    /// Block until the writer has committed every job up to this ticket, or report the
    /// failure that stopped it. The failure is *not* consumed: the store stays failed
    /// until it has been repaired (store-commit-pipeline issue 1).
    pub fn wait(&self) -> Result<()> {
        let Some(s) = &self.shared else { return Ok(()) };
        let mut p = s
            .progress
            .lock()
            .map_err(|_| StoreError::Writer("writer lock poisoned".into()))?;
        loop {
            if let WriterState::Failed { error } = &p.state {
                return Err(StoreError::Writer(error.clone()));
            }
            if p.processed >= self.target {
                return Ok(());
            }
            p =
                s.cv.wait(p)
                    .map_err(|_| StoreError::Writer("writer lock poisoned".into()))?;
        }
    }
}

pub(crate) enum Persister {
    Inline {
        shared: Arc<Shared>,
        submitted: u64,
    },
    Thread {
        tx: Sender<Msg>,
        shared: Arc<Shared>,
        handle: Option<JoinHandle<()>>,
        submitted: u64,
    },
}

impl Persister {
    fn shared(&self) -> &Arc<Shared> {
        match self {
            Persister::Inline { shared, .. } | Persister::Thread { shared, .. } => shared,
        }
    }

    /// Writes on the caller's thread (in-memory graphs, tests, the CLI).
    pub fn inline() -> Self {
        Persister::Inline {
            shared: Arc::new(Shared::default()),
            submitted: 0,
        }
    }

    /// Spawn a writer thread with its own connection to `path`.
    pub fn spawn(path: &std::path::Path, key: Option<&str>) -> Result<Self> {
        let mut conn = crate::crypt::open_keyed(path, key)?;
        schema::configure(&conn)?;
        schema::migrate(&mut conn)?;
        let (tx, rx) = channel::<Msg>();
        let shared = Arc::new(Shared::default());
        let s2 = shared.clone();
        let handle = std::thread::Builder::new()
            .name("utopia-sqlite-writer".into())
            .spawn(move || {
                for msg in rx {
                    match msg {
                        Msg::Job(seq, job) => {
                            // A failed writer must not process later jobs (issue 8), and
                            // jobs submitted before a repair are stale even after it clears
                            // the failure: their update builds on a snapshot that never
                            // landed. They are dropped; `Store::sync_index` rewrites the
                            // shards whole.
                            if s2.skip(seq) {
                                continue;
                            }
                            let r = write_job(&mut conn, &job);
                            if let Ok(referenced_before) = &r {
                                s2.publish_tag_pages(&conn, &job, referenced_before);
                            }
                            match r {
                                Ok(_) => s2.commit(seq),
                                Err(e) => {
                                    eprintln!("utopia: commit failed: {e}");
                                    s2.fail(e.to_string());
                                }
                            }
                        }
                        Msg::Stop => break,
                    }
                }
            })?;
        Ok(Persister::Thread {
            tx,
            shared,
            handle: Some(handle),
            submitted: 0,
        })
    }

    /// Hand a job to the writer and return its sequence number. A failed writer accepts no
    /// jobs until it has been repaired (issue 8).
    pub fn submit(&mut self, conn: &mut Connection, job: Job) -> Result<u64> {
        if let Some(e) = self.shared().failure() {
            return Err(StoreError::Writer(e));
        }
        match self {
            Persister::Inline { shared, submitted } => {
                *submitted += 1;
                let seq = *submitted;
                match write_job(conn, &job) {
                    Ok(referenced_before) => {
                        shared.publish_tag_pages(conn, &job, &referenced_before);
                        shared.commit(seq);
                        Ok(seq)
                    }
                    Err(e) => {
                        shared.fail(e.to_string());
                        Err(e)
                    }
                }
            }
            Persister::Thread {
                tx,
                submitted,
                shared,
                ..
            } => {
                *submitted += 1;
                if tx.send(Msg::Job(*submitted, job)).is_err() {
                    // The writer is gone; make waiters return instead of hanging.
                    shared.fail("writer thread stopped".into());
                    return Err(StoreError::Writer("writer thread stopped".into()));
                }
                Ok(*submitted)
            }
        }
    }

    pub fn ticket(&self) -> WriteTicket {
        let shared = self.shared();
        let target = match self {
            Persister::Inline { submitted, .. } | Persister::Thread { submitted, .. } => *submitted,
        };
        WriteTicket {
            shared: Some(shared.clone()),
            target,
        }
    }

    /// The failure that stopped the writer, if it is still failed.
    pub fn failure(&self) -> Option<StoreError> {
        self.shared().failure().map(StoreError::Writer)
    }

    /// Highest job sequence that actually committed.
    pub fn completed(&self) -> u64 {
        self.shared().completed()
    }

    /// Mark the writer healthy again, after `Store` has queued a full rewrite of every
    /// shard whose durable copy lagged. Nothing submitted before this point will be
    /// written, so the requeued snapshots are the source of truth.
    pub fn clear_failure(&self) {
        let through = match self {
            Persister::Inline { submitted, .. } | Persister::Thread { submitted, .. } => *submitted,
        };
        self.shared().clear_failure(through);
    }

    /// The tag-page classification published by the last commit, if any (SB-2).
    pub fn tag_pages(&self) -> Option<HashSet<PageId>> {
        self.shared()
            .tag_pages
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// How many per-page tag classifications the incremental path has run (tests, SB-2).
    pub fn tag_classifications(&self) -> u64 {
        self.shared()
            .tag_classifications
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// How many whole-graph tag classifications this writer has run (tests, SB-2).
    /// How many pages the incremental tag publisher looked at (tests, SB-2): the pages a
    /// commit wrote plus the destinations whose ref edges it changed, never the whole
    /// published set.
    pub fn tag_pages_visited(&self) -> u64 {
        self.shared()
            .tag_pages_visited
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn tag_scans(&self) -> u64 {
        self.shared()
            .tag_scans
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Wait for every submitted job; report the failure, which stays set until repaired.
    pub fn wait(&self) -> Result<()> {
        self.ticket().wait()
    }

    pub fn stop(&mut self) {
        if let Persister::Thread { tx, handle, .. } = self {
            let _ = tx.send(Msg::Stop);
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review P2: a job submitted before the repair (while `submit` still saw a healthy
    /// writer) must not run after `clear_failure`; only jobs submitted after it may.
    #[test]
    fn jobs_submitted_before_a_repair_are_stale() {
        let shared = Arc::new(Shared::default());
        shared.fail("boom".into());
        assert!(shared.skip(1), "a failed writer drops queued jobs");
        assert_eq!(shared.completed(), 0, "a dropped job is not durable");

        let ticket = WriteTicket {
            shared: Some(shared.clone()),
            target: 2,
        };
        assert!(ticket.wait().is_err(), "a failed writer reports the error");

        shared.clear_failure(2);
        assert_eq!(shared.failure(), None);
        assert!(shared.skip(2), "submitted before the repair: stale");
        assert!(!shared.skip(3), "the repair job itself is written");
        // Nothing was written for job 2, but the writer has accounted for it; a waiter on
        // the pre-repair target must not hang.
        ticket.wait().expect("a processed job releases its ticket");

        shared.commit(3);
        assert_eq!(shared.completed(), 3);
    }
}
