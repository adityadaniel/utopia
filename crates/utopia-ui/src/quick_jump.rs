//! Quick jump (Cmd+K, §9.1): fuzzy titles and aliases; Enter opens, Shift/Cmd+Enter opens
//! in the sidebar, and a novel title offers "Create". Dates resolve to daily pages.
//!
//! One shell is shared by every palette mode (quick jump, full-text search, quick capture,
//! free text). In page mode it has a useful empty state — today, yesterday and the pages
//! opened last (QJ-1) — labelled rows with an icon per type (QJ-2), the typed letters in
//! semibold (QJ-4), a footer of key hints (QJ-3), and, after a leading `>`, the actions
//! from [`crate::actions::CATALOG`] with their keys (QJ-5). A query also lists block hits
//! grouped under their page or day (SR-2), with scope and age chips (SR-3), the matched
//! words washed (SR-1) and a preview of the selected block (SR-4); page mode starts on the
//! first page, search on the first block (SR-5).
//!
//! All of this is view state. Rows come from the index (`Store::search_titles`,
//! `Store::tag_pages`) when the query changes, never while painting (rule 3), and nothing
//! here writes a shard.

use std::collections::HashSet;
use std::ops::Range;

use chrono::NaiveDate;
use gpui::{
    anchored, deferred, div, point, prelude::*, px, AnyElement, App, Context, Corner, Entity,
    EventEmitter, FontWeight, HighlightStyle, MouseButton, MouseDownEvent, ScrollHandle,
    StyledText, Subscription, Window,
};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, PageId};
use utopia_store::{BlockGroup, BlockHit, BlockPreview, SearchAge, SearchScope, TitleHit};

use crate::actions::{active_keys, pretty_keys, ActionDoc, CATALOG};
use crate::app_state::AppState;
use crate::editor::BlockEditor;
use crate::line_input::{LineInput, LineInputEvent};
use crate::theme::{quiet_control, quiet_menu, theme, Theme, CAPTION_SIZE, CONTROL_SIZE};

/// How many pages the empty state offers after today and yesterday (QJ-1).
pub const RECENT_ROWS: usize = 8;

#[derive(Debug, Clone)]
pub enum QjItem {
    Page(TitleHit),
    /// A day offered by the empty state (QJ-1). It opens by date, so a day nobody has
    /// written yet still opens as a draft rather than as a page (FR-1.2).
    Day {
        date: NaiveDate,
        title: String,
        /// "Today" or "Yesterday": the type label of a pinned day.
        label: &'static str,
    },
    Create(String),
    /// An action from [`CATALOG`], listed after a leading `>` (QJ-5).
    Command(&'static ActionDoc),
}

/// One line in the scrollable list. Page/day/command/create rows come from `results`;
/// block hits are appended under their page group (SR-2), and the Create row stays last
/// (QJ-2). Arrow keys step over group headers.
#[derive(Debug, Clone)]
enum Row {
    Item(usize),
    Group { group: usize },
    Block { group: usize, hit: usize },
}

impl Row {
    fn selectable(&self) -> bool {
        !matches!(self, Row::Group { .. })
    }
}

/// Whether the selected scope may list a page title (SR-3): Journal is days only, Pages
/// is ordinary pages only, To-dos is blocks only, All is everything.
fn scope_keeps_page(scope: SearchScope, daily: bool) -> bool {
    match scope {
        SearchScope::All => true,
        SearchScope::Journal => daily,
        SearchScope::Pages => !daily,
        SearchScope::Todos => false,
    }
}

/// A block row's text with the matched terms on a soft link wash (SR-1). The anchors
/// come from the store, which computed them against this same plain text.
fn marked(text: &str, anchors: &[(usize, usize)], th: &Theme) -> StyledText {
    let highlights: Vec<(Range<usize>, HighlightStyle)> = anchors
        .iter()
        .filter(|(a, b)| {
            a < b && *b <= text.len() && text.is_char_boundary(*a) && text.is_char_boundary(*b)
        })
        .map(|&(a, b)| {
            (
                a..b,
                HighlightStyle {
                    background_color: Some(th.link.opacity(0.16)),
                    ..Default::default()
                },
            )
        })
        .collect();
    StyledText::new(text.to_string()).with_highlights(highlights)
}

/// The page/day header of a result group: an icon on the top line, the name and the block
/// count. `items_start` keeps the icon with the first line when a long title wraps (SR-2).
fn group_header(group: usize, g: &BlockGroup, th: &Theme) -> gpui::Stateful<gpui::Div> {
    let icon = if g.daily { "◷" } else { "▤" };
    div()
        .id(("search-group", group))
        .debug_selector(move || format!("search-group-{group}"))
        .flex()
        .items_start()
        .gap_2()
        .px_3()
        .pt_2()
        .pb_1()
        .child(
            div()
                .debug_selector(move || format!("search-group-icon-{group}"))
                .w(px(14.))
                .flex_none()
                .text_color(th.muted)
                .child(icon),
        )
        .child(
            div()
                .debug_selector(move || format!("search-group-title-{group}"))
                .flex_1()
                .min_w_0()
                .text_color(th.ink)
                .child(g.title.clone()),
        )
        .child(
            div()
                .flex_none()
                .px_2()
                .rounded(px(crate::theme::RADIUS_CONTROL))
                .bg(th.line_soft)
                .text_size(px(CAPTION_SIZE))
                .text_color(th.muted)
                .child(format!("{}", g.hits.len())),
        )
}

/// What a result row is. The palette draws one glyph and one type label per kind (QJ-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Page,
    Day,
    Tag,
    Command,
    Create,
}

impl RowKind {
    /// The name the row's debug selector carries, for layout tests.
    fn key(self) -> &'static str {
        match self {
            RowKind::Page => "page",
            RowKind::Day => "day",
            RowKind::Tag => "tag",
            RowKind::Command => "command",
            RowKind::Create => "create",
        }
    }

    /// The type label at the right edge of the row.
    fn label(self) -> &'static str {
        match self {
            RowKind::Page => "Page",
            RowKind::Day => "Day",
            RowKind::Tag => "Tag",
            RowKind::Command => "",
            RowKind::Create => "",
        }
    }
}

/// What the palette is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    /// Cmd+K: titles, aliases and dates.
    Pages,
    /// Cmd+Shift+F: full-text search over block bodies (P1, FTS5).
    Search,
    /// Cmd+Shift+Space: one line appended to today's note (FR-2.19).
    Capture,
    /// A free-text answer (e.g. the title for "Extract block to page").
    Text,
}

#[derive(Debug, Clone)]
pub enum QuickJumpEvent {
    Chosen {
        page: PageId,
        sidebar: bool,
        focus: Option<BlockId>,
    },
    /// A day from the empty state (QJ-1): opened by date, draft or not.
    ChosenDay {
        date: NaiveDate,
        sidebar: bool,
    },
    /// A command from the catalog (QJ-5), by its `keymap.json` name.
    RunCommand(&'static str),
    Text(String),
    Dismissed,
}

pub struct JumpPalette {
    app: Entity<AppState>,
    /// The shared block editor: a command bound to the editor (`Indent`, `Bold`, …) can
    /// only act while a block is in edit (QJ-5).
    editor: Entity<BlockEditor>,
    pub input: Entity<LineInput>,
    results: Vec<QjItem>,
    /// Block hits under their page or day (SR-2).
    groups: Vec<BlockGroup>,
    /// The combined draw/selection order: page rows, then grouped blocks, then Create.
    rows: Vec<Row>,
    /// Ids among `results` that exist only because of a `#tag` (QJ-2).
    tags: HashSet<PageId>,
    /// The pages opened last, newest first, when this palette offers the empty state
    /// (QJ-1). `None` for the modes where the empty state would be a lie: redirecting a
    /// link or extracting a block has nothing to do with the pages you just visited.
    recent: Option<Vec<PageId>>,
    selected: usize,
    /// The result list, so ↑/↓ can bring the selected row into view: the command catalog is
    /// longer than the palette, and Enter would otherwise run a command nobody can see.
    scroll: ScrollHandle,
    title: &'static str,
    mode: PaletteMode,
    scope: SearchScope,
    age: SearchAge,
    age_menu: bool,
    /// The selected block in full (SR-4), read when the selection moves.
    preview: Option<BlockPreview>,
    _sub: Subscription,
}

impl EventEmitter<QuickJumpEvent> for JumpPalette {}

impl JumpPalette {
    pub fn with_mode(
        app: Entity<AppState>,
        editor: Entity<BlockEditor>,
        title: &'static str,
        mode: PaletteMode,
        recent: Option<Vec<PageId>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = match mode {
            PaletteMode::Pages => "Go to page or date, or > for a command…",
            PaletteMode::Search => "Search all blocks…",
            PaletteMode::Capture => "Capture to today's note…",
            PaletteMode::Text => "Title…",
        };
        let input = cx.new(|cx| LineInput::new(placeholder, cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, e: &LineInputEvent, window, cx| match e {
                LineInputEvent::Changed(q) => this.search(q, cx),
                LineInputEvent::Up => this.step(-1, cx),
                LineInputEvent::Down => this.step(1, cx),
                LineInputEvent::Submit { alt } => this.choose(this.selected, *alt, window, cx),
                LineInputEvent::Cancel => cx.emit(QuickJumpEvent::Dismissed),
            },
        );
        input.update(cx, |i, _| i.focus(window));
        let mut me = Self {
            app,
            editor,
            input,
            results: Vec::new(),
            groups: Vec::new(),
            rows: Vec::new(),
            tags: HashSet::new(),
            recent,
            selected: 0,
            scroll: ScrollHandle::new(),
            title,
            mode,
            scope: SearchScope::default(),
            age: SearchAge::default(),
            age_menu: false,
            preview: None,
            _sub: sub,
        };
        me.search("", cx);
        me
    }

    fn search(&mut self, q: &str, cx: &mut Context<Self>) {
        self.results.clear();
        self.groups.clear();
        self.rows.clear();
        self.tags.clear();
        self.preview = None;
        if matches!(self.mode, PaletteMode::Capture | PaletteMode::Text) {
            cx.notify();
            return;
        }
        // QJ-5: a leading `>` lists the command catalog instead of pages (page mode only).
        if self.mode == PaletteMode::Pages {
            if let Some(rest) = q.strip_prefix('>') {
                self.results = commands(rest, |d| active_keys(cx, d))
                    .into_iter()
                    .map(QjItem::Command)
                    .collect();
                self.rebuild_rows();
                self.select_initial();
                cx.notify();
                return;
            }
            // QJ-1: with nothing typed, offer today, yesterday and the pages opened last.
            if q.trim().is_empty() {
                self.results = self
                    .empty_state(cx)
                    .into_iter()
                    .filter(|r| match r {
                        QjItem::Day { .. } => scope_keeps_page(self.scope, true),
                        QjItem::Page(h) => scope_keeps_page(self.scope, h.daily),
                        _ => true,
                    })
                    .collect();
                self.rebuild_rows();
                self.select_initial();
                self.rank_tags(cx);
                cx.notify();
                return;
            }
        }
        // Search mode has no empty state: with nothing typed there is nothing to find.
        if self.mode == PaletteMode::Search && q.trim().is_empty() {
            cx.notify();
            return;
        }
        self.search_query(q, cx);
        self.rebuild_rows();
        self.select_initial();
        self.rank_tags(cx);
        self.load_preview(cx);
        cx.notify();
    }

    /// Page titles and grouped block hits for a non-empty query (SR-5: pages first).
    fn search_query(&mut self, q: &str, cx: &mut Context<Self>) {
        let store = &self.app.read(cx).store;
        let all_hits = store
            .search_titles_scoped(q, self.scope, 12)
            .unwrap_or_default();
        let key = normalize_title(q);
        let exact = all_hits.iter().any(|h| {
            normalize_title(&h.title) == key
                || h.alias.as_deref().map(normalize_title) == Some(key.clone())
        }) || store.find_page(q).ok().flatten().is_some();
        // The chips filter title rows as well as block groups (SR-3).
        let mut results: Vec<QjItem> = all_hits.into_iter().map(QjItem::Page).collect();
        // Block search lists existing blocks; only quick jump creates a page (SR-5).
        // Journal and To-dos cannot create a page, and Pages must not create a daily one.
        let date_like =
            utopia_core::date::parse_unambiguous_date(&key, store.date_format()).is_some();
        let can_create = self.mode == PaletteMode::Pages
            && matches!(self.scope, SearchScope::All | SearchScope::Pages)
            && (self.scope == SearchScope::All || !date_like);
        if can_create && !key.is_empty() && !exact {
            results.push(QjItem::Create(q.trim().to_string()));
        }
        // A date typed in any accepted form jumps straight to its day (if in scope).
        if let Ok(Some(p)) = store.find_page(q) {
            if !results
                .iter()
                .any(|r| matches!(r, QjItem::Page(h) if h.page == p))
            {
                if let Ok(Some(row)) = store.page_row(&p) {
                    let daily = row.is_daily();
                    if scope_keeps_page(self.scope, daily) {
                        results.insert(
                            0,
                            QjItem::Page(TitleHit {
                                page: p,
                                title: row.title,
                                alias: None,
                                daily,
                                score: i64::MAX,
                            }),
                        );
                    }
                }
            }
        }
        self.groups = store
            .search_groups_filtered(q, self.scope, self.age, 30)
            .unwrap_or_default();
        self.results = results;
    }

    /// Page rows first, then each page's block group, then Create last (QJ-2, SR-2).
    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (i, item) in self.results.iter().enumerate() {
            if !matches!(item, QjItem::Create(_)) {
                self.rows.push(Row::Item(i));
            }
        }
        for gi in 0..self.groups.len() {
            self.rows.push(Row::Group { group: gi });
            for hi in 0..self.groups[gi].hits.len() {
                self.rows.push(Row::Block { group: gi, hit: hi });
            }
        }
        if let Some(i) = self
            .results
            .iter()
            .position(|r| matches!(r, QjItem::Create(_)))
        {
            self.rows.push(Row::Item(i));
        }
    }

    /// Cmd+K starts on the first page, Cmd+Shift+F on the first block (SR-5); if there
    /// is no row of that kind, the first selectable one. A new query also scrolls back
    /// to the top so the selection and Enter agree.
    fn select_initial(&mut self) {
        let first_block = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Block { .. }));
        let first_selectable = self.rows.iter().position(Row::selectable);
        self.selected = if self.mode == PaletteMode::Search {
            first_block.or(first_selectable)
        } else {
            first_selectable
        }
        .unwrap_or_default();
        self.scroll.scroll_to_item(self.selected);
    }

    /// The empty state (QJ-1): today, yesterday, then the pages opened last, newest
    /// first. Recent pages come from local UI state the workspace keeps (rule 9); a day
    /// nobody has written yet is offered by date (FR-1.2).
    fn empty_state(&self, cx: &mut Context<Self>) -> Vec<QjItem> {
        let app = self.app.read(cx);
        let store = &app.store;
        let today = store.today();
        let yesterday = today.pred_opt();
        let fmt = store.date_format().clone();
        let mut out: Vec<QjItem> = vec![QjItem::Day {
            date: today,
            title: fmt.format(today),
            label: "Today",
        }];
        if let Some(d) = yesterday {
            out.push(QjItem::Day {
                date: d,
                title: fmt.format(d),
                label: "Yesterday",
            });
        }
        let Some(recent) = &self.recent else {
            // Not a "go to page" palette: no empty state.
            return Vec::new();
        };
        for id in recent {
            if out.len() >= 2 + RECENT_ROWS {
                break;
            }
            let Ok(Some(row)) = store.page_row(id) else {
                continue; // a draft day that was never written
            };
            // Deleted pages, and the tombstoned source of a merge, stay in `local_state`
            // but must not be offered — nor keep a slot from a live page (QJ-1).
            if !row.is_live() {
                continue;
            }
            // Today and yesterday are already the first two rows.
            if row.is_daily() && (row.daily_date == Some(today) || row.daily_date == yesterday) {
                continue;
            }
            out.push(QjItem::Page(TitleHit {
                page: row.id.clone(),
                daily: row.is_daily(),
                title: row.title,
                alias: None,
                score: 0,
            }));
        }
        out
    }

    /// Mark the pages that exist only because of a `#tag` (QJ-2), from the classification
    /// the commit path published and [`AppState`] cached (SB-2): no query runs here, and
    /// nothing is recomputed per keystroke (rule 3, rule 10).
    fn rank_tags(&mut self, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        self.tags = self
            .results
            .iter()
            .filter_map(|r| match r {
                QjItem::Page(h) => Some(h.page.clone()),
                _ => None,
            })
            .filter(|page| app.tag_pages.contains(page))
            .collect();
    }

    /// A new query starts again at the top, scrolled back to it, so ↑/↓ and Enter agree
    /// on which row is the current one.
    fn step(&mut self, d: isize, cx: &mut Context<Self>) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let mut i = self.selected;
        for _ in 0..n {
            i = (i as isize + d).rem_euclid(n as isize) as usize;
            if self.rows[i].selectable() {
                self.selected = i;
                // Keep the row Enter would open/run visible, past the group headers.
                self.scroll.scroll_to_item(i);
                self.load_preview(cx);
                cx.notify();
                return;
            }
        }
    }

    fn choose(&mut self, ix: usize, sidebar: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.mode, PaletteMode::Capture | PaletteMode::Text) {
            let text = self.input.read(cx).text().trim().to_string();
            if !text.is_empty() {
                cx.emit(QuickJumpEvent::Text(text));
            }
            return;
        }
        let Some(row) = self.rows.get(ix).cloned() else {
            // Nothing matched: only quick jump may create, and only when the scope allows
            // it (the Create row is what carries that decision; SR-3).
            if self.mode == PaletteMode::Search || !self.offers_create() {
                return;
            }
            let q = self.input.read(cx).text().to_string();
            if q.trim().is_empty() || q.starts_with('>') {
                return;
            }
            return self.create(&q, sidebar, cx);
        };
        match row {
            Row::Item(i) => match self.results[i].clone() {
                QjItem::Page(h) => cx.emit(QuickJumpEvent::Chosen {
                    page: h.page,
                    sidebar,
                    focus: None,
                }),
                QjItem::Day { date, .. } => cx.emit(QuickJumpEvent::ChosenDay { date, sidebar }),
                QjItem::Command(d) => {
                    if self.command_ready(d, cx) {
                        cx.emit(QuickJumpEvent::RunCommand(d.action));
                    }
                }
                QjItem::Create(t) => self.create(&t, sidebar, cx),
            },
            Row::Block { group, hit } => {
                let Some(b) = self.groups.get(group).and_then(|g| g.hits.get(hit)) else {
                    return;
                };
                cx.emit(QuickJumpEvent::Chosen {
                    page: b.page.clone(),
                    sidebar,
                    focus: Some(b.block.clone()),
                });
            }
            Row::Group { .. } => {}
        }
    }

    fn create(&mut self, title: &str, sidebar: bool, cx: &mut Context<Self>) {
        // A new page opens with the caret in its first block, so typing continues there.
        let created = self.app.update(cx, |app, cx| {
            let r = app
                .store
                .create_page(title)
                .and_then(|p| app.store.ensure_first_block(&p).map(|b| (p, b)));
            let p = app.report(r);
            app.changed(Vec::new(), true, cx);
            p
        });
        if let Some((page, block)) = created {
            cx.emit(QuickJumpEvent::Chosen {
                page,
                sidebar,
                focus: Some(block),
            });
        }
    }

    /// Whether a command can act right now (QJ-5). `Indent`, `Bold`, `Newline` and the
    /// rest of the editor bindings are handled by [`BlockEditor`]; with no block in edit
    /// gpui would dispatch them to nobody, so the palette marks them unavailable instead of
    /// offering a command that silently does nothing.
    fn command_ready(&self, d: &ActionDoc, cx: &App) -> bool {
        d.context.is_none() || self.editor.read(cx).target().is_some()
    }

    /// Results by page, for the scope tests and the preview (SR-2, SR-3).
    pub fn groups(&self) -> &[BlockGroup] {
        &self.groups
    }

    /// The selected block's preview (SR-4), `None` when a page row is selected.
    pub fn preview(&self) -> Option<&BlockPreview> {
        self.preview.as_ref()
    }

    /// Whether the list offers "Create page" (quick jump only; SR-5).
    pub fn offers_create(&self) -> bool {
        self.results.iter().any(|r| matches!(r, QjItem::Create(_)))
    }

    /// Page titles currently listed, for the scope tests (SR-3).
    pub fn page_titles(&self) -> Vec<String> {
        self.results
            .iter()
            .filter_map(|r| match r {
                QjItem::Page(h) => Some(h.title.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn scope(&self) -> SearchScope {
        self.scope
    }

    pub fn age(&self) -> SearchAge {
        self.age
    }

    fn selected_hit(&self) -> Option<&BlockHit> {
        match self.rows.get(self.selected) {
            Some(Row::Block { group, hit }) => self.groups.get(*group)?.hits.get(*hit),
            _ => None,
        }
    }

    /// Read the preview for the current selection. Store work stays out of `render` (rule
    /// 10); page mode is a jump list, so it does not preview blocks.
    fn load_preview(&mut self, cx: &mut Context<Self>) {
        if self.mode != PaletteMode::Search {
            self.preview = None;
            return;
        }
        let block = self.selected_hit().map(|h| h.block.clone());
        self.preview = block.and_then(|b| {
            let app = self.app.read(cx);
            match app.store.preview_block(&b) {
                Ok(p) => p,
                Err(e) => {
                    // Rule 11: a preview that cannot be read logs and leaves the pane empty.
                    eprintln!("utopia: {e}");
                    None
                }
            }
        });
    }

    /// The state of a previewed block as a short label (SR-4).
    fn state_label(kind: utopia_core::BlockKind, checked: bool) -> &'static str {
        use utopia_core::BlockKind::*;
        match kind {
            Todo if checked => "Done",
            Todo => "To-do",
            Heading(1) => "Heading 1",
            Heading(2) => "Heading 2",
            Heading(_) => "Heading 3",
            Quote => "Quote",
            Code => "Code",
            Numbered => "Numbered",
            Bullet => "Bullet",
        }
    }

    /// The right half: the selected block's path, text, state and first children (SR-4).
    fn preview_pane(
        &self,
        p: &BlockPreview,
        th: &Theme,
        width: f32,
        height: f32,
    ) -> gpui::Stateful<gpui::Div> {
        let mut crumbs: Vec<String> = vec![p.page_title.clone()];
        crumbs.extend(p.path.iter().map(|(_, t)| {
            let mut s: String = t.chars().take(40).collect();
            if t.chars().count() > 40 {
                s.push('…');
            }
            s
        }));
        let body = utopia_core::inline::plain_text(&p.text);
        let anchors = self
            .selected_hit()
            .map(|h| h.anchors.as_slice())
            .unwrap_or(&[]);
        let mut pane = div()
            .id("search-preview")
            .debug_selector(|| "search-preview".into())
            .w(px(width))
            .flex_none()
            .max_h(px(height))
            .overflow_y_scroll()
            .pl_3()
            .border_l_1()
            .border_color(th.line)
            .flex()
            .flex_col()
            .gap_2();
        pane = pane.child(
            div()
                .debug_selector(|| "search-preview-path".into())
                .text_size(px(CAPTION_SIZE))
                .text_color(th.muted)
                .line_clamp(2)
                .child(crumbs.join(" › ")),
        );
        let label = Self::state_label(p.kind, p.checked);
        let muted_state = p.kind == utopia_core::BlockKind::Bullet
            || (p.kind == utopia_core::BlockKind::Todo && p.checked);
        let fg = if muted_state { th.muted } else { th.link };
        let mut state = div().flex().items_center().gap_2().child(
            div()
                .debug_selector(|| "search-preview-state".into())
                .px_2()
                .py_0p5()
                .rounded(px(crate::theme::RADIUS_CONTROL))
                .bg(th.line_soft)
                .text_size(px(CAPTION_SIZE))
                .text_color(fg)
                .child(label),
        );
        if let Some(when) = chrono::DateTime::from_timestamp_millis(p.updated_at) {
            state = state.child(
                div()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(format!("Edited {}", when.format("%b %-d, %Y"))),
            );
        }
        pane = pane
            .child(state)
            .child(
                div()
                    .text_color(th.ink)
                    .line_clamp(6)
                    .child(marked(&body, anchors, th)),
            )
            .child(
                div()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(match p.child_count {
                        0 => "No children".to_string(),
                        1 => "1 child".to_string(),
                        n => format!("{n} children"),
                    }),
            );
        for (i, c) in p.children.iter().enumerate() {
            let lead = match (c.kind, c.checked) {
                (utopia_core::BlockKind::Todo, true) => "☑",
                (utopia_core::BlockKind::Todo, false) => "☐",
                _ => "•",
            };
            pane = pane.child(
                div()
                    .debug_selector(move || format!("search-preview-child-{i}"))
                    .flex()
                    .gap_2()
                    .items_start()
                    .child(div().flex_none().text_color(th.muted).child(lead))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(if c.checked { th.muted } else { th.ink })
                            .line_clamp(2)
                            .child(utopia_core::inline::plain_text(&c.text)),
                    ),
            );
        }
        if p.child_count > p.children.len() {
            pane = pane.child(
                div()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(format!("+{} more", p.child_count - p.children.len())),
            );
        }
        pane
    }

    /// The scope and age chips under the field (SR-3).
    fn scope_bar(&self, th: &Theme, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut bar = div()
            .id("search-scopes")
            .debug_selector(|| "search-scopes".into())
            .flex()
            .items_center()
            .gap_1()
            .px_1();
        for &s in SearchScope::ALL {
            let selected = s == self.scope;
            let key = s.label().to_lowercase().replace(' ', "-");
            bar = bar.child(
                quiet_control(
                    gpui::ElementId::Name(format!("search-scope-{key}").into()),
                    s.label(),
                    th,
                )
                .debug_selector(move || format!("search-scope-{key}"))
                .when(selected, |d| d.bg(th.line_soft).text_color(th.link))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.scope = s;
                    this.age_menu = false;
                    this.refilter(cx);
                })),
            );
        }
        let menu_open = self.age_menu;
        bar.child(div().flex_1()).child(
            div()
                .relative()
                .child(
                    quiet_menu("search-age", self.age.label(), th)
                        .debug_selector(|| "search-age".into())
                        .when(menu_open, |d| d.bg(th.line_soft).text_color(th.link))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.age_menu = !this.age_menu;
                            cx.notify();
                        })),
                )
                .children(menu_open.then(|| {
                    deferred(
                        anchored()
                            .anchor(Corner::TopRight)
                            .offset(point(px(0.), px(6.)))
                            .child(self.age_popup(th, cx)),
                    )
                    .with_priority(1)
                })),
        )
    }

    /// The "Any time" menu, drawn deferred so opening it never moves the chips or results.
    fn age_popup(&self, th: &Theme, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut menu = div()
            .id("search-age-popup")
            .debug_selector(|| "search-age-popup".into())
            .w(px(170.))
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .rounded(px(crate::theme::RADIUS_CONTROL))
            // The results list is underneath: stop the mouse-down here or a row would
            // choose itself before the menu item's click runs (found by the scope test).
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        for &a in SearchAge::ALL {
            let selected = a == self.age;
            let key = a.label().to_lowercase().replace(' ', "-");
            menu = menu.child(
                quiet_control(
                    gpui::ElementId::Name(format!("search-age-item-{key}").into()),
                    a.label(),
                    th,
                )
                .debug_selector(move || format!("search-age-{key}"))
                .w_full()
                .justify_start()
                .when(selected, |d| d.bg(th.line_soft).text_color(th.link))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.age = a;
                    this.age_menu = false;
                    this.refilter(cx);
                })),
            );
        }
        menu
    }

    /// Re-run the same query under a new chip (SR-3). The typed text stays in the field.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).text().to_string();
        self.search(&q, cx);
    }

    /// One grouped block hit (SR-2): indented under its page header, its matched words
    /// washed (SR-1), and selectable like any other row.
    fn block_row(
        &self,
        i: usize,
        group: usize,
        hit: usize,
        th: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(b) = self.groups.get(group).and_then(|g| g.hits.get(hit)) else {
            return div().into_any_element();
        };
        let text = utopia_core::inline::plain_text(&b.text);
        div()
            .id(("search-hit", i))
            .debug_selector(move || format!("search-hit-{group}-{hit}"))
            .px_3()
            .py_1()
            .pl(px(46.))
            .rounded(px(crate::theme::RADIUS_CHIP))
            .cursor_pointer()
            .hover(|d| d.bg(th.hover))
            .when(i == self.selected, |d| d.bg(th.hover))
            .child(
                div()
                    .debug_selector(move || format!("search-hit-text-{group}-{hit}"))
                    .text_color(th.ink)
                    .line_clamp(2)
                    .child(marked(&text, &b.anchors, th)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, w, cx| {
                    let beside = e.modifiers.shift || e.modifiers.platform;
                    this.choose(i, beside, w, cx)
                }),
            )
            .into_any_element()
    }

    /// What a row says, how it is labelled and what it is (QJ-2). `bound` is the keys in
    /// force for a command row, which is what the palette shows instead of the default.
    fn row_parts(&self, item: &QjItem, bound: Option<&str>) -> (RowKind, String, String) {
        match item {
            QjItem::Page(h) => {
                let kind = if h.daily {
                    RowKind::Day
                } else if self.tags.contains(&h.page) {
                    RowKind::Tag
                } else {
                    RowKind::Page
                };
                // An alias hit says why it matched; the icon still says what it is.
                let detail = if h.alias.is_some() {
                    "Alias".to_string()
                } else {
                    kind.label().to_string()
                };
                (kind, h.title.clone(), detail)
            }
            QjItem::Day { title, label, .. } => (RowKind::Day, title.clone(), label.to_string()),
            QjItem::Create(t) => (RowKind::Create, format!("Create \"{t}\""), String::new()),
            // The keys actually bound, not the catalog default: a rebinding in Settings
            // applies at once, so the row must not advertise the chord it replaced (§9.1).
            QjItem::Command(d) => (
                RowKind::Command,
                d.label.to_string(),
                pretty_keys(bound.unwrap_or(d.keys)),
            ),
        }
    }

    /// One result row: icon, label with the match in semibold (QJ-4), type label.
    fn row(
        &self,
        i: usize,
        item: &QjItem,
        needle: &str,
        bound: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = theme(cx).clone();
        let (kind, label, detail) = self.row_parts(item, bound);
        let selected = i == self.selected;
        let marks: Vec<(Range<usize>, HighlightStyle)> = if kind == RowKind::Create {
            Vec::new()
        } else {
            highlight_ranges(&label, needle)
                .into_iter()
                .map(|r| {
                    (
                        r,
                        HighlightStyle {
                            font_weight: Some(FontWeight::SEMIBOLD),
                            ..Default::default()
                        },
                    )
                })
                .collect()
        };
        let text: AnyElement = if marks.is_empty() {
            div().child(label).into_any_element()
        } else {
            div()
                .child(StyledText::new(label).with_highlights(marks))
                .into_any_element()
        };
        let accent = kind == RowKind::Create;
        // A command that cannot act (no block in edit) is shown greyed out. For command
        // rows the selector names the action and its state, so a test can tell which
        // command a row is — the palette draws text a test cannot read (QJ-5).
        let (selectable, selector) = match item {
            QjItem::Command(d) => {
                let ready = self.command_ready(d, cx);
                (
                    ready,
                    format!(
                        "qj-command-{i}-{}{}",
                        d.action,
                        if ready { "" } else { "-off" }
                    ),
                )
            }
            _ => (true, format!("qj-label-{i}")),
        };
        let label_color = if accent {
            th.link
        } else if selectable {
            th.ink
        } else {
            th.muted
        };
        div()
            .id(("qj", i))
            .debug_selector(move || format!("qj-row-{i}"))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(px(crate::theme::RADIUS_CHIP))
            .when(selectable, |d| d.cursor_pointer())
            .when(accent, |d| d.text_color(th.link))
            .when(selectable, |d| d.hover(|d| d.bg(th.hover)))
            .when(selected, |d| d.bg(th.line_soft))
            .child(row_icon(i, kind, &th))
            .child(
                div()
                    .debug_selector(move || selector)
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(label_color)
                    .child(text),
            )
            .child(
                div()
                    .debug_selector(move || format!("qj-detail-{i}"))
                    .flex_none()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(detail),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, w, cx| {
                    let beside = e.modifiers.shift || e.modifiers.platform;
                    this.choose(i, beside, w, cx)
                }),
            )
            .into_any_element()
    }
}

/// The one glyph per row type (QJ-2). Drawn rather than set in a font, except `#` and
/// `+`, so nothing depends on a symbol font being installed.
fn row_icon(i: usize, kind: RowKind, th: &Theme) -> AnyElement {
    let slot = div()
        .id(("qj-icon", i))
        .debug_selector(move || format!("qj-icon-{i}-{}", kind.key()))
        .w(px(14.))
        .flex_none()
        .flex()
        .justify_center()
        .items_center();
    match kind {
        RowKind::Page => slot
            .child(
                div()
                    .w(px(8.))
                    .h(px(10.))
                    .rounded(px(crate::theme::RADIUS_CHIP))
                    .border_1()
                    .border_color(th.line_strong),
            )
            .into_any_element(),
        RowKind::Day => slot
            .child(div().w(px(7.)).h(px(7.)).rounded_full().bg(th.ink_3))
            .into_any_element(),
        RowKind::Tag => slot
            .text_size(px(CONTROL_SIZE))
            .text_color(th.tag_ink)
            .font_weight(FontWeight::SEMIBOLD)
            .child("#")
            .into_any_element(),
        RowKind::Command => slot
            .text_size(px(CAPTION_SIZE))
            .text_color(th.muted)
            .child("⌘")
            .into_any_element(),
        RowKind::Create => slot
            .text_size(px(CONTROL_SIZE))
            .text_color(th.link)
            .font_weight(FontWeight::SEMIBOLD)
            .child("+")
            .into_any_element(),
    }
}

impl Render for JumpPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let query = self.input.read(cx).text().to_string();
        let needle = query.strip_prefix('>').unwrap_or(&query).to_string();
        // Page mode keeps the compact quick-jump width (QJ-1..5); search widens for the
        // preview (SR-4). Either way it must fit the window, which is 720 px at minimum.
        let wide = self.mode == PaletteMode::Search;
        let wanted: f32 = if wide { 860. } else { 520. };
        let viewport = window.viewport_size();
        let width = wanted.min(f32::from(viewport.width) - 24.).max(280.);
        let max_h = (f32::from(viewport.height) - 100.).clamp(220., 440.);
        let pane_h = (max_h - 150.).clamp(120., 290.);
        let preview_w = (width * 0.4).clamp(240., 340.);
        // The commands replace the page rows, so the page scope chips would be a lie.
        let show_scopes = matches!(self.mode, PaletteMode::Pages | PaletteMode::Search)
            && !(self.mode == PaletteMode::Pages && query.starts_with('>'));
        let scopes = show_scopes.then(|| self.scope_bar(&th, cx));
        let preview = (self.mode == PaletteMode::Search)
            .then(|| {
                self.preview
                    .as_ref()
                    .map(|p| self.preview_pane(p, &th, preview_w, pane_h))
            })
            .flatten();
        // The keys in force, read once per frame rather than per row: a rebinding in
        // Settings replaces the default, and the palette must show what actually works.
        let bound: Vec<Option<String>> = self
            .results
            .iter()
            .map(|r| match r {
                QjItem::Command(d) => Some(active_keys(cx, d)),
                _ => None,
            })
            .collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut create: Option<AnyElement> = None;
        for (i, row) in self.rows.clone().into_iter().enumerate() {
            match row {
                Row::Item(item_ix) => {
                    let item = &self.results[item_ix];
                    let el = self.row(
                        i,
                        item,
                        &needle,
                        bound.get(item_ix).and_then(|k| k.as_deref()),
                        cx,
                    );
                    if matches!(item, QjItem::Create(_)) {
                        create = Some(el);
                    } else {
                        rows.push(el);
                    }
                }
                Row::Group { group } => {
                    rows.push(group_header(group, &self.groups[group], &th).into_any_element())
                }
                Row::Block { group, hit } => rows.push(self.block_row(i, group, hit, &th, cx)),
            }
        }
        // "Create page" is last, above a hairline, in the link colour (QJ-2).
        let create_row = create.map(|el| {
            div()
                .debug_selector(|| "jump-create".into())
                .mt_1()
                .pt_1()
                .border_t_1()
                .border_color(th.line)
                .child(el)
        });
        let hints = match self.mode {
            PaletteMode::Pages => "↑↓ move · ↩ open · ⌘↩ open in split · esc",
            PaletteMode::Search => "↑↓ move · ↩ open · ⌘↩ open beside · esc",
            PaletteMode::Capture => "↩ save to today · esc",
            PaletteMode::Text => "↩ confirm · esc",
        };
        div()
            .debug_selector(|| "jump-palette".into())
            .w(px(width))
            .max_h(px(max_h))
            .flex()
            .flex_col()
            .bg(th.well)
            .border_1()
            .border_color(th.line)
            .rounded(px(crate::theme::RADIUS_INPUT))
            .p_2()
            .gap_1()
            .child(
                div()
                    .px_1()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(self.title),
            )
            .child(
                div()
                    .debug_selector(|| "search-input".into())
                    .px_2()
                    .py_1()
                    .border_1()
                    .border_color(th.line)
                    .rounded(px(crate::theme::RADIUS_CONTROL))
                    .child(self.input.clone()),
            )
            .children(scopes)
            .child(
                div()
                    .id("jump-results-row")
                    .flex()
                    .gap_2()
                    .flex_1()
                    .min_h_0()
                    .max_h(px(pane_h))
                    .child(
                        div()
                            .id("jump-results")
                            .debug_selector(|| "jump-results".into())
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .max_h(px(pane_h))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .children(rows)
                            .children(create_row),
                    )
                    .children(preview),
            )
            .child(
                div()
                    .debug_selector(|| "jump-footer".into())
                    .flex_none()
                    .border_t_1()
                    .border_color(th.line)
                    .pt_1()
                    .px_1()
                    .text_size(px(CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(hints),
            )
    }
}

/// The commands a `>` query lists (QJ-5), in catalog order so an empty query reads as a
/// menu grouped by category. Matches the label, the category, and the keys in force —
/// either as `keymap.json` spells them or as the row shows them — so after a rebinding the
/// command is found by its new chord and not by the default it replaced.
fn commands(query: &str, keys: impl Fn(&ActionDoc) -> String) -> Vec<&'static ActionDoc> {
    let q = query.trim().to_lowercase();
    CATALOG
        .iter()
        .filter(|d| {
            if q.is_empty() {
                return true;
            }
            let bound = keys(d);
            d.label.to_lowercase().contains(&q)
                || d.category.label().to_lowercase().contains(&q)
                || bound.to_lowercase().contains(&q)
                || pretty_keys(&bound).to_lowercase().contains(&q)
        })
        .collect()
}

/// Byte ranges of the typed letters inside `label`, for the semibold match (QJ-4). A
/// contiguous, case-insensitive match wins, so `ana` bolds just the `ana` of `Dana`;
/// otherwise the letters of the fuzzy subsequence are marked one at a time, which is why
/// `dn` still shows what made `Dana` a result.
///
/// Matching compares characters, never offsets into a folded copy of the string: folding
/// `ẞ` or `İ` moves where the byte boundaries are (they are five bytes either way, but the
/// lowercase characters are not the same bytes), and a range that splits a character
/// panics when gpui slices the text for a highlight. Every range returned here therefore
/// starts and ends on a boundary of `label` (AGENTS.md, text conventions).
fn highlight_ranges(label: &str, needle: &str) -> Vec<Range<usize>> {
    let needle = needle.trim();
    if needle.is_empty() || label.is_empty() {
        return Vec::new();
    }
    // Each character of the label: its byte range, and its case-folded form (which can be
    // more than one character: `İ` folds to `i` plus a combining dot).
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut stream: Vec<char> = Vec::new();
    let mut owner: Vec<usize> = Vec::new();
    for (i, c) in label.char_indices() {
        ranges.push(i..i + c.len_utf8());
        for folded in c.to_lowercase() {
            stream.push(folded);
            owner.push(ranges.len() - 1);
        }
    }
    let want: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if want.is_empty() {
        return Vec::new();
    }
    // The folded text as one stream, so a contiguous match can be found in it and widened
    // back to whole characters: `i` in `ẞİ` marks that `İ` (3..5), not one byte of `ẞ`.
    let mut out = Vec::new();
    let mut start = 0;
    while start + want.len() <= stream.len() {
        if stream[start..start + want.len()] == want[..] {
            let last = start + want.len() - 1;
            out.push(ranges[owner[start]].start..ranges[owner[last]].end);
            start += want.len();
        } else {
            start += 1;
        }
    }
    if !out.is_empty() {
        return out;
    }
    // Otherwise the letters of the fuzzy subsequence, one character at a time. Adjacent
    // ones merge, so two folded characters inside one character stay a single range.
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut from = 0;
    for c in want {
        let Some(i) = (from..stream.len()).find(|&i| stream[i] == c) else {
            return Vec::new();
        };
        from = i + 1;
        let r = ranges[owner[i]].clone();
        match out.last_mut() {
            // Overlapping as well as adjacent: one source character can lowercase to two
            // (İ → i + combining dot), so both matched folded characters map back to the
            // same range. Emitting it twice makes GPUI build a highlight that ends inside
            // the next character and the text shaper panics on user data.
            Some(prev) if r.start <= prev.end => prev.end = prev.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// QJ-4: the typed letters are marked inside the result, and nothing else is.
    #[test]
    fn the_match_is_marked() {
        assert_eq!(highlight_ranges("Dana", "dan"), vec![0..3]);
        assert_eq!(highlight_ranges("Dana", "ANA"), vec![1..4]);
        // Every occurrence is marked, not just the first.
        assert_eq!(
            highlight_ranges("Atlas launch", "a"),
            vec![0..1, 3..4, 7..8]
        );
        // A fuzzy match marks the letters it matched.
        assert_eq!(highlight_ranges("Dana", "dn"), vec![0..1, 2..3]);
        assert!(highlight_ranges("Dana", "x").is_empty());
        assert!(highlight_ranges("Dana", "").is_empty());
        // Ranges are byte ranges into the label, so a non-ASCII title stays intact.
        assert_eq!(highlight_ranges("Café notes", "é"), vec![3..5]);
        assert_eq!(highlight_ranges("Café notes", "notes"), vec![6..11]);
        // İ lowercases to two chars, so a fuzzy match can map both of them back to the same
        // source range. Overlapping ranges merge instead of being marked twice: a duplicate
        // range makes the highlighter slice inside the next character.
        assert_eq!(highlight_ranges("İaéx", "İx"), vec![0..2, 5..6]);
        assert_eq!(highlight_ranges("İx", "i"), vec![0..2]);
    }

    /// Case folding can change *where* the byte boundaries are even when the folded text
    /// has the same length: `ẞİ` and its lowercase are both five bytes, yet offset 2 lies
    /// inside `ẞ`. A range that splits a character panics gpui's text shaper (§6, rule 11),
    /// so every range must start and end on a boundary of the original label.
    #[test]
    fn match_ranges_stay_on_char_boundaries() {
        for (label, needle) in [
            ("ẞİ", "i"),
            ("İstanbul", "i"),
            ("İstanbul", "istan"),
            ("Straße", "s"),
            ("Straße", "ß"),
            ("ΣΟΦΊΑ", "σ"),
            ("ǅungla", "d"),
            ("Ça va", "ça"),
        ] {
            let ranges = highlight_ranges(label, needle);
            for r in &ranges {
                assert!(
                    label.get(r.clone()).is_some(),
                    "{label:?} / {needle:?}: {r:?} splits a character"
                );
            }
        }
        // Matching `i` must mark the `İ` it matched, not one byte of its UTF-8.
        assert_eq!(highlight_ranges("ẞİ", "i"), vec![3..5]);
    }

    /// QJ-5: `>` lists the catalog, filtered by label, category or key. `keys` is the
    /// chord in force; with the defaults installed that is the catalog's own.
    #[test]
    fn commands_come_from_the_catalog() {
        let defaults = |d: &ActionDoc| crate::actions::platform_keys(d.keys);
        assert_eq!(commands("", defaults).len(), CATALOG.len());
        let today = commands("today", defaults);
        assert!(
            today.iter().any(|d| d.action == "GoToday"),
            "{:?}",
            today.iter().map(|d| d.action).collect::<Vec<_>>()
        );
        assert!(commands("navigation", defaults)
            .iter()
            .all(|d| d.category.label() == "Navigation"));
        assert!(commands("cmd-shift-g", defaults)
            .iter()
            .any(|d| d.action == "ShowGraph"));
        assert!(commands("⇧⌘G", defaults)
            .iter()
            .any(|d| d.action == "ShowGraph"));
        assert!(commands("nonsense-nothing", defaults).is_empty());
        // A command is found by the chord that is in force, not by the one it replaced.
        let rebound = |d: &ActionDoc| {
            if d.action == "GoToday" {
                "alt-cmd-t".to_string()
            } else {
                crate::actions::platform_keys(d.keys)
            }
        };
        assert!(commands("alt-cmd-t", rebound)
            .iter()
            .any(|d| d.action == "GoToday"));
        assert!(!commands("cmd-shift-t", rebound)
            .iter()
            .any(|d| d.action == "GoToday"));
    }
}
