//! The Vim grammar as a state machine (VM-3, VM-5): one [`Key`] in, one [`Step`] out.
//!
//! The parser holds only pending keys — a count, a register, an operator, the `g` prefix
//! and the argument a key is waiting for. It reads no store and allocates nothing beyond
//! the label it hands back for the mode indicator (rule 10). The caller drops it with
//! [`Parser::reset`] after a completed command and after this module's own cancel.

use super::{Command, InsertAt, Key, Motion, Operator, Step, Target, TextObject};

/// The largest count a command may carry. Vim's own counts are effectively unbounded, but
/// every consumer here turns a count into a loop of real work: `99u` is 99 undos, each one
/// a commit and a page rebuild. Saturating at `usize::MAX` therefore made a typed digit run
/// a hang with no way out, which rule 11 does not allow on user input. A count above this
/// clamps to it, so a long digit run is slow at worst and never unbounded.
pub const MAX_COUNT: usize = 10_000;

/// A key the parser is waiting for before it can finish a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Await {
    /// `"` — the register name.
    Register,
    /// `f` `t` `F` `T` — the character to search for.
    Find { till: bool, back: bool },
    /// `r` — the character to write.
    Replace,
    /// `i` or `a` after an operator — the text object.
    Object { around: bool },
}

#[derive(Default, Debug)]
pub struct Parser {
    /// The count typed before the operator, if any.
    count: Option<usize>,
    /// The count typed after the operator, if any.
    count2: Option<usize>,
    register: Option<char>,
    op: Option<Operator>,
    /// The pending prefix key; only `g` has one.
    prefix: Option<char>,
    awaiting: Option<Await>,
    /// What has been typed so far, for the mode label.
    label: String,
    /// The last `f` `t` `F` `T`, for `;` and `,`. Survives [`Parser::reset`].
    last_find: Option<(char, bool, bool)>,
}

impl Parser {
    pub fn feed(&mut self, key: Key) -> Step {
        // 1. Escape drops everything pending; with nothing pending it leaves Normal (VM-13).
        if key == Key::Escape {
            let was_pending = self.is_pending();
            self.reset();
            return if was_pending {
                // The cancel is silent: the label clears, no command runs.
                Step::Pending(String::new())
            } else {
                Step::Run(Command::LeaveNormal)
            };
        }

        // `Key` is not `Copy`, and every branch below reads it, so take its character
        // once.
        let ch = match &key {
            Key::Char(c) => Some(*c),
            _ => None,
        };
        let ctrl = match &key {
            Key::Ctrl(c) => Some(*c),
            _ => None,
        };

        // 2. A key the parser is waiting for takes the next key as an argument. It comes
        //    before the digits so that `f2` finds a `2` and does not start a count.
        if let Some(awaiting) = self.awaiting.take() {
            return self.awaited(awaiting, key);
        }

        // 3. `g` is a prefix; `gg` is the only second key it has until PR 4.
        if let Some(prefix) = self.prefix.take() {
            if prefix == 'g' && key == Key::Char('g') {
                let count = self.take_count();
                let motion = Motion::PageFirst;
                return self.finish(Step::Run(Command::Move { motion, count }));
            }
            return self.cancel();
        }

        // 4. Digits build the count. A leading `0` is the LineStart motion, never a count.
        if let Some(c) = ch {
            if c.is_ascii_digit() && !(c == '0' && self.count_slot().is_none()) {
                let digit = c.to_digit(10).unwrap_or(0) as usize;
                let slot = self.count_slot();
                *slot = Some(
                    slot.unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit)
                        .min(MAX_COUNT),
                );
                self.push_label(c);
                return Step::Pending(self.pending());
            }
        }

        // 5. A key that takes a character argument: `f` `t` `F` `T` (also after an
        //    operator, as in `df,`), `r`, and the register prefix `"`.
        if let Some(c) = ch {
            if let Some((till, back)) = find_kind(c) {
                self.awaiting = Some(Await::Find { till, back });
                self.push_label(c);
                return Step::Pending(self.pending());
            }
            // `r` is a command of its own only without an operator; `dr` is not a thing.
            if c == 'r' && self.op.is_none() {
                self.awaiting = Some(Await::Replace);
                self.push_label(c);
                return Step::Pending(self.pending());
            }
            // A register comes before the operator (`"_dd`), never after (`d"x`).
            if c == '"' && self.op.is_none() {
                self.awaiting = Some(Await::Register);
                self.push_label(c);
                return Step::Pending(self.pending());
            }
            if c == 'g' && self.op.is_none() {
                self.prefix = Some('g');
                self.push_label(c);
                return Step::Pending(self.pending());
            }
        }

        // 6. An operator waits for a target.
        if let Some(op) = self.op {
            if let Some(c) = ch {
                if operator_of(c) == Some(op) {
                    let count = self.take_op_count();
                    let register = self.register.take();
                    let target = Target::Line;
                    return self.finish(Step::Run(Command::Operate {
                        op,
                        target,
                        count,
                        register,
                    }));
                }
                if c == 'i' || c == 'a' {
                    self.awaiting = Some(Await::Object { around: c == 'a' });
                    self.push_label(c);
                    return Step::Pending(self.pending());
                }
            }
            if let Some(motion) = motion_of(&key) {
                let count = self.take_op_count();
                let register = self.register.take();
                let target = Target::Motion(motion);
                return self.finish(Step::Run(Command::Operate {
                    op,
                    target,
                    count,
                    register,
                }));
            }
            return self.cancel();
        }

        // 7. With no operator pending, an operator key starts one.
        if let Some(c) = ch {
            if let Some(op) = operator_of(c) {
                self.op = Some(op);
                self.push_label(c);
                return Step::Pending(self.pending());
            }
        }

        // 8. A motion is a Move.
        if let Some(motion) = motion_of(&key) {
            let count = self.take_count();
            return self.finish(Step::Run(Command::Move { motion, count }));
        }

        // 9. Everything else the grammar knows maps to its own command.
        //
        // The Delete key deletes the grapheme under the cursor, as `x` does (VM-5 keeps
        // every key that is not a command out of the block's text).
        if key == Key::Delete {
            let count = self.take_count();
            let register = self.register.take();
            let step = Command::DeleteChar {
                back: false,
                count,
                register,
            };
            return self.finish(Step::Run(step));
        }
        if let Some(c) = ctrl {
            let count = self.take_count();
            let command = match c {
                'r' => Command::Redo { count },
                _ => return self.cancel(),
            };
            return self.finish(Step::Run(command));
        }
        let Some(c) = ch else {
            return self.cancel();
        };
        let count = self.take_count();
        let register = self.register.take();
        let command = match c {
            'i' => Command::Insert {
                at: InsertAt::Cursor,
                register,
            },
            'a' => Command::Insert {
                at: InsertAt::After,
                register,
            },
            'I' => Command::Insert {
                at: InsertAt::LineStart,
                register,
            },
            'A' => Command::Insert {
                at: InsertAt::LineEnd,
                register,
            },
            'o' => Command::Insert {
                at: InsertAt::BlockBelow,
                register,
            },
            'O' => Command::Insert {
                at: InsertAt::BlockAbove,
                register,
            },
            's' => Command::Insert {
                at: InsertAt::ReplaceChar,
                register,
            },
            'S' => Command::Insert {
                at: InsertAt::ReplaceLine,
                register,
            },
            'C' => Command::Insert {
                at: InsertAt::ToLineEnd,
                register,
            },
            'x' => Command::DeleteChar {
                back: false,
                count,
                register,
            },
            'X' => Command::DeleteChar {
                back: true,
                count,
                register,
            },
            'D' => Command::DeleteToLineEnd { count, register },
            'Y' => Command::Operate {
                op: Operator::Yank,
                target: Target::Line,
                count,
                register,
            },
            '~' => Command::ToggleCase { count },
            'p' => Command::Paste {
                before: false,
                count,
                register,
            },
            'P' => Command::Paste {
                before: true,
                count,
                register,
            },
            'J' => Command::JoinNext { count },
            'u' => Command::Undo { count },
            'v' => Command::EnterVisual { line: false },
            'V' => Command::EnterVisual { line: true },
            '.' => Command::RepeatChange,
            // 10. Anything else is not part of the grammar.
            _ => return self.cancel(),
        };
        self.finish(Step::Run(command))
    }

    /// Drop every pending key. Escape and a completed command call this.
    pub fn reset(&mut self) {
        self.count = None;
        self.count2 = None;
        self.register = None;
        self.op = None;
        self.prefix = None;
        self.awaiting = None;
        self.label.clear();
    }

    /// What the mode label shows after the mode name; "" when nothing is pending.
    pub fn pending(&self) -> String {
        self.label.clone()
    }

    /// The last `f`/`t`/`F`/`T` target, for `;` and `,`.
    pub fn last_find(&self) -> Option<(char, bool, bool)> {
        self.last_find
    }

    /// The key the parser is waiting for, if any.
    fn awaited(&mut self, awaiting: Await, key: Key) -> Step {
        match awaiting {
            Await::Register => {
                if let Key::Char(c @ ('_' | '+')) = key {
                    self.register = Some(c);
                    self.push_label(c);
                    return Step::Pending(self.pending());
                }
                self.cancel()
            }
            Await::Find { till, back } => {
                let Some(ch) = char_argument(key) else {
                    return self.cancel();
                };
                self.last_find = Some((ch, till, back));
                self.push_label(ch);
                let motion = Motion::Find { ch, till, back };
                match self.op.take() {
                    Some(op) => {
                        let count = self.take_op_count();
                        let register = self.register.take();
                        let target = Target::Motion(motion);
                        self.finish(Step::Run(Command::Operate {
                            op,
                            target,
                            count,
                            register,
                        }))
                    }
                    None => {
                        let count = self.take_count();
                        self.finish(Step::Run(Command::Move { motion, count }))
                    }
                }
            }
            Await::Replace => {
                let Some(ch) = char_argument(key) else {
                    return self.cancel();
                };
                self.finish(Step::Run(Command::ReplaceChar(ch)))
            }
            Await::Object { around } => {
                let Some(object) = (match key {
                    Key::Char(c) => text_object_of(c, around),
                    _ => None,
                }) else {
                    return self.cancel();
                };
                let Some(op) = self.op.take() else {
                    return self.cancel();
                };
                self.push_label_object(&object);
                let count = self.take_op_count();
                let register = self.register.take();
                let target = Target::Object(object);
                self.finish(Step::Run(Command::Operate {
                    op,
                    target,
                    count,
                    register,
                }))
            }
        }
    }

    fn is_pending(&self) -> bool {
        self.count.is_some()
            || self.count2.is_some()
            || self.register.is_some()
            || self.op.is_some()
            || self.prefix.is_some()
            || self.awaiting.is_some()
    }

    /// The count slot the next digit belongs in: before the operator, or after it.
    fn count_slot(&mut self) -> &mut Option<usize> {
        if self.op.is_some() {
            &mut self.count2
        } else {
            &mut self.count
        }
    }

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    /// `2d3w` runs the operator over six words, so the two counts multiply. The product
    /// clamps like each digit run does: `10000d10000w` is `MAX_COUNT` words, not a
    /// hundred million (rule 11, F9).
    fn take_op_count(&mut self) -> usize {
        let before = self.count.take().unwrap_or(1);
        let after = self.count2.take().unwrap_or(1);
        before.saturating_mul(after).min(MAX_COUNT)
    }

    fn push_label(&mut self, c: char) {
        self.label.push(c);
    }

    fn push_label_object(&mut self, object: &TextObject) {
        let c = match object {
            TextObject::Word { big: true, .. } => 'W',
            TextObject::Word { big: false, .. } => 'w',
            TextObject::Bracket { open, .. } => *open,
            TextObject::Quote { ch, .. } => *ch,
            TextObject::Ref { .. } => 'l',
        };
        self.push_label(c);
    }

    fn finish(&mut self, step: Step) -> Step {
        self.reset();
        step
    }

    fn cancel(&mut self) -> Step {
        self.reset();
        Step::Cancel
    }
}

/// `f` `t` `F` `T` as `(till, back)`.
fn find_kind(c: char) -> Option<(bool, bool)> {
    match c {
        'f' => Some((false, false)),
        't' => Some((true, false)),
        'F' => Some((false, true)),
        'T' => Some((true, true)),
        _ => None,
    }
}

fn operator_of(c: char) -> Option<Operator> {
    match c {
        'd' => Some(Operator::Delete),
        'c' => Some(Operator::Change),
        'y' => Some(Operator::Yank),
        '>' => Some(Operator::Indent),
        '<' => Some(Operator::Outdent),
        _ => None,
    }
}

/// The text object `c` names after `i` or `a`.
fn text_object_of(c: char, around: bool) -> Option<TextObject> {
    match c {
        'w' => Some(TextObject::Word { big: false, around }),
        'W' => Some(TextObject::Word { big: true, around }),
        '(' | ')' => Some(TextObject::Bracket { open: '(', around }),
        '[' | ']' => Some(TextObject::Bracket { open: '[', around }),
        '{' | '}' => Some(TextObject::Bracket { open: '{', around }),
        '"' | '\'' | '`' => Some(TextObject::Quote { ch: c, around }),
        // `il` and `al` need `inline::extract_refs` (PR 4): the grammar knows them, and
        // `textobj::range_of` answers `None` until then.
        'l' => Some(TextObject::Ref { around }),
        _ => None,
    }
}

/// The motion a key is, on its own or as an operator's target. `f` `t` `F` `T` and `g`
/// are not here: they need a second key, and the caller handles them first.
fn motion_of(key: &Key) -> Option<Motion> {
    Some(match key {
        Key::Char('h') | Key::Left | Key::Backspace => Motion::Left,
        Key::Char('l') | Key::Right | Key::Space => Motion::Right,
        Key::Char('j') | Key::Down | Key::Enter => Motion::Down,
        Key::Char('k') | Key::Up => Motion::Up,
        Key::Char('w') => Motion::WordForward { big: false },
        Key::Char('W') => Motion::WordForward { big: true },
        Key::Char('b') => Motion::WordBack { big: false },
        Key::Char('B') => Motion::WordBack { big: true },
        Key::Char('e') => Motion::WordEnd { big: false },
        Key::Char('E') => Motion::WordEnd { big: true },
        Key::Char('0') => Motion::LineStart,
        Key::Char('^') => Motion::LineFirstNonBlank,
        Key::Char('$') => Motion::LineEnd,
        Key::Char(';') => Motion::RepeatFind { reverse: false },
        Key::Char(',') => Motion::RepeatFind { reverse: true },
        Key::Char('G') => Motion::PageLast,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a key string, returning the last step.
    fn feed(keys: &str) -> Step {
        let mut parser = Parser::default();
        feed_with(&mut parser, keys)
    }

    fn feed_with(parser: &mut Parser, keys: &str) -> Step {
        let mut step = Step::Pending(String::new());
        for key in keys_of(keys) {
            step = parser.feed(key);
        }
        step
    }

    fn keys_of(row: &str) -> Vec<Key> {
        if let Some(rest) = row.strip_prefix("Ctrl-") {
            return rest.chars().map(Key::Ctrl).collect();
        }
        row.chars().map(Key::Char).collect()
    }

    fn run(keys: &str) -> Command {
        match feed(keys) {
            Step::Run(command) => command,
            other => panic!("{keys} gave {other:?}"),
        }
    }

    fn move_(motion: Motion, count: usize) -> Command {
        Command::Move { motion, count }
    }

    fn operate(op: Operator, target: Target, count: usize, register: Option<char>) -> Command {
        Command::Operate {
            op,
            target,
            count,
            register,
        }
    }

    /// 1A.6: key strings to commands.
    #[test]
    fn keys_to_commands() {
        let word = Motion::WordForward { big: false };
        assert_eq!(
            run("dw"),
            operate(Operator::Delete, Target::Motion(word), 1, None)
        );
        assert_eq!(
            run("d2w"),
            operate(Operator::Delete, Target::Motion(word), 2, None)
        );
        assert_eq!(
            run("2dw"),
            operate(Operator::Delete, Target::Motion(word), 2, None)
        );
        assert_eq!(
            run("2d3w"),
            operate(Operator::Delete, Target::Motion(word), 6, None)
        );
        assert_eq!(run("dd"), operate(Operator::Delete, Target::Line, 1, None));
        assert_eq!(run("3dd"), operate(Operator::Delete, Target::Line, 3, None));
        assert_eq!(
            run("ci["),
            operate(
                Operator::Change,
                Target::Object(TextObject::Bracket {
                    open: '[',
                    around: false
                }),
                1,
                None
            )
        );
        assert_eq!(
            run("caw"),
            operate(
                Operator::Change,
                Target::Object(TextObject::Word {
                    big: false,
                    around: true
                }),
                1,
                None
            )
        );
        assert_eq!(run("yy"), operate(Operator::Yank, Target::Line, 1, None));
        assert_eq!(
            run("f,"),
            move_(
                Motion::Find {
                    ch: ',',
                    till: false,
                    back: false
                },
                1
            )
        );
        assert_eq!(run("3j"), move_(Motion::Down, 3));
        assert_eq!(run("gg"), move_(Motion::PageFirst, 1));
        assert_eq!(run("G"), move_(Motion::PageLast, 1));
        assert_eq!(run("rx"), Command::ReplaceChar('x'));
        assert_eq!(
            run("\"_dd"),
            operate(Operator::Delete, Target::Line, 1, Some('_'))
        );
    }

    /// The insert family, the edits and the mode changes each map to their own command.
    #[test]
    fn the_rest_of_the_table() {
        for (keys, at) in [
            ("i", InsertAt::Cursor),
            ("a", InsertAt::After),
            ("I", InsertAt::LineStart),
            ("A", InsertAt::LineEnd),
            ("o", InsertAt::BlockBelow),
            ("O", InsertAt::BlockAbove),
            ("s", InsertAt::ReplaceChar),
            ("S", InsertAt::ReplaceLine),
            ("C", InsertAt::ToLineEnd),
        ] {
            assert_eq!(run(keys), Command::Insert { at, register: None }, "{keys}");
        }
        // A register before a command that deletes is kept for the delete it does first
        // (`"_s` discards, `"+C` puts it on the clipboard; F9).
        assert_eq!(
            run("\"_s"),
            Command::Insert {
                at: InsertAt::ReplaceChar,
                register: Some('_')
            }
        );
        assert_eq!(
            run("\"+3D"),
            Command::DeleteToLineEnd {
                count: 3,
                register: Some('+')
            }
        );
        assert_eq!(
            run("3x"),
            Command::DeleteChar {
                back: false,
                count: 3,
                register: None
            }
        );
        assert_eq!(
            run("X"),
            Command::DeleteChar {
                back: true,
                count: 1,
                register: None
            }
        );
        assert_eq!(
            run("D"),
            Command::DeleteToLineEnd {
                count: 1,
                register: None
            }
        );
        assert_eq!(run("2~"), Command::ToggleCase { count: 2 });
        assert_eq!(
            run("2p"),
            Command::Paste {
                before: false,
                count: 2,
                register: None
            }
        );
        assert_eq!(
            run("\"+P"),
            Command::Paste {
                before: true,
                count: 1,
                register: Some('+')
            }
        );
        assert_eq!(run("3J"), Command::JoinNext { count: 3 });
        assert_eq!(run("u"), Command::Undo { count: 1 });
        assert_eq!(run("2u"), Command::Undo { count: 2 });
        assert_eq!(run("Ctrl-r"), Command::Redo { count: 1 });
        assert_eq!(run("v"), Command::EnterVisual { line: false });
        assert_eq!(run("V"), Command::EnterVisual { line: true });
        assert_eq!(run("."), Command::RepeatChange);
        assert_eq!(run("Y"), operate(Operator::Yank, Target::Line, 1, None));
        assert_eq!(run(">>"), operate(Operator::Indent, Target::Line, 1, None));
        assert_eq!(run("<<"), operate(Operator::Outdent, Target::Line, 1, None));
    }

    /// The named keys are motions in Normal, and `d` takes them as targets.
    #[test]
    fn named_keys_are_motions() {
        for (key, motion) in [
            (Key::Space, Motion::Right),
            (Key::Backspace, Motion::Left),
            (Key::Enter, Motion::Down),
            (Key::Left, Motion::Left),
            (Key::Right, Motion::Right),
            (Key::Up, Motion::Up),
            (Key::Down, Motion::Down),
        ] {
            let mut parser = Parser::default();
            assert_eq!(
                parser.feed(key.clone()),
                Step::Run(move_(motion, 1)),
                "{key:?}"
            );
            let mut parser = Parser::default();
            let _ = parser.feed(Key::Char('2'));
            let _ = parser.feed(Key::Char('d'));
            assert_eq!(
                parser.feed(key.clone()),
                Step::Run(operate(Operator::Delete, Target::Motion(motion), 2, None)),
                "{key:?}"
            );
        }
        // The Delete key is Vim's `x`.
        let mut parser = Parser::default();
        assert_eq!(
            parser.feed(Key::Delete),
            Step::Run(Command::DeleteChar {
                back: false,
                count: 1,
                register: None
            })
        );
    }

    /// 1A.6: every unfinished prefix is `Pending`, and the label is what the mode shows.
    #[test]
    fn unfinished_prefixes_pend() {
        for (keys, label) in [
            ("d", "d"),
            ("2", "2"),
            ("2d", "2d"),
            ("g", "g"),
            ("f", "f"),
            ("\"", "\""),
            ("ci", "ci"),
            ("di", "di"),
        ] {
            assert_eq!(feed(keys), Step::Pending(label.to_string()), "{keys}");
        }
        // The label builds up and clears when the command completes.
        let mut parser = Parser::default();
        assert_eq!(parser.feed(Key::Char('2')), Step::Pending("2".into()));
        assert_eq!(parser.feed(Key::Char('d')), Step::Pending("2d".into()));
        assert_eq!(
            parser.feed(Key::Char('w')),
            Step::Run(feed("2dw").command())
        );
        assert_eq!(parser.pending(), "");
    }

    /// 1A.6: every unknown continuation is `Cancel`, and nothing is left pending.
    #[test]
    fn unknown_keys_cancel() {
        for keys in ["dq", "gq", "\"q", "zz", "d\"", "g0", "1q", "ciq"] {
            assert_eq!(feed(keys), Step::Cancel, "{keys}");
        }
        let mut parser = Parser::default();
        let _ = parser.feed(Key::Char('d'));
        assert_eq!(parser.feed(Key::Char('q')), Step::Cancel);
        assert_eq!(parser.pending(), "");
    }

    /// 1A.6: `0` after a digit is a count; `0` alone is `LineStart`.
    #[test]
    fn zero_is_a_count_or_the_line_start() {
        assert_eq!(run("0"), move_(Motion::LineStart, 1));
        assert_eq!(run("20j"), move_(Motion::Down, 20));
        assert_eq!(
            run("d0"),
            operate(Operator::Delete, Target::Motion(Motion::LineStart), 1, None)
        );
        assert_eq!(
            run("10dd"),
            operate(Operator::Delete, Target::Line, 10, None)
        );
        // `20` is a count, so `d20` still waits for the motion it applies to.
        assert_eq!(feed("d20"), Step::Pending("d20".into()));
    }

    /// Escape cancels what is pending and only leaves Normal when nothing was.
    #[test]
    fn escape_is_the_ladder() {
        let mut parser = Parser::default();
        assert_eq!(
            parser.feed(Key::Escape),
            Step::Run(Command::LeaveNormal),
            "nothing pending"
        );
        let _ = parser.feed(Key::Char('2'));
        assert_eq!(parser.feed(Key::Escape), Step::Pending(String::new()));
        assert_eq!(parser.pending(), "");
        // A second Escape from a clean parser leaves Normal.
        assert_eq!(parser.feed(Key::Escape), Step::Run(Command::LeaveNormal));
    }

    /// `;` and `,` reuse the last find; the target outlives the command and a reset.
    #[test]
    fn the_last_find_is_remembered() {
        let mut parser = Parser::default();
        assert_eq!(parser.last_find(), None);
        let _ = feed_with(&mut parser, "t,");
        assert_eq!(parser.last_find(), Some((',', true, false)));
        assert_eq!(parser.pending(), "");

        let mut parser = Parser::default();
        let _ = feed_with(&mut parser, "F;");
        assert_eq!(parser.last_find(), Some((';', false, true)));
        parser.reset();
        assert_eq!(parser.last_find(), Some((';', false, true)));
        assert_eq!(
            parser.feed(Key::Char(';')),
            Step::Run(move_(Motion::RepeatFind { reverse: false }, 1))
        );
        assert_eq!(
            parser.feed(Key::Char(',')),
            Step::Run(move_(Motion::RepeatFind { reverse: true }, 1))
        );
        assert_eq!(parser.last_find(), Some((';', false, true)));
    }

    /// `f` takes any character, including one an operator would use, and `df,` works.
    #[test]
    fn find_takes_its_character() {
        assert_eq!(
            run("dfd"),
            operate(
                Operator::Delete,
                Target::Motion(Motion::Find {
                    ch: 'd',
                    till: false,
                    back: false
                }),
                1,
                None
            )
        );
        assert_eq!(
            run("2t3"),
            move_(
                Motion::Find {
                    ch: '3',
                    till: true,
                    back: false
                },
                2
            )
        );
        // A non-character after `f` cancels instead of awaiting for ever.
        let mut parser = Parser::default();
        let _ = parser.feed(Key::Char('f'));
        assert_eq!(parser.feed(Key::Escape), Step::Pending(String::new()));
        let _ = parser.feed(Key::Char('f'));
        assert_eq!(parser.feed(Key::Left), Step::Cancel);
    }

    /// A register is only the two PR 2 names; a register before an operator is kept.
    #[test]
    fn registers() {
        assert_eq!(
            run("\"+yy"),
            operate(Operator::Yank, Target::Line, 1, Some('+'))
        );
        // `"_` is dropped by commands that carry no register, and the count still counts.
        assert_eq!(
            run("\"_2dd"),
            operate(Operator::Delete, Target::Line, 2, Some('_'))
        );
    }

    /// The text objects the grammar knows, and the ones it does not.
    #[test]
    fn text_objects() {
        for (keys, object) in [
            (
                "diw",
                TextObject::Word {
                    big: false,
                    around: false,
                },
            ),
            (
                "daW",
                TextObject::Word {
                    big: true,
                    around: true,
                },
            ),
            (
                "di(",
                TextObject::Bracket {
                    open: '(',
                    around: false,
                },
            ),
            (
                "da{",
                TextObject::Bracket {
                    open: '{',
                    around: true,
                },
            ),
            (
                "yi\"",
                TextObject::Quote {
                    ch: '"',
                    around: false,
                },
            ),
            (
                "ca`",
                TextObject::Quote {
                    ch: '`',
                    around: true,
                },
            ),
            ("yal", TextObject::Ref { around: true }),
        ] {
            match run(keys) {
                Command::Operate {
                    target: Target::Object(got),
                    ..
                } => assert_eq!(got, object, "{keys}"),
                other => panic!("{keys} gave {other:?}"),
            }
        }
        // `)` and `]` stand for the same pair as `(` and `[`.
        assert_eq!(run("di)"), run("di("));
        assert_eq!(run("da]"), run("da["));
        assert_eq!(feed("diz"), Step::Cancel);
    }

    /// A digit run cannot overflow and cannot become unbounded work either: it clamps to
    /// [`MAX_COUNT`]. Saturating at `usize::MAX` made `99…9u` a hang (rule 11).
    #[test]
    fn counts_clamp_instead_of_overflowing() {
        let huge = format!("{}w", "9".repeat(60));
        assert_eq!(
            run(&huge),
            move_(Motion::WordForward { big: false }, MAX_COUNT)
        );
    }

    trait UnwrapCommand {
        fn command(self) -> Command;
    }

    impl UnwrapCommand for Step {
        fn command(self) -> Command {
            match self {
                Step::Run(command) => command,
                other => panic!("{other:?}"),
            }
        }
    }
}

/// The character an awaited argument stands for. Space arrives as the named
/// [`Key::Space`] (named keys are classified before printable characters), but as an
/// argument it is the character it is: `f<Space>`, `t<Space>` and `r<Space>` all work
/// (F10).
fn char_argument(key: Key) -> Option<char> {
    match key {
        Key::Char(c) => Some(c),
        Key::Space => Some(' '),
        _ => None,
    }
}

#[cfg(test)]
mod count_cap_tests {
    use super::*;

    fn run(keys: &str) -> Step {
        let mut p = Parser::default();
        let mut last = Step::Cancel;
        for c in keys.chars() {
            last = p.feed(Key::Char(c));
        }
        last
    }

    /// Rule 11: a typed digit run must not become unbounded work. Before the cap, the
    /// count saturated at `usize::MAX` and `Command::Undo { count }` looped that many
    /// times, each loop committing and rebuilding the page, which froze the app.
    #[test]
    fn a_long_digit_run_clamps_instead_of_saturating() {
        let Step::Run(Command::Undo { count }) = run("99999999999999999999u") else {
            panic!("a digit run before `u` must still undo");
        };
        assert_eq!(count, MAX_COUNT);
    }

    #[test]
    fn a_count_below_the_cap_is_untouched() {
        let Step::Run(Command::Undo { count }) = run("12u") else {
            panic!("`12u` must undo");
        };
        assert_eq!(count, 12);
    }

    /// The cap applies to the product of the two counts as well (`2d3w` is 6 words).
    #[test]
    fn the_cap_holds_for_an_operator_count() {
        let Step::Run(Command::Operate { count, .. }) = run("9999d9999w") else {
            panic!("`9999d9999w` must be an operator command");
        };
        assert_eq!(count, MAX_COUNT, "the product clamps like each count does");
        // Two counts below the cap still multiply.
        let Step::Run(Command::Operate { count, .. }) = run("2d3w") else {
            panic!("`2d3w` must be an operator command");
        };
        assert_eq!(count, 6);
    }
}

#[cfg(test)]
mod argument_tests {
    use super::*;

    fn feed(keys: &[Key]) -> Step {
        let mut p = Parser::default();
        let mut last = Step::Cancel;
        for key in keys {
            last = p.feed(key.clone());
        }
        last
    }

    /// F10: Space is a named key, but it is also the character a find or a replace can
    /// take. `0f<Space>` moves to the blank instead of cancelling.
    #[test]
    fn space_is_a_character_argument() {
        let find = |till, back| {
            Step::Run(Command::Move {
                motion: Motion::Find {
                    ch: ' ',
                    till,
                    back,
                },
                count: 1,
            })
        };
        assert_eq!(feed(&[Key::Char('f'), Key::Space]), find(false, false));
        assert_eq!(feed(&[Key::Char('t'), Key::Space]), find(true, false));
        assert_eq!(feed(&[Key::Char('F'), Key::Space]), find(false, true));
        assert_eq!(feed(&[Key::Char('T'), Key::Space]), find(true, true));
        assert_eq!(
            feed(&[Key::Char('r'), Key::Space]),
            Step::Run(Command::ReplaceChar(' '))
        );
        // Space on its own is still the `Right` motion.
        assert_eq!(
            feed(&[Key::Space]),
            Step::Run(Command::Move {
                motion: Motion::Right,
                count: 1,
            })
        );
        // A named key that is not a character still cancels an awaited one.
        assert_eq!(feed(&[Key::Char('f'), Key::Enter]), Step::Cancel);
    }
}
