//! Vim mode's local state (VM-3, VM-5, VM-13): which of the four states the editor is in,
//! the keys the parser is still waiting for, and where a rest cursor wakes up.
//!
//! None of it is content: it is never put in a CRDT shard, the outbox or a sync message
//! (rule 9), and it is not written anywhere at all — a mode is a property of this session.
//! The grammar itself lives in [`utopia_core::vim`], which has no store and no `gpui`; this
//! module is where the UI keeps the answer between keystrokes.
//!
//! The mode outlives one block. [`crate::editor::BlockEditor::begin`] makes a fresh
//! session for every block and `j`/`k` cross blocks through it, so the state sits beside
//! the session rather than inside it, and only a finished edit (`end`, `detach`) puts it
//! back to Normal.

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

/// The state the editor is in (VM-3). Pending is not a state of its own: the keys a
/// command still needs live in [`VimState::parser`], which is what the mode label reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Motions and commands. A block in edit enters here, not in Insert (VM-4).
    #[default]
    Normal,
    /// Typing goes into the block.
    Insert,
    /// `v` / `V`: motions extend a selection from `anchor`.
    Visual { anchor: usize, line: bool },
}

impl Mode {
    /// The identifier this mode adds to the editor's key context (`VimNormal`,
    /// `VimInsert`, `VimVisual`), so a binding can answer in one mode only.
    pub fn context(self) -> &'static str {
        match self {
            Self::Normal => VIM_NORMAL_CONTEXT,
            Self::Insert => VIM_INSERT_CONTEXT,
            Self::Visual { .. } => VIM_VISUAL_CONTEXT,
        }
    }
}

/// What the last change was, for `.` (PR 3). PR 1 keeps the field so the state machine is
/// complete; nothing writes it and nothing reads it yet.
#[derive(Clone, Debug)]
pub struct Change;

/// Where a rest cursor wakes up: the caret, and the view that hosted the edit. One
/// [`BlockEditor`] serves the main pane and the sidebar's pages, so the view is part of
/// the resume point — waking a sidebar row in the main pane would put the caret on a
/// different page, or on none (VM-13, F5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    /// The [`crate::page_view::PageView`] that hosted the edit (`EditTarget::host`).
    pub host: EntityId,
    pub caret: Caret,
}

/// Everything Vim mode remembers, one per [`crate::editor::BlockEditor`].
#[derive(Debug)]
pub struct VimState {
    pub mode: Mode,
    /// The pending command. It is fed one stroke at a time and reset by every completed
    /// command and every Escape, so a half-typed `2d` cannot leak into the next block.
    pub parser: Parser,
    /// Where the caret was in the block the edit ended in, kept so the key that wakes the
    /// cursor (VM-13) reopens the same row at the same offset.
    pub resume: Option<Resume>,
    /// What `.` repeats (PR 3); `None` until there is a change to repeat.
    pub last_change: Option<Change>,
    /// An Insert run is open: every keystroke since `i`/`a`/`o` is one undo step (VM-12;
    /// PRD §9.1.1, the FR-2.12 note). The flag is the editor's bookkeeping for the undo
    /// stack's run: it is opened by [`crate::editor::BlockEditor::begin_insert_run`] and
    /// closed — which is what folds the stack's groups — by `close_insert_run`, never
    /// here, so the flag and the stack cannot drift apart.
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
    /// Back to Normal with nothing pending: the state a finished edit leaves behind. Not
    /// called by `begin`, which is how `j` and `k` carry the mode into the next block.
    ///
    /// The Insert run is not closed here: its undo groups belong to the store, so the
    /// editor closes it through [`crate::editor::BlockEditor::close_insert_run`] and the
    /// stack sees one call for each open.
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.parser.reset();
    }

    /// Leave Insert for Normal (VM-13). Escape, a blur and a click into the block all
    /// leave it the same way, and all of them close the Insert run with it — through the
    /// editor, which owns the store call (`close_insert_run`, VM-12).
    pub fn leave_insert(&mut self) {
        if self.mode == Mode::Insert {
            self.mode = Mode::Normal;
        }
    }
}

/// One register (VM-10, VM-11): a run of characters, or whole blocks with the tree they
/// were cut from, so `p` puts the same ids back and every `((reference))` still resolves.
#[derive(Clone, Debug)]
pub enum Register {
    Chars(String),
    Blocks {
        /// The blocks as Markdown, for the system clipboard (VM-11).
        markdown: String,
        /// The rows to write back, outermost first.
        tree: Vec<TreeRow>,
        /// The page and blocks a cut took them from: `p` moves rather than copies when the
        /// blocks are still tombstones waiting to be restored.
        cut: Option<(PageId, Vec<BlockId>)>,
    },
}

/// One block of a [`Register::Blocks`], with everything `p` needs to write it back: the depth
/// relative to the yank's first row, the text, the kind and the done state.
pub use utopia_core::outline::TreeRow;

thread_local! {
    /// The unnamed register. Per thread, which for this app (and for a test) is the
    /// session; like the kill buffer it is memory only (rule 9).
    static REGISTER: RefCell<Register> = const { RefCell::new(Register::Chars(String::new())) };
    /// What Utopia itself last put on the system clipboard (VM-11). `p` uses the clipboard
    /// only when its text differs from this, so a block yank survives the round trip as a
    /// structured register instead of coming back as Markdown.
    static CLIPBOARD: RefCell<Option<String>> = const { RefCell::new(None) };
    /// What the system clipboard held at the moment the register was written — by a delete
    /// or a change, which do not write the clipboard at all. `p` prefers the register while
    /// the clipboard still reads this, so `x` then `p` pastes what `x` took and not whatever
    /// another app had copied before it (F2). Once the clipboard changes it is newer than
    /// the register, and it wins (F6) — there is no cut here to outlive it.
    static CLIPBOARD_AT_REGISTER: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Remember a register's contents.
pub fn remember(register: Register) {
    REGISTER.with(|r| *r.borrow_mut() = register);
}

/// The unnamed register's contents, for `p` and `P`. A copy: the register is the app's,
/// what the caller pastes is its own.
pub fn last() -> Register {
    REGISTER.with(|r| r.borrow().clone())
}

/// Record what Utopia put on the system clipboard, so `p` can tell its own yank from
/// something the user copied elsewhere (VM-11).
pub fn clipboard_written(text: &str) {
    CLIPBOARD.with(|c| *c.borrow_mut() = Some(text.to_string()));
    // What the register was written against is now this text, so the write is not mistaken
    // for someone else's copy either.
    CLIPBOARD_AT_REGISTER.with(|c| *c.borrow_mut() = Some(text.to_string()));
}

/// Note the system clipboard as it stands while a register is written by a delete or a
/// change, which leave the clipboard alone.
pub fn clipboard_at_register(text: Option<String>) {
    CLIPBOARD_AT_REGISTER.with(|c| *c.borrow_mut() = text);
}

/// Whether `text` is what Utopia itself last wrote to the system clipboard (VM-11).
pub fn clipboard_is_ours(text: &str) -> bool {
    CLIPBOARD.with(|c| c.borrow().as_deref() == Some(text))
}

/// Whether the system clipboard has changed since the register was written: something else
/// copied into it, so it is newer than the register and `p` pastes it.
pub fn clipboard_is_newer(text: &str) -> bool {
    !clipboard_is_ours(text) && !CLIPBOARD_AT_REGISTER.with(|c| c.borrow().as_deref() == Some(text))
}

/// Drop the `cut` half of the register (VM-10): the first `p` restored and moved the blocks,
/// so a second one copies them, as Vim's second `p` does.
pub fn forget_cut() {
    REGISTER.with(|r| {
        if let Register::Blocks { cut, .. } = &mut *r.borrow_mut() {
            *cut = None;
        }
    });
}

/// The [`Key`] a platform keystroke is for the grammar (Task 1D.1), or `None` when the
/// keymap must see it unchanged: Tab/Shift-Tab, every Cmd chord, and an Alt chord without a
/// character.
///
/// The order matters: classify named keys by `key` first, because the test platform gives
/// Enter, Tab and Space a `key_char`, and a printable character second, because `$`, `^`
/// and `G` arrive as Shift or Option characters on layouts that need them. A claimed key the
/// parser rejects is swallowed by `vim_key` (VM-5).
pub fn key_from_keystroke(keystroke: &Keystroke) -> Option<Key> {
    let modifiers = &keystroke.modifiers;
    // A Cmd chord belongs to the keymap, never to the grammar.
    if modifiers.platform {
        return None;
    }
    // Tab and Shift-Tab indent or outdent. They are named keys with a `key_char` on the
    // test platform, so this check must come before the character branch.
    if keystroke.key == "tab" || keystroke.key == "backtab" {
        return None;
    }
    // Named keys, with no Cmd, Alt or Fn. Shift is left through: Shift+arrow is still a
    // motion in Normal (the selection is Visual's job, PR 3).
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
    // Ctrl+letter. On macOS claim every one: there `cmd-` is the command modifier, and an
    // unclaimed Ctrl key would type its control character into the block. Elsewhere the
    // Cmd defaults are spelled Ctrl, so only the keys the grammar knows may be claimed.
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
    // Anything printable, including Option chords that produce a character.
    if let Some(text) = &keystroke.key_char {
        if let Some(c) = text.chars().next() {
            return Some(Key::Char(c));
        }
    }
    None
}

/// One application of `motion` to `text`. Down/Up cross blocks and are handled by the
/// caller, and so are the finds, which take their count in one step ([`find_motion`]).
fn motion_once(text: &str, offset: usize, motion: Motion) -> usize {
    // The cursor never rests on a line break (1E.3), but a caller may hand us an arbitrary
    // offset; snapping here keeps every path inside the block (rule 11).
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
        // Down/Up and PageFirst/PageLast are handled before this function is reached, and
        // the finds through `find_motion`.
        Motion::Down
        | Motion::Up
        | Motion::PageFirst
        | Motion::PageLast
        | Motion::Find { .. }
        | Motion::RepeatFind { .. } => offset,
    }
}

/// A find motion with its whole count (`f` `t` `F` `T` and `;` `,`), or `None` when the
/// line has no such target: the cursor stays where it is.
///
/// The count is passed to the search instead of looping a one-target search: counting the
/// targets first is what lets `2tx` and `tx;` pass the first `x` (F6).
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
    /// A click into a rendered block starts in Normal with nothing pending (VM-4, F2).
    ///
    /// [`BlockEditor::begin`] deliberately keeps the mode, because `j`/`k` and the
    /// structural commands cross blocks with it (`o`, `O` must stay in Insert). A click is
    /// not a command stepping through blocks, so the view that answers the click says so
    /// before it begins: both rows share one focus handle, so no focus-loss reset fires
    /// between them.
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

    /// Feed one claimed keystroke to the parser and act on its answer (Task 1D.2). Nothing
    /// here runs unless the workspace's interceptor already decided the key is ours.
    pub fn vim_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<BlockEditor>) {
        let step = self.vim_mut().parser.feed(key);
        match step {
            Step::Pending(_) => {
                // The parser keeps the label; the mode indicator redraws from it.
                cx.emit(EditorEvent::VimChanged);
                cx.notify();
            }
            Step::Cancel => {
                // The parser already reset itself. No text is touched (VM-5).
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

    /// Run a parsed command. Motions and the insert entries are PR 1; PR 2 adds the
    /// charwise operators (`x X D C s S ~ r`, `d`/`c`/`y` with a motion or a text object)
    /// and the undo runs that make a `c` command and its typing one step. The linewise
    /// block commands (`dd`, `yy`, `p`, `J`, `>>`, `<<`) are PR 2C.
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
                                    // The block cursor rests on a grapheme, never past the
                                    // last one of its line (1E.3, F4): a shorter wrapped
                                    // line would otherwise leave it after its last
                                    // character, where `i` would insert behind it.
                                    let to = motion::clamp_normal(&self.field().text, ix);
                                    self.field_mut().move_to(to);
                                    self.field_mut().reset_caret_motion();
                                }
                                None => {
                                    // The block is out of wrapped lines; the host moves the
                                    // rest of the count in whole blocks.
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
                            // A find that finds nothing does not move at all.
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
            Command::Insert { at, register } => {
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
                        // Enter Insert before emitting: the host begins the new block right
                        // away, and `begin` keeps the mode it finds.
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
                    // `s`: the grapheme under the cursor goes first, in this same run. No
                    // clamp: the bar that follows may sit past a line's last grapheme.
                    InsertAt::ReplaceChar => {
                        self.vim_delete_chars(false, 1, register, false, window, cx)
                    }
                    // `S`: the block's text, which is `cc`. Its children are other blocks
                    // and stay (VM-8, PR 2B).
                    InsertAt::ReplaceLine => {
                        let len = self.field().text.len();
                        self.vim_delete(0..len, register, false, false, window, cx);
                    }
                    // `C`: to the end of the line, as `d$` does.
                    InsertAt::ToLineEnd => {
                        self.vim_delete_to_line_end(register, 1, false, window, cx)
                    }
                }
                // The delete is done, so Insert starts here — after it, inside the run the
                // first line of this arm opened (VM-12, 2B).
                self.vim_mut().mode = Mode::Insert;
                // The menu is Insert's (VM-3, §5): entering Insert where the caret sits
                // inside a trigger opens it, so `i` inside `[[Alp` offers the pages the
                // text already asked for (F3), and `ci[` inside `[[Alpha]]` offers the
                // link's (PR 2B).
                self.refresh_autocomplete(cx);
            }
            Command::DeleteChar {
                back,
                count,
                register,
            } => self.vim_delete_chars(back, count, register, true, window, cx),
            Command::DeleteToLineEnd { count, register } => {
                self.vim_delete_to_line_end(register, count, true, window, cx)
            }
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
                // Keep where the cursor was so a Normal key in Rest wakes this row and
                // offset (VM-13). `end` resets the mode but not the resume point, and the
                // view that hosted it is kept with it: the same editor serves the
                // sidebar's pages too (F5).
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
            // PR 2C handles `dd`, `yy`, `p`, `J`, `>>` and `<<` above; the rest is PR 3
            // (visual mode and `.`).
            _ => {}
        }
        cx.notify();
    }

    // ------------------------------------------------------- charwise operators (2B)

    /// Write a register (VM-11). A yank (`clipboard`) and `"+` also put the text on the
    /// system clipboard; `d`, `c` and `x` write the register only; `"_` discards.
    ///
    /// A delete leaves the clipboard alone, so the clipboard as it stands now is recorded
    /// with the register: `p` pastes the register while the clipboard still reads that, and
    /// the clipboard once it changed (F2, F6).
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
        } else {
            let current = cx.read_from_clipboard().and_then(|c| c.text());
            clipboard_at_register(current);
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
        register: Option<char>,
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
        self.vim_delete(range, register, false, clamp, window, cx);
    }

    /// `D` and `C`'s delete: the caret to the end of the line, exclusive of its break. A
    /// count reaches the end of that many lines (`3D`), as Vim's does.
    fn vim_delete_to_line_end(
        &mut self,
        register: Option<char>,
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
        let mut end = motion::line_end_exclusive(&text, cursor);
        for _ in 1..count.max(1) {
            if end >= text.len() {
                break;
            }
            // Over the break, then on to the end of the line it opens.
            end = motion::line_end_exclusive(&text, next_grapheme(&text, end));
        }
        self.vim_delete(cursor..end, register, false, clamp, window, cx);
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
            // `3cc` is the same shape and the same limit: changing three blocks means
            // deleting the text of three rows and opening Insert over them, which is a
            // multi-block plan PR 4 owns, so the count changes this block's text only. It is
            // in the PR's known limits (F9).
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
                // `cw` on a word takes the word under the cursor without the first step of
                // `e`, which would run on into the next word from a word's last character
                // (F5); on a blank it is `w`, which `motion_range` already has.
                let range = match (op, motion) {
                    (Operator::Change, Motion::WordForward { big }) => {
                        operator::change_word_range(&text, cursor, big, count).or_else(|| {
                            operator::motion_range(
                                &text,
                                cursor,
                                motion,
                                count,
                                self.vim().parser.last_find(),
                            )
                        })
                    }
                    _ => operator::motion_range(
                        &text,
                        cursor,
                        motion,
                        count,
                        self.vim().parser.last_find(),
                    ),
                };
                match range {
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
    /// outline paste. The system clipboard wins over the register only when it changed after
    /// the register was written — something the user copied elsewhere (F2, F6). `"+` asks for
    /// it by name.
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
        let pasted = match clipboard {
            Some(text) if register_char == Some('+') => Register::Chars(text),
            // A cut whose blocks are still tombstones wins over an unchanged clipboard:
            // `dd` then `p` is the MOVE that keeps ids (VM-10), and the clipboard cannot
            // carry a linewise flag, so preferring it would silently turn the move into a
            // copy. A clipboard that changed since is newer than the register, so it wins.
            Some(text) if clipboard_is_newer(&text) => Register::Chars(text),
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
