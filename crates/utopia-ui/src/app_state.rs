//! Shared application state: the store plus a change feed views observe.
//!
//! The store runs in debounced mode (§13.3): edits apply to the in-memory shards at once,
//! commits reach the writer thread after 250 ms idle or 1 s at most (immediately for
//! structural edits), and [`IndexUpdated`] fires once the writer has made them durable so
//! reference panels can re-query.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{Context, EventEmitter, Task};
use utopia_core::PageId;
use utopia_store::Store;

const IDLE_FLUSH: Duration = Duration::from_millis(250);
const MAX_FLUSH_DELAY: Duration = Duration::from_secs(1);
/// A failed repair is not retried on every keystroke (it snapshots and waits on disk);
/// wait this long before trying again.
const REPAIR_BACKOFF: Duration = Duration::from_secs(1);

/// Emitted after every mutation so views refresh only what they show.
#[derive(Debug, Clone)]
pub struct Changed {
    pub pages: Vec<PageId>,
    /// Tree shape, page identity or references changed (not just a keystroke).
    pub structural: bool,
    /// `AppState::version` after this change. A view that wrote and then rebuilt in the
    /// same handler compares this with the version it built from, so its own `Changed`
    /// (delivered after the update; gpui queues `emit`) does not dirty it again.
    pub version: u64,
}

/// The index (backlinks, page list, search) now reflects every flushed change.
#[derive(Debug, Clone)]
pub struct IndexUpdated;

/// The page a view or the editor holds is now an alias for another page (FR-1.2): a
/// promoted draft merged into the page that already owned its day. Whoever holds `from`
/// must retarget `to`; the old id has no shard of its own.
#[derive(Debug, Clone)]
pub struct PageReplaced {
    pub from: PageId,
    pub to: PageId,
}

pub struct AppState {
    pub store: Store,
    /// Bumped on every change.
    pub version: u64,
    /// Bumped when the index caught up; reference panels re-query on change.
    pub refs_version: u64,
    pub last_error: Option<String>,
    /// A transient message (e.g. from a plugin), shown in the sidebar footer.
    pub notice: Option<String>,
    /// Plugins from `<config>/plugins` (P2), offered in the slash menu.
    pub plugins: Vec<std::rc::Rc<utopia_plugin::Plugin>>,
    /// Pages that exist only because of a `#tag` (SB-2), cached from the index. Views read
    /// this; the classification query runs at startup and on [`IndexUpdated`], never in a
    /// render (rule 10).
    pub tag_pages: HashSet<PageId>,
    dirty_since: Option<Instant>,
    flush_task: Option<Task<()>>,
    wait_task: Option<Task<()>>,
    /// When a failed writer may be repaired again (backoff, review P3).
    repair_at: Option<Instant>,
    repair_task: Option<Task<()>>,
}

impl EventEmitter<Changed> for AppState {}
impl EventEmitter<IndexUpdated> for AppState {}
impl EventEmitter<PageReplaced> for AppState {}

impl AppState {
    pub fn new(mut store: Store) -> Self {
        if let Err(e) = store.set_debounced(true) {
            eprintln!("utopia: {e}");
        }
        // Classify once at startup and *publish* the answer (SB-2): the copy the UI starts
        // with is then the set the first commit updates, not a second classification living
        // beside that slot. Each commit publishes the new set with the job, so neither the
        // sidebar's render nor an index update runs the query (rule 10).
        let tag_pages = match store.republish_tag_pages() {
            Ok(pages) => pages,
            Err(e) => {
                eprintln!("utopia: {e}");
                HashSet::new()
            }
        };
        Self {
            store,
            version: 0,
            refs_version: 0,
            last_error: None,
            notice: None,
            plugins: Vec::new(),
            tag_pages,
            dirty_since: None,
            flush_task: None,
            wait_task: None,
            repair_at: None,
            repair_task: None,
        }
    }

    /// Make every pending edit durable before the process exits, and say so when the writer
    /// failed: a quit that drops the last keystroke or hides a failed commit is the bug this
    /// exists to prevent (rule 2, AC-1.1). The app's one quit path calls this; the
    /// `on_app_quit` backstop calls it again and finds nothing left to flush.
    pub fn flush_for_quit(&mut self) {
        let r = self.store.sync_index();
        self.report(r);
        let r = self.store.compact();
        self.report(r);
    }

    pub fn changed(&mut self, pages: Vec<PageId>, structural: bool, cx: &mut Context<Self>) {
        // A promoted draft that merged into the day, or a stale draft retired for it
        // (FR-1.2), leaves its id as a session alias: announce the replacement so views and
        // the editor retarget, and report the survivor in `Changed` so subscriptions keyed
        // on the page match. Replacements the store made on its own reach views the caller
        // did not name.
        let mut replaced: Vec<(PageId, PageId)> = Vec::new();
        let pages: Vec<PageId> = pages
            .into_iter()
            .map(|page| match self.store.redirect(&page) {
                Some(to) => {
                    replaced.push((page, to.clone()));
                    to
                }
                None => page,
            })
            .collect();
        for pair in self.store.take_replacements() {
            if !replaced.iter().any(|(from, _)| *from == pair.0) {
                replaced.push(pair);
            }
        }
        for (from, to) in replaced {
            cx.emit(PageReplaced { from, to });
        }
        self.version += 1;
        cx.emit(Changed {
            pages,
            structural,
            version: self.version,
        });
        if structural {
            self.flush_now(cx);
        } else {
            self.schedule_flush(cx);
        }
        cx.notify();
    }

    fn schedule_flush(&mut self, cx: &mut Context<Self>) {
        if !self.store.has_pending() {
            // Nothing to flush: drop the timer *and* the clock that caps it. Keeping the
            // old `dirty_since` would make the next keystroke flush at once (issue 4).
            self.flush_task = None;
            self.dirty_since = None;
            return;
        }
        let now = Instant::now();
        let first = *self.dirty_since.get_or_insert(now);
        if now.duration_since(first) >= MAX_FLUSH_DELAY {
            return self.flush_now(cx);
        }
        self.flush_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(IDLE_FLUSH).await;
            let _ = this.update(cx, |app, cx| app.flush_now(cx));
        }));
    }

    /// Hand pending edits to the writer and announce when they are durable.
    pub fn flush_now(&mut self, cx: &mut Context<Self>) {
        self.flush_task = None;
        self.dirty_since = None;
        if let Err(e) = self.store.flush() {
            // A failed writer is a state: report it, repair from the retained shards once,
            // and only claim the index moved when the repaired commit actually landed.
            self.report::<()>(Err(e));
            if self.repair_at.is_some_and(|at| Instant::now() < at) {
                // A repair is already backing off: do not snapshot and wait on disk again
                // from the keystroke path (rule 10, review P3).
                cx.notify();
                return;
            }
            let _ = self.store.sync_index();
            // The repair's retry takes `pending` even when it fails, so an empty queue
            // does not mean the index moved: ask the writer's ticket instead.
            match self.store.write_ticket().wait() {
                Ok(()) => {
                    self.repair_at = None;
                    self.repair_task = None;
                    self.index_updated(cx);
                }
                Err(e) => {
                    self.report::<()>(Err(e));
                    self.repair_at = Some(Instant::now() + REPAIR_BACKOFF);
                    self.schedule_repair(cx);
                    cx.notify();
                }
            }
            return;
        }
        // The writer is healthy enough to take this edit; a scheduled repair is moot.
        self.repair_at = None;
        self.repair_task = None;
        let ticket = self.store.write_ticket();
        if ticket.is_done() {
            match ticket.wait() {
                Ok(()) => self.index_updated(cx),
                Err(e) => {
                    self.report::<()>(Err(e));
                    cx.notify();
                }
            }
            return;
        }
        self.wait_task = Some(cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { ticket.wait() })
                .await;
            let _ = this.update(cx, |app, cx| match outcome {
                Ok(()) => app.index_updated(cx),
                Err(e) => {
                    // The waiter resolved on a commit that did not happen: report it, but
                    // do not emit `IndexUpdated`.
                    app.report::<()>(Err(e));
                    cx.notify();
                }
            });
        }));
    }

    /// Retry a failed repair after the backoff, without blocking the keystroke path.
    fn schedule_repair(&mut self, cx: &mut Context<Self>) {
        self.repair_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REPAIR_BACKOFF).await;
            let _ = this.update(cx, |app, cx| {
                app.repair_task = None;
                app.flush_now(cx);
            });
        }));
    }

    fn index_updated(&mut self, cx: &mut Context<Self>) {
        self.refs_version += 1;
        // The commit that just landed published the tag-page classification with it (SB-2):
        // take that copy. Re-classifying here would put a whole-graph query on the UI thread
        // after every keystroke flush (§15.1, rule 10).
        if let Some(pages) = self.store.tag_pages() {
            self.tag_pages = pages;
        }
        cx.emit(IndexUpdated);
        cx.notify();
    }

    /// Record a store error for the status line instead of panicking (AGENTS.md rule 11).
    /// Load every plugin under `dir`; broken ones are reported, not fatal.
    pub fn load_plugins(&mut self, dir: &std::path::Path) {
        self.plugins.clear();
        for (path, r) in utopia_plugin::load_all(dir) {
            match r {
                Ok(p) => self.plugins.push(std::rc::Rc::new(p)),
                Err(e) => {
                    eprintln!("utopia: plugin {}: {e}", path.display());
                    self.last_error = Some(format!("plugin {}: {e}", path.display()));
                }
            }
        }
    }

    pub fn report<T>(&mut self, r: Result<T, utopia_store::StoreError>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(utopia_store::StoreError::Domain(_)) => None,
            Err(e) => {
                eprintln!("utopia: {e}");
                self.last_error = Some(e.to_string());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use gpui::{AppContext as _, TestAppContext};

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
    }

    /// A store with one durable daily block, ready for a text edit.
    fn store() -> (Store, PageId, utopia_core::BlockId) {
        let mut store = Store::open_in_memory().unwrap();
        store.set_today(Some(today()));
        let (page, block) = store.open_daily(today()).unwrap();
        store
            .set_block_text(&page, &block, "a", None, None)
            .unwrap();
        store.sync_index().unwrap();
        (store, page, block)
    }

    /// Issue 4: a change with nothing pending returned early and left `dirty_since` from an
    /// earlier edit, so the next keystroke could flush immediately instead of debouncing.
    #[gpui::test]
    fn a_change_with_nothing_pending_forgets_the_dirty_clock(cx: &mut TestAppContext) {
        let (store, _page, _block) = store();
        let app = cx.new(|_| AppState::new(store));
        app.update(cx, |app, cx| {
            // A local-only change (no store write) after the timer was armed elsewhere.
            app.dirty_since = Some(Instant::now() - Duration::from_secs(2));
            app.changed(Vec::new(), false, cx);
            assert!(
                app.dirty_since.is_none(),
                "the dirty clock follows the pending set"
            );
        });
    }

    /// SB-2: one copy of the classification. A session that opens an existing graph has
    /// published nothing yet (the slot is `Unscanned`) while the index already has the tag
    /// pages, so the copy the UI starts with has to *be* the published set — otherwise two
    /// pieces of code classify the same index and only one of them is updated by commits.
    #[gpui::test]
    fn the_startup_tag_set_is_the_published_one(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.utopia");
        {
            let mut store = Store::open(&path).unwrap();
            store.set_today(Some(today()));
            let (day, block) = store.open_daily(today()).unwrap();
            store
                .set_block_text(&day, &block, "wrote about #atlas", None, None)
                .unwrap();
            store.commit_block(&day, &block).unwrap();
            store.sync_index().unwrap();
        }

        // A fresh session: the index has the tag page, no commit here has published a set.
        let store = Store::open(&path).unwrap();
        assert!(
            store.tag_pages().is_none(),
            "nothing has published a set in this session"
        );
        let app = cx.new(|_| AppState::new(store));
        app.update(cx, |app, _| {
            let atlas = app
                .store
                .find_page("atlas")
                .unwrap()
                .expect("the hashtag made the page");
            assert_eq!(
                app.store.tag_pages().as_ref(),
                Some(&app.tag_pages),
                "the startup copy is the published set"
            );
            assert!(app.tag_pages.contains(&atlas));
        });
    }

    const FAIL_COMMIT: &str = "CREATE TRIGGER fail_commit BEFORE INSERT ON outbox
         BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END";

    /// Review P3: a persistent failure must not make every keystroke snapshot and block the
    /// UI thread. The repair backs off; the edit stays queued for the retry.
    #[gpui::test]
    fn a_failed_repair_backs_off_before_retrying(cx: &mut TestAppContext) {
        let (store, page, block) = store();
        store.conn().execute_batch(FAIL_COMMIT).unwrap();
        let app = cx.new(|_| AppState::new(store));
        app.update(cx, |app, cx| {
            app.store
                .set_block_text(&page, &block, "ab", None, None)
                .unwrap();
            app.flush_now(cx);
            assert!(app.last_error.is_some(), "the failure is reported");
            assert!(app.store.has_pending(), "the edit is kept for the repair");
        });
        // The disk recovers, but the next keystroke arrives inside the backoff window.
        app.update(cx, |app, _| {
            app.store
                .conn()
                .execute_batch("DROP TRIGGER fail_commit")
                .unwrap();
        });
        app.update(cx, |app, cx| {
            app.store
                .set_block_text(&page, &block, "abc", None, None)
                .unwrap();
            app.flush_now(cx);
            assert!(
                app.store.has_pending(),
                "no synchronous repair inside the backoff window"
            );
            assert_eq!(app.refs_version, 0, "the index did not move");
        });
    }

    /// Review P1: with the thread persister the repair's retry takes `pending` even when it
    /// fails, so an empty queue does not mean the index moved. Key off the writer instead.
    #[gpui::test]
    fn a_failed_repair_does_not_claim_the_index_moved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("g.utopia")).unwrap();
        store.set_today(Some(today()));
        let (page, block) = store.open_daily(today()).unwrap();
        store
            .set_block_text(&page, &block, "a", None, None)
            .unwrap();
        store.sync_index().unwrap();
        store.conn().execute_batch(FAIL_COMMIT).unwrap();

        let app = cx.new(|_| AppState::new(store));
        app.update(cx, |app, cx| {
            app.store
                .set_block_text(&page, &block, "ab", None, None)
                .unwrap();
            app.flush_now(cx);
        });
        // The writer runs the first job and records the failure.
        app.update(cx, |app, _| {
            let _ = app.store.write_ticket().wait();
        });
        app.update(cx, |app, cx| {
            app.store
                .set_block_text(&page, &block, "abc", None, None)
                .unwrap();
            app.flush_now(cx);
            assert!(app.last_error.is_some(), "the failure is reported");
            assert_eq!(
                app.refs_version, 0,
                "a repair that also failed did not move the index"
            );
        });
    }

    /// Issue 2: a quit whose flush fails reports the writer error. The quit path is the
    /// last place the message is visible, and swallowing it there would silently drop the
    /// edit this flush exists to keep (rule 2).
    #[gpui::test]
    fn a_failed_quit_flush_is_reported(cx: &mut TestAppContext) {
        let (store, page, block) = store();
        store.conn().execute_batch(FAIL_COMMIT).unwrap();
        let app = cx.new(|_| AppState::new(store));
        app.update(cx, |app, _| {
            app.store
                .set_block_text(&page, &block, "hello", None, None)
                .unwrap();
            app.flush_for_quit();
            assert!(app.last_error.is_some(), "the writer failure is reported");
        });
    }

    /// Issue 2: the wait task resolves on a writer error, so the failure must be reported
    /// and must not be announced as an index update for a commit that did not happen.
    #[gpui::test]
    fn a_failed_commit_does_not_claim_the_index_moved(cx: &mut TestAppContext) {
        let (store, page, block) = store();
        // Fail the commit through SQLite itself: the writer path runs unchanged.
        store.conn().execute_batch(FAIL_COMMIT).unwrap();
        let app = cx.new(|_| AppState::new(store));
        app.update(cx, |app, cx| {
            app.store
                .set_block_text(&page, &block, "ab", None, None)
                .unwrap();
            assert!(app.store.has_pending());
            app.flush_now(cx);
            assert!(app.last_error.is_some(), "the failure is reported");
            assert_eq!(
                app.refs_version, 0,
                "no IndexUpdated for a commit that did not happen"
            );
        });
    }
}
