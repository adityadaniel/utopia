//! Syntax colouring for code blocks (DESIGN.md §3.11, PG-5).
//!
//! The grammars are bat's syntax set (`two-face`, about 200 languages) run by syntect's
//! parser on the pure-Rust regex engine. A block's language is the name on its first line
//! (`crate::page_view::code_language`). Every scope is folded into one of five roles, each
//! a theme token, so the colours follow the theme and its contrast rules rather than a
//! TextMate theme. Results are cached by language and text: a row is drawn many times, a
//! block's text changes rarely, and highlighting is never redone in `render` for text it
//! has already seen (rule 10).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

use gpui::HighlightStyle;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::theme::Theme;

/// What a run of code is, as far as colour goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Keyword,
    String,
    Comment,
    Number,
    /// Function, type and class names.
    Name,
}

impl Role {
    pub fn style(self, th: &Theme) -> HighlightStyle {
        let color = match self {
            Role::Keyword => th.syntax_keyword,
            Role::String => th.syntax_string,
            Role::Comment => th.muted,
            Role::Number => th.syntax_number,
            Role::Name => th.syntax_name,
        };
        HighlightStyle {
            color: Some(color),
            ..Default::default()
        }
    }
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

/// Load the grammars on a background thread, so the first code block on screen does not
/// pay for it (they are a few megabytes, decompressed on first use).
pub fn warm_up() {
    std::thread::spawn(|| {
        syntaxes();
    });
}

/// Names a block may use that bat's set spells another way.
fn token(lang: &str) -> String {
    let lang = lang.trim().to_ascii_lowercase();
    match lang.as_str() {
        "c#" => "cs".into(),
        "c++" => "cpp".into(),
        "f#" => "fs".into(),
        "shell" | "zsh" | "sh" => "bash".into(),
        "objective-c" | "objc" => "m".into(),
        _ => lang,
    }
}

fn syntax_for(lang: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let t = token(lang);
    set.find_syntax_by_token(&t)
        .or_else(|| set.find_syntax_by_name(lang.trim()))
}

/// The role a scope stack gives a run: the innermost scope that has one wins.
fn role(stack: &ScopeStack) -> Option<Role> {
    for scope in stack.as_slice().iter().rev() {
        let s = scope.build_string();
        let starts = |p: &str| s == p || s.starts_with(&format!("{p}."));
        if starts("comment") || starts("punctuation.definition.comment") {
            return Some(Role::Comment);
        }
        if starts("string") || starts("constant.character.escape") {
            return Some(Role::String);
        }
        if starts("constant.numeric") {
            return Some(Role::Number);
        }
        // Operators stay ink; a bracket or quote takes the colour of what it belongs to
        // (a string's quotes are part of the string), so look further out.
        if starts("keyword.operator") {
            return None;
        }
        if starts("punctuation") {
            continue;
        }
        if starts("keyword") || starts("storage") || starts("constant.language") {
            return Some(Role::Keyword);
        }
        if starts("entity.name")
            || starts("support.function")
            || starts("support.type")
            || starts("support.class")
            || starts("variable.function")
        {
            return Some(Role::Name);
        }
    }
    None
}

fn compute(lang: &str, code: &str) -> Vec<(Range<usize>, Role)> {
    let Some(syntax) = syntax_for(lang) else {
        return Vec::new();
    };
    let set = syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out: Vec<(Range<usize>, Role)> = Vec::new();
    let mut push = |range: Range<usize>, role: Option<Role>| {
        let Some(role) = role else { return };
        if range.is_empty() {
            return;
        }
        match out.last_mut() {
            Some((last, r)) if *r == role && last.end == range.start => last.end = range.end,
            _ => out.push((range, role)),
        }
    };
    let mut offset = 0;
    for line in LinesWithEndings::from(code) {
        // A grammar that fails on a line leaves the rest of the block plain (rule 11).
        let Ok(ops) = state.parse_line(line, set) else {
            break;
        };
        // The newline itself carries no colour.
        let text_end = line.trim_end_matches(['\n', '\r']).len();
        let mut last = 0;
        for (pos, op) in ops {
            let (a, b) = (last.min(text_end), pos.min(text_end));
            push(offset + a..offset + b, role(&stack));
            if stack.apply(&op).is_err() {
                return out;
            }
            last = pos;
        }
        push(offset + last.min(text_end)..offset + text_end, role(&stack));
        offset += line.len();
    }
    out
}

type Cache = Mutex<HashMap<u64, Arc<Vec<(Range<usize>, Role)>>>>;

/// The coloured runs of `code` in `lang`, as byte ranges into `code`; empty when the
/// language is not known. Cached, so a row that redraws does not parse again.
pub fn highlight(lang: &str, code: &str) -> Arc<Vec<(Range<usize>, Role)>> {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lang.hash(&mut h);
    code.hash(&mut h);
    let key = h.finish();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let runs = Arc::new(compute(lang, code));
    if let Ok(mut c) = cache.lock() {
        // A page has a handful of code blocks; editing one makes a new entry per change,
        // so the cache is simply emptied now and then instead of tracking use.
        if c.len() > 256 {
            c.clear();
        }
        c.insert(key, runs.clone());
    }
    runs
}

/// [`highlight`] as gpui highlights, shifted by `offset` (where `code` starts in the text
/// being drawn).
pub fn highlights(
    lang: &str,
    code: &str,
    offset: usize,
    th: &Theme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    highlight(lang, code)
        .iter()
        .map(|(r, role)| (r.start + offset..r.end + offset, role.style(th)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles<'a>(code: &'a str, lang: &str) -> Vec<(&'a str, Role)> {
        highlight(lang, code)
            .iter()
            .map(|(r, role)| (&code[r.clone()], *role))
            .collect()
    }

    /// The five roles come out of real grammars, on the bytes they cover.
    #[test]
    fn rust_gets_keywords_strings_numbers_comments_and_names() {
        let got = roles(
            "fn main() {\n    let x = \"hi\"; // say 42\n    1\n}\n",
            "rust",
        );
        for want in [
            ("fn", Role::Keyword),
            ("main", Role::Name),
            ("let", Role::Keyword),
            ("\"hi\"", Role::String),
            ("// say 42", Role::Comment),
            ("1", Role::Number),
        ] {
            assert!(got.contains(&want), "{want:?} in {got:?}");
        }
    }

    /// bat's set covers the languages the default Sublime set lacks.
    #[test]
    fn swift_typescript_and_toml_are_known() {
        for (lang, code, kw) in [
            ("swift", "let x = 1\n", "let"),
            ("typescript", "const x: number = 1\n", "const"),
            ("ts", "const x = 1\n", "const"),
            ("toml", "[a]\nb = \"c\"\n", "\"c\""),
        ] {
            let got = roles(code, lang);
            assert!(got.iter().any(|(t, _)| *t == kw), "{lang}: {got:?}");
        }
    }

    /// An unknown language is drawn plain, never guessed and never a panic.
    #[test]
    fn an_unknown_language_is_plain() {
        assert!(highlight("klingon", "qapla'\n").is_empty());
    }

    /// Ranges are in order and never overlap, as gpui needs.
    #[test]
    fn runs_are_ordered_and_disjoint() {
        let runs = highlight("python", "def f(a=1):\n    return 'x'  # y\n");
        assert!(!runs.is_empty());
        assert!(
            runs.windows(2).all(|w| w[0].0.end <= w[1].0.start),
            "{runs:?}"
        );
    }
}
