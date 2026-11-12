//! The Tasks view (PRD §9.2.2, DESIGN.md §4.5): every to-do in the graph, grouped by the
//! page it is written on. It reads `Store::tasks` on each index update and writes only by
//! ticking a box, through the command a page uses (rule 13). The workspace shows it in the
//! main area and draws its top bar (the filter lives there).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use chrono::NaiveDate;
use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, EventEmitter, MouseButton, StyledText,
    Subscription, Window,
};
use utopia_core::{BlockId, BlockKind, PageId};
use utopia_store::{Task, TaskGroup};

use crate::app_state::{AppState, IndexUpdated};
use crate::icons::{icon, Icon};
use crate::rich::{self, ChipState, LinkTarget};
use crate::theme::{
    count_chip, theme, Theme, CARET_SIZE, CONTENT_MAX_WIDTH, COUNT, LABEL_MONO, MONO, PAGE_TITLE,
    RADIUS_CHIP, RADIUS_CONTROL, RADIUS_PANEL, REF_LABEL, REF_TYPE_PADDING_X, SECTION, SMALL,
    TODO_CHECK_SIZE, TODO_GAP, TODO_SIZE,
};

/// Column padding (DESIGN.md §4.5): tighter than a page, so the list reads as one sheet.
const PAD_TOP: f32 = 20.;
const PAD_X: f32 = 28.;
const PAD_BOTTOM: f32 = 32.;
/// Space above and below a group's divider; the first group sits 12 px under the title.
const GROUP_GAP: f32 = 10.;
const FIRST_GROUP_GAP: f32 = 12.;
/// The fold chevron's button, hung in the column's left padding.
const FOLD_BUTTON: f32 = 24.;
/// The gap between the parts of a group header.
const HEADER_GAP: f32 = 8.;
/// A row's hover fill reaches this far past the text on each side.
const HOVER_BLEED: f32 = 8.;
/// A nested to-do: the thread line sits under the parent checkbox's centre.
const NEST_MARGIN: f32 = TODO_SIZE / 2. - 1.;
const NEST_PADDING: f32 = 18.;

/// Which tasks show (TK-3). Local UI state, never saved (rule 9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaskFilter {
    #[default]
    Open,
    Done,
    All,
}

impl TaskFilter {
    pub const ALL: [TaskFilter; 3] = [TaskFilter::Open, TaskFilter::Done, TaskFilter::All];

    pub fn key(self) -> &'static str {
        match self {
            TaskFilter::Open => "open",
            TaskFilter::Done => "done",
            TaskFilter::All => "all",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TaskFilter::Open => "Open",
            TaskFilter::Done => "Done",
            TaskFilter::All => "All",
        }
    }
}

/// What the workspace does for a click in the view.
#[derive(Debug, Clone)]
pub enum TasksEvent {
    /// A task's text (TK-6): its page, with the block revealed.
    OpenTask {
        page: PageId,
        block: BlockId,
        shift: bool,
    },
    /// A group title (TK-2).
    OpenPage { page: PageId, shift: bool },
    /// A link or chip inside a task's text.
    Follow { target: LinkTarget, shift: bool },
}

pub struct TasksView {
    app: Entity<AppState>,
    groups: Vec<TaskGroup>,
    today: NaiveDate,
    filter: TaskFilter,
    /// Tasks ticked under Open since the view was shown (TK-4): they stay, struck through,
    /// until the filter changes or the view is left.
    kept: HashSet<BlockId>,
    folded: HashSet<PageId>,
    /// `((uid))` chip states read with the tasks, so `render` never touches the store
    /// (rule 10).
    chips: HashMap<String, ChipState>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<TasksEvent> for TasksView {}

impl TasksView {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.subscribe(&app, |this, _, _: &IndexUpdated, cx| {
            this.reload(cx);
        })];
        let today = app.read(cx).store.today();
        let mut me = Self {
            app,
            groups: Vec::new(),
            today,
            filter: TaskFilter::default(),
            kept: HashSet::new(),
            folded: HashSet::new(),
            chips: HashMap::new(),
            _subs: subs,
        };
        me.reload(cx);
        me
    }

    /// Read the tasks again (TK-7): on each index update, edits from sync included.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let app = self.app.read(cx);
        let store = &app.store;
        self.today = store.today();
        match store.tasks() {
            Ok(groups) => self.groups = groups,
            Err(e) => eprintln!("utopia: tasks: {e}"),
        }
        let mut chips = HashMap::new();
        for t in self.groups.iter().flat_map(|g| &g.tasks) {
            for r in utopia_core::inline::extract_refs(&t.text) {
                if r.kind == utopia_core::inline::RefKind::Block && !chips.contains_key(&r.target) {
                    let state = crate::reference::chip_state(store, &r.target, &t.block);
                    chips.insert(r.target.clone(), state);
                }
            }
        }
        self.chips = chips;
        cx.notify();
    }

    pub fn filter(&self) -> TaskFilter {
        self.filter
    }

    pub fn set_filter(&mut self, filter: TaskFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.kept.clear();
        cx.notify();
    }

    /// Open, done and all to-dos (TK-3); each block counts once (TK-1).
    pub fn counts(&self) -> (usize, usize, usize) {
        let all = self.groups.iter().map(|g| g.tasks.len()).sum::<usize>();
        let open = self.groups.iter().map(TaskGroup::open).sum::<usize>();
        (open, all - open, all)
    }

    pub fn count(&self, filter: TaskFilter) -> usize {
        let (open, done, all) = self.counts();
        match filter {
            TaskFilter::Open => open,
            TaskFilter::Done => done,
            TaskFilter::All => all,
        }
    }

    /// The groups as loaded, for tests.
    pub fn groups(&self) -> &[TaskGroup] {
        &self.groups
    }

    fn shows(&self, t: &Task) -> bool {
        match self.filter {
            TaskFilter::Open => !t.checked || self.kept.contains(&t.block),
            TaskFilter::Done => t.checked,
            TaskFilter::All => true,
        }
    }

    /// Tick or untick (TK-4): the page's own command, so undo and sync behave the same.
    pub fn toggle(&mut self, page: &PageId, block: &BlockId, cx: &mut Context<Self>) {
        let Some(task) = self
            .groups
            .iter_mut()
            .flat_map(|g| g.tasks.iter_mut())
            .find(|t| &t.block == block)
        else {
            return;
        };
        let checked = !task.checked;
        // Shown at once; the index update that follows confirms it.
        task.checked = checked;
        if self.filter == TaskFilter::Open && checked {
            self.kept.insert(block.clone());
        }
        let (page, block) = (page.clone(), block.clone());
        self.app.update(cx, |app, cx| {
            let res = app
                .store
                .set_block_kind(&page, &block, BlockKind::Todo, checked);
            app.report(res);
            app.changed(vec![page.clone()], true, cx);
        });
        cx.notify();
    }

    fn toggle_fold(&mut self, page: &PageId, cx: &mut Context<Self>) {
        if !self.folded.remove(page) {
            self.folded.insert(page.clone());
        }
        cx.notify();
    }

    fn render_group(
        &self,
        gi: usize,
        g: &TaskGroup,
        shown: Vec<&Task>,
        first: bool,
        th: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folded = self.folded.contains(&g.page);
        let count = match self.filter {
            TaskFilter::Done => shown.len(),
            _ => shown
                .iter()
                .filter(|t| !t.checked || self.kept.contains(&t.block))
                .count(),
        };
        let page = g.page.clone();
        let fold_page = g.page.clone();
        let title = div()
            .id(("tasks-title", gi))
            .debug_selector(move || format!("tasks-title-{gi}"))
            // The full column width: a truncating child of a flex column with no width of
            // its own collapses to the ellipsis alone.
            .w_full()
            .truncate()
            .text_size(px(SECTION.size))
            .line_height(px(SECTION.line))
            .font_weight(SECTION.weight)
            .text_color(th.ink)
            .cursor_pointer()
            .hover(|d| d.underline().text_decoration_color(th.line_strong))
            .child(g.title.clone())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, e: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(TasksEvent::OpenPage {
                        page: page.clone(),
                        shift: e.modifiers.shift,
                    });
                }),
            );
        let header = div()
            .flex()
            .items_end()
            .gap(px(HEADER_GAP))
            .child(
                div()
                    .id(("tasks-fold", gi))
                    .debug_selector(move || format!("tasks-fold-{gi}"))
                    .flex_none()
                    .size(px(FOLD_BUTTON))
                    // Hung in the gutter, gap included, so the title starts on the column
                    // edge with the checkboxes (DESIGN.md §4.5).
                    .ml(px(-(FOLD_BUTTON + HEADER_GAP)))
                    .mb(px((SECTION.line - FOLD_BUTTON) / 2.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS_CONTROL))
                    .cursor_pointer()
                    .hover(|d| d.bg(th.hover))
                    .child(icon(
                        if folded {
                            Icon::ChevronRight
                        } else {
                            Icon::ChevronDown
                        },
                        CARET_SIZE,
                        th.bullet,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_fold(&fold_page, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when_some(g.daily, |d, day| {
                        d.child(
                            div()
                                .font_family(LABEL_MONO)
                                .text_size(px(COUNT.size))
                                .line_height(px(COUNT.line))
                                .text_color(th.muted)
                                .child(day.format("%A").to_string().to_uppercase()),
                        )
                    })
                    .child(title),
            )
            .child(
                div()
                    .flex_none()
                    .mb(px((SECTION.line - 20.) / 2.))
                    .child(count_chip(count, th)),
            );

        let mut body = div().flex().flex_col().mt_1();
        if !folded {
            // Tasks under no heading first, then one section per heading, in the order the
            // headings appear (DESIGN.md §4.5); outline order inside each.
            let mut sections: Vec<(Option<&str>, Vec<&Task>)> = vec![(None, Vec::new())];
            for t in &shown {
                let key = t.heading.as_deref();
                match sections.iter_mut().find(|(h, _)| *h == key) {
                    Some((_, ts)) => ts.push(t),
                    None => sections.push((key, vec![t])),
                }
            }
            let by_id: HashMap<&BlockId, &Task> = shown.iter().map(|t| (&t.block, *t)).collect();
            for (si, (heading, tasks)) in sections.into_iter().enumerate() {
                if tasks.is_empty() {
                    continue;
                }
                if let Some(h) = heading {
                    body = body.child(
                        div()
                            .debug_selector(move || format!("tasks-heading-{gi}-{si}"))
                            .mt(px(6.))
                            .text_size(px(SMALL.size))
                            .line_height(px(SMALL.line))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.ink_3)
                            .child(h.to_string()),
                    );
                }
                for t in tasks {
                    let mut depth = 0;
                    let mut up = t.parent_task.as_ref();
                    while let Some(p) = up.and_then(|p| by_id.get(p)) {
                        depth += 1;
                        up = p.parent_task.as_ref();
                    }
                    let mut row = self.render_task(t, th, cx);
                    for _ in 0..depth {
                        row = div()
                            .ml(px(NEST_MARGIN))
                            .pl(px(NEST_PADDING))
                            .border_l_1()
                            .border_color(th.line)
                            .child(row)
                            .into_any_element();
                    }
                    body = body.child(row);
                }
            }
        }

        div()
            .debug_selector(move || format!("tasks-group-{gi}"))
            .mt(px(if first { FIRST_GROUP_GAP } else { GROUP_GAP }))
            .when(!first, |d| {
                d.pt(px(GROUP_GAP)).border_t_1().border_color(th.line_soft)
            })
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_task(&self, t: &Task, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let lh = crate::appearance::density(cx).line_height();
        let id = t.block.0.clone();
        let chips = &self.chips;
        let row_block = t.block.clone();
        let rendered = rich::render(&t.text, th, &mut |uid| {
            if uid == row_block.as_str() {
                ChipState::Cyclic
            } else {
                chips.get(uid).cloned().unwrap_or(ChipState::Missing)
            }
        });
        let styled =
            StyledText::new(rendered.text.clone()).with_highlights(rendered.highlights.clone());
        let layout = styled.layout().clone();
        let links: Vec<Range<usize>> = rendered.links.iter().map(|(r, _)| r.clone()).collect();
        let text_chips = rendered.chips.clone();

        let (page, block) = (t.page.clone(), t.block.clone());
        let (cpage, cblock) = (t.page.clone(), t.block.clone());
        let checked = t.checked;
        let check_id = id.clone();
        let checkbox = div()
            .id(gpui::ElementId::Name(format!("task-check-{id}").into()))
            .debug_selector(move || format!("task-check-{check_id}"))
            .flex_none()
            .size(px(TODO_SIZE))
            .mt(px((lh - TODO_SIZE) / 2.))
            .rounded(px(RADIUS_CHIP))
            .border_1()
            .border_color(if checked { th.link } else { th.line_strong })
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .font_family(MONO)
            .text_size(px(TODO_CHECK_SIZE))
            .when(checked, |d| d.bg(th.link).text_color(th.paper).child("✓"))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(&cpage, &cblock, cx)));

        let text_id = id.clone();
        let text = div()
            .debug_selector(move || format!("task-text-{text_id}"))
            .min_w_0()
            .cursor_pointer()
            .when(checked, |d| d.text_color(th.muted).line_through())
            .child(rich::LinkText::new(styled, links).with_chips(text_chips, th.tag_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, e: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if let Ok(i) = layout.index_for_position(e.position) {
                        if let Some(target) = rendered.link_at(i).cloned() {
                            cx.emit(TasksEvent::Follow {
                                target,
                                shift: e.modifiers.shift,
                            });
                            return;
                        }
                    }
                    cx.emit(TasksEvent::OpenTask {
                        page: page.clone(),
                        block: block.clone(),
                        shift: e.modifiers.shift,
                    });
                }),
            );

        let due = if checked {
            None
        } else {
            match t.due {
                Some(d) if d < self.today => Some(("Overdue", th.danger, th.danger_bg)),
                Some(d) if d == self.today => Some(("Today", th.link, th.link_tint)),
                _ => None,
            }
        };

        let row_id = id.clone();
        div()
            .debug_selector(move || format!("task-{row_id}"))
            .mx(px(-HOVER_BLEED))
            .px(px(HOVER_BLEED))
            .rounded(px(RADIUS_CONTROL))
            .hover(|d| d.bg(th.well))
            .flex()
            .items_start()
            .gap(px(TODO_GAP))
            .child(checkbox)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(text)
                    .when_some(t.context.clone(), |d, ctx| {
                        d.child(
                            div()
                                .mt(px(-4.))
                                .pb(px(2.))
                                .truncate()
                                .text_size(px(SMALL.size))
                                .line_height(px(SMALL.line))
                                .text_color(th.muted)
                                .child(ctx),
                        )
                    }),
            )
            .when_some(due, |d, (label, fg, bg)| {
                d.child(
                    div().flex_none().h(px(lh)).flex().items_center().child(
                        div()
                            .px(px(REF_TYPE_PADDING_X))
                            .py(px(3.))
                            .rounded_full()
                            .bg(bg)
                            .font_family(LABEL_MONO)
                            .text_size(px(REF_LABEL.size))
                            .line_height(px(REF_LABEL.size))
                            .font_weight(REF_LABEL.weight)
                            .text_color(fg)
                            .child(label.to_uppercase()),
                    ),
                )
            })
            .into_any_element()
    }
}

impl Render for TasksView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let mut col = div()
            .debug_selector(|| "tasks-column".into())
            .w_full()
            .max_w(px(CONTENT_MAX_WIDTH))
            .mx_auto()
            .pt(px(PAD_TOP))
            .px(px(PAD_X))
            .pb(px(PAD_BOTTOM))
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "tasks-page-title".into())
                    .text_size(px(PAGE_TITLE.size))
                    .line_height(px(PAGE_TITLE.line))
                    .font_weight(PAGE_TITLE.weight)
                    .text_color(th.ink)
                    .child("Tasks"),
            );

        let groups: Vec<(usize, &TaskGroup, Vec<&Task>)> = self
            .groups
            .iter()
            .enumerate()
            .map(|(i, g)| (i, g, g.tasks.iter().filter(|t| self.shows(t)).collect()))
            .filter(|(_, _, ts): &(usize, &TaskGroup, Vec<&Task>)| !ts.is_empty())
            .collect();
        if groups.is_empty() {
            let what = match self.filter {
                TaskFilter::Done => "No done tasks.",
                TaskFilter::Open => "No open tasks.",
                TaskFilter::All => "No tasks yet.",
            };
            col = col.child(
                div()
                    .debug_selector(|| "tasks-empty".into())
                    .mt(px(48.))
                    .p(px(24.))
                    .rounded(px(RADIUS_PANEL))
                    .border_1()
                    .border_dashed()
                    .border_color(th.line_strong)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .text_color(th.muted)
                    .child(div().text_color(th.ink_2).child(what))
                    .child("Type /To-do or press ⌥↵ in any block to make one."),
            );
        } else {
            let mut elements = Vec::with_capacity(groups.len());
            for (n, (gi, g, shown)) in groups.into_iter().enumerate() {
                elements.push(self.render_group(gi, g, shown, n == 0, &th, cx));
            }
            col = col.children(elements);
        }

        div()
            .id("tasks-scroll")
            .debug_selector(|| "tasks-view".into())
            .size_full()
            .overflow_y_scroll()
            .bg(th.paper)
            .child(col)
    }
}
