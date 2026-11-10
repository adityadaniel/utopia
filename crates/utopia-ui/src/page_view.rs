//! A page (or zoomed block) as a virtualized outline plus its Linked References
//! (§6, §7, §8). Used for the main pane and for every right-sidebar pane.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{NaiveDate, TimeZone};
use gpui::{
    anchored, deferred, div, list, point, prelude::*, px, AnyElement, Context, Corner, Div, Entity,
    EventEmitter, FocusHandle, Focusable, ListAlignment, ListOffset, ListState, MouseButton,
    MouseDownEvent, SharedString, StyledText, Subscription, Window,
};
use utopia_core::inline::{extract_refs, plain_text};
use utopia_core::outline::{self, Focus, Plan};
use utopia_core::title::normalize_title;
use utopia_core::undo::Caret;
use utopia_core::{BlockId, BlockKind, Command, PageId, PageKind};
use utopia_store::{LinkedRefs, Mention, PageRow, RefChildren, RefHit, RefQuery, RefSort};

use crate::app_state::{AppState, Changed, IndexUpdated, PageReplaced};
use crate::editor::{BlockEditor, EditTarget, EditorEvent};
use crate::field::Goal;
use crate::icons::{icon, Icon};
use crate::line_input::{LineInput, LineInputEvent};
use crate::pane_stack::Click;
use crate::rich::{self, LinkTarget};
use crate::theme::{
    outline_button, quiet_control, quiet_danger, quiet_menu, ref_type_label, theme, today_pill,
    AVATAR_SIZE, AVATAR_TEXT, BULLET_SIZE, BULLET_SLOT, CARET_SIZE, CARET_SLOT, CHILD_INDENT,
    COLLAPSED_RING, COMPACT_INDENT, CONTENT_MAX_WIDTH, CONTENT_PADDING_BOTTOM, CONTENT_PADDING_TOP,
    CONTENT_PADDING_TOP_PAGE, CONTENT_PADDING_X, DAY_CAPTION_LINE, DAY_GAP, DAY_HEADER_GAP,
    DAY_HEADER_PAD_TOP, DAY_TITLE, EMPTY_DAY_GAP, EMPTY_DAY_HEIGHT, EMPTY_DAY_WEEKDAY_WIDTH, LABEL,
    LABEL_MONO, MAX_VISUAL_DEPTH, MENU_OFFSET, PAGE_MENU_WIDTH, PAGE_REFS_GAP,
    PROPERTY_GAP_BELOW_TITLE, PROPERTY_HOVER_BLEED, PROPERTY_ICON, PROPERTY_KEY_WIDTH,
    PROPERTY_KEY_WIDTH_COMPACT, PROPERTY_PADDING_Y, PROPERTY_ROW_MIN_HEIGHT, STATUS_DOT,
    STATUS_PILL_HEIGHT, STATUS_PILL_PADDING_X, THREAD_OFFSET, TODO_CHECK_SIZE, TODO_GAP, TODO_SIZE,
    UNLINKED_ROW_HEIGHT,
};

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
/// The reference filter field's preferred width (LR-1); it no longer stretches the column.
const REF_FILTER_WIDTH: f32 = 240.;
/// The narrowest the field gets before the pane is too small to show the controls at all.
const REF_FILTER_MIN_WIDTH: f32 = 90.;
/// Height of the pinned day heading (DN-3). It is exactly the day header's own height —
/// its top padding, the caption line, the date line, the gap below the title and the
/// hairline — so the pinned copy replaces the real one without duplicating it.
const STICKY_HEIGHT: f32 =
    DAY_HEADER_PAD_TOP + DAY_CAPTION_LINE + DAY_TITLE.line + DAY_HEADER_GAP + 1.;

#[derive(Debug, Clone)]
pub enum PageViewEvent {
    Open {
        page: PageId,
        zoom: Option<BlockId>,
        focus: Option<BlockId>,
        /// Shift (and not ⌘): stack the page in the right sidebar (RS-5).
        sidebar: bool,
        /// Plain or ⌘/Ctrl: which stacked-panes rule applies (DESIGN.md §4.3, SP-2/SP-3).
        /// ⌘⇧ is `Secondary` too: it used to open a split pane, which the stack replaced.
        click: Click,
        /// This view's own page is gone and `page` took its place (a rename merged it,
        /// §8.6): the pane shows `page` where it is, nothing is added.
        replace: bool,
    },
    Close,
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
        /// The feed day whose panel this is (`None` on a page view).
        date: Option<NaiveDate>,
        collapsed: bool,
    },
    RefsControls,
    /// The panel's rounded bottom edge (the flat list draws the border item by item).
    RefsEnd,
    GroupHeader {
        page: PageRow,
        count: usize,
        scheduled: bool,
        /// The first group has no divider above it.
        first: bool,
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
        /// The weekday label above the date, upper case (DESIGN.md §3.5).
        weekday: SharedString,
        /// Today carries a Today pill beside its weekday (§3.5).
        today: bool,
        first: bool,
    },
    /// A day with nothing on it (DESIGN.md §3.6): one 44 px row. Consecutive empty days
    /// are separate list items so the list still virtualizes them one row at a time; they
    /// read as one block because only the first of a run carries the 40 px gap and the
    /// single divider, and the rest sit 4 px apart.
    EmptyDay {
        date: NaiveDate,
        weekday: SharedString,
        title: SharedString,
        /// First of a run of empty days.
        first: bool,
    },
    /// The click-to-start row under an open day that has no blocks yet (a day with
    /// only references, or only attribute rows, keeps its section).
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
    /// One past the day's last item, so an empty-day group (whose rows carry their own
    /// dates) does not pin the previous day's heading over it.
    end: usize,
    date: NaiveDate,
    title: SharedString,
    weekday: SharedString,
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
    /// The page's leading consecutive `key:: value` blocks: the properties bar
    /// (DESIGN.md §3.8). Exactly these are drawn in the bar and nowhere else.
    properties: Vec<PropertyBlock>,
    /// "First mentioned <date>": the page's creation date, when the store knows it.
    first_mentioned: Option<(NaiveDate, String)>,
}

/// A leading `key:: value` block drawn in the properties bar (DESIGN.md §3.8).
#[derive(Debug, Clone)]
struct PropertyBlock {
    block: BlockId,
    /// The key exactly as typed (lowercase is the display form).
    key: String,
    /// The value's raw markup, after `key:: `.
    value: String,
    kind: PropertyKind,
}

/// How a property's value is drawn (DESIGN.md §3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropertyKind {
    /// `status`: a pill with the in-progress dot.
    Status,
    /// Exactly one page link: an avatar and the link.
    Linked,
    /// Plain text, with links drawn as links.
    Plain,
}

/// The page's leading consecutive property blocks. The first non-property root ends the
/// bar; a property lower in the outline is an ordinary bullet (§3.8 rules 1-3).
fn leading_properties(st: &utopia_core::PageState) -> Vec<PropertyBlock> {
    let mut out = Vec::new();
    for root in st.live_roots() {
        let Some(b) = st.block(&root) else { break };
        let Some((key, range)) = page_attribute(b.kind, &b.text) else {
            break;
        };
        // The bar draws a row, not a subtree. A property with live children therefore ends
        // the bar and stays an outline block, children included: skipping it would hide
        // content nothing could focus or reveal (DESIGN.md §3.8 rule 1, read to preserve
        // data rather than to orphan a child).
        if st.has_live_children(&root) {
            break;
        }
        let value = b.text[range].to_string();
        let kind = if key.eq_ignore_ascii_case("status") {
            PropertyKind::Status
        } else {
            let refs = utopia_core::inline::extract_refs(&value);
            if refs.len() == 1
                && refs[0].is_page_like()
                && value.trim() == &value[refs[0].range.clone()]
            {
                PropertyKind::Linked
            } else {
                PropertyKind::Plain
            }
        };
        out.push(PropertyBlock {
            block: root,
            key,
            value,
            kind,
        });
    }
    out
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
    /// The appearance preference this view mirrors (`ref_children_limit`) changed.
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
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .text_color(th.ink)
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
    /// Drawn as a stacked pane (DESIGN.md §4.3): the workspace's 44 px pane header sits
    /// above, so no top padding, 72 px below, and the smaller pane title sizes. Local view
    /// state (rule 9).
    in_pane: bool,
    /// The page the next pane shows, so its links here get a ring (§4.3).
    next_link: Option<NextLink>,
    /// On screen. A stack entry hidden by Single mode (§4.4) keeps its view so coming back
    /// keeps its scroll and folds, but it only marks itself stale when the store moves;
    /// [`PageView::set_active`] catches up, outside a frame (rule 10).
    active: bool,
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
    /// Normalized title and aliases of this page: a reference to it is bold ink, not a
    /// link (DESIGN.md §3.7).
    self_titles: Vec<String>,
    /// The properties-bar row being edited (DESIGN.md §3.8). The bar renders that block as
    /// its raw text; an empty commit deletes it.
    property_edit: Option<BlockId>,
    /// The empty property "Add property" inserted, until its first commit (it is not a
    /// `key:: value` block yet, so the bar draws it while it is being edited).
    pending_property: Option<BlockId>,
    /// The compact pane's own page menu (a right-sidebar or split card). The main pane's
    /// page actions live in the workspace top bar instead (DESIGN.md §3.3).
    page_menu: bool,
    /// Feed days whose Linked references panel is collapsed (DESIGN.md §3.7). Local view
    /// state only (rule 9).
    collapsed_ref_days: HashSet<NaiveDate>,
    /// The page header's "···" menu is open (ALL-3).
    /// The row the pointer is over, so its bullet can read as a drag handle and its
    /// collapse arrow can appear (PG-4). Local UI state, never persisted (rule 9).
    hovered_row: Option<BlockId>,
    /// How attribute blocks are drawn (PG-1), mirrored from `settings.json` so a change
    /// made in Settings rebuilds this page's rows.
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

/// The page open in the next pane, as the links in this one spell it (DESIGN.md §4.3:
/// "in pane i, any link whose target is open in pane i + 1 gets a ring"). The workspace
/// builds it from the store when the stack changes; a frame only compares strings.
#[derive(Debug, Clone, PartialEq)]
pub struct NextLink {
    /// `normalize_title` of the page's title and every alias (§10.3.1 resolver order).
    pub keys: Vec<String>,
    /// The day, when the page is a daily page: `[[2026-11-10]]` names it as well.
    pub date: Option<NaiveDate>,
    /// The block, when the next pane is zoomed into one: a `((uid))` chip names it.
    pub block: Option<BlockId>,
    pub date_format: utopia_core::date::DateFormat,
}

impl NextLink {
    pub fn matches(&self, target: &LinkTarget) -> bool {
        match target {
            LinkTarget::Page(title) => {
                self.keys.contains(&normalize_title(title))
                    || self.date.is_some_and(|d| {
                        utopia_core::date::parse_unambiguous_date(title, &self.date_format)
                            == Some(d)
                    })
            }
            LinkTarget::Block(uid) => self.block.as_ref().is_some_and(|b| &b.0 == uid),
            LinkTarget::Url(_) => false,
        }
    }
}

/// The stacked-panes click a pointer press means (DESIGN.md §4.3): ⌘ on macOS, Ctrl
/// elsewhere, with or without Shift.
pub fn click_of(m: &gpui::Modifiers) -> Click {
    if m.secondary() {
        Click::Secondary
    } else {
        Click::Plain
    }
}

/// The open a link clicked with `m` asks for: Shift alone stacks the page in the right
/// sidebar (RS-5); ⌘⇧ goes to the pane stack like ⌘ (it used to open a split pane).
pub fn open_event(
    page: PageId,
    zoom: Option<BlockId>,
    focus: Option<BlockId>,
    m: &gpui::Modifiers,
) -> PageViewEvent {
    let click = click_of(m);
    PageViewEvent::Open {
        page,
        zoom,
        focus,
        sidebar: m.shift && click == Click::Plain,
        click,
        replace: false,
    }
}

impl crate::reference::RefRowHost for PageView {
    fn open_reference(
        &mut self,
        page: PageId,
        block: BlockId,
        sidebar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.emit_open(page, None, Some(block), sidebar, window, cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |e, cx| e.end(cx));
        self.emit_open(page, None, block, sidebar, window, cx);
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
                    // An entry out of sight only remembers it is stale (`set_active`).
                    if !this.active {
                        return;
                    }
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
                if !this.active {
                    return;
                }
                // DESIGN.md §3.7: unlinked references are searched only while the row is
                // expanded. A closed row costs nothing when the index moves.
                if this.unlinked_open {
                    this.refresh_unlinked(cx);
                }
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
                if this.active && this.cycle.dirty() {
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
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        Self {
            app,
            editor,
            mode: Mode::Empty,
            compact,
            close,
            in_pane: false,
            next_link: None,
            active: true,
            items: Vec::new(),
            list,
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
            self_titles: Vec::new(),
            property_edit: None,
            pending_property: None,
            page_menu: false,
            collapsed_ref_days: HashSet::new(),
            hovered_row: None,
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

    /// Draw as a stacked pane (§4.3) or as the full-width page (§4.4 Single mode).
    pub fn set_in_pane(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.in_pane != on {
            self.in_pane = on;
            cx.notify();
        }
    }

    pub fn in_pane(&self) -> bool {
        self.in_pane
    }

    /// One of this view's own text fields (the title rename, the alias field, the
    /// reference filter) has the keyboard. The block editor is shared and tracked by its
    /// host instead. The workspace keeps such a view drawn and hands the keyboard back
    /// before hiding it: a focused element that is not drawn swallows every key.
    pub fn holds_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        self.rename
            .iter()
            .chain(self.alias_input.iter())
            .chain(std::iter::once(&self.filter))
            .any(|i| i.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Put the view on or off screen (Single mode hides all but the last entry). Coming
    /// back rebuilds what went stale while it was away, here and not in the next frame.
    pub fn set_active(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.active == on {
            return;
        }
        self.active = on;
        if on {
            self.sync_preferences(cx);
            if self.unlinked_open {
                self.refresh_unlinked(cx);
            }
            if self.cycle.dirty() {
                self.rebuild(cx);
            }
            cx.notify();
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Ring the links to the page open in the next pane (§4.3); `None` rings nothing.
    pub fn set_next_link(&mut self, next: Option<NextLink>, cx: &mut Context<Self>) {
        if self.next_link != next {
            self.next_link = next;
            cx.notify();
        }
    }

    pub fn next_link(&self) -> Option<&NextLink> {
        self.next_link.as_ref()
    }

    /// The page title's type role: smaller inside a pane (§4.3).
    fn page_title_role(&self) -> crate::theme::TypeRole {
        if self.in_pane {
            crate::theme::PANE_PAGE_TITLE
        } else {
            crate::theme::PAGE_TITLE
        }
    }

    /// A day title's type role: smaller inside a pane (§4.3).
    fn day_title_role(&self) -> crate::theme::TypeRole {
        if self.in_pane {
            crate::theme::PANE_DAY_TITLE
        } else {
            DAY_TITLE
        }
    }

    /// The pinned day heading's height (DN-3) for the day title in force.
    fn sticky_height(&self) -> f32 {
        STICKY_HEIGHT - DAY_TITLE.line + self.day_title_role().line
    }

    /// Air above the column: none in a pane, whose 44 px header is the air (§4.3).
    fn pad_top(&self, page: bool) -> f32 {
        match (self.in_pane, page) {
            (true, _) => 0.,
            (false, true) => CONTENT_PADDING_TOP_PAGE,
            (false, false) => CONTENT_PADDING_TOP,
        }
    }

    /// Air below the last item: 72 in a pane (§4.3), 96 otherwise (§2.3).
    fn pad_bottom(&self) -> f32 {
        if self.in_pane {
            crate::theme::PANE_PADDING_BOTTOM
        } else {
            CONTENT_PADDING_BOTTOM
        }
    }

    /// Ask the workspace to open `page` for a pointer press. `shift` is the press's own
    /// Shift; ⌘/Ctrl is read from the window, which holds the modifiers of the event being
    /// dispatched, so every link, chip and row follows the same rule (SP-2, SP-3).
    fn emit_open(
        &mut self,
        page: PageId,
        zoom: Option<BlockId>,
        focus: Option<BlockId>,
        shift: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let mut m = window.modifiers();
        m.shift = shift;
        cx.emit(open_event(page, zoom, focus, &m));
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

    /// The top bar's day navigation in the Daily Notes feed (DESIGN.md §3.3): move the
    /// column to the day section above or below. `delta < 0` is the previous (older) day,
    /// matching `PrevDay`, and `delta > 0` the next (newer) one. Feed days are newest
    /// first, so the older day is the next heading in the list.
    pub fn scroll_to_adjacent_day(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.journal().is_none() {
            return;
        }
        let top = self.list.logical_scroll_top().item_ix;
        // A stop is an open day's heading or the first row of an empty-day run: every day
        // is one stop, but a run of empty rows (DESIGN.md §3.6) is not stepped through row
        // by row. `day_headings` only has open days, which used to skip the empty days
        // between them.
        let stops: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(ix, item)| match item {
                Item::DayHeader { .. } => Some(ix),
                Item::EmptyDay { first: true, .. } => Some(ix),
                _ => None,
            })
            .collect();
        let current = stops.iter().rposition(|ix| *ix <= top);
        let target = if delta < 0 {
            // Previous = older, matching `PrevDay`; the feed lists newest first.
            match current {
                Some(i) => stops.get(i + 1).copied(),
                None => stops.first().copied(),
            }
        } else {
            current.and_then(|i| i.checked_sub(1)).map(|i| stops[i])
        };
        if let Some(ix) = target {
            self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: px(0.),
            });
            cx.notify();
            return;
        }
        // At the oldest loaded day, prev used to be a no-op forever. Scroll the tail into
        // view so the sentinel asks for the next window (FR-1.2); the list then grows.
        if delta < 0
            && self
                .items
                .last()
                .is_some_and(|i| matches!(i, Item::JournalEnd))
        {
            self.list.scroll_to(ListOffset {
                item_ix: self.items.len() - 1,
                offset_in_item: px(0.),
            });
            cx.notify();
        }
    }

    /// DESIGN.md §4.1: "Today" resets the feed's scroll position to today. Today's day
    /// is the feed's first item, so this scrolls the list to its top; the top bar's Today
    /// button (DESIGN.md §3.3) and the Today pill both call it.
    pub fn scroll_to_today(&mut self, cx: &mut Context<Self>) {
        if self.journal().is_none() {
            return;
        }
        self.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    /// How many empty-day rows the feed carries as separate list items (DESIGN.md §3.6),
    /// for scripts. One item per day is what lets the list virtualize a long blank run.
    pub fn empty_day_rows(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i, Item::EmptyDay { .. }))
            .count()
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
        // A property row's edit ends with the page it belongs to. `Workspace::open` ends the
        // editor, but its `Blurred` is delivered after this update; by then `reset_page_ui`
        // would have cleared the row and `self.page()` would be the *new* page. Finish it
        // here, while the old page is still the one this view shows (§3.8).
        self.editor.update(cx, |e, cx| e.end(cx));
        self.finish_property_edit(cx);
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
        self.page_menu = false;
        self.controls_menu = None;
        self.refs_key = None;
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
        self.unlinked_open = false;
        self.property_edit = None;
        self.pending_property = None;
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
        // DESIGN.md §3.8: a leading property block is edited in the bar (the header item),
        // not as a row. Any other target leaves the bar's edit state. The row being left is
        // cleaned up after `begin` commits it: the editor switches targets without emitting
        // `Blurred`, so waiting for that event would leave an emptied or pending row behind.
        // A live composition defers `begin` (editor.rs §13.5): the editor still targets
        // the old row, so its store text may be empty without the row being finished. The
        // deferred `begin` emits `Switched`, which finishes it then (see on_editor_event).
        let composing = self.editor.read(cx).composing();
        let previous = self.property_edit.take();
        self.property_edit = self.is_property_block(&block).then(|| block.clone());
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
                    // A properties-bar row edits its raw block text (§3.8) and bolds the key.
                    property: self.property_edit.as_ref() == Some(&block),
                },
                caret,
                goal,
                window,
                cx,
            )
        });
        if let Some(old) = previous {
            if !composing && self.property_edit.as_ref() != Some(&old) {
                self.finish_property_block(old, cx);
            }
        }
        if !drawn {
            // Rebuild with the edit target set, so the folded attribute opens and this
            // block gets a row that can hold the editor (a focused element that is not
            // rendered gets no keystrokes).
            self.rebuild(cx);
        }
        // The properties bar lives in the header item, which has no row index: keep its
        // editor and return before the row scroll/end logic.
        let in_bar = self.property_edit.as_ref() == Some(&block);
        let ix = if in_bar { None } else { self.row_index(&block) };
        self.splice_items(self.items.len(), cx);
        if in_bar {
            cx.notify();
            return;
        }
        match ix {
            // DN-3: the row was under the pinned heading: put its top below it.
            Some(ix) if under_heading => self.list.scroll_to(ListOffset {
                item_ix: ix,
                offset_in_item: -px(self.sticky_height()),
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
    /// next short row scrolled up under the pinned heading. Measured against bounds when the
    /// list has them, so a tall row that merely *follows* the top one does not trigger a
    /// scroll it does not need.
    fn row_overlaps_pinned_day(&self, ix: usize, line_height: f32) -> bool {
        if self.sticky_heading().is_none() {
            return false;
        }
        let viewport = self.list.viewport_bounds();
        if viewport.size.height > px(0.) {
            if let Some(row) = self.list.bounds_for_item(ix) {
                return row.top() < viewport.top() + px(self.sticky_height());
            }
        }
        // A splice (an edit starts by re-registering the rows so the editor can keep the
        // keystrokes) leaves the list with no measured rows until the next layout. Estimate
        // from the offset instead: the heading covers `STICKY_HEIGHT`, and a row is at
        // least one line tall, so only the first few rows can hide under it.
        if -self.list.logical_scroll_top().offset_in_item >= px(self.sticky_height()) {
            return false;
        }
        let rows = (self.sticky_height() / line_height.max(1.)).ceil() as usize;
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
                offset_in_item: -px(self.sticky_height()),
            });
        }
    }

    fn row_index(&self, block: &BlockId) -> Option<usize> {
        self.items
            .iter()
            .position(|i| matches!(i, Item::Row(r) if r.block == *block))
    }

    /// Re-read the appearance preferences this view mirrors. `rebuild` calls it too, so
    /// an index-triggered pass cannot build rows with the old setting and leave the next
    /// frame's `rebuild_if_dirty` to correct it.
    fn sync_preferences(&mut self, cx: &mut Context<Self>) {
        let children = crate::appearance::ref_children(cx);
        if self.ref_children_limit != children {
            self.ref_children_limit = children;
            // The children cache is keyed on the limit and heals on its own; the rows that
            // show those children are what is stale.
            self.cycle
                .invalidate(Invalidation::prefs() | Invalidation::rows());
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
        let property_editing = self.property_edit.is_some();
        let handles: Vec<Option<FocusHandle>> = self
            .items
            .iter()
            .map(|i| match i {
                Item::Row(r) if editor.is_editing(host, &r.page, &r.block) => Some(handle.clone()),
                // The properties bar lives in the header item; while one of its rows is
                // being edited the header must stay rendered, or the keystrokes are lost.
                Item::Header
                    if property_editing && editor.target().is_some_and(|t| t.host == host) =>
                {
                    Some(handle.clone())
                }
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
            // DESIGN.md §3.8: the bar is exactly the leading consecutive property blocks.
            if zoom.is_none() {
                header.properties = leading_properties(&st);
                let fmt = app.store.date_format().clone();
                header.first_mentioned = chrono::Local
                    .timestamp_millis_opt(st.meta.created_at)
                    .single()
                    .map(|dt| dt.date_naive())
                    .filter(|_| st.meta.created_at > 0)
                    .map(|d| (d, fmt.format(d)));
            }
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
            // The page view's leading properties are drawn once, in the bar, and skipped in
            // the outline (DESIGN.md §3.8 rule 2). A zoomed view has no bar, so it keeps
            // every block as it is.
            let mut skip: Vec<BlockId> =
                header.properties.iter().map(|p| p.block.clone()).collect();
            // The empty row "Add property" created is drawn by the bar; until it becomes a
            // real `key:: value` it is not a leading property, so it would otherwise also be
            // a body bullet (and the first row focus_first finds).
            if let Some(pending) = &self.pending_property {
                skip.push(pending.clone());
            }
            // A row that empty text just dropped from `header.properties` is still being
            // edited in the bar; it must not jump into the outline mid-edit.
            if let Some(editing) = &self.property_edit {
                skip.push(editing.clone());
            }
            push_page_rows(
                app,
                &st,
                &page,
                zoom.as_ref(),
                &collapsed,
                &open_embeds,
                &skip,
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
        self.self_titles = std::iter::once(self.header.title.clone())
            .chain(self.header.aliases.iter().cloned())
            .filter(|t| !t.is_empty())
            .map(|t| utopia_core::title::normalize_title(&t))
            .collect();
        let mut items = vec![Item::Header];
        if self.header.missing {
            self.replace_items(items, cx);
            return;
        }
        // A page whose only root blocks are properties (drawn in the bar) still needs
        // somewhere to click to start writing.
        if !rows.iter().any(|i| matches!(i, Item::Row(_))) {
            items.push(Item::Empty);
        }
        for r in rows {
            items.push(r);
        }
        self.push_refs(&page, &mut items, cx);
        if zoom.is_none() {
            // Unlinked References (FR-3.9, LR-5, DESIGN.md §3.7): a collapsed row that is
            // searched only once it is expanded, so a closed row never runs the FTS scan.
            let current = self
                .unlinked
                .as_ref()
                .filter(|(v, _)| *v == refs_version)
                .filter(|_| self.unlinked_open);
            let count = current.map(|(_, m)| m.len());
            let mentions = current.map(|(_, m)| m.clone());
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
        if !matches!(self.mode, Mode::Page { .. }) || !self.unlinked_open {
            // The section is only on a whole page, and only expanded content is searched
            // (DESIGN.md §3.7).
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
                        &[],
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
        let mut previous_empty = false;
        for (i, day) in days.into_iter().enumerate() {
            // Like Roam's feed: Linked References under the day when it has any, and no
            // Unlinked References (those stay on the day's own page). A day with
            // references is not empty even when it has no blocks, so it stays open.
            let refs = day
                .page
                .as_ref()
                .and_then(|p| refs_cache.get(p))
                .filter(|r| !r.groups.is_empty());
            // DESIGN.md §3.6: a day with no blocks (and no references) is one empty row,
            // not a section. Today is open from the start only because its draft already
            // has the block the caret goes in (FR-1.2); the first keystroke writes it.
            // PG-1: rows can exist and still not be visible — a root attribute block folds
            // to one muted row, so the day keeps its heading but still offers the prompt.
            let open = !day.rows.is_empty() || refs.is_some();
            if open {
                previous_empty = false;
                let weekday: SharedString = weekday_label(day.date).into();
                let heading_ix = headings.len();
                headings.push(DayHeading {
                    ix: items.len(),
                    end: items.len(),
                    date: day.date,
                    title: day.title.clone().into(),
                    weekday: weekday.clone(),
                    today: day.date == today_date,
                });
                items.push(Item::DayHeader {
                    date: day.date,
                    page: day.page.clone(),
                    title: day.title.into(),
                    weekday,
                    today: day.date == today_date,
                    first: i == 0,
                });
                if !day.rows.iter().any(|i| matches!(i, Item::Row(_))) {
                    items.push(Item::DayEmpty { date: day.date });
                }
                items.extend(day.rows);
                if let Some(page) = day.page {
                    // Like Roam's feed: Linked References under the day when it has any, and
                    // no Unlinked References (those stay on the day's own page).
                    if let Some(refs) = refs {
                        let collapsed = self.collapsed_ref_days.contains(&day.date);
                        items.push(Item::RefsHeader {
                            title: "Linked references".into(),
                            count: refs.shown(),
                            date: Some(day.date),
                            collapsed,
                        });
                        // The collapsed header is the whole panel (rounded all round); a
                        // bottom edge under it would read as a shadow (§2.4).
                        if !collapsed {
                            self.push_ref_groups(&page, refs, true, &mut items, cx);
                            items.push(Item::RefsEnd);
                        }
                    }
                    ids.push(page);
                }
                headings[heading_ix].end = items.len();
            } else {
                items.push(Item::EmptyDay {
                    date: day.date,
                    weekday: weekday_label(day.date).into(),
                    title: day.title.into(),
                    first: !previous_empty,
                });
                previous_empty = true;
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
            "Linked references"
        };
        items.push(Item::RefsHeader {
            title: title.into(),
            count: shown,
            date: None,
            collapsed: false,
        });
        if self.zoom().is_none() {
            items.push(Item::RefsControls);
        }
        if self.refs.groups.is_empty() {
            items.push(Item::NoRefs);
            items.push(Item::RefsEnd);
            return;
        }
        let daily = self.header.kind == PageKind::Daily;
        let refs = std::mem::take(&mut self.refs);
        self.push_ref_groups(page, &refs, daily, items, cx);
        self.refs = refs;
        items.push(Item::RefsEnd);
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
        let mut first_group = true;
        for g in &refs.groups {
            let scheduled = daily && g.page.id != *page;
            items.push(Item::GroupHeader {
                page: g.page.clone(),
                count: g.hits.len(),
                scheduled,
                first: std::mem::take(&mut first_group),
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
        if let EditorEvent::Switched { from } = event {
            // The composition ended and the editor finally moved to the row the user
            // picked; finish the row it left now that its text is committed.
            self.finish_property_block(from.clone(), cx);
            if self.property_edit.as_ref() == Some(from) {
                self.property_edit = None;
            }
            self.cycle.invalidate(Invalidation::rows());
            cx.notify();
            return;
        }
        if matches!(event, EditorEvent::Blurred) {
            self.finish_property_edit(cx);
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
                if self.is_property_block(&t.block) {
                    // DESIGN.md §3.8: Enter commits the row; at the end of the last one it
                    // moves on to the first body block instead of splitting the property.
                    let at_end = *caret >= editor.read(cx).field_text().len();
                    let last = self.is_last_property(&t.block);
                    self.editor.update(cx, |e, cx| e.end(cx));
                    if at_end && last {
                        self.focus_body(window, cx);
                    }
                    return;
                }
                let collapsed = self.app.read(cx).store.is_collapsed(&t.block);
                self.run_plan(
                    &t.page,
                    |st| outline::split_block(st, &t.block, *caret, collapsed),
                    window,
                    cx,
                );
            }
            EditorEvent::InsertAbove => {
                // The bar is not part of the outline: inserting above its first row would
                // put a plain block between the properties and the body and end the bar.
                if self.is_property_block(&t.block) {
                    return;
                }
                self.run_plan(
                    &t.page,
                    |st| outline::insert_above(st, &t.block),
                    window,
                    cx,
                );
            }
            EditorEvent::Indent => {
                if self.is_property_block(&t.block) || self.indent_reaches_a_property(&t, cx) {
                    return;
                }
                let caret = editor.read(cx).cursor();
                self.run_plan_keep(&t, caret, |st| outline::indent(st, &t.block), window, cx);
            }
            EditorEvent::Outdent => {
                if self.is_property_block(&t.block) {
                    return;
                }
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
                // Visibility here is the renderer's, not the store's alone.
                let properties: Vec<BlockId> = self
                    .header
                    .properties
                    .iter()
                    .map(|p| p.block.clone())
                    .collect();
                // The properties bar is not part of the outline. If the row above is a
                // property (or there is none), Backspace has nothing to merge into.
                let blocked = self.app.update(cx, |app, _| {
                    let Ok(st) = app.store.page_state(&t.page) else {
                        return true;
                    };
                    let rows = st.visible_rows(zoom.as_ref(), &|b| collapsed.contains(b));
                    match rows.iter().position(|(r, _)| r == &t.block) {
                        Some(0) | None => true,
                        Some(ix) => properties.contains(&rows[ix - 1].0),
                    }
                });
                if blocked {
                    return;
                }
                let now = self.app.read(cx).store.now();
                self.run_plan(
                    &t.page,
                    |st| {
                        outline::merge_into_previous(
                            st,
                            &t.block,
                            zoom.as_ref(),
                            &|b| collapsed.contains(b),
                            now,
                        )
                    },
                    window,
                    cx,
                );
            }
            EditorEvent::Navigate { up, x } => {
                if self.is_property_block(&t.block) {
                    let props = self.property_blocks();
                    let pos = props.iter().position(|b| b == &t.block);
                    let next = pos.and_then(|p| {
                        if *up {
                            p.checked_sub(1).map(|i| (props[i].clone(), true))
                        } else {
                            props.get(p + 1).map(|b| (b.clone(), false))
                        }
                    });
                    match next {
                        Some((block, to_end)) => {
                            let x = x.unwrap_or(px(0.));
                            let goal = if *up {
                                Goal::LastLine(x)
                            } else {
                                Goal::FirstLine(x)
                            };
                            let len = self.block_text_len(&block, cx);
                            self.edit_property_at(
                                block,
                                to_end.then_some(len),
                                Some(goal),
                                window,
                                cx,
                            );
                        }
                        // Down from the last row, or up from the first: leave the bar.
                        None if !*up => self.focus_body(window, cx),
                        None => {}
                    }
                    return;
                }
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
                if *up && next.is_none() {
                    // Arrow up from the first body block moves into the last property row.
                    if let Some(last) = self.property_blocks().last().cloned() {
                        let x = x.unwrap_or(px(0.));
                        let len = self.block_text_len(&last, cx);
                        self.edit_property_at(last, Some(len), Some(Goal::LastLine(x)), window, cx);
                        return;
                    }
                }
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
            // Option+Up/Down ran out of block: carry on at the start of the block above or
            // the end of the block below, which is what macOS text views do with a
            // paragraph (`editor.rs` `paragraph_up` / `paragraph_down`).
            EditorEvent::BlockEdge { to_start } => {
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
                let next = pos.and_then(|p| {
                    if *to_start {
                        p.checked_sub(1)
                    } else {
                        Some(p + 1)
                    }
                });
                if let Some(r) = next.and_then(|n| rows.get(n)).map(|(_, r)| (*r).clone()) {
                    // `None` puts the caret at the end of the block that opens.
                    let caret = to_start.then_some(0);
                    self.edit(r.page, r.block, caret, None, window, cx);
                }
            }
            EditorEvent::MoveBlock { up } => {
                if self.is_property_block(&t.block) {
                    return;
                }
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
                // The feed is the stack's first entry and stays the feed (DESIGN.md §4.3):
                // zooming into one of its days opens that zoom as the next entry.
                if self.page().as_ref() == Some(&t.page) && !self.is_journal() {
                    self.open(t.page.clone(), Some(t.block.clone()), cx);
                    self.focus_first(window, cx);
                } else {
                    // A key, not a press: always the plain rule, whatever is held. `focus` is
                    // the zoom root itself: the caret goes on into the zoomed rows.
                    cx.emit(PageViewEvent::Open {
                        page: t.page.clone(),
                        zoom: Some(t.block.clone()),
                        focus: Some(t.block.clone()),
                        sidebar: false,
                        click: Click::Plain,
                        replace: false,
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
            EditorEvent::Restore(_) | EditorEvent::Blurred | EditorEvent::Switched { .. } => {}
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
                    self.emit_open(page, None, None, sidebar, window, cx);
                }
            }
            LinkTarget::Block(uid) => {
                if sidebar || row.is_none() {
                    // A chip in a read-only row (a Linked Reference) has no row to expand,
                    // so following it opens the block it points at (FR-4.3).
                    if let Ok(Some(b)) = self.app.read(cx).store.block_row(&BlockId(uid.clone())) {
                        self.editor.update(cx, |e, cx| e.end(cx));
                        self.emit_open(b.page_id, Some(b.id), None, sidebar, window, cx);
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

    /// What the top bar's breadcrumb shows (DESIGN.md §3.3): the page's display title.
    pub(crate) fn page_title(&self) -> SharedString {
        self.header.title.clone().into()
    }

    /// The page kind, so the top bar can hide Rename for a daily title (rule 6).
    pub(crate) fn page_kind(&self) -> PageKind {
        self.header.kind
    }

    /// Whether the top bar offers "Rewrite references" (only with an alias to rewrite).
    pub(crate) fn has_aliases(&self) -> bool {
        !self.header.aliases.is_empty()
    }

    /// A day that has not been written yet has no page actions to offer (FR-1.2).
    pub(crate) fn is_draft(&self) -> bool {
        self.header.draft
    }

    /// "Open in graph" from the top bar's menu: the workspace owns the graph view, so the
    /// view only asks for it.
    pub(crate) fn request_open_graph(&mut self, cx: &mut Context<Self>) {
        if let Some(page) = self.page() {
            self.editor.update(cx, |e, cx| e.end(cx));
            cx.emit(PageViewEvent::OpenGraph { page });
        }
    }

    pub fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                click: Click::Plain,
                replace: true,
            }),
            _ => {
                self.cycle.invalidate(Invalidation::rows());
                cx.notify();
            }
        }
    }

    fn pick_date(&mut self, d: NaiveDate, sidebar: bool, w: &mut Window, cx: &mut Context<Self>) {
        self.show_calendar = false;
        let page = self.app.update(cx, |app, cx| {
            let r = app.store.draft_daily(d);
            let p = app.report(r);
            // A visit writes nothing (FR-1.2): no structural change, no index flush.
            app.changed(Vec::new(), false, cx);
            p
        });
        if let Some(page) = page {
            self.emit_open(page, None, None, sidebar, w, cx);
        }
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.calendar_month = crate::calendar::shift_month(self.calendar_month, delta);
        cx.notify();
    }

    pub(crate) fn page_action(&mut self, action: &str, cx: &mut Context<Self>) {
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
    pub(crate) fn export_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.render_rich_with(text, row_block, row, min_h, prefix, None, true, cx)
    }

    /// [`render_rich`] with one extra source-range highlight (an unlinked match) and, for
    /// rows whose debug selector is already used by the block itself, no `text-<block>`
    /// selector.
    #[allow(clippy::too_many_arguments)]
    fn render_rich_with(
        &self,
        text: &str,
        row_block: &BlockId,
        row: Option<RowItem>,
        min_h: f32,
        prefix: usize,
        extra: Option<(std::ops::Range<usize>, gpui::HighlightStyle)>,
        named: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let rendered = {
            let store = &self.app.read(cx).store;
            let mut chip = |uid: &str| crate::reference::chip_state(store, uid, row_block);
            match extra {
                // The mention range is in the source, not the rendered text (§3.7).
                Some((range, style)) => {
                    rich::render_highlighted(text, &th, &mut chip, range, style)
                }
                None => rich::render(text, &th, &mut chip),
            }
        };
        let styled =
            StyledText::new(rendered.text.clone()).with_highlights(rendered.highlights.clone());
        let layout = styled.layout().clone();
        let same = rendered.text.as_ref() == text;
        let end = prefix + text.len();
        let row2 = row.clone();
        let links = rendered.links.iter().map(|(r, _)| r.clone()).collect();
        let chips = rendered.chips.clone();
        // §4.3: the link the next pane came from. A string compare per link, no store read.
        let rings = self.next_link.as_ref().map_or_else(Vec::new, |next| {
            rendered
                .links
                .iter()
                .filter(|(_, t)| next.matches(t))
                .map(|(r, _)| r.clone())
                .collect()
        });
        let text_sel = format!("text-{}", row_block.0);
        let mut root = div().flex_1().min_h(px(min_h));
        if named {
            root = root.debug_selector(move || text_sel.clone());
        }
        root
            // An editable row shows the I-beam between its links; a read-only one (a query
            // or search hit) is itself clickable, so it keeps its container's pointer.
            .when(row.is_some(), |d| d.cursor_text())
            .child(
                rich::LinkText::new(styled, links)
                    .with_chips(chips, th.tag_bg)
                    .with_rings(rings, th.link_tint),
            )
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
        // DESIGN.md §3.1: children step right by 31 + 13 + the 1 px thread line; the row's
        // caret/bullet gutter hangs in the content column's side padding so block text
        // lines up with the page or day title. A compact pane is too narrow for 45 px a
        // level (the right sidebar is 380 px, a split less), so it indents less and its
        // thread lines follow that same step.
        let indent = if self.compact {
            COMPACT_INDENT
        } else {
            CHILD_INDENT
        };
        let step = r.depth.min(MAX_VISUAL_DEPTH) as f32 * indent;
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
        // DESIGN.md §3.1: a 20 px caret slot in the gutter. The chevron shows on hover (or
        // while collapsed, the only sign that content is hidden); collapse itself is also
        // an action (ToggleCollapse), so the pointer is not the only way in.
        let arrow = {
            let block = r.block.clone();
            let shown = r.has_children && (r.collapsed || hovered);
            div()
                .id(("arrow-slot", ix))
                .debug_selector(move || format!("caret-slot-{ix}"))
                .w(px(CARET_SLOT))
                .h(px(lh))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .when(r.has_children, |d| {
                    d.cursor_pointer().on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_collapse(&block, cx)),
                    )
                })
                .when(shown, |d| {
                    d.child(
                        div()
                            .debug_selector(move || format!("arrow-{ix}"))
                            .text_size(px(CARET_SIZE))
                            .text_color(th.bullet)
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
                .w(px(BULLET_SLOT))
                .h(px(lh))
                .flex_none()
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
                    if e.modifiers().shift
                        || e.modifiers().secondary()
                        || this.is_journal()
                        || Some(&page) != this.page().as_ref()
                    {
                        cx.emit(open_event(
                            page.clone(),
                            Some(block.clone()),
                            None,
                            &e.modifiers(),
                        ));
                    } else {
                        this.open(page.clone(), Some(block.clone()), cx);
                        this.focus_first(window, cx);
                    }
                }))
        };
        let target = r.clone();
        // DESIGN.md §2.3/§3.1: in the main pane the gutter hangs in the column's 40 px side
        // padding, so block text starts exactly where the title does. The right sidebar is
        // narrower (its design is still open, §6), so its rows keep the gutter inside.
        let pad_x = if self.compact {
            crate::theme::SPACING[3]
        } else {
            CONTENT_PADDING_X
        };
        let row_gutter = if self.compact {
            0.
        } else {
            pad_x - (CARET_SLOT + BULLET_SLOT)
        };
        div()
            .id(("row", ix))
            .debug_selector(move || format!("row-{ix}"))
            .relative()
            .ml(px(row_gutter))
            .mr(px(pad_x))
            .py(px(vpad))
            // Thread lines for every ancestor: a 1 px `line` segment at 31 px past each
            // ancestor's row start, which is under that ancestor's bullet centre.
            .children((0..r.depth.min(MAX_VISUAL_DEPTH)).map(|k| {
                div()
                    .debug_selector(move || format!("thread-{ix}-{k}"))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(k as f32 * indent + THREAD_OFFSET))
                    .w(px(1.))
                    .bg(th.line)
            }))
            .when(r.embed > 0, |d| {
                d.bg(th.well).border_l_2().border_color(th.line_soft)
            })
            .drag_over::<DraggedBlock>(move |s, _, _, _| s.border_b_2().border_color(th.link))
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .pl(px(step))
                    .child(arrow)
                    .child(bullet)
                    .when(r.kind == BlockKind::Todo, |d| {
                        let row = r.clone();
                        // DESIGN.md §3.1: a 16 px checkbox with the link accent and a 10 px
                        // gap before the text. A done to-do is the filled link box with a
                        // check cut out of it (the page colour). gpui has no native
                        // checkbox, so this is a drawn control with the same behaviour.
                        d.child(
                            div()
                                .id(("todo", ix))
                                .debug_selector(|| format!("todo-{ix}"))
                                .size(px(TODO_SIZE))
                                .mr(px(TODO_GAP))
                                .mt(px((lh - TODO_SIZE) / 2.))
                                .rounded(px(crate::theme::RADIUS_CHIP))
                                .border_1()
                                .border_color(if r.checked { th.link } else { th.line_strong })
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .font_family(crate::theme::MONO)
                                .text_size(px(TODO_CHECK_SIZE))
                                .when(r.checked, |d| d.bg(th.link).text_color(th.paper).child("✓"))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.toggle_checked(&row, cx)
                                    }),
                                ),
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
                            // PG-5: a 2 px link bar and a warm grey italic, not the interface
                            // muted grey.
                            BlockKind::Quote => c
                                .border_l_2()
                                .border_color(th.link)
                                .pl_2()
                                .text_color(th.ink_3)
                                .italic(),
                            // PG-5: a surface card in Menlo, 6 px corners, with the language (when
                            // the block names one) at the top right.
                            BlockKind::Code => {
                                let card = c
                                    .bg(th.well)
                                    .border_1()
                                    .border_color(th.line)
                                    .rounded(px(crate::theme::RADIUS_CONTROL))
                                    .px_2()
                                    .py_1()
                                    .font_family(crate::theme::MONO)
                                    .text_size(px(crate::theme::CODE_TEXT_SIZE));
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
                                .bg(th.line_soft)
                                .rounded(px(crate::theme::RADIUS_CHIP))
                                .child(format!("depth {}", r.depth)),
                        )
                    }),
            )
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

    /// The page actions for a compact pane's card (RS-5): the same items the top bar's menu
    /// offers for the main page. The main pane does not draw this — its menu is in the top
    /// bar (DESIGN.md §3.3).
    fn page_menu_popup(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        let graph = quiet_control("page-menu-graph", "Open in graph", &th)
            .debug_selector(|| "page-menu-graph".into())
            .on_click(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                this.request_open_graph(cx);
                cx.notify();
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
            .w(px(PAGE_MENU_WIDTH))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .child(graph.w_full().justify_start())
            .child(export.w_full().justify_start())
            .children(rename.map(|r| r.w_full().justify_start()))
            .children(rewrite.map(|r| r.w_full().justify_start()))
            .children(pin.map(|p| p.w_full().justify_start()))
            // A hairline sets the destructive item apart, always last.
            .child(div().h(px(1.)).my_1().bg(th.line))
            .child(delete.w_full().justify_start())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.page_menu = false;
                cx.notify();
            }))
    }

    // ---------------------------------------------------------------- properties bar

    /// The page's leading property blocks as the bar (DESIGN.md §3.8): they are drawn here
    /// and skipped by the outline, so the same data never appears twice.
    fn render_properties(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let key_w = if self.compact {
            PROPERTY_KEY_WIDTH_COMPACT
        } else {
            PROPERTY_KEY_WIDTH
        };
        let mut bar = div()
            .debug_selector(|| "properties-bar".into())
            // The header column already puts 4 px between children; the spec asks for 20.
            .mt(px(PROPERTY_GAP_BELOW_TITLE - 4.))
            .py(px(PROPERTY_PADDING_Y))
            .border_t_1()
            .border_b_1()
            .border_color(th.line_soft)
            .flex()
            .flex_col();
        for (i, p) in self.header.properties.iter().enumerate() {
            bar = bar.child(self.render_property_row(i, p, key_w, cx));
        }
        if let Some(pending) = &self.pending_property {
            if !self.header.properties.iter().any(|p| &p.block == pending) {
                bar = bar.child(self.render_property_edit(cx));
            }
        }
        // The same for an existing row whose text is now empty: until the edit ends (and
        // the block is removed) it stays a bar row instead of reappearing as a bullet.
        if let Some(editing) = &self.property_edit {
            if !self.header.properties.iter().any(|p| &p.block == editing)
                && self.pending_property.as_ref() != Some(editing)
            {
                bar = bar.child(self.render_property_edit(cx));
            }
        }
        bar = bar.child(self.render_alias_row(key_w, cx));
        bar = bar.child(self.render_add_property(cx));
        bar.into_any_element()
    }

    fn property_key_cell(
        &self,
        key: &str,
        glyph: Icon,
        key_w: f32,
        th: &crate::theme::Theme,
    ) -> Div {
        div()
            .w(px(key_w))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .child(icon(glyph, PROPERTY_ICON, th.ink_3))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(crate::theme::SMALL.size))
                    .text_color(th.ink_3)
                    .child(key.to_string()),
            )
    }

    fn render_property_row(
        &self,
        i: usize,
        p: &PropertyBlock,
        key_w: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        if self.property_edit.as_ref() == Some(&p.block) {
            return self.render_property_edit(cx);
        }
        let block = p.block.clone();
        div()
            .id(("property", i))
            .debug_selector(move || format!("property-{i}"))
            // The row reaches past the content so its hover fill does not move the text.
            .mx(px(-PROPERTY_HOVER_BLEED))
            .px(px(PROPERTY_HOVER_BLEED))
            .min_h(px(PROPERTY_ROW_MIN_HEIGHT))
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .flex()
            .items_center()
            .gap_2()
            .hover(|d| d.bg(th.shelf))
            .cursor_pointer()
            .child(self.property_key_cell(&p.key, Icon::DashedCircle, key_w, &th))
            .child(
                // A flex value cell lets the status pill size to its text instead of
                // filling the column like a block-level child (gpui divs are blocks).
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .child(self.render_property_value(p, cx)),
            )
            // On click, not mouse-down: gpui focuses the workspace root on mouse-down, and a
            // focus taken during that same event is lost again when it finishes.
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.edit_property(block.clone(), window, cx)
                }),
            )
            .into_any_element()
    }

    fn render_property_value(&self, p: &PropertyBlock, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        match p.kind {
            PropertyKind::Status => div()
                .debug_selector(|| "property-status".into())
                .h(px(STATUS_PILL_HEIGHT))
                .flex()
                .items_center()
                .gap_1()
                .px(px(STATUS_PILL_PADDING_X))
                .rounded_full()
                .border_1()
                .border_color(th.line_strong)
                .bg(th.paper)
                .child(
                    div()
                        .size(px(STATUS_DOT))
                        .flex_none()
                        .rounded_full()
                        .bg(th.status_progress),
                )
                .child(
                    div()
                        .text_size(px(crate::theme::SMALL.size))
                        .text_color(th.ink)
                        // Title-cased for display only; the block keeps what was typed.
                        .child(title_case(&p.value)),
                )
                .into_any_element(),
            PropertyKind::Linked => {
                let title = utopia_core::inline::extract_refs(&p.value)
                    .first()
                    .map(|r| r.target.clone())
                    .unwrap_or_else(|| p.value.clone());
                let follow = title.clone();
                div()
                    .debug_selector(|| "property-avatar-row".into())
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| "property-avatar".into())
                            .size(px(AVATAR_SIZE))
                            .flex_none()
                            .rounded_full()
                            .bg(th.tag_bg)
                            .text_color(th.tag_ink)
                            .text_size(px(AVATAR_TEXT))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(initials(&title)),
                    )
                    .child(
                        div()
                            .text_size(px(crate::theme::TAG.size))
                            .text_color(th.link)
                            .cursor_pointer()
                            .child(title)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.follow(
                                        LinkTarget::Page(follow.clone()),
                                        None,
                                        e.modifiers.shift,
                                        window,
                                        cx,
                                    );
                                }),
                            ),
                    )
                    .into_any_element()
            }
            PropertyKind::Plain => {
                let line = crate::appearance::density(cx).line_height();
                div()
                    .text_size(px(crate::theme::TAG.size))
                    .child(self.render_rich(&p.value, &p.block, None, line, 0, cx))
                    .into_any_element()
            }
        }
    }

    fn render_property_edit(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        div()
            .debug_selector(|| "property-edit".into())
            .mx(px(-PROPERTY_HOVER_BLEED))
            .px(px(PROPERTY_HOVER_BLEED))
            .min_h(px(PROPERTY_ROW_MIN_HEIGHT))
            .flex()
            .items_center()
            .child(
                // The inset 1.5 px link ring around the raw text (§3.8).
                div()
                    .flex_1()
                    .rounded(px(crate::theme::RADIUS_CONTROL))
                    .border(px(1.5))
                    .border_color(th.link)
                    .bg(th.paper)
                    .px_1()
                    .py_0p5()
                    .child(self.editor.clone()),
            )
            .into_any_element()
    }

    fn render_alias_row(&self, key_w: f32, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let mut value = div().flex().flex_wrap().items_center().gap_1();
        for (i, a) in self.header.aliases.iter().enumerate() {
            let alias = a.clone();
            value = value.child(
                div()
                    .id(("alias", i))
                    .debug_selector(move || format!("alias-{i}"))
                    .h(px(crate::theme::AVATAR_SIZE))
                    .flex()
                    .items_center()
                    .px_2()
                    .rounded(px(crate::theme::RADIUS_CHIP))
                    .bg(th.line_soft)
                    .text_size(px(crate::theme::SMALL.size))
                    .text_color(th.muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover).text_color(th.ink))
                    .child(format!("{a} ×"))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_alias(&alias, cx))),
            );
        }
        value = match &self.alias_input {
            Some(input) => value.child(
                div()
                    .w(px(crate::theme::PROPERTY_ALIAS_WIDTH))
                    .px_1()
                    .border_1()
                    .border_color(th.link)
                    .rounded(px(crate::theme::RADIUS_CHIP))
                    .child(input.clone()),
            ),
            None => value.child(
                self.button("add-alias", "Add alias", cx)
                    .debug_selector(|| "add-alias".into())
                    .on_click(cx.listener(|this, _, window, cx| this.start_alias(window, cx))),
            ),
        };
        div()
            .debug_selector(|| "property-aliases".into())
            .mx(px(-PROPERTY_HOVER_BLEED))
            .px(px(PROPERTY_HOVER_BLEED))
            .min_h(px(PROPERTY_ROW_MIN_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .child(self.property_key_cell("aliases", Icon::List, key_w, &th))
            .child(value)
            .into_any_element()
    }

    fn render_add_property(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let mut row = div()
            .debug_selector(|| "add-property-row".into())
            .min_h(px(PROPERTY_ROW_MIN_HEIGHT))
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("add-property")
                    .debug_selector(|| "add-property".into())
                    .h(px(crate::theme::PROPERTY_CONTROL_HEIGHT))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .rounded(px(crate::theme::RADIUS_CONTROL))
                    .text_size(px(crate::theme::SMALL.size))
                    .text_color(th.ink_3)
                    .cursor_pointer()
                    .hover(|d| d.bg(th.hover).text_color(th.ink))
                    .child(icon(Icon::Plus, crate::icons::ICON_SIZE, th.ink_3))
                    .child("Add property")
                    .on_click(cx.listener(|this, _, window, cx| this.add_property(window, cx))),
            )
            .child(div().flex_1());
        if let Some((date, label)) = self.header.first_mentioned.clone() {
            row = row.child(
                div()
                    .debug_selector(|| "first-mentioned".into())
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child("First mentioned")
                    .child(
                        div()
                            .text_color(th.link)
                            .cursor_pointer()
                            .child(label)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                    this.pick_date(date, e.modifiers.shift, window, cx)
                                }),
                            ),
                    ),
            );
        }
        row.into_any_element()
    }

    // ---------------------------------------------------------------- properties editing

    fn block_text_len(&self, block: &BlockId, cx: &mut Context<Self>) -> usize {
        let Some(page) = self.page() else { return 0 };
        self.app.update(cx, |app, _| {
            app.store
                .page_state(&page)
                .ok()
                .and_then(|st| st.block(block).map(|b| b.text.len()))
                .unwrap_or(0)
        })
    }

    /// True when indenting `t.block` would make a leading property its parent. The row
    /// above the first body block is the last bar row, and the bar cannot draw a subtree
    /// (DESIGN.md §3.1/§3.8): Tab there must do nothing rather than demote the properties.
    fn indent_reaches_a_property(&self, t: &EditTarget, cx: &mut Context<Self>) -> bool {
        if self.page().as_ref() != Some(&t.page) {
            return false;
        }
        let zoom = self.zoom();
        let properties: Vec<BlockId> = self
            .header
            .properties
            .iter()
            .map(|p| p.block.clone())
            .collect();
        self.app.update(cx, |app, _| {
            let collapsed = app.store.collapsed_set().clone();
            let Ok(st) = app.store.page_state(&t.page) else {
                return true;
            };
            let rows = st.visible_rows(zoom.as_ref(), &|b| collapsed.contains(b));
            match rows.iter().position(|(r, _)| r == &t.block) {
                Some(0) | None => true,
                Some(ix) => properties.contains(&rows[ix - 1].0),
            }
        })
    }

    /// The bar's blocks in the order the bar draws them: the leading properties, then the
    /// pending "Add property" row, then a row whose empty text just dropped out of the
    /// leading run but whose edit is still open.
    fn property_blocks(&self) -> Vec<BlockId> {
        let mut out: Vec<BlockId> = self
            .header
            .properties
            .iter()
            .map(|p| p.block.clone())
            .collect();
        for block in [&self.pending_property, &self.property_edit]
            .into_iter()
            .flatten()
        {
            if !out.contains(block) {
                out.push(block.clone());
            }
        }
        out
    }

    fn is_property_block(&self, block: &BlockId) -> bool {
        self.header.properties.iter().any(|p| &p.block == block)
            || self.pending_property.as_ref() == Some(block)
            // Empty text removes the block from `header.properties` immediately, but its
            // edit is still the bar's row until it ends: keyboard navigation and Split must
            // keep treating it as one, or the editor is stranded on a block with no row.
            || self.property_edit.as_ref() == Some(block)
    }

    fn is_last_property(&self, block: &BlockId) -> bool {
        self.property_blocks().last() == Some(block)
    }

    /// Draw a property row as its raw block text and put the caret in it (§3.8).
    fn edit_property(&mut self, block: BlockId, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.block_text_len(&block, cx);
        self.edit_property_at(block, Some(len), None, window, cx);
    }

    fn edit_property_at(
        &mut self,
        block: BlockId,
        caret: Option<usize>,
        goal: Option<Goal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.page() else { return };
        self.edit(page, block, caret, goal, window, cx);
        cx.notify();
    }

    /// "Add property": a new empty root block after the last property, edited at once. It
    /// joins the bar only when its text becomes a `key:: value`; until then the pending row
    /// draws it, and an empty commit removes it again.
    fn add_property(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = self.page() else { return };
        let after = self.header.properties.last().map(|p| p.block.clone());
        let id = BlockId::new();
        self.app.update(cx, |app, cx| {
            let r = app.store.execute(
                &page,
                vec![Command::InsertBlock {
                    id: id.clone(),
                    parent: None,
                    after,
                    text: String::new(),
                    kind: BlockKind::Bullet,
                }],
                // Record the insert, and keep it open for the first text edit (the
                // editor coalesces within 500 ms), so one Cmd+Z removes the whole row
                // instead of leaving an empty block behind (§3.8).
                utopia_store::EditOpts {
                    coalesce: Some(id.clone()),
                    ..Default::default()
                },
            );
            app.report(r);
            app.changed(vec![page.clone()], true, cx);
        });
        self.pending_property = Some(id.clone());
        self.edit_property_at(id, Some(0), None, window, cx);
    }

    /// The bar's edit ended (Enter, Escape, a click elsewhere): an emptied property row is
    /// removed (DESIGN.md §3.8), and a pending row stops being pending either way.
    fn finish_property_edit(&mut self, cx: &mut Context<Self>) {
        let Some(block) = self.property_edit.take() else {
            return;
        };
        self.finish_property_block(block, cx);
    }

    /// Clean up one property row whose edit is over. Split from `finish_property_edit` so a
    /// new target can finish the old row after the editor has committed it: `begin` switches
    /// targets without emitting `Blurred`, so this is the only place that sees both.
    fn finish_property_block(&mut self, block: BlockId, cx: &mut Context<Self>) {
        if self.pending_property.as_ref() == Some(&block) {
            self.pending_property = None;
        }
        let Some(page) = self.page() else { return };
        let text = self.app.update(cx, |app, _| {
            app.store
                .page_state(&page)
                .ok()
                .and_then(|st| st.block(&block).map(|b| b.text.clone()))
        });
        if !text.as_deref().is_some_and(|t| t.trim().is_empty()) {
            return;
        }
        let now = self.app.read(cx).store.now();
        self.app.update(cx, |app, cx| {
            let plan = app
                .store
                .page_state(&page)
                .ok()
                .and_then(|st| outline::delete_subtree(st, &block, now).ok());
            if let Some(plan) = plan {
                let r = app.store.execute_plan(&page, plan, None);
                app.report(r);
                app.changed(vec![page.clone()], true, cx);
            }
        });
        self.cycle.invalidate(Invalidation::rows());
    }

    /// Enter at the end of the last property row: the first body block, created if needed.
    fn focus_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_first(window, cx);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = theme(cx).clone();
        let h = self.header.clone();
        let mut col = div()
            .flex()
            .flex_col()
            .gap_1()
            .px(px(if self.compact {
                crate::theme::SPACING[3]
            } else {
                CONTENT_PADDING_X
            }))
            .pt(px(if self.compact {
                crate::theme::SPACING[3]
            } else {
                self.pad_top(true)
            }))
            .pb_2();
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
                    .hover(|s| s.text_color(th.link))
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
                        .when(i == last, |d| d.text_color(th.ink))
                        .hover(|s| s.text_color(th.link))
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
                .text_size(px(self.page_title_role().size))
                .line_height(px(self.page_title_role().line))
                .border_b_1()
                .border_color(th.link)
                .child(input.clone())
                .into_any_element(),
            None => div()
                .id("title")
                .debug_selector(|| "page-title".into())
                .text_size(px(self.page_title_role().size))
                .line_height(px(self.page_title_role().line))
                .font_weight(self.page_title_role().weight)
                .text_color(if h.deleted { th.muted } else { th.ink })
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
        // A compact card keeps its own page actions; the main pane's live in the
        // workspace top bar (DESIGN.md §3.3). The aliases moved into the properties bar.
        if self.zoom().is_none() && !h.deleted && self.compact && !h.draft {
            col = col.child(
                div()
                    .debug_selector(|| "page-toolbar".into())
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(
                        div()
                            .relative()
                            .child(
                                self.button("pane-page-menu", "···", cx)
                                    .debug_selector(|| "pane-page-menu".into())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.page_menu = !this.page_menu;
                                        cx.notify();
                                    })),
                            )
                            .children(self.page_menu.then(|| {
                                deferred(
                                    anchored()
                                        .anchor(Corner::TopRight)
                                        .offset(point(px(0.), px(MENU_OFFSET)))
                                        .child(self.page_menu_popup(cx)),
                                )
                                .with_priority(1)
                            })),
                    ),
            );
        }
        if self.show_calendar {
            let today = self.app.read(cx).store.today();
            let existing: HashSet<NaiveDate> = HashSet::new();
            col = col.child(
                div()
                    .w(px(crate::theme::CALENDAR_PANEL_WIDTH))
                    .p_2()
                    .border_1()
                    .border_color(th.line)
                    .rounded(px(crate::theme::RADIUS_CONTROL))
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
        if self.zoom().is_none() && !h.deleted {
            col = col.child(self.render_properties(cx));
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
        if self.unlinked_open {
            self.refresh_unlinked(cx);
        }
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
        if self.unlinked_open {
            self.refresh_unlinked(cx);
        }
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
                    .rounded(px(crate::theme::RADIUS_CONTROL))
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .bg(th.line_soft)
                    .text_color(th.link)
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
                    .rounded(px(crate::theme::RADIUS_CONTROL))
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
                            .text_color(th.muted)
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
                            .rounded(px(crate::theme::RADIUS_CONTROL))
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
                                        .offset(point(px(0.), px(MENU_OFFSET)))
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
                                        .offset(point(px(0.), px(MENU_OFFSET)))
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
                .rounded(px(crate::theme::RADIUS_CONTROL))
                .text_size(px(crate::theme::CONTROL_SIZE))
                .text_color(th.ink)
                .cursor_pointer()
                .hover(|s| s.bg(th.hover))
                .child(label)
                .child(if selected {
                    div().text_color(th.link).child("✓")
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
            .w(px(crate::theme::REFS_MENU_WIDTH))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .rounded(px(crate::theme::RADIUS_CONTROL))
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
            Item::Notice { depth, text } => div()
                .pl(px(
                    depth.min(MAX_VISUAL_DEPTH) as f32 * CHILD_INDENT + CONTENT_PADDING_X
                ))
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .child(text)
                .into_any_element(),
            Item::QueryHeader { depth, label } => div()
                .debug_selector(|| "query-header".into())
                .pl(px(
                    depth.min(MAX_VISUAL_DEPTH) as f32 * CHILD_INDENT + CONTENT_PADDING_X
                ))
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
                    .pl(px(
                        depth.min(MAX_VISUAL_DEPTH) as f32 * CHILD_INDENT + CONTENT_PADDING_X
                    ))
                    .pr_4()
                    .py_0p5()
                    .flex()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(
                        div()
                            .flex_none()
                            .w(px(crate::theme::REF_TITLE_WIDTH))
                            .overflow_hidden()
                            .text_size(px(crate::theme::BODY_SIZE))
                            .text_color(th.link)
                            .child(hit.page_title.clone()),
                    )
                    .child(div().flex_1().child(text))
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.end(cx));
                        cx.emit(open_event(
                            page.clone(),
                            None,
                            Some(block.clone()),
                            &e.modifiers(),
                        ));
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
                let pad = px(depth.min(MAX_VISUAL_DEPTH) as f32 * CHILD_INDENT + CONTENT_PADDING_X);
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
                                .rounded(px(crate::theme::RADIUS_CONTROL))
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
                                    .rounded(px(crate::theme::RADIUS_CONTROL))
                                    .bg(th.line_soft)
                                    .text_size(px(crate::theme::BODY_SIZE))
                                    .when(path.is_some(), |d| {
                                        d.cursor_pointer().hover(|s| s.bg(th.hover))
                                    })
                                    .child(icon)
                                    .child(blob.name.clone())
                                    .child(
                                        div()
                                            .text_size(px(crate::theme::CAPTION_SIZE))
                                            .text_color(th.muted)
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
            // DESIGN.md §4.2: an empty body is a muted placeholder behind a `line_strong`
            // bullet.
            Item::Empty => div()
                .id("empty")
                .debug_selector(|| "empty".into())
                .px(px(CONTENT_PADDING_X))
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .cursor_text()
                .child(
                    div()
                        .debug_selector(|| "empty-bullet".into())
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .bg(th.line_strong),
                )
                .child(div().text_color(th.muted).child("Click to start writing"))
                .on_click(cx.listener(|this, _, window, cx| this.focus_first(window, cx)))
                .into_any_element(),
            // DESIGN.md §3.5: a mono weekday label above the title, a Today pill on today,
            // the title exactly as stored, then 16 px before the first block.
            Item::DayHeader {
                date,
                page,
                title,
                weekday,
                today,
                first,
            } => div()
                .debug_selector(move || if first { "today-title" } else { "day-title" }.into())
                .px(px(CONTENT_PADDING_X))
                .when(first, |d| d.pt(px(self.pad_top(false))))
                .when(!first, |d| {
                    // 40 px between days, then the 1 px divider, then the heading. Keeping
                    // that gap in the margin means the pinned copy (STICKY_HEIGHT) is
                    // exactly this header, so the two never double up.
                    d.mt(px(DAY_GAP))
                        .border_t_1()
                        .border_color(th.line_soft)
                        .pt(px(DAY_HEADER_PAD_TOP))
                })
                .pb(px(DAY_HEADER_GAP))
                .child(
                    // Titles stay display only: identity is still `daily_date` (rule 6).
                    div()
                        .debug_selector(move || {
                            format!("day-caption-{}", utopia_core::date::iso(date))
                        })
                        .h(px(DAY_CAPTION_LINE))
                        .flex()
                        .items_center()
                        .gap_1()
                        .font_family(LABEL_MONO)
                        .text_size(px(LABEL.size))
                        .text_color(th.muted)
                        .child(
                            div()
                                .debug_selector(move || {
                                    format!("day-weekday-{}", utopia_core::date::iso(date))
                                })
                                .child(weekday),
                        )
                        .when(today, |d| {
                            d.child(
                                today_pill("Today", &th)
                                    .id("today-pill")
                                    .debug_selector(|| "today-pill".into())
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.scroll_to_today(cx)),
                                    ),
                            )
                        }),
                )
                .child(
                    div()
                        .id(("day-title", ix))
                        .debug_selector(move || {
                            format!("day-date-{}", utopia_core::date::iso(date))
                        })
                        .text_size(px(self.day_title_role().size))
                        .line_height(px(self.day_title_role().line))
                        .font_weight(self.day_title_role().weight)
                        .cursor_pointer()
                        .hover(|s| s.text_color(th.link))
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
                                cx.emit(open_event(page, None, None, &e.modifiers()));
                            }
                        })),
                )
                .into_any_element(),
            // DESIGN.md §3.6: consecutive days with nothing on them stack in one block —
            // a single divider above, 44 px rows 4 px apart, each with a fixed weekday
            // column, its date title, and a ghost Write button. Clicking either target
            // expands the day with one empty block focused; nothing is written until the
            // first keystroke (FR-1.2).
            // DESIGN.md §3.6, one list item per day so a long gap is virtualized: the first
            // row of a run carries the 40 px gap and the single 1 px divider, the rest sit
            // 4 px apart. Each row has the fixed weekday column, its date and a ghost Write;
            // clicking either target expands the day with one empty block focused, and
            // nothing is written until the first keystroke (FR-1.2).
            Item::EmptyDay {
                date,
                weekday,
                title,
                first,
            } => {
                let iso = utopia_core::date::iso(date);
                let write = date;
                let row_sel = format!("empty-day-{iso}");
                let title_sel = format!("empty-title-{iso}");
                let write_sel = format!("empty-write-{iso}");
                let weekday_sel = format!("empty-weekday-{iso}");
                div()
                    .id(gpui::ElementId::Name(format!("empty-day-{iso}").into()))
                    .debug_selector(move || row_sel.clone())
                    .px(px(CONTENT_PADDING_X))
                    .when(first, |d| {
                        d.mt(px(DAY_GAP))
                            .border_t_1()
                            .border_color(th.line_soft)
                            .pt(px(DAY_HEADER_PAD_TOP))
                    })
                    .when(!first, |d| d.mt(px(EMPTY_DAY_GAP)))
                    .h(px(EMPTY_DAY_HEIGHT))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    .child(
                        div()
                            .debug_selector(move || weekday_sel.clone())
                            .w(px(EMPTY_DAY_WEEKDAY_WIDTH))
                            .font_family(LABEL_MONO)
                            .text_size(px(LABEL.size))
                            .text_color(th.muted)
                            .child(weekday),
                    )
                    .child(
                        crate::theme::EMPTY_DAY_TITLE
                            .apply(div())
                            .debug_selector(move || title_sel.clone())
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(th.ink)
                            .child(title),
                    )
                    .child(
                        quiet_control(
                            gpui::ElementId::Name(format!("empty-write-{iso}").into()),
                            "Write",
                            &th,
                        )
                        .debug_selector(move || write_sel.clone())
                        .h(px(crate::theme::GHOST_TEXT_HEIGHT))
                        .ml_2()
                        .flex()
                        .items_center()
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.start_day(write, window, cx);
                            },
                        )),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.start_day(date, window, cx)),
                    )
                    .into_any_element()
            }
            Item::DayEmpty { date } => div()
                .id(("day-empty", ix))
                .debug_selector(|| "day-empty".into())
                .px(px(CONTENT_PADDING_X))
                .py_1()
                .text_color(th.muted)
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
                    .px(px(CONTENT_PADDING_X))
                    .pt_8()
                    .pb(px(self.pad_bottom()))
                    .text_size(px(crate::theme::BODY_SIZE))
                    .text_color(th.muted)
                    .child("Loading older days…")
                    .into_any_element()
            }
            Item::JournalExhausted => div()
                .debug_selector(|| "feed-end".into())
                .px(px(CONTENT_PADDING_X))
                .pt_8()
                .pb(px(self.pad_bottom()))
                .text_size(px(crate::theme::BODY_SIZE))
                .text_color(th.muted)
                .child("No earlier notes.")
                .into_any_element(),
            // DESIGN.md §3.7: a bordered well panel with a 12 x 16 header. On a page view
            // the header carries the Filter button; in the feed it is a full-width button
            // that collapses that day's panel.
            Item::RefsHeader {
                title,
                count,
                date,
                collapsed,
            } => {
                let th = th.clone();
                let pad = if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                };
                let mut header = div()
                    .id(("refs-header", ix))
                    .debug_selector(|| "refs-header".into())
                    .mx(px(pad))
                    .mt(px(if self.is_journal() {
                        crate::theme::SPACING[5]
                    } else {
                        PAGE_REFS_GAP
                    }))
                    .bg(th.well)
                    .border_1()
                    .border_color(th.line)
                    .when(collapsed, |d| d.rounded(px(crate::theme::RADIUS_PANEL)))
                    .when(!collapsed, |d| {
                        d.rounded_t(px(crate::theme::RADIUS_PANEL)).border_b_0()
                    })
                    .px(px(crate::theme::SPACING[3]))
                    .py(px(crate::theme::SPACING[2]))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(crate::theme::SMALL.size))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.ink_2);
                match date {
                    Some(date) => {
                        let iso = utopia_core::date::iso(date);
                        header = header
                            .cursor_pointer()
                            .hover(|d| d.bg(th.hover))
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!(
                                            "refs-state-{iso}-{}",
                                            if collapsed { "closed" } else { "open" }
                                        )
                                    })
                                    .child(icon(
                                        if collapsed {
                                            Icon::ChevronRight
                                        } else {
                                            Icon::ChevronDown
                                        },
                                        crate::icons::ICON_SIZE,
                                        th.ink_3,
                                    )),
                            )
                            .child(div().debug_selector(|| "refs-title".into()).child(title))
                            .child(
                                crate::theme::count_chip(count, &th)
                                    .debug_selector(move || format!("refs-count-{count}")),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed_ref_days.remove(&date) {
                                    this.collapsed_ref_days.insert(date);
                                }
                                this.cycle.invalidate(Invalidation::rows());
                                cx.notify();
                            }));
                    }
                    None => {
                        header = header
                            .child(div().debug_selector(|| "refs-title".into()).child(title))
                            .child(
                                crate::theme::count_chip(count, &th)
                                    .debug_selector(move || format!("refs-count-{count}")),
                            )
                            .child(div().flex_1())
                            .child(
                                outline_button("refs-filter", "Filter", &th)
                                    .debug_selector(|| "refs-filter".into())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.filter.update(cx, |f, _| f.focus(window));
                                    })),
                            );
                    }
                }
                header.into_any_element()
            }
            Item::RefsEnd => {
                let pad = if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                };
                div()
                    .debug_selector(|| "refs-end".into())
                    .mx(px(pad))
                    .h(px(crate::theme::SPACING[1]))
                    .bg(th.well)
                    .border_l_1()
                    .border_r_1()
                    .border_b_1()
                    .border_color(th.line)
                    .rounded_b(px(crate::theme::RADIUS_PANEL))
                    .into_any_element()
            }
            Item::RefsControls => {
                let pad = if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                };
                div()
                    .mx(px(pad))
                    .bg(th.well)
                    .border_l_1()
                    .border_r_1()
                    .border_color(th.line)
                    .child(self.render_controls(cx))
                    .into_any_element()
            }
            Item::GroupHeader {
                page,
                count,
                scheduled,
                first,
            } => {
                let id = page.id.clone();
                let pad = if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                };
                div()
                    .id(("group", ix))
                    .flex()
                    .items_center()
                    .mx(px(pad))
                    .bg(th.well)
                    .border_l_1()
                    .border_r_1()
                    .border_color(th.line)
                    .when(!first, |d| d.border_t_1())
                    .px(px(crate::theme::SPACING[4]))
                    .pt(px(crate::theme::SPACING[3] + 2.))
                    .pb(px(crate::theme::SPACING[1]))
                    .cursor_pointer()
                    .hover(|s| s.bg(th.hover))
                    // The source page link is 14/500 `link` (DESIGN.md §3.7); the count chip
                    // and the ref type label follow it.
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .debug_selector(|| format!("group-title-{ix}"))
                                    .text_size(px(crate::theme::TAG.size))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(th.link)
                                    .child(page.title.clone()),
                            )
                            .child(
                                crate::theme::count_chip(count, &th)
                                    .debug_selector(|| format!("group-count-{ix}-{count}")),
                            )
                            .when(scheduled, |d| {
                                d.child(
                                    ref_type_label("Scheduled for this day", &th)
                                        .debug_selector(|| "ref-type-label".into()),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |this, e: &gpui::ClickEvent, _, cx| {
                        this.editor.update(cx, |ed, cx| ed.end(cx));
                        cx.emit(open_event(id.clone(), None, None, &e.modifiers()));
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
                let pad = if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                };
                div()
                    .mx(px(pad))
                    .bg(th.well)
                    .border_l_1()
                    .border_r_1()
                    .border_color(th.line)
                    .child(crate::reference::reference_row(
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
                        &self.self_titles,
                        crate::theme::SPACING[4],
                        cx.entity(),
                    ))
                    .into_any_element()
            }
            Item::More { page, hidden } => {
                let pad = if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                };
                div()
                    .id(("more", ix))
                    .mx(px(pad))
                    .bg(th.well)
                    .border_l_1()
                    .border_r_1()
                    .border_color(th.line)
                    .px(px(crate::theme::SPACING[4]))
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.link)
                    .cursor_pointer()
                    .child(format!("+{hidden} more"))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_expanded(page.clone(), cx)),
                    )
                    .into_any_element()
            }
            Item::NoRefs => {
                let pad = if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                };
                div()
                    .mx(px(pad))
                    .bg(th.well)
                    .border_l_1()
                    .border_r_1()
                    .border_color(th.line)
                    .px(px(crate::theme::SPACING[4]))
                    .pb_6()
                    .text_size(px(crate::theme::BODY_SIZE))
                    .text_color(th.muted)
                    .child("No linked references yet.")
                    .into_any_element()
            }
            // DESIGN.md §3.7: a collapsed row at least 44 px tall with a top divider and a
            // chevron-right. The mentions are searched when it is expanded, not before.
            Item::UnlinkedHeader { count, open } => div()
                .id("unlinked-header")
                .debug_selector(|| "unlinked-header".into())
                .mt_4()
                .mx(px(if self.compact {
                    crate::theme::SPACING[3]
                } else {
                    CONTENT_PADDING_X
                }))
                .h(px(UNLINKED_ROW_HEIGHT))
                .px_2()
                .border_t_1()
                .border_color(th.line)
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(crate::theme::SMALL.size))
                .text_color(th.ink_2)
                .cursor_pointer()
                .hover(|s| s.bg(th.hover))
                .child(
                    div()
                        .debug_selector(move || format!("unlinked-arrow-{open}"))
                        .child(icon(
                            if open {
                                crate::icons::Icon::ChevronDown
                            } else {
                                crate::icons::Icon::ChevronRight
                            },
                            crate::icons::ICON_SIZE,
                            th.ink_3,
                        )),
                )
                .child(
                    div()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child("Unlinked references"),
                )
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
                    if this.unlinked_open {
                        // The one place the FTS scan runs (DESIGN.md §3.7, rule 10).
                        this.refresh_unlinked(cx);
                    }
                    this.cycle.invalidate(Invalidation::rows());
                    cx.notify();
                }))
                .into_any_element(),
            Item::Mention(m) => {
                let th2 = th.clone();
                let highlight = gpui::HighlightStyle {
                    background_color: Some(th2.highlight_bg),
                    ..Default::default()
                };
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
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(crate::theme::CAPTION_SIZE))
                                    .text_color(th.link)
                                    .child(m.page_title.clone()),
                            )
                            .child(
                                div()
                                    .debug_selector(move || format!("mention-text-{ix}"))
                                    .text_size(px(crate::theme::REFERENCE.size))
                                    .line_height(px(crate::theme::REFERENCE.line))
                                    .text_color(th.ink_2)
                                    .child(self.render_rich_with(
                                        &m.text,
                                        &m.block,
                                        None,
                                        crate::theme::REFERENCE.line,
                                        0,
                                        Some((m.range.clone(), highlight)),
                                        false,
                                        cx,
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .id(("link-mention", ix))
                            .debug_selector(|| format!("link-{ix}"))
                            .px_2()
                            .rounded(px(crate::theme::RADIUS_CHIP))
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.link)
                            .border_1()
                            .border_color(th.line)
                            .cursor_pointer()
                            .hover(|s| s.bg(th2.hover))
                            .child("Link")
                            .on_click(cx.listener(move |this, _, _, cx| this.promote(&m2, cx))),
                    )
                    .into_any_element()
            }
            Item::NoMentions => div()
                .debug_selector(|| "unlinked-empty".into())
                .px_6()
                .py_0p5()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .child("No unlinked mentions")
                .into_any_element(),
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

/// DESIGN.md §3.8: the status value is title-cased for display only.
fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The avatar's initials: the first letters of the first and last words ("Ana Lovelace" →
/// "AL"), two letters of a single word.
fn initials(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().collect();
    let mut out = String::new();
    if let Some(first) = words.first() {
        out.extend(first.chars().next().map(|c| c.to_uppercase().to_string()));
        if let Some(last) = words.last().filter(|_| words.len() > 1) {
            out.extend(last.chars().next().map(|c| c.to_uppercase().to_string()));
        } else {
            out.extend(first.chars().nth(1).map(|c| c.to_uppercase().to_string()));
        }
    }
    if out.is_empty() {
        out.push('?');
    }
    out
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

/// Push a page's visible rows. `skip` holds the leading property blocks the properties bar
/// already draws (DESIGN.md §3.8 rule 2): they and their subtrees are not drawn again.
#[allow(clippy::too_many_arguments)]
fn push_page_rows(
    app: &mut AppState,
    st: &utopia_core::PageState,
    page: &PageId,
    zoom: Option<&BlockId>,
    collapsed: &HashSet<BlockId>,
    open_embeds: &HashSet<(BlockId, String)>,
    skip: &[BlockId],
    out: &mut Vec<Item>,
) {
    let mut folded: Option<usize> = None;
    for (id, depth) in st.visible_rows(zoom, &|b| collapsed.contains(b)) {
        if folded.is_some_and(|d| depth > d) {
            continue;
        }
        folded = None;
        let Some(b) = st.block(&id) else { continue };
        if skip.contains(&id) {
            folded = Some(depth);
            continue;
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
        .w(px(BULLET_SLOT))
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
                    .child(div().size(px(2.)).rounded_full().bg(th.bullet))
                    .child(div().size(px(2.)).rounded_full().bg(th.bullet)),
            );
        }
        return cell.child(grip).into_any_element();
    }
    // DESIGN.md §3.1: a 6 px `bullet` dot; a collapsed block puts a 12 px `line_strong`
    // ring around a 6 px `ink_3` dot so it reads as "there is more in here".
    if r.collapsed && r.has_children {
        return cell
            .child(
                div()
                    .debug_selector(move || format!("ring-{ix}"))
                    .size(px(COLLAPSED_RING))
                    .rounded_full()
                    .border_1()
                    .border_color(th.line_strong)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .debug_selector(move || format!("dot-{ix}"))
                            .size(px(BULLET_SIZE))
                            .rounded_full()
                            .bg(th.ink_3),
                    ),
            )
            .into_any_element();
    }
    cell.child(
        div()
            .debug_selector(move || format!("dot-{ix}"))
            .size(px(BULLET_SIZE))
            .rounded_full()
            .bg(th.bullet),
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
        let (day_role, sticky_h) = (self.day_title_role(), self.sticky_height());
        div()
            .size_full()
            .bg(th.paper)
            .relative()
            // DN-3: the pinned day heading is drawn by this view from the list's scroll
            // offset, and scrolling the list does not re-render its parent. Ask for a frame;
            // the listener cannot read the list here (its state is borrowed during the
            // scroll), so `render` reads the new offset.
            .on_scroll_wheel(cx.listener(|_, _: &gpui::ScrollWheelEvent, _, cx| cx.notify()))
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
                            div().w_full().max_w(px(CONTENT_MAX_WIDTH)).child(
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
                                .max_w(px(CONTENT_MAX_WIDTH))
                                .h(px(sticky_h))
                                .px(px(CONTENT_PADDING_X))
                                .pt(px(DAY_HEADER_PAD_TOP))
                                .pb(px(DAY_HEADER_GAP))
                                .bg(th.paper)
                                .border_b_1()
                                .border_color(th.line)
                                .child(
                                    div()
                                        .h(px(DAY_CAPTION_LINE))
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .font_family(LABEL_MONO)
                                        .text_size(px(LABEL.size))
                                        .text_color(th.muted)
                                        .child(h.weekday.clone())
                                        .when(h.today, |d| {
                                            d.child(
                                                today_pill("Today", &th)
                                                    .id("sticky-today-pill")
                                                    .debug_selector(|| "sticky-today-pill".into())
                                                    .cursor_pointer()
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.scroll_to_today(cx)
                                                    })),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(day_role.size))
                                        .line_height(px(day_role.line))
                                        .font_weight(day_role.weight)
                                        .child(h.title.clone()),
                                ),
                        ),
                )
            })
    }
}

/// DESIGN.md §3.5: the mono weekday label above a day's title, in upper case.
/// Display only (§10.3: a day's identity is its date).
fn weekday_label(date: NaiveDate) -> String {
    utopia_core::date::weekday_name_of(date).to_uppercase()
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

    /// DESIGN.md §3.5: the label is the weekday alone, upper case (the Today pill says
    /// "today", so the label never repeats it).
    #[test]
    fn weekday_labels_are_upper_case_weekdays() {
        // Thursday, November 5th, 2026.
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        assert_eq!(weekday_label(today), "THURSDAY");
        assert_eq!(weekday_label(today.pred_opt().unwrap()), "WEDNESDAY");
        assert_eq!(weekday_label(today - chrono::Duration::days(3)), "MONDAY");
    }
}
