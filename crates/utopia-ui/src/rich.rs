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

use crate::theme::{Theme, RADIUS_TAG, TAG_HEIGHT, TAG_PADDING_X};

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
}

impl Rendered {
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
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, LinkTarget)>,
    chips: Vec<Range<usize>>,
}

impl Builder<'_> {
    fn is_self(&self, title: &str) -> bool {
        let key = utopia_core::title::normalize_title(title);
        self.self_titles.iter().any(|t| *t == key)
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

    fn walk(&mut self, nodes: &[Inline], m: Marks) {
        for n in nodes {
            match n {
                Inline::Text(t) => {
                    let st = self.style(m);
                    self.push(t, st);
                }
                Inline::Emph { mark, children, .. } => {
                    let mut m2 = m;
                    match mark {
                        Mark::Bold => m2.bold = true,
                        Mark::Italic => m2.italic = true,
                        Mark::Strike => m2.strike = true,
                        Mark::Highlight => m2.highlight = true,
                    }
                    self.walk(children, m2);
                }
                Inline::Code { text, .. } => {
                    let mut st = self.style(m);
                    st.background_color = Some(self.theme.code_bg);
                    self.push(text, st);
                }
                Inline::PageRef { title, .. } => {
                    if self.is_self(title) {
                        // DESIGN.md §3.7: the current page's name inside one of its own
                        // references is bold ink text, not a link to where you already are.
                        let mut st = self.style(m);
                        st.color = Some(self.theme.ink);
                        st.font_weight = Some(FontWeight::BOLD);
                        self.push(title, st);
                    } else {
                        let mut st = self.style(m);
                        st.color = Some(self.theme.link);
                        let r = self.push(title, st);
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
                    self.links.push((r, LinkTarget::Block(uid.clone())));
                }
                Inline::Link { text, url, .. } if url.starts_with("blob:") => {
                    // An attachment: shown as a file token; the preview sits under the block.
                    let mut st = self.style(m);
                    st.color = Some(self.theme.muted);
                    let r = self.push(&format!("📎 {text}"), st);
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
                    let r = self.push(text, st);
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

/// Render a block body for display, with the page's own titles (normalized) drawn as bold
/// ink rather than links (DESIGN.md §3.7).
pub fn render_self(
    src: &str,
    theme: &Theme,
    chip: &mut dyn FnMut(&str) -> ChipState,
    self_titles: &[String],
) -> Rendered {
    let mut b = Builder {
        theme,
        chip,
        self_titles,
        text: String::new(),
        highlights: Vec::new(),
        links: Vec::new(),
        chips: Vec::new(),
    };
    // `Key:: value` shows the key as a label (P1 attributes).
    let body = match utopia_core::inline::attribute(src) {
        Some((key, value)) => {
            let st = HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                color: Some(theme.muted),
                ..Default::default()
            };
            b.push(&format!("{key}: "), st);
            &src[value]
        }
        None => src,
    };
    b.walk(&parse(body), Marks::default());
    Rendered {
        text: b.text.into(),
        highlights: b.highlights,
        links: b.links,
        chips: b.chips,
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
}

impl LinkText {
    pub fn new(text: StyledText, links: Vec<Range<usize>>) -> Self {
        Self {
            text,
            links,
            chips: Vec::new(),
        }
    }

    /// Draw `chips` (tag runs) as rounded `bg` fills behind the text (DESIGN.md §3.1).
    pub fn with_chips(mut self, chips: Vec<Range<usize>>, bg: Hsla) -> Self {
        self.chips = chips.into_iter().map(|r| (r, bg)).collect();
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
}
