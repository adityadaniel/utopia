//! Text field core shared by the block editor and single-line inputs (§13.5): a source
//! buffer with selection and IME marked range, grapheme-aware movement, UTF-16 mapping
//! for the platform input handler, and an element that shapes wrapped text through
//! GPUI's `StyledText` and paints selection and caret on top.

use std::ops::Range;

use gpui::{
    fill, point, px, size, App, Bounds, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    PaintQuad, Pixels, Point, StyledText, TextLayout, Window,
};
use utopia_core::text as t;

use crate::theme::theme;

/// Where to put the caret once the next layout is known (Up/Down between blocks keeps
/// the horizontal position, §13.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Goal {
    FirstLine(Pixels),
    LastLine(Pixels),
}

/// Whether the platform input method owns a run of the buffer (§13.5).
///
/// While `Active` the text holds provisional glyphs the store has never seen, reported
/// back to the platform as the marked range, and the input method addresses them by
/// offset until it commits or unmarks them. Nothing else may persist, end the block or
/// edit the buffer meanwhile: clearing the range behind the IME's back leaves it
/// composing into offsets that no longer exist — the state a bare `Option<Range>` could
/// not name, and could be put into by any caller that touched the field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Composition {
    #[default]
    None,
    Active {
        range: Range<usize>,
    },
}

impl Composition {
    pub fn range(&self) -> Option<&Range<usize>> {
        match self {
            Composition::None => None,
            Composition::Active { range } => Some(range),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Composition::Active { .. })
    }
}

#[derive(Default)]
pub struct TextField {
    pub text: String,
    pub sel: Range<usize>,
    pub reversed: bool,
    pub composition: Composition,
    /// The buffer before the current or most recent composition began, so a host can
    /// tell a commit from a cancellation after [`Self::end_composition`] (§13.5).
    composition_base: Option<String>,
    pub multiline: bool,
    pub selecting: bool,
    pub goal: Option<Goal>,
    layout: Option<TextLayout>,
    layout_text: String,
    bounds: Option<Bounds<Pixels>>,
}

impl TextField {
    pub fn new(multiline: bool) -> Self {
        Self {
            multiline,
            ..Default::default()
        }
    }

    /// The marked range, as the platform input method last reported it.
    pub fn marked(&self) -> Option<&Range<usize>> {
        self.composition.range()
    }

    /// True while the input method composes: the buffer is provisional and the input
    /// method, not the host, owns every edit to it.
    pub fn composing(&self) -> bool {
        self.composition.is_active()
    }

    /// The input method inserted `range` of provisional glyphs (`setMarkedText:`).
    /// Call before the glyphs land in the buffer, so [`Self::composition_base`] is the
    /// text they replaced.
    pub fn begin_composition(&mut self, range: Range<usize>) {
        if !self.composition.is_active() {
            self.composition_base = Some(self.text.clone());
        }
        self.composition = Composition::Active { range };
    }

    /// The buffer as it was when the current or most recent composition started.
    pub fn composition_base(&self) -> Option<&str> {
        self.composition_base.as_deref()
    }

    /// The input method let go of its text: it committed (`replace_text_in_range`) or
    /// unmarked it. Returns whether it was composing.
    pub fn end_composition(&mut self) -> bool {
        std::mem::replace(&mut self.composition, Composition::None).is_active()
    }

    /// Replace the whole buffer with stored text. Only legal when the input method is not
    /// composing (callers check [`TextField::composing`] first, §13.5).
    pub fn set_text(&mut self, text: &str, caret: usize) {
        self.end_composition();
        self.text = text.to_string();
        let c = t::floor_char_boundary(&self.text, caret);
        self.sel = c..c;
        self.reversed = false;
    }

    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.sel.start
        } else {
            self.sel.end
        }
    }

    pub fn move_to(&mut self, o: usize) {
        let o = t::floor_char_boundary(&self.text, o);
        self.sel = o..o;
        self.reversed = false;
    }

    pub fn select_to(&mut self, o: usize) {
        let o = t::floor_char_boundary(&self.text, o);
        if self.reversed {
            self.sel.start = o;
        } else {
            self.sel.end = o;
        }
        if self.sel.end < self.sel.start {
            self.reversed = !self.reversed;
            self.sel = self.sel.end..self.sel.start;
        }
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.sel.clone()]
    }

    /// Replace `range` with `new`; caret lands after the insertion. Ends a composition:
    /// this is the path the input method commits its text through (§13.5).
    pub fn replace(&mut self, range: Range<usize>, new: &str) {
        self.end_composition();
        let start = t::floor_char_boundary(&self.text, range.start);
        let end = t::floor_char_boundary(&self.text, range.end.max(start));
        self.text.replace_range(start..end, new);
        let c = start + new.len();
        self.sel = c..c;
        self.reversed = false;
    }

    pub fn prev_boundary(&self) -> usize {
        t::prev_grapheme(&self.text, self.cursor())
    }

    pub fn next_boundary(&self) -> usize {
        t::next_grapheme(&self.text, self.cursor())
    }

    pub fn line_start(&self) -> usize {
        self.text[..self.cursor()].rfind('\n').map_or(0, |i| i + 1)
    }

    pub fn line_end(&self) -> usize {
        let c = self.cursor();
        self.text[c..].find('\n').map_or(self.text.len(), |i| c + i)
    }

    /// The wrapped line the caret sits on: what macOS means by "line" in Cmd+Left,
    /// Cmd+Shift+Right and Cmd+Delete. A paragraph (the text between newlines) may wrap
    /// into several of these. The end of a soft-wrapped line is where the next one starts,
    /// so selecting to it takes the space the line broke at, as TextEdit does.
    ///
    /// Until the field has been laid out (nothing painted yet) nothing has wrapped, so the
    /// paragraph answers.
    pub fn visual_line(&self) -> Range<usize> {
        self.visual_line_at(self.cursor())
    }

    /// [`Self::visual_line`] for the line `ix` is drawn on.
    pub fn visual_line_at(&self, ix: usize) -> Range<usize> {
        let ix = t::floor_char_boundary(&self.text, ix.min(self.text.len()));
        let para_start = self.text[..ix].rfind('\n').map_or(0, |i| i + 1);
        let para_end = self.text[ix..]
            .find('\n')
            .map_or(self.text.len(), |i| ix + i);
        let para = para_start..para_end;
        let Some(y) = self.point_for(ix).map(|p| p.y) else {
            return para;
        };
        // Probe char boundaries only, so a probe never lands inside `日` or an emoji.
        let bounds: Vec<usize> = self.text[para.clone()]
            .char_indices()
            .map(|(i, _)| para.start + i)
            .chain([para.end])
            .collect();
        let row = |b: usize| self.point_for(b).map_or(y, |p| p.y);
        let start = bounds[bounds.partition_point(|&b| row(b) < y)];
        let end = bounds
            .get(bounds.partition_point(|&b| row(b) <= y))
            .copied()
            .unwrap_or(para.end);
        start..end
    }

    /// Whether [`Self::visual_line`] reflects the wraps on screen: true once the current
    /// text has been laid out, or before anything was laid out (nothing has wrapped). False
    /// between an edit and the next paint, when the old layout no longer applies and the
    /// paragraph fallback could be several lines longer than the line the user sees.
    pub fn wraps_known(&self) -> bool {
        self.layout.is_none() || self.layout().is_some()
    }

    /// Where Cmd+Right puts the caret: the end of the wrapped line. On a soft-wrapped line
    /// that is before the last character (usually the space it broke at), because the
    /// offset where the next line starts is drawn on the next line.
    pub fn visual_line_end_caret(&self) -> usize {
        let line = self.visual_line();
        if line.end < self.line_end() {
            t::prev_grapheme(&self.text, line.end).max(line.start)
        } else {
            line.end
        }
    }

    // ------------------------------------------------------------ UTF-16 mapping

    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.text.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.text.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    pub fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }

    pub fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }

    // ------------------------------------------------------------ layout queries

    fn layout(&self) -> Option<&TextLayout> {
        // Only trust a layout shaped from the current text.
        self.layout
            .as_ref()
            .filter(|_| self.layout_text == self.text)
    }

    pub fn has_layout(&self) -> bool {
        self.layout().is_some()
    }

    pub fn point_for(&self, ix: usize) -> Option<Point<Pixels>> {
        self.layout()?.position_for_index(ix.min(self.text.len()))
    }

    pub fn line_height(&self) -> Pixels {
        self.layout()
            .map_or(px(crate::theme::LINE_HEIGHT), |l| l.line_height())
    }

    pub fn index_at(&self, p: Point<Pixels>) -> usize {
        let Some(l) = self.layout() else {
            return self.text.len();
        };
        let ix = match l.index_for_position(p) {
            Ok(i) | Err(i) => i,
        };
        t::floor_char_boundary(&self.text, ix)
    }

    pub fn caret_x(&self) -> Option<Pixels> {
        let b = self.bounds?;
        Some(self.point_for(self.cursor())?.x - b.left())
    }

    /// Caret index one visual line up/down, or `None` at the first/last line.
    pub fn vertical(&self, up: bool, x: Option<Pixels>) -> Option<usize> {
        let b = self.bounds?;
        let p = self.point_for(self.cursor())?;
        let lh = self.line_height();
        let first = self.point_for(0)?;
        let last = self.point_for(self.text.len())?;
        if (up && p.y <= first.y) || (!up && p.y >= last.y) {
            return None;
        }
        let x = x.map_or(p.x, |x| b.left() + x);
        let y = if up { p.y - lh / 2. } else { p.y + lh * 1.5 };
        Some(self.index_at(point(x, y)))
    }

    pub fn on_first_line(&self) -> bool {
        match (self.point_for(self.cursor()), self.point_for(0)) {
            (Some(p), Some(f)) => p.y <= f.y,
            _ => true,
        }
    }

    pub fn on_last_line(&self) -> bool {
        match (
            self.point_for(self.cursor()),
            self.point_for(self.text.len()),
        ) {
            (Some(p), Some(l)) => p.y >= l.y,
            _ => true,
        }
    }

    /// IME candidate window placement.
    pub fn bounds_for_range(&self, r: Range<usize>) -> Option<Bounds<Pixels>> {
        let a = self.point_for(r.start)?;
        let b = self.point_for(r.end)?;
        let lh = self.line_height();
        Some(Bounds::from_corners(a, point(b.x.max(a.x), b.y + lh)))
    }

    fn resolve_goal(&mut self) {
        let (Some(goal), Some(b)) = (self.goal, self.bounds) else {
            return;
        };
        if !self.has_layout() {
            return;
        }
        self.goal = None;
        let lh = self.line_height();
        let ix = match goal {
            Goal::FirstLine(x) => self.index_at(point(b.left() + x, b.top() + lh / 2.)),
            Goal::LastLine(x) => self.index_at(point(b.left() + x, b.bottom() - lh / 2.)),
        };
        self.move_to(ix);
    }
}

/// Implemented by entities that own a [`TextField`].
pub trait FieldHost: EntityInputHandler + 'static {
    fn field(&self) -> &TextField;
    fn field_mut(&mut self) -> &mut TextField;
    fn focus_handle_ref(&self) -> &FocusHandle;
}

/// For hosts that take typed text as it comes (see [`impl_field_input_handler`]).
pub fn no_intercept<T>(
    _: &mut T,
    _: Range<usize>,
    _: &str,
    _: &mut Window,
    _: &mut gpui::Context<T>,
) -> bool {
    false
}

/// Implements `EntityInputHandler` for a [`FieldHost`] by delegating to its field and
/// calling `$after(self, window, cx)` after every committed edit.
///
/// `$intercept(self, range, text, window, cx)` sees committed typing first (never IME
/// composition, which arrives through `replace_and_mark_text_in_range`) and returns
/// `true` when it applied the edit itself.
#[macro_export]
macro_rules! impl_field_input_handler {
    ($ty:ty, $after:path) => {
        $crate::impl_field_input_handler!($ty, $after, $crate::field::no_intercept);
    };
    ($ty:ty, $after:path, $intercept:path) => {
        impl gpui::EntityInputHandler for $ty {
            fn text_for_range(
                &mut self,
                range_utf16: std::ops::Range<usize>,
                actual_range: &mut Option<std::ops::Range<usize>>,
                _window: &mut gpui::Window,
                _cx: &mut gpui::Context<Self>,
            ) -> Option<String> {
                let f = $crate::field::FieldHost::field(self);
                let range = f.range_from_utf16(&range_utf16);
                let range = range.start.min(f.text.len())..range.end.min(f.text.len());
                actual_range.replace(f.range_to_utf16(&range));
                f.text.get(range).map(str::to_string)
            }

            fn selected_text_range(
                &mut self,
                _ignore_disabled_input: bool,
                _window: &mut gpui::Window,
                _cx: &mut gpui::Context<Self>,
            ) -> Option<gpui::UTF16Selection> {
                let f = $crate::field::FieldHost::field(self);
                Some(gpui::UTF16Selection {
                    range: f.range_to_utf16(&f.sel),
                    reversed: f.reversed,
                })
            }

            fn marked_text_range(
                &self,
                _window: &mut gpui::Window,
                _cx: &mut gpui::Context<Self>,
            ) -> Option<std::ops::Range<usize>> {
                let f = $crate::field::FieldHost::field(self);
                f.marked().map(|r| f.range_to_utf16(r))
            }

            fn unmark_text(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
                $crate::field::FieldHost::field_mut(self).end_composition();
                $after(self, window, cx);
            }

            fn replace_text_in_range(
                &mut self,
                range_utf16: Option<std::ops::Range<usize>>,
                new_text: &str,
                window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) {
                let f = $crate::field::FieldHost::field_mut(self);
                let range = range_utf16
                    .as_ref()
                    .map(|r| f.range_from_utf16(r))
                    .or_else(|| f.marked().cloned())
                    .unwrap_or(f.sel.clone());
                let text = if f.multiline {
                    new_text.to_string()
                } else {
                    new_text.replace('\n', " ")
                };
                if !$intercept(self, range.clone(), &text, window, cx) {
                    $crate::field::FieldHost::field_mut(self).replace(range, &text);
                    $after(self, window, cx);
                }
                cx.notify();
            }

            fn replace_and_mark_text_in_range(
                &mut self,
                range_utf16: Option<std::ops::Range<usize>>,
                new_text: &str,
                new_selected_range_utf16: Option<std::ops::Range<usize>>,
                window: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) {
                let f = $crate::field::FieldHost::field_mut(self);
                let range = range_utf16
                    .as_ref()
                    .map(|r| f.range_from_utf16(r))
                    .or_else(|| f.marked().cloned())
                    .unwrap_or(f.sel.clone());
                let start = range.start.min(f.text.len());
                let end = range.end.min(f.text.len()).max(start);
                let composing = !new_text.is_empty();
                if composing {
                    // Before the provisional glyphs land: this is the text a cancelled
                    // composition returns to.
                    f.begin_composition(start..start + new_text.len());
                }
                f.text.replace_range(start..end, new_text);
                if !composing {
                    // The input method emptied its text: the buffer is ours again.
                    f.end_composition();
                }
                let sel = new_selected_range_utf16
                    .as_ref()
                    .map(|r| {
                        // The selection is relative to the inserted text.
                        let inserted = &f.text[start..start + new_text.len()];
                        let to8 = |u: usize| {
                            let mut u8i = 0;
                            let mut u16i = 0;
                            for ch in inserted.chars() {
                                if u16i >= u {
                                    break;
                                }
                                u16i += ch.len_utf16();
                                u8i += ch.len_utf8();
                            }
                            start + u8i
                        };
                        to8(r.start)..to8(r.end)
                    })
                    .unwrap_or(start + new_text.len()..start + new_text.len());
                f.sel = sel;
                f.reversed = false;
                // Composition is provisional: nothing is persisted until it commits.
                cx.notify();
                if !composing {
                    $after(self, window, cx);
                }
            }

            fn bounds_for_range(
                &mut self,
                range_utf16: std::ops::Range<usize>,
                _bounds: gpui::Bounds<gpui::Pixels>,
                _window: &mut gpui::Window,
                _cx: &mut gpui::Context<Self>,
            ) -> Option<gpui::Bounds<gpui::Pixels>> {
                let f = $crate::field::FieldHost::field(self);
                f.bounds_for_range(f.range_from_utf16(&range_utf16))
            }

            fn character_index_for_point(
                &mut self,
                point: gpui::Point<gpui::Pixels>,
                _window: &mut gpui::Window,
                _cx: &mut gpui::Context<Self>,
            ) -> Option<usize> {
                let f = $crate::field::FieldHost::field(self);
                let ix = f.index_at(point);
                Some(f.offset_to_utf16(ix))
            }
        }
    };
}

/// Paints a [`TextField`]: wrapped text via `StyledText`, plus selection and caret.
pub struct FieldElement<V: FieldHost> {
    view: Entity<V>,
    text: StyledText,
    shown: String,
}

impl<V: FieldHost> FieldElement<V> {
    /// `shown` must be the exact string inside `text` (the buffer or a placeholder).
    pub fn new(view: Entity<V>, text: StyledText, shown: String) -> Self {
        Self { view, text, shown }
    }
}

pub struct Paint {
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl<V: FieldHost> IntoElement for FieldElement<V> {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl<V: FieldHost> Element for FieldElement<V> {
    type RequestLayoutState = ();
    type PrepaintState = Paint;

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
    ) -> Paint {
        Element::prepaint(&mut self.text, id, inspector_id, bounds, state, window, cx);
        let layout = self.text.layout().clone();
        let th = theme(cx).clone();
        let f = self.view.read(cx).field();
        let lh = layout.line_height();
        let real = self.shown == f.text;
        let pos = |ix: usize| -> Option<Point<Pixels>> {
            if real {
                layout.position_for_index(ix.min(f.text.len()))
            } else {
                layout.position_for_index(0)
            }
        };
        let mut selection = Vec::new();
        if real && !f.sel.is_empty() {
            if let (Some(a), Some(b)) = (pos(f.sel.start), pos(f.sel.end)) {
                if (a.y - b.y).abs() < px(1.) {
                    selection.push(fill(
                        Bounds::from_corners(a, point(b.x, b.y + lh)),
                        th.selection,
                    ));
                } else {
                    selection.push(fill(
                        Bounds::from_corners(a, point(bounds.right(), a.y + lh)),
                        th.selection,
                    ));
                    if b.y > a.y + lh {
                        selection.push(fill(
                            Bounds::from_corners(
                                point(bounds.left(), a.y + lh),
                                point(bounds.right(), b.y),
                            ),
                            th.selection,
                        ));
                    }
                    selection.push(fill(
                        Bounds::from_corners(point(bounds.left(), b.y), point(b.x, b.y + lh)),
                        th.selection,
                    ));
                }
            }
        }
        let cursor = if f.sel.is_empty() {
            pos(f.cursor()).map(|p| fill(Bounds::new(p, size(px(2.), lh)), th.link))
        } else {
            None
        };
        Paint { selection, cursor }
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        prepaint: &mut Paint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus_handle_ref().clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        for q in prepaint.selection.drain(..) {
            window.paint_quad(q);
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
        if focus.is_focused(window) {
            if let Some(c) = prepaint.cursor.take() {
                window.paint_quad(c);
            }
        }
        let layout = self.text.layout().clone();
        let shown = self.shown.clone();
        self.view.update(cx, |v, cx| {
            let f = v.field_mut();
            f.layout = Some(layout);
            f.layout_text = shown;
            f.bounds = Some(bounds);
            let had_goal = f.goal.is_some();
            f.resolve_goal();
            if had_goal && f.goal.is_none() {
                cx.notify();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_and_replace() {
        let mut f = TextField::new(true);
        f.set_text("hello world", 5);
        f.select_to(0);
        assert_eq!(f.selected_text(), "hello");
        assert!(f.reversed);
        f.replace(f.sel.clone(), "bye");
        assert_eq!(f.text, "bye world");
        assert_eq!(f.cursor(), 3);
    }

    #[test]
    fn utf16_round_trip() {
        let mut f = TextField::new(false);
        f.set_text("a🎉b日", 0);
        for (u8i, u16i) in [(0, 0), (1, 1), (5, 3), (6, 4), (9, 5)] {
            assert_eq!(f.offset_to_utf16(u8i), u16i);
            assert_eq!(f.offset_from_utf16(u16i), u8i);
        }
    }

    #[test]
    fn line_bounds() {
        let mut f = TextField::new(true);
        f.set_text("one\ntwo\nthree", 6);
        assert_eq!(f.line_start(), 4);
        assert_eq!(f.line_end(), 7);
    }

    /// Without a laid-out frame the visual line is the paragraph: nothing has wrapped.
    #[test]
    fn visual_line_falls_back_to_the_newline_delimited_line() {
        let mut f = TextField::new(true);
        f.set_text("one\ntwo three", 6);
        assert_eq!(f.visual_line(), 4..13);
        assert_eq!(f.visual_line_end_caret(), 13);
        f.move_to(1);
        assert_eq!(f.visual_line(), 0..3);
    }

    /// §13.5: whether the input method owns the buffer is one explicit mode, entered and
    /// left only by the transitions that mean it, and reported to the platform as the
    /// marked range in UTF-16.
    #[test]
    fn composition_is_an_explicit_mode() {
        let mut f = TextField::new(true);
        f.set_text("abにほ", 8);
        assert!(!f.composing());
        assert_eq!(f.marked(), None);
        f.begin_composition(2..8);
        assert!(f.composing());
        assert_eq!(f.marked().cloned(), Some(2..8));
        assert_eq!(f.range_to_utf16(f.marked().unwrap()), 2..4);
        // The input method commits through `replace`: one transition out.
        f.replace(2..8, "にほ");
        assert!(!f.composing());
        assert_eq!(f.text, "abにほ");
        // A host replacing the whole buffer ends it too (only callers that have checked
        // `composing` may, §13.5).
        f.begin_composition(2..8);
        f.set_text("ab", 2);
        assert!(!f.composing());
        assert_eq!(f.text, "ab");
    }
}
