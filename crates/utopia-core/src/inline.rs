use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mark {
    Bold,
    Italic,
    Strike,
    Highlight,
}

impl Mark {
    pub fn delimiter(self) -> &'static str {
        match self {
            Mark::Bold => "**",
            Mark::Italic => "_",
            Mark::Strike => "~~",
            Mark::Highlight => "^^",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Emph {
        mark: Mark,
        children: Vec<Inline>,
        range: Range<usize>,
    },
    Code {
        text: String,
        range: Range<usize>,
    },
    PageRef {
        title: String,
        range: Range<usize>,
    },
    Tag {
        title: String,
        bracketed: bool,
        range: Range<usize>,
    },
    BlockRef {
        uid: String,
        range: Range<usize>,
    },
    Link {
        text: String,
        url: String,
        range: Range<usize>,
    },
    Embed {
        uid: String,
        range: Range<usize>,
    },
}

impl Inline {
    pub fn range(&self) -> Option<&Range<usize>> {
        match self {
            Inline::Text(_) => None,
            Inline::Emph { range, .. }
            | Inline::Code { range, .. }
            | Inline::PageRef { range, .. }
            | Inline::Tag { range, .. }
            | Inline::BlockRef { range, .. }
            | Inline::Link { range, .. }
            | Inline::Embed { range, .. } => Some(range),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Page,
    Tag,
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefSpan {
    pub kind: RefKind,
    pub target: String,
    pub range: Range<usize>,
}

impl RefSpan {
    pub fn is_page_like(&self) -> bool {
        matches!(self.kind, RefKind::Page | RefKind::Tag)
    }
}

fn is_meta(b: u8) -> bool {
    matches!(
        b,
        b'[' | b']' | b'(' | b')' | b'#' | b'*' | b'_' | b'~' | b'^' | b'`' | b'\\'
    )
}

pub fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn prev_char(s: &str, i: usize) -> Option<char> {
    s[..i].chars().next_back()
}

fn next_char(s: &str, i: usize) -> Option<char> {
    s[i..].chars().next()
}

fn find_str(s: &str, from: usize, end: usize, pat: &str) -> Option<usize> {
    s[from..end].find(pat).map(|j| from + j)
}

pub fn parse(src: &str) -> Vec<Inline> {
    parse_range(src, 0, src.len(), &[])
}

fn parse_range(s: &str, start: usize, end: usize, active: &[Mark]) -> Vec<Inline> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = start;
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut buf)));
            }
        };
    }
    while i < end {
        let c = b[i];
        if c == b'\\' && i + 1 < end && is_meta(b[i + 1]) {
            buf.push(b[i + 1] as char);
            i += 2;
            continue;
        }
        if let Some((node, next)) = atom_at(s, i, end) {
            flush!();
            out.push(node);
            i = next;
            continue;
        }
        if let Some(mark) = mark_at(s, i, end) {
            if !active.contains(&mark) && active.len() < 2 {
                let dl = mark.delimiter().len();
                if let Some(close) = find_close(s, i + dl, end, mark) {
                    let mut inner_active = active.to_vec();
                    inner_active.push(mark);
                    flush!();
                    out.push(Inline::Emph {
                        mark,
                        children: parse_range(s, i + dl, close, &inner_active),
                        range: i..close + dl,
                    });
                    i = close + dl;
                    continue;
                }
            }
            let dl = mark.delimiter().len();
            buf.push_str(&s[i..i + dl]);
            i += dl;
            continue;
        }
        let ch = next_char(s, i).expect("i < end");
        buf.push(ch);
        i += ch.len_utf8();
    }
    flush!();
    out
}

fn atom_at(s: &str, i: usize, end: usize) -> Option<(Inline, usize)> {
    let rest = &s[i..end];
    let b = s.as_bytes();
    match b[i] {
        b'`' => {
            let close = find_str(s, i + 1, end, "`")?;
            Some((
                Inline::Code {
                    text: s[i + 1..close].to_string(),
                    range: i..close + 1,
                },
                close + 1,
            ))
        }
        b'!' if rest.starts_with("![") && !rest.starts_with("![[") => {
            let (link, next) = atom_at(s, i + 1, end)?;
            match link {
                Inline::Link { text, url, .. } => Some((
                    Inline::Link {
                        text,
                        url,
                        range: i..next,
                    },
                    next,
                )),
                _ => None,
            }
        }
        b'[' if rest.starts_with("[[") => {
            let (title, next) = bracket_title(s, i + 2, end)?;
            Some((
                Inline::PageRef {
                    title,
                    range: i..next,
                },
                next,
            ))
        }
        b'[' => {
            let mid = find_str(s, i + 1, end, "](")?;
            let text = &s[i + 1..mid];
            if text.is_empty() || text.contains('\n') || text.contains('[') {
                return None;
            }
            let close = find_str(s, mid + 2, end, ")")?;
            let url = &s[mid + 2..close];
            if url.is_empty() || url.chars().any(char::is_whitespace) {
                return None;
            }
            Some((
                Inline::Link {
                    text: text.to_string(),
                    url: url.to_string(),
                    range: i..close + 1,
                },
                close + 1,
            ))
        }
        b'{' if rest.starts_with("{{") => {
            let close = find_str(s, i + 2, end, "}}")?;
            let inner = s[i + 2..close].trim();
            if let Some(hash) = inner
                .strip_prefix("pdf:")
                .map(str::trim)
                .and_then(|t| t.strip_prefix("blob:"))
                .filter(|h| crate::attach::is_hash(h))
            {
                return Some((
                    Inline::Link {
                        text: "PDF".to_string(),
                        url: format!("blob:{hash}"),
                        range: i..close + 2,
                    },
                    close + 2,
                ));
            }
            if let Some((hash, page, _)) = crate::attach::parse_highlight(inner) {
                return Some((
                    Inline::Link {
                        text: format!("p. {}", page + 1),
                        url: format!("pdfpage:{hash}#{}", page + 1),
                        range: i..close + 2,
                    },
                    close + 2,
                ));
            }
            let target = inner
                .strip_prefix("[[embed]]:")
                .or_else(|| inner.strip_prefix("embed:"))?
                .trim();
            let uid = target.strip_prefix("((")?.strip_suffix("))")?;
            if !crate::ids::BlockId::is_valid_ref(uid) {
                return None;
            }
            Some((
                Inline::Embed {
                    uid: uid.to_string(),
                    range: i..close + 2,
                },
                close + 2,
            ))
        }
        b'(' if rest.starts_with("((") => {
            let close = find_str(s, i + 2, end, "))")?;
            let uid = &s[i + 2..close];
            if !crate::ids::BlockId::is_valid_ref(uid) {
                return None;
            }
            Some((
                Inline::BlockRef {
                    uid: uid.to_string(),
                    range: i..close + 2,
                },
                close + 2,
            ))
        }
        b'h' | b'H' => {
            let next = bare_url_end(s, i, end)?;
            Some((
                Inline::Link {
                    text: s[i..next].to_string(),
                    url: s[i..next].to_string(),
                    range: i..next,
                },
                next,
            ))
        }
        b'#' => {
            if prev_char(s, i).is_some_and(|c| c.is_alphanumeric() || c == '#') {
                return None;
            }
            if rest.starts_with("#[[") {
                let (title, next) = bracket_title(s, i + 3, end)?;
                return Some((
                    Inline::Tag {
                        title,
                        bracketed: true,
                        range: i..next,
                    },
                    next,
                ));
            }
            let name_len: usize = s[i + 1..end]
                .chars()
                .take_while(|c| is_tag_char(*c))
                .map(char::len_utf8)
                .sum();
            if name_len == 0 {
                return None;
            }
            let next = i + 1 + name_len;
            Some((
                Inline::Tag {
                    title: s[i + 1..next].to_string(),
                    bracketed: false,
                    range: i..next,
                },
                next,
            ))
        }
        _ => None,
    }
}

fn bracket_title(s: &str, from: usize, end: usize) -> Option<(String, usize)> {
    let close = find_str(s, from, end, "]]")?;
    let raw = &s[from..close];
    if raw.contains('\n') || raw.contains("[[") {
        return None;
    }
    let title = raw.trim();
    if title.is_empty() {
        return None;
    }
    Some((title.to_string(), close + 2))
}

fn mark_at(s: &str, i: usize, end: usize) -> Option<Mark> {
    let rest = &s[i..end];
    if rest.starts_with("**") {
        Some(Mark::Bold)
    } else if rest.starts_with("~~") {
        Some(Mark::Strike)
    } else if rest.starts_with("^^") {
        Some(Mark::Highlight)
    } else if rest.starts_with('_') {
        let before_ok = prev_char(s, i).is_none_or(|c| !c.is_alphanumeric());
        let after_ok = s[i + 1..end]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace());
        (before_ok && after_ok).then_some(Mark::Italic)
    } else {
        None
    }
}

fn bare_url_end(s: &str, i: usize, end: usize) -> Option<usize> {
    if prev_char(s, i).is_some_and(|c| c.is_alphanumeric()) {
        return None;
    }
    let rest = &s[i..end];
    let scheme = ["https://", "http://"].into_iter().find(|p| {
        rest.get(..p.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(p))
    })?;
    let len = rest
        .find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '`')
        .unwrap_or(rest.len());
    let mut url = &rest[..len];
    while let Some(last) = url.chars().next_back() {
        let trailing = match last {
            '.' | ',' | ':' | ';' | '!' | '?' | '\'' | '"' | '*' | '_' | '~' | '^' => true,
            ')' => url.matches(')').count() > url.matches('(').count(),
            ']' => url.matches(']').count() > url.matches('[').count(),
            _ => false,
        };
        if !trailing {
            break;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
    (url.len() > scheme.len()).then_some(i + url.len())
}

pub fn is_external_url(url: &str) -> bool {
    !["blob:", "pdfpage:", "utopia:"].iter().any(|p| {
        url.get(..p.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(p))
    })
}

fn find_close(s: &str, from: usize, end: usize, mark: Mark) -> Option<usize> {
    let delim = mark.delimiter();
    let b = s.as_bytes();
    let mut j = from;
    while j < end {
        if b[j] == b'\\' && j + 1 < end && is_meta(b[j + 1]) {
            j += 2;
            continue;
        }
        if s[j..end].starts_with(delim) && j > from {
            let ok = match mark {
                Mark::Italic => {
                    prev_char(s, j).is_some_and(|c| !c.is_whitespace())
                        && s[j + 1..end]
                            .chars()
                            .next()
                            .is_none_or(|c| !c.is_alphanumeric())
                }
                _ => true,
            };
            if ok {
                return Some(j);
            }
        }
        if let Some((_, next)) = atom_at(s, j, end) {
            j = next;
            continue;
        }
        j += next_char(s, j).map_or(1, char::len_utf8);
    }
    None
}

pub fn extract_refs(src: &str) -> Vec<RefSpan> {
    let mut out = Vec::new();
    collect_refs(&parse(src), &mut out);
    out
}

fn collect_refs(nodes: &[Inline], out: &mut Vec<RefSpan>) {
    for n in nodes {
        match n {
            Inline::Emph { children, .. } => collect_refs(children, out),
            Inline::PageRef { title, range } => out.push(RefSpan {
                kind: RefKind::Page,
                target: title.clone(),
                range: range.clone(),
            }),
            Inline::Tag { title, range, .. } => out.push(RefSpan {
                kind: RefKind::Tag,
                target: title.clone(),
                range: range.clone(),
            }),
            Inline::BlockRef { uid, range } | Inline::Embed { uid, range } => out.push(RefSpan {
                kind: RefKind::Block,
                target: uid.clone(),
                range: range.clone(),
            }),
            Inline::Text(_) | Inline::Code { .. } | Inline::Link { .. } => {}
        }
    }
}

pub fn plain_text(src: &str) -> String {
    fn walk(nodes: &[Inline], out: &mut String) {
        for n in nodes {
            match n {
                Inline::Text(t) => out.push_str(t),
                Inline::Emph { children, .. } => walk(children, out),
                Inline::Code { text, .. } => out.push_str(text),
                Inline::PageRef { title, .. } => out.push_str(title),
                Inline::Tag { title, .. } => {
                    out.push('#');
                    out.push_str(title);
                }
                Inline::BlockRef { uid, .. } | Inline::Embed { uid, .. } => {
                    out.push_str("((");
                    out.push_str(uid);
                    out.push_str("))");
                }
                Inline::Link { text, .. } => out.push_str(text),
            }
        }
    }
    let mut out = String::new();
    walk(&parse(src), &mut out);
    out
}

pub fn attribute(src: &str) -> Option<(String, Range<usize>)> {
    let line_end = src.find('\n').unwrap_or(src.len());
    let sep = src[..line_end].find("::")?;
    let key = src[..sep].trim();
    if key.is_empty() || key.len() > 64 || key.contains("[[") || key.contains('`') {
        return None;
    }
    let mut start = sep + 2;
    while src[start..].starts_with(' ') {
        start += 1;
    }
    Some((key.to_string(), start..src.len()))
}

fn atom_ranges(nodes: &[Inline], out: &mut Vec<Range<usize>>) {
    for n in nodes {
        match n {
            Inline::Emph { children, .. } => atom_ranges(children, out),
            Inline::Code { range, .. }
            | Inline::PageRef { range, .. }
            | Inline::Tag { range, .. }
            | Inline::BlockRef { range, .. }
            | Inline::Embed { range, .. }
            | Inline::Link { range, .. } => out.push(range.clone()),
            Inline::Text(_) => {}
        }
    }
}

pub fn find_mentions(src: &str, title: &str) -> Vec<Range<usize>> {
    let want: String = title.trim().to_lowercase();
    let n = title.trim().chars().count();
    if n == 0 {
        return Vec::new();
    }
    let mut atoms = Vec::new();
    atom_ranges(&parse(src), &mut atoms);
    let mut out = Vec::new();
    let boundaries: Vec<usize> = src
        .char_indices()
        .map(|(i, _)| i)
        .chain([src.len()])
        .collect();
    let mut k = 0;
    while k + n < boundaries.len() {
        let (a, b) = (boundaries[k], boundaries[k + n]);
        let hit = src[a..b].to_lowercase() == want
            && prev_char(src, a).is_none_or(|c| !c.is_alphanumeric())
            && next_char(src, b).is_none_or(|c| !c.is_alphanumeric())
            && !atoms.iter().any(|r| r.start < b && a < r.end);
        if hit {
            out.push(a..b);
            k += n;
        } else {
            k += 1;
        }
    }
    out
}

fn words_after(s: &str, mut i: usize, max: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    while ends.len() < max {
        if !s[i..].starts_with(' ') {
            break;
        }
        let start = i + 1;
        let len: usize = s[start..]
            .chars()
            .take_while(|c| is_tag_char(*c))
            .map(char::len_utf8)
            .sum();
        if len == 0 {
            break;
        }
        i = start + len;
        ends.push(i);
    }
    ends
}

const MAX_TAG_WORDS: usize = 6;

pub fn heal_tags(src: &str, is_live_title: impl Fn(&str) -> bool) -> Option<String> {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    for r in extract_refs(src) {
        if r.kind != RefKind::Tag || src[r.range.clone()].starts_with("#[[") {
            continue;
        }
        let ends = words_after(src, r.range.end, MAX_TAG_WORDS);
        for &end in ends.iter().rev() {
            let phrase = &src[r.range.start + 1..end];
            if is_live_title(phrase) {
                edits.push((r.range.start..end, format!("#[[{phrase}]]")));
                break;
            }
        }
    }
    if edits.is_empty() {
        return None;
    }
    let mut out = src.to_string();
    for (range, rep) in edits.into_iter().rev() {
        out.replace_range(range, &rep);
    }
    Some(out)
}

pub fn whole_phrase_tag(src: &str, tag_start: usize) -> Option<(Range<usize>, String)> {
    let r = extract_refs(src)
        .into_iter()
        .find(|r| r.kind == RefKind::Tag && r.range.start == tag_start)?;
    if src[r.range.clone()].starts_with("#[[") {
        return None;
    }
    let end = *words_after(src, r.range.end, MAX_TAG_WORDS).last()?;
    let phrase = &src[tag_start + 1..end];
    Some((tag_start..end, format!("#[[{phrase}]]")))
}

pub fn page_link_text(title: &str, as_tag: bool) -> String {
    if as_tag {
        if !title.is_empty() && title.chars().all(is_tag_char) {
            format!("#{title}")
        } else {
            format!("#[[{title}]]")
        }
    } else {
        format!("[[{title}]]")
    }
}

pub fn rewrite_page_refs(
    src: &str,
    matches: impl Fn(&str) -> bool,
    new_title: &str,
) -> Option<String> {
    let mut out = src.to_string();
    let mut changed = false;
    for r in extract_refs(src).into_iter().rev() {
        if r.is_page_like() && matches(&r.target) {
            let rep = page_link_text(new_title, r.kind == RefKind::Tag);
            out.replace_range(r.range, &rep);
            changed = true;
        }
    }
    changed.then_some(out)
}

pub fn unlink_page_refs(src: &str, matches: impl Fn(&str) -> bool) -> Option<String> {
    let mut out = src.to_string();
    let mut changed = false;
    for r in extract_refs(src).into_iter().rev() {
        if r.is_page_like() && matches(&r.target) {
            out.replace_range(r.range, &r.target);
            changed = true;
        }
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(s: &str) -> Vec<(RefKind, String)> {
        extract_refs(s)
            .into_iter()
            .map(|r| (r.kind, r.target))
            .collect()
    }

    #[test]
    fn page_refs_tags_and_block_refs() {
        let s = "Met [[Dana]] about #project and #[[Modern Religion]] see ((01J9F3K2M8Q4X7B6C5D4E3F2G1))";
        assert_eq!(
            refs(s),
            vec![
                (RefKind::Page, "Dana".into()),
                (RefKind::Tag, "project".into()),
                (RefKind::Tag, "Modern Religion".into()),
                (RefKind::Block, "01J9F3K2M8Q4X7B6C5D4E3F2G1".into()),
            ]
        );
        let r = &extract_refs(s)[0];
        assert_eq!(&s[r.range.clone()], "[[Dana]]");
    }

    #[test]
    fn ac_3_2_unclosed_ref_is_not_a_ref() {
        assert!(refs("typing [[Alpha").is_empty());
        assert!(refs("[[]] and (( )) and #").is_empty());
    }

    #[test]
    fn code_is_opaque() {
        assert!(refs("`[[Not]] #ref ((abc))`").is_empty());
        assert_eq!(refs("`x` [[Yes]]"), vec![(RefKind::Page, "Yes".into())]);
    }

    #[test]
    fn hash_rules() {
        assert!(refs("# heading").is_empty(), "# + space is literal");
        assert!(refs("C# and a#b").is_empty(), "intraword # is literal");
        assert!(refs("#!").is_empty());
        assert_eq!(refs("(#tag)"), vec![(RefKind::Tag, "tag".into())]);
        assert_eq!(refs("#tag-1_x."), vec![(RefKind::Tag, "tag-1_x".into())]);
        assert_eq!(refs("#café"), vec![(RefKind::Tag, "café".into())]);
    }

    #[test]
    fn escapes() {
        assert!(refs(r"\[[Not]]").is_empty());
        assert!(refs(r"\#tag").is_empty());
        assert_eq!(parse(r"\*\*x\*\*"), vec![Inline::Text("**x**".into())]);
    }

    #[test]
    fn emphasis_and_unclosed_literal() {
        assert_eq!(
            parse("**bold** _it_"),
            vec![
                Inline::Emph {
                    mark: Mark::Bold,
                    children: vec![Inline::Text("bold".into())],
                    range: 0..8
                },
                Inline::Text(" ".into()),
                Inline::Emph {
                    mark: Mark::Italic,
                    children: vec![Inline::Text("it".into())],
                    range: 9..13
                },
            ]
        );
        assert_eq!(parse("**bold"), vec![Inline::Text("**bold".into())]);
        assert_eq!(
            parse("snake_case_name"),
            vec![Inline::Text("snake_case_name".into())]
        );
        assert_eq!(plain_text("^^hi^^ ~~no~~ `c`"), "hi no c");
    }

    #[test]
    fn mixed_emphasis_one_level() {
        let p = parse("**a _b_ c**");
        match &p[0] {
            Inline::Emph { mark, children, .. } => {
                assert_eq!(*mark, Mark::Bold);
                assert!(matches!(
                    children[1],
                    Inline::Emph {
                        mark: Mark::Italic,
                        ..
                    }
                ));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            refs("**met [[A**B]]**"),
            vec![(RefKind::Page, "A**B".into())]
        );
    }

    #[test]
    fn links() {
        assert_eq!(
            parse("[site](https://x.y)"),
            vec![Inline::Link {
                text: "site".into(),
                url: "https://x.y".into(),
                range: 0..19
            }]
        );
    }

    #[test]
    fn ac_2_8_longest_match_tags() {
        let live = |t: &str| crate::title::normalize_title(t) == "modern religion";
        assert_eq!(
            heal_tags("I like #Modern Religion.", live).as_deref(),
            Some("I like #[[Modern Religion]].")
        );
        assert_eq!(heal_tags("#modern society", live), None);
        assert_eq!(heal_tags("#2 pencils", live), None);
        assert_eq!(
            whole_phrase_tag("#modern religion now", 0),
            Some((0..20, "#[[modern religion now]]".into()))
        );
        assert_eq!(whole_phrase_tag("#solo.", 0), None);
    }

    #[test]
    fn embeds_attributes_and_mentions() {
        let s = "see {{embed: ((abc123))}} and {{[[embed]]: ((xyz))}} {{not embed}}";
        assert_eq!(
            refs(s),
            vec![
                (RefKind::Block, "abc123".into()),
                (RefKind::Block, "xyz".into())
            ]
        );
        assert_eq!(
            attribute("Email:: dana@example.com"),
            Some(("Email".into(), 8..24))
        );
        assert_eq!(attribute("no attribute here"), None);
        assert_eq!(attribute("[[A]]:: x"), None);
        let t = "Mindfulness is key; [[Mindfulness]] and mindfulness-ish, unmindfulness. `mindfulness` MINDFULNESS";
        let m: Vec<&str> = find_mentions(t, "mindfulness")
            .iter()
            .map(|r| &t[r.clone()])
            .collect();
        assert_eq!(m, vec!["Mindfulness", "mindfulness", "MINDFULNESS"]);
        assert_eq!(
            find_mentions("Modern Religion today", "modern religion").len(),
            1
        );
        assert!(find_mentions("anything", "").is_empty());
    }

    #[test]
    fn link_text_forms() {
        assert_eq!(
            page_link_text("Modern Religion", true),
            "#[[Modern Religion]]"
        );
        assert_eq!(page_link_text("mindfulness", true), "#mindfulness");
        assert_eq!(page_link_text("Alpha", false), "[[Alpha]]");
    }

    #[test]
    fn rewrite_and_unlink() {
        let m = |t: &str| t == "Alpha";
        assert_eq!(
            rewrite_page_refs("see [[Alpha]] and #Alpha", m, "Alpha Prime").as_deref(),
            Some("see [[Alpha Prime]] and #[[Alpha Prime]]")
        );
        assert_eq!(
            unlink_page_refs("see [[Alpha]]!", m).as_deref(),
            Some("see Alpha!")
        );
    }

    #[test]
    fn unicode_offsets_are_bytes() {
        let s = "日本 [[東京]] 🎉 #タグ";
        for r in extract_refs(s) {
            assert!(s.is_char_boundary(r.range.start) && s.is_char_boundary(r.range.end));
        }
        assert_eq!(
            refs(s),
            vec![
                (RefKind::Page, "東京".into()),
                (RefKind::Tag, "タグ".into())
            ]
        );
    }

    fn urls(s: &str) -> Vec<String> {
        fn walk(nodes: &[Inline], out: &mut Vec<String>) {
            for n in nodes {
                match n {
                    Inline::Link { url, .. } => out.push(url.clone()),
                    Inline::Emph { children, .. } => walk(children, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&parse(s), &mut out);
        out
    }

    #[test]
    fn a_bare_url_is_a_link() {
        let t = "read https://example.com/a?b=1 later";
        assert_eq!(
            parse(t),
            vec![
                Inline::Text("read ".into()),
                Inline::Link {
                    text: "https://example.com/a?b=1".into(),
                    url: "https://example.com/a?b=1".into(),
                    range: 5..30,
                },
                Inline::Text(" later".into()),
            ]
        );
        assert_eq!(urls("HTTP://X.Y"), ["HTTP://X.Y"]);
        assert_eq!(urls("http://x.y"), ["http://x.y"]);
        for t in ["https://", "ftp://x.y", "xhttps://x.y", "`https://x.y`"] {
            assert_eq!(urls(t), Vec::<String>::new(), "{t}");
        }
        assert_eq!(urls("[site](https://x.y)"), ["https://x.y"]);
    }

    #[test]
    fn a_bare_url_leaves_trailing_punctuation_out() {
        assert_eq!(urls("see https://x.y."), ["https://x.y"]);
        assert_eq!(urls("(see https://x.y)."), ["https://x.y"]);
        assert_eq!(urls("is it https://x.y/?"), ["https://x.y/"]);
        assert_eq!(
            urls("https://en.wikipedia.org/wiki/Rust_(language)"),
            ["https://en.wikipedia.org/wiki/Rust_(language)"]
        );
        assert_eq!(urls("**https://x.y**"), ["https://x.y"]);
        assert!(matches!(
            &parse("**https://x.y**")[0],
            Inline::Emph {
                mark: Mark::Bold,
                ..
            }
        ));
        assert_eq!(urls("<https://x.y>"), ["https://x.y"]);
    }

    #[test]
    fn a_bare_url_holds_no_tags_or_emphasis() {
        let t = "https://x.y/#intro and https://x.y/a_b_c_ #real";
        assert_eq!(refs(t), vec![(RefKind::Tag, "real".into())]);
        assert_eq!(urls(t), ["https://x.y/#intro", "https://x.y/a_b_c"]);
        assert_eq!(heal_tags("https://x.y/#a b", |p| p == "a b"), None);
        assert_eq!(plain_text("go https://x.y/#a"), "go https://x.y/#a");
        assert_eq!(find_mentions("https://x.y/ana ana", "ana"), vec![16..19]);
    }

    #[test]
    fn external_urls_leave_the_app() {
        for u in ["https://x.y", "http://x.y", "mailto:a@b.c"] {
            assert!(is_external_url(u), "{u}");
        }
        for u in [
            "blob:abc",
            "pdfpage:abc#2",
            "utopia://page/x",
            "UTOPIA://page/x",
        ] {
            assert!(!is_external_url(u), "{u}");
        }
    }
}
