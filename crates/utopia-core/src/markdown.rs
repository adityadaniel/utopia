use crate::command::BlockKind;
use crate::ids::BlockId;
use crate::tree::PageState;

const INDENT: &str = "  ";

pub fn parse_outline(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut widths: Vec<usize> = Vec::new();
    for line in src.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let width: usize = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum();
        while widths.last().is_some_and(|top| *top > width) {
            widths.pop();
        }
        if widths.last() != Some(&width) {
            widths.push(width);
        }
        let depth = widths.len() - 1;
        out.push((depth, strip_marker(line.trim()).to_string()));
    }
    out
}

fn strip_marker(s: &str) -> &str {
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(m) {
            return rest;
        }
    }
    if matches!(s, "-" | "*" | "+") {
        return "";
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let rest = &s[digits..];
        for m in [". ", ") "] {
            if let Some(r) = rest.strip_prefix(m) {
                return r;
            }
        }
    }
    s
}

fn push_block(st: &PageState, id: &BlockId, depth: usize, out: &mut String) {
    let Some(b) = st.block(id) else { return };
    let pad = INDENT.repeat(depth);
    let prefix = match b.kind {
        BlockKind::Todo if b.checked => "[x] ".to_string(),
        BlockKind::Todo => "[ ] ".to_string(),
        BlockKind::Heading(l) => format!("{} ", "#".repeat(l.clamp(1, 3) as usize)),
        BlockKind::Quote => "> ".to_string(),
        _ => String::new(),
    };
    let mut lines = b.text.split('\n');
    let first = lines.next().unwrap_or("");
    out.push_str(&format!("{pad}- {prefix}{first}\n"));
    for l in lines {
        out.push_str(&format!("{pad}{INDENT}{l}\n"));
    }
}

pub fn blocks_to_markdown(st: &PageState, roots: &[BlockId]) -> String {
    let mut out = String::new();
    for r in roots {
        if !st.is_live(r) {
            continue;
        }
        let base = st.depth(r);
        for id in st.subtree(r, true) {
            push_block(st, &id, st.depth(&id) - base, &mut out);
        }
    }
    out
}

pub fn subtree_to_markdown(st: &PageState, id: &BlockId) -> String {
    blocks_to_markdown(st, std::slice::from_ref(id))
}

pub fn page_to_markdown(st: &PageState, title: &str) -> String {
    format!("# {title}\n\n{}", blocks_to_markdown(st, &st.live_roots()))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn pages_to_opml(pages: &[(String, &PageState)]) -> String {
    fn outline(st: &PageState, id: &BlockId, depth: usize, out: &mut String) {
        let Some(b) = st.block(id) else { return };
        let pad = "  ".repeat(depth);
        let kids = st.live_children(Some(id));
        if kids.is_empty() {
            out.push_str(&format!(
                "{pad}<outline text=\"{}\"/>\n",
                xml_escape(&b.text)
            ));
        } else {
            out.push_str(&format!(
                "{pad}<outline text=\"{}\">\n",
                xml_escape(&b.text)
            ));
            for k in &kids {
                outline(st, k, depth + 1, out);
            }
            out.push_str(&format!("{pad}</outline>\n"));
        }
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n<head><title>Utopia export</title></head>\n<body>\n",
    );
    for (title, st) in pages {
        out.push_str(&format!("  <outline text=\"{}\">\n", xml_escape(title)));
        for r in st.live_roots() {
            outline(st, &r, 2, &mut out);
        }
        out.push_str("  </outline>\n");
    }
    out.push_str("</body>\n</opml>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_markers_and_indentation() {
        let md = "- one\n  - two\n    * three\n- four\n\n1. five\n\t- six\n";
        assert_eq!(
            parse_outline(md),
            vec![
                (0, "one".into()),
                (1, "two".into()),
                (2, "three".into()),
                (0, "four".into()),
                (0, "five".into()),
                (1, "six".into()),
            ]
        );
    }

    #[test]
    fn dedent_to_unseen_width_clamps() {
        let items = parse_outline("a\n    b\n  c\nd");
        assert_eq!(items[1].0, 1);
        assert_eq!(items[2].0, 1);
        assert_eq!(items[3].0, 0);
    }

    #[test]
    fn opml_escapes_and_nests() {
        use crate::command::{BlockKind, Command, PageMeta};
        let mut st = PageState::new("p".into(), PageMeta::default());
        for (id, parent, text) in [("a", None, "A & <b>"), ("b", Some("a"), "child \"q\"")] {
            st.apply(
                &Command::InsertBlock {
                    id: id.into(),
                    parent: parent.map(Into::into),
                    after: None,
                    text: text.into(),
                    kind: BlockKind::Bullet,
                },
                0,
            )
            .unwrap();
        }
        let x = pages_to_opml(&[("Page".into(), &st)]);
        assert!(x.contains("<outline text=\"A &amp; &lt;b&gt;\">"));
        assert!(x.contains("<outline text=\"child &quot;q&quot;\"/>"));
        assert!(x.trim_end().ends_with("</opml>"));
    }

    #[test]
    fn plain_lines_keep_relative_indent() {
        assert_eq!(
            parse_outline("  x\n    y\n  z"),
            vec![(0, "x".into()), (1, "y".into()), (0, "z".into())]
        );
    }
}
