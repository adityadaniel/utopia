use super::{Command, InsertAt, Key, Motion, Operator, Step, Target, TextObject};

pub const MAX_COUNT: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Await {
    Register,
    Find { till: bool, back: bool },
    Replace,
    Object { around: bool },
}

#[derive(Default, Debug)]
pub struct Parser {
    count: Option<usize>,
    count2: Option<usize>,
    register: Option<char>,
    op: Option<Operator>,
    prefix: Option<char>,
    awaiting: Option<Await>,
    label: String,
    last_find: Option<(char, bool, bool)>,
}

impl Parser {
    pub fn feed(&mut self, key: Key) -> Step {
        if key == Key::Escape {
            let was_pending = self.is_pending();
            self.reset();
            return if was_pending {
                Step::Pending(String::new())
            } else {
                Step::Run(Command::LeaveNormal)
            };
        }

        let ch = match &key {
            Key::Char(c) => Some(*c),
            _ => None,
        };
        let ctrl = match &key {
            Key::Ctrl(c) => Some(*c),
            _ => None,
        };

        if let Some(awaiting) = self.awaiting.take() {
            return self.awaited(awaiting, key);
        }

        if let Some(prefix) = self.prefix.take() {
            if prefix == 'g' && key == Key::Char('g') {
                let count = self.take_count();
                let motion = Motion::PageFirst;
                return self.finish(Step::Run(Command::Move { motion, count }));
            }
            return self.cancel();
        }

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

        if let Some(c) = ch {
            if let Some((till, back)) = find_kind(c) {
                self.awaiting = Some(Await::Find { till, back });
                self.push_label(c);
                return Step::Pending(self.pending());
            }
            if c == 'r' && self.op.is_none() {
                self.awaiting = Some(Await::Replace);
                self.push_label(c);
                return Step::Pending(self.pending());
            }
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

        if let Some(c) = ch {
            if let Some(op) = operator_of(c) {
                self.op = Some(op);
                self.push_label(c);
                return Step::Pending(self.pending());
            }
        }

        if let Some(motion) = motion_of(&key) {
            let count = self.take_count();
            return self.finish(Step::Run(Command::Move { motion, count }));
        }

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
            _ => return self.cancel(),
        };
        self.finish(Step::Run(command))
    }

    pub fn reset(&mut self) {
        self.count = None;
        self.count2 = None;
        self.register = None;
        self.op = None;
        self.prefix = None;
        self.awaiting = None;
        self.label.clear();
    }

    pub fn pending(&self) -> String {
        self.label.clone()
    }

    pub fn last_find(&self) -> Option<(char, bool, bool)> {
        self.last_find
    }

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

fn text_object_of(c: char, around: bool) -> Option<TextObject> {
    match c {
        'w' => Some(TextObject::Word { big: false, around }),
        'W' => Some(TextObject::Word { big: true, around }),
        '(' | ')' => Some(TextObject::Bracket { open: '(', around }),
        '[' | ']' => Some(TextObject::Bracket { open: '[', around }),
        '{' | '}' => Some(TextObject::Bracket { open: '{', around }),
        '"' | '\'' | '`' => Some(TextObject::Quote { ch: c, around }),
        'l' => Some(TextObject::Ref { around }),
        _ => None,
    }
}

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
        let mut parser = Parser::default();
        assert_eq!(parser.feed(Key::Char('2')), Step::Pending("2".into()));
        assert_eq!(parser.feed(Key::Char('d')), Step::Pending("2d".into()));
        assert_eq!(
            parser.feed(Key::Char('w')),
            Step::Run(feed("2dw").command())
        );
        assert_eq!(parser.pending(), "");
    }

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
        assert_eq!(feed("d20"), Step::Pending("d20".into()));
    }

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
        assert_eq!(parser.feed(Key::Escape), Step::Run(Command::LeaveNormal));
    }

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
        let mut parser = Parser::default();
        let _ = parser.feed(Key::Char('f'));
        assert_eq!(parser.feed(Key::Escape), Step::Pending(String::new()));
        let _ = parser.feed(Key::Char('f'));
        assert_eq!(parser.feed(Key::Left), Step::Cancel);
    }

    #[test]
    fn registers() {
        assert_eq!(
            run("\"+yy"),
            operate(Operator::Yank, Target::Line, 1, Some('+'))
        );
        assert_eq!(
            run("\"_2dd"),
            operate(Operator::Delete, Target::Line, 2, Some('_'))
        );
    }

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
        assert_eq!(run("di)"), run("di("));
        assert_eq!(run("da]"), run("da["));
        assert_eq!(feed("diz"), Step::Cancel);
    }

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

    #[test]
    fn the_cap_holds_for_an_operator_count() {
        let Step::Run(Command::Operate { count, .. }) = run("9999d9999w") else {
            panic!("`9999d9999w` must be an operator command");
        };
        assert_eq!(count, MAX_COUNT, "the product clamps like each count does");
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
        assert_eq!(
            feed(&[Key::Space]),
            Step::Run(Command::Move {
                motion: Motion::Right,
                count: 1,
            })
        );
        assert_eq!(feed(&[Key::Char('f'), Key::Enter]), Step::Cancel);
    }
}
