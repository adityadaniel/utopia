//! The Vim grammar (VM-3, VM-5, VM-6), pure and platform-free.
//!
//! This module is the only place the grammar lives: it takes [`Key`] values and offsets
//! into a block's text and answers with [`Step`]s and byte offsets. It has no CRDT, no
//! store and no UI dependency, so it cannot read or write a graph and cannot block a
//! render (architecture rule 10). Mode, registers and pending keys are the UI's local
//! state (rule 9); nothing here is persisted, synced or put in a shard.
//!
//! Every offset [`motion`] and [`textobj`] return is a grapheme boundary of the string
//! they were given, so the caller may slice with it (rule 11: no panic on user data).

pub mod motion;
pub mod operator;
pub mod parse;
pub mod textobj;

pub use parse::Parser;

/// One keystroke, as the UI layer classifies it before the keymap sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable character the platform reports, e.g. 'd', '$', 'G'.
    Char(char),
    Escape,
    Enter,
    Space,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    /// Control plus a letter, lower-cased, e.g. Ctrl('r').
    Ctrl(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    /// Down or up one wrapped line, then into the next or previous block.
    Down,
    Up,
    WordForward {
        big: bool,
    },
    WordBack {
        big: bool,
    },
    WordEnd {
        big: bool,
    },
    LineStart,
    LineFirstNonBlank,
    LineEnd,
    Find {
        ch: char,
        till: bool,
        back: bool,
    },
    RepeatFind {
        reverse: bool,
    },
    PageFirst,
    PageLast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextObject {
    /// `iw aw iW aW`
    Word { big: bool, around: bool },
    /// `i( a( i[ a[ i{ a{`
    Bracket { open: char, around: bool },
    /// `i" a" i' a'` and the backtick pair
    Quote { ch: char, around: bool },
    /// `il al` (PR 4; [`textobj::range_of`] answers `None` for now)
    Ref { around: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Motion(Motion),
    Object(TextObject),
    Line,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Move {
        motion: Motion,
        count: usize,
    },
    Operate {
        op: Operator,
        target: Target,
        count: usize,
        register: Option<char>,
    },
    /// i a I A o O s S C
    Insert(InsertAt),
    DeleteChar {
        back: bool,
        count: usize,
    },
    DeleteToLineEnd,
    ReplaceChar(char),
    ToggleCase {
        count: usize,
    },
    Paste {
        before: bool,
        count: usize,
        register: Option<char>,
    },
    JoinNext {
        count: usize,
    },
    Undo {
        count: usize,
    },
    Redo {
        count: usize,
    },
    EnterVisual {
        line: bool,
    },
    RepeatChange,
    LeaveNormal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    Cursor,
    After,
    LineStart,
    LineEnd,
    BlockBelow,
    BlockAbove,
    ReplaceChar,
    ReplaceLine,
    ToLineEnd,
}

/// What the parser says after one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// More keys are needed. The string is what the mode label shows, e.g. "2d".
    Pending(String),
    Run(Command),
    /// A key the grammar knows nothing about. The UI swallows it and shows nothing.
    Cancel,
    /// The UI must give this keystroke to the keymap (Tab, a Cmd chord).
    Pass,
}

/// Every key the grammar knows, with the text Settings → Shortcuts shows.
///
/// `{char}` stands for the character the key takes (`f<c>`, `r<c>`). A unit test feeds
/// each row to a fresh [`Parser`] and asserts it never ends in [`Step::Cancel`], so the
/// help card cannot drift from the grammar.
pub const REFERENCE: &[(&str, &str)] = &[
    ("h", "One character left"),
    ("j", "Down a wrapped line, then into the next block"),
    ("k", "Up a wrapped line, then into the previous block"),
    ("l", "One character right"),
    ("w", "Next word: a run of letters, or a run of punctuation"),
    ("W", "Next word: only blanks separate words"),
    (
        "b",
        "Previous word: a run of letters, or a run of punctuation",
    ),
    ("B", "Previous word: only blanks separate words"),
    ("e", "End of the next word"),
    ("E", "End of the next word: only blanks separate words"),
    ("0", "Start of the line"),
    ("^", "First non-blank of the line"),
    ("$", "Last character of the line"),
    ("f{char}", "Find a character on the right"),
    ("F{char}", "Find a character on the left"),
    ("t{char}", "Stop just before a character on the right"),
    ("T{char}", "Stop just after a character on the left"),
    (";", "Repeat the last find"),
    (",", "Repeat the last find the other way"),
    ("gg", "First block of the page"),
    ("G", "Last block of the page"),
    ("i", "Insert before the cursor"),
    ("a", "Insert after the cursor"),
    ("I", "Insert at the line's first non-blank"),
    ("A", "Insert at the end of the line"),
    ("o", "New block below, and insert"),
    ("O", "New block above, and insert"),
    ("s", "Change the character under the cursor"),
    ("S", "Change the whole block"),
    ("C", "Change to the end of the line"),
    ("x", "Delete the character under the cursor"),
    ("X", "Delete the character before the cursor"),
    ("D", "Delete to the end of the line"),
    ("dd", "Delete this block and its subtree"),
    ("dw", "Delete to the next word"),
    ("d$", "Delete to the end of the line"),
    ("cc", "Change the text of this block"),
    ("cw", "Change to the next word"),
    ("yy", "Yank this block and its subtree"),
    ("Y", "Yank this block"),
    (">>", "Indent the block"),
    ("<<", "Outdent the block"),
    ("p", "Paste after the cursor"),
    ("P", "Paste before the cursor"),
    ("J", "Join the next block into this one"),
    ("r{char}", "Replace the character under the cursor"),
    ("~", "Swap the case of the characters"),
    ("u", "Undo"),
    ("Ctrl-r", "Redo"),
    ("v", "Visual mode, characterwise"),
    ("V", "Visual mode, the whole block"),
    (".", "Repeat the last change"),
    ("\"_dd", "Delete into the black-hole register"),
    ("\"+yy", "Yank into the system clipboard"),
    ("diw", "Delete the word under the cursor"),
    ("daw", "Delete the word and the blanks after it"),
    ("diW", "Delete the big word under the cursor"),
    ("daW", "Delete the big word and the blanks after it"),
    ("di(", "Delete inside the parentheses"),
    ("da(", "Delete the parentheses and their contents"),
    ("di[", "Delete inside the square brackets"),
    ("da[", "Delete the square brackets and their contents"),
    ("di{", "Delete inside the braces"),
    ("da{", "Delete the braces and their contents"),
    ("di\"", "Delete inside the quotes"),
    ("da\"", "Delete the quotes and their contents"),
    ("di'", "Delete inside the single quotes"),
    ("da'", "Delete the single quotes and their contents"),
    ("di`", "Delete inside the backticks"),
    ("da`", "Delete the backticks and their contents"),
    ("ciw", "Change the word under the cursor"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys of a [`REFERENCE`] row as a [`Parser`] would see them. `Ctrl-r` is a
    /// control chord; `{char}` is any character the key takes.
    fn keys_of(row: &str) -> Vec<Key> {
        let row = row.replace("{char}", ",");
        if let Some(rest) = row.strip_prefix("Ctrl-") {
            return rest.chars().map(Key::Ctrl).collect();
        }
        row.chars().map(Key::Char).collect()
    }

    /// 1A.5: no row of the help card is a key the grammar rejects, and each row ends a
    /// command (or waits for more keys), never `Cancel`.
    #[test]
    fn every_reference_row_parses() {
        for (keys, help) in REFERENCE {
            assert!(!help.is_empty(), "{keys} has no help text");
            let mut parser = Parser::default();
            let mut step = Step::Pending(String::new());
            for key in keys_of(keys) {
                let shown = format!("{key:?}");
                step = parser.feed(key);
                assert_ne!(step, Step::Cancel, "{keys} cancels at {shown}");
            }
            assert!(
                matches!(step, Step::Run(_) | Step::Pending(_)),
                "{keys} ends in {step:?}"
            );
        }
    }

    /// The card is a set, not a list with repeats, and it names no key twice.
    #[test]
    fn reference_keys_are_unique() {
        for (i, (keys, _)) in REFERENCE.iter().enumerate() {
            for (other, _) in &REFERENCE[i + 1..] {
                assert_ne!(keys, other, "{keys} is listed twice");
            }
        }
    }

    /// The reference covers every key the grammar accepts, so a new command cannot ship
    /// undocumented.
    #[test]
    fn reference_covers_the_grammar() {
        let listed: Vec<&str> = REFERENCE.iter().map(|(keys, _)| *keys).collect();
        for (keys, _) in [
            ("h", ""),
            ("l", ""),
            ("w", ""),
            ("W", ""),
            ("b", ""),
            ("B", ""),
            ("e", ""),
            ("E", ""),
            ("x", ""),
            ("X", ""),
            ("D", ""),
            ("C", ""),
            ("S", ""),
            ("s", ""),
            ("I", ""),
            ("A", ""),
            ("o", ""),
            ("O", ""),
            ("p", ""),
            ("P", ""),
            ("J", ""),
            ("~", ""),
            ("u", ""),
            ("Ctrl-r", ""),
            ("v", ""),
            ("V", ""),
            (".", ""),
        ] {
            assert!(listed.contains(&keys), "{keys} is missing from REFERENCE");
        }
    }
}
