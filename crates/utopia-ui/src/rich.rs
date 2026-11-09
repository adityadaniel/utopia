//! Display rendering of a block body (§6.2): marks and reference chips become styled runs
//! inside one shaped text, never child widgets (§13.4). The focused block shows its
//! source instead; see [`source_highlights`].

use std::ops::Range;

use gpui::{
    point, px, App, Bounds, CursorStyle, Element, ElementId, FontStyle, FontWeight,
    GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, StrikethroughStyle, StyledText, TextLayout, UnderlineStyle,
    Window,
};
use utopia_core::inline::{extract_refs, parse, Inline, Mark, RefKind};

use crate::theme::Theme;

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
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    links: Vec<(Range<usize>, LinkTarget)>,
}

impl Builder<'_> {
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
                    let mut st = self.style(m);
                    st.color = Some(self.theme.link);
                    let r = self.push(title, st);
                    self.links.push((r, LinkTarget::Page(title.clone())));
                }
                Inline::Tag { title, .. } => {
                    let mut st = self.style(m);
                    st.color = Some(self.theme.tag_ink);
                    let r = self.push(&format!("#{title}"), st);
                    self.links.push((r, LinkTarget::Page(title.clone())));
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
    let mut b = Builder {
        theme,
        chip,
        text: String::new(),
        highlights: Vec::new(),
        links: Vec::new(),
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
}

impl LinkText {
    pub fn new(text: StyledText, links: Vec<Range<usize>>) -> Self {
        Self { text, links }
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
        // Highlights are sorted and non-overlapping (StyledText requirement).
        for w in r.highlights.windows(2) {
            assert!(w[0].0.end <= w[1].0.start);
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
}
