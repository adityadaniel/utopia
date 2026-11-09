//! A page (or zoomed block) as a virtualized outline plus its Linked References
//! (§6, §7, §8). Used for the main pane and for every right-sidebar pane.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use gpui::{
    anchored, deferred, div, list, point, prelude::*, px, AnyElement, Context, Corner, Entity,
    EventEmitter, FocusHandle, Focusable, ListAlignment, ListOffset, ListState, MouseButton,
    MouseDownEvent, SharedString, StyledText, Subscription, Window,
};
use utopia_core::inline::{extract_refs, plain_text};
use utopia_core::outline::{self, Focus, Plan};
use utopia_core::undo::Caret;
use utopia_core::{BlockId, BlockKind, PageId, PageKind};
use utopia_store::{LinkedRefs, Mention, PageRow, RefChildren, RefHit, RefQuery, RefSort};

use crate::app_state::{AppState, Changed, IndexUpdated, PageReplaced};
use crate::editor::{BlockEditor, EditTarget, EditorEvent};
use crate::field::Goal;
use crate::line_input::{LineInput, LineInputEvent};
use crate::rich::{self, LinkTarget};
use crate::theme::{quiet_control, quiet_danger, quiet_menu, theme, INDENT, MAX_VISUAL_DEPTH};

/// Transclusions stop expanding at this depth (FR-4.5).
const MAX_EMBED_DEPTH: u8 = 3;
/// A query block lists at most this many results.
const MAX_QUERY_HITS: usize = 50;
/// An embed renders at most this many blocks (§13.4).
const MAX_EMBED_BLOCKS: usize = 100;
/// Groups show this many references before "+N more" (FR-3.2).
const GROUP_PREVIEW: usize = 3;
/// The Daily Notes feed loads this many more days whenever its end comes into view.
const JOURNAL_BATCH: usize = 10;
/// Widest the page column gets (about 95 characters of body text per line).
const READABLE_WIDTH: f32 = 760.;
/// The reference filter field's preferred width (LR-1); it no longer stretches the column.
const REF_FILTER_WIDTH: f32 = 240.;
/// The narrowest the field gets before the pane is too small to show the controls at all.
const REF_FILTER_MIN_WIDTH: f32 = 90.;
/// Height of the pinned day heading (DN-3), fixed so that revealing a row can leave room
/// for it: its padding, the caption line, the date line and the hairline.
const STICKY_HEIGHT: f32 = 4. + 20. + 34. + 4. + 1.;

#[derive(Debug, Clone)]
pub enum PageViewEvent {
    Open {
        page: PageId,
        zoom: Option<BlockId>,
        focus: Option<BlockId>,
        sidebar: bool,
    },
    Close,
    /// Cmd+Shift+Click: open beside the main pane (P1 split panes).
    OpenSplit {
        page: PageId,
    },
    /// Slash "Extract block to page": the workspace asks for a title (FR-2.18).
    RequestExtract {
        page: PageId,
        block: BlockId,
    },
    /// "Open in graph" in the page menu (PG-2): show the graph with this page focused.
    OpenGraph {
        page: PageId,
    },
    /// Pin or unpin this page in the left sidebar (SB-3). The workspace owns the local
    /// preference; the page itself is untouched (rule 9).
    TogglePin {
        page: PageId,
    },
    /// A `{{pdf: blob:…}}` attachment was clicked (P2 PDF viewer).
    OpenPdf {
        hash: String,
        name: String,
        /// 0-based page to show first.
        page: Option<usize>,
        /// The block the PDF was opened from; new highlights go under it.
        host: Option<BlockId>,
    },
}

#[derive(Debug, Clone)]
pub struct RowItem {
    pub page: PageId,
    pub block: BlockId,
    pub text: String,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
    pub embed: u8,
    pub kind: BlockKind,
    pub checked: bool,
    /// 1-based position among live siblings, for numbered blocks.
    pub number: usize,
}

#[derive(Debug, Clone)]
enum Item {
    Header,
    Row(RowItem),
    /// A page attribute (`key:: value`) drawn as one compact muted row (PG-1). View only:
    /// the block stays the source, and "Show as blocks" draws it in full again.
    Attribute {
        depth: usize,
        key: String,
        value: String,
        block: BlockId,
        /// Children folded away with the block, marked with "…" so nothing vanishes
        /// silently; "Show as blocks" brings them back.
        children: usize,
    },
    Notice {
        depth: usize,
        text: SharedString,
    },
    /// Results of a `{{query: …}}` block (P2), under the block.
    QueryHeader {
        depth: usize,
        label: SharedString,
    },
    QueryHit {
        depth: usize,
        hit: utopia_store::QueryHit,
    },
    /// An attachment referenced by the row above (P2).
    Attachment {
        depth: usize,
        blob: utopia_core::attach::BlobRef,
        path: Option<std::path::PathBuf>,
        size: Option<u64>,
        /// This device has the bytes (in-memory graphs have no path).
        present: bool,
        /// The block whose text attaches it.
        host: BlockId,
    },
    Empty,
    RefsHeader {
        title: SharedString,
        count: usize,
    },
    RefsControls,
    GroupHeader {
        page: PageRow,
        count: usize,
        scheduled: bool,
    },
    /// A referencing block (LR-3, LR-4): the group's page title and the block's first
    /// children travel with it, so paint never reads the store (rule 3).
    Hit {
        hit: RefHit,
        page_title: SharedString,
        /// The children shown inline, in outline order.
        children: Vec<(BlockId, String)>,
        /// Live children past the inline limit, behind the "+N more" chip.
        hidden: usize,
    },
    More {
        page: PageId,
        hidden: usize,
    },
    NoRefs,
    UnlinkedHeader {
        count: Option<usize>,
        open: bool,
    },
    Mention(Mention),
    /// The unlinked section is open and there is nothing left to link (LR-5).
    NoMentions,
    /// A day's title in the Daily Notes feed (`page` is None for a blank day).
    DayHeader {
        date: NaiveDate,
        page: Option<PageId>,
        title: SharedString,
        /// "Thursday · Today" above the date (DN-2).
        caption: SharedString,
        /// Today's caption is in the accent colour (DN-2).
        today: bool,
        first: bool,
    },
    /// A day in the feed with nothing on it, folded to one 36 px row (DN-1).
    DayFolded {
        date: NaiveDate,
        weekday: SharedString,
        title: SharedString,
    },
    /// The click-to-start row under an open day that has no blocks yet (today and
    /// yesterday always stay open, DN-1).
    DayEmpty {
        date: NaiveDate,
    },
    /// The end of the loaded days; older ones load when it comes into view.
    JournalEnd,
    /// The end of the feed (FR-1.2): the oldest daily page is loaded, so no older day
    /// can hold anything. Drawn instead of [`Item::JournalEnd`].
    JournalExhausted,
}

/// A day heading in the feed, kept so the sticky header can find the day at the top of
/// the viewport without walking the items in `render` (DN-3, rule 10).
#[derive(Debug, Clone)]
struct DayHeading {
    /// Index of the day's [`Item::DayHeader`] in `items`.
    ix: usize,
    /// One past the day's last item, so a folded day (whose own date is in its row) does
    /// not pin the previous day's heading over it.
    end: usize,
    date: NaiveDate,
    title: SharedString,
    caption: SharedString,
    today: bool,
}

/// One calendar day of the feed while it is being built.
struct FeedDay {
    date: NaiveDate,
    page: Option<PageId>,
    title: String,
    rows: Vec<Item>,
}

/// Pagination state of the Daily Notes feed (FR-1.2, `journal-feed-pagination`).
///
/// The dates are *calendar* days: the feed shows every day from today back to
/// `shown_through`, written or blank. [`Journal::pages`] counts only the days that own a
/// page, so the two units stay apart instead of hiding in one counter.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Feed {
    /// The window down to `shown_through` is what is on screen; the row after it asks
    /// for more when it is laid out.
    Idle { shown_through: NaiveDate },
    /// One batch has been asked for and not rebuilt yet: `want_through` is the day the
    /// next rebuild loads down to. Keeping the request in the state means a second
    /// layout of the sentinel cannot ask twice.
    Loading {
        shown_through: NaiveDate,
        want_through: NaiveDate,
    },
    /// Every calendar day back to the oldest live daily page is loaded, so there is
    /// nothing older to ask for. Recomputed on each rebuild: a page created or arriving
    /// by sync re-opens the feed instead of latching it shut.
    Exhausted { shown_through: NaiveDate },
}

impl Feed {
    /// How far back the loaded window reaches.
    fn shown_through(&self) -> NaiveDate {
        match *self {
            Feed::Idle { shown_through }
            | Feed::Loading { shown_through, .. }
            | Feed::Exhausted { shown_through } => shown_through,
        }
    }

    /// The day a rebuild must load down to (the pending request, if any).
    fn want_through(&self) -> NaiveDate {
        match *self {
            Feed::Loading { want_through, .. } => want_through,
            Feed::Idle { shown_through } | Feed::Exhausted { shown_through } => shown_through,
        }
    }

    /// Ask for the next batch: only from an idle feed, one request at a time.
    fn request(&self) -> Option<Feed> {
        match *self {
            Feed::Idle { shown_through } => Some(
                match shown_through.checked_sub_days(chrono::Days::new(JOURNAL_BATCH as u64)) {
                    Some(want_through) => Feed::Loading {
                        shown_through,
                        want_through,
                    },
                    // Older than any representable date: nothing to load.
                    None => Feed::Exhausted { shown_through },
                },
            ),
            Feed::Loading { .. } | Feed::Exhausted { .. } => None,
        }
    }

    /// Resolve a rebuild that loaded down to `shown_through`, given the oldest live
    /// daily page (`None`: the graph has no days at all).
    fn settle(self, shown_through: NaiveDate, oldest: Option<NaiveDate>) -> Feed {
        match oldest {
            // The oldest page is inside the loaded window: every older calendar day is
            // blank, so loading them would only walk back through empty years.
            Some(oldest) if shown_through <= oldest => Feed::Exhausted { shown_through },
            None => Feed::Exhausted { shown_through },
            Some(_) => Feed::Idle { shown_through },
        }
    }

    /// The row drawn after the last day asks for more when laid out (only while idle).
    fn sentinel(&self) -> bool {
        matches!(self, Feed::Idle { .. })
    }
}

/// The Daily Notes feed: today, then every older calendar day, as one editable outline.
#[derive(Debug, Clone)]
struct Journal {
    /// How much of the calendar is loaded, and whether a batch is pending.
    feed: Feed,
    /// The days with a page, newest first (fewer than the calendar days loaded).
    pages: Vec<PageId>,
    /// Each day's Linked References, valid for one `refs_version`.
    refs: HashMap<PageId, LinkedRefs>,
    refs_version: u64,
}

/// What this view is showing (`docs/state-machines/page-view-mode.md`). One type instead
/// of `page` + `zoom` + `journal`, because the three were a state machine in disguise:
/// a feed always has today's page and never a zoom, and a zoom always has a page. Those
/// combinations are now unrepresentable rather than merely implied.
#[derive(Debug, Clone)]
enum Mode {
    /// Nothing opened yet: a view that was constructed but never `open`ed.
    Empty,
    /// A whole page (FR-2.14).
    Page { page: PageId },
    /// One block of a page, with breadcrumbs back to the page (FR-2.7).
    Zoomed { page: PageId, root: BlockId },
    /// The Daily Notes feed (FR-1.2). `today` is the page the caret starts in.
    Journal { today: PageId, journal: Journal },
}

impl Mode {
    /// The page on screen, in every mode but [`Mode::Empty`].
    fn page(&self) -> Option<&PageId> {
        match self {
            Mode::Empty => None,
            Mode::Page { page } | Mode::Zoomed { page, .. } => Some(page),
            Mode::Journal { today, .. } => Some(today),
        }
    }

    /// The zoom root, in [`Mode::Zoomed`] only.
    fn zoom(&self) -> Option<&BlockId> {
        match self {
            Mode::Zoomed { root, .. } => Some(root),
            _ => None,
        }
    }

    /// The feed, in [`Mode::Journal`] only.
    fn journal(&self) -> Option<&Journal> {
        match self {
            Mode::Journal { journal, .. } => Some(journal),
            _ => None,
        }
    }

    fn journal_mut(&mut self) -> Option<&mut Journal> {
        match self {
            Mode::Journal { journal, .. } => Some(journal),
            _ => None,
        }
    }

    fn is_journal(&self) -> bool {
        matches!(self, Mode::Journal { .. })
    }

    /// Point the view at `to` instead of `from`: a promoted draft day was merged into the
    /// page that owns the date (FR-1.2).
    fn retarget(&mut self, from: &PageId, to: &PageId) {
        match self {
            Mode::Page { page } if page == from => *page = to.clone(),
            Mode::Zoomed { page, .. } if page == from => *page = to.clone(),
            Mode::Journal { today, .. } if today == from => *today = to.clone(),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Header {
    title: String,
    kind: PageKind,
    date: Option<NaiveDate>,
    aliases: Vec<String>,
    deleted: bool,
    missing: bool,
    /// A day that is shown but not written yet (FR-1.2): nothing to delete.
    draft: bool,
    zoom_path: Vec<(BlockId, String)>,
    /// `Key:: value` root blocks: the page metadata header (P1, W7).
    attributes: Vec<(String, String)>,
}

/// Which of the reference controls has its menu open (LR-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlMenu {
    Date,
    Sort,
}

/// One choice in a reference control's menu (LR-1).
struct ControlChoice {
    selector: &'static str,
    label: &'static str,
    /// "Last N days" for the date menu; `None` is any time.
    days: Option<u64>,
    sort: Option<RefSort>,
    selected: bool,
}

/// What a handler invalidated, so the next pass recomputes it
/// (`docs/state-machines/page-view-rebuild-cycle.md`). A reason set instead of one `bool`:
/// every invalidation says what it is about, and `rebuild` clears all of it in one place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Invalidation {
    /// `items` describe an older store state.
    rows: bool,
    /// `refs` was fetched for another page, zoom or index version.
    refs: bool,
    /// The unlinked count is for another page or index version.
    unlinked: bool,
    /// One of the appearance mirrors (`ref_children_limit`, `attribute_blocks`) changed.
    prefs: bool,
}

impl Invalidation {
    fn rows() -> Self {
        Self {
            rows: true,
            ..Self::default()
        }
    }

    fn refs() -> Self {
        Self {
            refs: true,
            ..Self::default()
        }
    }

    fn unlinked() -> Self {
        Self {
            unlinked: true,
            ..Self::default()
        }
    }

    fn prefs() -> Self {
        Self {
            prefs: true,
            ..Self::default()
        }
    }

    fn any(self) -> bool {
        self.rows || self.refs || self.unlinked || self.prefs
    }
}

impl std::ops::BitOr for Invalidation {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self {
            rows: self.rows || other.rows,
            refs: self.refs || other.refs,
            unlinked: self.unlinked || other.unlinked,
            prefs: self.prefs || other.prefs,
        }
    }
}

/// The rebuild cycle's state: what is stale, and which store version the rows were built
/// from. `pending` being empty is the one definition of "clean" — the old boolean plus 20+
/// write sites, with only `rebuild` clearing it, is what let a handler's rebuild and a
/// deferred event disagree about whether the items were stale. `store` is what tells the
/// handler's own deferred `Changed` apart from a newer change.
#[derive(Default)]
struct RebuildCycle {
    pending: Invalidation,
    /// `AppState::version` the rows were built from.
    store: u64,
}

impl RebuildCycle {
    fn new() -> Self {
        // A view is born without rows and must build them before its first paint.
        Self {
            pending: Invalidation::rows(),
            store: 0,
        }
    }

    fn invalidate(&mut self, why: Invalidation) {
        self.pending = self.pending | why;
    }

    fn dirty(&self) -> bool {
        self.pending.any()
    }

    /// Mark the change an `AppState::Changed` reports. Returns `false` when the rows were
    /// already built from that store state — the handler that emitted the event rebuilt
    /// before gpui delivered it. A stale `true` here costs a second pass over the same
    /// data on every insert, split, template and plan (machine issue 1).
    fn store_changed(&mut self, version: u64, why: Invalidation) -> bool {
        if version <= self.store {
            return false;
        }
        self.invalidate(why);
        true
    }

    fn built(&mut self, store: u64) {
        self.pending = Invalidation::default();
        self.store = store;
    }
}

/// What store changes this view's rows depend on (rebuild-cycle issue 2). A query block
/// reads every page through the index, and an open transclusion renders another page's
/// blocks; everything else only needs `Changed` for the page (or feed day) it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interest {
    ThisPage,
    AllPages,
}

#[derive(Clone)]
struct DraggedBlock {
    page: PageId,
    block: BlockId,
    label: SharedString,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        div()
            .px_2()
            .py_1()
            .bg(th.surface)
            .border_1()
            .border_color(th.line)
            .rounded_md()
            .text_color(th.text)
            .child(self.0.clone())
    }
}

pub struct PageView {
    app: Entity<AppState>,
    editor: Entity<BlockEditor>,
    /// What is on screen. The only place `page`, `zoom` and the feed are decided, so no
    /// two of them can disagree (`docs/state-machines/page-view-mode.md`).
    mode: Mode,
    pub compact: bool,
    /// Whether this pane draws its own ✕ in the page header (split panes do, sidebar cards
    /// carry the close in their card header, RS-5).
    close: bool,
    items: Vec<Item>,
    list: ListState,
    header: Header,
    refs: LinkedRefs,
    refs_key: Option<(PageId, Option<BlockId>, u64)>,
    query: RefQuery,
    /// Co-reference chips offered under the controls: page, title, hit count (LR-1).
    suggestions: Vec<(PageId, String, usize)>,
    filter_titles: BTreeMap<PageId, String>,
    expanded_groups: HashSet<PageId>,
    /// Referencing blocks whose "+N more" chip was opened (LR-4). Local view state.
    expanded_children: HashSet<BlockId>,
    /// Children per reference in force when `items` was built (LR-4); the setting can change
    /// while a page is open, which rebuilds once.
    ref_children_limit: usize,
    /// How many `ref_children` reads this view has made. Debug builds only, so the
    /// regression test can prove a keystroke rebuild does not query the database.
    #[cfg(debug_assertions)]
    ref_children_queries: std::cell::Cell<usize>,
    /// How many of those reads happened inside `render` (rule 10). Debug builds only.
    #[cfg(debug_assertions)]
    ref_children_frame_queries: std::cell::Cell<usize>,
    /// True while `render` is rebuilding, so the debug counters can tell a handler read
    /// from a frame read.
    in_render: bool,
    /// Reference children already read, keyed by block and whether every child was asked
    /// for (the "+N more" row). Cleared when the index version or the limit changes, so a
    /// rebuild for an unrelated reason does not run the queries again (rule 10).
    ref_children_cache: HashMap<(BlockId, bool), RefChildren>,
    ref_children_cache_key: Option<(u64, usize)>,
    /// A reference control's menu is open (LR-1). Local view state, never persisted.
    controls_menu: Option<ControlMenu>,
    open_embeds: HashSet<(BlockId, String)>,
    rename: Option<Entity<LineInput>>,
    filter: Entity<LineInput>,
    show_calendar: bool,
    unlinked_open: bool,
    unlinked: Option<(u64, Vec<Mention>)>,
    alias_input: Option<Entity<LineInput>>,
    calendar_month: NaiveDate,
    /// The feed's day headings, in item order, for the sticky header (DN-3).
    day_headings: Vec<DayHeading>,
    /// The page header's "···" menu is open (ALL-3).
    page_menu: bool,
    /// The row the pointer is over, so its bullet can read as a drag handle and its
    /// collapse arrow can appear (PG-4). Local UI state, never persisted (rule 9).
    hovered_row: Option<BlockId>,
    /// How attribute blocks are drawn (PG-1), mirrored from `settings.json` so a change
    /// made in Settings rebuilds this page's rows.
    attribute_blocks: crate::appearance::AttributeBlocks,
    /// What store changes this view's rows depend on, recomputed from the rows it just
    /// built: a query block or an open transclusion makes every page's change relevant.
    interest: Interest,
    /// How many rows currently carry the editor's focus handle. When an edit ends the
    /// handle must leave the row with the target, or the list keeps force-rendering a row
    /// that is drawn as text (issue 4).
    focused_rows: usize,
    cycle: RebuildCycle,
    /// How many times the rows were rebuilt, and how many of those passes ran inside a
    /// frame (rule 10). Debug builds only, so the rebuild-cycle tests can prove a change
    /// is answered once and outside `render`.
    #[cfg(debug_assertions)]
    rebuilds: usize,
    #[cfg(debug_assertions)]
    frame_rebuilds: usize,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PageViewEvent> for PageView {}

impl crate::reference::RefRowHost for PageView {
    fn open_reference(
        &mut self,
        page: PageId,
        block: BlockId,
        sidebar: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        cx.emit(PageViewEvent::Open {
            page,
            zoom: None,
            focus: Some(block),
            sidebar,
        });
    }

    fn follow_reference(
        &mut self,
        target: LinkTarget,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.follow(target, None, sidebar, window, cx);
    }

    fn open_reference_page(
        &mut self,
        page: PageId,
        block: Option<BlockId>,
        sidebar: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        cx.emit(PageViewEvent::Open {
            page,
            zoom: None,
            focus: block,
            sidebar,
        });
    }

    /// LR-4: the "+N more" chip opens the rest of this block's children. The children are a
    /// store read, so fill the cache now, before the next frame.
    fn expand_reference_children(
        &mut self,
        block: &BlockId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.expanded_children.insert(block.clone()) {
            self.rebuild(cx);
            cx.notify();
        }
    }
}

impl PageView {
    /// `compact` is a pane rather than the main page; `close` draws the pane's own ✕ — the
    /// right sidebar's cards carry theirs (RS-5), split panes do not.
    pub fn new(
        app: Entity<AppState>,
        editor: Entity<BlockEditor>,
        compact: bool,
        close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| LineInput::new("Filter references…", cx));
        let subs = vec![
            cx.subscribe(&app, |this, _, e: &Changed, cx| {
                let shown = this.page().is_some_and(|p| e.pages.contains(&p))
                    || this
                        .journal()
                        .is_some_and(|j| j.pages.iter().any(|d| e.pages.contains(d)));
                let relevant = e.structural
                    || e.pages.is_empty()
                    || shown
                    || this.interest == Interest::AllPages;
                if relevant && this.cycle.store_changed(e.version, Invalidation::rows()) {
                    if this.interest == Interest::AllPages {
                        // A query block or an open transclusion reads other pages: answer
                        // in this handler, never in a frame (rule 10).
                        this.rebuild(cx);
                    }
                    cx.notify();
                }
            }),
            // A promoted draft merged into the day's page (FR-1.2): this view may be
            // showing the discarded id, so follow the page that owns the day.
            cx.subscribe(&app, |this, _, e: &PageReplaced, cx| {
                if this.page().as_ref() == Some(&e.from) {
                    this.mode.retarget(&e.from, &e.to);
                    this.refs_key = None;
                    // Transclusions belong to the page that was replaced.
                    this.open_embeds.clear();
                    this.interest = Interest::ThisPage;
                    this.cycle.invalidate(Invalidation::rows());
                    cx.notify();
                }
            }),
            cx.subscribe(&app, |this, _, _: &IndexUpdated, cx| {
                this.cycle
                    .invalidate(Invalidation::refs() | Invalidation::unlinked());
                // LR-5: the unlinked count is keyed by the index version, so it is refreshed
                // here rather than in `render` (rule 10: no database work in a frame).
                this.refresh_unlinked(cx);
                // Rebuild now, while the index version is new: reference children are
                // database reads, and a frame must only read the cache they fill.
                this.rebuild(cx);
                cx.notify();
            }),
            // LR-4: "Children shown in references" is a setting (rule 9), and PG-1's
            // "attribute blocks" is another this view mirrors. Both are resynced in one
            // place, so any rebuild builds with the preference in force (issue 3).
            cx.observe_global::<crate::appearance::Appearance>(|this, cx| {
                this.sync_preferences(cx);
                if this.cycle.dirty() {
                    this.rebuild(cx);
                    cx.notify();
                }
            }),
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.subscribe(&filter, |this, _, e: &LineInputEvent, cx| {
                if let LineInputEvent::Changed(t) = e {
                    this.query.text = (!t.trim().is_empty()).then(|| t.clone());
                    this.refs_key = None;
                    this.cycle
                        .invalidate(Invalidation::rows() | Invalidation::refs());
                    cx.notify();
                }
            }),
        ];
        let today = app.read(cx).store.today();
        Self {
            app,
            editor,
            mode: Mode::Empty,
            compact,
            close,
            items: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            header: Header::default(),
            refs: LinkedRefs::default(),
            refs_key: None,
            query: RefQuery::default(),
            suggestions: Vec::new(),
            filter_titles: BTreeMap::new(),
            expanded_groups: HashSet::new(),
            expanded_children: HashSet::new(),
            ref_children_limit: crate::appearance::ref_children(cx),
            #[cfg(debug_assertions)]
            ref_children_queries: std::cell::Cell::new(0),
            #[cfg(debug_assertions)]
            ref_children_frame_queries: std::cell::Cell::new(0),
            in_render: false,
            ref_children_cache: HashMap::new(),
            ref_children_cache_key: None,
            controls_menu: None,
            open_embeds: HashSet::new(),
            rename: None,
            filter,
            show_calendar: false,
            unlinked_open: false,
            unlinked: None,
            alias_input: None,
            calendar_month: today,
            day_headings: Vec::new(),
            page_menu: false,
            hovered_row: None,
            attribute_blocks: crate::appearance::attribute_blocks(cx),
            interest: Interest::ThisPage,
            focused_rows: 0,
            cycle: RebuildCycle::new(),
            #[cfg(debug_assertions)]
            rebuilds: 0,
            #[cfg(debug_assertions)]
            frame_rebuilds: 0,
            _subs: subs,
        }
    }

    /// How many rebuild passes this view has run (debug builds only).
    #[cfg(debug_assertions)]
    pub fn rebuilds(&self) -> usize {
        self.rebuilds
    }

    /// How many of those passes ran inside `render` (debug builds only, rule 10).
    #[cfg(debug_assertions)]
    pub fn frame_rebuilds(&self) -> usize {
        self.frame_rebuilds
    }

    /// How many rows currently carry the editor's focus handle (debug builds only).
    #[cfg(debug_assertions)]
    pub fn focused_rows(&self) -> usize {
        self.focused_rows
    }

    /// Whether this view's rows depend on every page's changes (debug builds only): a
    /// query block or an open transclusion is present.
    #[cfg(debug_assertions)]
    pub fn interested_in_all_pages(&self) -> bool {
        self.interest == Interest::AllPages
    }

    /// The interest the rows just built imply: a query header or an open transclusion
    /// means another page's change can change this view.
    fn interest_after_build(&self) -> Interest {
        let query = self
            .items
            .iter()
            .any(|i| matches!(i, Item::QueryHeader { .. }));
        if query || !self.open_embeds.is_empty() {
            Interest::AllPages
        } else {
            Interest::ThisPage
        }
    }

    /// The page the view is showing, if any (`None` before the first `open`).
    pub fn page(&self) -> Option<PageId> {
        self.mode.page().cloned()
    }

    /// The block the view is zoomed into, if any.
    pub fn zoom(&self) -> Option<BlockId> {
        self.mode.zoom().cloned()
    }

    /// The feed, while the view shows it.
    fn journal(&self) -> Option<&Journal> {
        self.mode.journal()
    }

    /// Whether the page title is being renamed (debug builds only): `start_rename` draws an
    /// input in place of the title, which is the only sign of it in a frame.
    #[cfg(debug_assertions)]
    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    /// Whether the header's date calendar is open (debug builds only).
    #[cfg(debug_assertions)]
    pub fn calendar_is_open(&self) -> bool {
        self.show_calendar
    }

    /// Show `page` (optionally zoomed into a block).
    pub fn open(&mut self, page: PageId, zoom: Option<BlockId>, cx: &mut Context<Self>) {
        self.set_mode(
            match zoom {
                Some(root) => Mode::Zoomed { page, root },
                None => Mode::Page { page },
            },
            cx,
        );
    }

    /// The Daily Notes feed (Roam's default view): `today` first, then every older journal
    /// page newest first, each editable in place. Days are never created by scrolling past
    /// them (FR-1.2).
    pub fn open_journal(&mut self, today: PageId, cx: &mut Context<Self>) {
        // The first window is the last `JOURNAL_BATCH` calendar days, today included.
        let today_date = self.app.read(cx).store.today();
        let shown_through = today_date
            .checked_sub_days(chrono::Days::new((JOURNAL_BATCH - 1) as u64))
            .unwrap_or(today_date);
        self.set_mode(
            Mode::Journal {
                today,
                journal: Journal {
                    feed: Feed::Idle { shown_through },
                    pages: Vec::new(),
                    refs: HashMap::new(),
                    refs_version: 0,
                },
            },
            cx,
        );
    }

    /// How many reference-children reads this view has made (debug builds only).
    #[cfg(debug_assertions)]
    pub fn ref_children_queries(&self) -> usize {
        self.ref_children_queries.get()
    }

    /// How many reference-children reads happened inside a frame (debug builds only).
    #[cfg(debug_assertions)]
    pub fn ref_children_frame_queries(&self) -> usize {
        self.ref_children_frame_queries.get()
    }

    pub fn is_journal(&self) -> bool {
        self.mode.is_journal()
    }

    /// Days in the Daily Notes feed that own a page, newest first (empty outside the
    /// feed). Blank calendar days are loaded but have no page; use
    /// [`Self::journal_shown_through`] for how far back the window reaches.
    pub fn journal_days(&self) -> Vec<PageId> {
        self.journal().map(|j| j.pages.clone()).unwrap_or_default()
    }

    /// The oldest calendar day the feed has loaded (FR-1.2), `None` outside the feed.
    pub fn journal_shown_through(&self) -> Option<NaiveDate> {
        self.journal().map(|j| j.feed.shown_through())
    }

    /// The day heading pinned to the top of the feed at the current scroll offset
    /// (DN-3): the last heading at or above the first visible item, unless that heading
    /// is itself exactly at the top. A binary search over the loaded days, so `render`
    /// does no more work than the offset it already reads (rule 10).
    fn sticky_heading(&self) -> Option<&DayHeading> {
        self.journal()?;
        let top = self.list.logical_scroll_top();
        let ix = self
            .day_headings
            .partition_point(|h| h.ix <= top.item_ix)
            .checked_sub(1)?;
        let heading = &self.day_headings[ix];
        if top.item_ix >= heading.end {
            return None;
        }
        let at_top = heading.ix == top.item_ix && top.offset_in_item <= px(0.5);
        (!at_top).then_some(heading)
    }

    /// The date whose heading is stuck at the top of the feed, for scripts (DN-3).
    pub fn sticky_day(&self) -> Option<NaiveDate> {
        self.sticky_heading().map(|h| h.date)
    }

    /// How many days the feed shows open, with a date heading (DN-1), for scripts.
    pub fn open_day_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, Item::DayHeader { .. }))
            .count()
    }

    /// Put the view in `next`: the one place that changes what is on screen. Everything
    /// keyed on the page or the zoom is reset here, so a mode change cannot leave a popup,
    /// a reference cache or an unlinked count behind for the view it replaced
    /// (`docs/state-machines/page-view-mode.md`).
    fn set_mode(&mut self, next: Mode, cx: &mut Context<Self>) {
        let prev = std::mem::replace(&mut self.mode, next);
        // Every opening of the feed counts as a page change: it starts a fresh window of
        // days and builds them from the default query, so the view it replaces must not
        // lend it its expansion state. Any other change is a page change when the page
        // differs; zooming within one page keeps that page's state.
        let page_changed = prev.page() != self.mode.page() || self.mode.is_journal();
        let zoom_changed = prev.zoom() != self.mode.zoom();
        if page_changed {
            self.reset_page_ui(cx);
        }
        if page_changed || zoom_changed {
            // A rename input or the date calendar belongs to the view that opened it, not
            // to the one that replaced it (issues 2 and 3).
            self.rename = None;
            self.show_calendar = false;
        }
        // Menus are transient by definition: any navigation closes them.
        self.page_menu = false;
        self.controls_menu = None;
        self.refs_key = None;
        // Navigation is a handler, not a frame: count the mentions before painting (LR-5).
        self.refresh_unlinked(cx);
        self.cycle
            .invalidate(Invalidation::rows() | Invalidation::refs() | Invalidation::unlinked());
        self.rebuild(cx);
        self.list.reset(self.items.len());
        cx.notify();
    }

    /// The state that belongs to one page: its reference filter, expansion, alias input
    /// and unlinked mentions. Zooming within the page keeps it, because it is still that
    /// page's (issue 2's counterpart).
    fn reset_page_ui(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        self.query = RefQuery::default();
        self.filter_titles.clear();
        self.unlinked = None;
        self.alias_input = None;
        self.filter.update(cx, |f, cx| f.set_text("", cx));
        let saved = self
            .app
            .read(cx)
            .store
            .local_get(&format!("refs_expanded:{page}"))
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default();
        self.expanded_groups = saved.into_iter().map(PageId).collect();
        self.expanded_children.clear();
        // An opened transclusion was a row of the page being left; keeping it would make
        // every later change on any page rebuild this one (rebuild-cycle issue 2).
        self.open_embeds.clear();
        self.interest = Interest::ThisPage;
    }

    /// Start editing the first block (creating one on an empty page, §9.3).
    pub fn focus_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        // `r.page == page`: in the Daily Notes feed the next rows belong to older days.
        let first = self.items.iter().find_map(|i| match i {
            Item::Row(r) if r.embed == 0 && r.page == page => Some(r.block.clone()),
            _ => None,
        });
        let block = match first {
            Some(b) => b,
            None => {
                let zoom = self.zoom();
                let created = self.app.update(cx, |app, cx| {
                    let r = match &zoom {
                        Some(z) => {
                            let plan = app
                                .store
                                .page_state(&page)
                                .map(|st| outline::append_block(st, Some(z)));
                            plan.and_then(|p| app.store.execute_plan(&page, p, None))
                                .map(|f| f.map(|f| (f.block, true)))
                        }
                        None => start_note(&mut app.store, &page).map(|n| Some((n.block, n.wrote))),
                    };
                    let structural = match &r {
                        Ok(Some((_, wrote))) => *wrote,
                        _ => true,
                    };
                    let b = app.report(r).flatten();
                    app.changed(vec![page.clone()], structural, cx);
                    b
                });
                self.rebuild(cx);
                match created {
                    Some((block, _)) => block,
                    None => return,
                }
            }
        };
        self.edit(page, block, Some(0), None, window, cx);
    }

    /// Show `block` and put the caret in it: unfold collapsed ancestors and leave a zoom
    /// that does not contain it first. The Outline tab lists every heading of the page
    /// (RS-3), including ones folded away or outside the zoom, so a click there must reach
    /// them — `focus_block` only sees the rows currently built.
    pub fn reveal_block(&mut self, block: &BlockId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        let zoom = self.zoom();
        let mut unfolded = false;
        let reachable = self.app.update(cx, |app, _| {
            let Ok(st) = app.store.page_state(&page) else {
                return true;
            };
            // A zoomed page only builds rows below its root; the root itself is the header.
            let inside = match &zoom {
                None => true,
                Some(z) => z != block && st.ancestors(block).contains(z),
            };
            for a in st.ancestors(block) {
                if app.store.is_collapsed(&a) {
                    let _ = app.store.set_collapsed(&a, false);
                    unfolded = true;
                }
            }
            inside
        });
        if !reachable {
            // Leaving the zoom is a navigation like `open`: run the same reset, so the
            // unlinked count, the reference cache and the popups do not stay those of the
            // zoomed view (machine issue 1).
            self.set_mode(Mode::Page { page }, cx);
        } else if unfolded {
            self.cycle.invalidate(Invalidation::rows());
        }
        self.focus_block(block, None, window, cx);
    }

    pub fn focus_block(
        &mut self,
        block: &BlockId,
        offset: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_if_dirty(cx);
        let page = self
            .items
            .iter()
            .find_map(|i| match i {
                Item::Row(r) if r.block == *block => Some(r.page.clone()),
                _ => None,
            })
            .or_else(|| {
                // PG-1: a compact attribute row — or a block hidden under one — has no Row
                // until it is the editor's target. Find the page that holds it so `edit` can
                // open the attribute's path around it (search results, backlinks, undo).
                let pages: Vec<PageId> = match self.journal() {
                    Some(j) => j.pages.clone(),
                    None => self.page().into_iter().collect(),
                };
                pages.into_iter().find(|p| {
                    self.app.update(cx, |app, _| {
                        app.store
                            .page_state(p)
                            .ok()
                            .is_some_and(|st| st.is_live(block))
                    })
                })
            });
        if let Some(page) = page {
            self.edit(page, block.clone(), offset, None, window, cx);
        }
    }

    fn edit(
        &mut self,
        page: PageId,
        block: BlockId,
        caret: Option<usize>,
        goal: Option<Goal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // DN-3: the row may be drawn under the pinned day heading. Asked of the frame the
        // user is looking at, before the rebuild below re-measures every row — and asked
        // about this *block*, then applied to the same block's index below, so a rebuild
        // that shifts rows cannot hand the offset to a different row (the concern in
        // `page-view-rebuild-cycle.md` issue 7). After a splice the list has no measured
        // rows left, so the post-rebuild frame could not be measured anyway; scrolling
        // this block below the heading is at worst one heading's worth of motion.
        let line = crate::appearance::density(cx).line_height();
        let under_heading = self
            .row_index(&block)
            .is_some_and(|ix| self.row_overlaps_pinned_day(ix, line));
        self.rebuild_if_dirty(cx);
        let drawn = self.row_index(&block).is_some();
        if !drawn {
            // Not drawn right now: a compact attribute row or a block hidden under one
            // (PG-1). Start the edit anyway and rebuild below, which opens the attribute's
            // path around the editor. A block this view cannot draw at all — the zoom root,
            // which the header shows instead, or one that is not on this page — still ends
            // the edit, so focus is never stranded on an element nothing renders.
            let zoom_root = self.zoom().as_ref() == Some(&block);
            let live_here = self.app.update(cx, |app, _| {
                app.store
                    .page_state(&page)
                    .ok()
                    .is_some_and(|st| st.is_live(&block))
            });
            if zoom_root || !live_here {
                self.editor.update(cx, |e, cx| e.end(cx));
                return;
            }
        }
        let host = cx.entity_id();
        self.editor.update(cx, |e, cx| {
            e.begin(
                EditTarget {
                    host,
                    page,
                    block: block.clone(),
                },
                caret,
                goal,
                window,
                cx,
            )
        });
        if !drawn {
            // Rebuild with the edit target set, so the folded attribute opens and this
            // block gets a row that can hold the editor (a focused element that is not
            // rendered gets no keystrokes).
            self.rebuild(cx);
        }
        let ix = self.row_index(&block);
        self.splice_items(self.items.len(), cx);
        match ix {
            // DN-3: the row was under the pinned heading: put its top below it.
            Some(ix) if under_heading => self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: -px(STICKY_HEIGHT),
            }),
            Some(ix) => {
                self.list.scroll_to_reveal_item(ix);
                // The row the list just put at its top edge is under the heading too.
                self.leave_room_for_the_pinned_day(ix);
            }
            // Still no row (an ordinary collapsed ancestor hides it): stop rather than keep
            // an editor whose focus handle is not drawn.
            None => self.editor.update(cx, |e, cx| e.end(cx)),
        }
        cx.notify();
    }

    /// DN-3: the pinned day heading is drawn *over* the top of the list, so a row the list
    /// counts as visible can still be hidden behind it. True when the row's own top sits
    /// above the heading's bottom edge — the row revealed at the list's top edge, or the
    /// next short row scrolled up under the 63 px heading. Measured against bounds when the
    /// list has them, so a tall row that merely *follows* the top one does not trigger a
    /// scroll it does not need.
    fn row_overlaps_pinned_day(&self, ix: usize, line_height: f32) -> bool {
        if self.sticky_heading().is_none() {
            return false;
        }
        let viewport = self.list.viewport_bounds();
        if viewport.size.height > px(0.) {
            if let Some(row) = self.list.bounds_for_item(ix) {
                return row.top() < viewport.top() + px(STICKY_HEIGHT);
            }
        }
        // A splice (an edit starts by re-registering the rows so the editor can keep the
        // keystrokes) leaves the list with no measured rows until the next layout. Estimate
        // from the offset instead: the heading covers `STICKY_HEIGHT`, and a row is at
        // least one line tall, so only the first few rows can hide under it.
        if -self.list.logical_scroll_top().offset_in_item >= px(STICKY_HEIGHT) {
            return false;
        }
        let rows = (STICKY_HEIGHT / line_height.max(1.)).ceil() as usize;
        ix <= self.list.logical_scroll_top().item_ix + rows
    }

    /// DN-3: a row the list reveals at its top edge is under the pinned heading. The rows
    /// re-measured by a splice have no bounds yet, so this index check stays as the
    /// fallback for that case.
    fn leave_room_for_the_pinned_day(&self, ix: usize) {
        let top = self.list.logical_scroll_top();
        if top.item_ix != ix || top.offset_in_item > px(0.) {
            return;
        }
        if self.sticky_heading().is_some() {
            self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: -px(STICKY_HEIGHT),
            });
        }
    }

    fn row_index(&self, block: &BlockId) -> Option<usize> {
        self.items
            .iter()
            .position(|i| matches!(i, Item::Row(r) if r.block == *block))
    }

    /// Re-read the two appearance preferences this view mirrors. The one place both are
    /// resynced: `rebuild` calls it too, so an index-triggered pass cannot build rows with
    /// the old setting and leave the next frame's `rebuild_if_dirty` to correct it (issue
    /// 3: only `rebuild_if_dirty` used to notice `attribute_blocks`).
    fn sync_preferences(&mut self, cx: &mut Context<Self>) {
        let children = crate::appearance::ref_children(cx);
        let attributes = crate::appearance::attribute_blocks(cx);
        let mut why = Invalidation::default();
        if self.ref_children_limit != children {
            self.ref_children_limit = children;
            // The children cache is keyed on the limit and heals on its own; the rows that
            // show those children are what is stale.
            why = why | Invalidation::prefs() | Invalidation::rows();
        }
        if self.attribute_blocks != attributes {
            self.attribute_blocks = attributes;
            why = why | Invalidation::prefs() | Invalidation::rows();
        }
        if why.any() {
            self.cycle.invalidate(why);
        }
    }

    fn rebuild_if_dirty(&mut self, cx: &mut Context<Self>) {
        self.sync_preferences(cx);
        if self.cycle.dirty() {
            self.rebuild(cx);
        }
    }

    /// Tell the list every item changed, keeping the scroll position (a rebuild after an
    /// edit, or older days loading into the feed, must not jump back to the top).
    fn replace_items(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
        let old_len = self.items.len();
        self.items = items;
        self.splice_items(old_len, cx);
    }

    /// Re-register every item with the list. The row being edited carries the editor's
    /// focus handle, so the list keeps rendering it after it scrolls out of view: a
    /// focused element that is not rendered receives no keystrokes, and every shortcut
    /// (Cmd+[, Cmd+K, undo) would stop working until something was clicked.
    fn splice_items(&mut self, old_len: usize, cx: &mut Context<Self>) {
        let top = self.list.logical_scroll_top();
        let host = cx.entity_id();
        let editor = self.editor.read(cx);
        let handle = editor.focus_handle(cx);
        let handles: Vec<Option<FocusHandle>> = self
            .items
            .iter()
            .map(|i| match i {
                Item::Row(r) if editor.is_editing(host, &r.page, &r.block) => Some(handle.clone()),
                _ => None,
            })
            .collect();
        self.focused_rows = handles.iter().filter(|h| h.is_some()).count();
        self.list.splice_focusable(0..old_len, handles);
        if top.item_ix > 0 || top.offset_in_item > px(0.) {
            self.list.scroll_to(top);
        }
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        #[cfg(debug_assertions)]
        {
            self.rebuilds += 1;
            if self.in_render {
                self.frame_rebuilds += 1;
            }
        }
        // Captured before the reads: a change made during this pass belongs to the next
        // one, even if it is this view's own (nothing here writes).
        self.sync_preferences(cx);
        let store = self.app.read(cx).version;
        self.rebuild_rows(cx);
        self.interest = self.interest_after_build();
        // Every path through `rebuild_rows` ends with the list spliced, including the
        // early returns for a missing page or header: clear the whole reason set here, in
        // one place, so no caller can leave a reason behind.
        self.cycle.built(store);
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        if !self.is_journal() {
            // Only the feed pins a day heading (DN-3).
            self.day_headings.clear();
        }
        // The mode decides what to build; the feed case cannot be reached without the page
        // its days hang off, because `Mode::Journal` carries it.
        let (page, zoom, feed) = match &self.mode {
            Mode::Empty => {
                self.replace_items(Vec::new(), cx);
                return;
            }
            Mode::Page { page } => (page.clone(), None, None),
            Mode::Zoomed { page, root } => (page.clone(), Some(root.clone()), None),
            Mode::Journal { today, journal } => {
                (today.clone(), None, Some(journal.feed.want_through()))
            }
        };
        if let Some(want_through) = feed {
            self.rebuild_journal(page, want_through, cx);
            return;
        }
        let open_embeds = self.open_embeds.clone();
        let compact_attributes = zoom.is_none() && self.attributes_are_compact();
        let editing = self.editing_block(cx.entity_id(), cx);
        let refs_version = self.app.read(cx).refs_version;
        let need_refs = self.refs_key != Some((page.clone(), zoom.clone(), refs_version));
        let query = self.query.clone();
        let (header, rows, refs) = self.app.update(cx, |app, _| {
            let mut header = Header::default();
            let collapsed = app.store.collapsed_set().clone();
            let Ok(st) = app.store.page_state(&page).cloned() else {
                header.missing = true;
                return (header, Vec::new(), None);
            };
            // From the shard, not the index: the index may lag by one flush.
            header.title = utopia_store::display_title(&st, app.store.date_format());
            header.deleted = st.meta.deleted_at.is_some();
            header.kind = st.meta.kind;
            header.draft = app.store.is_draft(&page);
            header.attributes = st
                .live_roots()
                .iter()
                .filter_map(|r| {
                    let b = st.block(r)?;
                    page_attribute(b.kind, &b.text)
                        .map(|(k, v)| (k, utopia_core::inline::plain_text(&b.text[v])))
                })
                .collect();
            header.date = st.meta.daily_date;
            header.aliases = st.meta.aliases.clone();
            if let Some(z) = &zoom {
                for a in st
                    .ancestors(z)
                    .into_iter()
                    .chain(std::iter::once(z.clone()))
                {
                    let t = st
                        .block(&a)
                        .map(|b| plain_text(&b.text))
                        .unwrap_or_default();
                    header.zoom_path.push((a, t));
                }
            }
            let mut rows = Vec::new();
            // The header only exists on a whole page, so only there do we fold attributes
            // into compact rows (PG-1); a zoomed view keeps its blocks as they are.
            push_page_rows(
                app,
                &st,
                &page,
                zoom.as_ref(),
                &collapsed,
                &open_embeds,
                compact_attributes,
                editing.as_ref(),
                &mut rows,
            );
            let refs = if need_refs {
                Some(match &zoom {
                    Some(z) => {
                        let hits = app.store.block_backlinks(z).unwrap_or_default();
                        let mut groups: Vec<utopia_store::RefGroup> = Vec::new();
                        for h in hits {
                            match groups.iter_mut().find(|g| g.page.id == h.page) {
                                Some(g) => g.hits.push(h),
                                None => {
                                    if let Ok(Some(p)) = app.store.page_row(&h.page) {
                                        groups.push(utopia_store::RefGroup {
                                            page: p,
                                            hits: vec![h],
                                        });
                                    }
                                }
                            }
                        }
                        let total = groups.iter().map(|g| g.hits.len()).sum();
                        LinkedRefs { total, groups }
                    }
                    None => app
                        .store
                        .linked_references(&page, &query)
                        .unwrap_or_default(),
                })
            } else {
                None
            };
            (header, rows, refs)
        });
        if let Some(refs) = refs {
            self.refs = refs;
            self.refs_key = Some((page.clone(), zoom.clone(), refs_version));
            self.suggestions = self.compute_suggestions(&page, cx);
        }
        self.header = header;
        let mut items = vec![Item::Header];
        if self.header.missing {
            self.replace_items(items, cx);
            return;
        }
        // A page whose only root blocks are folded attributes (PG-1) still needs somewhere
        // to click to start writing.
        if !rows.iter().any(|i| matches!(i, Item::Row(_))) {
            items.push(Item::Empty);
        }
        for r in rows {
            items.push(r);
        }
        self.push_refs(&page, &mut items, cx);
        if zoom.is_none() {
            // Unlinked References (FR-3.9, LR-5): the header always carries the count. It is
            // read from the cache `refresh_unlinked` fills when the index reports a durable
            // change or the page opens; the FTS scan behind it is database work and must not
            // run in a frame (rule 10), so a stale entry shows no number for one frame
            // rather than scanning here.
            let current = self.unlinked.as_ref().filter(|(v, _)| *v == refs_version);
            let count = current.map(|(_, m)| m.len());
            let mentions = current
                .map(|(_, m)| m.clone())
                .filter(|_| self.unlinked_open);
            items.push(Item::UnlinkedHeader {
                count,
                open: self.unlinked_open,
            });
            if self.unlinked_open {
                match mentions {
                    Some(m) if !m.is_empty() => items.extend(m.into_iter().map(Item::Mention)),
                    Some(_) => items.push(Item::NoMentions),
                    None => {}
                }
            }
        }
        self.replace_items(items, cx);
    }

    /// Refresh the unlinked-reference count for the page on screen (FR-3.9, LR-5).
    ///
    /// The scan is an FTS query over up to 2,000 blocks per title and alias, so it belongs
    /// in the index-update and navigation handlers; a paint must never run it (rule 10). The
    /// answer is cached per index version, so a frame (or a rebuild for any other reason)
    /// costs nothing.
    fn refresh_unlinked(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        if !matches!(self.mode, Mode::Page { .. }) {
            // The section is only on a whole page; `set_mode` refreshes it when it is again.
            return;
        }
        let version = self.app.read(cx).refs_version;
        if self.unlinked.as_ref().is_some_and(|(v, _)| *v == version) {
            return;
        }
        let found = self.app.update(cx, |app, _| {
            let r = app.store.unlinked_references_now(&page);
            app.report(r).unwrap_or_default()
        });
        self.unlinked = Some((version, found));
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    /// Rebuild the feed's outline for the calendar window down to `want_through`.
    fn rebuild_journal(&mut self, today: PageId, want_through: NaiveDate, cx: &mut Context<Self>) {
        let open_embeds = self.open_embeds.clone();
        let compact_attributes = self.attributes_are_compact();
        let editing = self.editing_block(cx.entity_id(), cx);
        let refs_version = self.app.read(cx).refs_version;
        let mut refs_cache = match self.mode.journal_mut() {
            Some(j) if j.refs_version == refs_version => std::mem::take(&mut j.refs),
            _ => HashMap::new(),
        };
        let (header, days, today_date, shown_through, oldest) = self.app.update(cx, |app, _| {
            let collapsed = app.store.collapsed_set().clone();
            let fmt = app.store.date_format().clone();
            let today_date = app.store.today();
            // The requested window is absolute, so a rebuild never grows it by accident.
            let from = want_through.min(today_date);
            let mut pages = app.store.days_between(from, today_date).unwrap_or_default();
            // The feed stops at the oldest daily page (FR-1.2). `oldest_daily` is one seek
            // on the partial `page_daily_uq` index, not a scan. A failed read is reported
            // and leaves the feed idle rather than claiming there is nothing older; the
            // sentinel then re-requests a batch per frame until the store recovers. That is
            // the chosen degradation: nothing is written and the error is in `last_error`.
            let oldest = app.report(app.store.oldest_daily());
            // Today comes from the caller (its draft), not the index, which may lag.
            pages.insert(today_date, today.clone());
            let mut header = Header {
                kind: PageKind::Daily,
                date: Some(today_date),
                ..Default::default()
            };
            let mut days = Vec::new();
            let mut date = today_date;
            loop {
                let mut day = FeedDay {
                    date,
                    page: None,
                    title: fmt.format(date),
                    rows: Vec::new(),
                };
                let st = pages
                    .get(&date)
                    .and_then(|p| Some((p.clone(), app.store.page_state(p).ok()?.clone())))
                    .filter(|(_, st)| st.meta.deleted_at.is_none());
                if let Some((page, st)) = st {
                    day.title = utopia_store::display_title(&st, &fmt);
                    push_page_rows(
                        app,
                        &st,
                        &page,
                        None,
                        &collapsed,
                        &open_embeds,
                        compact_attributes,
                        editing.as_ref(),
                        &mut day.rows,
                    );
                    if !refs_cache.contains_key(&page) {
                        let refs = app
                            .store
                            .linked_references(&page, &RefQuery::default())
                            .unwrap_or_default();
                        refs_cache.insert(page.clone(), refs);
                    }
                    day.page = Some(page);
                }
                if date == today_date {
                    header.title = day.title.clone();
                }
                days.push(day);
                match date.pred_opt() {
                    Some(d) if date > from => date = d,
                    _ => break,
                }
            }
            (header, days, today_date, from, oldest)
        });
        self.header = header;
        let mut items = Vec::new();
        let mut ids = Vec::new();
        let mut headings = Vec::new();
        let yesterday = today_date.pred_opt();
        for (i, day) in days.into_iter().enumerate() {
            // Like Roam's feed: Linked References under the day when it has any, and no
            // Unlinked References (those stay on the day's own page). A day with
            // references is not empty even when it has no blocks, so it stays open.
            let refs = day
                .page
                .as_ref()
                .and_then(|p| refs_cache.get(p))
                .filter(|r| !r.groups.is_empty());
            // DN-1: a day with nothing on it folds to one 36 px row. Today and yesterday
            // always stay open, so the day the caret starts in always shows its heading;
            // folding is a view change only, the day is still written on its first key.
            // PG-1: rows can exist and still not be visible — a root attribute block folds
            // to one muted row, so the day keeps its heading but still offers the prompt.
            let open =
                !day.rows.is_empty() || refs.is_some() || i == 0 || Some(day.date) == yesterday;
            // The day's heading, for the sticky header, when the day is open.
            let mut heading = None;
            if open {
                let caption: SharedString = day_caption(day.date, today_date).into();
                heading = Some(headings.len());
                headings.push(DayHeading {
                    ix: items.len(),
                    end: items.len(),
                    date: day.date,
                    title: day.title.clone().into(),
                    caption: caption.clone(),
                    today: day.date == today_date,
                });
                items.push(Item::DayHeader {
                    date: day.date,
                    page: day.page.clone(),
                    title: day.title.into(),
                    caption,
                    today: day.date == today_date,
                    first: i == 0,
                });
                if !day.rows.iter().any(|i| matches!(i, Item::Row(_))) {
                    items.push(Item::DayEmpty { date: day.date });
                }
            } else {
                items.push(Item::DayFolded {
                    date: day.date,
                    weekday: utopia_core::date::weekday_name_of(day.date).into(),
                    title: day.title.into(),
                });
            }
            items.extend(day.rows);
            if let Some(page) = day.page {
                // Like Roam's feed: Linked References under the day when it has any, and no
                // Unlinked References (those stay on the day's own page).
                if let Some(refs) = refs {
                    items.push(Item::RefsHeader {
                        title: "Linked References".into(),
                        count: refs.shown(),
                    });
                    self.push_ref_groups(&page, refs, true, &mut items, cx);
                }
                ids.push(page);
            }
            if let Some(h) = heading {
                headings[h].end = items.len();
            }
        }
        // A rebuild resolves the window it just loaded into a terminal or open state.
        // A read error stays `Idle`, so the next rebuild tries again instead of the feed
        // claiming there are no earlier notes.
        let next_feed = match (self.journal(), oldest) {
            (Some(j), Some(oldest)) => j.feed.clone().settle(shown_through, oldest),
            (Some(_), None) | (None, _) => Feed::Idle { shown_through },
        };
        items.push(if next_feed.sentinel() {
            Item::JournalEnd
        } else {
            Item::JournalExhausted
        });
        self.day_headings = headings;
        if let Some(j) = self.mode.journal_mut() {
            j.pages = ids;
            j.refs = refs_cache;
            j.refs_version = refs_version;
            j.feed = next_feed;
        }
        self.replace_items(items, cx);
    }

    /// The block this view is editing, if any: its row must be drawn as a row (PG-1).
    fn editing_block(&self, host: gpui::EntityId, cx: &gpui::App) -> Option<BlockId> {
        self.editor
            .read(cx)
            .target()
            .filter(|t| t.host == host)
            .map(|t| t.block.clone())
    }

    /// PG-1: an attribute block is drawn as one compact row unless the preference says to
    /// show the blocks.
    fn attributes_are_compact(&self) -> bool {
        self.attribute_blocks == crate::appearance::AttributeBlocks::Compact
    }

    /// The header's "Show as blocks" toggle, and a click on a compact row: set the
    /// preference, remember it, and rebuild the outline (PG-1).
    fn set_attribute_blocks(
        &mut self,
        blocks: crate::appearance::AttributeBlocks,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = crate::appearance::set_attribute_blocks(cx, blocks) {
            self.app.update(cx, |app, _| {
                app.last_error = Some(format!("could not save settings.json: {e}"))
            });
        }
        self.attribute_blocks = blocks;
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    /// Load the next batch of older days into the feed (once per batch).
    fn load_older_days(&mut self, cx: &mut Context<Self>) {
        let Some(j) = self.mode.journal_mut() else {
            return;
        };
        let Some(next) = j.feed.request() else { return };
        j.feed = next;
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    /// Start writing on a feed day with nothing on it: a draft until the first keystroke
    /// (FR-1.2), or the first block of a page that so far only has references.
    fn start_day(&mut self, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(date).and_then(|page| {
                let note = start_note(&mut app.store, &page)?;
                Ok((page, note))
            });
            // The click writes nothing on a draft, so only a real write is structural
            // (an index flush for a commit that does not exist is a wasted cycle).
            let structural = match &r {
                Ok((_, note)) => note.wrote,
                Err(_) => true,
            };
            let t = app.report(r).map(|(page, note)| (page, note.block));
            app.changed(Vec::new(), structural, cx);
            t
        });
        self.rebuild(cx);
        if let Some((page, block)) = target {
            self.edit(page, block, Some(0), None, window, cx);
        }
    }

    fn push_refs(&mut self, page: &PageId, items: &mut Vec<Item>, cx: &mut Context<Self>) {
        let shown = self.refs.shown();
        let title = if self.zoom().is_some() {
            "Referenced by"
        } else {
            "Linked References"
        };
        items.push(Item::RefsHeader {
            title: title.into(),
            count: shown,
        });
        if self.zoom().is_none() {
            items.push(Item::RefsControls);
        }
        if self.refs.groups.is_empty() {
            items.push(Item::NoRefs);
            return;
        }
        let daily = self.header.kind == PageKind::Daily;
        let refs = std::mem::take(&mut self.refs);
        self.push_ref_groups(page, &refs, daily, items, cx);
        self.refs = refs;
    }

    /// One header per referencing page, then its hits (folded past `GROUP_PREVIEW`), each
    /// with its first children inline (LR-4).
    fn push_ref_groups(
        &mut self,
        page: &PageId,
        refs: &LinkedRefs,
        daily: bool,
        items: &mut Vec<Item>,
        cx: &mut Context<Self>,
    ) {
        // Only the hits that become rows need their children; an opened "+N more" wants all.
        let shown = |g: &utopia_store::RefGroup| {
            if self.expanded_groups.contains(&g.page.id) {
                usize::MAX
            } else {
                GROUP_PREVIEW
            }
        };
        let mut folded: Vec<BlockId> = Vec::new();
        let mut opened: Vec<BlockId> = Vec::new();
        for g in &refs.groups {
            for h in g.hits.iter().take(shown(g)) {
                if self.expanded_children.contains(&h.block) {
                    opened.push(h.block.clone());
                } else {
                    folded.push(h.block.clone());
                }
            }
        }
        // The showed children only change when the index does (or the setting does), so
        // keep them across rebuilds instead of running SQL for every block on every edit.
        let key = (self.app.read(cx).refs_version, self.ref_children_limit);
        if self.ref_children_cache_key != Some(key) {
            self.ref_children_cache.clear();
            self.ref_children_cache_key = Some(key);
        }
        let mut children = HashMap::new();
        let mut missing_folded = Vec::new();
        let mut missing_opened = Vec::new();
        for id in &folded {
            match self.ref_children_cache.get(&(id.clone(), false)) {
                Some(c) => {
                    children.insert(id.clone(), c.clone());
                }
                None => missing_folded.push(id.clone()),
            }
        }
        for id in &opened {
            match self.ref_children_cache.get(&(id.clone(), true)) {
                Some(c) => {
                    children.insert(id.clone(), c.clone());
                }
                None => missing_opened.push(id.clone()),
            }
        }
        let (folded_read, opened_read) = {
            let store = &self.app.read(cx).store;
            let folded_read = if missing_folded.is_empty() {
                HashMap::new()
            } else {
                #[cfg(debug_assertions)]
                {
                    self.ref_children_queries
                        .set(self.ref_children_queries.get() + 1);
                    if self.in_render {
                        self.ref_children_frame_queries
                            .set(self.ref_children_frame_queries.get() + 1);
                    }
                }
                store
                    .ref_children(&missing_folded, self.ref_children_limit)
                    .unwrap_or_default()
            };
            let opened_read = if missing_opened.is_empty() {
                HashMap::new()
            } else {
                #[cfg(debug_assertions)]
                {
                    self.ref_children_queries
                        .set(self.ref_children_queries.get() + 1);
                    if self.in_render {
                        self.ref_children_frame_queries
                            .set(self.ref_children_frame_queries.get() + 1);
                    }
                }
                store
                    .ref_children(&missing_opened, usize::MAX)
                    .unwrap_or_default()
            };
            (folded_read, opened_read)
        };
        for (id, c) in folded_read {
            self.ref_children_cache
                .insert((id.clone(), false), c.clone());
            children.insert(id, c);
        }
        for (id, c) in opened_read {
            self.ref_children_cache
                .insert((id.clone(), true), c.clone());
            children.insert(id, c);
        }
        for g in &refs.groups {
            let scheduled = daily && g.page.id != *page;
            items.push(Item::GroupHeader {
                page: g.page.clone(),
                count: g.hits.len(),
                scheduled,
            });
            let limit = shown(g);
            for h in g.hits.iter().take(limit) {
                let kids = children.get(&h.block).cloned().unwrap_or_default();
                let hidden = kids.total.saturating_sub(kids.shown.len());
                items.push(Item::Hit {
                    hit: h.clone(),
                    page_title: g.page.title.clone().into(),
                    children: kids.shown,
                    hidden,
                });
            }
            if g.hits.len() > limit {
                items.push(Item::More {
                    page: g.page.id.clone(),
                    hidden: g.hits.len() - limit,
                });
            }
        }
    }

    /// Pages co-referenced by the current results, for the co-reference chips (FR-3.10,
    /// LR-1). The store resolves them from the index and counts the hits each would keep
    /// with the same predicate the filter applies, so the chip's number is what it selects.
    fn compute_suggestions(
        &self,
        page: &PageId,
        cx: &mut Context<Self>,
    ) -> Vec<(PageId, String, usize)> {
        let hits: Vec<RefHit> = self
            .refs
            .groups
            .iter()
            .flat_map(|g| g.hits.iter().cloned())
            .collect();
        self.app
            .read(cx)
            .store
            .co_references(&hits, page, &self.query.also)
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.page, c.title, c.hits))
            .collect()
    }

    // ---------------------------------------------------------------- editor events

    fn on_editor_event(
        &mut self,
        editor: &Entity<BlockEditor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = editor.read(cx).target().cloned();
        if let EditorEvent::Restore(caret) = event {
            self.restore(caret, window, cx);
            return;
        }
        if matches!(event, EditorEvent::Blurred) {
            // The editor dropped its target (Escape, navigation, a reference followed, the
            // block went away). The list still holds the focus handle it registered on the
            // old row, so it would keep force-rendering a row now drawn as text; re-splice
            // from the editor's current target instead of waiting for the next rebuild
            // (issue 4). An event from another host's edit has nothing to re-splice.
            let host = cx.entity_id();
            let editing_here = editor.read(cx).target().is_some_and(|t| t.host == host);
            if self.focused_rows > 0 || editing_here {
                self.splice_items(self.items.len(), cx);
                cx.notify();
            }
            return;
        }
        let Some(t) = target else { return };
        if t.host != cx.entity_id() {
            return;
        }
        self.rebuild_if_dirty(cx);
        match event {
            EditorEvent::Split { caret } => {
                let collapsed = self.app.read(cx).store.is_collapsed(&t.block);
                self.run_plan(
                    &t.page,
                    |st| outline::split_block(st, &t.block, *caret, collapsed),
                    window,
                    cx,
                );
            }
            EditorEvent::InsertAbove => {
                self.run_plan(
                    &t.page,
                    |st| outline::insert_above(st, &t.block),
                    window,
                    cx,
                );
            }
            EditorEvent::Indent => {
                let caret = editor.read(cx).cursor();
                self.run_plan_keep(&t, caret, |st| outline::indent(st, &t.block), window, cx);
            }
            EditorEvent::Outdent => {
                let caret = editor.read(cx).cursor();
                let zoom = self.zoom();
                // Never outdent out of the zoomed subtree.
                let at_zoom_root = self.app.update(cx, |app, _| {
                    app.store
                        .page_state(&t.page)
                        .ok()
                        .and_then(|st| st.parent_of(&t.block))
                        == zoom
                }) && zoom.is_some();
                if !at_zoom_root {
                    self.run_plan_keep(&t, caret, |st| outline::outdent(st, &t.block), window, cx);
                }
            }
            EditorEvent::MergeBackward => {
                let zoom = if self.page().as_ref() == Some(&t.page) {
                    self.zoom()
                } else {
                    None
                };
                let collapsed = self.app.read(cx).store.collapsed_set().clone();
                // PG-1 hides the children of a compact attribute row, so Backspace must merge
                // into the previous row the user can *see*, not into a hidden descendant
                // (FR-2.5). Visibility here is the renderer's, not the store's alone.
                let compact = self.zoom().is_none() && self.attributes_are_compact();
                let editing = t.block.clone();
                let now = self.app.read(cx).store.now();
                self.run_plan(
                    &t.page,
                    |st| {
                        outline::merge_into_previous(
                            st,
                            &t.block,
                            zoom.as_ref(),
                            &|b| {
                                collapsed.contains(b)
                                    || is_folded_attribute(st, compact, Some(&editing), b)
                            },
                            now,
                        )
                    },
                    window,
                    cx,
                );
            }
            EditorEvent::Navigate { up, x } => {
                let Some(ix) = self.row_index(&t.block) else {
                    return;
                };
                let rows: Vec<(usize, &RowItem)> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, it)| match it {
                        Item::Row(r) => Some((i, r)),
                        _ => None,
                    })
                    .collect();
                let pos = rows.iter().position(|(i, _)| *i == ix);
                let next = pos.and_then(|p| if *up { p.checked_sub(1) } else { Some(p + 1) });
                if let Some(r) = next.and_then(|n| rows.get(n)).map(|(_, r)| (*r).clone()) {
                    let x = x.unwrap_or(px(0.));
                    let goal = if *up {
                        Goal::LastLine(x)
                    } else {
                        Goal::FirstLine(x)
                    };
                    let caret = if *up { r.text.len() } else { 0 };
                    self.edit(r.page, r.block, Some(caret), Some(goal), window, cx);
                }
            }
            EditorEvent::MoveBlock { up } => {
                let caret = editor.read(cx).cursor();
                let up = *up;
                self.run_plan_keep(
                    &t,
                    caret,
                    |st| {
                        if up {
                            outline::move_up(st, &t.block)
                        } else {
                            outline::move_down(st, &t.block)
                        }
                    },
                    window,
                    cx,
                );
            }
            EditorEvent::ToggleCollapse => self.toggle_collapse(&t.block, cx),
            EditorEvent::ToggleCollapseAll => self.toggle_collapse_all(&t.page, cx),
            EditorEvent::ZoomIn => {
                self.editor.update(cx, |e, cx| e.end(cx));
                if self.page().as_ref() == Some(&t.page) {
                    self.open(t.page.clone(), Some(t.block.clone()), cx);
                    self.focus_first(window, cx);
                } else {
                    cx.emit(PageViewEvent::Open {
                        page: t.page.clone(),
                        zoom: Some(t.block.clone()),
                        focus: None,
                        sidebar: false,
                    });
                }
            }
            EditorEvent::ZoomOut => self.zoom_out(window, cx),
            EditorEvent::DeleteBlock => {
                let now = self.app.read(cx).store.now();
                self.run_plan(
                    &t.page,
                    |st| outline::delete_subtree(st, &t.block, now),
                    window,
                    cx,
                );
            }
            EditorEvent::PasteOutline(text) => {
                let sel = editor.read(cx).selection();
                self.run_plan(
                    &t.page,
                    |st| outline::paste(st, &t.block, sel.clone(), text),
                    window,
                    cx,
                );
            }
            EditorEvent::ExtractRequested => {
                self.editor.update(cx, |e, cx| e.end(cx));
                cx.emit(PageViewEvent::RequestExtract {
                    page: t.page.clone(),
                    block: t.block.clone(),
                });
            }
            EditorEvent::InsertTemplate(template) => {
                let first = self.app.update(cx, |app, cx| {
                    let parent = app
                        .store
                        .page_state(&t.page)
                        .ok()
                        .and_then(|st| st.parent_of(&t.block));
                    let r = app.store.apply_template(
                        &t.page,
                        parent.as_ref(),
                        Some(&t.block),
                        template,
                    );
                    let f = app.report(r).flatten();
                    app.changed(vec![t.page.clone()], true, cx);
                    f
                });
                self.rebuild(cx);
                if let Some(f) = first {
                    self.edit(t.page.clone(), f, None, None, window, cx);
                }
            }
            EditorEvent::Restore(_) | EditorEvent::Blurred => {}
        }
    }

    fn restore(&mut self, caret: &Caret, window: &mut Window, cx: &mut Context<Self>) {
        // The view that hosted the edit restores the caret; with no edit in progress the
        // main pane does.
        let mine = match self.editor.read(cx).target() {
            Some(t) => t.host == cx.entity_id(),
            None => !self.compact,
        };
        if mine {
            self.restore_caret(caret, window, cx);
        }
    }

    /// Put the caret back where an undo/redo says, if that block can be shown here.
    pub fn restore_caret(&mut self, caret: &Caret, window: &mut Window, cx: &mut Context<Self>) {
        self.rebuild(cx);
        // `edit` opens a folded attribute's path when needed, and declines a block this view
        // cannot draw.
        self.edit(
            caret.page.clone(),
            caret.block.clone(),
            Some(caret.offset),
            None,
            window,
            cx,
        );
    }

    fn apply_plan(&mut self, page: &PageId, plan: Plan, cx: &mut Context<Self>) -> Option<Focus> {
        let caret_before = {
            let e = self.editor.read(cx);
            e.target().map(|t| Caret {
                page: t.page.clone(),
                block: t.block.clone(),
                offset: e.cursor(),
            })
        };
        let page = page.clone();
        let focus = self.app.update(cx, |app, cx| {
            let r = app.store.execute_plan(&page, plan, caret_before);
            let f = app.report(r).flatten();
            app.changed(vec![page.clone()], true, cx);
            f
        });
        self.rebuild(cx);
        focus
    }

    fn run_plan(
        &mut self,
        page: &PageId,
        plan: impl FnOnce(&utopia_core::PageState) -> Result<Plan, utopia_core::DomainError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let plan = self
            .app
            .update(cx, |app, _| app.store.page_state(page).ok().map(plan));
        let Some(Ok(plan)) = plan else { return };
        if let Some(f) = self.apply_plan(page, plan, cx) {
            self.edit(page.clone(), f.block, Some(f.offset), None, window, cx);
        }
    }

    /// Run a plan and keep the caret where it was (indent, outdent, move).
    fn run_plan_keep(
        &mut self,
        t: &EditTarget,
        caret: usize,
        plan: impl FnOnce(&utopia_core::PageState) -> Result<Plan, utopia_core::DomainError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let p = self
            .app
            .update(cx, |app, _| app.store.page_state(&t.page).ok().map(plan));
        let Some(Ok(plan)) = p else { return };
        self.apply_plan(&t.page, plan, cx);
        self.edit(
            t.page.clone(),
            t.block.clone(),
            Some(caret),
            None,
            window,
            cx,
        );
    }

    fn toggle_collapse(&mut self, block: &BlockId, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let c = app.store.is_collapsed(block);
            let r = app.store.set_collapsed(block, !c);
            app.report(r);
            app.changed(Vec::new(), false, cx);
        });
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    fn toggle_collapse_all(&mut self, page: &PageId, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let Ok(st) = app.store.page_state(page).cloned() else {
                return;
            };
            let parents: Vec<BlockId> = st
                .live_preorder()
                .into_iter()
                .map(|(id, _)| id)
                .filter(|id| st.has_live_children(id))
                .collect();
            let any_collapsed = parents.iter().any(|p| app.store.is_collapsed(p));
            for p in parents {
                let r = app.store.set_collapsed(&p, !any_collapsed);
                app.report(r);
            }
            app.changed(Vec::new(), false, cx);
        });
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        let Some(z) = self.zoom() else { return };
        let parent = self.app.update(cx, |app, _| {
            app.store
                .page_state(&page)
                .ok()
                .and_then(|st| st.parent_of(&z))
        });
        self.editor.update(cx, |e, cx| e.end(cx));
        self.open(page, parent, cx);
        self.focus_block(&z, None, window, cx);
    }

    fn follow(
        &mut self,
        link: LinkTarget,
        row: Option<&RowItem>,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match link {
            LinkTarget::Page(title) => {
                let page = self.app.update(cx, |app, cx| {
                    let found = app.store.find_page(&title).ok().flatten();
                    match found {
                        Some(p) => Some(p),
                        // An unresolved intent: following it creates the page (FR-4.14).
                        None => {
                            let r = app.store.create_page(&title);
                            let p = app.report(r);
                            app.changed(Vec::new(), true, cx);
                            p
                        }
                    }
                });
                if let Some(page) = page {
                    self.editor.update(cx, |e, cx| e.end(cx));
                    if sidebar && window.modifiers().platform {
                        cx.emit(PageViewEvent::OpenSplit { page });
                    } else {
                        cx.emit(PageViewEvent::Open {
                            page,
                            zoom: None,
                            focus: None,
                            sidebar,
                        });
                    }
                }
            }
            LinkTarget::Block(uid) => {
                if sidebar || row.is_none() {
                    // A chip in a read-only row (a Linked Reference) has no row to expand,
                    // so following it opens the block it points at (FR-4.3).
                    if let Ok(Some(b)) = self.app.read(cx).store.block_row(&BlockId(uid.clone())) {
                        self.editor.update(cx, |e, cx| e.end(cx));
                        cx.emit(PageViewEvent::Open {
                            page: b.page_id,
                            zoom: Some(b.id),
                            focus: None,
                            sidebar,
                        });
                    }
                } else if let Some(row) = row {
                    let key = (row.block.clone(), uid);
                    if !self.open_embeds.remove(&key) {
                        self.open_embeds.insert(key);
                    }
                    self.rebuild(cx);
                    cx.notify();
                }
            }
            LinkTarget::Url(url) if url.starts_with("pdfpage:") => {
                // `{{highlight: …}}`: open the PDF at the highlighted page.
                let rest = &url["pdfpage:".len()..];
                let (hash, page) = rest.split_once('#').unwrap_or((rest, "1"));
                let page = page.parse::<usize>().unwrap_or(1).saturating_sub(1);
                let name = self
                    .app
                    .read(cx)
                    .store
                    .attachment(hash)
                    .ok()
                    .flatten()
                    .map_or_else(|| "PDF".to_string(), |a| a.name);
                // From a highlight's "p. N": its parent is the block that holds the PDF.
                let host = row.and_then(|r| {
                    self.app
                        .read(cx)
                        .store
                        .block_row(&r.block)
                        .ok()
                        .flatten()
                        .and_then(|b| b.parent_id)
                });
                cx.emit(PageViewEvent::OpenPdf {
                    hash: hash.to_string(),
                    name,
                    page: Some(page),
                    host,
                });
            }
            LinkTarget::Url(url) => match url.strip_prefix("blob:") {
                // Attachments open locally (PDFs in the viewer), never as a URL.
                Some(hash) => {
                    let store = &self.app.read(cx).store;
                    let pdf = store
                        .attachment(hash)
                        .ok()
                        .flatten()
                        .filter(|a| a.mime == "application/pdf");
                    match (pdf, store.blob_path(hash)) {
                        (Some(a), Some(_)) => cx.emit(PageViewEvent::OpenPdf {
                            hash: a.hash,
                            name: a.name,
                            page: None,
                            host: row.map(|r| r.block.clone()),
                        }),
                        (None, Some(p)) => cx.open_with_system(&p),
                        _ => {}
                    }
                }
                None => cx.open_url(&url),
            },
        }
        let _ = window;
    }

    fn drop_block(&mut self, d: &DraggedBlock, target: &RowItem, cx: &mut Context<Self>) {
        if d.block == target.block {
            return;
        }
        if d.page != target.page {
            // Drag across pages (P1): the block keeps its id and lands after the target.
            let (dp, tp) = (d.page.clone(), target.page.clone());
            self.app.update(cx, |app, cx| {
                let parent = app
                    .store
                    .page_state(&tp)
                    .ok()
                    .and_then(|st| st.parent_of(&target.block));
                let r = app.store.move_block_to_page(
                    &dp,
                    &d.block,
                    &tp,
                    parent.as_ref(),
                    Some(&target.block),
                );
                app.report(r);
                app.changed(vec![dp.clone(), tp.clone()], true, cx);
            });
            cx.notify();
            return;
        }
        let page = d.page.clone();
        let plan = self.app.update(cx, |app, _| {
            app.store.page_state(&page).ok().map(|st| {
                outline::move_to(
                    st,
                    &d.block,
                    st.parent_of(&target.block).as_ref(),
                    Some(&target.block),
                )
            })
        });
        if let Some(Ok(plan)) = plan {
            self.apply_plan(&page, plan, cx);
            cx.notify();
        }
    }

    fn set_expanded(&mut self, group: PageId, cx: &mut Context<Self>) {
        self.expanded_groups.insert(group);
        if let Some(page) = self.page() {
            let list: Vec<&str> = self.expanded_groups.iter().map(|p| p.as_str()).collect();
            let json = serde_json::to_string(&list).unwrap_or_default();
            let _ = self
                .app
                .read(cx)
                .store
                .local_set(&format!("refs_expanded:{page}"), &json);
        }
        self.rebuild(cx);
        cx.notify();
    }

    fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.header.kind == PageKind::Daily || self.zoom().is_some() {
            self.show_calendar = !self.show_calendar;
            if let Some(d) = self.header.date {
                self.calendar_month = d;
            }
            cx.notify();
            return;
        }
        let title = self.header.title.clone();
        let input = cx.new(|cx| {
            let mut i = LineInput::new("Page title", cx);
            i.set_text(&title, cx);
            i
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, e: &LineInputEvent, window, cx| match e {
                LineInputEvent::Submit { .. } => {
                    let text = input.read(cx).text().to_string();
                    this.finish_rename(&text, window, cx);
                }
                LineInputEvent::Cancel => {
                    this.rename = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        self._subs.push(sub);
        input.update(cx, |i, _| i.focus(window));
        self.rename = Some(input);
        cx.notify();
    }

    fn finish_rename(&mut self, text: &str, _window: &mut Window, cx: &mut Context<Self>) {
        self.rename = None;
        let Some(page) = self.page() else {
            return;
        };
        let survivor = self.app.update(cx, |app, cx| {
            let r = app.store.rename_page(&page, text);
            let s = app.report(r);
            app.changed(vec![page.clone()], true, cx);
            s
        });
        match survivor {
            Some(s) if s != page => cx.emit(PageViewEvent::Open {
                page: s,
                zoom: None,
                focus: None,
                sidebar: false,
            }),
            _ => {
                self.cycle.invalidate(Invalidation::rows());
                cx.notify();
            }
        }
    }

    fn pick_date(&mut self, d: NaiveDate, sidebar: bool, _w: &mut Window, cx: &mut Context<Self>) {
        self.show_calendar = false;
        let page = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(d);
            let p = app.report(r);
            // A visit writes nothing (FR-1.2): no structural change, no index flush.
            app.changed(Vec::new(), false, cx);
            p
        });
        if let Some(page) = page {
            cx.emit(PageViewEvent::Open {
                page,
                zoom: None,
                focus: None,
                sidebar,
            });
        }
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.calendar_month = crate::calendar::shift_month(self.calendar_month, delta);
        cx.notify();
    }

    fn page_action(&mut self, action: &str, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        self.editor.update(cx, |e, cx| e.end(cx));
        self.app.update(cx, |app, cx| {
            let r = match action {
                "delete" => app.store.delete_page(&page).map(|_| ()),
                "restore" => app.store.restore_page(&page).map(|_| ()),
                "rewrite" => app.store.rewrite_references(&page).map(|_| ()),
                _ => Ok(()),
            };
            app.report(r);
            app.changed(Vec::new(), true, cx);
        });
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    /// "Export" in the page menu (PG-2): write Markdown, `graph.json` and `graph.opml` to a
    /// folder the user picks, exactly what `utopia export <graph> <dir>` writes (G4).
    ///
    /// Only the snapshot is taken here, on the UI thread; serializing three formats and
    /// copying the files (attachments included) runs on a worker, so a large graph does not
    /// freeze every window while it is written (rule 10).
    fn export_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Export".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(dir) = paths.into_iter().next() else {
                return;
            };
            let snapshot = match this
                .update_in(cx, |this, _, cx| {
                    this.app.update(cx, |app, _| app.store.export_snapshot())
                })
                .ok()
            {
                Some(Ok(snapshot)) => snapshot,
                Some(Err(e)) => {
                    let _ = this.update_in(cx, |this, _, cx| {
                        this.app.update(cx, |app, cx| {
                            app.last_error = Some(e.to_string());
                            cx.notify();
                        });
                    });
                    return;
                }
                None => return,
            };
            let exported = cx
                .background_executor()
                .spawn(async move { utopia_store::write_export(&snapshot, &dir).map(|n| (n, dir)) })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                this.app.update(cx, |app, cx| {
                    let dir = exported.as_ref().ok().map(|(_, d)| d.clone());
                    if let Some(n) = app.report(exported.map(|(n, _)| n)) {
                        if let Some(dir) = dir {
                            app.notice = Some(format!("Exported {n} pages to {}", dir.display()));
                        }
                    }
                    app.changed(Vec::new(), false, cx);
                });
            });
        })
        .detach();
    }

    // ---------------------------------------------------------------- rendering

    /// `prefix` is how many bytes of the block's text `text` starts after (a code block's
    /// language line, PG-5), so a click still puts the caret at the right offset.
    fn render_rich(
        &self,
        text: &str,
        row_block: &BlockId,
        row: Option<RowItem>,
        min_h: f32,
        prefix: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let rendered = {
            let store = &self.app.read(cx).store;
            rich::render(text, &th, &mut |uid| {
                crate::reference::chip_state(store, uid, row_block)
            })
        };
        let styled =
            StyledText::new(rendered.text.clone()).with_highlights(rendered.highlights.clone());
        let layout = styled.layout().clone();
        let same = rendered.text.as_ref() == text;
        let end = prefix + text.len();
        let row2 = row.clone();
        let links = rendered.links.iter().map(|(r, _)| r.clone()).collect();
        let text_sel = format!("text-{}", row_block.0);
        div()
            .flex_1()
            .min_h(px(min_h))
            .debug_selector(move || text_sel.clone())
            // An editable row shows the I-beam between its links; a read-only one (a query
            // or search hit) is itself clickable, so it keeps its container's pointer.
            .when(row.is_some(), |d| d.cursor_text())
            .child(rich::LinkText::new(styled, links))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                    let ix = layout.index_for_position(e.position);
                    if let Ok(i) = ix {
                        if let Some(link) = rendered.link_at(i).cloned() {
                            cx.stop_propagation();
                            this.follow(link, row2.as_ref(), e.modifiers.shift, window, cx);
                            return;
                        }
                    }
                    if let Some(r) = &row2 {
                        let caret = match ix {
                            Ok(i) | Err(i) if same => (prefix + i).min(end),
                            _ => end,
                        };
                        cx.stop_propagation();
                        this.edit(
                            r.page.clone(),
                            r.block.clone(),
                            Some(caret),
                            None,
                            window,
                            cx,
                        );
                    }
                }),
            )
            .into_any_element()
    }

    fn render_row(&self, ix: usize, r: &RowItem, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let density = crate::appearance::density(cx);
        let lh = density.line_height();
        let vpad = density.block_spacing() / 2.;
        let host = cx.entity_id();
        let editing = self.editor.read(cx).is_editing(host, &r.page, &r.block);
        let indent = r.depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT;
        // PG-3: a heading's own line box, so H3 is not padded out to the body's 24 px.
        let line = match r.kind {
            BlockKind::Heading(level) => crate::theme::heading_metrics(level).1,
            _ => lh,
        };
        // PG-5: a code block whose first line is a language name shows it as the card's
        // label instead of drawing it as the first line of code. View only: the block's
        // text is unchanged, and while editing the raw text (label line included) is shown.
        let code = (r.kind == BlockKind::Code && !editing)
            .then(|| code_language(&r.text))
            .flatten();
        let (shown, prefix) = match &code {
            Some((_, body, skip)) => (*body, *skip),
            None => (r.text.as_str(), 0),
        };
        let row = r.clone();
        let content: AnyElement = if editing {
            self.editor.clone().into_any_element()
        } else {
            self.render_rich(shown, &r.block, Some(row.clone()), line, prefix, cx)
        };
        // PG-4: the collapse arrow belongs to the row the pointer is over. A collapsed
        // block keeps its arrow (it is the only sign that content is hidden); an expanded
        // one shows it only on hover, which leaves the outline quiet to read.
        let hovered = self.hovered_row.as_ref() == Some(&r.block);
        let arrow = {
            let block = r.block.clone();
            let shown = r.has_children && (r.collapsed || hovered);
            div()
                .id(("arrow", ix))
                .w(px(14.))
                .h(px(lh))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.faint)
                .when(r.has_children, |d| {
                    d.cursor_pointer().on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_collapse(&block, cx)),
                    )
                })
                .when(shown, |d| {
                    d.child(
                        div()
                            .debug_selector(move || format!("arrow-{ix}"))
                            .child(if r.collapsed { "▸" } else { "▾" }),
                    )
                })
        };
        let bullet = {
            let page = r.page.clone();
            let block = r.block.clone();
            let label: SharedString = plain_text(&r.text)
                .chars()
                .take(60)
                .collect::<String>()
                .into();
            div()
                .id(("bullet", ix))
                .w(px(18.))
                .h(px(lh))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(r.kind == BlockKind::Numbered, |d| {
                    d.text_size(px(crate::theme::BODY_SIZE))
                        .text_color(th.muted)
                        .child(format!("{}.", r.number))
                })
                .when(r.kind != BlockKind::Numbered, |d| {
                    // PG-4: the bullet is the drag handle. At rest it is the ink dot; under
                    // the pointer it opens into a two-by-three grip, the one affordance in
                    // the row that says "you can move this".
                    d.child(drag_handle(ix, hovered, r, &th, lh))
                })
                .on_drag(
                    DraggedBlock {
                        page: page.clone(),
                        block: block.clone(),
                        label,
                    },
                    |d, _, _, cx| cx.new(|_| DragPreview(d.label.clone())),
                )
                .on_click(cx.listener(move |this, e: &gpui::ClickEvent, window, cx| {
                    this.editor.update(cx, |ed, cx| ed.end(cx));
                    if e.modifiers().shift || Some(&page) != this.page().as_ref() {
                        cx.emit(PageViewEvent::Open {
                            page: page.clone(),
                            zoom: Some(block.clone()),
                            focus: None,
                            sidebar: e.modifiers().shift,
                        });
                    } else {
                        this.open(page.clone(), Some(block.clone()), cx);
                        this.focus_first(window, cx);
                    }
                }))
        };
        let target = r.clone();
        div()
            .id(("row", ix))
            .debug_selector(move || format!("row-{ix}"))
            .relative()
            .flex()
            .flex_row()
            .items_start()
            .pl(px(indent))
            .pr_2()
            .py(px(vpad))
            .when(r.embed > 0, |d| {
                d.bg(th.surface).border_l_2().border_color(th.chip_bg)
            })
            .drag_over::<DraggedBlock>(move |s, _, _, _| s.border_b_2().border_color(th.accent))
            .on_drop(
                cx.listener(move |this, d: &DraggedBlock, _, cx| this.drop_block(d, &target, cx)),
            )
            .on_hover(cx.listener({
                let id = r.block.clone();
                move |this, hovered: &bool, _, cx| {
                    // Leaving a row for the next one can deliver the old row's `false`
                    // after the new row's `true`; only clear our own id.
                    let next = if *hovered {
                        Some(id.clone())
                    } else if this.hovered_row.as_ref() == Some(&id) {
                        None
                    } else {
                        this.hovered_row.clone()
                    };
                    if this.hovered_row != next {
                        this.hovered_row = next;
                        cx.notify();
                    }
                }
            }))
            .child(arrow)
            .child(bullet)
            .when(r.kind == BlockKind::Todo, |d| {
                let row = r.clone();
                // PG-4: a drawn box, not a ☐ glyph from whichever font is in force, with
                // 3 px corners. A done to-do is a filled accent box with a check cut out of
                // it (the page colour), so its state reads at a glance.
                d.child(
                    div()
                        .id(("todo", ix))
                        .w(px(20.))
                        .h(px(lh))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .child(
                            div()
                                .debug_selector(|| format!("todo-{ix}"))
                                .size(px(13.))
                                .rounded(px(3.))
                                .border_1()
                                .border_color(if r.checked { th.accent } else { th.muted })
                                .flex()
                                .items_center()
                                .justify_center()
                                .font_family(crate::theme::MONO)
                                .text_size(px(9.))
                                .when(r.checked, |d| d.bg(th.accent).text_color(th.bg).child("✓")),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_checked(&row, cx))),
                )
            })
            .child({
                // Block types (FR-2.13): headings, quotes, code and done to-dos.
                let mut c = div()
                    .flex_1()
                    .min_w_0()
                    .debug_selector(|| format!("content-{ix}"));
                c = match r.kind {
                    BlockKind::Heading(l) => c
                        .text_size(px(crate::theme::heading_metrics(l).0))
                        .line_height(px(crate::theme::heading_metrics(l).1))
                        // PG-3: semibold, not bold. The title stays 28 px, so an outline
                        // heading never competes with it (ALL-1).
                        .font_weight(gpui::FontWeight::SEMIBOLD),
                    // PG-5: a 2 px accent bar and a warm grey italic, not the interface
                    // muted grey.
                    BlockKind::Quote => c
                        .border_l_2()
                        .border_color(th.accent)
                        .pl_2()
                        .text_color(th.quote)
                        .italic(),
                    // PG-5: a surface card in Menlo, 6 px corners, with the language (when
                    // the block names one) at the top right.
                    BlockKind::Code => {
                        let card = c
                            .bg(th.surface)
                            .border_1()
                            .border_color(th.line)
                            .rounded_md()
                            .px_2()
                            .py_1()
                            .font_family(crate::theme::MONO)
                            .text_size(px(13.));
                        match &code {
                            Some((lang, _, _)) => card.child(
                                div()
                                    .debug_selector(|| format!("code-language-{ix}"))
                                    .flex()
                                    .justify_end()
                                    .pb_0p5()
                                    .text_size(px(crate::theme::CAPTION_SIZE))
                                    .text_color(th.muted)
                                    .child(lang.clone()),
                            ),
                            None => card,
                        }
                    }
                    // PG-4: struck through in the muted colour, which still reads (the audit
                    // measured `faint` under 3:1 on white).
                    BlockKind::Todo if r.checked => c.text_color(th.muted).line_through(),
                    _ => c,
                };
                c.child(content)
            })
            .when(r.depth > MAX_VISUAL_DEPTH, |d| {
                d.child(
                    div()
                        .ml_1()
                        .px_1()
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .bg(th.chip_bg)
                        .rounded_sm()
                        .child(format!("depth {}", r.depth)),
                )
            })
            .into_any_element()
    }

    fn button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        quiet_control(id, label, &th)
    }

    /// The page actions menu. It is drawn deferred and anchored, so opening it floats over
    /// the page instead of taking a row in the column (ALL-3). Order per PG-2: the two
    /// navigation/export actions, then the two that change the page, then the danger item.
    fn page_menu_popup(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let graph = quiet_control("page-menu-graph", "Open in graph", &th)
            .debug_selector(|| "page-menu-graph".into())
            .on_click(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                if let Some(page) = this.page() {
                    this.editor.update(cx, |e, cx| e.end(cx));
                    cx.emit(PageViewEvent::OpenGraph { page });
                }
            }));
        let export = quiet_control("page-menu-export", "Export", &th)
            .debug_selector(|| "page-menu-export".into())
            .on_click(cx.listener(|this, _, window, cx| {
                this.page_menu = false;
                this.export_graph(window, cx);
            }));
        // Daily titles are display-only (rule 6): `start_rename` would open the calendar
        // for them, so Rename is only offered on ordinary pages.
        let rename = (self.header.kind != PageKind::Daily).then(|| {
            quiet_control("page-menu-rename", "Rename", &th)
                .debug_selector(|| "page-menu-rename".into())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.page_menu = false;
                    this.start_rename(window, cx);
                }))
        });
        // FR-3.6: after a rename the old title stays an alias, and this rewrites every
        // reference to it. It only applies when there is an alias to rewrite.
        let rewrite =
            (!self.header.aliases.is_empty() && self.header.kind == PageKind::Page).then(|| {
                quiet_control("page-menu-rewrite", "Rewrite references", &th)
                    .debug_selector(|| "page-menu-rewrite".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page_menu = false;
                        this.page_action("rewrite", cx);
                    }))
            });
        let pin = self.page().map(|page| {
            let pinned = crate::appearance::is_pinned(cx, &page);
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
                cx.emit(PageViewEvent::TogglePin { page: page.clone() });
            }))
        });
        let delete = quiet_danger("page-menu-delete", "Delete page", &th)
            .debug_selector(|| "page-menu-delete".into())
            .on_click(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                this.page_action("delete", cx);
            }));
        div()
            .id("page-menu-popup")
            .debug_selector(|| "page-menu-popup".into())
            // Without `occlude` the menu is drawn over the page but clicks pass through it
            // to whatever is underneath, so its items never fire (found by test).
            .occlude()
            .w(px(180.))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.surface)
            .border_1()
            .border_color(th.line)
            .rounded_md()
            .shadow_md()
            .child(graph.w_full().justify_start())
            .child(export.w_full().justify_start())
            .children(rename.map(|r| r.w_full().justify_start()))
            .children(rewrite.map(|r| r.w_full().justify_start()))
            .children(pin.map(|p| p.w_full().justify_start()))
            // A hairline sets the destructive item apart, always last.
            .child(div().h(px(1.)).my_1().bg(th.line))
            .child(delete.w_full().justify_start())
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let h = self.header.clone();
        let mut col = div().flex().flex_col().gap_1().px_4().pt_4().pb_2();
        if h.missing {
            return col
                .child(
                    div()
                        .text_color(th.muted)
                        .child("This page no longer exists."),
                )
                .into_any_element();
        }
        let top = div()
            .flex()
            .items_center()
            .gap_1()
            .when(self.compact && self.close, |d| {
                d.child(
                    self.button("close", "✕", cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PageViewEvent::Close))),
                )
            })
            .when(h.kind == PageKind::Daily, |d| {
                d.child(self.button("prev-day", "‹", cx).on_click(cx.listener(
                    |this, e: &gpui::ClickEvent, w, cx| {
                        if let Some(date) = this.header.date {
                            this.pick_date(
                                date.pred_opt().unwrap_or(date),
                                e.modifiers().shift,
                                w,
                                cx,
                            );
                        }
                    },
                )))
                .child(self.button("next-day", "›", cx).on_click(cx.listener(
                    |this, e: &gpui::ClickEvent, w, cx| {
                        if let Some(date) = this.header.date {
                            this.pick_date(
                                date.succ_opt().unwrap_or(date),
                                e.modifiers().shift,
                                w,
                                cx,
                            );
                        }
                    },
                )))
            })
            .child(div().flex_1())
            .when(h.deleted, |d| {
                d.child(
                    self.button("restore", "Restore from trash", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.page_action("restore", cx))),
                )
            });
        col = col.child(top);
        if !h.zoom_path.is_empty() {
            let mut crumbs = div()
                .flex()
                .flex_wrap()
                .gap_1()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted);
            crumbs = crumbs.child(
                div()
                    .id("crumb-page")
                    .cursor_pointer()
                    .hover(|s| s.text_color(th.accent))
                    .child(h.title.clone())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(p) = this.page() {
                            this.editor.update(cx, |e, cx| e.end(cx));
                            this.open(p, None, cx);
                        }
                    })),
            );
            let last = h.zoom_path.len() - 1;
            for (i, (id, text)) in h.zoom_path.iter().enumerate() {
                let id = id.clone();
                let label: String = text.chars().take(40).collect();
                crumbs = crumbs.child("›").child(
                    div()
                        .id(("crumb", i))
                        .cursor_pointer()
                        .when(i == last, |d| d.text_color(th.text))
                        .hover(|s| s.text_color(th.accent))
                        .child(if label.is_empty() {
                            "(empty)".to_string()
                        } else {
                            label
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = this.page() {
                                this.editor.update(cx, |e, cx| e.end(cx));
                                this.open(p, Some(id.clone()), cx);
                            }
                        })),
                );
            }
            col = col.child(crumbs);
        }
        let title_text = if self.zoom().is_some() {
            h.zoom_path
                .last()
                .map(|(_, t)| t.clone())
                .unwrap_or_default()
        } else {
            h.title.clone()
        };
        let title_el: AnyElement = match &self.rename {
            Some(input) => div()
                .text_size(px(crate::theme::TITLE_SIZE))
                .line_height(px(34.))
                .border_b_1()
                .border_color(th.accent)
                .child(input.clone())
                .into_any_element(),
            None => div()
                .id("title")
                .debug_selector(|| "page-title".into())
                .text_size(px(crate::theme::TITLE_SIZE))
                .line_height(px(34.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(if h.deleted { th.muted } else { th.text })
                .cursor_pointer()
                .child(if title_text.is_empty() {
                    "Untitled".to_string()
                } else {
                    title_text
                })
                .on_click(cx.listener(|this, _, window, cx| this.start_rename(window, cx)))
                .into_any_element(),
        };
        col = col.child(title_el);
        // PG-2: the page's own controls in one row under the title: alias chips, "+ alias",
        // and the "···" menu kept at the far right of the column.
        if self.zoom().is_none() && !h.deleted {
            let mut left = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted);
            for (i, a) in h.aliases.iter().enumerate() {
                let alias = a.clone();
                left = left.child(
                    div()
                        .id(("alias", i))
                        .debug_selector(move || format!("alias-{i}"))
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(th.chip_bg)
                        .text_color(th.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(th.hover).text_color(th.text))
                        .child(format!("{a} ×"))
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_alias(&alias, cx))),
                );
            }
            left = match &self.alias_input {
                Some(input) => left.child(
                    div()
                        .w(px(160.))
                        .px_1()
                        .border_1()
                        .border_color(th.accent)
                        .rounded_md()
                        .child(input.clone()),
                ),
                None => left.child(
                    self.button("add-alias", "+ alias", cx)
                        .debug_selector(|| "add-alias".into())
                        .on_click(cx.listener(|this, _, window, cx| this.start_alias(window, cx))),
                ),
            };
            let menu = (!h.draft).then(|| {
                div()
                    .relative()
                    .child(
                        self.button("page-menu", "···", cx)
                            .debug_selector(|| "page-menu".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page_menu = !this.page_menu;
                                cx.notify();
                            })),
                    )
                    .children(self.page_menu.then(|| {
                        deferred(
                            anchored()
                                .anchor(Corner::TopRight)
                                .offset(point(px(0.), px(6.)))
                                .child(self.page_menu_popup(cx)),
                        )
                        .with_priority(1)
                    }))
            });
            col = col.child(
                div()
                    .debug_selector(|| "page-toolbar".into())
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(left)
                    .children(menu),
            );
        }
        if self.show_calendar {
            let today = self.app.read(cx).store.today();
            let existing: HashSet<NaiveDate> = HashSet::new();
            col = col.child(
                div()
                    .w(px(200.))
                    .p_2()
                    .border_1()
                    .border_color(th.line)
                    .rounded_md()
                    .child(crate::calendar::render(
                        cx,
                        self.calendar_month,
                        today,
                        &existing,
                        Self::pick_date,
                        Self::shift_month,
                    )),
            );
        }
        if self.zoom().is_none() && !h.attributes.is_empty() {
            // Page metadata header (P1, W7): the page's `Key:: value` blocks at a glance.
            // PG-1 keeps it, and adds the toggle that decides whether the blocks are drawn
            // in full under it or folded to one muted row each.
            let mut grid = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .py_1()
                .text_size(px(crate::theme::BODY_SIZE));
            for (k, v) in &h.attributes {
                let key = k.clone();
                grid = grid.child(
                    div()
                        .debug_selector(move || format!("meta-{key}"))
                        .flex()
                        .gap_2()
                        .child(div().w(px(110.)).text_color(th.muted).child(k.clone()))
                        .child(div().text_color(th.text).child(v.clone())),
                );
            }
            let on = !self.attributes_are_compact();
            let toggle = div()
                .id("attribute-blocks-toggle")
                .debug_selector(|| "attribute-blocks-toggle".into())
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_0p5()
                .rounded_md()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(if on { th.text } else { th.muted })
                .cursor_pointer()
                .hover(|d| d.bg(th.hover).text_color(th.text))
                .child(
                    div()
                        .size(px(10.))
                        .flex_none()
                        .rounded(px(2.))
                        .border_1()
                        .border_color(if on { th.accent } else { th.faint })
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(crate::theme::MONO)
                        .text_size(px(8.))
                        .when(on, |d| d.bg(th.accent).text_color(th.bg).child("✓")),
                )
                .child("Show as blocks")
                .on_click(cx.listener(move |this, _, _, cx| {
                    let next = if on {
                        crate::appearance::AttributeBlocks::Compact
                    } else {
                        crate::appearance::AttributeBlocks::Blocks
                    };
                    this.set_attribute_blocks(next, cx);
                }));
            col = col.child(div().flex().items_start().gap_2().child(grid).child(toggle));
        }
        col.into_any_element()
    }

    fn start_alias(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| LineInput::new("New alias", cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, e: &LineInputEvent, _, cx| match e {
                LineInputEvent::Submit { .. } => {
                    let text = input.read(cx).text().to_string();
                    this.alias_input = None;
                    if let Some(page) = this.page() {
                        this.app.update(cx, |app, cx| {
                            let r = app.store.add_alias(&page, &text);
                            if let Err(e) = &r {
                                app.last_error = Some(e.to_string());
                            }
                            app.changed(vec![page.clone()], true, cx);
                        });
                    }
                    this.cycle.invalidate(Invalidation::rows());
                    cx.notify();
                }
                LineInputEvent::Cancel => {
                    this.alias_input = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        self._subs.push(sub);
        input.update(cx, |i, _| i.focus(window));
        self.alias_input = Some(input);
        cx.notify();
    }

    fn remove_alias(&mut self, alias: &str, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let r = app.store.remove_alias(&page, alias);
            app.report(r);
            app.changed(vec![page.clone()], true, cx);
        });
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    /// "Link" on an unlinked mention (FR-3.11).
    fn promote(&mut self, m: &Mention, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        self.app.update(cx, |app, cx| {
            let r = app.store.promote_mention(&m.block, m.range.clone(), &page);
            app.report(r);
            app.changed(vec![m.page.clone(), page.clone()], true, cx);
        });
        self.unlinked = None;
        self.cycle
            .invalidate(Invalidation::rows() | Invalidation::unlinked());
        cx.notify();
    }

    /// "Link all" in the Unlinked References header (LR-5): one undo step for every mention.
    fn promote_all(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.page() else {
            return;
        };
        if self.unlinked.as_ref().is_none_or(|(_, m)| m.is_empty()) {
            return;
        }
        self.app.update(cx, |app, cx| {
            let linked = app.store.promote_all_mentions(&page);
            app.report(linked);
            app.changed(vec![page.clone()], true, cx);
        });
        self.unlinked = None;
        self.cycle
            .invalidate(Invalidation::rows() | Invalidation::unlinked());
        cx.notify();
    }

    fn toggle_checked(&mut self, r: &RowItem, cx: &mut Context<Self>) {
        let (page, block, checked) = (r.page.clone(), r.block.clone(), !r.checked);
        self.app.update(cx, |app, cx| {
            let res = app
                .store
                .set_block_kind(&page, &block, BlockKind::Todo, checked);
            app.report(res);
            app.changed(vec![page.clone()], true, cx);
        });
        self.cycle.invalidate(Invalidation::rows());
        cx.notify();
    }

    /// LR-1: one control row (filter field, then the date and sort menu buttons) and the
    /// co-reference chips on the line below it. Menus float over the page, so opening one
    /// never moves the references (ALL-3).
    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let sort_label = match self.query.sort {
            RefSort::Title => "Title",
            RefSort::Recency => "Recent",
        };
        let mut chips = div()
            .debug_selector(|| "refs-chips".into())
            .flex()
            .flex_wrap()
            .gap_1()
            .items_center();
        for (i, f) in self.query.also.iter().enumerate() {
            let id = f.clone();
            let title = self
                .filter_titles
                .get(f)
                .cloned()
                .unwrap_or_else(|| f.to_string());
            chips = chips.child(
                div()
                    .id(("filter", i))
                    .debug_selector(move || format!("filter-chip-{i}"))
                    .px_2()
                    .rounded_md()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .bg(th.chip_bg)
                    .text_color(th.accent)
                    .cursor_pointer()
                    .child(format!("{title} ×"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.query.also.retain(|p| *p != id);
                        this.refs_key = None;
                        this.cycle
                            .invalidate(Invalidation::rows() | Invalidation::refs());
                        cx.notify();
                    })),
            );
        }
        for (i, (id, title, count)) in self.suggestions.iter().enumerate() {
            let id = id.clone();
            let t = title.clone();
            // The chip carries its own count ("Ana Lovelace 3"), so a co-reference can be
            // judged before it narrows the results.
            chips = chips.child(
                div()
                    .id(("suggest", i))
                    .debug_selector(move || format!("suggest-chip-{i}"))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .rounded_md()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .border_1()
                    .border_color(th.line)
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(title.clone())
                    .child(
                        div()
                            .debug_selector(move || format!("suggest-count-{i}-{count}"))
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.faint)
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.query.also.push(id.clone());
                        this.filter_titles.insert(id.clone(), t.clone());
                        this.refs_key = None;
                        this.cycle
                            .invalidate(Invalidation::rows() | Invalidation::refs());
                        cx.notify();
                    })),
            );
        }
        let (date_label, date_key) = match self.query.since {
            None => ("Any time".to_string(), "any-time"),
            Some(d) => {
                let today = self.app.read(cx).store.today();
                (
                    format!("Last {} days", (today - d).num_days() + 1),
                    match (today - d).num_days() + 1 {
                        7 => "last-7-days",
                        30 => "last-30-days",
                        _ => "custom",
                    },
                )
            }
        };
        let sort_key = match self.query.sort {
            RefSort::Title => "title",
            RefSort::Recency => "recent",
        };
        let date_open = self.controls_menu == Some(ControlMenu::Date);
        let sort_open = self.controls_menu == Some(ControlMenu::Sort);
        div()
            .debug_selector(|| "refs-controls".into())
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .pb_2()
            .child(
                div()
                    .debug_selector(|| "refs-control-row".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| "refs-filter".into())
                            // 240 px is the preferred width; in the 380 px sidebar or a
                            // narrow split the field gives way so the menu buttons stay
                            // reachable instead of being clipped (LR-1).
                            .w(px(REF_FILTER_WIDTH))
                            .min_w(px(REF_FILTER_MIN_WIDTH))
                            .whitespace_nowrap()
                            .px_2()
                            .border_1()
                            .border_color(th.line)
                            .rounded_md()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .child(self.filter.clone()),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .child(
                                quiet_menu("date-filter", date_label, &th)
                                    .debug_selector(move || format!("refs-date-menu-{date_key}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.controls_menu = if date_open {
                                            None
                                        } else {
                                            Some(ControlMenu::Date)
                                        };
                                        cx.notify();
                                    })),
                            )
                            .children(date_open.then(|| {
                                deferred(
                                    anchored()
                                        .anchor(Corner::TopLeft)
                                        .offset(point(px(0.), px(6.)))
                                        .child(self.controls_menu_popup(ControlMenu::Date, cx)),
                                )
                                .with_priority(1)
                            })),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .child(
                                quiet_menu("sort", sort_label, &th)
                                    .debug_selector(move || format!("refs-sort-menu-{sort_key}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.controls_menu = if sort_open {
                                            None
                                        } else {
                                            Some(ControlMenu::Sort)
                                        };
                                        cx.notify();
                                    })),
                            )
                            .children(sort_open.then(|| {
                                deferred(
                                    anchored()
                                        .anchor(Corner::TopLeft)
                                        .offset(point(px(0.), px(6.)))
                                        .child(self.controls_menu_popup(ControlMenu::Sort, cx)),
                                )
                                .with_priority(1)
                            })),
                    ),
            )
            .child(chips)
            .into_any_element()
    }

    /// The menu behind a reference control (LR-1): plain choices, the current one marked.
    fn controls_menu_popup(
        &self,
        menu: ControlMenu,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let today = self.app.read(cx).store.today();
        let date_span = self.query.since.map(|d| (today - d).num_days() + 1);
        let items = match menu {
            ControlMenu::Date => vec![
                ControlChoice {
                    selector: "refs-date-option-any-time",
                    label: "Any time",
                    days: None,
                    sort: None,
                    selected: date_span.is_none(),
                },
                ControlChoice {
                    selector: "refs-date-option-last-7-days",
                    label: "Last 7 days",
                    days: Some(7),
                    sort: None,
                    selected: date_span == Some(7),
                },
                ControlChoice {
                    selector: "refs-date-option-last-30-days",
                    label: "Last 30 days",
                    days: Some(30),
                    sort: None,
                    selected: date_span == Some(30),
                },
            ],
            ControlMenu::Sort => vec![
                ControlChoice {
                    selector: "refs-sort-option-title",
                    label: "Title",
                    days: None,
                    sort: Some(RefSort::Title),
                    selected: self.query.sort == RefSort::Title,
                },
                ControlChoice {
                    selector: "refs-sort-option-recent",
                    label: "Recent",
                    days: None,
                    sort: Some(RefSort::Recency),
                    selected: self.query.sort == RefSort::Recency,
                },
            ],
        };
        let rows = items.into_iter().map(|choice| {
            let ControlChoice {
                selector,
                label,
                days,
                sort,
                selected,
            } = choice;
            div()
                .id(selector)
                .debug_selector(|| selector.into())
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_2()
                .py_1()
                .rounded_md()
                .text_size(px(crate::theme::CONTROL_SIZE))
                .text_color(th.text)
                .cursor_pointer()
                .hover(|s| s.bg(th.hover))
                .child(label)
                .child(if selected {
                    div().text_color(th.accent).child("✓")
                } else {
                    div()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(days) = days {
                        this.query.since = this
                            .app
                            .read(cx)
                            .store
                            .today()
                            .checked_sub_days(chrono::Days::new(days - 1));
                    } else if let Some(sort) = sort {
                        this.query.sort = sort;
                    } else {
                        this.query.since = None;
                    }
                    this.controls_menu = None;
                    this.refs_key = None;
                    this.cycle
                        .invalidate(Invalidation::rows() | Invalidation::refs());
                    cx.notify();
                }))
        });
        div()
            .id("refs-menu-popup")
            .debug_selector(|| "refs-menu-popup".into())
            .occlude()
            .w(px(160.))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.surface)
            .border_1()
            .border_color(th.line)
            .rounded_md()
            .shadow_md()
            .children(rows)
    }

    /// Build one list item. The list calls this lazily while it lays out a frame, after
    /// `render` has returned, so the frame-read counters have to be armed here too
    /// (issue 6); `in_render` is restored because a handler that makes the list render an
    /// item must not have its own reads counted as frame reads.
    fn render_item(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let was = self.in_render;
        self.in_render = true;
        let item = self.render_item_inner(ix, window, cx);
        self.in_render = was;
        item
    }

    fn render_item_inner(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let Some(item) = self.items.get(ix).cloned() else {
            return div().into_any_element();
        };
        match item {
            Item::Header => self.render_header(cx),
            Item::Row(r) => self.render_row(ix, &r, cx),
            // PG-1: the attribute's block, folded to the one line the header already
            // carries. A click shows the blocks again, which is what the row means.
            Item::Attribute {
                depth,
                key,
                value,
                block,
                children,
            } => {
                div()
                    .id(("attr", ix))
                    .debug_selector(|| format!("attr-{ix}"))
                    .pl(px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.))
                    .pr_2()
                    .py_0p5()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(crate::theme::CONTROL_SIZE))
                    .line_height(px(18.))
                    .text_color(th.muted)
                    .cursor_pointer()
                    .hover(|s| s.text_color(th.text))
                    .child(format!("{key} · {value}"))
                    .when(children > 0, |d| {
                        d.child(div().text_color(th.faint).child(format!("… {children}")))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // Reveal the block and start editing it: the row is a shortcut to
                        // the real block, not a second copy of the data.
                        this.set_attribute_blocks(crate::appearance::AttributeBlocks::Blocks, cx);
                        this.focus_block(&block, Some(0), window, cx);
                    }))
                    .into_any_element()
            }
            Item::Notice { depth, text } => div()
                .pl(px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.))
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .child(text)
                .into_any_element(),
            Item::QueryHeader { depth, label } => div()
                .debug_selector(|| "query-header".into())
                .pl(px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.))
                .pt_1()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .child(label)
                .into_any_element(),
            Item::QueryHit { depth, hit } => {
                let page = hit.page.clone();
                let block = hit.block.clone();
                let text = self.render_rich(
                    &hit.text,
                    &hit.block,
                    None,
                    crate::appearance::density(cx).line_height(),
                    0,
                    cx,
                );
                div()
                    .id(("query-hit", ix))
                    .pl(px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.))
                    .pr_4()
                    .py_0p5()
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(
                        div()
                            .flex_none()
                            .w(px(160.))
                            .overflow_hidden()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .text_color(th.accent)
                            .child(hit.page_title.clone()),
                    )
                    .child(div().flex_1().child(text))
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.end(cx));
                        cx.emit(PageViewEvent::Open {
                            page: page.clone(),
                            zoom: None,
                            focus: Some(block.clone()),
                            sidebar: e.modifiers().shift,
                        });
                    }))
                    .into_any_element()
            }
            Item::Attachment {
                depth,
                blob,
                path,
                size,
                present,
                host,
            } => {
                use utopia_core::attach::BlobKind;
                let pad = px(depth.min(MAX_VISUAL_DEPTH) as f32 * INDENT + 32.);
                let open_path = path.clone();
                let kind = blob.kind;
                match (kind, path.clone()) {
                    (BlobKind::Image, Some(p)) => div()
                        .id(("attachment", ix))
                        .debug_selector(|| "attachment-image".into())
                        .pl(pad)
                        .py_1()
                        .cursor_pointer()
                        .child(
                            gpui::img(p)
                                .max_w(px(480.))
                                .max_h(px(320.))
                                .rounded_md()
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if let Some(p) = &open_path {
                                cx.open_with_system(p);
                            }
                        }))
                        .into_any_element(),
                    _ => {
                        let icon = match kind {
                            BlobKind::Image => "🖼",
                            BlobKind::Pdf => "📄",
                            BlobKind::File => "📎",
                        };
                        let detail = match (present, size) {
                            (false, _) => "waiting for sync".to_string(),
                            (true, Some(n)) => human_size(n),
                            (true, None) => String::new(),
                        };
                        let (hash, name) = (blob.hash.clone(), blob.name.clone());
                        let host = host.clone();
                        div()
                            .id(("attachment", ix))
                            .debug_selector(|| "attachment-chip".into())
                            .pl(pad)
                            .py_0p5()
                            .flex()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(th.chip_bg)
                                    .text_size(px(crate::theme::BODY_SIZE))
                                    .when(path.is_some(), |d| {
                                        d.cursor_pointer().hover(|s| s.bg(th.hover))
                                    })
                                    .child(icon)
                                    .child(blob.name.clone())
                                    .child(
                                        div()
                                            .text_size(px(crate::theme::CAPTION_SIZE))
                                            .text_color(th.faint)
                                            .child(detail),
                                    ),
                            )
                            .on_click(cx.listener(move |_, _, _, cx| match (kind, &open_path) {
                                (BlobKind::Pdf, Some(_)) => cx.emit(PageViewEvent::OpenPdf {
                                    hash: hash.clone(),
                                    name: name.clone(),
                                    page: None,
                                    host: Some(host.clone()),
                                }),
                                (_, Some(p)) => cx.open_with_system(p),
                                _ => {}
                            }))
                            .into_any_element()
                    }
                }
            }
            Item::Empty => div()
                .id("empty")
                .debug_selector(|| "empty".into())
                .px_4()
                .py_1()
                .pl(px(32.))
                .text_color(th.faint)
                .cursor_text()
                .child("Click to start writing")
                .on_click(cx.listener(|this, _, window, cx| this.focus_first(window, cx)))
                .into_any_element(),
            Item::DayHeader {
                date,
                page,
                title,
                caption,
                today,
                first,
            } => div()
                .debug_selector(move || if first { "today-title" } else { "day-title" }.into())
                .when(!first, |d| d.mt_8().border_t_1().border_color(th.line))
                .px_4()
                .pt_4()
                .pb_2()
                .child(
                    // DN-2: the weekday, plus "Today"/"Yesterday". Titles stay display
                    // only: identity is still `daily_date` (rule 6).
                    div()
                        .debug_selector(move || {
                            format!("day-caption-{}", utopia_core::date::iso(date))
                        })
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(if today { th.accent } else { th.muted })
                        .child(caption),
                )
                .child(
                    div()
                        .id(("day-title", ix))
                        .debug_selector(move || {
                            format!("day-date-{}", utopia_core::date::iso(date))
                        })
                        .text_size(px(crate::theme::TITLE_SIZE))
                        .line_height(px(34.))
                        .font_weight(gpui::FontWeight::BOLD)
                        .cursor_pointer()
                        .hover(|s| s.text_color(th.accent))
                        .child(title)
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                            this.editor.update(cx, |ed, cx| ed.end(cx));
                            let page = page.clone().or_else(|| {
                                this.app.update(cx, |app, _| {
                                    let r = app.store.draft_daily(date);
                                    app.report(r)
                                })
                            });
                            if let Some(page) = page {
                                cx.emit(PageViewEvent::Open {
                                    page,
                                    zoom: None,
                                    focus: None,
                                    sidebar: e.modifiers().shift,
                                });
                            }
                        })),
                )
                .into_any_element(),
            // DN-1: a day with nothing on it is one 36 px row. The whole row is the click
            // target; the hint only shows on hover, and no page is written until the first
            // keystroke (FR-1.2).
            Item::DayFolded {
                date,
                weekday,
                title,
            } => {
                let group: SharedString = format!("day-folded-{ix}").into();
                div()
                    .id(("day-folded", ix))
                    .debug_selector(move || format!("day-folded-{}", utopia_core::date::iso(date)))
                    .group(group.clone())
                    .h(px(36.))
                    .px_4()
                    .pl(px(32.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(crate::theme::BODY_SIZE))
                    .text_color(th.muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(weekday)
                    .child(title)
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.faint)
                            .opacity(0.)
                            .group_hover(group, |s| s.opacity(1.))
                            .child("Click to start writing"),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.start_day(date, window, cx)),
                    )
                    .into_any_element()
            }
            Item::DayEmpty { date } => div()
                .id(("day-empty", ix))
                .debug_selector(|| "day-empty".into())
                .px_4()
                .py_1()
                .pl(px(32.))
                .text_color(th.faint)
                .cursor_text()
                .child("Click to start writing")
                .on_click(cx.listener(move |this, _, window, cx| this.start_day(date, window, cx)))
                .into_any_element(),
            Item::JournalEnd => {
                // Coming into view (the list renders a margin past the viewport) loads
                // older days, so the feed scrolls on without a button, like Roam's.
                let this = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = this.update(cx, |v, cx| v.load_older_days(cx));
                });
                div()
                    .debug_selector(|| "journal-end".into())
                    .px_4()
                    .pt_8()
                    .pb(px(120.))
                    .text_size(px(crate::theme::BODY_SIZE))
                    .text_color(th.faint)
                    .child("Loading older days…")
                    .into_any_element()
            }
            Item::JournalExhausted => div()
                .debug_selector(|| "feed-end".into())
                .px_4()
                .pt_8()
                .pb_6()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.faint)
                .child("No earlier notes.")
                .into_any_element(),
            Item::RefsHeader { title, count } => div()
                .debug_selector(|| "refs-header".into())
                .mt_6()
                .mx_4()
                .pt_2()
                .pb_1()
                .border_t_1()
                .border_color(th.line)
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .child(div().debug_selector(|| "refs-title".into()).child(title))
                .child(
                    crate::theme::count_chip(count, &th)
                        .debug_selector(move || format!("refs-count-{count}")),
                )
                .into_any_element(),
            Item::RefsControls => self.render_controls(cx),
            Item::GroupHeader {
                page,
                count,
                scheduled,
            } => {
                let id = page.id.clone();
                div()
                    .id(("group", ix))
                    .flex()
                    .items_center()
                    .mx_2()
                    .px_2()
                    .pt_2()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    // The count follows the name as a chip (LR-2), and there are no icons in
                    // the header: the title and its count are enough.
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .debug_selector(|| format!("group-title-{ix}"))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(th.accent)
                                    .child(page.title.clone()),
                            )
                            .child(
                                crate::theme::count_chip(count, &th)
                                    .debug_selector(|| format!("group-count-{ix}-{count}")),
                            )
                            .when(scheduled, |d| {
                                d.child(
                                    div()
                                        .text_size(px(crate::theme::CAPTION_SIZE))
                                        .text_color(th.tag)
                                        .child("Scheduled for this day"),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.end(cx));
                        cx.emit(PageViewEvent::Open {
                            page: id.clone(),
                            zoom: None,
                            focus: None,
                            sidebar: e.modifiers().shift,
                        });
                    }))
                    .into_any_element()
            }
            Item::Hit {
                hit,
                page_title,
                children,
                hidden,
            } => {
                // The same row the right sidebar draws (RS-1), with the path above it (LR-3)
                // and its first children inline (LR-4).
                let th = th.clone();
                crate::reference::reference_row(
                    "hit",
                    ix,
                    crate::reference::RefRow {
                        hit: &hit,
                        page_title: &page_title,
                        children: &children,
                        hidden,
                    },
                    &th,
                    &self.app.read(cx).store,
                    24.,
                    cx.entity(),
                )
            }
            Item::More { page, hidden } => div()
                .id(("more", ix))
                .px_6()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.accent)
                .cursor_pointer()
                .child(format!("+{hidden} more"))
                .on_click(cx.listener(move |this, _, _, cx| this.set_expanded(page.clone(), cx)))
                .into_any_element(),
            Item::NoRefs => div()
                .px_4()
                .pb_6()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.faint)
                .child("No linked references yet.")
                .into_any_element(),
            Item::UnlinkedHeader { count, open } => div()
                .id("unlinked-header")
                .debug_selector(|| "unlinked-header".into())
                .mt_4()
                .mx_4()
                .pt_2()
                .pb_1()
                .border_t_1()
                .border_color(th.line)
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .cursor_pointer()
                .hover(|s| s.text_color(th.text))
                .child(
                    div()
                        .debug_selector(move || format!("unlinked-arrow-{open}"))
                        .child(if open { "▾" } else { "▸" }),
                )
                .child(div().child("Unlinked References"))
                .children(count.map(|c| {
                    crate::theme::count_chip(c, &th)
                        .debug_selector(move || format!("unlinked-count-{c}"))
                }))
                .child(div().flex_1())
                .children(count.filter(|c| *c > 0).map(|_| {
                    quiet_control("link-all", "Link all", &th)
                        .debug_selector(|| "link-all".into())
                        .on_click(cx.listener(|this, _, _, cx| {
                            // The header itself toggles the section.
                            cx.stop_propagation();
                            this.promote_all(cx);
                        }))
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.unlinked_open = !this.unlinked_open;
                    this.cycle.invalidate(Invalidation::rows());
                    cx.notify();
                }))
                .into_any_element(),
            Item::NoMentions => div()
                .debug_selector(|| "unlinked-empty".into())
                .px_6()
                .py_0p5()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.faint)
                .child("No unlinked mentions")
                .into_any_element(),
            Item::Mention(m) => {
                let th2 = th.clone();
                let mut hl = vec![(
                    m.range.clone(),
                    gpui::HighlightStyle {
                        background_color: Some(th2.highlight_bg),
                        ..Default::default()
                    },
                )];
                hl.retain(|(r, _)| r.end <= m.text.len());
                let m2 = m.clone();
                div()
                    .id(("mention", ix))
                    .debug_selector(|| format!("mention-{ix}"))
                    .px_6()
                    .py_0p5()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(crate::theme::CAPTION_SIZE))
                                    .text_color(th.accent)
                                    .child(m.page_title.clone()),
                            )
                            .child(StyledText::new(m.text.clone()).with_highlights(hl)),
                    )
                    .child(
                        div()
                            .id(("link-mention", ix))
                            .debug_selector(|| format!("link-{ix}"))
                            .px_2()
                            .rounded_sm()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.accent)
                            .border_1()
                            .border_color(th.line)
                            .cursor_pointer()
                            .hover(|s| s.bg(th2.hover))
                            .child("Link")
                            .on_click(cx.listener(move |this, _, _, cx| this.promote(&m2, cx))),
                    )
                    .into_any_element()
            }
        }
    }
}

/// What "Click to start writing" did: the block the caret goes to, and whether the store
/// now has a write coming. Starting a draft (FR-1.2) writes nothing, so its caller must not
/// report a structural change and force an index flush.
struct StartedNote {
    block: BlockId,
    wrote: bool,
}

/// The block a "Click to start writing" prompt writes into (FR-1.2): a new root block after
/// whatever the page already has. Not `ensure_first_block` alone — on a page whose only
/// blocks are attributes that hands back the attribute, and typing rewrites its key (PG-1).
fn start_note(
    store: &mut utopia_store::Store,
    page: &PageId,
) -> Result<StartedNote, utopia_store::StoreError> {
    let (reuse, has_roots) = {
        let st = store.page_state(page)?;
        let roots = st.live_roots();
        // A day's draft starts with one empty block (FR-1.2): write into it, so clicking a
        // blank day stays free of writes until the first keystroke.
        let reuse = match roots.as_slice() {
            [only] if st.block(only).is_some_and(|b| b.text.is_empty()) => Some(only.clone()),
            _ => None,
        };
        (reuse, !roots.is_empty())
    };
    if let Some(block) = reuse {
        return Ok(StartedNote {
            block,
            wrote: false,
        });
    }
    if store.is_draft(page) {
        // A journal template already gave the draft blocks: adding the row the caret goes
        // to must not write the day, so it lives in the draft's memory until a keystroke.
        return Ok(StartedNote {
            block: store.draft_append_block(page)?,
            wrote: false,
        });
    }
    if !has_roots {
        return Ok(StartedNote {
            block: store.ensure_first_block(page)?,
            wrote: true,
        });
    }
    // The borrow of the shard ends here, before the write.
    let after = store.page_state(page)?.last_child(None, false);
    let id = BlockId::new();
    store.execute(
        page,
        vec![utopia_core::Command::InsertBlock {
            id: id.clone(),
            parent: None,
            after,
            text: String::new(),
            kind: BlockKind::Bullet,
        }],
        utopia_store::EditOpts {
            no_undo: true,
            ..Default::default()
        },
    )?;
    Ok(StartedNote {
        block: id,
        wrote: true,
    })
}

/// PG-1's one classification of page metadata: a root `key:: value` block is an attribute,
/// except a code block — the index never records code as an attribute (`attr_row` is skipped
/// for `BlockKind::Code`), so `std::cout << …` stays a code block and not a `std` key.
fn page_attribute(kind: BlockKind, text: &str) -> Option<(String, std::ops::Range<usize>)> {
    if kind == BlockKind::Code {
        return None;
    }
    utopia_core::inline::attribute(text)
}

/// PG-1 folds a root `key:: value` block into one compact row and hides its children, so
/// its subtree is not drawn. The subtree holding `editing` stays open: the row that holds
/// the editor — and its focus handle — must always be rendered, or every keystroke after
/// the fold is lost (AGENTS.md, "the list keeps rendering the row being edited").
///
/// This is the one answer to "is this attribute drawn folded", used by the renderer and by
/// anything that asks which row the user can see (Backspace's merge, FR-2.5).
fn is_folded_attribute(
    st: &utopia_core::PageState,
    compact: bool,
    editing: Option<&BlockId>,
    id: &BlockId,
) -> bool {
    if !compact || st.depth(id) != 0 {
        return false;
    }
    let Some(block) = st.block(id) else {
        return false;
    };
    if page_attribute(block.kind, &block.text).is_none() {
        return false;
    }
    match editing {
        Some(active) => active != id && !st.ancestors(active).contains(id),
        None => true,
    }
}

/// Push a page's visible rows, folding each root `key:: value` attribute block into one
/// compact row (PG-1) when `compact_attributes`. The block and its subtree stay in the
/// shard; folding is only what this frame draws, and the "…" says children went with it.
#[allow(clippy::too_many_arguments)]
fn push_page_rows(
    app: &mut AppState,
    st: &utopia_core::PageState,
    page: &PageId,
    zoom: Option<&BlockId>,
    collapsed: &HashSet<BlockId>,
    open_embeds: &HashSet<(BlockId, String)>,
    compact_attributes: bool,
    editing: Option<&BlockId>,
    out: &mut Vec<Item>,
) {
    let mut folded: Option<usize> = None;
    for (id, depth) in st.visible_rows(zoom, &|b| collapsed.contains(b)) {
        if folded.is_some_and(|d| depth > d) {
            continue;
        }
        folded = None;
        let Some(b) = st.block(&id) else { continue };
        if compact_attributes && is_folded_attribute(st, true, editing, &id) {
            if let Some((key, value)) = page_attribute(b.kind, &b.text) {
                let children = st.live_children(Some(&id)).len();
                out.push(Item::Attribute {
                    depth,
                    key,
                    value: plain_text(&b.text[value]),
                    block: id.clone(),
                    children,
                });
                folded = Some(depth);
                continue;
            }
        }
        let item = RowItem {
            page: page.clone(),
            block: id.clone(),
            text: b.text.clone(),
            depth,
            has_children: st.has_live_children(&id),
            collapsed: collapsed.contains(&id),
            embed: 0,
            kind: b.kind,
            checked: b.checked,
            number: st.index_in_parent(&id, true).map_or(1, |i| i + 1),
        };
        push_with_embeds(
            app,
            item,
            open_embeds,
            collapsed,
            &mut vec![id.clone()],
            out,
        );
    }
}

/// The bullet, which is also the row's drag handle (PG-4). At rest it is the ink dot (an
/// 8 px ring when the block is collapsed); while the pointer is over the row it opens into
/// a two-by-three grip, so the one control that moves a block announces itself.
fn drag_handle(
    ix: usize,
    hovered: bool,
    r: &RowItem,
    th: &crate::theme::Theme,
    lh: f32,
) -> AnyElement {
    let cell = div()
        .w(px(18.))
        .h(px(lh))
        .flex()
        .items_center()
        .justify_center();
    if hovered {
        // A "grip" of six 2 px dots, drawn rather than typed: no font has to carry it.
        let mut grip = div()
            .debug_selector(move || format!("grip-{ix}"))
            .flex()
            .flex_col()
            .gap(px(2.));
        for _ in 0..3 {
            grip = grip.child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(div().size(px(2.)).rounded_full().bg(th.faint))
                    .child(div().size(px(2.)).rounded_full().bg(th.faint)),
            );
        }
        return cell.child(grip).into_any_element();
    }
    cell.child(
        div()
            .debug_selector(move || format!("dot-{ix}"))
            .size(px(6.))
            .rounded_full()
            .bg(th.bullet)
            .when(r.collapsed && r.has_children, |d| {
                d.size(px(8.)).border_2().border_color(th.chip_bg)
            }),
    )
    .into_any_element()
}

/// Language names a code block may announce on its first line (PG-5). A Roam/Markdown
/// fence imports as `"rust\nfn main() { … }"` (see `docs/unslop-audit.md`), so a first line
/// that is exactly one of these names is drawn as the card's label rather than as code.
/// The list is closed: a single word that happens to be code (`return`, `fn`) stays code.
const CODE_LANGUAGES: &[&str] = &[
    "bash",
    "c",
    "c#",
    "c++",
    "clojure",
    "cpp",
    "cs",
    "css",
    "dart",
    "diff",
    "elixir",
    "erlang",
    "f#",
    "go",
    "golang",
    "haskell",
    "html",
    "ini",
    "java",
    "javascript",
    "js",
    "json",
    "jsx",
    "julia",
    "kotlin",
    "lua",
    "makefile",
    "markdown",
    "md",
    "nim",
    "nix",
    "objc",
    "objective-c",
    "ocaml",
    "perl",
    "php",
    "plaintext",
    "py",
    "python",
    "r",
    "ruby",
    "rust",
    "scala",
    "sh",
    "shell",
    "sql",
    "swift",
    "tex",
    "text",
    "toml",
    "ts",
    "tsx",
    "typescript",
    "vb",
    "xml",
    "yaml",
    "yml",
    "zig",
    "zsh",
];

/// `(language, the rest of the block, bytes skipped)` when a code block's first line names
/// its language (PG-5). The block keeps its text; this only decides what is drawn.
fn code_language(text: &str) -> Option<(String, &str, usize)> {
    let (first, rest) = text.split_once('\n')?;
    let word = first.trim();
    if word.is_empty()
        || word.len() > 16
        || !CODE_LANGUAGES.iter().any(|l| l.eq_ignore_ascii_case(word))
    {
        return None;
    }
    let skip = first.len() + 1;
    Some((word.to_string(), rest, skip))
}

/// Uids of `{{embed: ((uid))}}` blocks, which always render expanded (P1 block embeds).
fn embed_uids(text: &str) -> Vec<String> {
    fn walk(nodes: &[utopia_core::inline::Inline], out: &mut Vec<String>) {
        for n in nodes {
            match n {
                utopia_core::inline::Inline::Embed { uid, .. } => out.push(uid.clone()),
                utopia_core::inline::Inline::Emph { children, .. } => walk(children, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&utopia_core::inline::parse(text), &mut out);
    out
}

/// Push a row and, after it, any expanded transclusions of the blocks it references
/// (FR-4.3–4.5): nested rows edit the original block, cycles stop at depth 3.
fn push_with_embeds(
    app: &mut AppState,
    item: RowItem,
    open: &HashSet<(BlockId, String)>,
    collapsed: &HashSet<BlockId>,
    chain: &mut Vec<BlockId>,
    out: &mut Vec<Item>,
) {
    let embedded = embed_uids(&item.text);
    let refs: Vec<String> = extract_refs(&item.text)
        .into_iter()
        .filter(|r| r.kind == utopia_core::inline::RefKind::Block)
        .map(|r| r.target)
        .filter(|uid| embedded.contains(uid) || open.contains(&(item.block.clone(), uid.clone())))
        .collect();
    let depth = item.depth;
    let embed = item.embed;
    let query = utopia_core::query::query_block(&item.text);
    let mut blobs = utopia_core::attach::blob_refs(&item.text);
    let host_block = item.block.clone();
    // `{{pdf: …}} [name.pdf](blob:…)` names one file twice; show it once.
    let mut seen = HashSet::new();
    blobs.retain(|b| seen.insert(b.hash.clone()));
    out.push(Item::Row(item));
    for blob in blobs {
        let path = app.store.blob_path(&blob.hash);
        let size = app
            .store
            .attachment(&blob.hash)
            .ok()
            .flatten()
            .map(|a| a.size);
        let present = app.store.has_blob(&blob.hash);
        out.push(Item::Attachment {
            depth: depth + 1,
            blob,
            path,
            size,
            present,
            host: host_block.clone(),
        });
    }
    match query {
        Some(Ok(expr)) => match app.store.run_query(&expr, MAX_QUERY_HITS) {
            Ok(r) => {
                let shown = if r.total > r.hits.len() {
                    format!("{} of {}", r.hits.len(), r.total)
                } else {
                    r.total.to_string()
                };
                out.push(Item::QueryHeader {
                    depth: depth + 1,
                    label: format!("Query {expr} · {shown} results").into(),
                });
                out.extend(r.hits.into_iter().map(|hit| Item::QueryHit {
                    depth: depth + 1,
                    hit,
                }));
            }
            Err(e) => out.push(Item::Notice {
                depth: depth + 1,
                text: format!("Query failed: {e}").into(),
            }),
        },
        Some(Err(e)) => out.push(Item::Notice {
            depth: depth + 1,
            text: format!("Query syntax: {e}").into(),
        }),
        None => {}
    }
    for uid in refs {
        let target = BlockId(uid.clone());
        if chain.contains(&target) || embed >= MAX_EMBED_DEPTH {
            out.push(Item::Notice {
                depth: depth + 1,
                text: "↻ cyclic reference".into(),
            });
            continue;
        }
        let Ok(Some(row)) = app.store.block_row(&target) else {
            out.push(Item::Notice {
                depth: depth + 1,
                text: format!("Missing block (({uid}))").into(),
            });
            continue;
        };
        let Ok(st) = app.store.page_state(&row.page_id).cloned() else {
            continue;
        };
        if !st.is_live(&target) {
            out.push(Item::Notice {
                depth: depth + 1,
                text: "deleted block".into(),
            });
            continue;
        }
        let mut count = 0;
        let mut rows: Vec<(BlockId, usize)> = vec![(target.clone(), 0)];
        rows.extend(
            st.visible_rows(Some(&target), &|b| collapsed.contains(b))
                .into_iter()
                .map(|(id, d)| (id, d + 1)),
        );
        for (id, d) in rows {
            count += 1;
            if count > MAX_EMBED_BLOCKS {
                break;
            }
            let Some(b) = st.block(&id) else { continue };
            let child = RowItem {
                page: row.page_id.clone(),
                block: id.clone(),
                text: b.text.clone(),
                depth: depth + 1 + d,
                has_children: st.has_live_children(&id),
                collapsed: collapsed.contains(&id),
                embed: embed + 1,
                kind: b.kind,
                checked: b.checked,
                number: st.index_in_parent(&id, true).map_or(1, |i| i + 1),
            };
            chain.push(target.clone());
            push_with_embeds(app, child, open, collapsed, chain, out);
            chain.pop();
        }
    }
}

impl Render for PageView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.in_render = true;
        self.rebuild_if_dirty(cx);
        self.in_render = false;
        let th = theme(cx).clone();
        let view = cx.entity();
        // DN-3: the day the column is inside stays at the top of the column while a long
        // day scrolls. Reading the offset and one heading is all `render` does for it.
        let sticky = self.sticky_heading().cloned();
        div()
            .size_full()
            .bg(th.bg)
            .relative()
            .child(
                list(self.list.clone(), move |ix, window, cx| {
                    // List items are laid out as roots, which shrink to their content; a
                    // full-width wrapper gives margins, spacers and rules the column's width.
                    // The column stops at a readable line length and stays centred, like Roam.
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(
                            div().w_full().max_w(px(READABLE_WIDTH)).child(
                                view.update(cx, |this, cx| this.render_item(ix, window, cx)),
                            ),
                        )
                        .into_any_element()
                })
                .size_full(),
            )
            .when_some(sticky, |d, h| {
                // Drawn after the list so it paints above the rows, and with no hitbox of
                // its own, so a click at the top of the column still reaches the block.
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .debug_selector(|| "sticky-day".into())
                                .w_full()
                                .max_w(px(READABLE_WIDTH))
                                .h(px(STICKY_HEIGHT))
                                .px_4()
                                .pt_1()
                                .pb_1()
                                .bg(th.bg)
                                .border_b_1()
                                .border_color(th.line)
                                .child(
                                    div()
                                        .text_size(px(crate::theme::CAPTION_SIZE))
                                        .line_height(px(20.))
                                        .text_color(if h.today { th.accent } else { th.muted })
                                        .child(h.caption.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(crate::theme::TITLE_SIZE))
                                        .line_height(px(34.))
                                        .font_weight(gpui::FontWeight::BOLD)
                                        .child(h.title.clone()),
                                ),
                        ),
                )
            })
    }
}

/// DN-2: the caption above a day in the feed: "Thursday · Today", "Wednesday ·
/// Yesterday", or just "Monday". Display only (§10.3: a day's identity is its date).
fn day_caption(date: NaiveDate, today: NaiveDate) -> String {
    let weekday = utopia_core::date::weekday_name_of(date);
    match (today - date).num_days() {
        0 => format!("{weekday} · Today"),
        1 => format!("{weekday} · Yesterday"),
        _ => weekday.to_string(),
    }
}

fn human_size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.0} KB", n as f64 / 1024.),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FR-1.2: a laid-out sentinel asks for exactly one batch. A second layout before the
    /// rebuild runs must not ask again — the pending request lives in the state, so there
    /// is no separate `loading` flag to leave behind. The request moves the window by
    /// *calendar* days, not by pages: a blank day has no page to count
    /// (journal-feed-pagination, "two units for how much is loaded").
    #[test]
    fn feed_requests_one_batch_at_a_time() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let shown = today - chrono::Duration::days(JOURNAL_BATCH as i64 - 1);
        let idle = Feed::Idle {
            shown_through: shown,
        };
        assert!(idle.sentinel(), "an idle feed draws its end row");
        let loading = idle.request().expect("an idle feed asks for more");
        assert_eq!(
            loading,
            Feed::Loading {
                shown_through: shown,
                want_through: shown - chrono::Duration::days(JOURNAL_BATCH as i64),
            }
        );
        assert_eq!(
            loading.want_through(),
            shown - chrono::Duration::days(JOURNAL_BATCH as i64)
        );
        assert_eq!(
            loading.shown_through(),
            shown,
            "nothing moves until the rebuild"
        );
        assert_eq!(
            (shown - loading.want_through()).num_days(),
            JOURNAL_BATCH as i64,
            "one request moves the calendar window by a batch, pages or not"
        );
        assert_eq!(loading.request(), None, "one request in flight at a time");
        assert!(!loading.sentinel());
    }

    /// FR-1.2: the feed is exhausted once the oldest daily page is inside the loaded
    /// window. `settle` recomputes it from the store on every rebuild, so a page created
    /// or synced in later re-opens the feed rather than latching it shut.
    #[test]
    fn feed_exhausts_at_the_oldest_daily_page() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let shown = today - chrono::Duration::days(29);
        let idle = Feed::Idle {
            shown_through: shown,
        };
        // Older pages exist beyond the window: keep the sentinel.
        let older = idle
            .clone()
            .settle(shown, Some(today - chrono::Duration::days(40)));
        assert_eq!(older, idle);
        assert!(older.sentinel());
        // The oldest page is at or inside the window: no older calendar day can hold
        // anything, so the sentinel goes away.
        for oldest in [shown, today] {
            let next = idle.clone().settle(shown, Some(oldest));
            assert_eq!(
                next,
                Feed::Exhausted {
                    shown_through: shown
                }
            );
            assert!(!next.sentinel());
            assert_eq!(next.request(), None, "an exhausted feed asks for nothing");
        }
        assert_eq!(
            idle.settle(shown, None),
            Feed::Exhausted {
                shown_through: shown
            },
            "a graph with no daily page at all is exhausted too"
        );
    }

    /// FR-1.2: the mode pairs the page with the zoom and the feed, so "a feed without a
    /// page" (the transient `open_journal` used to pass through) and "a zoom without a
    /// page" cannot be held at all (page-view-mode issue 5). `rebuild_rows` matches on the
    /// mode, so it cannot reach a branch where the page it needs is missing.
    #[test]
    fn a_mode_always_names_the_page_it_draws() {
        let page = PageId("page".into());
        let root = BlockId("block".into());
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let journal = Journal {
            feed: Feed::Exhausted {
                shown_through: today,
            },
            pages: Vec::new(),
            refs: HashMap::new(),
            refs_version: 0,
        };

        assert_eq!(Mode::Empty.page(), None);
        assert_eq!(Mode::Empty.zoom(), None);
        assert!(Mode::Empty.journal().is_none());

        let plain = Mode::Page { page: page.clone() };
        assert_eq!(plain.page(), Some(&page));
        assert_eq!(plain.zoom(), None, "a whole page is not zoomed");
        assert!(plain.journal().is_none());

        let zoomed = Mode::Zoomed {
            page: page.clone(),
            root: root.clone(),
        };
        assert_eq!(zoomed.page(), Some(&page));
        assert_eq!(zoomed.zoom(), Some(&root));
        assert!(zoomed.journal().is_none());

        let feed = Mode::Journal {
            today: page.clone(),
            journal,
        };
        assert_eq!(feed.page(), Some(&page), "the feed always shows today");
        assert_eq!(feed.zoom(), None, "the feed is never zoomed");
        assert!(feed.journal().is_some() && feed.is_journal());
    }

    /// PG-5: a fenced code block's first line is its language, shown as the card's label.
    /// Anything else stays the first line of code, so a block of real code is never eaten.
    #[test]
    fn a_code_blocks_language_is_its_first_line_when_it_names_one() {
        let (lang, body, skip) = code_language("rust\nfn main() {}").expect("rust is a language");
        assert_eq!(lang, "rust");
        assert_eq!(body, "fn main() {}");
        assert_eq!(&"rust\nfn main() {}"[skip..], body);

        // Case and a trailing space on the label line are tolerated.
        let (lang, body, _) = code_language("Rust  \nfn main() {}").unwrap();
        assert_eq!((lang.as_str(), body), ("Rust", "fn main() {}"));

        // A label with no code under it still labels the (empty) card.
        assert_eq!(
            code_language("sql\n").map(|(l, b, _)| (l, b)),
            Some(("sql".into(), ""))
        );

        // Ordinary code, prose and unknown words keep every line they have.
        for text in [
            "fn main() {}",
            "return\n1",
            "the quick brown fox\njumps",
            "",
            "rustup update\nx",
            "python3\nx",
            "averylongwordthatisnotalanguagename\ncode",
        ] {
            assert_eq!(code_language(text), None, "{text:?}");
        }
        assert!(!CODE_LANGUAGES.is_empty());
    }

    /// DN-2: "Thursday · Today", "Wednesday · Yesterday", "Monday".
    #[test]
    fn day_captions_name_the_weekday_and_the_neighbours() {
        // Thursday, November 5th, 2026.
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        assert_eq!(day_caption(today, today), "Thursday · Today");
        assert_eq!(
            day_caption(today.pred_opt().unwrap(), today),
            "Wednesday · Yesterday"
        );
        assert_eq!(
            day_caption(today - chrono::Duration::days(3), today),
            "Monday"
        );
        // Days further back get the weekday alone; the feed never shows a future day.
        assert_eq!(
            day_caption(today - chrono::Duration::days(30), today),
            "Tuesday"
        );
        assert_eq!(day_caption(today.succ_opt().unwrap(), today), "Friday");
    }
}
