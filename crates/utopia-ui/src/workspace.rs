//! The main window (§9.2): left sidebar (journal, date picker, pages, storage status),
//! the main page, the right sidebar of stacked pages plus the reference inspector, quick
//! jump, and the new-day banner (FR-1.7).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use chrono::NaiveDate;
use gpui::{
    anchored, deferred, div, prelude::*, px, AnyElement, Context, Entity, EntityId, FocusHandle,
    Focusable, MouseButton, MouseDownEvent, SharedString, Subscription, Task, Window,
};
use utopia_core::inline::{extract_refs, plain_text};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{OutgoingRef, PageRow, RefTarget};

use crate::actions::*;
use crate::app_state::{AppState, IndexUpdated};
use crate::editor::{BlockEditor, EditorEvent};
use crate::graph_view::{GraphEvent, GraphView};
use crate::icons::{icon, Icon, ICON_SIZE};
use crate::page_view::{PageView, PageViewEvent};
use crate::pdf_view::{PdfEvent, PdfView};
use crate::quick_jump::{JumpPalette, PaletteMode, QuickJumpEvent};
use crate::sync_runner::{
    exchange_owned, status_line, sync_dot, BoxTransport, SyncDot, SyncPhase, SyncRunner,
};
use crate::theme::{
    ghost_icon, ghost_text, kbd, nav_row, quiet_control, quiet_danger, theme, BUTTON_HEIGHT, COUNT,
    FOOTER_DOT, LABEL_MONO, MENU_OFFSET, PAGE_MENU_WIDTH, PAGE_ROW_HEIGHT, RADIUS_CHIP,
    RADIUS_CONTROL, RADIUS_INPUT, RADIUS_TAG, SEARCH_HEIGHT, SIDEBAR_LABEL, SMALL, TAG, TAG_HEIGHT,
    TAG_PADDING_X, TOOLTIP_MAX_WIDTH, TOP_BAR_MIN_HEIGHT, TOP_BAR_PADDING_X, TOP_BAR_PADDING_Y, UI,
    VAULT_NAME_MAX_WIDTH, WORKSPACE_NAME, WORKSPACE_SQUARE,
};

/// The reference panel's sub-tabs: which way the edges are looked at (FR-4.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorTab {
    Incoming,
    Outgoing,
    Unresolved,
}

/// What the right sidebar's own tabs show (RS-3): the page's references, its headings, or
/// the pages pinned to stay reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanelTab {
    References,
    Outline,
    Pinned,
}

impl PanelTab {
    /// Spelled in a `debug_selector`, so tests can name the panel they are looking at.
    fn id(self) -> &'static str {
        match self {
            PanelTab::References => "references",
            PanelTab::Outline => "outline",
            PanelTab::Pinned => "pinned",
        }
    }
}

/// One heading of the page in the main pane, for the Outline tab (RS-3).
#[derive(Debug, Clone)]
struct OutlineEntry {
    block: BlockId,
    /// `BlockKind::Heading(level)`: 1–3.
    level: u8,
    depth: usize,
    text: String,
}

#[derive(Debug, Clone)]
enum JumpMode {
    Open,
    Redirect { block: BlockId, key: String },
    Search,
    Capture,
    Extract { page: PageId, block: BlockId },
}

/// A page stacked in the right sidebar (RS-5): its view, whether its card is folded to the
/// header, and the title the header shows.
struct SidePane {
    view: Entity<PageView>,
    collapsed: bool,
    title: SharedString,
}

#[derive(Default)]
struct SidebarData {
    /// Every live page, daily ones included: pins resolve against this (§5.5).
    all: Vec<PageRow>,
    /// The Pages list: `all` without daily pages and without tag pages (DESIGN.md §3.2:
    /// tags live in their own section).
    pages: Vec<PageRow>,
    /// Pages that exist only because of a `#tag` (FR-2.15), drawn as chips under TAGS.
    tag_rows: Vec<PageRow>,
    days: HashSet<NaiveDate>,
    /// How many live blocks reference each page (DESIGN.md §3.2), read in one grouped
    /// query per index update, never one query per row.
    ref_counts: HashMap<PageId, usize>,
}

#[derive(Default)]
struct InspectorData {
    /// Linked References, grouped by source page; each group carries its blocks (RS-1).
    incoming: Vec<utopia_store::RefGroup>,
    outgoing: Vec<OutgoingRef>,
    /// Unresolved targets with a display title recovered from the source text.
    unresolved: Vec<(OutgoingRef, String)>,
    /// The page's headings in document order (RS-3).
    outline: Vec<OutlineEntry>,
}

/// `local_state` key holding the pages opened last, newest first (QJ-1). Device-local UI
/// state like collapse, so it never syncs and never enters a shard (rule 9).
const RECENT_KEY: &str = "recent_pages";
/// How many ids are kept: Quick jump shows eight after today and yesterday, and pages
/// deleted since the list was written are skipped, so a few spares are kept.
const RECENT_KEEP: usize = 24;

/// The label an icon-only button can carry: gpui 0.2 has no accessibility tree, so a
/// tooltip is the nearest thing to the accessible name DESIGN.md §2.5 asks for. A real view
/// rather than a styled div because gpui tooltips are views.
struct Tip {
    text: SharedString,
}

impl Tip {
    fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        div()
            .debug_selector(|| "tip".into())
            .max_w(px(TOOLTIP_MAX_WIDTH))
            .px_2()
            .py_1()
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .text_size(px(crate::theme::CAPTION_SIZE))
            .text_color(th.ink)
            .child(self.text.clone())
    }
}

fn tip(text: &'static str) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView + 'static {
    move |_, cx| cx.new(|_| Tip::new(text)).into()
}

/// What the main column shows (P2): one thing at a time. Before this the full-screen graph
/// and the PDF viewer were two `Option`s, so `(Some, Some)` was representable and the graph
/// painted over a live PDF that silently reappeared when the graph closed
/// (`docs/state-machines/workspace-main-pane-content.md`). The page is always underneath
/// (`main`), so leaving an overlay is just going back to it. The sidebar's `graph_panel`
/// sits beside the page rather than replacing it, so it is not part of this state (GR-5).
enum MainContent {
    Page,
    Graph(Entity<GraphView>),
    Pdf(Entity<PdfView>),
}

pub struct Workspace {
    app: Entity<AppState>,
    editor: Entity<BlockEditor>,
    main: Entity<PageView>,
    /// What the main column shows in place of `main` (P2).
    content: MainContent,
    panes: Vec<SidePane>,
    splits: Vec<Entity<PageView>>,
    right_open: bool,
    /// The right sidebar's width in points (RS-4), kept in `settings.json` (rule 9).
    right_width: f32,
    /// A drag of the sidebar's left edge: where it started and the width then.
    resize_from: Option<(f32, f32)>,
    /// Left sidebar; local UI state (rule 9), kept in `local_state`.
    left_open: bool,
    inspector_open: bool,
    tab: InspectorTab,
    /// Which of the sidebar's own tabs is showing (RS-3).
    panel_tab: PanelTab,
    /// Pages pinned in the Pinned tab, resolved to titles, and the ids they came from (one
    /// list shared with the left sidebar's Pinned group, SB-3/RS-3).
    pinned: Vec<(PageId, String)>,
    pinned_ids: Vec<PageId>,
    /// The `refs_version` the resolved titles were read at, so an index update (a rename, a
    /// merge) re-resolves them even though the id list is unchanged.
    pinned_version: Option<u64>,
    jump: Option<(Entity<JumpPalette>, JumpMode)>,
    /// The local graph in the right sidebar, beside the page that stays editable (GR-5).
    graph_panel: Option<Entity<GraphView>>,
    /// The main pane's page when the sidebar was hidden, so a panel kept across the hide
    /// can tell whether the page moved while it was away and only then follow it. `Some(None)`
    /// is a hide with no page open; `None` means no hide to catch up from.
    hidden_main_page: Option<Option<PageId>>,
    sidebar: SidebarData,
    sidebar_seen: Option<u64>,
    /// The top bar's page menu is open (DESIGN.md §3.3). Local view state.
    page_menu: bool,
    inspector: InspectorData,
    inspector_seen: Option<(PageId, u64)>,
    /// Pages opened last, newest first, for the Quick jump empty state (QJ-1). Read from
    /// and written back to `local_state`, never to a shard (rule 9).
    recent: Vec<PageId>,
    month: NaiveDate,
    today: NaiveDate,
    new_day: bool,
    backup_dir: Option<PathBuf>,
    /// The graph is encrypted at rest (SB-4): read once at startup, never in `render`.
    encrypted: bool,
    /// The sync target as a line without secrets (SB-4), e.g. "Relay · http://…".
    sync_target: Option<String>,
    /// The footer's sync view is open (SB-4); it floats above the footer.
    sync_open: bool,
    /// The dot state the last painted frame showed. Checked by tests to prove that a state
    /// change actually repainted (debug bounds linger after an element stops being drawn).
    rendered_sync_dot: Option<SyncDot>,
    sync: Option<SyncRunner>,
    focus: FocusHandle,
    subs: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl crate::reference::RefRowHost for Workspace {
    /// A reference row was clicked: open its page with that block focused (§7). Shift
    /// stacks the page in this sidebar instead of replacing the main pane.
    fn open_reference(
        &mut self,
        page: PageId,
        block: BlockId,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(page, None, Some(block), shift, window, cx);
    }

    fn open_reference_page(
        &mut self,
        page: PageId,
        block: Option<BlockId>,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(page, None, block, shift, window, cx);
    }

    fn follow_reference(
        &mut self,
        target: crate::rich::LinkTarget,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow_link(target, shift, window, cx);
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(app: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| BlockEditor::new(app.clone(), cx));
        let main =
            cx.new(|cx| PageView::new(app.clone(), editor.clone(), false, false, window, cx));
        let today = app.read(cx).store.today();
        let backup_dir = app
            .read(cx)
            .store
            .path()
            .and_then(|p| p.parent().map(|d| d.join("backup")));
        let mut subs = vec![
            cx.subscribe_in(&main, window, Self::on_page_event),
            cx.subscribe_in(
                &editor,
                window,
                |this, editor, e: &EditorEvent, window, cx| {
                    // Escape, or the edited block disappearing, must not strand focus on an
                    // element that is no longer rendered (global shortcuts would stop working).
                    if matches!(e, EditorEvent::Blurred) && editor.read(cx).target().is_none() {
                        window.focus(&this.focus);
                    }
                },
            ),
            // §13.5: the input method can only reach the editor while its handle has
            // focus, and gpui does not tell the platform's input context when focus
            // moves. A composition left behind by a host that focuses elsewhere (the
            // window deactivating, a popup, a pane) would never get its commit, so take
            // it as committed text here.
            cx.on_focus_out(
                &editor.read(cx).focus_handle(cx),
                window,
                |this, _, _window, cx| {
                    this.editor.update(cx, |e, cx| e.focus_lost(cx));
                },
            ),
            cx.observe(&app, |_, _, cx| cx.notify()),
        ];
        let flag = |key: &str, default: bool| {
            app.read(cx)
                .store
                .local_get(key)
                .ok()
                .flatten()
                .map_or(default, |v| v == "1")
        };
        let left_open = flag("left_sidebar", true);
        let recent = app
            .read(cx)
            .store
            .local_get(RECENT_KEY)
            .ok()
            .flatten()
            .map(|v| {
                v.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .take(RECENT_KEEP)
                    .map(|l| PageId(l.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        // `is_encrypted` opens the file, so it belongs here and not in `render` (rule 10).
        let encrypted = app
            .read(cx)
            .store
            .path()
            .is_some_and(utopia_store::is_encrypted);
        {
            // DESIGN.md §3.3: the page menu is transient. An interceptor sees Escape before
            // the keymap, so closing the menu does not also end the block edit behind it.
            let handle = window.window_handle();
            let me = cx.entity().downgrade();
            subs.push(cx.intercept_keystrokes(move |ev, window, cx| {
                if window.window_handle() != handle || ev.keystroke.key != "escape" {
                    return;
                }
                let closed = me
                    .update(cx, |this, cx| {
                        if this.page_menu {
                            this.page_menu = false;
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if closed {
                    cx.stop_propagation();
                }
            }));
        }
        let mut me = Self {
            app,
            editor,
            main,
            content: MainContent::Page,
            panes: Vec::new(),
            splits: Vec::new(),
            right_open: false,
            right_width: crate::appearance::right_sidebar_width(cx),
            resize_from: None,
            left_open,
            inspector_open: true,
            tab: InspectorTab::Incoming,
            panel_tab: PanelTab::References,
            pinned: Vec::new(),
            pinned_ids: Vec::new(),
            pinned_version: None,
            jump: None,
            graph_panel: None,
            hidden_main_page: None,
            sidebar: SidebarData::default(),
            sidebar_seen: None,
            page_menu: false,
            inspector: InspectorData::default(),
            inspector_seen: None,
            recent,
            month: today,
            today,
            new_day: false,
            backup_dir,
            encrypted,
            sync_target: None,
            sync_open: false,
            rendered_sync_dot: None,
            sync: None,
            focus: cx.focus_handle(),
            subs,
            _tasks: Vec::new(),
        };
        // GR-5: the sidebar graph follows the page open beside it, so "around this page"
        // keeps meaning the page you are reading or editing. A panel hidden with the sidebar
        // is not drawn, so it does no work; `show_right` brings it up to date when the
        // sidebar returns (workspace-main-pane-content.md, issue 3).
        let main = me.main.clone();
        me.subs.push(cx.observe(&main, |this, main, cx| {
            if !this.right_open {
                return;
            }
            let page = main.read(cx).page();
            if let Some(panel) = this.graph_panel.clone() {
                panel.update(cx, |graph, cx| graph.set_origin(page, cx));
            }
        }));
        // GR-5: the panel is an index projection. In debounced mode links arrive after the
        // keystroke, so reload when the writer has caught up, not only when the page changes.
        let app = me.app.clone();
        me.subs
            .push(cx.subscribe(&app, |this, _, _: &IndexUpdated, cx| {
                if !this.right_open {
                    return;
                }
                if let Some(panel) = this.graph_panel.clone() {
                    panel.update(cx, |graph, cx| graph.refresh(cx));
                }
            }));
        me.spawn_timers(cx);
        me
    }

    fn spawn_timers(&mut self, cx: &mut Context<Self>) {
        // FR-1.7: today recomputes on rollover and time zone change; never auto-navigate.
        self._tasks.push(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(30))
                .await;
            let alive = this.update(cx, |this, cx| {
                let today = this.app.read(cx).store.today();
                if today != this.today {
                    this.today = today;
                    this.new_day = true;
                    this.sidebar_seen = None;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        }));
        // §15.3: rolling snapshots every 5 minutes, 6 generations.
        self._tasks.push(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_secs(300))
                .await;
            let alive = this.update(cx, |this, cx| {
                if let Some(dir) = this.backup_dir.clone() {
                    this.app.update(cx, |app, _| {
                        let r = app.store.backup(&dir, 6);
                        app.report(r);
                    });
                }
            });
            if alive.is_err() {
                break;
            }
        }));
    }

    /// Start background sync for `cfg`: relay or folder, optionally end-to-end encrypted,
    /// with realtime notices from a relay (§12). Local editing never waits on it. Returns
    /// whether a runner was actually started; [Self::start_sync] is what the app uses, so
    /// Settings → Sync only claims a sync that exists.
    pub fn enable_sync(
        &mut self,
        cfg: crate::sync_runner::SyncConfig,
        cx: &mut Context<Self>,
    ) -> bool {
        drop(self.stop_sync(cx));
        // Say where sync goes without the token or passphrase (never shown, §15.6); the
        // sidebar names the target even when the transport below cannot be built.
        self.sync_target = Some(match &cfg.target {
            crate::sync_runner::SyncTarget::Relay { url, .. } => format!("Relay · {url}"),
            crate::sync_runner::SyncTarget::Folder(dir) => format!("Folder · {}", dir.display()),
        });
        let client = self.app.update(cx, |app, _| {
            utopia_sync::SyncEngine::new(&app.store).map(|e| e.client_id().to_string())
        });
        let transport = client.and_then(|c| crate::sync_runner::build_transport(&cfg, &c));
        match transport {
            Ok(t) => {
                let nudge = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let started = self.enable_sync_with(t, Some(nudge.clone()), cx);
                if started {
                    if let crate::sync_runner::SyncTarget::Relay { url, token } = &cfg.target {
                        if let Some(runner) = &mut self.sync {
                            runner.realtime = utopia_sync::realtime::Listener::start(
                                url.clone(),
                                cfg.graph.clone(),
                                token.clone(),
                                nudge,
                            )
                            .ok();
                        }
                    }
                }
                started
            }
            Err(e) => {
                eprintln!("utopia: sync disabled: {e}");
                self.app.update(cx, |app, _| {
                    app.last_error = Some(format!("sync disabled: {e}"))
                });
                false
            }
        }
    }

    /// Stop polling immediately; the returned listener is joined off the UI thread.
    pub fn stop_sync(&mut self, cx: &mut Context<Self>) -> Option<utopia_sync::realtime::Listener> {
        let listener = self
            .sync
            .take()
            .and_then(|mut runner| runner.realtime.take());
        self.sync_target = None;
        let mut facts = crate::sync_runner::facts(cx);
        facts.target = None;
        facts.encrypted = false;
        cx.set_global(facts);
        cx.notify();
        listener
    }

    /// Install a transport prepared off the UI thread (key derivation and folder I/O).
    pub fn start_prepared_sync(
        &mut self,
        cfg: &crate::sync_runner::SyncConfig,
        transport: BoxTransport,
        cx: &mut Context<Self>,
    ) -> bool {
        drop(self.stop_sync(cx));
        let facts = crate::sync_runner::SyncFacts::from_config(cfg);
        self.sync_target = facts.target.clone();
        let started = self.enable_sync_with(transport, None, cx);
        if started {
            if let (Some(runner), crate::sync_runner::SyncTarget::Relay { url, token }) =
                (&mut self.sync, &cfg.target)
            {
                runner.realtime = utopia_sync::realtime::Listener::start(
                    url.clone(),
                    cfg.graph.clone(),
                    token.clone(),
                    runner.nudge.clone(),
                )
                .ok();
            }
            if let Some(runner) = &self.sync {
                runner
                    .nudge
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            cx.set_global(facts);
        }
        cx.notify();
        started
    }

    /// Start sync for `cfg` and publish what Settings → Sync should say (ST-5). The enabled
    /// facts go out only once a runner exists: a transport that cannot be built, or a LAN
    /// discovery that finds no relay, leaves the pane saying sync is off rather than
    /// promising a sync that never started. Returns whether sync is now running.
    pub fn start_sync(
        &mut self,
        cfg: crate::sync_runner::SyncConfig,
        cx: &mut Context<Self>,
    ) -> bool {
        let facts = crate::sync_runner::SyncFacts::from_config(&cfg);
        let started = self.enable_sync(cfg, cx);
        cx.set_global(if started {
            facts
        } else {
            crate::sync_runner::SyncFacts {
                target: None,
                graph: facts.graph,
                encrypted: false,
            }
        });
        started
    }

    /// Start background sync over an already-built transport (tests, custom transports).
    /// Returns whether the runner started.
    pub fn enable_sync_with(
        &mut self,
        transport: BoxTransport,
        nudge: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let engine = self
            .app
            .update(cx, |app, _| utopia_sync::SyncEngine::new(&app.store));
        let engine = match engine {
            Ok(e) => e,
            Err(e) => {
                eprintln!("utopia: sync disabled: {e}");
                return false;
            }
        };
        let mut runner = SyncRunner::new(engine, transport);
        if let Some(n) = nudge {
            runner.nudge = n;
        }
        let generation = runner.generation;
        // Installing the runner drops the previous one, and with it the previous loop's
        // task: one runner, one loop. The generation below is the second guard, for a loop
        // that is already past an await when this new runner arrives.
        self.sync = Some(runner);
        let task = cx.spawn(async move |this, cx| loop {
            let wait = this.update(cx, |this, _| {
                this.sync
                    .as_ref()
                    .filter(|s| s.is_generation(generation))
                    .map(|s| {
                        // §12.6: a refused relay gets the slow cadence and no notice-driven
                        // rounds, but it is retried, so a transient refusal heals.
                        let refused = matches!(s.phase, SyncPhase::Refused { .. });
                        (s.interval, s.nudge.clone(), refused)
                    })
            });
            let Ok(Some((interval, nudge, refused))) = wait else {
                break;
            };
            // Poll interval, cut short by a realtime notice — except while refused, where a
            // notice must not bring the fast polling back.
            let mut waited = Duration::ZERO;
            while waited < interval {
                let step = Duration::from_millis(250).min(interval - waited);
                cx.background_executor().timer(step).await;
                waited += step;
                if !refused && nudge.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
            }
            // 1. Gather pushes on the UI thread (short store reads).
            let prepared = this.update(cx, |this, cx| {
                let Workspace { app, sync, .. } = this;
                let runner = sync.as_mut().filter(|r| r.is_generation(generation))?;
                let transport = runner.begin_round()?;
                // The footer's dot says "syncing" from now until the exchange returns; ask
                // for the frame now, or an idle window keeps last round's green (SB-4).
                cx.notify();
                let out = app.update(cx, |app, _| runner.engine.prepare(&mut app.store));
                match out {
                    Ok(out) => {
                        runner.exchange();
                        Some((out, transport))
                    }
                    Err(e) => {
                        let status =
                            utopia_sync::SyncStatus::from_result(&Err(e), &app.read(cx).store);
                        runner.round_failed(transport, status);
                        None
                    }
                }
            });
            let Ok(prepared) = prepared else { break };
            let Some((out, transport)) = prepared else {
                continue;
            };
            // 2. Network on a background thread.
            let (result, transport) = cx
                .background_executor()
                .spawn(async move { exchange_owned(transport, out) })
                .await;
            // 3. Import on the UI thread and announce merges.
            let applied = this.update(cx, |this, cx| {
                let Workspace { app, sync, .. } = this;
                let Some(runner) = sync.as_mut().filter(|r| r.is_generation(generation)) else {
                    return;
                };
                runner.exchange_returned();
                let report = match result {
                    Ok(inc) => app.update(cx, |app, cx| {
                        let r = runner.engine.apply(&mut app.store, inc);
                        if let Ok(rep) = &r {
                            if !rep.imported.is_empty() {
                                app.changed(rep.imported.clone(), true, cx);
                            }
                        }
                        r
                    }),
                    Err(e) => Err(e),
                };
                if let Ok(rep) = &report {
                    if !rep.imported.is_empty() {
                        let now = app.read(cx).store.now();
                        let n = rep.imported.len();
                        let pages = if n == 1 { "page" } else { "pages" };
                        runner.notice = Some((
                            format!("Merged changes from the relay into {n} {pages}"),
                            now,
                        ));
                    }
                }
                let status = utopia_sync::SyncStatus::from_result(&report, &app.read(cx).store);
                runner.end_round(transport, &report, status);
                cx.notify();
            });
            if applied.is_err() {
                break;
            }
        });
        if let Some(runner) = self.sync.as_mut() {
            if runner.is_generation(generation) {
                runner.loop_task = Some(task);
            }
        }
        cx.notify();
        true
    }

    /// Split panes (P1): an editable page beside the main one (Cmd+Shift+Click). At most
    /// two splits; the oldest closes first.
    pub fn open_split(&mut self, page: PageId, window: &mut Window, cx: &mut Context<Self>) {
        // Cmd+Shift+Click opens a split without going through `open`, and it is still a page
        // opened: the Quick jump empty state should offer it next time (QJ-1).
        self.page_menu = false;
        self.note_opened(&page, cx);
        let v = cx.new(|cx| {
            PageView::new(
                self.app.clone(),
                self.editor.clone(),
                true,
                true,
                window,
                cx,
            )
        });
        self.subs
            .push(cx.subscribe_in(&v, window, Self::on_page_event));
        v.update(cx, |v, cx| v.open(page, None, cx));
        if self.splits.len() >= 2 {
            self.splits.remove(0);
        }
        self.splits.push(v);
        cx.notify();
    }

    /// Current sync status, if sync is enabled.
    pub fn sync_status(&self) -> Option<utopia_sync::SyncStatus> {
        self.sync.as_ref().and_then(|s| s.last.clone())
    }

    /// The round's phase, if sync is enabled: which half of a round is running and where the
    /// transport is. The dot is derived from the same field (`sync_dot`), but the dot alone
    /// cannot tell "connecting" from "an exchange in flight" (both are amber); tests that
    /// need that distinction read the phase.
    pub fn sync_phase(&self) -> Option<&SyncPhase> {
        self.sync.as_ref().map(|s| &s.phase)
    }

    /// The shared block editor (startup/latency probe).
    pub fn editor(&self) -> Entity<BlockEditor> {
        self.editor.clone()
    }

    /// The main pane's view, for debug-only probes (query counts).
    #[cfg(debug_assertions)]
    pub fn main_view(&self) -> Entity<PageView> {
        self.main.clone()
    }

    /// The page shown in the main pane.
    pub fn main_page(&self, cx: &gpui::App) -> Option<PageId> {
        self.main.read(cx).page()
    }

    /// The block the main pane is zoomed into, if any.
    pub fn main_zoom(&self, cx: &gpui::App) -> Option<BlockId> {
        self.main.read(cx).zoom()
    }

    /// Pages open in split panes beside the main one.
    pub fn split_pages(&self, cx: &gpui::App) -> Vec<PageId> {
        self.splits
            .iter()
            .filter_map(|p| p.read(cx).page())
            .collect()
    }

    /// The title a sidebar card shows for `page` (RS-5): the display title, from the shard,
    /// so a draft day and a renamed page both read right.
    fn pane_title(&self, page: &PageId, cx: &mut Context<Self>) -> SharedString {
        self.app
            .update(cx, |app, _| {
                let fmt = app.store.date_format().clone();
                app.store
                    .page_state(page)
                    .map(|st| utopia_store::display_title(st, &fmt))
                    .unwrap_or_default()
            })
            .into()
    }

    /// Pages pinned in the sidebar's Pinned tab, in the order they show (RS-3).
    pub fn pinned_pages(&self, _cx: &gpui::App) -> Vec<PageId> {
        self.pinned.iter().map(|(p, _)| p.clone()).collect()
    }

    /// The titles the Pinned tab shows, in the order they show (RS-3).
    pub fn pinned_titles(&self) -> Vec<String> {
        self.pinned.iter().map(|(_, title)| title.clone()).collect()
    }

    /// Pages stacked in the right sidebar, top first.
    pub fn sidebar_pages(&self, cx: &gpui::App) -> Vec<PageId> {
        self.panes
            .iter()
            .filter_map(|p| p.view.read(cx).page())
            .collect()
    }

    /// Open the configured launch page (FR-1.1): today by default, or the last page.
    pub fn open_launch_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (mode, last) = {
            let s = &self.app.read(cx).store;
            (
                s.local_get("launch")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "today".into()),
                s.local_get("last_page").ok().flatten(),
            )
        };
        if mode == "last" {
            if let Some(p) = last.map(PageId) {
                if self
                    .app
                    .read(cx)
                    .store
                    .page_row(&p)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.is_live())
                {
                    self.open(p, None, None, false, window, cx);
                    return;
                }
            }
        }
        self.open_journal(window, cx);
    }

    /// Remember that `page` was opened, for the Quick jump empty state (QJ-1). Local UI
    /// state in `local_state` (rule 9): a page opened on another device is not reordered
    /// here, and a write failure costs the ordering, never the page.
    fn note_opened(&mut self, page: &PageId, cx: &mut Context<Self>) {
        if self.recent.first() == Some(page) {
            return;
        }
        self.recent.retain(|p| p != page);
        self.recent.insert(0, page.clone());
        self.recent.truncate(RECENT_KEEP);
        let joined = self
            .recent
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if let Err(e) = self.app.read(cx).store.local_set(RECENT_KEY, &joined) {
            eprintln!("utopia: {e}");
        }
    }

    fn on_page_event(
        &mut self,
        view: &Entity<PageView>,
        e: &PageViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match e {
            PageViewEvent::Open {
                page,
                zoom,
                focus,
                sidebar,
            } => self.open(
                page.clone(),
                zoom.clone(),
                focus.clone(),
                *sidebar,
                window,
                cx,
            ),
            PageViewEvent::Close => {
                self.panes.retain(|p| &p.view != view);
                self.splits.retain(|p| p != view);
                cx.notify();
            }
            PageViewEvent::OpenGraph { page } => {
                self.open_graph(Some(page.clone()), window, cx);
            }
            PageViewEvent::OpenSplit { page } => self.open_split(page.clone(), window, cx),
            PageViewEvent::OpenPdf {
                hash,
                name,
                page,
                host,
            } => self.open_pdf(hash.clone(), name.clone(), *page, host.clone(), window, cx),
            PageViewEvent::RequestExtract { page, block } => {
                self.start_jump(
                    JumpMode::Extract {
                        page: page.clone(),
                        block: block.clone(),
                    },
                    window,
                    cx,
                );
            }
            PageViewEvent::TogglePin { page } => self.toggle_pin(page.clone(), cx),
        }
    }

    pub fn open(
        &mut self,
        page: PageId,
        zoom: Option<BlockId>,
        focus: Option<BlockId>,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.page_menu = false;
        self.note_opened(&page, cx);
        if sidebar {
            // Shift-click stacks the page beside the current one (§0, W6).
            let existing = self.panes.iter().position(|p| {
                p.view.read(cx).page().as_ref() == Some(&page) && p.view.read(cx).zoom() == zoom
            });
            let pane = match existing {
                // Re-opening moves the card back to the top but keeps it folded if it was.
                Some(i) => self.panes.remove(i),
                None => {
                    let v = cx.new(|cx| {
                        PageView::new(
                            self.app.clone(),
                            self.editor.clone(),
                            true,
                            false,
                            window,
                            cx,
                        )
                    });
                    self.subs
                        .push(cx.subscribe_in(&v, window, Self::on_page_event));
                    v.update(cx, |v, cx| v.open(page.clone(), zoom.clone(), cx));
                    SidePane {
                        view: v,
                        collapsed: false,
                        title: self.pane_title(&page, cx),
                    }
                }
            };
            self.panes.insert(0, pane);
            self.show_right(cx);
        } else {
            // A page opened in the main column replaces whatever it was showing: a PDF is
            // closed and a full-screen graph gives way, so the page is never opened
            // invisibly behind the graph (workspace-main-pane-content.md, issues 1 and 2).
            self.content = MainContent::Page;
            self.main
                .update(cx, |m, cx| m.open(page.clone(), zoom.clone(), cx));
            let _ = self
                .app
                .read(cx)
                .store
                .local_set("last_page", page.as_str());
            // From the shard: a draft day has no index row yet.
            let title = self.app.update(cx, |app, _| {
                let fmt = app.store.date_format().clone();
                app.store
                    .page_state(&page)
                    .map(|st| utopia_store::display_title(st, &fmt))
                    .unwrap_or_default()
            });
            window.set_window_title(&format!("{title} — Utopia"));
            match focus {
                // A reference row or a link can point at a block that is folded away, so
                // reveal it rather than quietly failing to place the caret (RS-1, RS-3).
                Some(b) => self.main.update(cx, |m, cx| m.reveal_block(&b, window, cx)),
                None => window.focus(&self.focus),
            }
        }
        cx.notify();
    }

    // ---------------------------------------------------------------- actions

    fn quick_jump(&mut self, _: &QuickJump, window: &mut Window, cx: &mut Context<Self>) {
        self.start_jump(JumpMode::Open, window, cx);
    }

    fn start_jump(&mut self, mode: JumpMode, window: &mut Window, cx: &mut Context<Self>) {
        let (title, palette) = match mode {
            JumpMode::Open => ("Quick jump", PaletteMode::Pages),
            JumpMode::Redirect { .. } => ("Redirect link to…", PaletteMode::Pages),
            JumpMode::Search => ("Search", PaletteMode::Search),
            JumpMode::Capture => ("Quick capture → today", PaletteMode::Capture),
            JumpMode::Extract { .. } => ("Extract block to page", PaletteMode::Text),
        };
        // QJ-1: only "go to page" has an empty state worth having — today, yesterday and
        // the pages opened last. Redirecting a link or naming an extracted page has
        // nothing to do with where you have been.
        let recent = match mode {
            JumpMode::Open => Some(self.recent.clone()),
            _ => None,
        };
        let app = self.app.clone();
        let editor = self.editor.clone();
        let jump =
            cx.new(|cx| JumpPalette::with_mode(app, editor, title, palette, recent, window, cx));
        self.subs.push(cx.subscribe_in(
            &jump,
            window,
            |this, _, e: &QuickJumpEvent, window, cx| {
                let mode = this.jump.take().map(|(_, m)| m);
                match (e, mode) {
                    (
                        QuickJumpEvent::Chosen {
                            page,
                            sidebar,
                            focus,
                        },
                        Some(JumpMode::Open | JumpMode::Search),
                    ) => this.open(page.clone(), None, focus.clone(), *sidebar, window, cx),
                    (
                        QuickJumpEvent::Chosen { page, .. },
                        Some(JumpMode::Redirect { block, key }),
                    ) => {
                        this.app.update(cx, |app, cx| {
                            let r = app.store.redirect_intent(&block, &key, page);
                            app.report(r);
                            app.changed(Vec::new(), true, cx);
                        });
                    }
                    (QuickJumpEvent::ChosenDay { date, sidebar }, Some(JumpMode::Open)) => {
                        this.open_date(*date, *sidebar, !*sidebar, window, cx)
                    }
                    // QJ-5: run the command exactly as its keystroke would, once the
                    // palette is gone. Focus goes back where it was before the palette
                    // opened: an editor command (`Indent`, `Bold`, …) is handled by the
                    // block editor, and gpui dispatches through the focused element's
                    // ancestors only, so the workspace would never reach it.
                    (QuickJumpEvent::RunCommand(name), _) => {
                        if this.editor.read(cx).target().is_some() {
                            let handle = this.editor.read(cx).focus_handle(cx);
                            window.focus(&handle);
                        } else {
                            window.focus(&this.focus);
                        }
                        match action_for(name) {
                            Some(action) => window.dispatch_action(action, cx),
                            None => eprintln!("utopia: no action named {name}"),
                        }
                    }
                    (QuickJumpEvent::Text(text), Some(JumpMode::Capture)) => {
                        this.app.update(cx, |app, cx| {
                            let r = app.store.capture(text);
                            if let Some((page, _)) = app.report(r) {
                                app.changed(vec![page], true, cx);
                            }
                        });
                    }
                    (QuickJumpEvent::Text(text), Some(JumpMode::Extract { page, block })) => {
                        let dst = this.app.update(cx, |app, cx| {
                            let r = app.store.extract_block_to_page(&page, &block, text);
                            let d = app.report(r);
                            app.changed(vec![page.clone()], true, cx);
                            d
                        });
                        if let Some(dst) = dst {
                            this.open(dst, None, None, true, window, cx);
                        }
                    }
                    _ => {}
                }
                if this.editor.read(cx).target().is_none() {
                    window.focus(&this.focus);
                }
                cx.notify();
            },
        ));
        self.jump = Some((jump, mode));
        cx.notify();
    }

    fn full_search(&mut self, _: &FullSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.start_jump(JumpMode::Search, window, cx);
    }

    /// Cmd+Shift+G: show or hide the full-screen graph (P2).
    pub fn show_graph(&mut self, _: &ShowGraph, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.content, MainContent::Graph(_)) {
            self.content = MainContent::Page;
            window.focus(&self.focus);
            cx.notify();
            return;
        }
        self.open_graph_full(window, cx);
    }

    fn open_graph_full(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let origin = self.main.read(cx).page();
        self.open_graph(origin, window, cx);
    }

    /// Show the graph view with `origin` (the page the user came from) highlighted (GR-1).
    /// Cmd+Shift+G passes the main pane's page; the page menu's "Open in graph" (PG-2)
    /// passes the page whose menu it was.
    fn open_graph(&mut self, origin: Option<PageId>, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.graph_panel = None;
        let view = cx.new(|cx| GraphView::new(self.app.clone(), origin, false, cx));
        self.subs
            .push(cx.subscribe_in(&view, window, Self::on_graph_event));
        window.focus(&view.focus_handle(cx));
        self.content = MainContent::Graph(view);
        cx.notify();
    }

    /// GR-5: "Around this page" in the right sidebar. The main page stays editable. The
    /// origin is the graph's own: the control can be pressed on a graph opened from a
    /// sidebar or split page, and switching to the main pane's page would silently show a
    /// different neighbourhood than the one it names.
    fn open_graph_panel(
        &mut self,
        origin: Option<PageId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin = origin.or_else(|| self.main.read(cx).page());
        let Some(origin) = origin else {
            return;
        };
        self.editor.update(cx, |e, cx| e.end(cx));
        let view = cx.new(|cx| GraphView::new(self.app.clone(), Some(origin), true, cx));
        self.subs
            .push(cx.subscribe_in(&view, window, Self::on_graph_event));
        self.show_right(cx);
        self.graph_panel = Some(view);
        window.focus(&self.focus);
        cx.notify();
    }

    fn on_graph_event(
        &mut self,
        view: &Entity<GraphView>,
        e: &GraphEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from_panel = self.graph_panel.as_ref() == Some(view);
        // Only the graph that is actually on screen may change what is on screen: a stale
        // view's event must not dismiss a PDF or a newer graph (issue 2).
        let showing = matches!(&self.content, MainContent::Graph(g) if g == view);
        // AGENTS.md focus invariant: a removed element gets no keystrokes. If this graph
        // owned focus, hand it back; otherwise leave an editor's focus untouched.
        let had_focus = view.read(cx).focus_handle(cx).is_focused(window);
        match e {
            GraphEvent::Open {
                page,
                sidebar,
                split,
            } => {
                if !from_panel && showing {
                    self.content = MainContent::Page;
                }
                if *split {
                    self.open_split(page.clone(), window, cx);
                    if had_focus {
                        window.focus(&self.focus);
                    }
                } else {
                    self.open(page.clone(), None, None, *sidebar, window, cx);
                }
            }
            GraphEvent::OpenPanel => {
                let origin = view.read(cx).origin();
                if showing {
                    self.content = MainContent::Page;
                }
                self.open_graph_panel(origin, window, cx);
            }
            GraphEvent::Expand => {
                self.graph_panel = None;
                self.open_graph_full(window, cx);
            }
            GraphEvent::Close => {
                if showing {
                    self.content = MainContent::Page;
                }
                self.graph_panel = None;
                if had_focus {
                    window.focus(&self.focus);
                }
            }
        }
        cx.notify();
    }

    /// Open a PDF attachment in the viewer, optionally at a page (0-based).
    pub fn open_pdf(
        &mut self,
        hash: String,
        name: String,
        page: Option<usize>,
        host: Option<BlockId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        let view = cx.new(|cx| PdfView::new(self.app.clone(), hash, name, page, host, cx));
        self.subs.push(
            cx.subscribe_in(&view, window, |this, pdf, e: &PdfEvent, window, cx| {
                match e {
                    PdfEvent::Close => {
                        // Only the viewer on screen owns the main column; one that was
                        // replaced must not take focus back (issue 2).
                        if matches!(&this.content, MainContent::Pdf(v) if v == pdf) {
                            this.content = MainContent::Page;
                            window.focus(&this.focus);
                        }
                    }
                    // The PDF stays open; the highlight's block opens beside it.
                    PdfEvent::OpenBlock { page, block } => {
                        this.open(page.clone(), Some(block.clone()), None, true, window, cx)
                    }
                }
                cx.notify();
            }),
        );
        window.focus(&view.focus_handle(cx));
        self.content = MainContent::Pdf(view);
        cx.notify();
    }

    pub fn pdf_view(&self) -> Option<Entity<PdfView>> {
        match &self.content {
            MainContent::Pdf(pdf) => Some(pdf.clone()),
            MainContent::Page | MainContent::Graph(_) => None,
        }
    }

    /// Files dropped on the window become attachments: at the caret when editing, else a
    /// new block at the end of the open page.
    pub fn drop_files(
        &mut self,
        paths: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .editor
            .update(cx, |e, cx| e.attach_paths(paths, window, cx))
        {
            return;
        }
        let Some(page) = self.main.read(cx).page() else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let markup: Vec<String> = paths
                .iter()
                .filter_map(|p| {
                    let r = app.store.attach_file(p);
                    app.report(r)
                })
                .map(|a| utopia_core::attach::markup(&a.name, &a.hash, &a.mime))
                .collect();
            if markup.is_empty() {
                return;
            }
            let r = (|| {
                let st = app.store.page_state(&page)?;
                let roots = st.live_roots();
                let reuse = match roots.as_slice() {
                    [only] => st.block(only).is_some_and(|b| b.text.is_empty()),
                    _ => false,
                };
                let block = if reuse {
                    roots[0].clone()
                } else {
                    let plan = utopia_core::outline::append_block(st, None);
                    match app.store.execute_plan(&page, plan, None)? {
                        Some(f) => f.block,
                        None => return Ok(()),
                    }
                };
                app.store
                    .set_block_text(&page, &block, &markup.join(" "), None, None)?;
                app.store.commit_block(&page, &block).map(|_| ())
            })();
            app.report(r);
            app.changed(vec![page.clone()], true, cx);
        });
    }

    pub fn graph_view(&self) -> Option<Entity<GraphView>> {
        match &self.content {
            MainContent::Graph(graph) => Some(graph.clone()),
            MainContent::Page | MainContent::Pdf(_) => None,
        }
    }

    /// The local graph panel in the right sidebar (GR-5).
    pub fn graph_panel(&self) -> Option<Entity<GraphView>> {
        self.graph_panel.clone()
    }

    /// The open palette, for tests that read the results it is showing.
    pub fn jump_palette(&self) -> Option<Entity<JumpPalette>> {
        self.jump.as_ref().map(|(j, _)| j.clone())
    }

    /// Full-text search with `text` already typed (`utopia://search`).
    pub fn search_for(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.full_search(&FullSearch, window, cx);
        if let Some((jump, _)) = &self.jump {
            let input = jump.read(cx).input.clone();
            input.update(cx, |i, cx| i.set_text(text, cx));
        }
    }

    /// Open the page a title names (dates and aliases resolve like links), creating it if
    /// it does not exist (`utopia://page`).
    pub fn open_title(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.app.update(cx, |app, cx| {
            let s = &mut app.store;
            let found =
                utopia_store::resolve_page_key(s.conn(), &normalize_title(title), s.date_format());
            let r = match found {
                Ok(Some(p)) => Ok(p),
                Ok(None) => {
                    let r = s.create_page(title);
                    app.changed(Vec::new(), true, cx);
                    r
                }
                Err(e) => Err(e),
            };
            app.report(r)
        });
        if let Some(page) = page {
            self.open(page, None, None, false, window, cx);
        }
    }

    pub fn app(&self) -> &Entity<AppState> {
        &self.app
    }

    /// Append a line to today's page (`utopia://capture`).
    pub fn capture_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let r = app.store.capture(text);
            if let Some((page, _)) = app.report(r) {
                app.changed(vec![page], true, cx);
            }
        });
    }

    /// Open the page holding a block, zoomed into it (`utopia://block`).
    pub fn open_block(&mut self, uid: &str, window: &mut Window, cx: &mut Context<Self>) {
        let block = BlockId(uid.to_string());
        let page = self.app.update(cx, |app, _| {
            let r = app.store.block_row(&block);
            app.report(r).flatten().map(|b| b.page_id)
        });
        if let Some(page) = page {
            self.open(page, Some(block), None, false, window, cx);
        }
    }

    /// Quick capture (FR-2.19): from anywhere in the app, and from the system-wide hotkey.
    pub fn quick_capture(&mut self, _: &QuickCapture, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.start_jump(JumpMode::Capture, window, cx);
    }

    /// Daily Note (§9.1): the whole feed, which is also the launch view.
    fn go_daily_notes(&mut self, _: &GoDailyNotes, window: &mut Window, cx: &mut Context<Self>) {
        self.open_journal(window, cx);
    }

    /// Today (§9.1): today's page on its own, not the feed. A day stays a draft until it is
    /// written (FR-1.2), so this opens the draft rather than creating the page.
    fn go_today(&mut self, _: &GoToday, window: &mut Window, cx: &mut Context<Self>) {
        let today = self.app.read(cx).store.today();
        self.today = today;
        self.open_date(today, false, true, window, cx);
    }

    /// Yesterday (§9.1). `pred_opt` only returns `None` at the start of the proleptic
    /// calendar, where staying put is the sane answer.
    fn go_yesterday(&mut self, _: &GoYesterday, window: &mut Window, cx: &mut Context<Self>) {
        let today = self.app.read(cx).store.today();
        self.today = today;
        let d = today.pred_opt().unwrap_or(today);
        self.open_date(d, false, true, window, cx);
    }

    /// The Daily Notes feed in the main pane, caret in today's first block (FR-1.1).
    /// Today is a draft until it is written (FR-1.2); older days only if they exist.
    pub fn open_journal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_day = false;
        self.page_menu = false;
        let today = self.app.read(cx).store.today();
        self.today = today;
        let page = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(today);
            let p = app.report(r);
            // A visit writes nothing (a draft, or a page already there): no structural
            // change, so no synchronous index flush (FR-1.2).
            app.changed(Vec::new(), false, cx);
            p
        });
        let Some(page) = page else { return };
        self.editor.update(cx, |e, cx| e.end(cx));
        self.content = MainContent::Page;
        self.main
            .update(cx, |m, cx| m.open_journal(page.clone(), cx));
        let _ = self
            .app
            .read(cx)
            .store
            .local_set("last_page", page.as_str());
        window.set_window_title("Daily Notes — Utopia");
        self.main.update(cx, |m, cx| m.focus_first(window, cx));
        cx.notify();
    }

    /// Days shown in the main pane's Daily Notes feed, newest first (empty otherwise).
    pub fn journal_days(&self, cx: &gpui::App) -> Vec<PageId> {
        self.main.read(cx).journal_days()
    }

    /// The oldest calendar day the feed has loaded (FR-1.2); `None` outside the feed.
    pub fn journal_shown_through(&self, cx: &gpui::App) -> Option<NaiveDate> {
        self.main.read(cx).journal_shown_through()
    }

    /// The date whose heading is pinned at the top of the feed (DN-3), for scripts.
    pub fn main_sticky_day(&self, cx: &gpui::App) -> Option<NaiveDate> {
        self.main.read(cx).sticky_day()
    }

    /// How many days the feed shows open, with a date heading (DN-1), for scripts.
    pub fn open_journal_days(&self, cx: &gpui::App) -> usize {
        self.main.read(cx).open_day_count()
    }

    /// Empty-day rows in the feed as separate list items (DESIGN.md §3.6), for scripts:
    /// one item per day keeps the list virtualizing a long run of blank days.
    pub fn main_empty_day_rows(&self, cx: &gpui::App) -> usize {
        self.main.read(cx).empty_day_rows()
    }

    pub fn open_date(
        &mut self,
        d: NaiveDate,
        sidebar: bool,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page_menu = false;
        let page = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(d);
            let p = app.report(r);
            // A visit writes nothing (FR-1.2): the caret's own block, if one has to be
            // created, reports its write from `focus_first`.
            app.changed(Vec::new(), false, cx);
            p
        });
        if let Some(page) = page {
            self.open(page, None, None, sidebar, window, cx);
            if focus && !sidebar {
                self.main.update(cx, |m, cx| m.focus_first(window, cx));
            }
        }
    }

    fn current_date(&self, cx: &mut Context<Self>) -> NaiveDate {
        let page = self.main.read(cx).page();
        page.and_then(|p| {
            self.app
                .update(cx, |app, _| app.store.page_state(&p).ok()?.meta.daily_date)
        })
        .unwrap_or(self.today)
    }

    fn prev_day(&mut self, _: &PrevDay, window: &mut Window, cx: &mut Context<Self>) {
        let d = self.current_date(cx);
        self.open_date(d.pred_opt().unwrap_or(d), false, false, window, cx);
    }

    fn next_day(&mut self, _: &NextDay, window: &mut Window, cx: &mut Context<Self>) {
        let d = self.current_date(cx);
        self.open_date(d.succ_opt().unwrap_or(d), false, false, window, cx);
    }

    fn toggle_right(
        &mut self,
        _: &ToggleRightSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.right_open {
            self.hide_right(window, cx);
        } else {
            self.show_right(cx);
            cx.notify();
        }
    }

    /// Show the sidebar (RS-4). A graph panel kept from last time did no work while the
    /// sidebar was hidden (it was not drawn), so it catches up here instead: it follows the
    /// main pane only if the page actually moved while it was away, and it re-reads the
    /// index. A panel opened around a sidebar or split page's graph keeps that origin, and
    /// its pan and zoom, when the sidebar is merely hidden and shown again — that is not
    /// navigation (workspace-main-pane-content.md, issue 3).
    fn show_right(&mut self, cx: &mut Context<Self>) {
        let was_hidden_with = self.hidden_main_page.take();
        if self.right_open {
            return;
        }
        self.right_open = true;
        if let Some(panel) = self.graph_panel.clone() {
            let origin = self.main.read(cx).page();
            let moved = was_hidden_with.is_some_and(|page| page != origin);
            panel.update(cx, |graph, cx| {
                if moved {
                    graph.set_origin(origin, cx);
                }
                graph.refresh(cx);
            });
        }
    }

    /// End the edit if `host` owns it and hand the keyboard back to the workspace. Hiding a
    /// card removes its `PageView` from the window, and a focused element that is not
    /// rendered gets no keystrokes, so every shortcut would die (AGENTS.md, §13.4).
    fn end_edit_in(&mut self, host: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let owned = self
            .editor
            .read(cx)
            .target()
            .is_some_and(|t| t.host == host);
        if owned {
            self.editor.update(cx, |e, cx| e.end(cx));
            window.focus(&self.focus);
        }
    }

    /// Close the sidebar: end any edit a stacked card hosts (the same invariant as folding
    /// one card, RS-5) and hand focus back from the graph panel, whose drawing stops with
    /// the sidebar (GR-5). Either way a focused element that is no longer rendered would
    /// swallow every shortcut. The panel itself is kept — reopening the sidebar brings it
    /// back — but it does no work while it is hidden (`show_right`).
    fn hide_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hosts: Vec<EntityId> = self.panes.iter().map(|p| p.view.entity_id()).collect();
        for host in hosts {
            self.end_edit_in(host, window, cx);
        }
        let focused = self
            .graph_panel
            .as_ref()
            .is_some_and(|panel| panel.read(cx).focus_handle(cx).is_focused(window));
        if focused {
            window.focus(&self.focus);
        }
        self.hidden_main_page = Some(self.main.read(cx).page());
        self.right_open = false;
        cx.notify();
    }

    /// Dragging the sidebar's left edge (RS-4). The window-level move listener runs wherever
    /// the pointer is, so dragging past the edge keeps the width following the pointer.
    fn resize_move(&mut self, e: &gpui::MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((start_x, start_width)) = self.resize_from else {
            return;
        };
        if e.pressed_button.is_none() {
            self.resize_from = None;
            return;
        }
        let width = crate::appearance::clamp_sidebar_width(
            start_width + (start_x - f32::from(e.position.x)),
        );
        if (width - self.right_width).abs() >= 1. {
            self.right_width = width;
            crate::appearance::set_right_sidebar_width(cx, width);
        }
    }

    /// The drag ended: remember the width in `settings.json` (rule 9). One write per drag,
    /// not one per pixel.
    fn resize_done(&mut self, _: &gpui::MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.resize_from.take().is_none() {
            return;
        }
        self.right_width = crate::appearance::right_sidebar_width(cx);
        if let Err(e) = crate::appearance::remember(cx) {
            eprintln!("utopia: settings.json: {e}");
        }
        cx.notify();
    }

    fn toggle_left(&mut self, _: &ToggleLeftSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.left_open = !self.left_open;
        let _ = (self.app.read(cx).store).local_set("left_sidebar", flag(self.left_open));
        cx.notify();
    }

    pub fn left_sidebar_open(&self) -> bool {
        self.left_open
    }

    /// Whether the top bar's page menu is open (DESIGN.md §3.3), for scripts: it is
    /// transient and must be false after any navigation, Escape or an outside click.
    pub fn page_menu_open(&self) -> bool {
        self.page_menu
    }

    /// Whether the footer's sync view is open (SB-4).
    pub fn sync_view_open(&self) -> bool {
        self.sync_open
    }

    /// The dot state from the runner right now (SB-4): off, synced, in flight or failed.
    pub fn sync_dot(&self) -> SyncDot {
        sync_dot(self.sync.as_ref())
    }

    /// The dot state the last painted frame showed (SB-4), for tests that must tell a
    /// repaint from a stale one.
    pub fn rendered_sync_dot(&self) -> Option<SyncDot> {
        self.rendered_sync_dot
    }

    fn toggle_refs(&mut self, _: &ToggleReferences, _: &mut Window, cx: &mut Context<Self>) {
        if self.right_open && self.inspector_open {
            self.inspector_open = false;
        } else {
            self.show_right(cx);
            self.inspector_open = true;
        }
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.history(true, window, cx);
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.history(false, window, cx);
    }

    /// Undo/redo while no block is being edited; the caret goes back into the main pane.
    fn history(&mut self, undo: bool, window: &mut Window, cx: &mut Context<Self>) {
        let caret = self.app.update(cx, |app, cx| {
            let r = if undo {
                app.store.undo()
            } else {
                app.store.redo()
            };
            let caret = app.report(r).flatten();
            app.changed(Vec::new(), true, cx);
            caret
        });
        if let Some(c) = caret {
            self.main
                .update(cx, |m, cx| m.restore_caret(&c, window, cx));
        }
    }

    /// Commit the block being edited, marked (IME) text included, without moving focus.
    /// The app's single quit path calls this before it flushes the store (AC-1.1); quitting
    /// itself is the app's job (`main.rs`), so no window owns a half of it.
    pub fn finish_edit(&mut self, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.end(cx));
    }

    fn pick_date(
        &mut self,
        d: NaiveDate,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_date(d, sidebar, !sidebar, window, cx);
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.month = crate::calendar::shift_month(self.month, delta);
        cx.notify();
    }

    // ---------------------------------------------------------------- data

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let v = self.app.read(cx).refs_version;
        // RS-4: the width lives in the preferences, so a reset from Settings (or another
        // window's drag) reaches this sidebar too. Never while this one is dragging.
        if self.resize_from.is_none() {
            self.right_width = crate::appearance::right_sidebar_width(cx);
        }
        // Pinned pages are a local preference (rule 9); re-read when it changes, and on an
        // index update so a renamed page's new title reaches its row (RS-3).
        if crate::appearance::pinned(cx) != self.pinned_ids || self.pinned_version != Some(v) {
            self.load_pinned(cx);
        }
        if self.sidebar_seen != Some(v) {
            self.sidebar_seen = Some(v);
            let store = &self.app.read(cx).store;
            let all = store.list_pages(true).unwrap_or_default();
            // The tag classification is cached by `AppState` on the index feed, never
            // computed from a frame (SB-2, rule 10).
            let tags = self.app.read(cx).tag_pages.clone();
            let ref_counts = store.page_backlink_counts().unwrap_or_default();
            let pages = all
                .iter()
                .filter(|p| !p.is_daily() && !tags.contains(&p.id))
                .cloned()
                .collect();
            let tag_rows = all
                .iter()
                .filter(|p| tags.contains(&p.id))
                .cloned()
                .collect();
            self.sidebar = SidebarData {
                days: all.iter().filter_map(|p| p.daily_date).collect(),
                pages,
                tag_rows,
                ref_counts,
                all,
            };
            // Card headers name their page (RS-5); a rename must reach them.
            let pages: Vec<Option<PageId>> =
                self.panes.iter().map(|p| p.view.read(cx).page()).collect();
            let titles: Vec<SharedString> = self.app.update(cx, |app, _| {
                let fmt = app.store.date_format().clone();
                pages
                    .into_iter()
                    .map(|page| {
                        page.and_then(|p| {
                            app.store
                                .page_state(&p)
                                .ok()
                                .map(|st| utopia_store::display_title(st, &fmt))
                        })
                        .unwrap_or_default()
                        .into()
                    })
                    .collect()
            });
            for (pane, title) in self.panes.iter_mut().zip(titles) {
                pane.title = title;
            }
        }
        let Some(page) = self.main.read(cx).page() else {
            return;
        };
        if self.inspector_seen.as_ref() != Some(&(page.clone(), v)) {
            self.inspector_seen = Some((page.clone(), v));
            // RS-3's Outline: the page's headings in document order, so a long page is
            // navigable from the panel. Read from the shard, never resolved at paint time.
            let outline = self.app.update(cx, |app, _| {
                app.store
                    .page_state(&page)
                    .map(|st| {
                        st.live_preorder()
                            .into_iter()
                            .filter_map(|(id, depth)| {
                                let b = st.block(&id)?;
                                match b.kind {
                                    utopia_core::BlockKind::Heading(level) => Some(OutlineEntry {
                                        block: id,
                                        level: level.clamp(1, 3),
                                        depth,
                                        text: plain_text(&b.text),
                                    }),
                                    _ => None,
                                }
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
            let store = &self.app.read(cx).store;
            let incoming = store
                .linked_references(&page, &Default::default())
                .unwrap_or_default()
                .groups;
            let outgoing = store.outgoing_refs(&page, None).unwrap_or_default();
            let unresolved = store
                .unresolved_refs(&page)
                .unwrap_or_default()
                .into_iter()
                .map(|o| {
                    let label = match &o.target {
                        RefTarget::UnresolvedPage { key } => store
                            .block_row(&o.from_block)
                            .ok()
                            .flatten()
                            .and_then(|b| {
                                extract_refs(&b.text)
                                    .into_iter()
                                    .find(|r| {
                                        r.is_page_like() && normalize_title(&r.target) == *key
                                    })
                                    .map(|r| r.target)
                            })
                            .unwrap_or_else(|| key.clone()),
                        RefTarget::Block { id, .. } => format!("(({id}))"),
                        RefTarget::Page { title, .. } => title.clone(),
                    };
                    (o, label)
                })
                .collect();
            self.inspector = InspectorData {
                incoming,
                outgoing,
                unresolved,
                outline,
            };
        }
    }

    // ---------------------------------------------------------------- rendering

    /// The graph file's name without its extension: the workspace row and the footer both
    /// call the vault by it (DESIGN.md §3.2).
    fn vault_name(&self, cx: &gpui::App) -> String {
        self.app
            .read(cx)
            .store
            .path()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Utopia".into())
    }

    /// One clickable line in the right sidebar's reference lists (RS-2): an outgoing link
    /// or an unresolved title. The left sidebar's own rows are [`Self::sidebar_page_row`].
    fn nav_item(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<SharedString>,
        page: Option<PageId>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let main = self.main.read(cx);
        let active = page.is_some() && !main.is_journal() && main.page() == page;
        div()
            .id(id)
            .px_2()
            .py_0p5()
            .rounded(px(RADIUS_CHIP))
            .cursor_pointer()
            .text_size(px(SMALL.size))
            .line_height(px(SMALL.line))
            .text_color(if active { th.link } else { th.ink })
            .when(active, |d| d.bg(th.hover))
            .hover(|s| s.bg(th.hover))
            .child(label.into())
            .when_some(page, |d, page| {
                d.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        this.open(page.clone(), None, None, e.modifiers.shift, window, cx)
                    }),
                )
            })
    }

    /// A page row in the sidebar (DESIGN.md §3.2): 32 px tall, the page icon, the name and
    /// the reference count in mono 12 muted on the right. Tags are not in this list — they
    /// are chips under TAGS. `unpin` builds the Pinned group's row, which carries the
    /// control that takes a page out of it (SB-3).
    fn sidebar_page_row(
        &self,
        index: usize,
        page: &PageRow,
        unpin: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let main = self.main.read(cx);
        let active = !main.is_journal() && main.page().as_ref() == Some(&page.id);
        let count = self.sidebar.ref_counts.get(&page.id).copied().unwrap_or(0);
        let id = page.id.clone();
        let group = if unpin { "pinned" } else { "page" };
        div()
            .id((group, index))
            .debug_selector(move || format!("{group}-row-{index}"))
            .h(px(PAGE_ROW_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .rounded(px(RADIUS_CONTROL))
            .cursor_pointer()
            .text_size(px(UI.size))
            .line_height(px(UI.line))
            .text_color(if active { th.ink } else { th.ink_2 })
            .when(active, |d| {
                d.bg(th.selected).font_weight(gpui::FontWeight::MEDIUM)
            })
            .hover(|d| d.bg(th.nav_hover))
            .child(icon(
                Icon::Page,
                ICON_SIZE,
                if active { th.link } else { th.ink_3 },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(page.title.clone()),
            )
            .when(unpin, |d| {
                let unpin_page = id.clone();
                d.child(
                    div()
                        .id(("unpin", index))
                        .debug_selector(move || format!("unpin-{index}"))
                        .flex_none()
                        .px_1()
                        .rounded(px(RADIUS_CHIP))
                        .text_size(px(SMALL.size))
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(th.line).text_color(th.ink))
                        .child("✕")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_pin(unpin_page.clone(), cx);
                            }),
                        ),
                )
            })
            .child(
                div()
                    .debug_selector(move || format!("{group}-count-{index}"))
                    .flex_none()
                    .font_family(LABEL_MONO)
                    .text_size(px(COUNT.size))
                    .text_color(th.muted)
                    .child(count.to_string()),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    this.open(id.clone(), None, None, e.modifiers.shift, window, cx)
                }),
            )
    }

    /// SB-3: pin or unpin a page; the preference is local (`settings.json`, rule 9).
    fn toggle_pin(&mut self, page: PageId, cx: &mut Context<Self>) {
        match crate::appearance::toggle_pinned(cx, &page) {
            Ok(_) => cx.notify(),
            Err(e) => {
                self.app.update(cx, |app, cx| {
                    app.last_error =
                        Some(format!("could not save {}: {e}", crate::appearance::FILE));
                    cx.notify();
                });
            }
        }
    }

    fn render_left(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let today = self.today;
        let journal = self.main.read(cx).is_journal();
        let vault = self.vault_name(cx);
        let initial: String = vault
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_else(|| "U".into());
        let pages = self.sidebar.pages.clone();
        let tag_rows = self.sidebar.tag_rows.clone();
        // SB-3: pinned pages first, in pin order, above the ordinary list. A pin may point
        // at a daily page, which is not in `pages` but is in `all`.
        let pinned: Vec<PageRow> = crate::appearance::pinned(cx)
            .iter()
            .filter_map(|id| self.sidebar.all.iter().find(|p| &p.id == id).cloned())
            .collect();
        let label = |text: &'static str, id: &'static str| {
            div()
                .debug_selector(move || id.into())
                .mt_3()
                .px_2()
                .font_family(LABEL_MONO)
                .text_size(px(SIDEBAR_LABEL.size))
                .font_weight(SIDEBAR_LABEL.weight)
                .text_color(th.muted)
                .child(text)
        };

        // 1. Workspace row: the initial in an ink square, the name, and the collapse
        // button. No workspace switcher: this app has one graph at a time.
        let workspace = div()
            .debug_selector(|| "workspace-row".into())
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .child(
                div()
                    .debug_selector(|| "workspace-initial".into())
                    .size(px(WORKSPACE_SQUARE))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_CONTROL))
                    .bg(th.ink)
                    .text_color(th.paper)
                    .text_size(px(SMALL.size))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(initial),
            )
            .child(
                div()
                    .debug_selector(|| "workspace-name".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(WORKSPACE_NAME.size))
                    .line_height(px(WORKSPACE_NAME.line))
                    .font_weight(WORKSPACE_NAME.weight)
                    .text_color(th.ink)
                    .child(vault.clone()),
            )
            .child(
                ghost_icon("collapse-left", Icon::SidebarLeft, &th)
                    .debug_selector(|| "collapse-left".into())
                    .tooltip(tip("Collapse sidebar"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_left(&ToggleLeftSidebar, window, cx)
                    })),
            );

        // 2. The search field is a button that opens the command palette (§3.2).
        let search = div()
            .id("open-jump")
            .debug_selector(|| "sidebar-search".into())
            .mt_3()
            .h(px(SEARCH_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .rounded(px(RADIUS_INPUT))
            .bg(th.paper)
            .border_1()
            .border_color(th.line_strong)
            .cursor_pointer()
            .hover(|d| d.bg(th.hover))
            .child(icon(Icon::Search, ICON_SIZE, th.ink_3))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(UI.size))
                    .text_color(th.muted)
                    .child("Search or jump to"),
            )
            .child(kbd("⌘K", &th))
            .on_click(
                cx.listener(|this, _, window, cx| this.start_jump(JumpMode::Open, window, cx)),
            );

        // 3. Nav. "All pages" has no view in this app, so it is not offered rather than
        // invented (the report says so).
        let nav = nav_row(
            "daily-notes",
            "Daily Notes",
            Some(Icon::Calendar),
            journal,
            &th,
        )
        .debug_selector(|| "daily-notes".into())
        .mt_3()
        .on_click(cx.listener(|this, _, window, cx| this.open_journal(window, cx)));

        // 4. The mini calendar, always shown (§3.2).
        let calendar = div()
            .mt_3()
            .px_1()
            .child(crate::calendar::render_month_strip(
                cx,
                self.month,
                today,
                &self.sidebar.days,
                crate::calendar::StripActions {
                    pick: Self::pick_date,
                    shift: Self::shift_month,
                },
            ));

        let mut scroll = div()
            .id("left-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .mt_2();
        if !pinned.is_empty() {
            scroll = scroll.child(label("PINNED", "pinned-header"));
            for (i, page) in pinned.into_iter().enumerate() {
                scroll = scroll.child(self.sidebar_page_row(i, &page, true, cx));
            }
        }
        // 5. Pages, without tag pages.
        scroll = scroll.child(label("PAGES", "pages-header"));
        if pages.is_empty() {
            scroll = scroll.child(
                div()
                    .px_2()
                    .pt_1()
                    .text_size(px(SMALL.size))
                    .text_color(th.muted)
                    .child("Type [[ or # to make a page."),
            );
        }
        for (i, page) in pages.into_iter().enumerate() {
            scroll = scroll.child(self.sidebar_page_row(i, &page, false, cx));
        }
        // 6. Tags, as chips in a wrapping row — never in Pages (§3.2, FR-2.15).
        if !tag_rows.is_empty() {
            scroll = scroll.child(label("TAGS", "tags-header"));
            let mut chips = div()
                .debug_selector(|| "tags".into())
                .px_2()
                .pt_1()
                .flex()
                .flex_wrap()
                .gap_1();
            for (i, tag) in tag_rows.iter().enumerate() {
                let page = tag.id.clone();
                let title = tag.title.clone();
                let sel = format!("tag-chip-{}", tag.title);
                chips = chips.child(
                    div()
                        .id(("tag-chip", i))
                        .debug_selector(move || sel.clone())
                        .h(px(TAG_HEIGHT))
                        .flex()
                        .items_center()
                        .px(px(TAG_PADDING_X))
                        .rounded(px(RADIUS_TAG))
                        .bg(th.tag_bg)
                        .text_color(th.tag_ink)
                        .text_size(px(TAG.size))
                        .cursor_pointer()
                        .child(format!("#{title}"))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                this.open(page.clone(), None, None, e.modifiers.shift, window, cx)
                            }),
                        ),
                );
            }
            scroll = scroll.child(chips);
        }

        div()
            .debug_selector(|| "left-sidebar".into())
            .w(px(crate::theme::SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(th.shelf)
            .border_r_1()
            .border_color(th.line)
            .p(px(crate::theme::SIDEBAR_PADDING))
            .child(workspace)
            .child(search)
            .child(nav)
            .child(calendar)
            .child(scroll)
            .child(self.render_footer(cx))
            .into_any_element()
    }

    /// The sidebar footer (DESIGN.md §3.2): an 8 px status dot (`disabled` while this graph
    /// is local only), the vault name in `ink` at 500, " · Local only" muted and the
    /// settings button. The status area still opens the sync view above the footer (SB-4);
    /// the settings button is a separate target, so clicking it never toggles sync.
    fn render_footer(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let th = theme(cx).clone();
        let vault = self.vault_name(cx);
        let app = self.app.read(cx);
        let (err, notice) = (app.last_error.clone(), app.notice.clone());
        let dot = sync_dot(self.sync.as_ref());
        self.rendered_sync_dot = Some(dot);
        let dot_color = match dot {
            SyncDot::Off => th.disabled,
            SyncDot::Synced => th.success,
            SyncDot::Syncing => th.warning,
            SyncDot::Error => th.danger,
        };
        let local_only = self.sync.is_none();
        let open = self.sync_open;
        div()
            .relative()
            .pt_2()
            .border_t_1()
            .border_color(th.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("sidebar-footer")
                            .debug_selector(|| "sidebar-footer".into())
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1()
                            .py_0p5()
                            .rounded(px(RADIUS_CONTROL))
                            .cursor_pointer()
                            .hover(|d| d.bg(th.hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sync_open = !this.sync_open;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .debug_selector(move || format!("sync-dot-{}", dot.key()))
                                    .flex_none()
                                    .size(px(FOOTER_DOT))
                                    .rounded_full()
                                    .bg(dot_color),
                            )
                            .child(
                                div()
                                    .debug_selector(|| "vault-name".into())
                                    .flex_none()
                                    .max_w(px(VAULT_NAME_MAX_WIDTH))
                                    .truncate()
                                    .text_size(px(SMALL.size))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(th.ink)
                                    .child(vault),
                            )
                            .when(local_only, |d| {
                                d.child(
                                    div()
                                        .debug_selector(|| "local-only".into())
                                        .flex_none()
                                        .text_size(px(SMALL.size))
                                        .text_color(th.muted)
                                        .child("· Local only"),
                                )
                            })
                            .when(self.encrypted, |d| {
                                d.child(
                                    div()
                                        .id("encryption-lock")
                                        .debug_selector(|| "encryption-lock".into())
                                        .flex_none()
                                        .flex()
                                        .flex_col()
                                        .items_center()
                                        .tooltip(tip("Encrypted at rest"))
                                        // A drawn lock: the bundled IBM Plex Sans has no
                                        // 🔒 glyph.
                                        .child(
                                            div()
                                                .w(px(6.))
                                                .h(px(4.))
                                                .border_1()
                                                .border_b_0()
                                                .rounded_t_sm()
                                                .border_color(th.ink_3),
                                        )
                                        .child(
                                            div()
                                                .w(px(8.))
                                                .h(px(6.))
                                                .rounded(px(RADIUS_CHIP))
                                                .bg(th.ink_3),
                                        ),
                                )
                            }),
                    )
                    .child(
                        ghost_icon("open-settings", Icon::Settings, &th)
                            .debug_selector(|| "open-settings".into())
                            .tooltip(tip("Settings"))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(OpenSettings), cx)
                            }),
                    )
                    .children(open.then(|| {
                        deferred(
                            anchored()
                                .anchor(gpui::Corner::BottomLeft)
                                .offset(gpui::point(px(0.), px(-6.)))
                                .child(self.sync_popover(cx)),
                        )
                        .with_priority(1)
                    })),
            )
            .when_some(notice, |d, n| d.child(div().text_color(th.ink).child(n)))
            .when_some(err, |d, e| d.child(div().text_color(th.danger).child(e)))
    }

    /// The sync view the footer opens (SB-4): what sync is doing, where it goes and whether
    /// this graph is encrypted at rest. Read-only; setup lives in Settings → Sync, and no
    /// secret is shown here (§15.6).
    fn sync_popover(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let (line, warn) = match &self.sync {
            None => ("Off.".to_string(), false),
            Some(runner) => status_line(runner, self.app.read(cx).store.now()),
        };
        div()
            .id("sync-popover")
            .debug_selector(|| "sync-popover".into())
            .w(px(crate::theme::SYNC_POPOVER_WIDTH))
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .child(
                div()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(th.ink)
                    .child("Sync"),
            )
            .child(div().when(warn, |d| d.text_color(th.warning)).child(line))
            .when_some(self.sync_target.clone(), |d, target| {
                d.child(div().child(target))
            })
            .child(div().child(if self.encrypted {
                "Encrypted at rest."
            } else {
                "Not encrypted at rest."
            }))
            .when(self.sync.is_none(), |d| {
                d.child(
                    div()
                        .text_color(th.muted)
                        .child("Set it up in Settings → Sync."),
                )
            })
    }

    /// The muted line a panel shows when it has nothing to list (RS-2, RS-3). `id` names
    /// it for `debug_selector`, so a test can tell the empty state from a missing panel.
    fn panel_empty(
        &self,
        id: &'static str,
        text: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        div()
            .debug_selector(move || id.to_string())
            .px_2()
            .py_1()
            .text_size(px(crate::theme::CAPTION_SIZE))
            .text_color(th.muted)
            .child(text)
            .into_any_element()
    }

    /// Follow a page or block chip clicked in the right sidebar: a page opens in the main
    /// pane, a block opens its page zoomed to it. An unresolved title is created, exactly as
    /// following it from the outline does (FR-4.14). Local UI state only — nothing here
    /// writes a shard beyond that page creation.
    fn follow_link(
        &mut self,
        target: crate::rich::LinkTarget,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            crate::rich::LinkTarget::Page(title) => {
                let page = self.app.update(cx, |app, cx| {
                    let found = app.store.find_page(&title).ok().flatten();
                    match found {
                        Some(p) => Some(p),
                        None => {
                            let r = app.store.create_page(&title);
                            let p = app.report(r);
                            app.changed(Vec::new(), true, cx);
                            p
                        }
                    }
                });
                if let Some(page) = page {
                    self.open(page, None, None, sidebar, window, cx);
                }
            }
            crate::rich::LinkTarget::Block(uid) => {
                if let Ok(Some(b)) = self.app.read(cx).store.block_row(&BlockId(uid)) {
                    self.open(b.page_id, Some(b.id), None, sidebar, window, cx);
                }
            }
            // Attachments and URLs are the page's business, not the panel's.
            crate::rich::LinkTarget::Url(_) => {}
        }
    }

    /// The right sidebar's reference panel: three tabs (RS-3) over the page's references,
    /// its headings, or the pages pinned to stay reachable.
    fn render_inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let panel_tab = |this: &Self,
                         id: &'static str,
                         t: PanelTab,
                         label: &'static str,
                         cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .px_2()
                .py_0p5()
                .rounded(px(crate::theme::RADIUS_CHIP))
                .text_size(px(crate::theme::CONTROL_SIZE))
                .cursor_pointer()
                .text_color(if this.panel_tab == t {
                    th.link
                } else {
                    th.muted
                })
                .when(this.panel_tab == t, |d| d.bg(th.hover))
                .hover(|d| d.bg(th.hover))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.panel_tab = t;
                    cx.notify();
                }))
        };
        let header = div()
            .flex()
            .items_center()
            .gap_1()
            .child(panel_tab(
                self,
                "panel-references",
                PanelTab::References,
                "References",
                cx,
            ))
            .child(panel_tab(
                self,
                "panel-outline",
                PanelTab::Outline,
                "Outline",
                cx,
            ))
            .child(panel_tab(
                self,
                "panel-pinned",
                PanelTab::Pinned,
                "Pinned",
                cx,
            ));
        let body = match self.panel_tab {
            PanelTab::References => self.render_references(cx),
            PanelTab::Outline => self.render_outline(cx),
            PanelTab::Pinned => self.render_pinned(cx),
        };
        // With no pages stacked above it, the panel is the sidebar and fills it. With
        // pages, it keeps a fixed share at the bottom so both stay usable.
        let only_panel = self.panes.is_empty();
        div()
            .debug_selector(|| format!("panel-{}", self.panel_tab.id()))
            .when(only_panel, |d| d.flex_1().min_h_0())
            .when(!only_panel, |d| d.h(px(260.)).flex_none())
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(th.line)
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_references(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let tab = |this: &Self,
                   id: &'static str,
                   t: InspectorTab,
                   label: String,
                   cx: &mut Context<Self>| {
            div()
                .id(id)
                .debug_selector(move || id.to_string())
                .px_2()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .rounded(px(crate::theme::RADIUS_CHIP))
                .cursor_pointer()
                .text_color(if this.tab == t { th.link } else { th.muted })
                .when(this.tab == t, |d| d.bg(th.hover))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = t;
                    cx.notify();
                }))
        };
        let n_in: usize = self.inspector.incoming.iter().map(|g| g.hits.len()).sum();
        let tabs = div()
            .flex()
            .gap_1()
            .child(tab(
                self,
                "tab-in",
                InspectorTab::Incoming,
                format!("Incoming {n_in}"),
                cx,
            ))
            .child(tab(
                self,
                "tab-out",
                InspectorTab::Outgoing,
                format!("Outgoing {}", self.inspector.outgoing.len()),
                cx,
            ))
            .child(tab(
                self,
                "tab-un",
                InspectorTab::Unresolved,
                format!("Unresolved {}", self.inspector.unresolved.len()),
                cx,
            ));
        let mut body = div()
            .id("inspector-body")
            .flex()
            .flex_col()
            .gap_0p5()
            .overflow_y_scroll()
            .flex_1();
        match self.tab {
            InspectorTab::Incoming => {
                // One group per source page, its count next to the name (LR-2), and below
                // it each referencing block drawn the way Linked References draws it (RS-1).
                let groups: Vec<utopia_store::RefGroup> = self.inspector.incoming.clone();
                let me = cx.entity();
                if groups.is_empty() {
                    body =
                        body.child(self.panel_empty("references-empty", "No references yet.", cx));
                }
                let mut ix = 0usize;
                for (gi, group) in groups.iter().enumerate() {
                    let page = group.page.id.clone();
                    let header = div()
                        .id(("ref-group", gi))
                        .debug_selector(move || format!("ref-group-{gi}"))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .pt_2()
                        .pb_0p5()
                        .rounded(px(crate::theme::RADIUS_CHIP))
                        .cursor_pointer()
                        .hover(|d| d.bg(th.hover))
                        .child(
                            div()
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(th.muted)
                                .child(group.page.title.clone()),
                        )
                        .child(
                            div()
                                .px_1()
                                .rounded(px(crate::theme::RADIUS_CHIP))
                                .bg(th.line_soft)
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .text_color(th.muted)
                                .child(group.hits.len().to_string()),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                this.open(page.clone(), None, None, e.modifiers.shift, window, cx)
                            }),
                        );
                    body = body.child(header);
                    for hit in &group.hits {
                        body = body.child(crate::reference::reference_row(
                            "ref-block",
                            ix,
                            // The group header above names the page, and the sidebar is too
                            // narrow for LR-4's inline children.
                            crate::reference::RefRow::alone(hit, &group.page.title),
                            &th,
                            &self.app.read(cx).store,
                            &[],
                            8.,
                            me.clone(),
                        ));
                        ix += 1;
                    }
                }
            }
            InspectorTab::Outgoing => {
                let rows: Vec<OutgoingRef> = self.inspector.outgoing.clone();
                for (i, o) in rows.into_iter().enumerate() {
                    let (label, page): (String, Option<PageId>) = match &o.target {
                        RefTarget::Page { id, title } => (title.clone(), Some(id.clone())),
                        RefTarget::UnresolvedPage { key } => (format!("{key} (unresolved)"), None),
                        RefTarget::Block { status, id } => match status {
                            utopia_store::BlockRefStatus::Live { text, page, .. } => (
                                format!(
                                    "(( {} ))",
                                    plain_text(text).chars().take(40).collect::<String>()
                                ),
                                Some(page.clone()),
                            ),
                            utopia_store::BlockRefStatus::Deleted { .. } => {
                                ("deleted block".into(), None)
                            }
                            utopia_store::BlockRefStatus::Missing => {
                                (format!("Missing block (({id}))"), None)
                            }
                        },
                    };
                    body = body.child(self.nav_item(
                        ("out", i),
                        format!("{label}  ·  {}", o.count),
                        page,
                        cx,
                    ));
                }
            }
            InspectorTab::Unresolved => {
                // RS-2: every dangling reference is a row you can act on — the title becomes
                // a page, or the intent is pointed elsewhere or turned back into text
                // (FR-4.14). A missing `((block))` has no page to create, only these choices.
                let rows = self.inspector.unresolved.clone();
                if rows.is_empty() {
                    body = body.child(self.panel_empty(
                        "unresolved-empty",
                        "No unresolved references",
                        cx,
                    ));
                }
                for (i, (o, label)) in rows.into_iter().enumerate() {
                    let block = o.from_block.clone();
                    let mut row = div().flex().flex_col().gap_1().px_2().py_1().child(
                        div()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .text_color(th.danger)
                            .child(label.clone()),
                    );
                    if let RefTarget::UnresolvedPage { key } = &o.target {
                        let (k2, k3) = (key.clone(), key.clone());
                        let (b2, b3) = (block.clone(), block.clone());
                        let title = label.clone();
                        let action =
                            |id: &'static str, text: &'static str, th: &crate::theme::Theme| {
                                quiet_control((id, i), text, th)
                                    .debug_selector(move || format!("unresolved-{id}-{i}"))
                            };
                        row = row.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_1()
                                .child(action("create", "Create page", &th).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        // The row's title is the page title, exactly as
                                        // following the intent from the outline would
                                        // create it (FR-4.14).
                                        let page = this.app.update(cx, |app, cx| {
                                            let r = app.store.create_page(&title);
                                            let page = app.report(r);
                                            app.changed(Vec::new(), true, cx);
                                            page
                                        });
                                        let _ = page;
                                    },
                                )))
                                .child(action("redirect", "Redirect…", &th).on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.start_jump(
                                            JumpMode::Redirect {
                                                block: b2.clone(),
                                                key: k2.clone(),
                                            },
                                            window,
                                            cx,
                                        )
                                    },
                                )))
                                .child(action("plain", "Show as text", &th).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.app.update(cx, |app, cx| {
                                            let r = app.store.unlink_intent(&b3, &k3);
                                            app.report(r);
                                            app.changed(Vec::new(), true, cx);
                                        });
                                    },
                                ))),
                        );
                    }
                    body = body.child(row);
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .flex_1()
            .min_h_0()
            .child(tabs)
            .child(body)
            .into_any_element()
    }

    /// RS-3: the headings of the page in the main pane, indented as they are in the outline
    /// and drawn at the type scale's heading sizes. Clicking one jumps to it (caret at its
    /// start, scrolled into view).
    fn render_outline(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let entries = self.inspector.outline.clone();
        let mut body = div()
            .id("outline-body")
            .debug_selector(|| "outline-body".into())
            .flex()
            .flex_col()
            .gap_0p5()
            .overflow_y_scroll()
            .flex_1()
            .min_h_0();
        if entries.is_empty() {
            body = body.child(self.panel_empty("outline-empty", "No headings in this page.", cx));
        }
        for (i, e) in entries.iter().enumerate() {
            let block = e.block.clone();
            body = body.child(
                div()
                    .id(("outline", i))
                    .debug_selector(move || format!("outline-{i}"))
                    .flex()
                    .items_center()
                    .pr_2()
                    .pl(px(8. + e.depth.min(6) as f32 * 10.))
                    .py_0p5()
                    .rounded(px(crate::theme::RADIUS_CHIP))
                    .cursor_pointer()
                    .text_size(px(match e.level {
                        1 => crate::theme::H1_SIZE,
                        2 => crate::theme::H2_SIZE,
                        _ => crate::theme::H3_SIZE,
                    }))
                    .text_color(th.ink)
                    .hover(|d| d.bg(th.hover))
                    .child(div().flex_1().min_w_0().child(e.text.clone()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.main
                            .update(cx, |m, cx| m.reveal_block(&block, window, cx));
                    })),
            );
        }
        body.into_any_element()
    }

    /// RS-3: pages pinned to the sidebar, and the control that pins the page being read.
    /// Pinning is local UI state, kept in `settings.json` (rule 9) and never synced.
    fn render_pinned(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let current = self.main.read(cx).page();
        let pinned_here = current
            .as_ref()
            .is_some_and(|p| self.pinned.iter().any(|(q, _)| q == p));
        let mut body = div()
            .id("pinned-body")
            .debug_selector(|| "pinned-body".into())
            .flex()
            .flex_col()
            .gap_0p5()
            .overflow_y_scroll()
            .flex_1()
            .min_h_0();
        if let Some(page) = current {
            body = body.child(
                quiet_control(
                    "pin-current",
                    if pinned_here {
                        "Unpin this page"
                    } else {
                        "Pin this page"
                    },
                    &th,
                )
                .debug_selector(|| "pin-current".into())
                .on_click(cx.listener(move |this, _, _, cx| {
                    // The same pin the left sidebar's page menu toggles (SB-3).
                    this.toggle_pin(page.clone(), cx);
                })),
            );
        }
        if self.pinned.is_empty() {
            body = body.child(self.panel_empty("pinned-empty", "No pinned pages yet.", cx));
        }
        for (i, (page, title)) in self.pinned.clone().into_iter().enumerate() {
            let open_page = page.clone();
            let unpin = page.clone();
            body = body.child(
                div()
                    .id(("pinned", i))
                    .debug_selector(move || format!("pinned-{i}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded(px(crate::theme::RADIUS_CHIP))
                    .cursor_pointer()
                    .hover(|d| d.bg(th.hover))
                    .child(div().flex_1().min_w_0().child(title))
                    .child(
                        div()
                            .id(("unpin", i))
                            .debug_selector(move || format!("unpin-{i}"))
                            .px_1()
                            .rounded(px(crate::theme::RADIUS_CHIP))
                            .text_color(th.muted)
                            .cursor_pointer()
                            .hover(|d| d.bg(th.hover).text_color(th.ink))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_pin(unpin.clone(), cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                        this.open(
                            open_page.clone(),
                            None,
                            None,
                            e.modifiers().shift,
                            window,
                            cx,
                        )
                    })),
            );
        }
        body.into_any_element()
    }

    /// Read the pinned ids from the preferences and resolve each to its current title, so a
    /// rename shows up without a restart and an id from another graph is skipped.
    fn load_pinned(&mut self, cx: &mut Context<Self>) {
        let ids = crate::appearance::pinned(cx);
        let resolved = self.app.update(cx, |app, _| {
            let fmt = app.store.date_format().clone();
            ids.iter()
                .filter_map(|page| {
                    let title = app
                        .store
                        .page_state(page)
                        .map(|st| utopia_store::display_title(st, &fmt))
                        .ok()
                        .filter(|t| !t.is_empty())?;
                    Some((page.clone(), title))
                })
                .collect::<Vec<_>>()
        });
        self.pinned = resolved;
        self.pinned_ids = ids;
        self.pinned_version = Some(self.app.read(cx).refs_version);
    }

    /// The main pane's top bar (DESIGN.md §3.3): title or breadcrumb on the left, actions on
    /// the right, a `line_soft` hairline below. Back/forward are not drawn because the app
    /// has no navigation history to move through (the report says so).
    fn render_top_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let (journal, title, page, draft) = {
            let main = self.main.read(cx);
            (
                main.is_journal(),
                main.page_title(),
                main.page(),
                main.is_draft(),
            )
        };
        let mut bar = div()
            .debug_selector(|| "top-bar".into())
            .min_h(px(TOP_BAR_MIN_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px(px(TOP_BAR_PADDING_X))
            .py(px(TOP_BAR_PADDING_Y))
            .bg(th.paper)
            .border_b_1()
            .border_color(th.line_soft);
        if journal {
            bar = bar
                .child(
                    div()
                        .debug_selector(|| "topbar-title".into())
                        .text_size(px(UI.size))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(th.ink)
                        .child("Daily Notes"),
                )
                .child(div().flex_1())
                .child(self.day_nav(cx));
        } else {
            bar = bar.child(
                div()
                    .debug_selector(|| "breadcrumb".into())
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .text_size(px(UI.size))
                    .child(div().flex_none().text_color(th.ink_3).child("Pages"))
                    .child(div().flex_none().text_color(th.ink_3).child("/"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(th.ink)
                            .child(title),
                    ),
            );
            if page.is_some() && !draft {
                bar = bar.child(div().flex_1()).child(self.page_menu_button(cx));
            } else {
                bar = bar.child(div().flex_1());
            }
        }
        let open = self.right_open;
        bar.child(
            ghost_icon("toggle-right", Icon::SidebarRight, &th)
                .debug_selector(|| "toggle-right".into())
                .tooltip(tip("Toggle right sidebar"))
                .when(open, |d| d.bg(th.hover))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_right(&ToggleRightSidebar, window, cx)
                })),
        )
        .into_any_element()
    }

    /// The Daily Notes day navigation group: prev | Today | next (DESIGN.md §3.3). Prev and
    /// next move between the loaded days in the feed (older left, newer right, the same
    /// direction as `PrevDay`/`NextDay`); Today resets the feed to today (§4.1).
    fn day_nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let seg = |id: &'static str, glyph: Icon, th: &crate::theme::Theme| {
            div()
                .id(id)
                .debug_selector(move || id.into())
                .w(px(BUTTON_HEIGHT))
                .h(px(BUTTON_HEIGHT))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(RADIUS_CONTROL))
                .cursor_pointer()
                .hover(|d| d.bg(th.hover))
                .child(icon(glyph, ICON_SIZE, th.ink_3))
        };
        // 20 px: the group is 32 px tall, leaving 6 px above and below the hairline.
        let divider = || {
            div()
                .w(px(1.))
                .h(px(BUTTON_HEIGHT - 12.))
                .flex_none()
                .bg(th.line)
        };
        div()
            .debug_selector(|| "day-nav".into())
            .h(px(BUTTON_HEIGHT))
            .flex()
            .items_center()
            .rounded(px(RADIUS_INPUT))
            .border_1()
            .border_color(th.line_strong)
            .child(
                seg("day-prev", Icon::ChevronLeft, &th)
                    .tooltip(tip("Previous day"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.main
                            .update(cx, |m, cx| m.scroll_to_adjacent_day(-1, cx))
                    })),
            )
            .child(divider())
            .child(
                ghost_text("day-today", "Today", &th)
                    .debug_selector(|| "day-today".into())
                    .h(px(BUTTON_HEIGHT - 2.))
                    .px_2()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.main.update(cx, |m, cx| m.scroll_to_today(cx))
                    })),
            )
            .child(divider())
            .child(
                seg("day-next", Icon::ChevronRight, &th)
                    .tooltip(tip("Next day"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.main
                            .update(cx, |m, cx| m.scroll_to_adjacent_day(1, cx))
                    })),
            )
            .into_any_element()
    }

    /// The page view's ⋯ button and its menu (DESIGN.md §3.3): the page actions that exist,
    /// with Delete page last. It floats under the button, so opening it never moves the page.
    fn page_menu_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let open = self.page_menu;
        div()
            .relative()
            .child(
                ghost_icon("page-menu", Icon::More, &th)
                    .debug_selector(|| "page-menu".into())
                    .tooltip(tip("Page actions"))
                    .when(open, |d| d.bg(th.hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page_menu = !this.page_menu;
                        cx.notify();
                    })),
            )
            .children(open.then(|| {
                deferred(
                    anchored()
                        .anchor(gpui::Corner::TopRight)
                        .offset(gpui::point(px(0.), px(MENU_OFFSET)))
                        .child(self.page_menu_popup(cx)),
                )
                .with_priority(1)
            }))
            .into_any_element()
    }

    fn page_menu_popup(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let page = self.main.read(cx).page();
        let kind = self.main.read(cx).page_kind();
        let has_aliases = self.main.read(cx).has_aliases();
        let pinned = page
            .as_ref()
            .is_some_and(|p| crate::appearance::is_pinned(cx, p));
        let graph = quiet_control("page-menu-graph", "Open in graph", &th)
            .debug_selector(|| "page-menu-graph".into())
            .on_click(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                this.main.update(cx, |m, cx| m.request_open_graph(cx));
                cx.notify();
            }));
        let export = quiet_control("page-menu-export", "Export", &th)
            .debug_selector(|| "page-menu-export".into())
            .on_click(cx.listener(|this, _, window, cx| {
                this.page_menu = false;
                this.main.update(cx, |m, cx| m.export_graph(window, cx));
                cx.notify();
            }));
        // Daily titles are display-only (rule 6): Rename would open the calendar instead.
        let rename = (kind != utopia_core::PageKind::Daily).then(|| {
            quiet_control("page-menu-rename", "Rename", &th)
                .debug_selector(|| "page-menu-rename".into())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.page_menu = false;
                    this.main.update(cx, |m, cx| m.start_rename(window, cx));
                    cx.notify();
                }))
        });
        let rewrite = (has_aliases && kind == utopia_core::PageKind::Page).then(|| {
            quiet_control("page-menu-rewrite", "Rewrite references", &th)
                .debug_selector(|| "page-menu-rewrite".into())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.page_menu = false;
                    this.main.update(cx, |m, cx| m.page_action("rewrite", cx));
                    cx.notify();
                }))
        });
        let pin = page.clone().map(|p| {
            let pin_page = p.clone();
            quiet_control(
                "page-menu-pin",
                if pinned {
                    "Unpin from sidebar"
                } else {
                    "Pin to sidebar"
                },
                &th,
            )
            .debug_selector(|| "page-menu-pin".into())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page_menu = false;
                this.toggle_pin(pin_page.clone(), cx);
                cx.notify();
            }))
        });
        let delete = quiet_danger("page-menu-delete", "Delete page", &th)
            .debug_selector(|| "page-menu-delete".into())
            .on_click(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                this.main.update(cx, |m, cx| m.page_action("delete", cx));
                cx.notify();
            }));
        div()
            .id("page-menu-popup")
            .debug_selector(|| "page-menu-popup".into())
            // Without `occlude` the menu is drawn over the page but clicks pass through it,
            // so its items never fire (found by test).
            .occlude()
            .w(px(PAGE_MENU_WIDTH))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .rounded(px(RADIUS_CONTROL))
            .child(graph.w_full().justify_start())
            .child(export.w_full().justify_start())
            .children(rename.map(|r| r.w_full().justify_start()))
            .children(rewrite.map(|r| r.w_full().justify_start()))
            .children(pin.map(|p| p.w_full().justify_start()))
            .child(div().h(px(1.)).my_1().bg(th.line))
            .child(delete.w_full().justify_start())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                cx.notify();
            }))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh(cx);
        let th = theme(cx).clone();
        let density = crate::appearance::density(cx);
        let left = if self.left_open {
            self.render_left(cx)
        } else {
            // Collapsed: a thin rail that brings the sidebar back (Cmd+\).
            div()
                .debug_selector(|| "left-rail".into())
                .w(px(32.))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .pt_2()
                .bg(th.shelf)
                .border_r_1()
                .border_color(th.line)
                .child(
                    div()
                        .id("expand-left")
                        .px_1()
                        .rounded(px(crate::theme::RADIUS_CHIP))
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(th.hover))
                        .child("»")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_left(&ToggleLeftSidebar, window, cx)
                        })),
                )
                .into_any_element()
        };
        let mut main_col = div().flex_1().min_w_0().h_full().flex().flex_col();
        if self.new_day {
            main_col = main_col.child(
                div()
                    .id("new-day")
                    .flex()
                    .justify_center()
                    .py_1()
                    .bg(th.banner_bg)
                    .text_size(px(crate::theme::BODY_SIZE))
                    .cursor_pointer()
                    .child("It's a new day → open today")
                    .on_click(cx.listener(|this, _, window, cx| this.open_journal(window, cx))),
            );
        }
        let notice = self.sync.as_ref().and_then(|s| s.notice.clone());
        if let Some((text, at)) = notice {
            // Shown for five seconds after a merge (§9.3 "Merged N changes").
            if self.app.read(cx).store.now() - at < 5_000 {
                main_col = main_col.child(
                    div()
                        .flex()
                        .justify_center()
                        .py_1()
                        .bg(th.banner_bg)
                        .text_size(px(crate::theme::BODY_SIZE))
                        .child(text),
                );
            }
        }
        if matches!(self.content, MainContent::Page) {
            main_col = main_col.child(self.render_top_bar(cx));
        }
        let mut panes_row = div()
            .debug_selector(move || format!("ui-density-{}", density.key()))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_row()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .map(|d| match &self.content {
                        MainContent::Graph(graph) => d.child(graph.clone()),
                        MainContent::Pdf(pdf) => d.child(pdf.clone()),
                        MainContent::Page => d.child(self.main.clone()),
                    }),
            );
        for (i, sp) in self.splits.iter().enumerate() {
            panes_row = panes_row.child(
                div()
                    .debug_selector(move || format!("split-{i}"))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .border_l_1()
                    .border_color(th.line)
                    .child(sp.clone()),
            );
        }
        main_col = main_col.child(panes_row);
        let main_col = main_col.debug_selector(|| "main-pane".into());
        let right = self.right_open.then(|| {
            let mut col = div()
                .debug_selector(|| "right-sidebar".into())
                .w(px(self.right_width))
                .h_full()
                .flex_none()
                .flex()
                .flex_row()
                .bg(th.well)
                .border_l_1()
                .border_color(th.line)
                // RS-4: a 4 px handle on the left edge. The drag is tracked from here and
                // read by the window-level move handler below, so it survives the pointer
                // leaving the sidebar.
                .child(
                    div()
                        .id("right-resize")
                        .debug_selector(|| "right-resize".into())
                        .w(px(4.))
                        .h_full()
                        .flex_none()
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                        .when(self.resize_from.is_some(), |d| d.bg(th.link))
                        .hover(|d| d.bg(th.link))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, e: &MouseDownEvent, _, cx| {
                                this.resize_from =
                                    Some((f32::from(e.position.x), this.right_width));
                                cx.notify();
                            }),
                        ),
                );
            let mut panel = div().flex().flex_col().flex_1().min_w_0().h_full().child(
                // The sidebar's own header: it collapses here (RS-4).
                div().flex().items_center().justify_end().px_1().child(
                    div()
                        .id("right-close")
                        .debug_selector(|| "right-close".into())
                        .px_1()
                        .rounded(px(crate::theme::RADIUS_CHIP))
                        .text_size(px(crate::theme::CONTROL_SIZE))
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|d| d.bg(th.hover).text_color(th.ink))
                        .child("×")
                        .on_click(cx.listener(|this, _, window, cx| this.hide_right(window, cx))),
                ),
            );
            // GR-5: the page's local graph sits above the reference panel, beside the page
            // that stays editable. It keeps its own focus handling (`hide_right`).
            if let Some(graph) = &self.graph_panel {
                panel = panel.child(
                    div()
                        .debug_selector(|| "graph-panel".into())
                        .w_full()
                        .h(px(400.))
                        .flex_none()
                        .border_b_1()
                        .border_color(th.line)
                        .child(graph.clone()),
                );
            }
            if self.panes.is_empty() && !self.inspector_open {
                panel = panel.child(
                    div()
                        .p_4()
                        .text_size(px(crate::theme::BODY_SIZE))
                        .text_color(th.muted)
                        .child("Shift-click a link to open it here."),
                );
            }
            for (i, pane) in self.panes.iter().enumerate() {
                // RS-5: a card per page, its header carrying the fold arrow and the close ×,
                // like Roam's right sidebar. A folded card is its header and nothing more.
                let collapsed = pane.collapsed;
                let title = pane.title.clone();
                let mut card = div()
                    .id(("pane-card", i))
                    .debug_selector(move || format!("pane-{i}"))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .border_b_1()
                    .border_color(th.line)
                    .when(collapsed, |d| d.flex_none())
                    .when(!collapsed, |d| d.flex_1().min_h(px(140.)));
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .py_0p5()
                        .hover(|d| d.bg(th.hover))
                        .child(
                            div()
                                .id(("pane-collapse", i))
                                .debug_selector(move || format!("pane-collapse-{i}"))
                                .px_1()
                                .rounded(px(crate::theme::RADIUS_CHIP))
                                .text_size(px(crate::theme::CONTROL_SIZE))
                                .text_color(th.muted)
                                .cursor_pointer()
                                .hover(|d| d.text_color(th.ink))
                                .child(if collapsed { "▸" } else { "▾" })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let host = this.panes.get(i).map(|p| p.view.entity_id());
                                    let folding = this.panes.get(i).is_some_and(|p| !p.collapsed);
                                    if let (true, Some(host)) = (folding, host) {
                                        this.end_edit_in(host, window, cx);
                                    }
                                    if let Some(pane) = this.panes.get_mut(i) {
                                        pane.collapsed = !pane.collapsed;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .debug_selector(move || format!("pane-title-{i}"))
                                .flex_1()
                                .min_w_0()
                                .text_size(px(crate::theme::CAPTION_SIZE))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(th.muted)
                                .child(title),
                        )
                        .child(
                            div()
                                .id(("pane-close", i))
                                .debug_selector(move || format!("pane-close-{i}"))
                                .px_1()
                                .rounded(px(crate::theme::RADIUS_CHIP))
                                .text_size(px(crate::theme::CONTROL_SIZE))
                                .text_color(th.muted)
                                .cursor_pointer()
                                .hover(|d| d.bg(th.hover).text_color(th.ink))
                                .child("×")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if i < this.panes.len() {
                                        let host = this.panes[i].view.entity_id();
                                        this.end_edit_in(host, window, cx);
                                        this.panes.remove(i);
                                    }
                                    cx.notify();
                                })),
                        ),
                );
                if !collapsed {
                    card = card.child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .border_t_1()
                            .border_color(th.line)
                            .child(pane.view.clone()),
                    );
                }
                panel = panel.child(card);
            }
            if self.inspector_open {
                panel = panel.child(self.render_inspector(cx));
            }
            col = col.child(panel);
            col
        });
        let overlay = self.jump.as_ref().map(|(j, _)| {
            div()
                .id("jump-overlay")
                .absolute()
                .size_full()
                // The scrim is a sibling behind the palette, not its background: a menu
                // deferred out of the palette (the age menu) must not be dismissed by the
                // same click that chooses an item.
                .child(
                    div()
                        .id("jump-scrim")
                        .absolute()
                        .size_full()
                        .bg(th.scrim)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.jump = None;
                                window.focus(&this.focus);
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .justify_center()
                        .pt(px(90.))
                        .child(
                            div()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(j.clone()),
                        ),
                )
        });
        let _ = window;
        let font = crate::appearance::ui_font(cx);
        div()
            // Names the font in force, for tests (Settings → Appearance).
            .debug_selector(move || format!("ui-font-{}", font.key()))
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::quick_jump))
            .on_action(cx.listener(Self::full_search))
            .on_action(cx.listener(Self::show_graph))
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, window, cx| {
                    this.drop_files(paths.paths(), window, cx)
                }),
            )
            .on_action(cx.listener(Self::quick_capture))
            .on_action(cx.listener(Self::go_daily_notes))
            .on_action(cx.listener(Self::go_today))
            .on_action(cx.listener(Self::go_yesterday))
            .on_action(cx.listener(Self::prev_day))
            .on_action(cx.listener(Self::next_day))
            .on_action(cx.listener(Self::toggle_right))
            .on_mouse_move(cx.listener(Self::resize_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::resize_done))
            .on_action(cx.listener(Self::toggle_left))
            .on_action(cx.listener(Self::toggle_refs))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .bg(th.paper)
            .text_color(th.ink)
            .text_size(px(density.text_size()))
            .line_height(px(density.line_height()))
            .font_family(font.family())
            .child(left)
            .child(main_col)
            .children(right)
            .children(overlay)
    }
}

fn flag(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}
