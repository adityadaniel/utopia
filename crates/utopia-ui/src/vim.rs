use std::cell::RefCell;

use gpui::{Context, EntityId, Keystroke, Window};
use utopia_core::text::{next_grapheme, prev_grapheme};
use utopia_core::undo::Caret;
use utopia_core::vim::{motion, Command, InsertAt, Key, Motion, Parser, Step};
use utopia_core::{BlockId, BlockKind, PageId};

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
        self.insert_run = false;
        self.parser.reset();
    }

    pub fn leave_insert(&mut self) {
        self.insert_run = false;
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

#[derive(Clone, Debug)]
pub struct TreeRow {
    pub depth: usize,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
}

thread_local! {
    static REGISTER: RefCell<Register> = const { RefCell::new(Register::Chars(String::new())) };
}

pub fn remember(register: Register) {
    REGISTER.with(|r| *r.borrow_mut() = register);
}

pub fn last() -> Register {
    REGISTER.with(|r| r.borrow().clone())
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
                    InsertAt::ReplaceChar | InsertAt::ReplaceLine | InsertAt::ToLineEnd => return,
                }
                self.vim_mut().mode = Mode::Insert;
                self.vim_mut().insert_run = true;
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
}
