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

use gpui::{Context, EntityId, Keystroke, Window};
use utopia_core::text::{next_grapheme, prev_grapheme};
use utopia_core::undo::Caret;
use utopia_core::vim::{motion, Command, InsertAt, Key, Motion, Parser, Step};
use utopia_core::{BlockId, BlockKind, PageId};

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
    /// An Insert run is open: every keystroke since `i`/`a`/`o` is one undo step (FR-2.12
    /// in Vim mode). PR 2 turns the flag into the undo group; PR 1 only carries it.
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
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.insert_run = false;
        self.parser.reset();
    }

    /// Leave Insert for Normal (VM-13). Escape, a blur and a click into the block all
    /// leave it the same way, and all of them close the Insert run with it.
    pub fn leave_insert(&mut self) {
        self.insert_run = false;
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

/// One block of a [`Register::Blocks`], with everything `p` needs to write it back.
#[derive(Clone, Debug)]
pub struct TreeRow {
    pub depth: usize,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
}

thread_local! {
    /// The unnamed register. Per thread, which for this app (and for a test) is the
    /// session; like the kill buffer it is memory only (rule 9).
    static REGISTER: RefCell<Register> = const { RefCell::new(Register::Chars(String::new())) };
}

/// Remember a register's contents. PR 2 fills the register; nothing calls this yet.
pub fn remember(register: Register) {
    REGISTER.with(|r| *r.borrow_mut() = register);
}

/// The unnamed register's contents, for `p` and `P`. A copy: the register is the app's,
/// what the caller pastes is its own. PR 2 fills the register.
pub fn last() -> Register {
    REGISTER.with(|r| r.borrow().clone())
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

    /// Run a parsed command. PR 1 understands motions, the four insert entries, undo/redo
    /// and leaving Normal; every other command is a no-op until PR 2 fills it in.
    pub fn vim_run(
        &mut self,
        command: Command,
        _window: &mut Window,
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
            Command::Insert(at) => {
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
                        self.vim_mut().insert_run = true;
                        self.commit(cx);
                        cx.emit(EditorEvent::Split { caret });
                        return;
                    }
                    InsertAt::BlockAbove => {
                        self.vim_mut().mode = Mode::Insert;
                        self.vim_mut().insert_run = true;
                        self.commit(cx);
                        cx.emit(EditorEvent::InsertAbove);
                        return;
                    }
                    // These delete first and are PR 2; the plan keeps them out of PR 1 so no
                    // command in this stage removes a character.
                    InsertAt::ReplaceChar | InsertAt::ReplaceLine | InsertAt::ToLineEnd => return,
                }
                // `i`, `a`, `I` and `A` only moved the caret. The mode change is shared,
                // so none of the four can forget it; every other arm returned (VM-3).
                self.vim_mut().mode = Mode::Insert;
                self.vim_mut().insert_run = true;
                // The menu is Insert's (VM-3, §5): entering Insert where the caret sits
                // inside a trigger opens it, so `i` inside `[[Alp` offers the pages the
                // text already asked for (F3).
                self.refresh_autocomplete(cx);
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
            // PR 2: operators, registers, visual mode and the rest.
            _ => {}
        }
        cx.notify();
    }
}
