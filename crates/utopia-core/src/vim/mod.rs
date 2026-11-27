pub mod motion;
pub mod parse;
pub mod textobj;

pub use parse::Parser;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
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
    Ctrl(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Down,
    Up,
    WordForward { big: bool },
    WordBack { big: bool },
    WordEnd { big: bool },
    LineStart,
    LineFirstNonBlank,
    LineEnd,
    Find { ch: char, till: bool, back: bool },
    RepeatFind { reverse: bool },
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
    Word { big: bool, around: bool },
    Bracket { open: char, around: bool },
    Quote { ch: char, around: bool },
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Pending(String),
    Run(Command),
    Cancel,
    Pass,
}

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

    fn keys_of(row: &str) -> Vec<Key> {
        let row = row.replace("{char}", ",");
        if let Some(rest) = row.strip_prefix("Ctrl-") {
            return rest.chars().map(Key::Ctrl).collect();
        }
        row.chars().map(Key::Char).collect()
    }

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

    #[test]
    fn reference_keys_are_unique() {
        for (i, (keys, _)) in REFERENCE.iter().enumerate() {
            for (other, _) in &REFERENCE[i + 1..] {
                assert_ne!(keys, other, "{keys} is listed twice");
            }
        }
    }

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
