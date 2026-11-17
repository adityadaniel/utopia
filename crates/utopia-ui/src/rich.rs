//! Display rendering of a block body (§6.2): marks and reference chips become styled runs
//! inside one shaped text, never child widgets (§13.4). The focused block shows its
//! source instead; see [`source_highlights`].

use std::ops::Range;

use gpui::{
    point, px, App, Bounds, CursorStyle, Element, ElementId, FontStyle, FontWeight,
    GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, SharedString, StrikethroughStyle, StyledText, TextLayout,
    UnderlineStyle, Window,
};
use utopia_core::inline::{extract_refs, parse, Inline, Mark, RefKind};

use crate::theme::{
    Theme, LINK_RING_WIDTH, RADIUS_LINK_RING, RADIUS_TAG, TAG_HEIGHT, TAG_PADDING_X,
};

/// Drawn after the text of a link that leaves the app (a thin space, then ↗).
pub const EXTERNAL_ARROW: &str = "\u{2009}↗";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkTarget {
    /// Page title as typed; resolution happens in the store.
    Page(String),
    Block(String),
    Url(String),
}

/// How a `((uid))` chip should look (FR-4.3, FR-4.7, AC-4.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChipState {
    Live(String),
    Deleted,
    Missing,
    Cyclic,
}

#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub text: SharedString,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    pub links: Vec<(Range<usize>, LinkTarget)>,
    /// `#tag` runs, drawn as chips by [`LinkText`] (DESIGN.md §3.1).
    pub chips: Vec<Range<usize>>,
    /// Every run of `text` with the source bytes it came from, in order.
    spans: Vec<Span>,
    source_len: usize,
}

/// A run of the rendered text and where it came from, for [`Rendered::source_offset`].
#[derive(Debug, Clone)]
struct Span {
    shown: Range<usize>,
    source: Range<usize>,
    /// The shown bytes are the source bytes, one for one. Otherwise the run stands for
    /// markup it does not spell out (a chip's label, `#[[a tag]]`), and only its two ends
    /// have a place in the source. A run with no source behind it ([`EXTERNAL_ARROW`]) has
    /// an empty source range: a caret in it goes where the run was added.
    exact: bool,
}

impl Rendered {
    /// The source offset of a caret at byte `ix` of the rendered text. Markup is stripped
    /// from that text (`[[Ana]]` → `Ana`, `**x**` → `x`), so the two offsets agree only in
    /// a block that has none: a click has to be translated, or it can only be answered
    /// with the end of the block. The caret stays before the character it was before;
    /// past the last one it is the end of the source.
    pub fn source_offset(&self, ix: usize) -> usize {
        match self.spans.iter().find(|s| ix < s.shown.end) {
            Some(s) if s.exact => s.source.start + ix.saturating_sub(s.shown.start),
            Some(s) if ix <= s.shown.start => s.source.start,
            Some(s) => s.source.end,
            None => self.source_len,
        }
    }

    pub fn link_at(&self, ix: usize) -> Option<&LinkTarget> {
        self.links
            .iter()
            .find(|(r, _)| r.start <= ix && ix < r.end)
            .map(|(_, l)| l)
    }
}

#[derive(Clone, Copy, Default)]
struct Marks {
    bold: bool,
    italic: bool,
    strike: bool,
    highlight: bool,
}

fn chip_label(text: &str) -> String {
    let plain = utopia_core::inline::plain_text(text).replace('\n', " ");
    let mut label: String = plain.chars().take(40).collect();
    if plain.chars().count() > 40 {
        label.push('…');
    }
    label
}

struct Builder<'a> {
    theme: &'a Theme,
    chip: &'a mut dyn FnMut(&str) -> ChipState,
    /// Normalized titles of the page the text is being read on (DESIGN.md §3.7): its own
    /// name is bold ink, not a link.
    self_titles: &'a [String],
    /// The source being walked, and where it starts in the block's text (an attribute's
    /// value starts after its `Key:: `).
    src: &'a str,
    base: usize,
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, LinkTarget)>,
    chips: Vec<Range<usize>>,
    spans: Vec<Span>,
}

impl Builder<'_> {
    fn is_self(&self, title: &str) -> bool {
        let key = utopia_core::title::normalize_title(title);
        self.self_titles.contains(&key)
    }

    fn style(&self, m: Marks) -> HighlightStyle {
        HighlightStyle {
            font_weight: m.bold.then_some(FontWeight::BOLD),
            font_style: m.italic.then_some(FontStyle::Italic),
            strikethrough: m.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            background_color: m.highlight.then_some(self.theme.highlight_bg),
            ..Default::default()
        }
    }

    fn push(&mut self, s: &str, style: HighlightStyle) -> Range<usize> {
        let start = self.text.len();
        self.text.push_str(s);
        let range = start..self.text.len();
        if !s.is_empty() && style != HighlightStyle::default() {
            self.highlights.push((range.clone(), style));
        }
        range
    }

    /// Record that `shown` is the text node at `source` of [`Self::src`]. A text node is
    /// the source verbatim, except that a backslash which escaped a character is dropped
    /// (`\*` → `*`): the run is split there, so each piece is the source one for one.
    fn span_text(&mut self, shown: Range<usize>, source: Range<usize>) {
        let (text, src) = (&self.text[shown.clone()], &self.src[source.clone()]);
        let mut pieces: Vec<(Range<usize>, Range<usize>)> = Vec::new();
        let (mut d, mut s) = (0, 0);
        let mut open: Option<(usize, usize)> = None;
        for ch in text.chars() {
            while s < src.len() && !src[s..].starts_with(ch) {
                if let Some((d0, s0)) = open.take() {
                    pieces.push((d0..d, s0..s));
                }
                s += src[s..].chars().next().map_or(1, char::len_utf8);
            }
            open.get_or_insert((d, s));
            d += ch.len_utf8();
            s = (s + ch.len_utf8()).min(src.len());
        }
        if let Some((d0, s0)) = open {
            pieces.push((d0..d, s0..s));
        }
        let at = self.base + source.start;
        self.spans.extend(pieces.into_iter().map(|(d, s)| Span {
            shown: shown.start + d.start..shown.start + d.end,
            source: at + s.start..at + s.end,
            exact: true,
        }));
    }

    /// Record that `shown` stands for the atom at `source` of [`Self::src`]: exactly, when
    /// the atom spells the shown text out (a link's title, code), else by its ends alone.
    fn span_atom(&mut self, shown: Range<usize>, source: Range<usize>) {
        let label = &self.text[shown.clone()];
        let spelled = self.src[source.clone()]
            .find(label)
            .filter(|_| !label.is_empty())
            .map(|at| self.base + source.start + at);
        let span = match spelled {
            Some(start) => Span {
                source: start..start + shown.len(),
                shown,
                exact: true,
            },
            None => Span {
                shown,
                source: self.base + source.start..self.base + source.end,
                exact: false,
            },
        };
        self.spans.push(span);
    }

    /// `within` is the range of [`Self::src`] that `nodes` were parsed from.
    fn walk(&mut self, nodes: &[Inline], within: Range<usize>, m: Marks) {
        let mut at = within.start;
        for (k, n) in nodes.iter().enumerate() {
            // A text node has no range of its own: it is the gap up to the next node.
            let source = n.range().cloned().unwrap_or_else(|| {
                let next = nodes.get(k + 1).and_then(Inline::range);
                at..next.map_or(within.end, |r| r.start)
            });
            at = source.end;
            match n {
                Inline::Text(t) => {
                    let st = self.style(m);
                    let r = self.push(t, st);
                    self.span_text(r, source);
                }
                Inline::Emph { mark, children, .. } => {
                    let mut m2 = m;
                    match mark {
                        Mark::Bold => m2.bold = true,
                        Mark::Italic => m2.italic = true,
                        Mark::Strike => m2.strike = true,
                        Mark::Highlight => m2.highlight = true,
                    }
                    let dl = mark.delimiter().len();
                    self.walk(children, source.start + dl..source.end - dl, m2);
                }
                Inline::Code { text, .. } => {
                    let mut st = self.style(m);
                    st.background_color = Some(self.theme.code_bg);
                    let r = self.push(text, st);
                    self.span_atom(r, source);
                }
                Inline::PageRef { title, .. } => {
                    if self.is_self(title) {
                        // DESIGN.md §3.7: the current page's name inside one of its own
                        // references is bold ink text, not a link to where you already are.
                        let mut st = self.style(m);
                        st.color = Some(self.theme.ink);
                        st.font_weight = Some(FontWeight::BOLD);
                        let r = self.push(title, st);
                        self.span_atom(r, source);
                    } else {
                        let mut st = self.style(m);
                        st.color = Some(self.theme.link);
                        let r = self.push(title, st);
                        self.span_atom(r.clone(), source);
                        self.links.push((r, LinkTarget::Page(title.clone())));
                    }
                }
                Inline::Tag { title, .. } => {
                    // A tag is a chip (DESIGN.md §3.1), not just coloured text. A shaped
                    // text run cannot pad or round its background, so the fill is painted
                    // under the run by `LinkText`, which knows the glyph boxes.
                    let mut st = self.style(m);
                    st.color = Some(self.theme.tag_ink);
                    let r = self.push(&format!("#{title}"), st);
                    self.span_atom(r.clone(), source);
                    self.links
                        .push((r.clone(), LinkTarget::Page(title.clone())));
                    self.chips.push(r);
                }
                Inline::BlockRef { uid, .. } => {
                    let state = (self.chip)(uid);
                    let (label, bg, fg) = match state {
                        ChipState::Live(t) => {
                            (format!(" {} ›", chip_label(&t)), self.theme.line_soft, None)
                        }
                        ChipState::Deleted => (
                            " deleted block ".to_string(),
                            self.theme.line_soft,
                            Some(self.theme.muted),
                        ),
                        ChipState::Missing => (
                            format!(" Missing block (({uid})) "),
                            self.theme.danger_bg,
                            Some(self.theme.danger),
                        ),
                        ChipState::Cyclic => (
                            " ↻ cyclic reference ".to_string(),
                            self.theme.line_soft,
                            Some(self.theme.muted),
                        ),
                    };
                    let st = HighlightStyle {
                        background_color: Some(bg),
                        color: fg,
                        ..Default::default()
                    };
                    let r = self.push(&label, st);
                    self.span_atom(r.clone(), source);
                    self.links.push((r, LinkTarget::Block(uid.clone())));
                }
                Inline::Embed { uid, .. } => {
                    // The embedded block renders below the row; the chip marks where.
                    let label = match (self.chip)(uid) {
                        ChipState::Live(t) => format!(" ⧉ {} ", chip_label(&t)),
                        ChipState::Deleted => " ⧉ deleted block ".to_string(),
                        ChipState::Missing => format!(" Missing block (({uid})) "),
                        ChipState::Cyclic => " ↻ cyclic reference ".to_string(),
                    };
                    let st = HighlightStyle {
                        background_color: Some(self.theme.line_soft),
                        color: Some(self.theme.muted),
                        ..Default::default()
                    };
                    let r = self.push(&label, st);
                    self.span_atom(r.clone(), source);
                    self.links.push((r, LinkTarget::Block(uid.clone())));
                }
                Inline::Link { text, url, .. } if url.starts_with("blob:") => {
                    // An attachment: shown as a file token; the preview sits under the block.
                    let mut st = self.style(m);
                    st.color = Some(self.theme.muted);
                    let r = self.push(&format!("📎 {text}"), st);
                    self.span_atom(r.clone(), source);
                    self.links.push((r, LinkTarget::Url(url.clone())));
                }
                Inline::Link { text, url, .. } => {
                    let mut st = self.style(m);
                    st.color = Some(self.theme.link);
                    st.underline = Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(self.theme.link),
                        wavy: false,
                    });
                    let mut r = self.push(text, st);
                    self.span_atom(r.clone(), source.clone());
                    if utopia_core::inline::is_external_url(url) {
                        // A link that leaves the app ends in ↗, so it reads differently
                        // from a page link before it is clicked. The arrow is part of the
                        // link (it opens it too) but not underlined.
                        let arrow = HighlightStyle {
                            color: Some(self.theme.link),
                            ..self.style(m)
                        };
                        let a = self.push(EXTERNAL_ARROW, arrow);
                        r.end = a.end;
                        // No source behind the arrow: a caret in it goes to the link's end.
                        let at = self.base + source.end;
                        self.spans.push(Span {
                            shown: a,
                            source: at..at,
                            exact: false,
                        });
                    }
                    self.links.push((r, LinkTarget::Url(url.clone())));
                }
            }
        }
    }
}

/// Render a block body for display.
pub fn render(src: &str, theme: &Theme, chip: &mut dyn FnMut(&str) -> ChipState) -> Rendered {
    render_self(src, theme, chip, &[])
}

/// Render a block body for display, highlighting one range of the **source** at the same
/// place in the rendered text. Markup is stripped (`[[Ana]]` → `Ana`, `**x**` → `x`), so a
/// source offset does not index the rendered string; the range is located by rendering the
/// source with two private-use markers around it and reading their positions back. The
/// range must be plain text (an unlinked mention never lies inside a link, tag or code).
pub fn render_highlighted(
    src: &str,
    theme: &Theme,
    chip: &mut dyn FnMut(&str) -> ChipState,
    range: Range<usize>,
    style: HighlightStyle,
) -> Rendered {
    let mut out = render(src, theme, chip);
    if range.start >= range.end
        || range.end > src.len()
        || !src.is_char_boundary(range.start)
        || !src.is_char_boundary(range.end)
    {
        return out;
    }
    const OPEN: char = '\u{E000}';
    const CLOSE: char = '\u{E001}';
    let mut marked = String::with_capacity(src.len() + 6);
    marked.push_str(&src[..range.start]);
    marked.push(OPEN);
    marked.push_str(&src[range.start..range.end]);
    marked.push(CLOSE);
    marked.push_str(&src[range.end..]);
    let probe = render(&marked, theme, chip);
    let (Some(open), Some(close)) = (probe.text.find(OPEN), probe.text.find(CLOSE)) else {
        return out;
    };
    // The rendered string drops the markers; the end shifts back by the opening marker.
    out.highlights
        .push((open..close.saturating_sub(OPEN.len_utf8()), style));
    out.highlights.sort_by_key(|(r, _)| r.start);
    out
}

/// Render a block body for display, with the page's own titles (normalized) drawn as bold
/// ink rather than links (DESIGN.md §3.7).
pub fn render_self(
    src: &str,
    theme: &Theme,
    chip: &mut dyn FnMut(&str) -> ChipState,
    self_titles: &[String],
) -> Rendered {
    // `Key:: value` shows the key as a label (P1 attributes).
    let attr = utopia_core::inline::attribute(src);
    let base = attr.as_ref().map_or(0, |(_, value)| value.start);
    let body = &src[base..];
    let mut b = Builder {
        theme,
        chip,
        self_titles,
        src: body,
        base,
        text: String::new(),
        highlights: Vec::new(),
        links: Vec::new(),
        chips: Vec::new(),
        spans: Vec::new(),
    };
    if let Some((key, _)) = &attr {
        let st = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            color: Some(theme.muted),
            ..Default::default()
        };
        let label = b.push(&format!("{key}: "), st);
        // The label is the source's key and a separator that stands for its `:: `.
        let at = src.find(key.as_str()).unwrap_or(0);
        let (shown, source) = (label.start + key.len(), at + key.len());
        b.spans.push(Span {
            shown: label.start..shown,
            source: at..source,
            exact: true,
        });
        b.spans.push(Span {
            shown: shown..label.end,
            source: source..base,
            exact: false,
        });
    }
    b.walk(&parse(body), 0..body.len(), Marks::default());
    Rendered {
        text: b.text.into(),
        highlights: b.highlights,
        links: b.links,
        chips: b.chips,
        spans: b.spans,
        source_len: src.len(),
    }
}

/// A rendered block body whose links show the pointing hand on hover.
///
/// Links are runs inside one shaped text (§13.4), not child elements, so no `div` can
/// carry `cursor_pointer` for them. Instead each link's line boxes get a hitbox with the
/// pointing-hand cursor: gpui then switches the cursor as the mouse crosses them without
/// re-rendering, and clicks still reach the row's own handler (the hitboxes don't block).
pub struct LinkText {
    text: StyledText,
    links: Vec<Range<usize>>,
    chips: Vec<(Range<usize>, Hsla)>,
    rings: Vec<Range<usize>>,
    ring_color: Hsla,
}

impl LinkText {
    pub fn new(text: StyledText, links: Vec<Range<usize>>) -> Self {
        Self {
            text,
            links,
            chips: Vec::new(),
            rings: Vec::new(),
            ring_color: gpui::transparent_black(),
        }
    }

    /// Draw `chips` (tag runs) as rounded `bg` fills behind the text (DESIGN.md §3.1).
    pub fn with_chips(mut self, chips: Vec<Range<usize>>, bg: Hsla) -> Self {
        self.chips = chips.into_iter().map(|r| (r, bg)).collect();
        self
    }

    /// Ring the links at `rings` (DESIGN.md §4.3: the link whose page is open in the next
    /// pane gets a 2 px `link_tint` ring, radius 3). A tag's ring goes round its chip.
    pub fn with_rings(mut self, rings: Vec<Range<usize>>, color: Hsla) -> Self {
        self.rings = rings;
        self.ring_color = color;
        self
    }
}

impl IntoElement for LinkText {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for LinkText {
    type RequestLayoutState = ();
    type PrepaintState = Vec<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        Element::request_layout(&mut self.text, id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Hitbox> {
        Element::prepaint(&mut self.text, id, inspector_id, bounds, state, window, cx);
        let layout = self.text.layout().clone();
        self.links
            .iter()
            .flat_map(|r| link_boxes(&layout, r, bounds))
            .map(|b| window.insert_hitbox(b, HitboxBehavior::Normal))
            .collect()
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        hitboxes: &mut Vec<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Chips first, so their fill sits under the glyphs. A run's own background would
        // fill the whole line box; a tag chip is 22 px high. Its 7 px sides reach into the
        // whitespace around the tag, because a shaped run cannot move its glyphs; two
        // adjacent tags share that whitespace and each takes half, leaving a 2 px gap.
        let layout = self.text.layout().clone();
        let line_height = layout.line_height();
        for (range, bg) in &self.chips {
            let Some(start) = layout.position_for_index(range.start) else {
                continue;
            };
            let Some(end) = layout.position_for_index(range.end) else {
                continue;
            };
            let (left, right) = chip_padding(&layout, &self.chips, range);
            for rect in chip_line_rects(start, end, line_height, bounds, left, right) {
                window.paint_quad(gpui::fill(rect, *bg).corner_radii(px(RADIUS_TAG)));
            }
        }
        for range in &self.rings {
            let (Some(start), Some(end)) = (
                layout.position_for_index(range.start),
                layout.position_for_index(range.end),
            ) else {
                continue;
            };
            // Outside the chip for a tag, just outside the glyphs for a link.
            let (left, right) = if self.chips.iter().any(|(r, _)| r == range) {
                chip_padding(&layout, &self.chips, range)
            } else {
                (0., 0.)
            };
            let w = LINK_RING_WIDTH;
            for rect in chip_line_rects(start, end, line_height, bounds, left + w, right + w) {
                let rect = Bounds::from_corners(
                    point(rect.left(), rect.top() - px(w)),
                    point(rect.right(), rect.bottom() + px(w)),
                );
                window.paint_quad(gpui::quad(
                    rect,
                    px(RADIUS_LINK_RING),
                    gpui::transparent_black(),
                    px(w),
                    self.ring_color,
                    gpui::BorderStyle::Solid,
                ));
            }
        }
        Element::paint(
            &mut self.text,
            id,
            inspector_id,
            bounds,
            state,
            &mut (),
            window,
            cx,
        );
        for h in hitboxes.iter() {
            window.set_cursor_style(CursorStyle::PointingHand, h);
        }
    }
}

/// One chip fill per visual line a tag covers. `link_boxes` gives a link a single tall box
/// for the lines it crosses in full, but a chip is a fixed 22 px tall, so every line needs
/// its own rect or the interior lines lose their background.
fn chip_line_rects(
    start: Point<Pixels>,
    end: Point<Pixels>,
    line_height: Pixels,
    bounds: Bounds<Pixels>,
    left_pad: f32,
    right_pad: f32,
) -> Vec<Bounds<Pixels>> {
    let lh = f32::from(line_height);
    let lines = ((f32::from(end.y) - f32::from(start.y)).max(0.) / lh).round() as usize + 1;
    let mut out = Vec::with_capacity(lines);
    for i in 0..lines {
        let y = px(f32::from(start.y) + lh * i as f32);
        let x0 = if i == 0 {
            start.x - px(left_pad)
        } else {
            bounds.left()
        };
        let x1 = if i + 1 == lines {
            // Ending exactly at a line break puts the end on the next line's left edge;
            // that is not a line of its own.
            if end.x <= bounds.left() && lines > 1 {
                continue;
            }
            end.x + px(right_pad)
        } else {
            bounds.right()
        };
        let (x0, x1) = (x0.max(bounds.left()), x1.min(bounds.right()));
        if x1 <= x0 {
            continue;
        }
        let top = y + px((lh - TAG_HEIGHT) / 2.);
        out.push(Bounds::from_corners(
            point(x0, top),
            point(x1, top + px(TAG_HEIGHT)),
        ));
    }
    out
}

/// How far a tag chip may reach into the whitespace on each side. The spec's 7 px padding
/// cannot move glyphs inside one shaped run, so it is spent on the spaces that are already
/// there: a tag followed by a word keeps its side out of the word, and two tags separated
/// by one space split it, leaving a 2 px gap between their chips.
fn chip_padding(
    layout: &TextLayout,
    chips: &[(Range<usize>, Hsla)],
    range: &Range<usize>,
) -> (f32, f32) {
    let text = layout.text();
    let side = |edge: usize, back: bool| -> f32 {
        let Some(space) = (if back {
            prev_char_boundary(&text, edge)
        } else {
            Some(edge)
        }) else {
            return 0.;
        };
        let Some(next) = next_char_boundary(&text, space) else {
            return 0.;
        };
        let gap = &text[space..next];
        if !gap.chars().all(char::is_whitespace) || gap.is_empty() {
            return 0.;
        }
        // The width of that whitespace on the line the tag is on.
        let (Some(a), Some(b)) = (
            layout.position_for_index(space),
            layout.position_for_index(next),
        ) else {
            return 0.;
        };
        if a.y != b.y {
            return 0.;
        }
        let width = f32::from(b.x - a.x);
        if width <= 0. {
            return 0.;
        }
        // Is the other side of the space another chip's edge? Then the space is shared.
        let other = if back { space } else { next };
        let shared = chips.iter().any(|(r, _)| {
            if back {
                r.end == other
            } else {
                r.start == other
            }
        });
        if shared {
            ((width - 2.).max(0.)) / 2.
        } else {
            width.min(TAG_PADDING_X)
        }
    };
    let left = side(range.start, true);
    let right = side(range.end, false);
    (left, right)
}

fn prev_char_boundary(text: &str, i: usize) -> Option<usize> {
    if i == 0 {
        return None;
    }
    let mut p = i - 1;
    while p > 0 && !text.is_char_boundary(p) {
        p -= 1;
    }
    Some(p)
}

fn next_char_boundary(text: &str, i: usize) -> Option<usize> {
    if i >= text.len() {
        return None;
    }
    let mut p = i + 1;
    while p < text.len() && !text.is_char_boundary(p) {
        p += 1;
    }
    Some(p)
}

/// The boxes a run of text covers: one per visual line it touches.
fn link_boxes(
    layout: &TextLayout,
    r: &Range<usize>,
    bounds: Bounds<Pixels>,
) -> Vec<Bounds<Pixels>> {
    let (Some(a), Some(b)) = (
        layout.position_for_index(r.start),
        layout.position_for_index(r.end),
    ) else {
        return Vec::new();
    };
    let lh = layout.line_height();
    if (a.y - b.y).abs() < px(1.) {
        return vec![Bounds::from_corners(a, point(b.x, a.y + lh))];
    }
    let mut out = vec![Bounds::from_corners(a, point(bounds.right(), a.y + lh))];
    if b.y > a.y + lh {
        out.push(Bounds::from_corners(
            point(bounds.left(), a.y + lh),
            point(bounds.right(), b.y),
        ));
    }
    out.push(Bounds::from_corners(
        point(bounds.left(), b.y),
        point(b.x, b.y + lh),
    ));
    out
}

/// Syntax colouring for the focused block's source text: references are coloured, the
/// characters themselves are untouched so caret offsets map 1:1.
pub fn source_highlights(src: &str, theme: &Theme) -> Vec<(Range<usize>, HighlightStyle)> {
    extract_refs(src)
        .into_iter()
        .map(|r| {
            let color = match r.kind {
                RefKind::Tag => theme.tag_ink,
                _ => theme.link,
            };
            (
                r.range,
                HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_refs_as_links_without_brackets() {
        let t = Theme::light();
        let r = render(
            "Met [[Dana]] re #proj and ((abc)) **now**",
            &t,
            &mut |uid| {
                if uid == "abc" {
                    ChipState::Live("a block".into())
                } else {
                    ChipState::Missing
                }
            },
        );
        assert_eq!(r.text.as_ref(), "Met Dana re #proj and  a block › now");
        assert_eq!(r.links.len(), 3);
        assert_eq!(r.link_at(5), Some(&LinkTarget::Page("Dana".into())));
        assert_eq!(r.link_at(13), Some(&LinkTarget::Page("proj".into())));
        assert!(matches!(r.link_at(24), Some(LinkTarget::Block(u)) if u == "abc"));
        assert_eq!(r.link_at(0), None);
        // DESIGN.md §3.1: a tag is a chip, not just coloured text.
        assert_eq!(r.chips.len(), 1);
        assert_eq!(&r.text[r.chips[0].clone()], "#proj");
        // Highlights are sorted and non-overlapping (StyledText requirement).
        for w in r.highlights.windows(2) {
            assert!(w[0].0.end <= w[1].0.start);
        }
    }

    #[test]
    fn a_long_tag_is_chipped_end_to_end() {
        let t = Theme::light();
        let words = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi \
            omicron pi rho sigma tau upsilon phi chi psi omega one two three four five six \
            seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen";
        let src = format!("#[[{words}]]");
        let r = render(&src, &t, &mut |_| ChipState::Missing);
        assert_eq!(r.chips.len(), 1);
        let chip = r.chips[0].clone();
        assert_eq!(chip.start, 0);
        assert_eq!(chip.end, r.text.len(), "the chip covers the whole tag");
    }

    /// A chip is 22 px tall, so a run that crosses four lines needs four fills; a single
    /// tall rect would leave the middle two lines blank.
    #[test]
    fn a_wrapped_tag_chip_fills_every_visual_line() {
        let bounds = Bounds::new(point(px(40.), px(100.)), gpui::size(px(600.), px(500.)));
        let rects = chip_line_rects(
            point(px(100.), px(100.)),
            point(px(200.), px(184.)),
            px(28.),
            bounds,
            7.,
            7.,
        );
        assert_eq!(rects.len(), 4, "{rects:?}");
        for (i, r) in rects.iter().enumerate() {
            assert!(
                (f32::from(r.origin.y) - (103. + 28. * i as f32)).abs() < 0.5,
                "line {i}: {r:?}"
            );
            assert!((f32::from(r.size.height) - 22.).abs() < 0.5, "{r:?}");
        }
        assert!(
            (f32::from(rects[0].origin.x) - 93.).abs() < 0.5,
            "{rects:?}"
        );
        assert!(
            (f32::from(rects[3].size.width) - 167.).abs() < 0.5,
            "{rects:?}"
        );
    }

    /// DESIGN.md §3.7: the current page's name inside one of its own references is bold
    /// ink text, not a link; every other page stays a link.
    #[test]
    fn the_current_pages_name_is_bold_ink_not_a_link() {
        let t = Theme::light();
        let self_titles = vec![utopia_core::title::normalize_title("Atlas Launch")];
        let r = render_self(
            "see [[Atlas Launch]] and [[Ana Lovelace]]",
            &t,
            &mut |_| ChipState::Missing,
            &self_titles,
        );
        assert_eq!(r.links.len(), 1, "{:?}", r.links);
        assert_eq!(r.link_at(4), None, "the self reference is not a link");
        assert!(
            matches!(r.links[0].1, LinkTarget::Page(ref p) if p == "Ana Lovelace"),
            "another page stays a link"
        );
        let run = r
            .highlights
            .iter()
            .find(|(range, _)| &r.text[range.clone()] == "Atlas Launch")
            .expect("the self title has a highlight");
        assert_eq!(run.1.color, Some(t.ink), "bold ink, not the link colour");
        assert_eq!(run.1.font_weight, Some(FontWeight::BOLD));
    }

    /// An unlinked mention's range is a source offset. Markup before it is stripped, so the
    /// highlight must be mapped to the rendered text, not pushed unchanged (§3.7).
    #[test]
    fn a_mention_highlight_maps_through_stripped_markup() {
        let t = Theme::light();
        let src = "Met [[Ana Lovelace]] about the import plan";
        let start = src.find("import plan").expect("the mention");
        let style = HighlightStyle {
            background_color: Some(t.highlight_bg),
            ..Default::default()
        };
        let r = render_highlighted(
            src,
            &t,
            &mut |_| ChipState::Missing,
            start..start + "import plan".len(),
            style,
        );
        let run = r
            .highlights
            .iter()
            .find(|(range, _)| &r.text[range.clone()] == "import plan")
            .unwrap_or_else(|| panic!("the highlight covers the mention: {:?}", r.highlights));
        // The raw offset is four bytes late once `[[ ]]` is stripped; that was the bug.
        let rendered_start = r.text.find("import plan").expect("rendered mention");
        assert_eq!(run.0.start, rendered_start);
        assert_ne!(
            rendered_start, start,
            "source offsets do not index the text"
        );
    }

    /// The same mapping at the end of the block: a raw range whose end is past the rendered
    /// length used to be dropped by a length guard, leaving the mention unhighlighted.
    #[test]
    fn a_mention_at_the_end_after_a_link_is_highlighted() {
        let t = Theme::light();
        let src = "See [[Ana Lovelace]] about the import plan";
        let start = src.rfind("import plan").expect("the mention");
        let style = HighlightStyle {
            background_color: Some(t.highlight_bg),
            ..Default::default()
        };
        let r = render_highlighted(
            src,
            &t,
            &mut |_| ChipState::Missing,
            start..src.len(),
            style,
        );
        assert!(
            r.highlights
                .iter()
                .any(|(range, _)| &r.text[range.clone()] == "import plan"),
            "the highlight still lands on the mention: {:?}",
            r.highlights
        );
    }

    /// A click in a rendered block is answered in the rendered text, and the caret goes
    /// into the source. With markup before it the two offsets differ; the click used to
    /// be answered with the end of the block.
    #[test]
    fn a_rendered_offset_maps_to_the_source_under_it() {
        let t = Theme::light();
        let caret = |src: &str, shown_before: &str| {
            let r = render(src, &t, &mut |_| ChipState::Live("a block".into()));
            let ix = r.text.find(shown_before).expect(shown_before);
            (r.text.to_string(), r.source_offset(ix))
        };
        // Plain text: the offsets are the same.
        assert_eq!(caret("alpha beta", "ta").1, 8);
        // After a link, a tag, emphasis and code: the caret is before the same character.
        for (src, word) in [
            ("Met [[Ana Lovelace]] about the plan", "lan"),
            ("#project and #[[Modern Religion]] today", "day"),
            ("a **bold** and _soft_ and ~~gone~~ word", "ord"),
            ("run `cargo test` twice", "wice"),
            ("see ((abc)) and {{embed: ((abc))}} here", "ere"),
            ("read [the docs](https://example.com) first", "irst"),
            (r"2 \* 3 \* 4 is [[Math]] done", "one"),
            ("日本語 [[東京]] の天気", "天気"),
        ] {
            let (shown, at) = caret(src, word);
            assert_eq!(&src[at..], word, "{src:?} shown as {shown:?}");
        }
        // Inside emphasis and code: the character under the click, inside the markup.
        let src = "a **bold** word";
        assert_eq!(&src[caret(src, "ld").1..], "ld** word");
        let src = "run `cargo test` twice";
        assert_eq!(&src[caret(src, "test").1..], "test` twice");
        // An escaped character is one character on screen and two in the source.
        let src = r"2 \* 3 \* 4";
        assert_eq!(&src[caret(src, "3").1..], r"3 \* 4");
        // An attribute's label: the key is the source's key, the value starts after `:: `.
        let src = "Status:: in **progress** now";
        assert_eq!(&src[caret(src, "tus").1..], "tus:: in **progress** now");
        assert_eq!(&src[caret(src, "in ").1..], "in **progress** now");
        assert_eq!(&src[caret(src, "now").1..], "now");
        // Past the last character is the end of the source, closing markup included.
        for src in ["ends in **bold**", "ends in [[a link]]", "", "Key:: "] {
            let r = render(src, &t, &mut |_| ChipState::Missing);
            assert_eq!(r.source_offset(r.text.len()), src.len(), "{src:?}");
        }
    }

    #[test]
    fn missing_chip_is_explicit() {
        let t = Theme::light();
        let r = render("((zzz))", &t, &mut |_| ChipState::Missing);
        assert!(r.text.contains("Missing block ((zzz))"));
    }

    #[test]
    fn long_chip_is_truncated() {
        assert_eq!(chip_label(&"x".repeat(50)).chars().count(), 41);
    }

    /// A link that leaves the app ends in ↗; the arrow opens it too, is not underlined, and
    /// has no source behind it, so a caret in it goes to the end of the link's source.
    #[test]
    fn an_external_link_ends_in_an_up_right_arrow() {
        let t = Theme::light();
        let src = "go https://x.y now";
        let r = render(src, &t, &mut |_| ChipState::Missing);
        assert_eq!(
            r.text.as_ref(),
            format!("go https://x.y{EXTERNAL_ARROW} now")
        );
        let url = LinkTarget::Url("https://x.y".into());
        let arrow = r.text.find('↗').unwrap();
        assert_eq!(r.link_at(3), Some(&url));
        assert_eq!(r.link_at(arrow), Some(&url), "the arrow opens the link");
        assert_eq!(r.link_at(arrow + '↗'.len_utf8()), None);
        let underlined: Vec<_> = r
            .highlights
            .iter()
            .filter(|(_, s)| s.underline.is_some())
            .map(|(range, _)| &r.text[range.clone()])
            .collect();
        assert_eq!(underlined, ["https://x.y"]);
        let after = r.text.find(" now").unwrap();
        assert_eq!(r.source_offset(after), src.find(" now").unwrap());
        assert_eq!(
            r.source_offset(arrow + 1),
            "go https://x.y".len(),
            "inside the arrow"
        );
        assert_eq!(r.source_offset(2), 2);

        let md = render("[site](https://x.y)", &t, &mut |_| ChipState::Missing);
        assert_eq!(md.text.as_ref(), format!("site{EXTERNAL_ARROW}"));
        let arrow = md.text.find('↗').unwrap();
        assert_eq!(md.source_offset(2), "[si".len());
        assert_eq!(md.source_offset(arrow), "[site](https://x.y)".len());
        // Links that stay in the app get no arrow.
        let r = render("[p](utopia://page/x)", &t, &mut |_| ChipState::Missing);
        assert_eq!(r.text.as_ref(), "p");
        let h = "a".repeat(64);
        let r = render(&format!("[f.txt](blob:{h})"), &t, &mut |_| {
            ChipState::Missing
        });
        assert_eq!(r.text.as_ref(), "📎 f.txt");
    }
}
