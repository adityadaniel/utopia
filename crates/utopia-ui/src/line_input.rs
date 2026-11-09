//! Single-line text input used by quick jump, page rename and reference filters.

use gpui::{
    div, prelude::*, px, ClipboardItem, Context, EventEmitter, FocusHandle, Focusable,
    HighlightStyle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, SharedString,
    StyledText, Window,
};

use crate::actions::*;
use crate::field::{FieldElement, FieldHost, TextField};
use crate::theme::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineInputEvent {
    Changed(String),
    /// Enter; `alt` for Cmd+Enter or Shift+Enter (e.g. open in sidebar).
    Submit {
        alt: bool,
    },
    Cancel,
    Up,
    Down,
}

pub struct LineInput {
    field: TextField,
    focus: FocusHandle,
    placeholder: SharedString,
    /// Passphrase entry: shown as `*`, never copied.
    masked: bool,
}

impl EventEmitter<LineInputEvent> for LineInput {}

impl Focusable for LineInput {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FieldHost for LineInput {
    fn field(&self) -> &TextField {
        &self.field
    }
    fn field_mut(&mut self) -> &mut TextField {
        &mut self.field
    }
    fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus
    }
}

crate::impl_field_input_handler!(LineInput, LineInput::after_edit);

impl LineInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            field: TextField::new(false),
            focus: cx.focus_handle(),
            placeholder: placeholder.into(),
            masked: false,
        }
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        self.masked = masked;
        cx.notify();
    }

    pub fn text(&self) -> &str {
        &self.field.text
    }

    /// Set the whole text (a host-written value, e.g. a `utopia://search` query) and tell
    /// the host, exactly as typing does, so a host mirroring `text()` never lags (§13.5).
    ///
    /// Writing the text it already shows is not a change: the reference filter is cleared
    /// on every page change (`PageView::show`, §13.4), and emitting there would make its
    /// listener rebuild the item list a second time for nothing (rule 10).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.field.text == text {
            return;
        }
        self.field.set_text(text, text.len());
        self.field.move_to(0);
        self.field.select_to(text.len());
        cx.emit(LineInputEvent::Changed(self.field.text.clone()));
        cx.notify();
    }

    /// Show a saved value for editing (a Settings field): caret at the end, nothing
    /// selected. [`Self::set_text`] selects what it wrote, which is right for a value the
    /// user is expected to replace and wrong for one they are expected to keep.
    pub fn fill(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.field.text == text {
            return;
        }
        self.field.set_text(text, text.len());
        cx.emit(LineInputEvent::Changed(self.field.text.clone()));
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus);
    }

    fn after_edit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.field.composing() {
            cx.emit(LineInputEvent::Changed(self.field.text.clone()));
        }
    }

    fn edit(
        &mut self,
        range: std::ops::Range<usize>,
        s: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.field.replace(range, s);
        self.after_edit(window, cx);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        // §13.5: the input method owns the buffer while it composes.
        if self.field.composing() {
            return;
        }
        if self.field.sel.is_empty() {
            let to = self.field.prev_boundary();
            self.field.select_to(to);
        }
        let sel = self.field.sel.clone();
        self.edit(sel, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        // §13.5: see `backspace`.
        if self.field.composing() {
            return;
        }
        if self.field.sel.is_empty() {
            let to = self.field.next_boundary();
            self.field.select_to(to);
        }
        let sel = self.field.sel.clone();
        self.edit(sel, "", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.prev_boundary()
        } else {
            self.field.sel.start
        };
        self.field.move_to(to);
        cx.notify();
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.next_boundary()
        } else {
            self.field.sel.end
        };
        self.field.move_to(to);
        cx.notify();
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.prev_boundary();
        self.field.select_to(to);
        cx.notify();
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.next_boundary();
        self.field.select_to(to);
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        self.field.move_to(0);
        self.field.select_to(len);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.field.move_to(0);
        cx.notify();
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        self.field.move_to(len);
        cx.notify();
    }

    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.field.move_to(0);
        cx.notify();
    }

    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        self.field.move_to(len);
        cx.notify();
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = utopia_core::text::prev_word_start(&self.field.text, self.move_origin(true));
        self.field.move_to(to);
        cx.notify();
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = utopia_core::text::next_word_end(&self.field.text, self.move_origin(false));
        self.field.move_to(to);
        cx.notify();
    }

    /// Where a plain move starts from, macOS style: with a selection it collapses to the
    /// edge the move heads towards instead of travelling from the far end (§9.1).
    fn move_origin(&self, leftward: bool) -> usize {
        if self.field.sel.is_empty() {
            self.field.cursor()
        } else if leftward {
            self.field.sel.start
        } else {
            self.field.sel.end
        }
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = utopia_core::text::prev_word_start(&self.field.text, self.field.cursor());
        self.field.select_to(to);
        cx.notify();
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = utopia_core::text::next_word_end(&self.field.text, self.field.cursor());
        self.field.select_to(to);
        cx.notify();
    }

    /// The whole text is one line here, so Cmd+Shift+Left / Shift+Home extend the
    /// selection to the start of the field and Cmd+Shift+Right / Shift+End to its end.
    fn select_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to_edge(false, cx);
    }

    fn select_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to_edge(true, cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to_edge(false, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to_edge(true, cx);
    }

    /// Grow the selection to the start or end of the field, never shrinking it (the
    /// macOS "select to the end of the line" behaviour, see `BlockEditor::extend_to`).
    fn select_to_edge(&mut self, end: bool, cx: &mut Context<Self>) {
        let sel = self.field.sel.clone();
        if end {
            self.field.move_to(sel.start);
            self.field.select_to(self.field.text.len());
        } else {
            self.field.move_to(sel.end);
            self.field.select_to(0);
        }
        cx.notify();
    }

    /// Remove `range` as one edit, replacing the selection the caret came from.
    fn delete_range(
        &mut self,
        range: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.field.composing() || range.is_empty() {
            return;
        }
        self.edit(range, "", window, cx);
    }

    /// Option+Backspace: back to the start of the previous word; at the start, nothing.
    fn delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = self.field.sel.clone();
        let range = if sel.is_empty() {
            let c = self.field.cursor();
            utopia_core::text::prev_word_start(&self.field.text, c)..c
        } else {
            sel
        };
        self.delete_range(range, window, cx);
    }

    /// Option+Fn+Delete: forward to the end of the next word.
    fn delete_word_forward(
        &mut self,
        _: &DeleteWordForward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = self.field.sel.clone();
        let range = if sel.is_empty() {
            let c = self.field.cursor();
            c..utopia_core::text::next_word_end(&self.field.text, c)
        } else {
            sel
        };
        self.delete_range(range, window, cx);
    }

    /// Cmd+Backspace: the text before the caret, which in a single-line field is the whole
    /// line.
    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = self.field.sel.clone();
        let range = if sel.is_empty() {
            0..self.field.cursor()
        } else {
            sel
        };
        self.delete_range(range, window, cx);
    }

    fn emacs_start_of_line(
        &mut self,
        _: &EmacsStartOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.field.move_to(0);
        cx.notify();
    }

    fn emacs_end_of_line(&mut self, _: &EmacsEndOfLine, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        self.field.move_to(len);
        cx.notify();
    }

    fn emacs_back_char(&mut self, _: &EmacsBackChar, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.prev_boundary()
        } else {
            self.field.sel.start
        };
        self.field.move_to(to);
        cx.notify();
    }

    fn emacs_forward_char(&mut self, _: &EmacsForwardChar, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.next_boundary()
        } else {
            self.field.sel.end
        };
        self.field.move_to(to);
        cx.notify();
    }

    /// Ctrl+P / Ctrl+N: the input's own up and down, which in the palette and the
    /// reference menus move through the list.
    fn emacs_prev_line(&mut self, _: &EmacsPrevLine, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Up);
    }

    fn emacs_next_line(&mut self, _: &EmacsNextLine, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Down);
    }

    /// Ctrl+K: kill from the caret to the end of the text and remember it for Ctrl+Y.
    fn emacs_kill_line(&mut self, _: &EmacsKillLine, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.composing() {
            return;
        }
        let sel = self.field.sel.clone();
        let range = if sel.is_empty() {
            self.field.cursor()..self.field.text.len()
        } else {
            sel
        };
        if range.is_empty() {
            return;
        }
        // A passphrase field is masked for a reason: what it kills is not kept in a buffer
        // that outlives the field (masking is a local UI choice, rule 9).
        if !self.masked {
            crate::kill::remember(&self.field.text[range.clone()]);
        }
        self.delete_range(range, window, cx);
    }

    fn emacs_yank(&mut self, _: &EmacsYank, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.composing() {
            return;
        }
        // A masked field shows no text, so a yank would put something there the user
        // cannot see, and a passphrase typed that way would be a stranger's.
        if self.masked {
            return;
        }
        let text = crate::kill::last();
        if text.is_empty() {
            return;
        }
        let sel = self.field.sel.clone();
        self.edit(sel, &text.replace('\n', " "), window, cx);
    }

    fn emacs_delete_char(
        &mut self,
        _: &EmacsDeleteChar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(&Delete, window, cx);
    }

    fn emacs_backspace(&mut self, _: &EmacsBackspace, window: &mut Window, cx: &mut Context<Self>) {
        self.backspace(&Backspace, window, cx);
    }

    /// Ctrl+T: swap the graphemes around the caret (see [`utopia_core::text::transpose`]).
    fn emacs_transpose(&mut self, _: &EmacsTranspose, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.composing() {
            return;
        }
        let caret = self.field.cursor();
        let Some((range, swapped, caret)) = utopia_core::text::transpose(&self.field.text, caret)
        else {
            return;
        };
        self.edit(range, &swapped, window, cx);
        self.field.move_to(caret);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.field.sel.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.field.selected_text().to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.field.sel.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.field.selected_text().to_string(),
            ));
            let sel = self.field.sel.clone();
            self.edit(sel, "", window, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
            let sel = self.field.sel.clone();
            self.edit(sel, &text.replace('\n', " "), window, cx);
        }
    }

    fn submit(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Submit { alt: false });
    }

    fn submit_alt(&mut self, _: &InsertAbove, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Submit { alt: true });
    }

    fn submit_shift(&mut self, _: &SoftBreak, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Submit { alt: true });
    }

    fn cancel(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Cancel);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Up);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(LineInputEvent::Down);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        let ix = self.field.index_at(e.position);
        if e.modifiers.shift {
            self.field.select_to(ix);
        } else {
            self.field.move_to(ix);
        }
        self.field.selecting = true;
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.field.selecting {
            let ix = self.field.index_at(e.position);
            self.field.select_to(ix);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.field.selecting = false;
    }
}

impl Render for LineInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let (styled, shown) = if self.field.text.is_empty() {
            let p = self.placeholder.to_string();
            (
                StyledText::new(p.clone()).with_highlights([(
                    0..p.len(),
                    HighlightStyle {
                        color: Some(th.faint),
                        ..Default::default()
                    },
                )]),
                p,
            )
        } else if self.masked {
            // One `*` per byte keeps caret and selection offsets aligned with the text.
            let stars = "*".repeat(self.field.text.len());
            (StyledText::new(stars.clone()), stars)
        } else {
            (
                StyledText::new(self.field.text.clone()),
                self.field.text.clone(),
            )
        };
        div()
            .key_context(INPUT_CONTEXT)
            .track_focus(&self.focus)
            .w_full()
            .min_h(px(crate::appearance::density(cx).line_height()))
            .overflow_hidden()
            .cursor_text()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::delete_word_backward))
            .on_action(cx.listener(Self::delete_word_forward))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::emacs_start_of_line))
            .on_action(cx.listener(Self::emacs_end_of_line))
            .on_action(cx.listener(Self::emacs_back_char))
            .on_action(cx.listener(Self::emacs_forward_char))
            .on_action(cx.listener(Self::emacs_prev_line))
            .on_action(cx.listener(Self::emacs_next_line))
            .on_action(cx.listener(Self::emacs_kill_line))
            .on_action(cx.listener(Self::emacs_yank))
            .on_action(cx.listener(Self::emacs_delete_char))
            .on_action(cx.listener(Self::emacs_backspace))
            .on_action(cx.listener(Self::emacs_transpose))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::submit_alt))
            .on_action(cx.listener(Self::submit_shift))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .child(FieldElement::new(cx.entity(), styled, shown))
    }
}
