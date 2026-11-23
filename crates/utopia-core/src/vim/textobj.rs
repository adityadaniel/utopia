//! Vim text objects (docs/vim-mode-plan.md 1A.4): the range an operator acts on.
//!
//! [`range_of`] answers `None` when the cursor is not inside such an object, so an operator
//! with an object that is not there writes nothing. Every end of the range is a grapheme
//! boundary of `text`, derived from `grapheme_indices` and never from character arithmetic,
//! so the caller may slice with it (rule 11).

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::motion::{class_at, grapheme_is, line_end_exclusive, line_start, snap, Class};
use super::TextObject;
use crate::text::{next_grapheme, prev_grapheme};

/// The range an operator acts on, or `None` when the cursor is not in such an object.
pub fn range_of(text: &str, offset: usize, obj: TextObject) -> Option<Range<usize>> {
    match obj {
        TextObject::Word { big, around } => word_range(text, offset, big, around),
        TextObject::Bracket { open, around } => bracket_range(text, offset, open, around),
        TextObject::Quote { ch, around } => quote_range(text, offset, ch, around),
        // `il` and `al` range over a Utopia reference, from `inline::extract_refs` (PR 4).
        TextObject::Ref { .. } => None,
    }
}

/// The run of one character class under the cursor, plus the blanks after it for `a`
/// (or the blanks before it, when the word ends the line).
fn word_range(text: &str, offset: usize, big: bool, around: bool) -> Option<Range<usize>> {
    let offset = snap(text, offset);
    let class = class_at(text, offset, big)?;
    let mut start = offset;
    while start > 0 {
        let previous = prev_grapheme(text, start);
        if class_at(text, previous, big) != Some(class) {
            break;
        }
        start = previous;
    }
    let mut end = next_grapheme(text, offset);
    while class_at(text, end, big) == Some(class) {
        end = next_grapheme(text, end);
    }
    if around {
        let word_end = end;
        while class_at(text, end, big) == Some(Class::Blank) {
            end = next_grapheme(text, end);
        }
        if end == word_end {
            // Nothing after the word: take the blanks in front of it instead.
            while start > 0 {
                let previous = prev_grapheme(text, start);
                if class_at(text, previous, big) != Some(Class::Blank) {
                    break;
                }
                start = previous;
            }
        }
    }
    (start < end).then_some(start..end)
}

/// The innermost `open`…`close` pair holding the cursor, found the way Vim's `current_block`
/// finds it: back for the opening bracket that nothing closes, then on for its partner.
fn bracket_range(text: &str, offset: usize, open: char, around: bool) -> Option<Range<usize>> {
    let close = closing_of(open)?;
    let offset = snap(text, offset);
    let bounds: Vec<(usize, usize)> = text
        .grapheme_indices(true)
        .map(|(i, g)| (i, i + g.len()))
        .collect();
    if bounds.is_empty() {
        return None;
    }
    let chars: Vec<Option<char>> = text
        .grapheme_indices(true)
        .map(|(_, g)| single_char(g))
        .collect();
    // The grapheme the cursor is on. Past the last one, the last grapheme stands in for it.
    let cursor = bounds
        .partition_point(|(_, end)| *end <= offset)
        .min(bounds.len() - 1);
    // On the closing bracket, the cursor counts as inside the pair it closes: that is what
    // a reader means by `di(` there, and what Vim's findmatch does.
    let scan_from = if chars[cursor] == Some(close) {
        cursor.saturating_sub(1)
    } else {
        cursor
    };
    let mut depth = 0usize;
    let mut open_at = None;
    for (i, c) in chars.iter().enumerate().take(scan_from + 1).rev() {
        match *c {
            Some(c) if c == close => depth = depth.saturating_add(1),
            Some(c) if c == open => {
                if depth == 0 {
                    open_at = Some(i);
                    break;
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    let open_at = open_at?;
    let mut depth = 0usize;
    let mut close_at = None;
    for (i, c) in chars.iter().enumerate().skip(open_at) {
        match *c {
            Some(c) if c == open => depth = depth.saturating_add(1),
            Some(c) if c == close => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    close_at = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close_at = close_at?;
    if close_at <= open_at {
        return None;
    }
    let (start, open_end) = bounds[open_at];
    let (close_start, end) = bounds[close_at];
    Some(if around {
        start..end
    } else {
        open_end..close_start
    })
}

/// The quote pair on the cursor's line, taken in order, that the cursor is in or before.
fn quote_range(text: &str, offset: usize, ch: char, around: bool) -> Option<Range<usize>> {
    let offset = snap(text, offset);
    let start = line_start(text, offset);
    let end = line_end_exclusive(text, offset);
    if start >= end {
        return None;
    }
    let mut quotes: Vec<(usize, usize)> = Vec::new();
    for (i, g) in text[start..end].grapheme_indices(true) {
        if grapheme_is(g, ch) {
            quotes.push((start + i, start + i + g.len()));
        }
    }
    for pair in quotes.chunks_exact(2) {
        let (open_start, open_end) = pair[0];
        let (close_start, close_end) = pair[1];
        if offset <= close_start {
            return Some(if around {
                open_start..close_end
            } else {
                open_end..close_start
            });
        }
    }
    None
}

fn closing_of(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// The character of a grapheme that is exactly one character (`(`, `"`, …).
fn single_char(grapheme: &str) -> Option<char> {
    let mut chars = grapheme.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `offset` is a grapheme boundary of `text`.
    fn is_grapheme_boundary(text: &str, offset: usize) -> bool {
        offset == text.len() || text.grapheme_indices(true).any(|(i, _)| i == offset)
    }

    fn word(big: bool, around: bool) -> TextObject {
        TextObject::Word { big, around }
    }

    fn bracket(open: char, around: bool) -> TextObject {
        TextObject::Bracket { open, around }
    }

    fn quote(ch: char, around: bool) -> TextObject {
        TextObject::Quote { ch, around }
    }

    /// A range must sit on grapheme boundaries and inside the text.
    fn check(text: &str, range: Option<Range<usize>>) -> Range<usize> {
        let range = range.expect("no range");
        assert!(
            range.start <= range.end && range.end <= text.len(),
            "{text:?}: {range:?} is out of bounds"
        );
        for offset in [range.start, range.end] {
            assert!(
                offset == text.len() || is_grapheme_boundary(text, offset),
                "{text:?}: {range:?} splits a grapheme"
            );
        }
        range
    }

    /// `iw` is the run under the cursor; `aw` takes the blanks after it, or before it when
    /// there are none after.
    #[test]
    fn the_word_under_the_cursor() {
        let text = "hello world";
        assert_eq!(check(text, range_of(text, 1, word(false, false))), 0..5);
        assert_eq!(check(text, range_of(text, 0, word(false, false))), 0..5);
        assert_eq!(check(text, range_of(text, 8, word(false, false))), 6..11);
        assert_eq!(check(text, range_of(text, 0, word(false, true))), 0..6);
        // The last word has no blanks after it, so `aw` takes the ones before.
        assert_eq!(check(text, range_of(text, 8, word(false, true))), 5..11);
        // Punctuation is a word of its own, and `_` belongs to the word around it.
        let text = "a-b_c";
        assert_eq!(check(text, range_of(text, 1, word(false, false))), 1..2);
        assert_eq!(check(text, range_of(text, 3, word(false, false))), 2..5);
        assert_eq!(check(text, range_of(text, 0, word(true, false))), 0..5);
        // Blanks are a run of their own.
        let text = "a  b";
        assert_eq!(check(text, range_of(text, 1, word(false, false))), 1..3);
        assert_eq!(check(text, range_of(text, 3, word(false, true))), 1..4);
        // A grapheme with a combining mark is never split.
        let text = "e\u{301}cole two";
        assert_eq!(check(text, range_of(text, 0, word(false, false))), 0..7);
        // A family emoji is one grapheme and one run.
        let text = "👨‍👩‍👦 cat";
        assert_eq!(check(text, range_of(text, 0, word(false, false))), 0..18);
        let text = "日本語 text";
        assert_eq!(check(text, range_of(text, 0, word(false, false))), 0..9);
        // No word at the end, or in an empty block.
        assert_eq!(range_of("", 0, word(false, false)), None);
        assert_eq!(range_of("abc", 3, word(false, false)), None);
        assert_eq!(range_of(" ", 0, word(false, true)), Some(0..1));
    }

    /// Brackets count nesting and take the innermost pair holding the cursor.
    #[test]
    fn brackets_count_nesting() {
        let text = "f(a(bb)c)d";
        assert_eq!(check(text, range_of(text, 5, bracket('(', false))), 4..6);
        assert_eq!(check(text, range_of(text, 5, bracket('(', true))), 3..7);
        assert_eq!(check(text, range_of(text, 2, bracket('(', false))), 2..8);
        assert_eq!(check(text, range_of(text, 2, bracket('(', true))), 1..9);
        // The opening bracket itself is inside its pair, and so is the closing one.
        assert_eq!(check(text, range_of(text, 1, bracket('(', false))), 2..8);
        assert_eq!(check(text, range_of(text, 6, bracket('(', false))), 4..6);
        // An empty pair is a range of no width.
        assert_eq!(check("()", range_of("()", 0, bracket('(', false))), 1..1);
        assert_eq!(check("()", range_of("()", 1, bracket('(', true))), 0..2);
        // The other two pairs, and a bracket with no partner.
        let text = "[a {b} c]";
        assert_eq!(check(text, range_of(text, 4, bracket('{', false))), 4..5);
        assert_eq!(check(text, range_of(text, 1, bracket('[', true))), 0..9);
        assert_eq!(range_of("f(a", 2, bracket('(', false)), None, "unbalanced");
        assert_eq!(range_of("abc", 1, bracket('(', false)), None, "no bracket");
        assert_eq!(range_of("", 0, bracket('(', false)), None, "empty");
        assert_eq!(
            range_of("f(x", 1, bracket('[', false)),
            None,
            "the wrong pair"
        );
        // A bracket character is one grapheme; a full-width one is not a bracket.
        let text = "(日 本)";
        assert_eq!(check(text, range_of(text, 4, bracket('(', false))), 1..8);
    }

    /// Quotes pair up along the cursor's line only.
    #[test]
    fn quotes_pair_on_the_line() {
        let text = "say \"hi there\" now";
        assert_eq!(check(text, range_of(text, 6, quote('"', false))), 5..13);
        assert_eq!(check(text, range_of(text, 6, quote('"', true))), 4..14);
        // Before the pair, on either quote, and after it.
        assert_eq!(check(text, range_of(text, 0, quote('"', false))), 5..13);
        assert_eq!(check(text, range_of(text, 4, quote('"', false))), 5..13);
        assert_eq!(check(text, range_of(text, 13, quote('"', false))), 5..13);
        assert_eq!(range_of(text, 16, quote('"', false)), None);
        // The second pair on the line is the one a cursor in it gets.
        let text = "\"a\" \"b\"";
        assert_eq!(check(text, range_of(text, 5, quote('"', false))), 5..6);
        // A pair that spans a line break is not on the cursor's line, so there is none.
        let text = "a \"b\nc\"";
        assert_eq!(range_of(text, 5, quote('"', false)), None);
        assert_eq!(range_of(text, 3, quote('"', false)), None);
        // An empty pair, the other two quote characters, and an empty block.
        assert_eq!(check("\"\"", range_of("\"\"", 0, quote('"', false))), 1..1);
        assert_eq!(check("'x'", range_of("'x'", 1, quote('\'', false))), 1..2);
        assert_eq!(check("`co`", range_of("`co`", 2, quote('`', false))), 1..3);
        assert_eq!(range_of("", 0, quote('"', false)), None);
    }

    /// `il` and `al` are PR 4: the grammar knows them, there is no range yet.
    #[test]
    fn references_have_no_range_yet() {
        let text = "see [[Alpha]] now";
        assert_eq!(range_of(text, 6, TextObject::Ref { around: false }), None);
        assert_eq!(range_of(text, 6, TextObject::Ref { around: true }), None);
    }
}
