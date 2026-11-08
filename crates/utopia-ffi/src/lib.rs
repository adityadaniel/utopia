//! The mobile core (PRD §14, Option A): the same store, CRDT and sync engine as the
//! desktop, behind a small UniFFI surface. The surface covers the minimal mobile scope
//! (§14.3): today's page, bullet capture, references (with title search for `[[`),
//! backlinks, full offline editing and sync. Nothing here knows about any UI toolkit.
//!
//! Swift bindings: `cargo run -p utopia-ffi --bin uniffi-bindgen -- generate
//! --library <libutopia_ffi> --language swift --out-dir <dir>` (see `xtask mobile`).

use std::sync::{Arc, Mutex, MutexGuard};

use chrono::NaiveDate;
use utopia_core::{outline, BlockId, PageId};
use utopia_store::{RefQuery, Store, StoreError};
use utopia_sync::{StoreDone, StoreStep, SyncEngine, SyncError, SyncReport, SyncStatus, Transport};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("this graph is encrypted; the passphrase is missing or wrong")]
    Locked,
    #[error("not found: {msg}")]
    NotFound { msg: String },
    #[error("{msg}")]
    Invalid { msg: String },
    #[error("sync: {msg}")]
    Sync { msg: String },
    #[error("{msg}")]
    Internal { msg: String },
}

impl From<StoreError> for FfiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Encrypted => FfiError::Locked,
            StoreError::NotFound(msg) => FfiError::NotFound { msg },
            StoreError::Invalid(msg) => FfiError::Invalid { msg },
            StoreError::Domain(d) => FfiError::Invalid { msg: d.to_string() },
            other => FfiError::Internal {
                msg: other.to_string(),
            },
        }
    }
}

impl From<SyncError> for FfiError {
    fn from(e: SyncError) -> Self {
        // A store fault keeps its own answer; everything else can only happen while the
        // transport is being built (a round reports its failures as a `SyncOutcome`).
        match e {
            SyncError::Store(e) => FfiError::from(e),
            other => FfiError::Sync {
                msg: other.to_string(),
            },
        }
    }
}

type Result<T, E = FfiError> = std::result::Result<T, E>;

/// One visible block of a page, in outline order.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BlockView {
    pub id: String,
    pub text: String,
    /// Plain text with markup removed (for list rows).
    pub plain: String,
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PageView {
    pub id: String,
    pub title: String,
    /// `YYYY-MM-DD` for journal pages.
    pub date: Option<String>,
    pub blocks: Vec<BlockView>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RefView {
    pub page_id: String,
    pub page_title: String,
    pub block_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct SyncSummary {
    pub pushed: u32,
    pub pulled_pages: u32,
    pub imported_pages: u32,
    /// Pages that failed while the rest of the round applied (§12.6). The shell shows
    /// the counts as a successful sync and this as a warning, instead of one error.
    pub failed: u32,
}

/// What one [`Graph::sync`] call did, in the terms §12.6 asks the shell to show.
///
/// One variant per answer the shell has to render, so a wrong token, a relay speaking
/// another protocol and an unreachable relay cannot collapse into one "Offline"
/// (`docs/state-machines/ffi-mobile-sync.md`, issue 3). `SyncSummary` is unchanged: it is
/// still the counts, and the outcome says which counts mean what.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SyncOutcome {
    /// The relay was reached and every page went through.
    Synced { summary: SyncSummary },
    /// The relay was reached but at least one page failed; the rest of the round applied
    /// (§12.6). `queued` counts this device's unsent updates afterwards.
    Partial { summary: SyncSummary, queued: u32 },
    /// The relay was not reached: everything is still queued locally (§12.6).
    Offline { queued: u32 },
    /// The round failed on this device (the store, a shard, a projection), so the network
    /// is not blamed for it.
    LocalError { message: String },
    /// 401/403: the token is wrong or revoked. Sync halts until it is replaced (§12.6).
    NeedsAuth,
    /// The relay speaks a protocol version this client does not; nothing was pushed
    /// (§12.6). `version` is the relay's; 0 means the client never learned it (it is only
    /// unknown when the mismatch did not come from the round, which cannot happen today).
    Unsupported { version: u32 },
    /// A round was already running on this graph, so this call did not start a second one
    /// (issue 2). The running round's own outcome is the one to show.
    Busy,
}

/// Where a graph's sync engine is, which is also whether a round is running: the one fact
/// the old surface kept only as "the store mutex is held".
///
/// ```text
/// Idle(engine) ──take──▶ Running ──put back──▶ Idle(engine)
/// ```
///
/// `Idle` is the only state a round starts from, and a round owns the engine for its whole
/// length (it is not needed on the wire), so a second round cannot start behind the first
/// and the queued compaction snapshots survive from one call to the next (issues 2 and 4).
enum SyncSlot {
    /// Home between rounds.
    Idle(SyncEngine),
    /// A round is in flight and holds the engine.
    Running,
}

/// An open graph. All calls are serialized on one store.
#[derive(uniffi::Object)]
pub struct Graph {
    store: Mutex<Store>,
    /// The sync machine: never "no engine", which is what made every call build a new one.
    sync: Mutex<SyncSlot>,
}

impl Graph {
    fn with_store(store: Store) -> Result<Self> {
        let engine = SyncEngine::new(&store)?;
        Ok(Self {
            store: Mutex::new(store),
            sync: Mutex::new(SyncSlot::Idle(engine)),
        })
    }

    fn guard(&self) -> std::result::Result<MutexGuard<'_, Store>, StoreError> {
        self.store
            .lock()
            .map_err(|_| StoreError::Invalid("graph lock poisoned".into()))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        let mut s = self.guard()?;
        f(&mut s)
    }

    /// Take the engine for one round, or `None` when a round already holds it.
    fn take_engine(&self) -> Result<Option<SyncEngine>> {
        let mut slot = self.sync.lock().map_err(|_| FfiError::Internal {
            msg: "sync lock poisoned".into(),
        })?;
        Ok(match std::mem::replace(&mut *slot, SyncSlot::Running) {
            SyncSlot::Idle(engine) => Some(engine),
            // Another round is in flight and `Running` is already the state.
            SyncSlot::Running => None,
        })
    }

    /// Hand the engine back at the end of a round, whatever the round did.
    fn put_engine(&self, engine: SyncEngine) -> Result<()> {
        let mut slot = self.sync.lock().map_err(|_| FfiError::Internal {
            msg: "sync lock poisoned".into(),
        })?;
        // Invariant: only `take_engine` moves the engine out, and only this puts it back.
        debug_assert!(matches!(*slot, SyncSlot::Running));
        *slot = SyncSlot::Idle(engine);
        Ok(())
    }

    /// The round itself, over whatever transport is in force: the HTTP relay for the shell,
    /// an in-memory one for tests. The store lock is taken for `prepare` and `apply` only;
    /// the exchange runs outside it (issue 1).
    fn sync_via(&self, mut transport: Box<dyn Transport>) -> Result<SyncOutcome> {
        // One round at a time: a second call while this one is on the wire does nothing
        // rather than queueing a whole round behind it (issue 2).
        let Some(mut engine) = self.take_engine()? else {
            return Ok(SyncOutcome::Busy);
        };
        let result = utopia_sync::sync_round(&mut engine, transport.as_mut(), |engine, step| {
            let mut store = self.guard()?;
            match step {
                StoreStep::Prepare => engine.prepare(&mut store).map(StoreDone::Prepared),
                StoreStep::Apply(inc) => engine.apply(&mut store, *inc).map(StoreDone::Applied),
            }
        });
        // The engine goes home whatever the round did: the next call starts from it and
        // keeps the compaction snapshots this one queued (issue 4).
        let restored = self.put_engine(engine);
        let outcome = self.with(|store| Ok(outcome_of(&result, store)));
        restored?;
        outcome
    }
}

fn page_view(store: &mut Store, page: &PageId) -> Result<PageView> {
    let fmt = store.date_format().clone();
    let st = store.page_state(page)?.clone();
    let blocks = st
        .live_preorder()
        .into_iter()
        .filter_map(|(id, depth)| {
            let b = st.block(&id)?;
            Some(BlockView {
                id: id.as_str().to_string(),
                plain: utopia_core::inline::plain_text(&b.text),
                text: b.text.clone(),
                depth: depth as u32,
            })
        })
        .collect();
    Ok(PageView {
        id: page.as_str().to_string(),
        title: utopia_store::display_title(&st, &fmt),
        date: st.meta.daily_date.map(|d| d.to_string()),
        blocks,
    })
}

#[uniffi::export]
impl Graph {
    /// Open (or create) the graph file. `passphrase` unlocks a graph encrypted at rest.
    #[uniffi::constructor]
    pub fn open(path: String, passphrase: Option<String>) -> Result<Arc<Self>> {
        let store = Store::open_with_key(&path, passphrase.as_deref())?;
        Ok(Arc::new(Self::with_store(store)?))
    }

    /// An in-memory graph (previews and tests).
    #[uniffi::constructor]
    pub fn in_memory() -> Result<Arc<Self>> {
        Ok(Arc::new(Self::with_store(Store::open_in_memory()?)?))
    }

    /// Pin "today" (tests, screenshots); `None` follows the clock.
    pub fn set_today(&self, date: Option<String>) -> Result<()> {
        let d = match date {
            Some(s) => Some(
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(|e| FfiError::Invalid { msg: e.to_string() })?,
            ),
            None => None,
        };
        self.with(|s| {
            s.set_today(d);
            Ok(())
        })
    }

    /// Today's journal page, created (with one empty block) on first open (FR-1.1).
    pub fn today(&self) -> Result<PageView> {
        self.with(|s| {
            let (page, _) = s.open_daily(s.today())?;
            page_view(s, &page)
        })
    }

    pub fn page(&self, page_id: String) -> Result<PageView> {
        self.with(|s| page_view(s, &PageId(page_id)))
    }

    /// The page a title names (dates and aliases resolve like links), if any.
    pub fn find_page(&self, title: String) -> Result<Option<PageView>> {
        self.with(|s| {
            let key = utopia_core::title::normalize_title(&title);
            match utopia_store::resolve_page_key(s.conn(), &key, s.date_format())? {
                Some(p) => page_view(s, &p).map(Some),
                None => Ok(None),
            }
        })
    }

    /// Append a top-level block (fills a lone empty block instead). Links in the text
    /// create their pages and references on commit.
    pub fn append_block(&self, page_id: String, text: String) -> Result<BlockView> {
        self.with(|s| {
            let page = PageId(page_id);
            let st = s.page_state(&page)?.clone();
            let roots = st.live_roots();
            let reuse = match roots.as_slice() {
                [only] => {
                    st.block(only).is_some_and(|b| b.text.is_empty()) && !st.has_live_children(only)
                }
                _ => false,
            };
            s.seal_undo();
            let block = if reuse {
                roots[0].clone()
            } else {
                let plan = outline::append_block(&st, None);
                s.execute_plan(&page, plan, None)?
                    .ok_or_else(|| FfiError::Internal {
                        msg: "insert produced no block".into(),
                    })?
                    .block
            };
            s.set_block_text(&page, &block, &text, None, None)?;
            s.commit_block(&page, &block)?;
            s.flush()?;
            let depth = 0;
            Ok(BlockView {
                id: block.as_str().to_string(),
                plain: utopia_core::inline::plain_text(&text),
                text,
                depth,
            })
        })
    }

    pub fn set_block_text(&self, block_id: String, text: String) -> Result<()> {
        self.with(|s| {
            let block = BlockId(block_id);
            let row = s.block_row(&block)?.ok_or_else(|| FfiError::NotFound {
                msg: format!("block {block}"),
            })?;
            s.seal_undo();
            s.set_block_text(&row.page_id, &block, &text, None, None)?;
            s.commit_block(&row.page_id, &block)?;
            s.flush()?;
            Ok(())
        })
    }

    /// Linked References of a page, newest source pages first.
    pub fn backlinks(&self, page_id: String) -> Result<Vec<RefView>> {
        self.with(|s| {
            let page = PageId(page_id);
            let refs = s.linked_references(
                &page,
                &RefQuery {
                    sort: utopia_store::RefSort::Recency,
                    ..Default::default()
                },
            )?;
            Ok(refs
                .groups
                .into_iter()
                .flat_map(|g| {
                    let title = g.page.title.clone();
                    let pid = g.page.id.as_str().to_string();
                    g.hits.into_iter().map(move |h| RefView {
                        page_id: pid.clone(),
                        page_title: title.clone(),
                        block_id: h.block.as_str().to_string(),
                        text: h.text,
                    })
                })
                .collect())
        })
    }

    /// Page titles for `[[` autocomplete.
    pub fn search_titles(&self, query: String, limit: u32) -> Result<Vec<String>> {
        self.with(|s| {
            Ok(s.search_titles(&query, limit as usize)?
                .into_iter()
                .map(|h| h.title)
                .collect())
        })
    }

    /// One sync round with a relay (§12). `e2e_passphrase` turns on end-to-end
    /// encryption; the same passphrase must be used on every device.
    ///
    /// The network half runs without the store lock (§14.3, rule 10), so `today`, `page`
    /// and `append_block` keep working while the relay is slow, and a call made while a
    /// round is in flight answers [`SyncOutcome::Busy`] instead of queueing a second round.
    pub fn sync(
        &self,
        relay_url: String,
        graph_id: String,
        token: Option<String>,
        e2e_passphrase: Option<String>,
    ) -> Result<SyncOutcome> {
        let transport = relay_transport(&relay_url, &graph_id, token, e2e_passphrase)?;
        self.sync_via(transport)
    }
}

/// The relay transport for a sync call, with end-to-end encryption when a passphrase is
/// given. Building it touches no store, so it happens before the round starts.
fn relay_transport(
    relay_url: &str,
    graph_id: &str,
    token: Option<String>,
    e2e_passphrase: Option<String>,
) -> Result<Box<dyn Transport>> {
    use utopia_sync::http::HttpTransport;
    let http = HttpTransport::new(relay_url, graph_id, token)?;
    Ok(match e2e_passphrase {
        Some(p) => {
            let key = utopia_sync::e2e::GraphKey::derive(&p, graph_id)?;
            Box::new(utopia_sync::e2e::Encrypted::new(
                http,
                key,
                graph_id.to_string(),
            ))
        }
        None => Box::new(http),
    })
}

/// The round's answer for the shell: [`SyncStatus`] (the desktop's one reading of a round,
/// so mobile and desktop cannot drift) plus the counts the shell shows.
fn outcome_of(result: &std::result::Result<SyncReport, SyncError>, store: &Store) -> SyncOutcome {
    // A refusal is read from the error, because only the error knows the relay's protocol
    // version (§12.6).
    match result {
        Err(SyncError::Unauthorized) => return SyncOutcome::NeedsAuth,
        Err(SyncError::ProtocolVersion(version)) => {
            return SyncOutcome::Unsupported { version: *version }
        }
        _ => {}
    }
    // `SyncStatus::from_result` answers `Synced`/`Partial` exactly for an `Ok` report, so a
    // summary is always there; the fallback keeps this total without a panic (rule 11).
    let summary = || result.as_ref().map(summary_of).unwrap_or_default();
    match SyncStatus::from_result(result, store) {
        SyncStatus::Synced { .. } => SyncOutcome::Synced { summary: summary() },
        SyncStatus::Partial { queued, .. } => SyncOutcome::Partial {
            summary: summary(),
            queued: queued as u32,
        },
        SyncStatus::Offline { queued } => SyncOutcome::Offline {
            queued: queued as u32,
        },
        SyncStatus::LocalError { message } => SyncOutcome::LocalError { message },
        SyncStatus::NeedsAuth => SyncOutcome::NeedsAuth,
        // Returned above; kept total without a panic (rule 11) and without inventing a
        // version the client never saw.
        SyncStatus::Unsupported => SyncOutcome::Unsupported { version: 0 },
    }
}

/// The counts the shell shows. One failed page can appear twice in a report (a push and a
/// pull), so this counts pages, not failures.
fn summary_of(r: &SyncReport) -> SyncSummary {
    SyncSummary {
        pushed: r.pushed as u32,
        pulled_pages: r.pulled_pages as u32,
        imported_pages: r.imported.len() as u32,
        failed: r
            .failed
            .iter()
            .map(|f| f.page.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use utopia_sync::wire::{Manifest, ManifestPage, PullResponse, PushRequest, PushResponse};
    use utopia_sync::{CursorKind, CursorSpace, PROTOCOL_VERSION};

    type Result<T> = std::result::Result<T, SyncError>;

    type ManifestStep = Box<dyn FnMut() -> Result<Option<Manifest>> + Send>;
    type PushStep = Box<dyn FnMut(&PageId, PushRequest) -> Result<PushResponse> + Send>;
    type PullStep = Box<dyn FnMut(&PageId, u64) -> Result<PullResponse> + Send>;
    type SnapshotStep = Box<dyn FnMut(&PageId, u64) -> Result<()> + Send>;

    /// A transport with one closure per step, so a test can put a round in a known state
    /// (on the wire, refused, refused for one page) without a server.
    struct Scripted {
        /// The cursor space this "relay" speaks; a snapshot is only valid in the one that
        /// measured it.
        space: CursorSpace,
        on_manifest: ManifestStep,
        on_push: PushStep,
        on_pull: PullStep,
        on_snapshot: SnapshotStep,
    }

    impl Scripted {
        /// A reached, empty relay: no pages to pull, every push accepted.
        fn quiet() -> Self {
            Self {
                space: CursorSpace::new("relay:scripted#g", CursorKind::Seq),
                on_manifest: Box::new(|| Ok(Some(empty_manifest()))),
                on_push: Box::new(|_page, req| {
                    Ok(PushResponse {
                        accepted: req.updates.len(),
                        head_seq: 1,
                        duplicate: 0,
                    })
                }),
                on_pull: Box::new(|_page, _since| {
                    Ok(PullResponse {
                        snapshot: None,
                        updates: Vec::new(),
                        head_seq: 0,
                    })
                }),
                on_snapshot: Box::new(|_page, _covers| Ok(())),
            }
        }

        /// Every step answers `error` (once; a step asked again says the script is spent).
        fn failing(error: SyncError) -> Self {
            fn spent() -> SyncError {
                SyncError::Transport("scripted: spent".into())
            }
            let err = std::sync::Arc::new(std::sync::Mutex::new(Some(error)));
            let step = |err: &std::sync::Arc<std::sync::Mutex<Option<SyncError>>>| {
                err.lock()
                    .expect("the script is not poisoned")
                    .take()
                    .unwrap_or_else(spent)
            };
            Self {
                space: CursorSpace::new("relay:scripted#g", CursorKind::Seq),
                on_manifest: Box::new({
                    let err = err.clone();
                    move || Err(step(&err))
                }),
                on_push: Box::new({
                    let err = err.clone();
                    move |_page, _req| Err(step(&err))
                }),
                on_pull: Box::new({
                    let err = err.clone();
                    move |_page, _since| Err(step(&err))
                }),
                on_snapshot: Box::new(move |_page, _covers| Err(step(&err))),
            }
        }
    }

    impl Transport for Scripted {
        fn manifest(&mut self, _if_none_match: Option<u64>) -> Result<Option<Manifest>> {
            (self.on_manifest)()
        }
        fn cursor_space(&self) -> CursorSpace {
            self.space.clone()
        }
        fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
            (self.on_push)(page, req)
        }
        fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse> {
            (self.on_pull)(page, since_seq)
        }
        fn put_snapshot(
            &mut self,
            page: &PageId,
            covers_seq: u64,
            _payload: Vec<u8>,
        ) -> Result<()> {
            (self.on_snapshot)(page, covers_seq)
        }
    }

    fn empty_manifest() -> Manifest {
        Manifest {
            rev: 1,
            protocol_version: PROTOCOL_VERSION,
            pages: Vec::new(),
            instance: "scripted".into(),
        }
    }

    /// One page, `head_seq` long, with `snapshot_seq` already compacted.
    fn manifest_with(page: &PageId, head_seq: u64, snapshot_seq: u64) -> Manifest {
        Manifest {
            pages: vec![ManifestPage {
                page_id: page.as_str().to_string(),
                head_seq,
                frontier: String::new(),
                snapshot_seq,
                tail_bytes: 0,
                updated_at: 0,
                deleted: false,
            }],
            ..empty_manifest()
        }
    }

    fn graph_with_two_days() -> (Arc<Graph>, PageView, PageView) {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.today().unwrap();
        g.append_block(day.id.clone(), "one".into()).unwrap();
        g.set_today(Some("2026-11-03".into())).unwrap();
        let next = g.today().unwrap();
        g.append_block(next.id.clone(), "two".into()).unwrap();
        (g, day, next)
    }

    /// Issue 1: other FFI calls keep working while a round is on the wire. The store lock
    /// is taken for `prepare`/`apply`, never for the network.
    #[test]
    fn a_round_does_not_hold_the_store_lock_on_the_wire() {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.today().unwrap();
        g.append_block(day.id.clone(), "queued".into()).unwrap();
        let free = Arc::new(AtomicBool::new(true));
        let probes = Arc::new(AtomicU32::new(0));
        let probe = |free: &Arc<AtomicBool>, probes: &Arc<AtomicU32>, g: &Arc<Graph>| {
            probes.fetch_add(1, Ordering::SeqCst);
            if g.store.try_lock().is_err() {
                free.store(false, Ordering::SeqCst);
            }
        };
        let page = PageId(day.id.clone());
        let mut t = Scripted::quiet();
        t.on_manifest = Box::new({
            let (free, probes, g) = (free.clone(), probes.clone(), g.clone());
            move || {
                probe(&free, &probes, &g);
                Ok(Some(manifest_with(&page, 0, 0)))
            }
        });
        t.on_push = Box::new({
            let (free, probes, g) = (free.clone(), probes.clone(), g.clone());
            move |_page, req| {
                probe(&free, &probes, &g);
                Ok(PushResponse {
                    accepted: req.updates.len(),
                    head_seq: 1,
                    duplicate: 0,
                })
            }
        });
        t.on_pull = Box::new({
            let (free, probes, g) = (free.clone(), probes.clone(), g.clone());
            move |_page, _since| {
                probe(&free, &probes, &g);
                Ok(PullResponse {
                    snapshot: None,
                    updates: Vec::new(),
                    head_seq: 1,
                })
            }
        });
        let outcome = g.sync_via(Box::new(t)).unwrap();
        assert!(matches!(outcome, SyncOutcome::Synced { .. }), "{outcome:?}");
        assert!(
            probes.load(Ordering::SeqCst) >= 3,
            "the round must actually reach the wire"
        );
        assert!(
            free.load(Ordering::SeqCst),
            "the store lock was held during the exchange, so `today`/`append_block` block"
        );
    }

    /// Issue 2: a second tap cannot queue a second round behind the first.
    #[test]
    fn a_second_round_while_one_runs_is_busy() {
        let g = Graph::in_memory().unwrap();
        let (reached, on_wire) = mpsc::channel();
        let (release, held) = mpsc::channel::<()>();
        let mut blocking = Scripted::quiet();
        blocking.on_manifest = Box::new(move || {
            reached.send(()).expect("the test is listening");
            held.recv_timeout(Duration::from_secs(10))
                .expect("the test releases the round");
            Ok(Some(empty_manifest()))
        });
        let round = {
            let g = g.clone();
            std::thread::spawn(move || g.sync_via(Box::new(blocking)).unwrap())
        };
        on_wire
            .recv_timeout(Duration::from_secs(10))
            .expect("the first round reached the wire");
        assert_eq!(
            g.sync_via(Box::new(Scripted::quiet())).unwrap(),
            SyncOutcome::Busy,
            "a second round must not start while the first holds the engine"
        );
        release.send(()).expect("the round is still waiting");
        assert!(matches!(round.join().unwrap(), SyncOutcome::Synced { .. }));
        // The engine came home, so the next tap runs a round again.
        assert!(!matches!(
            g.sync_via(Box::new(Scripted::quiet())).unwrap(),
            SyncOutcome::Busy
        ));
    }

    /// The engine goes home on every path, including a round that fails before it applies:
    /// a stuck `Running` would answer `Busy` forever.
    #[test]
    fn the_engine_comes_home_after_a_failed_round() {
        let g = Graph::in_memory().unwrap();
        let outcome = g
            .sync_via(Box::new(Scripted::failing(SyncError::Transport(
                "offline".into(),
            ))))
            .unwrap();
        assert!(
            matches!(outcome, SyncOutcome::Offline { .. }),
            "{outcome:?}"
        );
        assert!(
            matches!(
                g.sync_via(Box::new(Scripted::quiet())).unwrap(),
                SyncOutcome::Synced { .. }
            ),
            "a failed round must not leave the graph busy"
        );
    }

    /// Issue 3: the shell can tell a refusal from a bad network, and neither blames the
    /// other.
    #[test]
    fn a_refusal_is_not_offline() {
        let g = Graph::in_memory().unwrap();
        let outcome = |error: SyncError| {
            g.sync_via(Box::new(Scripted::failing(error)))
                .expect("a round reports its answer, it does not fail the call")
        };
        assert_eq!(
            outcome(SyncError::Unauthorized),
            SyncOutcome::NeedsAuth,
            "§12.6: 401 halts sync for a re-auth, it is not Offline"
        );
        assert_eq!(
            outcome(SyncError::ProtocolVersion(9)),
            SyncOutcome::Unsupported { version: 9 },
            "§12.6: another protocol version is its own answer"
        );
        assert!(
            matches!(
                outcome(SyncError::Transport("refused".into())),
                SyncOutcome::Offline { .. }
            ),
            "a transport failure is Offline"
        );
        let local = outcome(SyncError::Store(StoreError::Invalid("disk".into())));
        assert!(
            matches!(&local, SyncOutcome::LocalError { message } if message.contains("disk")),
            "{local:?}"
        );
    }

    /// §12.6: a relay that took some pages and refused one is not "Offline" and not a clean
    /// sync either; the shell shows the counts and the warning.
    #[test]
    fn a_page_that_fails_while_the_rest_moves_is_partial() {
        let (g, refused_day, other_day) = graph_with_two_days();
        let refused = refused_day.id.clone();
        assert_ne!(refused, other_day.id);
        let mut t = Scripted::quiet();
        t.on_push = Box::new(move |page, req| {
            if page.as_str() == refused {
                Err(SyncError::Transport("relay refused this page".into()))
            } else {
                Ok(PushResponse {
                    accepted: req.updates.len(),
                    head_seq: 1,
                    duplicate: 0,
                })
            }
        });
        match g.sync_via(Box::new(t)).unwrap() {
            SyncOutcome::Partial { summary, queued } => {
                assert_eq!(summary.failed, 1, "one page failed");
                assert!(summary.pushed > 0, "the other page's push was accepted");
                assert!(queued >= 1, "the refused page's rows stay queued");
            }
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    /// Issue 4: a compaction snapshot queued by one round is uploaded by the next, because
    /// the engine (which holds the queue) lives on the graph, not on the round.
    #[test]
    fn a_queued_snapshot_rides_the_next_round() {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.today().unwrap();
        g.append_block(day.id.clone(), "long tail".into()).unwrap();
        let page = PageId(day.id.clone());

        // Round 1: the relay's tail for the page looks 1000 updates long, so applying it
        // queues a snapshot; a refused push stops the round before that snapshot goes out.
        let mut first = Scripted::quiet();
        first.on_manifest = Box::new({
            let page = page.clone();
            move || Ok(Some(manifest_with(&page, 1000, 0)))
        });
        first.on_push =
            Box::new(|_page, _req| Err(SyncError::Transport("refused for the test".into())));
        first.on_pull = Box::new(|_page, _since| {
            Ok(PullResponse {
                snapshot: None,
                updates: Vec::new(),
                head_seq: 1000,
            })
        });
        let _ = g.sync_via(Box::new(first)).unwrap();

        // Round 2: the tail now looks compacted, so nothing asks for a new snapshot; the
        // only way one is uploaded is if round 1's queue outlived it.
        let sent = Arc::new(AtomicU32::new(0));
        let mut second = Scripted::quiet();
        second.on_manifest = Box::new(move || Ok(Some(manifest_with(&page, 1000, 1000))));
        second.on_snapshot = Box::new({
            let sent = sent.clone();
            move |_page, _covers| {
                sent.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let _ = g.sync_via(Box::new(second)).unwrap();
        assert_eq!(
            sent.load(Ordering::SeqCst),
            1,
            "the snapshot queued by the first round must ride the second"
        );
    }

    /// The round's answer keeps the desktop's wording (`SyncStatus`) so the two surfaces
    /// cannot drift: the same report maps to the same counts.
    #[test]
    fn the_summary_counts_pages_not_failures() {
        let page = PageId("p".into());
        let report = SyncReport {
            pushed: 2,
            duplicates: 1,
            pulled_pages: 3,
            imported: vec![page.clone()],
            failed: vec![
                utopia_sync::Failure {
                    page: Some(page.clone()),
                    kind: utopia_sync::FailureKind::Transport("push".into()),
                },
                utopia_sync::Failure {
                    page: Some(page.clone()),
                    kind: utopia_sync::FailureKind::Transport("pull".into()),
                },
            ],
            ..Default::default()
        };
        let summary = summary_of(&report);
        assert_eq!(summary.pushed, 2);
        assert_eq!(summary.imported_pages, 1);
        assert_eq!(summary.failed, 1, "one page, two failures");
        assert_eq!(
            report
                .failed
                .iter()
                .map(|f| f.page.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            1
        );
    }

    /// Retaining the engine must not carry a compaction snapshot to another destination:
    /// the snapshot is only this device's page, and a relay's log is not ours to truncate
    /// (AGENTS.md rule 5). A snapshot measured on one relay must not be uploaded to a
    /// graph id or relay url the next call points at.
    #[test]
    fn a_snapshot_queued_for_one_relay_is_not_uploaded_to_another() {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.today().unwrap();
        g.append_block(day.id.clone(), "long tail".into()).unwrap();
        let page = PageId(day.id.clone());

        // Round 1 against the first relay: the tail looks 1000 updates long, so applying
        // it queues a snapshot, and a refused push stops the round before it goes out.
        let mut first = Scripted::quiet();
        first.space = CursorSpace::new("relay:first#g", CursorKind::Seq);
        first.on_manifest = Box::new({
            let page = page.clone();
            move || Ok(Some(manifest_with(&page, 1000, 0)))
        });
        first.on_push =
            Box::new(|_page, _req| Err(SyncError::Transport("refused for the test".into())));
        first.on_pull = Box::new(|_page, _since| {
            Ok(PullResponse {
                snapshot: None,
                updates: Vec::new(),
                head_seq: 1000,
            })
        });
        let _ = g.sync_via(Box::new(first));

        // Round 2 goes to another relay that already holds its own history for the page.
        let uploaded = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let mut second = Scripted::quiet();
        second.space = CursorSpace::new("relay:second#g", CursorKind::Seq);
        second.on_manifest = Box::new({
            let page = page.clone();
            move || Ok(Some(manifest_with(&page, 5000, 5000)))
        });
        second.on_pull = Box::new(|_page, _since| {
            Ok(PullResponse {
                snapshot: None,
                updates: Vec::new(),
                head_seq: 5000,
            })
        });
        second.on_snapshot = Box::new({
            let uploaded = uploaded.clone();
            move |_page, covers| {
                uploaded.lock().expect("not poisoned").push(covers);
                Ok(())
            }
        });
        let _ = g.sync_via(Box::new(second));
        assert_eq!(
            uploaded.lock().expect("not poisoned").as_slice(),
            &[] as &[u64],
            "a snapshot covering the first relay's head was uploaded to the second"
        );
    }
}
