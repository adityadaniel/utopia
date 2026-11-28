use std::ops::Range;

use super::motion;
use super::Motion;
use crate::text::{next_grapheme, prev_grapheme};

pub fn motion_range(
    text: &str,
    offset: usize,
    motion: Motion,
    count: usize,
    last_find: Option<(char, bool, bool)>,
) -> Option<Range<usize>> {
    let count = count.max(1);
    let offset = motion::clamp_normal(text, offset);
    match motion {
        Motion::Down | Motion::Up | Motion::PageFirst | Motion::PageLast => None,
        Motion::LineEnd => {
            let mut end = motion::line_end_exclusive(text, offset);
            for _ in 1..count {
                if end >= text.len() {
                    break;
                }
                end = motion::line_end_exclusive(text, next_grapheme(text, end));
            }
            Some(offset..end)
        }
        Motion::WordEnd { big } => {
            let mut end = offset;
            for _ in 0..count {
                end = motion::word_end(text, end, big);
            }
            Some(offset..inclusive_end(text, end))
        }
        Motion::Find { ch, till, back } => {
            let target = motion::find_char_nth(text, offset, ch, till, back, count)?;
            Some(find_range(text, offset, target, back))
        }
        Motion::RepeatFind { reverse } => {
            let (ch, till, back) = last_find?;
            let target = motion::repeat_find(text, offset, ch, till, back != reverse, count)?;
            Some(find_range(text, offset, target, back != reverse))
        }
        Motion::WordForward { big } => {
            let mut end = offset;
            for _ in 0..count {
                end = motion::word_forward_raw(text, end, big);
            }
            Some(offset..exclusive_end(text, end).max(offset))
        }
        Motion::Right => {
            let mut end = offset;
            for _ in 0..count {
                let limit = motion::line_end_exclusive(text, end);
                if end >= limit {
                    break;
                }
                end = next_grapheme(text, end).min(limit);
            }
            Some(offset..exclusive_end(text, end).max(offset))
        }
        Motion::LineFirstNonBlank => {
            let mut at = offset;
            for _ in 0..count {
                at = motion::line_first_non_blank(text, at);
            }
            Some(if at > offset { offset..at } else { at..offset })
        }
        Motion::Left | Motion::WordBack { .. } | Motion::LineStart => {
            let mut start = offset;
            for _ in 0..count {
                start = match motion {
                    Motion::WordBack { big } => motion::word_back(text, start, big),
                    Motion::LineStart => motion::line_start(text, start),
                    _ => prev_grapheme(text, start),
                };
            }
            Some(start.min(offset)..offset)
        }
    }
}

pub fn change_word_range(
    text: &str,
    offset: usize,
    big: bool,
    count: usize,
) -> Option<Range<usize>> {
    let offset = motion::clamp_normal(text, offset);
    if motion::class_at(text, offset, big) == Some(motion::Class::Blank) {
        return None;
    }
    let word =
        super::textobj::range_of(text, offset, super::TextObject::Word { big, around: false })?;
    let mut end = word.end;
    for _ in 1..count.max(1) {
        let last = prev_grapheme(text, end);
        let next = next_grapheme(text, motion::word_end(text, last, big));
        if next <= end {
            break;
        }
        end = next;
    }
    Some(offset..end)
}

fn inclusive_end(text: &str, target: usize) -> usize {
    next_grapheme(text, target)
}

fn find_range(text: &str, offset: usize, target: usize, back: bool) -> Range<usize> {
    if back {
        target.min(offset)..offset
    } else {
        offset..inclusive_end(text, target)
    }
}

fn exclusive_end(text: &str, end: usize) -> usize {
    if end == 0 || end > text.len() {
        return end.min(text.len());
    }
    if motion::line_start(text, end) == end {
        return motion::line_end_exclusive(text, prev_grapheme(text, end));
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;

    fn range(text: &str, at: usize, motion: Motion, count: usize) -> Range<usize> {
        let range = motion_range(text, at, motion, count, None).expect("no range");
        check(text, range)
    }

    fn check(text: &str, range: Range<usize>) -> Range<usize> {
        assert!(
            range.start <= range.end && range.end <= text.len(),
            "{text:?}: {range:?}"
        );
        for offset in [range.start, range.end] {
            assert!(
                offset == text.len() || text.grapheme_indices(true).any(|(i, _)| i == offset),
                "{text:?}: {range:?} splits a grapheme"
            );
        }
        range
    }

    fn word_forward() -> Motion {
        Motion::WordForward { big: false }
    }

    #[test]
    fn exclusive_motions_stop_before_their_target() {
        let text = "alpha beta";
        assert_eq!(range(text, 0, word_forward(), 1), 0..6);
        assert_eq!(range(text, 0, word_forward(), 2), 0..10, "d2w takes both");
        assert_eq!(range(text, 0, Motion::Right, 1), 0..1);
        assert_eq!(range(text, 0, Motion::Right, 3), 0..3);
        assert_eq!(range(text, 10, Motion::WordBack { big: false }, 1), 6..9);
        assert_eq!(range(text, 6, Motion::LineStart, 1), 0..6);
        assert_eq!(range(text, 4, Motion::Left, 2), 2..4);
    }

    #[test]
    fn d_w_on_the_last_word_takes_it() {
        assert_eq!(range("alpha beta", 6, word_forward(), 1), 6..10);
        assert_eq!(range("alpha beta", 6, word_forward(), 2), 6..10);
        assert_eq!(range("ab c", 0, word_forward(), 1), 0..3);
        assert_eq!(range("alpha\nbeta", 0, word_forward(), 1), 0..5);
        assert_eq!(range("alpha\nbeta", 0, word_forward(), 2), 0..10);
        assert_eq!(range("alpha", 0, word_forward(), 1), 0..5);
        assert_eq!(range("alpha\n", 0, word_forward(), 1), 0..5);
        assert_eq!(range("", 0, word_forward(), 1), 0..0);
        assert_eq!(range(" ", 0, word_forward(), 1), 0..1);
    }

    #[test]
    fn cw_takes_the_word_under_the_cursor() {
        for at in 0..5 {
            assert_eq!(
                change_word_range("alpha beta", at, false, 1),
                Some(at..5),
                "`cw` from {at} takes the rest of `alpha`, not `beta`"
            );
        }
        assert_eq!(
            change_word_range("alpha beta gamma", 0, false, 2),
            Some(0..10)
        );
        assert_eq!(
            change_word_range("alpha beta gamma", 4, false, 2),
            Some(4..10)
        );
        assert_eq!(change_word_range("alpha beta", 5, false, 1), None);
        assert_eq!(change_word_range("alpha", 0, false, 3), Some(0..5));
        assert_eq!(change_word_range("a b-c d", 2, true, 1), Some(2..5));
        for text in ["alpha beta", "e\u{301}cole x", "日本 語", "", "  "] {
            for at in 0..=text.len() {
                if let Some(range) = change_word_range(text, at, false, 2) {
                    check(text, range);
                }
            }
        }
    }

    #[test]
    fn d_caret_from_the_indent_takes_the_blanks() {
        let text = "  abc";
        assert_eq!(range(text, 0, Motion::LineFirstNonBlank, 1), 0..2);
        assert_eq!(range(text, 1, Motion::LineFirstNonBlank, 1), 1..2);
        assert_eq!(range(text, 2, Motion::LineFirstNonBlank, 1), 2..2);
        assert_eq!(range(text, 4, Motion::LineFirstNonBlank, 1), 2..4);
    }

    #[test]
    fn d_l_stops_at_the_end_of_the_line() {
        assert_eq!(range("ab\ncd", 0, Motion::Right, 5), 0..2);
        assert_eq!(range("ab\ncd", 3, Motion::Right, 5), 3..5);
        assert_eq!(range("ab\ncd", 0, Motion::Right, 1), 0..1);
    }

    #[test]
    fn inclusive_motions_take_their_target() {
        let text = "alpha beta";
        assert_eq!(range(text, 0, Motion::WordEnd { big: false }, 1), 0..5);
        assert_eq!(range(text, 0, Motion::WordEnd { big: false }, 2), 0..10);
        assert_eq!(range(text, 6, Motion::LineEnd, 1), 6..10);
        assert_eq!(range("alpha\nbeta", 0, Motion::LineEnd, 1), 0..5);
        assert_eq!(range("ab\ncd\nef", 0, Motion::LineEnd, 2), 0..5);
        let find = Motion::Find {
            ch: 't',
            till: false,
            back: false,
        };
        assert_eq!(range(text, 6, find, 1), 6..9, "df includes the target");
        let missing = Motion::Find {
            ch: 'z',
            till: false,
            back: false,
        };
        assert_eq!(motion_range(text, 6, missing, 1, None), None);
        let till = Motion::Find {
            ch: 'a',
            till: true,
            back: false,
        };
        assert_eq!(range(text, 6, till, 1), 6..9);
        let back = Motion::Find {
            ch: 'b',
            till: false,
            back: true,
        };
        assert_eq!(range(text, 10, back, 1), 6..9);
    }

    #[test]
    fn repeat_find_ranges() {
        let text = "a,b,c";
        let last = Some((',', false, false));
        let semi =
            motion_range(text, 0, Motion::RepeatFind { reverse: false }, 1, last).expect("a range");
        assert_eq!(
            check(text, semi),
            0..2,
            "d; takes up to and including the `,`"
        );
        let comma =
            motion_range(text, 4, Motion::RepeatFind { reverse: true }, 1, last).expect("a range");
        assert_eq!(check(text, comma), 3..4, "d, takes the previous `,`");
        assert_eq!(
            motion_range(text, 0, Motion::RepeatFind { reverse: false }, 1, None),
            None,
            "no previous find, no range"
        );
    }

    #[test]
    fn block_motions_have_no_charwise_range() {
        for motion in [
            Motion::Down,
            Motion::Up,
            Motion::PageFirst,
            Motion::PageLast,
        ] {
            assert_eq!(motion_range("abc", 1, motion, 1, None), None, "{motion:?}");
        }
    }

    #[test]
    fn hard_text_never_splits_a_grapheme() {
        for text in [
            "",
            " ",
            "👨‍👩‍👦 cat",
            "e\u{301}cole two",
            "日本語 text",
            "ab\ncd",
            "\n",
            "a\u{301}",
            "a\r\nb",
        ] {
            for at in 0..=text.len() {
                for motion in [
                    word_forward(),
                    Motion::WordForward { big: true },
                    Motion::WordBack { big: false },
                    Motion::WordEnd { big: false },
                    Motion::Left,
                    Motion::Right,
                    Motion::LineStart,
                    Motion::LineFirstNonBlank,
                    Motion::LineEnd,
                ] {
                    if let Some(range) = motion_range(text, at, motion, 2, None) {
                        check(text, range);
                    }
                }
            }
        }
    }
}
