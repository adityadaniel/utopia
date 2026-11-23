use std::cell::RefCell;

use gpui::{ClipboardItem, Context, EntityId, Keystroke, Window};
use utopia_core::text::{next_grapheme, prev_grapheme};
use utopia_core::undo::Caret;
use utopia_core::vim::{
    motion, operator, textobj, Command, InsertAt, Key, Motion, Operator, Parser, Step, Target,
};
use utopia_core::{BlockId, PageId};

use crate::actions::{VIM_INSERT_CONTEXT, VIM_NORMAL_CONTEXT, VIM_VISUAL_CONTEXT};
use crate::editor::{BlockEditor, EditorEvent};
use crate::field::FieldHost;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual {
        anchor: usize,
        line: bool,
    },
}

impl Mode {
    pub fn context(self) -> &'static str {
        match self {
            Self::Normal => VIM_NORMAL_CONTEXT,
            Self::Insert => VIM_INSERT_CONTEXT,
            Self::Visual { .. } => VIM_VISUAL_CONTEXT,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Change;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    pub host: EntityId,
    pub caret: Caret,
}

#[derive(Debug)]
pub struct VimState {
    pub mode: Mode,
    pub parser: Parser,
    pub resume: Option<Resume>,
    pub last_change: Option<Change>,
    pub insert_run: bool,
}

impl Default for VimState {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            parser: Parser::default(),
            resume: None,
            last_change: None,
            insert_run: false,
        }
    }
}

impl VimState {
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.parser.reset();
    }

    pub fn leave_insert(&mut self) {
        if self.mode == Mode::Insert {
            self.mode = Mode::Normal;
        }
    }
}

#[derive(Clone, Debug)]
pub enum Register {
    Chars(String),
    Blocks {
        markdown: String,
        tree: Vec<TreeRow>,
        cut: Option<(PageId, Vec<BlockId>)>,
    },
}

pub use utopia_core::outline::TreeRow;

thread_local! {
    static REGISTER: RefCell<Register> = const { RefCell::new(Register::Chars(String::new())) };
    /// What Utopia itself last put on the system clipboard (VM-11). `p` uses the clipboard
    /// only when its text differs from this, so a block yank survives the round trip as a
    /// structured register instead of coming back as Markdown.
    static CLIPBOARD: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn remember(register: Register) {
    REGISTER.with(|r| *r.borrow_mut() = register);
}

pub fn last() -> Register {
    REGISTER.with(|r| r.borrow().clone())
}

pub fn clipboard_written(text: &str) {
    CLIPBOARD.with(|c| *c.borrow_mut() = Some(text.to_string()));
}

pub fn clipboard_is_ours(text: &str) -> bool {
    CLIPBOARD.with(|c| c.borrow().as_deref() == Some(text))
}

pub fn forget_cut() {
    REGISTER.with(|r| {
        if let Register::Blocks { cut, .. } = &mut *r.borrow_mut() {
            *cut = None;
        }
    });
}

pub fn key_from_keystroke(keystroke: &Keystroke) -> Option<Key> {
    let modifiers = &keystroke.modifiers;
    if modifiers.platform {
        return None;
    }
    if keystroke.key == "tab" || keystroke.key == "backtab" {
        return None;
    }
    if !modifiers.control && !modifiers.alt && !modifiers.function {
        let named = match keystroke.key.as_str() {
            "escape" => Some(Key::Escape),
            "enter" => Some(Key::Enter),
            "space" => Some(Key::Space),
            "backspace" => Some(Key::Backspace),
            "delete" => Some(Key::Delete),
            "left" => Some(Key::Left),
            "right" => Some(Key::Right),
            "up" => Some(Key::Up),
            "down" => Some(Key::Down),
            _ => None,
        };
        if named.is_some() {
            return named;
        }
    }
    if modifiers.control && !modifiers.alt && !modifiers.function {
        let mut chars = keystroke.key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                let c = c.to_ascii_lowercase();
                #[cfg(target_os = "macos")]
                {
                    return Some(Key::Ctrl(c));
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let mut probe = Parser::default();
                    if !matches!(probe.feed(Key::Ctrl(c)), Step::Cancel) {
                        return Some(Key::Ctrl(c));
                    }
                }
            }
        }
        return None;
    }
    if let Some(text) = &keystroke.key_char {
        if let Some(c) = text.chars().next() {
            return Some(Key::Char(c));
        }
    }
    None
}

fn motion_once(text: &str, offset: usize, motion: Motion) -> usize {
    let offset = motion::clamp_normal(text, offset);
    match motion {
        Motion::Left => {
            let start = motion::line_start(text, offset);
            if offset <= start {
                offset
            } else {
                motion::clamp_normal(text, prev_grapheme(text, offset))
            }
        }
        Motion::Right => {
            let end = motion::line_end_inclusive(text, offset);
            if offset >= end {
                offset
            } else {
                motion::clamp_normal(text, next_grapheme(text, offset))
            }
        }
        Motion::WordForward { big } => motion::word_forward(text, offset, big),
        Motion::WordBack { big } => motion::word_back(text, offset, big),
        Motion::WordEnd { big } => motion::word_end(text, offset, big),
        Motion::LineStart => motion::line_start(text, offset),
        Motion::LineFirstNonBlank => motion::line_first_non_blank(text, offset),
        Motion::LineEnd => motion::line_end_inclusive(text, offset),
        Motion::Down
        | Motion::Up
        | Motion::PageFirst
        | Motion::PageLast
        | Motion::Find { .. }
        | Motion::RepeatFind { .. } => offset,
    }
}

fn find_motion(
    text: &str,
    offset: usize,
    motion: Motion,
    last_find: Option<(char, bool, bool)>,
    count: usize,
) -> Option<usize> {
    match motion {
        Motion::Find { ch, till, back } => {
            motion::find_char_nth(text, offset, ch, till, back, count)
        }
        Motion::RepeatFind { reverse } => last_find.and_then(|(ch, till, back)| {
            motion::repeat_find(text, offset, ch, till, back != reverse, count)
        }),
        _ => None,
    }
}

impl BlockEditor {
    pub fn vim_click_entry(&mut self, cx: &mut Context<BlockEditor>) {
        if !crate::appearance::vim(cx) {
            return;
        }
        // The click leaves Insert, so it leaves the Insert run too (VM-12).
        self.close_insert_run(cx);
        self.vim_mut().leave_insert();
        self.vim_mut().mode = Mode::Normal;
        self.vim_mut().parser.reset();
    }

    pub fn vim_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<BlockEditor>) {
        let step = self.vim_mut().parser.feed(key);
        match step {
            Step::Pending(_) => {
                cx.emit(EditorEvent::VimChanged);
                cx.notify();
            }
            Step::Cancel => {
                cx.emit(EditorEvent::VimChanged);
                cx.notify();
            }
            Step::Pass => {}
            Step::Run(command) => {
                self.vim_run(command, window, cx);
                self.vim_mut().parser.reset();
                cx.emit(EditorEvent::VimChanged);
                cx.notify();
            }
        }
    }

    pub fn vim_run(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        match command {
            Command::Move { motion, count } => {
                let count = count.max(1);
                match motion {
                    Motion::Down | Motion::Up => {
                        let up = motion == Motion::Up;
                        for i in 0..count {
                            match self.field().vertical(up) {
                                Some(ix) => {
                                    let to = motion::clamp_normal(&self.field().text, ix);
                                    self.field_mut().move_to(to);
                                    self.field_mut().reset_caret_motion();
                                }
                                None => {
                                    let x = self.field().caret_window_x();
                                    cx.emit(EditorEvent::Navigate {
                                        up,
                                        x,
                                        count: count - i,
                                    });
                                    return;
                                }
                            }
                        }
                        cx.notify();
                    }
                    Motion::PageFirst => cx.emit(EditorEvent::GoEdge { first: true }),
                    Motion::PageLast => cx.emit(EditorEvent::GoEdge { first: false }),
                    motion => {
                        let text = self.field().text.clone();
                        let last_find = self.vim().parser.last_find();
                        let start = self.field().cursor();
                        let found = find_motion(&text, start, motion, last_find, count);
                        let is_find =
                            matches!(motion, Motion::Find { .. } | Motion::RepeatFind { .. });
                        let offset = match found {
                            Some(at) => at,
                            None if is_find => start,
                            None => {
                                let mut at = start;
                                for _ in 0..count {
                                    let next = motion_once(&text, at, motion);
                                    if next == at {
                                        break;
                                    }
                                    at = next;
                                }
                                at
                            }
                        };
                        let offset = motion::clamp_normal(&text, offset);
                        self.field_mut().move_to(offset);
                        self.field_mut().reset_caret_motion();
                        cx.notify();
                    }
                }
            }
            Command::Insert(at) => {
                // The run opens before the caret moves, the text is deleted or the block is
                // split, so what the command does and what is typed afterwards undo
                // together (VM-12).
                self.begin_insert_run(cx);
                match at {
                    InsertAt::Cursor => {}
                    InsertAt::After => {
                        let text = self.field().text.clone();
                        let to = next_grapheme(&text, self.field().cursor());
                        self.field_mut().move_to(to);
                    }
                    InsertAt::LineStart => {
                        let text = self.field().text.clone();
                        let to = motion::line_first_non_blank(&text, self.field().cursor());
                        self.field_mut().move_to(to);
                    }
                    InsertAt::LineEnd => {
                        let text = self.field().text.clone();
                        let to = motion::line_end_exclusive(&text, self.field().cursor());
                        self.field_mut().move_to(to);
                    }
                    InsertAt::BlockBelow => {
                        let caret = self.field().text.len();
                        self.vim_mut().mode = Mode::Insert;
                        self.commit(cx);
                        cx.emit(EditorEvent::Split { caret });
                        return;
                    }
                    InsertAt::BlockAbove => {
                        self.vim_mut().mode = Mode::Insert;
                        self.commit(cx);
                        cx.emit(EditorEvent::InsertAbove);
                        return;
                    }
                    InsertAt::ReplaceChar => self.vim_delete_chars(false, 1, false, window, cx),
                    InsertAt::ReplaceLine => {
                        let len = self.field().text.len();
                        self.vim_delete(0..len, None, false, false, window, cx);
                    }
                    InsertAt::ToLineEnd => self.vim_delete_to_line_end(false, window, cx),
                }
                self.vim_mut().mode = Mode::Insert;
                self.refresh_autocomplete(cx);
            }
            Command::DeleteChar { back, count } => {
                self.vim_delete_chars(back, count, true, window, cx)
            }
            Command::DeleteToLineEnd => self.vim_delete_to_line_end(true, window, cx),
            Command::ReplaceChar(ch) => self.vim_replace_char(ch, window, cx),
            Command::ToggleCase { count } => self.vim_toggle_case(count, window, cx),
            Command::Operate {
                op,
                target,
                count,
                register,
            } => self.vim_operate(op, target, count, register, window, cx),
            Command::Paste {
                before,
                count,
                register,
            } => self.vim_paste(before, count, register, window, cx),
            Command::JoinNext { count } => {
                self.emit_after_commit(EditorEvent::JoinNext { count }, cx)
            }
            Command::Undo { count } => {
                for _ in 0..count.max(1) {
                    self.history(true, cx);
                }
            }
            Command::Redo { count } => {
                for _ in 0..count.max(1) {
                    self.history(false, cx);
                }
            }
            Command::LeaveNormal => {
                if let Some(t) = self.target().cloned() {
                    let offset = self.field().cursor();
                    self.vim_mut().resume = Some(Resume {
                        host: t.host,
                        caret: Caret {
                            page: t.page,
                            block: t.block,
                            offset,
                        },
                    });
                }
                self.end(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    // ------------------------------------------------------- charwise operators (2B)

    /// Write a register (VM-11). A yank (`clipboard`) and `"+` also put the text on the
    /// system clipboard; `d`, `c` and `x` write the register only; `"_` discards.
    fn vim_set_register(
        &self,
        text: &str,
        register: Option<char>,
        clipboard: bool,
        cx: &mut Context<BlockEditor>,
    ) {
        if register == Some('_') {
            return;
        }
        remember(Register::Chars(text.to_string()));
        if clipboard || register == Some('+') {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            clipboard_written(text);
        }
    }

    /// Delete `range` as one undo step, putting what it took in the register. The caret
    /// lands where the text was removed from; `clamp` then puts it on the block cursor's
    /// resting place, which a delete wants and a `c` command does not — its Insert bar may
    /// sit past the last grapheme of the line.
    fn vim_delete(
        &mut self,
        range: std::ops::Range<usize>,
        register: Option<char>,
        clipboard: bool,
        clamp: bool,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        if self.field().composing() {
            return;
        }
        let len = self.field().text.len();
        let range = range.start.min(len)..range.end.min(len);
        if range.is_empty() {
            return;
        }
        let cut = self.field().text[range.clone()].to_string();
        self.vim_set_register(&cut, register, clipboard, cx);
        let at = range.start;
        self.discrete_edit(range, "", Some(at), window, cx);
        if clamp {
            self.clamp_normal_cursor(cx);
        }
    }

    /// `x` and `X`: `count` graphemes under and after the cursor, or before it, inside the
    /// line (Vim's `x` never joins two lines).
    fn vim_delete_chars(
        &mut self,
        back: bool,
        count: usize,
        clamp: bool,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        if self.field().composing() {
            return;
        }
        let text = self.field().text.clone();
        let cursor = self.field().cursor();
        let count = count.max(1);
        let range = if back {
            let limit = motion::line_start(&text, cursor);
            let mut start = cursor;
            for _ in 0..count {
                if start <= limit {
                    break;
                }
                start = prev_grapheme(&text, start);
            }
            start..cursor
        } else {
            let limit = motion::line_end_exclusive(&text, cursor);
            let mut end = cursor;
            for _ in 0..count {
                if end >= limit {
                    break;
                }
                end = next_grapheme(&text, end);
            }
            cursor..end
        };
        self.vim_delete(range, None, false, clamp, window, cx);
    }

    /// `D` and `C`'s delete: the caret to the end of the line, exclusive of its break.
    fn vim_delete_to_line_end(
        &mut self,
        clamp: bool,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        if self.field().composing() {
            return;
        }
        let text = self.field().text.clone();
        let cursor = self.field().cursor();
        let end = motion::line_end_exclusive(&text, cursor);
        self.vim_delete(cursor..end, None, false, clamp, window, cx);
    }

    /// `r{char}`: put `ch` over the grapheme under the cursor. Vim writes no register for
    /// it — nothing was taken, it was replaced.
    fn vim_replace_char(&mut self, ch: char, window: &mut Window, cx: &mut Context<BlockEditor>) {
        if self.field().composing() {
            return;
        }
        let text = self.field().text.clone();
        let cursor = self.field().cursor();
        let end = next_grapheme(&text, cursor);
        if end == cursor {
            // An empty block has nothing under the cursor.
            return;
        }
        self.discrete_edit(cursor..end, &ch.to_string(), Some(cursor), window, cx);
        self.clamp_normal_cursor(cx);
    }

    /// `~`: swap the case of `count` graphemes and step over them, as Vim's does. It is a
    /// replacement, so it writes no register.
    fn vim_toggle_case(
        &mut self,
        count: usize,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        if self.field().composing() {
            return;
        }
        let text = self.field().text.clone();
        let cursor = self.field().cursor();
        let limit = motion::line_end_exclusive(&text, cursor);
        let mut end = cursor;
        for _ in 0..count.max(1) {
            if end >= limit {
                break;
            }
            end = next_grapheme(&text, end);
        }
        if end == cursor {
            return;
        }
        let mut swapped = String::with_capacity(end - cursor);
        for c in text[cursor..end].chars() {
            if c.is_lowercase() {
                swapped.extend(c.to_uppercase());
            } else if c.is_uppercase() {
                swapped.extend(c.to_lowercase());
            } else {
                swapped.push(c);
            }
        }
        // The cursor steps past the last toggled grapheme, and rests on it at a line's
        // end (1E.3).
        let at = motion::clamp_normal(&text, end);
        self.discrete_edit(cursor..end, &swapped, Some(at), window, cx);
        self.clamp_normal_cursor(cx);
    }

    /// `d`, `c` and `y` with a motion or a text object: one range from
    /// [`utopia_core::vim::operator`] or [`utopia_core::vim::textobj`], one undo step.
    ///
    /// A linewise operator (`dd`, `yy`, `>>`, `<<`) acts on BLOCKS rather than on text, so it
    /// is one host plan; only `cc` stays here, because it changes the block's own text and
    /// leaves its children (VM-8, 2B).
    fn vim_operate(
        &mut self,
        op: Operator,
        target: Target,
        count: usize,
        register: Option<char>,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        if self.field().composing() {
            return;
        }
        if matches!(target, Target::Line) {
            // A count on `>>` / `<<` would take the next siblings too; multi-block indent is
            // PR 4, so a count there indents the block the caret is on and nothing else.
            let event = match op {
                Operator::Delete => Some(EditorEvent::DeleteBlocks {
                    count: count.max(1),
                    register,
                }),
                Operator::Yank => Some(EditorEvent::YankBlocks {
                    count: count.max(1),
                    register,
                }),
                Operator::Indent => Some(EditorEvent::Indent),
                Operator::Outdent => Some(EditorEvent::Outdent),
                Operator::Change => None,
            };
            if let Some(event) = event {
                self.emit_after_commit(event, cx);
                return;
            }
        }
        let text = self.field().text.clone();
        let cursor = self.field().cursor();
        let count = count.max(1);
        let range = match target {
            // `cc`: the block's text. Its children are other blocks, so they stay (VM-8).
            Target::Line => 0..text.len(),
            Target::Object(obj) => match textobj::range_of(&text, cursor, obj) {
                Some(range) => range,
                // Not in such an object: the command writes nothing.
                None => return,
            },
            Target::Motion(motion) => {
                let motion = change_word_as_word_end(&text, cursor, op, motion);
                match operator::motion_range(
                    &text,
                    cursor,
                    motion,
                    count,
                    self.vim().parser.last_find(),
                ) {
                    Some(range) => range,
                    None => return,
                }
            }
        };
        let len = text.len();
        let range = range.start.min(len)..range.end.min(len);
        match op {
            Operator::Yank => {
                // VM-11: a yank writes the register and the system clipboard, and the
                // cursor moves to the start of what it took.
                if range.is_empty() {
                    return;
                }
                let taken = text[range.clone()].to_string();
                self.vim_set_register(&taken, register, true, cx);
                let at = range.start;
                self.field_mut().move_to(at);
                self.field_mut().reset_caret_motion();
                self.clamp_normal_cursor(cx);
            }
            Operator::Delete => self.vim_delete(range, register, false, true, window, cx),
            Operator::Change => {
                // The run opens before the delete, so the delete and the typing are one
                // undo step (VM-12), and Insert starts after it. A range of no width
                // (`ci(` in `()`) still enters Insert, as Vim's does.
                self.begin_insert_run(cx);
                self.vim_delete(range, register, false, false, window, cx);
                self.vim_mut().mode = Mode::Insert;
                self.refresh_autocomplete(cx);
            }
            // `>` with a motion (`>w`) would indent lines; that is PR 4.
            Operator::Indent | Operator::Outdent => {}
        }
    }

    /// `p` / `P` (VM-10, VM-11).
    ///
    /// What gets pasted: a block register goes to the host as one plan (it makes or moves
    /// blocks); characters insert at the caret, and clipboard text with line breaks is today's
    /// outline paste. The system clipboard wins over the register whenever its text is not what
    /// Utopia last put there, so copying in another app and then `p` pastes that.
    fn vim_paste(
        &mut self,
        before: bool,
        count: usize,
        register_char: Option<char>,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        if self.field().composing() {
            return;
        }
        // `"_p` is the black hole: it pastes nothing at all.
        if register_char == Some('_') {
            return;
        }
        let count = count.max(1);
        let clipboard = cx.read_from_clipboard().and_then(|c| c.text());
        let register = last();
        // A cut whose blocks are still tombstones wins over the clipboard: `dd` then `p` is
        // the MOVE that keeps ids (VM-10), and the clipboard cannot carry a linewise flag, so
        // preferring it would silently turn the move into a copy. `"+` still asks for the
        // clipboard by name.
        let pending_cut = matches!(&register, Register::Blocks { cut: Some(_), .. });
        let pasted = match clipboard {
            Some(text) if register_char == Some('+') => Register::Chars(text),
            Some(_) if pending_cut => register,
            Some(text) if !clipboard_is_ours(&text) => Register::Chars(text),
            _ => register,
        };
        match pasted {
            Register::Blocks { tree, .. } if register_char != Some('+') && !tree.is_empty() => {
                self.emit_after_commit(EditorEvent::PasteBlocks { before, count }, cx)
            }
            Register::Blocks { markdown, .. } => {
                // The clipboard copy of a block yank is Markdown, which is all a system paste
                // can carry (the clipboard has no linewise flag).
                self.vim_paste_text(&markdown, before, count, window, cx)
            }
            Register::Chars(text) => self.vim_paste_text(&text, before, count, window, cx),
        }
    }

    /// Insert `text` at the caret (`P`) or just after it (`p`), as one undo step. Text with a
    /// line break goes to the host as an outline paste, which is what Cmd+V does.
    fn vim_paste_text(
        &mut self,
        text: &str,
        before: bool,
        count: usize,
        window: &mut Window,
        cx: &mut Context<BlockEditor>,
    ) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if text.trim_end_matches('\n').contains('\n') {
            return self.emit_after_commit(EditorEvent::PasteOutline(text), cx);
        }
        let line = text.trim_end_matches('\n');
        if line.is_empty() {
            return;
        }
        let repeated = line.repeat(count.max(1));
        let field = self.field();
        let start = if before {
            field.cursor()
        } else {
            next_grapheme(&field.text, field.cursor())
        };
        // Vim leaves the cursor on the last character it pasted; `clamp_normal_cursor`
        // puts it back on a grapheme once the text is in the buffer.
        let after = start + repeated.len();
        self.discrete_edit(start..start, &repeated, Some(after), window, cx);
        self.clamp_normal_cursor(cx);
    }
}

/// `cw` on a word acts like `ce`: Vim changes the word and leaves the blank after it, so
/// what is typed replaces the word and not the gap (2B). On a blank it behaves as `w` does.
fn change_word_as_word_end(text: &str, cursor: usize, op: Operator, motion: Motion) -> Motion {
    match (op, motion) {
        (Operator::Change, Motion::WordForward { big }) => {
            let on_blank = text
                .get(cursor..)
                .and_then(|rest| rest.chars().next())
                .is_some_and(char::is_whitespace);
            if on_blank {
                motion
            } else {
                Motion::WordEnd { big }
            }
        }
        _ => motion,
    }
}
