//! Graph view (P2): pages as nodes, links as edges, laid out force-directed off the main
//! thread. Drag to pan, scroll to zoom, click a node to open its page (shift-click opens
//! it in the right sidebar, ⌘-click in a split).
//!
//! The redesign's graph screen (GR-1..GR-5) is a view change only: the store still owns
//! identity, references and the index. Nodes grow from 6 px to 18 px with their distinct
//! link count, the page the graph was opened from keeps a 2 px link ring (GR-1); hovering
//! fades everything but the page and its neighbours to 25 % and floats a card with the
//! first block (GR-2); the count and colour key sit at the bottom left and the zoom controls
//! at the bottom right (GR-3); edges grow from 1 px to 3 px with the stored reference count
//! and arrowheads point at the target (GR-4). "Around this page" moves the local graph into
//! the right sidebar so the page stays editable (GR-5); "All pages" is the full-screen mode.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, size, transparent_black, Bounds,
    Context, EventEmitter, FocusHandle, Focusable, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, ScrollWheelEvent, SharedString, Task,
    TextRun, Window,
};
use utopia_core::PageId;
use utopia_store::{GraphOptions, PageGraph};

use crate::app_state::AppState;
use crate::theme::{quiet_control, theme, Theme, CAPTION_SIZE, CONTROL_SIZE};

const LAYOUT_ITERATIONS: usize = 160;
const MARGIN: f32 = 48.;
/// GR-1: a node's diameter runs from 6 px to 18 px across the graph's degree range.
const MIN_DIAMETER: f32 = 6.;
const MAX_DIAMETER: f32 = 18.;
/// GR-2: everything that is neither the hovered page nor a neighbour drops to this.
const FADE: f32 = 0.25;
const ZOOM_MIN: f32 = 0.25;
const ZOOM_MAX: f32 = 8.;

pub enum GraphEvent {
    /// A node was clicked. `sidebar` is shift-click, `split` is ⌘-click (GR-2).
    Open {
        page: PageId,
        sidebar: bool,
        split: bool,
    },
    /// "Around this page": the local graph becomes a right-sidebar panel (GR-5).
    OpenPanel,
    /// "All pages" in the panel: back to the full-screen graph (GR-5).
    Expand,
    Close,
}

/// What the hover card shows, read once when the hover changes rather than at paint time
/// (rule 3, rule 10).
struct HoverInfo {
    title: String,
    links: usize,
    first: String,
}

pub struct GraphView {
    app: gpui::Entity<AppState>,
    /// The page that was open when the graph was shown (the local graph's center).
    origin: Option<PageId>,
    local: bool,
    /// Drawn in the right sidebar instead of full screen (GR-5).
    panel: bool,
    include_daily: bool,
    graph: PageGraph,
    /// Largest degree in `graph`; sizes nodes relative to it (GR-1).
    max_degree: usize,
    pos: Vec<(f32, f32)>,
    laid_out: bool,
    pan: Point<Pixels>,
    zoom: f32,
    press: Option<(Point<Pixels>, Point<Pixels>)>,
    hover: Option<usize>,
    /// The hovered node and its neighbours; everyone else fades (GR-2).
    lit: Vec<bool>,
    hover_info: Option<HoverInfo>,
    hover_at: Point<Pixels>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    _layout: Option<Task<()>>,
}

impl EventEmitter<GraphEvent> for GraphView {}

impl Focusable for GraphView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl GraphView {
    pub fn new(
        app: gpui::Entity<AppState>,
        origin: Option<PageId>,
        panel: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut v = Self {
            app,
            origin,
            // The panel is always "around this page"; the full-screen view opens on all.
            local: panel,
            panel,
            // Most links in a daily-notes graph come from journal pages; hiding them by
            // default leaves the picture nearly empty.
            include_daily: true,
            graph: PageGraph::default(),
            max_degree: 0,
            pos: Vec::new(),
            laid_out: false,
            pan: point(px(0.), px(0.)),
            zoom: 1.0,
            press: None,
            hover: None,
            lit: Vec::new(),
            hover_info: None,
            hover_at: point(px(0.), px(0.)),
            bounds: Rc::new(Cell::new(Bounds::default())),
            focus: cx.focus_handle(),
            _layout: None,
        };
        v.reload(true, cx);
        v
    }

    pub fn node_count(&self) -> usize {
        self.graph.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edges.len()
    }

    /// Directed `(source, target, count)` edges from the stored references (GR-4).
    pub fn directed_edges(&self) -> &[(usize, usize, usize)] {
        &self.graph.directed
    }

    pub fn is_laid_out(&self) -> bool {
        self.laid_out
    }

    pub fn is_panel(&self) -> bool {
        self.panel
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn hovered(&self) -> Option<usize> {
        self.hover
    }

    pub fn origin(&self) -> Option<PageId> {
        self.origin.clone()
    }

    pub fn set_local(&mut self, local: bool, cx: &mut Context<Self>) {
        if self.local == local {
            return;
        }
        self.local = local;
        self.reload(true, cx);
    }

    /// Re-read the graph after the index changed while the view stayed open. The viewport
    /// (pan and zoom) is the user's: an ordinary commit must not throw it away.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload(false, cx);
    }

    /// Keep the panel around the page that is open beside it (GR-5).
    pub fn set_origin(&mut self, origin: Option<PageId>, cx: &mut Context<Self>) {
        if self.origin == origin {
            return;
        }
        self.origin = origin;
        if self.local {
            self.reload(true, cx);
        } else {
            cx.notify();
        }
    }

    /// Screen position of a node, for tests and hit-testing.
    pub fn node_screen_pos(&self, i: usize) -> Option<Point<Pixels>> {
        let b = self.bounds.get();
        self.pos.get(i).map(|p| self.to_screen(*p, b))
    }

    pub fn node_page(&self, i: usize) -> Option<PageId> {
        self.graph.nodes.get(i).map(|n| n.page.clone())
    }

    /// Radius in px, from 3 (6 px across) to 9 (18 px across), for tests (GR-1).
    pub fn node_radius(&self, i: usize) -> Option<f32> {
        self.graph
            .nodes
            .get(i)
            .map(|n| Self::radius(n.degree, self.max_degree))
    }

    /// Re-read the graph. `reset_view` is for navigation (a new origin, a different scope):
    /// a refresh of the same origin keeps the pan and zoom the user set.
    fn reload(&mut self, reset_view: bool, cx: &mut Context<Self>) {
        let opts = GraphOptions {
            center: self.local.then(|| self.origin.clone()).flatten(),
            depth: 2,
            include_daily: self.include_daily,
            limit: 400,
        };
        let graph = self.app.update(cx, |app, _| {
            let r = app.store.page_graph(&opts);
            app.report(r).unwrap_or_default()
        });
        let n = graph.nodes.len();
        let max_degree = graph.nodes.iter().map(|n| n.degree).max().unwrap_or(0);
        let edges: Vec<(usize, usize)> = graph.edges.iter().map(|e| (e.0, e.1)).collect();
        self.graph = graph;
        self.max_degree = max_degree;
        self.laid_out = false;
        self.hover = None;
        self.lit = Vec::new();
        self.hover_info = None;
        if reset_view {
            self.pan = point(px(0.), px(0.));
            self.zoom = 1.0;
        }
        let task = cx
            .background_executor()
            .spawn(async move { utopia_core::graph::layout(n, &edges, LAYOUT_ITERATIONS, 0x7e11) });
        self._layout = Some(cx.spawn(async move |this, cx| {
            let pos = task.await;
            let _ = this.update(cx, |this, cx| {
                if pos.len() == this.graph.nodes.len() {
                    this.pos = pos;
                    this.laid_out = true;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn scale(b: Bounds<Pixels>) -> f32 {
        (f32::from(b.size.width).min(f32::from(b.size.height)) / 2. - MARGIN).max(20.)
    }

    fn to_screen(&self, p: (f32, f32), b: Bounds<Pixels>) -> Point<Pixels> {
        let s = Self::scale(b) * self.zoom;
        let c = b.center();
        point(
            c.x + px(p.0 * s) + self.pan.x,
            c.y + px(p.1 * s) + self.pan.y,
        )
    }

    /// GR-1: the diameter grows linearly from 6 px (least connected) to 18 px (most
    /// connected). A graph where every page has one link stays at the minimum.
    fn diameter(degree: usize, max_degree: usize) -> f32 {
        if max_degree <= 1 {
            return MIN_DIAMETER;
        }
        let t = degree.saturating_sub(1) as f32 / (max_degree - 1) as f32;
        MIN_DIAMETER + (MAX_DIAMETER - MIN_DIAMETER) * t.clamp(0., 1.)
    }

    fn radius(degree: usize, max_degree: usize) -> f32 {
        Self::diameter(degree, max_degree) / 2.
    }

    /// GR-4: 1 px for one reference, 2 px for two, 3 px for three or more.
    fn edge_width(references: usize) -> f32 {
        1. + (references.saturating_sub(1) as f32).min(2.)
    }

    fn hit(&self, at: Point<Pixels>) -> Option<usize> {
        if !self.laid_out {
            return None;
        }
        let b = self.bounds.get();
        let mut best: Option<(usize, f32)> = None;
        for (i, n) in self.graph.nodes.iter().enumerate() {
            let p = self.to_screen(self.pos[i], b);
            let d = f32::from((p.x - at.x).abs()).hypot(f32::from((p.y - at.y).abs()));
            let r = Self::radius(n.degree, self.max_degree);
            if d <= r + 4. && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Change the hovered node and read its card once. Non-neighbours are marked for the
    /// fade (GR-2) and the first block comes from the stored projection (rule 3).
    fn set_hover(&mut self, hover: Option<usize>, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.hover = hover;
        self.hover_at = at;
        self.lit = lit_nodes(self.graph.nodes.len(), &self.graph.edges, hover);
        self.hover_info = None;
        if let Some(i) = hover {
            if let Some(n) = self.graph.nodes.get(i) {
                let first = self
                    .app
                    .read(cx)
                    .store
                    .page_preview(&n.page, 1)
                    .ok()
                    .and_then(|v| v.into_iter().next())
                    .unwrap_or_default();
                self.hover_info = Some(HoverInfo {
                    title: n.title.clone(),
                    links: n.degree,
                    first,
                });
            }
        }
        cx.notify();
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.press = Some((e.position, e.position));
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((start, last)) = self.press {
            if e.pressed_button == Some(MouseButton::Left) {
                self.pan += e.position - last;
                self.press = Some((start, e.position));
                cx.notify();
                return;
            }
            self.press = None;
        }
        let h = self.hit(e.position);
        if h != self.hover {
            self.set_hover(h, e.position, cx);
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((start, _)) = self.press.take() else {
            return;
        };
        let moved =
            f32::from((e.position.x - start.x).abs()) + f32::from((e.position.y - start.y).abs());
        if moved < 4. {
            if let Some(i) = self.hit(e.position) {
                cx.emit(GraphEvent::Open {
                    page: self.graph.nodes[i].page.clone(),
                    sidebar: e.modifiers.shift,
                    split: e.modifiers.platform,
                });
            }
        }
        cx.notify();
    }

    fn scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let dy = f32::from(e.delta.pixel_delta(px(16.)).y);
        let old = self.zoom;
        self.zoom = (self.zoom * (1. + dy * 0.004)).clamp(ZOOM_MIN, ZOOM_MAX);
        // Zoom about the cursor: keep the point under it fixed.
        let c = self.bounds.get().center();
        let k = self.zoom / old;
        let rel = e.position - c - self.pan;
        self.pan += point(rel.x * (1. - k), rel.y * (1. - k));
        cx.notify();
    }

    /// GR-3: the "−" and "+" controls. Zoom about the middle of the canvas.
    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let old = self.zoom;
        self.zoom = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let k = self.zoom / old;
        self.pan = point(self.pan.x * k, self.pan.y * k);
        cx.notify();
    }

    /// GR-3: "Fit" returns to the layout's own frame, which always spans the canvas.
    fn fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.0;
        self.pan = point(px(0.), px(0.));
        cx.notify();
    }

    fn chip(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        on: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let th = theme(cx).clone();
        quiet_control(id, label, &th)
            .debug_selector(move || id.into())
            .when(on, |d| d.bg(th.line_soft).text_color(th.link))
            .when(!on, |d| d.text_color(th.muted))
    }

    fn legend(&self, th: &Theme) -> gpui::Stateful<gpui::Div> {
        let key = |color: Hsla, label: &'static str| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .w(px(7.))
                        .h(px(7.))
                        .rounded_full()
                        .bg(color)
                        .flex_none(),
                )
                .child(label)
        };
        let ring = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .w(px(9.))
                    .h(px(9.))
                    .rounded_full()
                    .border_2()
                    .border_color(th.link)
                    .flex_none(),
            )
            .child("Current page");
        div()
            .id("graph-legend")
            .debug_selector(|| "graph-legend".into())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex()
            .flex_col()
            .gap_0p5()
            .text_size(px(CAPTION_SIZE))
            .text_color(th.muted)
            .child(key(th.bullet, "Pages"))
            .child(key(th.tag_ink, "Journal pages"))
            .child(ring)
            .child(
                div()
                    .debug_selector(|| "graph-legend-count".into())
                    .pt_0p5()
                    .text_color(th.muted)
                    .child(format!(
                        "{} pages · {} links",
                        self.graph.nodes.len(),
                        self.graph.edges.len()
                    )),
            )
    }

    fn zoom_controls(&self, th: &Theme, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("graph-zoom-controls")
            .occlude()
            // GR-3: these float over the canvas. Consume the press so a node underneath
            // cannot also be opened by the same click.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .flex()
            .items_center()
            .gap_1()
            .child(
                quiet_control("graph-zoom-out", "−", th)
                    .debug_selector(|| "graph-zoom-out".into())
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1. / 1.25, cx))),
            )
            .child(
                quiet_control("graph-zoom-in", "+", th)
                    .debug_selector(|| "graph-zoom-in".into())
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.25, cx))),
            )
            .child(
                quiet_control("graph-zoom-fit", "Fit", th)
                    .debug_selector(|| "graph-zoom-fit".into())
                    .on_click(cx.listener(|this, _, _, cx| this.fit(cx))),
            )
    }
}

/// GR-2: the hovered node and its direct neighbours stay at full strength; everyone else
/// fades to 25 %. Kept separate from the paint closure so the rule is testable.
fn lit_nodes(nodes: usize, edges: &[(usize, usize, usize)], hover: Option<usize>) -> Vec<bool> {
    let mut lit = vec![false; nodes];
    let Some(i) = hover else {
        return lit;
    };
    if let Some(slot) = lit.get_mut(i) {
        *slot = true;
    }
    for &(a, b, _) in edges {
        if a == i {
            if let Some(slot) = lit.get_mut(b) {
                *slot = true;
            }
        }
        if b == i {
            if let Some(slot) = lit.get_mut(a) {
                *slot = true;
            }
        }
    }
    lit
}

/// GR-4: a small filled triangle just before the target node, pointing along the stored
/// reference direction.
fn arrowhead(
    from: Point<Pixels>,
    to: Point<Pixels>,
    target_radius: f32,
) -> Option<gpui::Path<Pixels>> {
    let (fx, fy) = (f32::from(from.x), f32::from(from.y));
    let (tx, ty) = (f32::from(to.x), f32::from(to.y));
    let (dx, dy) = (tx - fx, ty - fy);
    let d = dx.hypot(dy);
    if d < target_radius + 12. {
        return None;
    }
    let (ux, uy) = (dx / d, dy / d);
    let tip = (
        tx - ux * (target_radius + 1.),
        ty - uy * (target_radius + 1.),
    );
    let base = (tip.0 - ux * 7., tip.1 - uy * 7.);
    let (px_, py_) = (-uy * 3.5, ux * 3.5);
    let mut path = PathBuilder::fill();
    path.move_to(point(px(tip.0), px(tip.1)));
    path.line_to(point(px(base.0 + px_), px(base.1 + py_)));
    path.line_to(point(px(base.0 - px_), px(base.1 - py_)));
    path.close();
    path.build().ok()
}

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let origin_title = self.origin.as_ref().and_then(|p| {
            self.app
                .read(cx)
                .store
                .page_row(p)
                .ok()
                .flatten()
                .map(|r| r.title)
        });

        // GR-3: the filters and Close stay in the top bar; the count moves into the legend.
        let header = if self.panel {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(th.line)
                .child(
                    div()
                        .text_size(px(CONTROL_SIZE))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child("Around this page"),
                )
                .child(div().flex_1())
                .child(
                    self.chip("graph-expand", "All pages", false, cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(GraphEvent::Expand))),
                )
                .child(
                    self.chip("graph-daily", "Journal", self.include_daily, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.include_daily = !this.include_daily;
                            this.reload(true, cx);
                        })),
                )
                .child(
                    self.chip("graph-close", "✕", false, cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(GraphEvent::Close))),
                )
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(th.line)
                .child(div().font_weight(gpui::FontWeight::BOLD).child("Graph"))
                .child(
                    self.chip("graph-all", "All pages", !self.local, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.set_local(false, cx))),
                )
                .when_some(origin_title, |d, title| {
                    // GR-5: "Around this page" is the sidebar panel, so the page beside it
                    // stays editable; "All pages" is the full-screen view.
                    d.child(
                        self.chip("graph-local", format!("Around {title}"), false, cx)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(GraphEvent::OpenPanel))),
                    )
                })
                .child(
                    self.chip("graph-daily", "Journal pages", self.include_daily, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.include_daily = !this.include_daily;
                            this.reload(true, cx);
                        })),
                )
                .child(div().flex_1())
                .child(
                    self.chip("graph-close", "Close", false, cx)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(GraphEvent::Close))),
                )
        };

        // Snapshot for the paint closure.
        let laid_out = self.laid_out;
        let screen: Vec<(f32, f32)> = self.pos.clone();
        let nodes = self.graph.nodes.clone();
        let edges = self.graph.edges.clone();
        let directed = self.graph.directed.clone();
        let max_degree = self.max_degree;
        let lit = self.lit.clone();
        let (pan, zoom, hover) = (self.pan, self.zoom, self.hover);
        let bounds_cell = self.bounds.clone();
        let origin_page = self.origin.clone();
        let font = window.text_style().font();
        let empty = nodes.is_empty();
        let body = div()
            .id("graph-canvas")
            .debug_selector(|| "graph-canvas".into())
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_hover(cx.listener(|this, hovering, _, cx| {
                if !hovering && this.hover.is_some() {
                    let at = this.hover_at;
                    this.set_hover(None, at, cx);
                }
            }))
            .when(self.hover.is_some(), |d| d.cursor_pointer())
            .child(
                canvas(
                    |_, _, _| {},
                    move |b, _, window, cx| {
                        bounds_cell.set(b);
                        if !laid_out {
                            return;
                        }
                        let s = GraphView::scale(b) * zoom;
                        let c = b.center();
                        let at = |p: (f32, f32)| {
                            point(c.x + px(p.0 * s) + pan.x, c.y + px(p.1 * s) + pan.y)
                        };
                        let lit = |i: usize| lit.get(i).copied().unwrap_or(false);
                        // GR-2: while hovering, only the page and its neighbours stay lit.
                        let faded = |i: usize| hover.is_some() && !lit(i);
                        // GR-4: one width bucket per stored reference count, split by
                        // whether the hover highlights it. Six cheap passes beat one path
                        // per edge.
                        for width in [1., 2., 3.] {
                            for accent in [false, true] {
                                let mut path = PathBuilder::stroke(px(width));
                                let mut any = false;
                                for &(a, b, references) in &edges {
                                    let touching = hover.is_some_and(|h| h == a || h == b);
                                    let lit_edge = hover.is_some() && touching;
                                    if lit_edge != accent
                                        || GraphView::edge_width(references) != width
                                    {
                                        continue;
                                    }
                                    path.move_to(at(screen[a]));
                                    path.line_to(at(screen[b]));
                                    any = true;
                                }
                                if !any {
                                    continue;
                                }
                                let color = if accent {
                                    th.link
                                } else if hover.is_some() {
                                    th.line.opacity(FADE)
                                } else {
                                    th.line
                                };
                                if let Ok(p) = path.build() {
                                    window.paint_path(p, color);
                                }
                            }
                        }
                        // GR-4: arrowheads point at the target of each stored reference.
                        for &(a, b, _) in &directed {
                            let touching = hover.is_some_and(|h| h == a || h == b);
                            let color = if hover.is_some() {
                                if touching {
                                    th.link
                                } else {
                                    th.line.opacity(FADE)
                                }
                            } else {
                                th.line
                            };
                            let r = GraphView::radius(nodes[b].degree, max_degree);
                            if let Some(p) = arrowhead(at(screen[a]), at(screen[b]), r) {
                                window.paint_path(p, color);
                            }
                        }
                        // Nodes, then labels for the hovered node, the origin and the
                        // best-connected pages (more as you zoom in).
                        let label_budget = ((30. * zoom * zoom) as usize).max(12);
                        for (i, n) in nodes.iter().enumerate() {
                            let r = GraphView::radius(n.degree, max_degree);
                            let p = at(screen[i]);
                            let is_origin = origin_page.as_ref() == Some(&n.page);
                            let is_hover = hover == Some(i);
                            let dim = faded(i);
                            let base = if n.daily { th.tag_ink } else { th.bullet };
                            let color = if is_hover {
                                th.link
                            } else if dim {
                                base.opacity(FADE)
                            } else {
                                base
                            };
                            let nb = Bounds::centered_at(p, size(px(r * 2.), px(r * 2.)));
                            window.paint_quad(gpui::fill(nb, color).corner_radii(px(r)));
                            if is_origin {
                                // GR-1: the page the graph was opened from keeps a 2 px
                                // link ring, whether or not it is the local centre.
                                let ring =
                                    Bounds::centered_at(p, size(px(r * 2. + 4.), px(r * 2. + 4.)));
                                window.paint_quad(
                                    gpui::fill(ring, transparent_black())
                                        .border_widths(px(2.))
                                        .border_color(if dim {
                                            th.link.opacity(FADE)
                                        } else {
                                            th.link
                                        })
                                        .corner_radii(px(r + 2.)),
                                );
                            }
                            let show = is_hover
                                || is_origin
                                || (hover.is_some() && lit(i))
                                || (hover.is_none() && i < label_budget);
                            if show {
                                let text: SharedString = n.title.clone().into();
                                let run = TextRun {
                                    len: text.len(),
                                    font: font.clone(),
                                    color: if is_hover {
                                        th.ink
                                    } else if dim {
                                        th.muted.opacity(FADE)
                                    } else {
                                        th.muted
                                    },
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                let line =
                                    window.text_system().shape_line(text, px(12.), &[run], None);
                                let origin = point(p.x - line.width / 2., p.y + px(r + 2.));
                                let _ = line.paint(origin, px(16.), window, cx);
                            }
                        }
                    },
                )
                .size_full(),
            )
            .children(self.hover_info.as_ref().map(|info| {
                // GR-2: the card floats over the canvas, so hovering never moves the graph.
                let at = self.hover_at;
                deferred(
                    anchored()
                        .position(at)
                        .offset(point(px(12.), px(14.)))
                        .child(
                            div()
                                .occlude()
                                .debug_selector(|| "graph-tooltip".into())
                                .w(px(240.))
                                .p_2()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .bg(th.well)
                                .border_1()
                                .border_color(th.line)
                                .rounded(px(crate::theme::RADIUS_CONTROL))
                                .text_size(px(CONTROL_SIZE))
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(th.ink)
                                        .child(info.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(CAPTION_SIZE))
                                        .text_color(th.muted)
                                        .child(format!("{} links", info.links)),
                                )
                                .when(!info.first.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .text_size(px(CAPTION_SIZE))
                                            .text_color(th.muted)
                                            .child(info.first.clone()),
                                    )
                                }),
                        ),
                )
                .with_priority(2)
            }))
            .when(empty, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_1_2()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_color(th.muted)
                        .child("No links yet. Type [[ to link pages and they appear here."),
                )
            });

        let selector = if self.panel {
            "graph-panel-view"
        } else {
            "graph-view"
        };
        let body = if self.panel {
            body
        } else {
            // GR-3: in the full-screen graph the legend and controls float over the canvas,
            // so they never take space from the picture.
            body.child(self.legend(&th).absolute().left_3().bottom_3())
                .child(self.zoom_controls(&th, cx).absolute().right_3().bottom_3())
        };
        div()
            .debug_selector(move || selector.into())
            .key_context("GraphView")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, e: &gpui::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(GraphEvent::Close);
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(th.paper)
            .child(header)
            .child(body)
            .when(self.panel, |d| {
                // The panel has less room, so the key and the zoom controls sit in a footer
                // under the graph instead of floating over it.
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .border_t_1()
                        .border_color(th.line)
                        .child(self.legend(&th).flex_row().flex_wrap().gap_2())
                        .child(div().flex_1())
                        .child(self.zoom_controls(&th, cx).flex_none()),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GR-1: the smallest page draws at 6 px across and the most connected at 18 px; the
    /// size never leaves that band, and a graph without a degree spread stays at the minimum.
    #[test]
    fn node_size_spans_six_to_eighteen_px() {
        assert_eq!(GraphView::diameter(1, 1), 6.);
        assert_eq!(GraphView::diameter(1, 5), 6.);
        assert_eq!(GraphView::diameter(5, 5), 18.);
        assert_eq!(GraphView::diameter(3, 5), 12.);
        for max in 1..20 {
            for degree in 0..=max {
                let d = GraphView::diameter(degree, max);
                assert!((6. ..=18.).contains(&d), "{degree}/{max} -> {d}");
            }
        }
    }

    /// GR-2: only the hovered page and its direct neighbours stay lit.
    #[test]
    fn hover_lights_the_page_and_its_neighbours() {
        let edges = [(0, 1, 1), (1, 2, 1)];
        assert_eq!(lit_nodes(4, &edges, None), [false; 4]);
        assert_eq!(lit_nodes(4, &edges, Some(0)), [true, true, false, false]);
        assert_eq!(lit_nodes(4, &edges, Some(1)), [true, true, true, false]);
        assert_eq!(lit_nodes(4, &edges, Some(3)), [false, false, false, true]);
    }

    /// GR-4: edge width comes from the stored reference count, capped at 3 px.
    #[test]
    fn edge_width_grows_with_stored_references() {
        assert_eq!(GraphView::edge_width(1), 1.);
        assert_eq!(GraphView::edge_width(2), 2.);
        assert_eq!(GraphView::edge_width(3), 3.);
        assert_eq!(GraphView::edge_width(42), 3.);
    }

    /// GR-4: an arrowhead stops short of the target node rather than crossing it.
    #[test]
    fn arrowhead_points_at_the_target() {
        let arrow = arrowhead(point(px(0.), px(0.)), point(px(100.), px(0.)), 6.).unwrap();
        let _ = arrow;
        assert!(arrowhead(point(px(0.), px(0.)), point(px(8.), px(0.)), 6.).is_none());
    }
}
