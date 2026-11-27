use unicode_segmentation::UnicodeSegmentation;

use crate::text::{floor_char_boundary, next_grapheme, prev_grapheme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Blank,
    Word,
    Punct,
}

impl Class {
    fn folded(self, big: bool) -> Class {
        if big && self == Class::Punct {
            Class::Word
        } else {
            self
        }
    }
}

pub(crate) fn class_of(grapheme: &str) -> Class {
    match grapheme.chars().next() {
        None => Class::Blank,
        Some(c) if c.is_whitespace() => Class::Blank,
        Some(c) if c.is_alphanumeric() || c == '_' => Class::Word,
        Some(_) => Class::Punct,
    }
}

pub(crate) fn class_at(text: &str, offset: usize, big: bool) -> Option<Class> {
    text.get(offset..)?
        .graphemes(true)
        .next()
        .map(|g| class_of(g).folded(big))
}

pub(crate) fn grapheme_is(grapheme: &str, ch: char) -> bool {
    grapheme.len() == ch.len_utf8() && grapheme.starts_with(ch)
}

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

pub fn line_end_inclusive(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    let start = line_start(text, offset);
    let end = line_break_at(text, start);
    if end <= start {
        start
    } else {
        prev_grapheme(text, end)
    }
}

pub fn line_end_exclusive(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    line_break_at(text, line_start(text, offset))
}

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

pub fn clamp_normal(text: &str, offset: usize) -> usize {
    let offset = snap(text, offset);
    let end = line_end_inclusive(text, offset);
    if offset > end {
        end
    } else {
        offset
    }
}

pub fn word_forward(text: &str, offset: usize, big: bool) -> usize {
    clamp_normal(text, word_forward_raw(text, offset, big))
}

pub(crate) fn word_forward_raw(text: &str, offset: usize, big: bool) -> usize {
    let start = snap(text, offset);
    let Some(class) = class_at(text, start, big) else {
        return text.len();
    };
    let mut i = start;
    while class_at(text, i, big) == Some(class) {
        i = next_grapheme(text, i);
    }
    while is_blank(text, i) {
        i = next_grapheme(text, i);
    }
    i
}

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

pub fn find_char(text: &str, offset: usize, ch: char, till: bool, back: bool) -> Option<usize> {
    find_char_nth(text, offset, ch, till, back, 1)
}

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
            continue;
        };
        seen += 1;
        if seen == count {
            return Some(stop);
        }
    }
    None
}

fn nth_target(text: &str, offset: usize, ch: char, back: bool, count: usize) -> Option<usize> {
    let offset = snap(text, offset);
    let line_start = line_start(text, offset);
    let line_end = line_break_at(text, line_start);
    let count = count.max(1);
    let mut seen = 0;
    if back {
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
        &text[offset..]
    }

    #[test]
    fn the_family_emoji_is_one_grapheme() {
        let text = "👨‍👩‍👦 cat";
        let family = "👨‍👩‍👦".len();
        assert_eq!(family, 18);
        let cat = word_forward(text, 0, false);
        assert_eq!(cat, family + 1);
        assert_eq!(slice(text, cat), "cat");
        assert_eq!(word_back(text, text.len(), false), cat);
        assert_eq!(word_back(text, cat, false), 0);
        assert_eq!(word_end(text, 0, false), text.len() - 1);
        for offset in [cat, word_back(text, text.len(), false)] {
            assert!(is_boundary(text, offset), "{offset} splits a grapheme");
        }
        assert_eq!(word_forward(text, 0, true), cat);
    }

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
        let lone = "a\u{301}b";
        assert_eq!(word_forward(lone, 0, false), 3, "the end of the only word");
        assert!(is_boundary(lone, word_end(lone, 0, false)));
    }

    #[test]
    fn cjk_is_one_word() {
        let text = "日本語 text";
        assert_eq!(word_forward(text, 0, false), 10);
        assert_eq!(slice(text, 10), "text");
        assert_eq!(word_end(text, 0, false), 6);
        assert_eq!(slice(text, 6), "語 text");
        assert_eq!(word_back(text, 10, false), 0);
        assert_eq!(line_end_inclusive(text, 0), text.len() - 1);
    }

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
        assert_eq!(clamp_normal(text, 2), 1);
        let text = "a\n  ";
        assert_eq!(line_first_non_blank(text, 2), 3);
        assert_eq!(clamp_normal(text, 4), 3);
        let text = "a\n";
        assert_eq!(line_start(text, 2), 2);
        assert_eq!(line_end_inclusive(text, 2), 2);
        assert_eq!(clamp_normal(text, 2), 2);
        let text = "a\r\nb";
        assert_eq!(line_end_inclusive(text, 0), 0);
        assert_eq!(line_start(text, 3), 3);
        assert_eq!(clamp_normal(text, 1), 0);
        let text = "  one\ntwo  ";
        assert_eq!(line_first_non_blank(text, 0), 2);
        assert_eq!(line_first_non_blank(text, 2), 2);
        assert_eq!(line_end_inclusive(text, 0), 4);
        assert_eq!(line_first_non_blank(text, 6), 6);
        assert_eq!(line_end_inclusive(text, 6), 10);
    }

    #[test]
    fn punctuation_is_a_word_of_its_own() {
        let text = "a-b_c.d";
        let mut stops = Vec::new();
        let mut offset = 0;
        for _ in 0..4 {
            offset = word_forward(text, offset, false);
            stops.push(offset);
        }
        assert_eq!(stops, vec![1, 2, 5, 6]);
        assert_eq!(word_forward(text, 0, true), 6);
        assert_eq!(word_back(text, 7, true), 0);
        assert_eq!(word_end(text, 0, true), 6);
        assert_eq!(word_end(text, 0, false), 1);
        assert_eq!(word_end(text, 2, false), 4);
        assert_eq!(word_end(text, 5, false), 6);
        assert_eq!(word_back(text, 6, false), 5);
        assert_eq!(word_back(text, 5, false), 2);
        assert_eq!(word_back(text, 4, true), 0);
    }

    #[test]
    fn a_count_or_a_repeat_passes_the_first_target() {
        let text = "a x x z";
        assert_eq!(find_char_nth(text, 0, 'x', true, false, 2), Some(3));
        assert_eq!(find_char(text, 0, 'x', true, false), Some(1));
        assert_eq!(repeat_find(text, 1, 'x', true, false, 1), Some(3));
        let text = "x a x b x";
        assert_eq!(find_char_nth(text, 8, 'x', true, true, 2), Some(1));
        assert_eq!(find_char(text, 8, 'x', true, true), Some(5));
        assert_eq!(repeat_find(text, 5, 'x', true, false, 1), Some(7));
        assert_eq!(find_char_nth(text, 8, 'x', false, true, 3), None);
    }

    #[test]
    fn find_char_stays_on_the_line() {
        let text = "a bc bd";
        assert_eq!(find_char(text, 0, 'b', false, false), Some(2));
        assert_eq!(find_char(text, 0, 'b', true, false), Some(1));
        assert_eq!(find_char(text, 0, 'd', false, false), Some(6));
        assert_eq!(find_char(text, 6, 'a', false, true), Some(0));
        assert_eq!(find_char(text, 6, 'b', false, true), Some(5));
        assert_eq!(find_char(text, 6, 'a', true, true), Some(1));
        assert_eq!(find_char(text, 0, 'z', false, false), None);
        assert_eq!(find_char(text, 1, 'b', true, false), None);
        assert_eq!(find_char(text, 1, 'a', true, true), None);
        let text = "ab\ncd";
        assert_eq!(find_char(text, 0, 'd', false, false), None);
        assert_eq!(find_char(text, 3, 'a', false, true), None);
        let text = "日本語";
        assert_eq!(find_char(text, 0, '語', false, false), Some(6));
        let text = "e\u{301}e";
        assert_eq!(find_char(text, 0, 'e', false, false), Some(3));
    }

    #[test]
    fn every_motion_lands_on_a_grapheme_boundary() {
        for text in HARD {
            for big in [false, true] {
                for offset in boundaries(text) {
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

    #[test]
    fn clamp_normal_keeps_the_cursor_on_a_grapheme() {
        assert_eq!(clamp_normal("", 0), 0);
        let text = "a👨‍👩‍👦\u{301}b";
        for offset in 0..=text.len() + 3 {
            let result = clamp_normal(text, offset);
            assert!(is_boundary(text, result), "{offset} gave {result}");
            assert!(result <= text.len());
        }
        assert_eq!(clamp_normal(text, text.len()), text.len() - 1);
        assert_eq!(slice(text, text.len() - 1), "b");
    }

    #[test]
    fn a_motion_stops_at_the_blocks_edge() {
        let text = "one";
        assert_eq!(word_forward(text, 0, false), 2);
        assert_eq!(word_forward(text, 1, false), 2);
        assert_eq!(word_forward(text, 2, false), 2);
        assert_eq!(word_end(text, 0, false), 2);
        assert_eq!(word_end(text, 2, false), 2);
        assert_eq!(word_back(text, 0, false), 0);
        let text = "one two";
        assert_eq!(line_start(text, 5), 0);
        assert_eq!(line_end_inclusive(text, 0), 6);
        assert_eq!(line_end_exclusive(text, 0), 7);
    }
}
