//! Operator ranges (docs/vim-mode-plan.md 2B): what `d`, `c` and `y` take.
//!
//! A motion is a jump and an operator is a span, and the two are not the same range. `w`
//! puts the cursor on the next word's first character; `dw` takes the text *up to* that
//! character and leaves it. `e` and `f` include the grapheme they land on, and `$` stops
//! before the line break. That is Vim's inclusive/exclusive distinction, and it lives here
//! — with the motions and the text objects, not in the UI — so `d`, `c` and `y` cannot
//! disagree about it.
//!
//! Every range is built from the grapheme-derived offsets [`super::motion`] returns, so it
//! sits on grapheme boundaries of `text` and the caller may slice with it (rule 11).

use std::ops::Range;

use super::motion;
use super::Motion;
use crate::text::{next_grapheme, prev_grapheme};

/// The range an operator acts on for `motion`, or `None` when the motion is not charwise
/// inside one block (`j`/`k` cross blocks, `gg`/`G` go to the page's edges) or finds
/// nothing.
///
/// `count` is the motion's count (`d2w`), and `last_find` is the parser's `;`/`,` target.
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
        // Not charwise: the host moves the cursor between blocks (PR 2C and PR 4).
        Motion::Down | Motion::Up | Motion::PageFirst | Motion::PageLast => None,
        // `$` is exclusive at the line's end: `d$` takes the line's text, not its break. A
        // count takes that many lines.
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
        // `e`, `f` and `t` include the grapheme they stop on.
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
        // `w` is exclusive, and an end that reaches column 1 belongs to the line before it.
        Motion::WordForward { big } => {
            let mut end = offset;
            for _ in 0..count {
                end = motion::word_forward_raw(text, end, big);
            }
            Some(offset..exclusive_end(text, end).max(offset))
        }
        // `l` is exclusive (`dl` is `x`), and stops at the end of the line.
        Motion::Right => {
            let mut end = offset;
            for _ in 0..count {
                end = next_grapheme(text, end);
            }
            Some(offset..exclusive_end(text, end).max(offset))
        }
        // Backwards: the target's grapheme is included, the cursor's is not.
        Motion::Left | Motion::WordBack { .. } | Motion::LineStart | Motion::LineFirstNonBlank => {
            let mut start = offset;
            for _ in 0..count {
                start = match motion {
                    Motion::WordBack { big } => motion::word_back(text, start, big),
                    Motion::LineStart => motion::line_start(text, start),
                    Motion::LineFirstNonBlank => motion::line_first_non_blank(text, start),
                    // `h`.
                    _ => prev_grapheme(text, start),
                };
            }
            Some(start.min(offset)..offset)
        }
    }
}

/// One past the last grapheme of the target: an inclusive motion's span.
fn inclusive_end(text: &str, target: usize) -> usize {
    next_grapheme(text, target)
}

/// The span between the cursor and a find's target. A find to the right includes the
/// grapheme it lands on; one to the left starts on it, so the cursor's own grapheme stays,
/// as Vim's `dF` leaves the character under the cursor alone.
fn find_range(text: &str, offset: usize, target: usize, back: bool) -> Range<usize> {
    if back {
        target.min(offset)..offset
    } else {
        offset..inclusive_end(text, target)
    }
}

/// Vim's rule for an exclusive motion (`:help exclusive`): an end that lands on the first
/// grapheme of a line belongs to the end of the previous one, so `dw` over a line's last
/// word takes the word and leaves the break after it.
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

    /// A range must sit on grapheme boundaries and inside the text.
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

    /// `dw` takes the text up to the next word, and the next word stays.
    #[test]
    fn exclusive_motions_stop_before_their_target() {
        let text = "alpha beta";
        assert_eq!(range(text, 0, word_forward(), 1), 0..6);
        assert_eq!(range(text, 0, word_forward(), 2), 0..10, "d2w takes both");
        // `dl` is `x`.
        assert_eq!(range(text, 0, Motion::Right, 1), 0..1);
        assert_eq!(range(text, 0, Motion::Right, 3), 0..3);
        // Backwards: the target is included, the cursor's own grapheme is not.
        // The block cursor never rests past the last grapheme, so `db` from the end is
        // taken from the last character.
        assert_eq!(range(text, 10, Motion::WordBack { big: false }, 1), 6..9);
        assert_eq!(range(text, 6, Motion::LineStart, 1), 0..6);
        assert_eq!(range(text, 4, Motion::Left, 2), 2..4);
    }

    /// 2B: `dw` on the last word of a block takes the whole word — no stray character and
    /// no line break.
    #[test]
    fn d_w_on_the_last_word_takes_it() {
        // The cursor on the last word: `w` would clamp onto its last grapheme, and a naive
        // `cursor..target` would leave the first letters behind.
        assert_eq!(range("alpha beta", 6, word_forward(), 1), 6..10);
        assert_eq!(range("alpha beta", 6, word_forward(), 2), 6..10);
        // The cursor on the first of two words: the blank goes, the second word stays.
        assert_eq!(range("ab c", 0, word_forward(), 1), 0..3);
        // A line break is not part of the word: `dw` leaves it there.
        assert_eq!(range("alpha\nbeta", 0, word_forward(), 1), 0..5);
        assert_eq!(range("alpha\nbeta", 0, word_forward(), 2), 0..10);
        // A `w` that runs out of the text still takes what is left.
        assert_eq!(range("alpha", 0, word_forward(), 1), 0..5);
        assert_eq!(range("alpha\n", 0, word_forward(), 1), 0..5);
        assert_eq!(range("", 0, word_forward(), 1), 0..0);
        assert_eq!(range(" ", 0, word_forward(), 1), 0..1);
    }

    /// `de`, `d$` and `df` include the grapheme they land on; `d$` stops before the break.
    #[test]
    fn inclusive_motions_take_their_target() {
        let text = "alpha beta";
        assert_eq!(range(text, 0, Motion::WordEnd { big: false }, 1), 0..5);
        assert_eq!(range(text, 0, Motion::WordEnd { big: false }, 2), 0..10);
        assert_eq!(range(text, 6, Motion::LineEnd, 1), 6..10);
        // `$` on a line with a break after it never takes the break.
        assert_eq!(range("alpha\nbeta", 0, Motion::LineEnd, 1), 0..5);
        // A count takes further lines, ending before the last line's break.
        assert_eq!(range("ab\ncd\nef", 0, Motion::LineEnd, 2), 0..5);
        let find = Motion::Find {
            ch: 't',
            till: false,
            back: false,
        };
        assert_eq!(range(text, 6, find, 1), 6..9, "df includes the target");
        // A find that finds nothing has no range: the operator writes nothing.
        let missing = Motion::Find {
            ch: 'z',
            till: false,
            back: false,
        };
        assert_eq!(motion_range(text, 6, missing, 1, None), None);
        // A `t` stops before its target, so the operator covers what it passed over.
        let till = Motion::Find {
            ch: 'a',
            till: true,
            back: false,
        };
        // `dta` takes the text before the `a`, which is its `t` stop included.
        assert_eq!(range(text, 6, till, 1), 6..9);
        // A find to the left includes the target and not the cursor's grapheme.
        let back = Motion::Find {
            ch: 'b',
            till: false,
            back: true,
        };
        assert_eq!(range(text, 10, back, 1), 6..9);
    }

    /// `;` and `,` reuse the last find, in the direction the repeat names.
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

    /// The block-crossing motions are the host's: an operator gets no range for them.
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

    /// The offsets are grapheme boundaries on text a careless implementation splits.
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
