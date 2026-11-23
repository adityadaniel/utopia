//! Vim motions on one block's text (docs/vim-mode-plan.md 1A.3, VM-6).
//!
//! Three character classes, as Vim has them: **word** characters (alphanumeric plus `_`),
//! **punctuation** (any other non-blank) and **blank** (whitespace). `w`, `b` and `e` stop
//! at the start of the next word *or* of the next run of punctuation; `W`, `B` and `E` fold
//! punctuation into word, so only blanks separate.
//!
//! [`crate::text::prev_word_start`] and [`crate::text::next_word_end`] are the Emacs rules
//! (they skip punctuation) and stay as they are: Alt+Left and Alt+Right use them.
//!
//! Every function takes a byte offset into `text` and returns one. The offset is snapped
//! down to a grapheme boundary on the way in and every value returned is derived from a
//! grapheme, so the caller may slice with it (§10.5, rule 11). A motion never runs past
//! the block's text, and in Normal mode never past the last grapheme of a line, because
//! that is where the block cursor rests (1E.3).

use unicode_segmentation::UnicodeSegmentation;

use crate::text::{floor_char_boundary, next_grapheme, prev_grapheme};

/// Vim's three character classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Blank,
    Word,
    Punct,
}

impl Class {
    /// The class as `big` groups it: `W`/`B`/`E` treat punctuation as part of a word.
    fn folded(self, big: bool) -> Class {
        if big && self == Class::Punct {
            Class::Word
        } else {
            self
        }
    }
}

/// The class of one grapheme, from its first character (`e` + a combining acute is a word
/// character, a lone combining mark is punctuation).
pub(crate) fn class_of(grapheme: &str) -> Class {
    match grapheme.chars().next() {
        None => Class::Blank,
        Some(c) if c.is_whitespace() => Class::Blank,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        Some(_) => Class::Punct,
    }
}

/// The class of the grapheme at `offset`, folded by `big`; `None` at the end of the text.
pub(crate) fn class_at(text: &str, offset: usize, big: bool) -> Option<Class> {
    text.get(offset..)?
        .graphemes(true)
        .next()
        .map(|g| class_of(g).folded(big))
}

/// Whether one grapheme is exactly the character `ch` (a `f`/`t` target, a bracket, a quote).
pub(crate) fn grapheme_is(grapheme: &str, ch: char) -> bool {
    grapheme.len() == ch.len_utf8() && grapheme.starts_with(ch)
}

/// Snap `offset` down to the nearest grapheme boundary, never past the end of the text.
///
/// A grapheme boundary is also a char boundary, so every slice below is safe.
pub(crate) fn snap(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    if offset >= text.len() {
        return text.len();
    }
    let mut end = 0;
    for (i, g) in text.grapheme_indices(true) {
        if i + g.len() > offset {
            return i;
        }
        end = i + g.len();
    }
    end
}

/// The byte index of the line break that ends the line starting at `from`, or the end of
/// the text when that line is the last one. A break is whole grapheme (`\r\n` is one).
fn line_break_at(text: &str, from: usize) -> usize {
    if from >= text.len() {
        return text.len();
    }
    for (i, g) in text[from..].grapheme_indices(true) {
        if g.contains('\n') {
            return from + i;
        }
    }
    text.len()
}

fn is_blank(text: &str, offset: usize) -> bool {
    class_at(text, offset, false) == Some(Class::Blank)
}

/// Start of the line holding `offset` (Vim's `0`).
pub fn line_start(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    let mut start = 0;
    for (i, g) in text[..offset].grapheme_indices(true) {
        if g.contains('\n') {
            start = i + g.len();
        }
    }
    start
}

/// The last grapheme of the line, not one past it (Normal clamp, Vim's `$` as a motion).
pub fn line_end_inclusive(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    let start = line_start(text, offset);
    let end = line_break_at(text, start);
    if end <= start {
        // An empty line: the cursor sits where the line is.
        start
    } else {
        prev_grapheme(text, end)
    }
}

/// One past the last grapheme of the line, for `$` used as an operator target (`d$`
/// takes the line's text, not its line break).
pub fn line_end_exclusive(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    line_break_at(text, line_start(text, offset))
}

/// First non-blank of the line (Vim's `^`); the last grapheme when the line is all blanks.
pub fn line_first_non_blank(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    let start = line_start(text, offset);
    let end = line_break_at(text, start);
    let mut i = start;
    while i < end && is_blank(text, i) {
        i = next_grapheme(text, i);
    }
    if i >= end {
        line_end_inclusive(text, start)
    } else {
        i
    }
}

/// Keeps the cursor on a grapheme and never past the last one of its line (1E.3): an empty
/// string has it at 0, and a position on a line break comes back to the line's last
/// grapheme, as Vim's does.
pub fn clamp_normal(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    let end = line_end_inclusive(text, offset);
    if offset > end {
        end
    } else {
        offset
    }
}

/// Start of the next word, or of the next run of punctuation (Vim's `w`).
///
/// The value is clamped to the Normal cursor's resting place (the line's last grapheme),
/// because that is where a motion puts the block cursor. An operator needs the unclamped
/// one instead: see [`word_forward_raw`].
pub fn word_forward(text: &str, offset: usize, big: bool) -> usize {
    clamp_normal(text, word_forward_raw(text, offset, big))
}

/// Start of the next word before the Normal clamp: one past the text when the words run
/// out, or a line's first grapheme. `dw` takes the text up to it, and Vim's exclusive rule
/// turns an end that reaches column 1 back into the previous line's end, so the operator
/// needs the end the motion *wanted*, not the one the cursor would rest on.
pub(crate) fn word_forward_raw(text: &str, offset: usize, big: bool) -> usize {
    let start = snap(text, offset);
    let Some(class) = class_at(text, start, big) else {
        return text.len();
    };
    let mut i = start;
    // The run the cursor is in (a blank run too: `w` leaves the blanks behind it).
    while class_at(text, i, big) == Some(class) {
        i = next_grapheme(text, i);
    }
    // Then the blanks that follow a word, which `w` never stops on.
    while is_blank(text, i) {
        i = next_grapheme(text, i);
    }
    i
}

/// Start of the previous word, or of the previous run of punctuation (Vim's `b`).
pub fn word_back(text: &str, offset: usize, big: bool) -> usize {
    let start = snap(text, offset);
    if start == 0 {
        return 0;
    }
    let mut i = prev_grapheme(text, start);
    while i > 0 && class_at(text, i, big) == Some(Class::Blank) {
        i = prev_grapheme(text, i);
    }
    let Some(class) = class_at(text, i, big) else {
        return i;
    };
    if class == Class::Blank {
        // The text's first characters are blanks: there is no word before the cursor.
        return i;
    }
    while i > 0 {
        let previous = prev_grapheme(text, i);
        if class_at(text, previous, big) != Some(class) {
            break;
        }
        i = previous;
    }
    i
}

/// The last grapheme of the word the cursor is in or of the next one (Vim's `e`).
pub fn word_end(text: &str, offset: usize, big: bool) -> usize {
    let start = snap(text, offset);
    if start >= text.len() {
        return clamp_normal(text, start);
    }
    let mut i = next_grapheme(text, start);
    while is_blank(text, i) {
        i = next_grapheme(text, i);
    }
    let Some(class) = class_at(text, i, big) else {
        return clamp_normal(text, i);
    };
    let mut last = i;
    while class_at(text, i, big) == Some(class) {
        last = i;
        i = next_grapheme(text, i);
    }
    clamp_normal(text, last)
}

/// The character `ch` on the cursor's line, right of the cursor (`f`, `t`) or left of it
/// (`F`, `T`), or `None` when it is not there.
///
/// With `till` the cursor stops one grapheme short, and an adjacent target leaves it where
/// it is (Vim does not move either).
pub fn find_char(text: &str, offset: usize, ch: char, till: bool, back: bool) -> Option<usize> {
    find_char_nth(text, offset, ch, till, back, 1)
}

/// The `count`th `ch` away from the cursor on its line (`2fx`, `2tx`), or `None` when the
/// line does not hold that many.
///
/// The targets are counted first and the `till` adjustment is applied once, to the one
/// that was found. Counting stops instead of targets would make `2tx` stop on the first
/// `x` (`a x x z`: the space before the second `x` is byte 3, not byte 1).
pub fn find_char_nth(
    text: &str,
    offset: usize,
    ch: char,
    till: bool,
    back: bool,
    count: usize,
) -> Option<usize> {
    let offset = snap(text, offset);
    let target = nth_target(text, offset, ch, back, count)?;
    till_stop(text, offset, target, till, back)
}

/// `;` and `,`: the next find in the same or the opposite direction, `count` times.
///
/// A `t`/`T` leaves the cursor one grapheme short of its target, so a repeat that looked
/// for the nearest target again would stop on the one it already stands next to. It
/// passes those instead: `tx` then `;` in `a x x z` lands on the space before the second
/// `x`, as Vim's does.
pub fn repeat_find(
    text: &str,
    offset: usize,
    ch: char,
    till: bool,
    back: bool,
    count: usize,
) -> Option<usize> {
    let offset = snap(text, offset);
    let count = count.max(1);
    let mut seen = 0;
    let mut n = 0;
    while let Some(target) = nth_target(text, offset, ch, back, n + 1) {
        n += 1;
        let Some(stop) = till_stop(text, offset, target, till, back) else {
            // The cursor already stands next to this target: it is spent.
            continue;
        };
        seen += 1;
        if seen == count {
            return Some(stop);
        }
    }
    None
}

/// The offset of the `count`th occurrence of `ch` in the direction of travel on the
/// cursor's line, the nearest one first.
fn nth_target(text: &str, offset: usize, ch: char, back: bool, count: usize) -> Option<usize> {
    let offset = snap(text, offset);
    let line_start = line_start(text, offset);
    let line_end = line_break_at(text, line_start);
    let count = count.max(1);
    let mut seen = 0;
    if back {
        // Right to left, so the nearest occurrence comes first.
        for (i, g) in text[line_start..offset].grapheme_indices(true).rev() {
            if grapheme_is(g, ch) {
                seen += 1;
                if seen == count {
                    return Some(line_start + i);
                }
            }
        }
        None
    } else {
        for (i, g) in text[offset..line_end].grapheme_indices(true) {
            // `i == 0` is the cursor's own grapheme: `f` searches past it.
            if i > 0 && grapheme_is(g, ch) {
                seen += 1;
                if seen == count {
                    return Some(offset + i);
                }
            }
        }
        None
    }
}

/// Where the cursor comes to rest for `target`: on it (`f`, `F`), or one grapheme short of
/// it (`t`, `T`). `None` when the `till` stop would not move in the direction of travel.
fn till_stop(text: &str, offset: usize, target: usize, till: bool, back: bool) -> Option<usize> {
    if !till {
        return Some(target);
    }
    let stop = if back {
        next_grapheme(text, target)
    } else {
        prev_grapheme(text, target)
    };
    if back {
        (stop < offset).then_some(stop)
    } else {
        (stop > offset).then_some(stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text a careless implementation splits: a family emoji (one grapheme, four code
    /// points, 18 bytes), a combining acute, CJK, a line break.
    const HARD: &[&str] = &[
        "",
        " ",
        "   ",
        "👨‍👩‍👦 cat",
        "e\u{301}cole",
        "日本語 text",
        "ab\ncd",
        "\n",
        "a\u{301}",
        "a b",
        "a-b_c.d",
        "\t x",
        "a\r\nb",
    ];

    fn boundaries(text: &str) -> Vec<usize> {
        let mut out: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        out.push(text.len());
        out
    }

    fn is_boundary(text: &str, offset: usize) -> bool {
        boundaries(text).contains(&offset)
    }

    fn slice(text: &str, offset: usize) -> &str {
        // A panic here means an offset was not a char boundary.
        &text[offset..]
    }

    /// `w` on "👨‍👩‍👦 cat": the cluster is punctuation, so `w` steps over it and the blank.
    #[test]
    fn the_family_emoji_is_one_grapheme() {
        let text = "👨‍👩‍👦 cat";
        let family = "👨‍👩‍👦".len();
        assert_eq!(family, 18);
        let cat = word_forward(text, 0, false);
        assert_eq!(cat, family + 1);
        assert_eq!(slice(text, cat), "cat");
        // `b` from the end of the text comes back to the word, then to the cluster.
        assert_eq!(word_back(text, text.len(), false), cat);
        assert_eq!(word_back(text, cat, false), 0);
        // `e` lands on the last grapheme of "cat", not past it.
        assert_eq!(word_end(text, 0, false), text.len() - 1);
        // Nothing split the cluster.
        for offset in [cat, word_back(text, text.len(), false)] {
            assert!(is_boundary(text, offset), "{offset} splits a grapheme");
        }
        // The same motions with `big`: punctuation is a word, so `W` skips to "cat" too.
        assert_eq!(word_forward(text, 0, true), cat);
    }

    /// "e" + U+0301 is one grapheme of three bytes, and a lone combining mark is a grapheme
    /// of its own.
    #[test]
    fn a_combining_mark_is_part_of_its_grapheme() {
        let text = "e\u{301}cole two";
        assert_eq!(next_grapheme(text, 0), 3, "the accent belongs to the e");
        assert_eq!(word_forward(text, 0, false), 8);
        assert_eq!(slice(text, 8), "two");
        assert_eq!(word_back(text, text.len(), false), 8);
        assert_eq!(word_end(text, 0, false), 6);
        assert_eq!(slice(text, 6), "e two");
        assert_eq!(clamp_normal(text, text.len()), text.len() - 1);
        // A lone combining mark is punctuation, and the text before it is one word.
        let lone = "a\u{301}b";
        assert_eq!(word_forward(lone, 0, false), 3, "the end of the only word");
        assert!(is_boundary(lone, word_end(lone, 0, false)));
    }

    /// CJK characters are alphanumeric, so a run of them is one word.
    #[test]
    fn cjk_is_one_word() {
        let text = "日本語 text";
        assert_eq!(word_forward(text, 0, false), 10);
        assert_eq!(slice(text, 10), "text");
        // `e` lands on the last character of the run, which is three bytes wide.
        assert_eq!(word_end(text, 0, false), 6);
        assert_eq!(slice(text, 6), "語 text");
        assert_eq!(word_back(text, 10, false), 0);
        assert_eq!(line_end_inclusive(text, 0), text.len() - 1);
    }

    /// An empty string answers 0 everywhere and finds nothing.
    #[test]
    fn the_empty_string_is_safe() {
        assert_eq!(clamp_normal("", 0), 0);
        assert_eq!(clamp_normal("", 99), 0);
        assert_eq!(word_forward("", 0, false), 0);
        assert_eq!(word_back("", 0, false), 0);
        assert_eq!(word_end("", 0, false), 0);
        assert_eq!(line_start("", 5), 0);
        assert_eq!(line_end_inclusive("", 0), 0);
        assert_eq!(line_end_exclusive("", 0), 0);
        assert_eq!(line_first_non_blank("", 0), 0);
        assert_eq!(find_char("", 0, 'a', false, false), None);
        assert_eq!(find_char("", 0, 'a', true, true), None);
    }

    /// A string of only blanks never runs off either end.
    #[test]
    fn only_blanks() {
        let text = "   ";
        assert_eq!(
            word_forward(text, 0, false),
            2,
            "the clamp keeps it on a grapheme"
        );
        assert_eq!(word_back(text, 2, false), 0);
        assert_eq!(word_end(text, 0, false), 2);
        assert_eq!(line_start(text, 2), 0);
        assert_eq!(line_end_inclusive(text, 0), 2);
        assert_eq!(line_end_exclusive(text, 0), 3);
        assert_eq!(line_first_non_blank(text, 2), 2);
        assert_eq!(clamp_normal(text, 3), 2);
    }

    /// A line break is not a position in Normal mode, and a line is per block.
    #[test]
    fn lines_break_on_the_newline() {
        let text = "ab\ncd";
        assert_eq!(line_start(text, 0), 0);
        assert_eq!(line_start(text, 3), 3);
        assert_eq!(line_start(text, 5), 3);
        assert_eq!(line_end_inclusive(text, 0), 1);
        assert_eq!(line_end_inclusive(text, 3), 4);
        assert_eq!(line_end_exclusive(text, 0), 2);
        assert_eq!(line_end_exclusive(text, 3), 5);
        // The cursor cannot rest on the line break itself.
        assert_eq!(clamp_normal(text, 2), 1);
        // A blank-only second line has the cursor at its start, not on the break.
        let text = "a\n  ";
        assert_eq!(line_first_non_blank(text, 2), 3);
        assert_eq!(clamp_normal(text, 4), 3);
        // An empty line is a position.
        let text = "a\n";
        assert_eq!(line_start(text, 2), 2);
        assert_eq!(line_end_inclusive(text, 2), 2);
        assert_eq!(clamp_normal(text, 2), 2);
        // A CRLF is one grapheme, so it is never cut in half.
        let text = "a\r\nb";
        assert_eq!(line_end_inclusive(text, 0), 0);
        assert_eq!(line_start(text, 3), 3);
        assert_eq!(clamp_normal(text, 1), 0);
        // `$` and `^` inside a paragraph.
        let text = "  one\ntwo  ";
        assert_eq!(line_first_non_blank(text, 0), 2);
        assert_eq!(line_first_non_blank(text, 2), 2);
        assert_eq!(line_end_inclusive(text, 0), 4);
        assert_eq!(line_first_non_blank(text, 6), 6);
        assert_eq!(line_end_inclusive(text, 6), 10);
    }

    /// Punctuation is its own word for `w`/`b`/`e` and part of the word for `W`/`B`/`E`.
    #[test]
    fn punctuation_is_a_word_of_its_own() {
        let text = "a-b_c.d";
        // `w` stops at every run: "-", "b_c", ".", then the end of the block.
        let mut stops = Vec::new();
        let mut offset = 0;
        for _ in 0..4 {
            offset = word_forward(text, offset, false);
            stops.push(offset);
        }
        assert_eq!(stops, vec![1, 2, 5, 6]);
        // `W` sees one word, and stops on its last grapheme (the block's edge).
        assert_eq!(word_forward(text, 0, true), 6);
        assert_eq!(word_back(text, 7, true), 0);
        assert_eq!(word_end(text, 0, true), 6);
        // `e` stops at the end of each run, starting with the next one.
        assert_eq!(word_end(text, 0, false), 1);
        assert_eq!(word_end(text, 2, false), 4);
        assert_eq!(word_end(text, 5, false), 6);
        // `b` from the last word reaches the `.`, then the run before it.
        assert_eq!(word_back(text, 6, false), 5);
        assert_eq!(word_back(text, 5, false), 2);
        assert_eq!(word_back(text, 4, true), 0);
    }

    /// A count counts targets and a repeat passes the target it stands next to (F6):
    /// `2tx` and `tx` then `;` each reach the second `x`, and so do `2Tx` and `Tx` then
    /// `,`.
    #[test]
    fn a_count_or_a_repeat_passes_the_first_target() {
        let text = "a x x z";
        // `2tx`: two targets, the `till` adjustment applied once, to the second.
        assert_eq!(find_char_nth(text, 0, 'x', true, false, 2), Some(3));
        // `tx` alone stops on the space before the first `x`; `;` then passes it.
        assert_eq!(find_char(text, 0, 'x', true, false), Some(1));
        assert_eq!(repeat_find(text, 1, 'x', true, false, 1), Some(3));
        // The same pair backwards: `2Tx` from the last character, and `Tx` then `,`
        // (which repeats the last find in the opposite direction).
        let text = "x a x b x";
        assert_eq!(find_char_nth(text, 8, 'x', true, true, 2), Some(1));
        assert_eq!(find_char(text, 8, 'x', true, true), Some(5));
        assert_eq!(repeat_find(text, 5, 'x', true, false, 1), Some(7));
        // A count with fewer targets than it asks for moves nothing.
        assert_eq!(find_char_nth(text, 8, 'x', false, true, 3), None);
    }

    /// `f`, `t`, `F` and `T`: the cursor's line only, and `t` never lands on the target.
    #[test]
    fn find_char_stays_on_the_line() {
        let text = "a bc bd";
        assert_eq!(find_char(text, 0, 'b', false, false), Some(2));
        assert_eq!(find_char(text, 0, 'b', true, false), Some(1));
        assert_eq!(find_char(text, 0, 'd', false, false), Some(6));
        assert_eq!(find_char(text, 6, 'a', false, true), Some(0));
        assert_eq!(find_char(text, 6, 'b', false, true), Some(5));
        assert_eq!(find_char(text, 6, 'a', true, true), Some(1));
        // Nothing there, an adjacent `t`/`T`, and the wrong line: no move.
        assert_eq!(find_char(text, 0, 'z', false, false), None);
        assert_eq!(find_char(text, 1, 'b', true, false), None);
        assert_eq!(find_char(text, 1, 'a', true, true), None);
        let text = "ab\ncd";
        assert_eq!(find_char(text, 0, 'd', false, false), None);
        assert_eq!(find_char(text, 3, 'a', false, true), None);
        // A multi-byte target is found by its grapheme.
        let text = "日本語";
        assert_eq!(find_char(text, 0, '語', false, false), Some(6));
        // A grapheme is never cut: a target with a combining mark is not the bare letter.
        let text = "e\u{301}e";
        assert_eq!(find_char(text, 0, 'e', false, false), Some(3));
    }

    /// Every motion, on every hard input and from every grapheme boundary, returns a
    /// grapheme boundary inside the text (rule 11, 1A.6).
    #[test]
    fn every_motion_lands_on_a_grapheme_boundary() {
        for text in HARD {
            for big in [false, true] {
                for offset in boundaries(text) {
                    // Offsets inside a grapheme, and past the end, are snapped first.
                    for input in [offset, offset + 1, offset + 7, text.len() + 5] {
                        let results = [
                            word_forward(text, input, big),
                            word_back(text, input, big),
                            word_end(text, input, big),
                            line_start(text, input),
                            line_end_inclusive(text, input),
                            line_end_exclusive(text, input),
                            line_first_non_blank(text, input),
                            clamp_normal(text, input),
                        ];
                        for result in results {
                            assert!(
                                result <= text.len() && is_boundary(text, result),
                                "{text:?} at {input}: {result} is not a grapheme boundary"
                            );
                            let _ = slice(text, result);
                        }
                    }
                    for ch in ['a', ' ', '\n', 'あ', '\u{301}', '👩'] {
                        for till in [false, true] {
                            for back in [false, true] {
                                if let Some(result) = find_char(text, offset, ch, till, back) {
                                    assert!(
                                        result <= text.len() && is_boundary(text, result),
                                        "{text:?} finding {ch:?} at {offset}: {result}"
                                    );
                                    let _ = slice(text, result);
                                }
                            }
                        }
                    }
                    // A find never crosses the cursor's line and never moves to the cursor.
                    for ch in ['a', 'b', ' '] {
                        if let Some(result) = find_char(text, offset, ch, false, false) {
                            assert!(
                                result > offset,
                                "{text:?} at {offset} found {ch:?} at {result}"
                            );
                            assert_eq!(line_start(text, result), line_start(text, offset));
                        }
                        if let Some(result) = find_char(text, offset, ch, false, true) {
                            assert!(
                                result < offset,
                                "{text:?} at {offset} found {ch:?} at {result}"
                            );
                            assert_eq!(line_start(text, result), line_start(text, offset));
                        }
                    }
                }
            }
        }
    }

    /// `clamp_normal` on an empty string is 0, and it never returns a non-boundary.
    #[test]
    fn clamp_normal_keeps_the_cursor_on_a_grapheme() {
        assert_eq!(clamp_normal("", 0), 0);
        let text = "a👨‍👩‍👦\u{301}b";
        for offset in 0..=text.len() + 3 {
            let result = clamp_normal(text, offset);
            assert!(is_boundary(text, result), "{offset} gave {result}");
            assert!(result <= text.len());
        }
        // At the end of the text the cursor stays on the last grapheme of its line.
        assert_eq!(clamp_normal(text, text.len()), text.len() - 1);
        assert_eq!(slice(text, text.len() - 1), "b");
    }

    /// Words never leave the block: at the last word `w` stays on the last grapheme.
    #[test]
    fn a_motion_stops_at_the_blocks_edge() {
        let text = "one";
        assert_eq!(word_forward(text, 0, false), 2);
        assert_eq!(word_forward(text, 1, false), 2);
        assert_eq!(word_forward(text, 2, false), 2);
        assert_eq!(word_end(text, 0, false), 2);
        assert_eq!(word_end(text, 2, false), 2);
        assert_eq!(word_back(text, 0, false), 0);
        // `0` and `$` stay inside the line they were called on.
        let text = "one two";
        assert_eq!(line_start(text, 5), 0);
        assert_eq!(line_end_inclusive(text, 0), 6);
        assert_eq!(line_end_exclusive(text, 0), 7);
    }
}
