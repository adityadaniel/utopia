//! Text helpers shared by the editor and the store (§10.5): grapheme-aware cursor
//! movement, minimal edit diffs, NFC normalization, and mark toggling (Cmd+B/I/E).

use std::ops::Range;

use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::command::TextEdit;

pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

pub fn prev_grapheme(s: &str, offset: usize) -> usize {
    s[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

pub fn next_grapheme(s: &str, offset: usize) -> usize {
    s[offset..]
        .graphemes(true)
        .next()
        .map_or(s.len(), |g| offset + g.len())
}

/// Start of the word at or before `offset` (Alt+Left / double-click).
pub fn prev_word_start(s: &str, offset: usize) -> usize {
    let mut last = 0;
    for (i, w) in s.split_word_bound_indices() {
        if i >= offset {
            break;
        }
        if w.chars().any(char::is_alphanumeric) {
            last = i;
        }
    }
    last
}

/// End of the word at or after `offset` (Alt+Right / double-click).
pub fn next_word_end(s: &str, offset: usize) -> usize {
    for (i, w) in s.split_word_bound_indices() {
        let end = i + w.len();
        if end > offset && w.chars().any(char::is_alphanumeric) {
            return end;
        }
    }
    s.len()
}

/// Ctrl+T (Emacs `transpose-chars`, Cocoa `transpose:`): swap the two graphemes either
/// side of `caret` and step past them.
///
/// With nothing after the caret on its paragraph (the text ends there, or a newline
/// follows) the two graphemes *before* it swap instead and the caret stays, as TextEdit
/// does. Returns the edit as `(range, replacement, caret after)` so callers apply it
/// through their usual replace path, or `None` when there is no pair to swap.
pub fn transpose(text: &str, caret: usize) -> Option<(Range<usize>, String, usize)> {
    let caret = floor_char_boundary(text, caret.min(text.len()));
    let at_end = text[caret..].chars().next().is_none_or(|c| c == '\n');
    let (left, mid, right) = if at_end {
        let mid = prev_grapheme(text, caret);
        (prev_grapheme(text, mid), mid, caret)
    } else {
        (
            prev_grapheme(text, caret),
            caret,
            next_grapheme(text, caret),
        )
    };
    if left == mid || mid == right {
        return None;
    }
    let swapped = format!("{}{}", &text[mid..right], &text[left..mid]);
    Some((left..right, swapped, right))
}

/// The word range containing `offset`, for double-click selection.
pub fn word_range_at(s: &str, offset: usize) -> Range<usize> {
    for (i, w) in s.split_word_bound_indices() {
        if offset >= i && offset < i + w.len() {
            return i..i + w.len();
        }
    }
    s.len()..s.len()
}

/// Clamp `offset` down to the nearest char boundary.
pub fn floor_char_boundary(s: &str, offset: usize) -> usize {
    let mut o = offset.min(s.len());
    while !s.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// Minimal single-range edit turning `old` into `new` (common prefix/suffix trim),
/// always on char boundaries.
pub fn diff(old: &str, new: &str) -> Option<TextEdit> {
    if old == new {
        return None;
    }
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((i, _), _)| i);
    let prefix = floor_char_boundary(new, floor_char_boundary(old, prefix));
    let old_rest = &old[prefix..];
    let new_rest = &new[prefix..];
    let suffix: usize = old_rest
        .chars()
        .rev()
        .zip(new_rest.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let suffix = suffix.min(old_rest.len()).min(new_rest.len());
    Some(TextEdit {
        range: prefix..old.len() - suffix,
        insert: new[prefix..new.len() - suffix].to_string(),
    })
}

/// Apply `edit` to `s`, validating bounds and char boundaries.
pub fn apply_edit(s: &str, edit: &TextEdit) -> Option<String> {
    let Range { start, end } = edit.range.clone();
    if start > end || end > s.len() || !s.is_char_boundary(start) || !s.is_char_boundary(end) {
        return None;
    }
    let mut out = String::with_capacity(s.len() + edit.insert.len());
    out.push_str(&s[..start]);
    out.push_str(&edit.insert);
    out.push_str(&s[end..]);
    Some(out)
}

/// Wrap/unwrap the selection with `delim` (FR-2.16, §9.1 Cmd+B/I/E). With an empty
/// selection, an unclosed opener before the caret is closed (AC-2.9); otherwise an empty
/// pair is inserted with the caret inside. Returns the new text and selection.
pub fn toggle_mark(text: &str, sel: Range<usize>, delim: &str) -> (String, Range<usize>) {
    let dl = delim.len();
    if sel.start == sel.end {
        let caret = sel.start;
        let before = &text[..caret];
        let opens = before.matches(delim).count();
        if opens % 2 == 1 {
            let mut out = text.to_string();
            out.insert_str(caret, delim);
            return (out, caret + dl..caret + dl);
        }
        let mut out = text.to_string();
        out.insert_str(caret, &format!("{delim}{delim}"));
        return (out, caret + dl..caret + dl);
    }
    let selected = &text[sel.clone()];
    // Selection includes the delimiters.
    if selected.len() >= 2 * dl && selected.starts_with(delim) && selected.ends_with(delim) {
        let inner = &selected[dl..selected.len() - dl];
        let mut out = String::new();
        out.push_str(&text[..sel.start]);
        out.push_str(inner);
        out.push_str(&text[sel.end..]);
        return (out, sel.start..sel.start + inner.len());
    }
    // Delimiters sit just outside the selection.
    if sel.start >= dl && text[..sel.start].ends_with(delim) && text[sel.end..].starts_with(delim) {
        let mut out = String::new();
        out.push_str(&text[..sel.start - dl]);
        out.push_str(selected);
        out.push_str(&text[sel.end + dl..]);
        return (out, sel.start - dl..sel.end - dl);
    }
    let mut out = String::new();
    out.push_str(&text[..sel.start]);
    out.push_str(delim);
    out.push_str(selected);
    out.push_str(delim);
    out.push_str(&text[sel.end..]);
    (out, sel.start + dl..sel.end + dl)
}

/// The closing bracket for a typed opening one. Only brackets wrap a selection or pair
/// as you type: `**`/`_`/`` ` `` have Cmd+B/I/E (FR-2.16), and a quote, `*` or `_` is as
/// often an apostrophe or a bullet as a delimiter, so those type as themselves.
pub fn closing_bracket(open: &str) -> Option<&'static str> {
    match open {
        "(" => Some(")"),
        "[" => Some("]"),
        "{" => Some("}"),
        _ => None,
    }
}

/// Typing an opening bracket over a non-empty selection surrounds it instead of replacing
/// it, and keeps the original text selected inside the new brackets, so typing `[` twice
/// over `word` gives `[[word]]`. `None` when `open` is not a bracket or nothing is selected.
pub fn wrap_selection(text: &str, sel: Range<usize>, open: &str) -> Option<(String, Range<usize>)> {
    let close = closing_bracket(open)?;
    if sel.start >= sel.end
        || sel.end > text.len()
        || !text.is_char_boundary(sel.start)
        || !text.is_char_boundary(sel.end)
    {
        return None;
    }
    let mut out = String::with_capacity(text.len() + open.len() + close.len());
    out.push_str(&text[..sel.start]);
    out.push_str(open);
    out.push_str(&text[sel.clone()]);
    out.push_str(close);
    out.push_str(&text[sel.end..]);
    Some((out, sel.start + open.len()..sel.end + open.len()))
}

/// What typing `typed` at an empty selection does to the brackets around the caret:
/// - an opening bracket inserts the pair with the caret between (`(|)`), unless the caret
///   sits directly before a word character, where a lone opener is what is meant (`(foo`);
/// - a closing bracket that is already the next character is stepped over, so typing the
///   pair by hand still gives `()` and not `())`.
///
/// Returns the new text and caret, or `None` for an ordinary keystroke. Quotes,
/// backticks, `*` and `_` are not paired, matching [`closing_bracket`].
pub fn auto_pair(text: &str, caret: usize, typed: &str) -> Option<(String, usize)> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let next = text[caret..].chars().next();
    if matches!(typed, ")" | "]" | "}") && next.map(String::from).as_deref() == Some(typed) {
        return Some((text.to_string(), caret + typed.len()));
    }
    let close = closing_bracket(typed)?;
    if next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let mut out = String::with_capacity(text.len() + typed.len() + close.len());
    out.push_str(&text[..caret]);
    out.push_str(typed);
    out.push_str(close);
    out.push_str(&text[caret..]);
    Some((out, caret + typed.len()))
}

/// Backspace inside an empty pair (`(|)`, `[|]`, `{|}`) deletes both brackets: the range
/// to remove, or `None` for an ordinary backspace.
pub fn empty_pair_at(text: &str, caret: usize) -> Option<Range<usize>> {
    let before = text.get(..caret)?;
    let after = text.get(caret..)?;
    let open = before.chars().next_back()?;
    let close = closing_bracket(open.encode_utf8(&mut [0; 4]))?;
    after
        .starts_with(close)
        .then(|| caret - open.len_utf8()..caret + close.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_respect_clusters() {
        let s = "a👩‍👩‍👧b";
        let after_a = next_grapheme(s, 0);
        assert_eq!(after_a, 1);
        let after_family = next_grapheme(s, after_a);
        assert_eq!(&s[after_family..], "b");
        assert_eq!(prev_grapheme(s, after_family), 1);
        let combining = "e\u{301}x";
        assert_eq!(next_grapheme(combining, 0), 3);
    }

    #[test]
    fn diff_round_trips() {
        for (a, b) in [
            ("hello", "hello world"),
            ("hello world", "hello"),
            ("abc", "axc"),
            ("", "日本"),
            ("日本語", "日語"),
            ("aaa", "aa"),
            ("🎉x", "🎊x"),
        ] {
            let e = diff(a, b).unwrap();
            assert_eq!(apply_edit(a, &e).unwrap(), b, "{a} -> {b}");
        }
        assert!(diff("same", "same").is_none());
    }

    #[test]
    fn apply_edit_rejects_bad_ranges() {
        let e = TextEdit {
            range: 1..2,
            insert: String::new(),
        };
        assert!(apply_edit("日", &e).is_none());
        let e = TextEdit {
            range: 0..9,
            insert: String::new(),
        };
        assert!(apply_edit("abc", &e).is_none());
    }

    #[test]
    fn ac_2_9_cmd_b_closes_unclosed() {
        let (t, sel) = toggle_mark("**bold", 6..6, "**");
        assert_eq!(t, "**bold**");
        assert_eq!(sel, 8..8);
    }

    #[test]
    fn wrap_and_unwrap() {
        let (t, sel) = toggle_mark("a word here", 2..6, "**");
        assert_eq!(t, "a **word** here");
        assert_eq!(&t[sel.clone()], "word");
        let (t2, sel2) = toggle_mark(&t, sel, "**");
        assert_eq!(t2, "a word here");
        assert_eq!(&t2[sel2], "word");
        let (t3, _) = toggle_mark("x", 1..1, "`");
        assert_eq!(t3, "x``");
    }

    #[test]
    fn brackets_wrap_the_selection() {
        let (t, sel) = wrap_selection("see 日本語 now", 4..13, "[").unwrap();
        assert_eq!(t, "see [日本語] now");
        assert_eq!(&t[sel.clone()], "日本語");
        // Again on the still-selected text: the Roam page-link idiom.
        let (t, sel) = wrap_selection(&t, sel, "[").unwrap();
        assert_eq!(t, "see [[日本語]] now");
        assert_eq!(&t[sel], "日本語");
        let fam = "a👩‍👩‍👧b";
        let (t, sel) = wrap_selection(fam, 1..fam.len() - 1, "(").unwrap();
        assert_eq!(t, "a(👩‍👩‍👧)b");
        assert_eq!(&t[sel], "👩‍👩‍👧");
        assert_eq!(wrap_selection("x", 0..1, "{").unwrap().0, "{x}");
        // Nothing selected, or not a bracket: an ordinary keystroke.
        assert!(wrap_selection("x", 1..1, "[").is_none());
        for ch in ["\"", "'", "`", "*", "_", ")", "]", "x"] {
            assert!(wrap_selection("word", 0..4, ch).is_none(), "{ch}");
        }
        assert!(
            wrap_selection("日", 0..1, "[").is_none(),
            "not a char boundary"
        );
    }

    #[test]
    fn brackets_pair_as_you_type() {
        assert_eq!(auto_pair("", 0, "("), Some(("()".into(), 1)));
        assert_eq!(auto_pair("a ", 2, "["), Some(("a []".into(), 3)));
        assert_eq!(auto_pair("x}", 1, "{"), Some(("x{}}".into(), 2)));
        // `[[` is two pairs, caret in the middle.
        let (t, c) = auto_pair("[]", 1, "[").unwrap();
        assert_eq!((t.as_str(), c), ("[[]]", 2));
        // The closer already there is stepped over, not doubled.
        assert_eq!(auto_pair("()", 1, ")"), Some(("()".into(), 2)));
        assert_eq!(auto_pair("[[a]]", 3, "]"), Some(("[[a]]".into(), 4)));
        assert_eq!(auto_pair("a", 1, ")"), None, "nothing to step over");
        // Before a word character an opener is just an opener.
        assert_eq!(auto_pair("foo", 0, "("), None);
        assert_eq!(auto_pair("日本", 0, "["), None);
        assert_eq!(auto_pair("_x", 0, "{"), None);
        assert!(auto_pair(" foo", 0, "(").is_some(), "a space is not a word");
        for ch in ["\"", "'", "`", "*", "_", "x"] {
            assert_eq!(auto_pair("", 0, ch), None, "{ch}");
        }
        assert_eq!(auto_pair("日", 1, "("), None, "not a char boundary");
    }

    #[test]
    fn backspace_clears_an_empty_pair() {
        assert_eq!(empty_pair_at("a()", 2), Some(1..3));
        assert_eq!(empty_pair_at("[[]]", 2), Some(1..3));
        assert_eq!(empty_pair_at("{}", 1), Some(0..2));
        assert_eq!(empty_pair_at("(x)", 2), None);
        assert_eq!(empty_pair_at("(]", 1), None);
        assert_eq!(empty_pair_at("()", 0), None);
        assert_eq!(empty_pair_at("日()", 4), Some(3..5));
    }

    /// Ctrl+T swaps the pair around the caret and moves past it; at a line end the two
    /// before it swap instead, and a grapheme cluster is never split.
    #[test]
    fn transpose_swaps_graphemes() {
        let apply = |t: &str, c: usize| {
            transpose(t, c).map(|(r, new, caret)| {
                let mut out = t.to_string();
                out.replace_range(r, &new);
                (out, caret)
            })
        };
        assert_eq!(apply("abcd", 1), Some(("bacd".into(), 2)));
        assert_eq!(apply("abcd", 3), Some(("abdc".into(), 4)));
        // End of the text: the two before it, caret stays.
        assert_eq!(apply("abcd", 4), Some(("abdc".into(), 4)));
        // End of a paragraph: same, and the newline stays where it was.
        assert_eq!(apply("ab\ncd", 2), Some(("ba\ncd".into(), 2)));
        // Nothing to do: empty, at the very start, one grapheme only.
        assert_eq!(apply("", 0), None);
        assert_eq!(apply("a", 0), None);
        assert_eq!(apply("a", 1), None);
        // Multi-byte and a family emoji move as whole graphemes.
        assert_eq!(apply("a日", 4), Some(("日a".into(), 4)));
        assert_eq!(apply("日本", 3), Some(("本日".into(), 6)));
        let fam = "a👩‍👩‍👧b";
        assert_eq!(apply(fam, 1), Some(("👩‍👩‍👧ab".into(), 1 + "👩‍👩‍👧".len())));
    }

    #[test]
    fn words() {
        let s = "hello big world";
        assert_eq!(prev_word_start(s, 9), 6);
        assert_eq!(next_word_end(s, 6), 9);
        assert_eq!(word_range_at(s, 7), 6..9);
    }
}
