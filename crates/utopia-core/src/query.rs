use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Ref(String),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
    Between(String, String),
    Attr { key: String, value: Option<String> },
    Text(String),
    Todo,
    Done,
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, op: &str, xs: &[Expr]| {
            write!(f, "{{{op}:")?;
            for x in xs {
                write!(f, " {x}")?;
            }
            write!(f, "}}")
        };
        match self {
            Expr::Ref(t) => write!(f, "[[{t}]]"),
            Expr::And(xs) => list(f, "and", xs),
            Expr::Or(xs) => list(f, "or", xs),
            Expr::Not(x) => write!(f, "{{not: {x}}}"),
            Expr::Between(a, b) => write!(f, "{{between: [[{a}]] [[{b}]]}}"),
            Expr::Attr { key, value: None } => write!(f, "{{attr: {key}}}"),
            Expr::Attr {
                key,
                value: Some(v),
            } => write!(f, "{{attr: {key} {v}}}"),
            Expr::Text(t) => write!(f, "\"{t}\""),
            Expr::Todo => write!(f, "[[TODO]]"),
            Expr::Done => write!(f, "[[DONE]]"),
        }
    }
}

pub fn query_block(text: &str) -> Option<Result<Expr, String>> {
    let t = text.trim();
    let inner = t.strip_prefix("{{")?.strip_suffix("}}")?.trim_start();
    let body = inner
        .strip_prefix("query:")
        .or_else(|| inner.strip_prefix("[[query]]:"))?;
    Some(parse(body))
}

pub fn parse(src: &str) -> Result<Expr, String> {
    let mut p = Parser { s: src, i: 0 };
    let mut terms = Vec::new();
    loop {
        p.ws();
        if p.done() {
            break;
        }
        terms.push(p.term()?);
    }
    match terms.len() {
        0 => Err("empty query".into()),
        1 => Ok(terms.pop().expect("one term")),
        _ => Ok(Expr::And(terms)),
    }
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }

    fn done(&self) -> bool {
        self.i >= self.s.len()
    }

    fn ws(&mut self) {
        let r = self.rest();
        self.i += r.len() - r.trim_start_matches([' ', '\t', '\n', ',']).len();
    }

    fn term(&mut self) -> Result<Expr, String> {
        self.ws();
        let r = self.rest();
        if r.starts_with("[[") {
            let t = self.bracketed()?;
            Ok(page_term(t))
        } else if let Some(after) = r.strip_prefix('#') {
            self.i += 1;
            if after.starts_with("[[") {
                let t = self.bracketed()?;
                return Ok(page_term(t));
            }
            let len = after
                .find(|c: char| c.is_whitespace() || matches!(c, '}' | ',' | '{'))
                .unwrap_or(after.len());
            if len == 0 {
                return Err("empty #tag".into());
            }
            self.i += len;
            Ok(page_term(after[..len].to_string()))
        } else if let Some(after) = r.strip_prefix('"') {
            let end = after.find('"').ok_or("unterminated \"text\"")?;
            self.i += end + 2;
            let t = after[..end].trim();
            if t.is_empty() {
                return Err("empty \"text\"".into());
            }
            Ok(Expr::Text(t.to_string()))
        } else if r.starts_with('{') {
            self.i += 1;
            self.ws();
            let r = self.rest();
            let op_len = r.find(':').ok_or("expected {op: …}")?;
            let op = r[..op_len].trim().to_ascii_lowercase();
            self.i += op_len + 1;
            let mut args = Vec::new();
            match op.as_str() {
                "attr" | "attribute" => {
                    self.ws();
                    let close = self.rest().find('}').ok_or("unclosed {attr: …}")?;
                    let body = self.rest()[..close].trim();
                    self.i += close + 1;
                    let (key, value) = match body.split_once(char::is_whitespace) {
                        Some((k, v)) => (k, Some(strip_brackets(v.trim()))),
                        None => (body, None),
                    };
                    let key = strip_brackets(key.trim_end_matches(':'));
                    if key.is_empty() {
                        return Err("{attr: …} needs a key".into());
                    }
                    return Ok(Expr::Attr {
                        key,
                        value: value.filter(|v| !v.is_empty()),
                    });
                }
                "and" | "or" | "not" | "between" => {}
                other => return Err(format!("unknown operator {other:?}")),
            }
            loop {
                self.ws();
                if self.rest().starts_with('}') {
                    self.i += 1;
                    break;
                }
                if self.done() {
                    return Err(format!("unclosed {{{op}: …}}"));
                }
                args.push(self.term()?);
            }
            match op.as_str() {
                "and" | "or" if args.is_empty() => Err(format!("{{{op}: …}} needs a term")),
                "and" => Ok(Expr::And(args)),
                "or" => Ok(Expr::Or(args)),
                "not" => match args.len() {
                    1 => Ok(Expr::Not(Box::new(args.pop().expect("one")))),
                    0 => Err("{not: …} needs a term".into()),
                    _ => Ok(Expr::Not(Box::new(Expr::Or(args)))),
                },
                _ => match args.as_slice() {
                    [Expr::Ref(a), Expr::Ref(b)] => Ok(Expr::Between(a.clone(), b.clone())),
                    _ => Err("{between: [[date]] [[date]]} needs two dates".into()),
                },
            }
        } else {
            let end = r
                .find(|c: char| c.is_whitespace() || c == '}')
                .unwrap_or(r.len());
            Err(format!("unexpected {:?}", &r[..end.max(1)]))
        }
    }

    fn bracketed(&mut self) -> Result<String, String> {
        let r = self.rest();
        let mut depth = 0usize;
        let b = r.as_bytes();
        let mut j = 0;
        while j + 1 < b.len() {
            if b[j] == b'[' && b[j + 1] == b'[' {
                depth += 1;
                j += 2;
            } else if b[j] == b']' && b[j + 1] == b']' {
                depth -= 1;
                j += 2;
                if depth == 0 {
                    let t = r[2..j - 2].trim().to_string();
                    self.i += j;
                    return if t.is_empty() {
                        Err("empty [[ ]]".into())
                    } else {
                        Ok(t)
                    };
                }
            } else {
                j += 1;
            }
        }
        Err("unclosed [[".into())
    }
}

fn page_term(t: String) -> Expr {
    match t.as_str() {
        "TODO" => Expr::Todo,
        "DONE" => Expr::Done,
        _ => Expr::Ref(t),
    }
}

fn strip_brackets(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix("[[")
        .and_then(|x| x.strip_suffix("]]"))
        .or_else(|| s.strip_prefix('#'))
        .unwrap_or(s)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: &str) -> Expr {
        Expr::Ref(t.into())
    }

    #[test]
    fn parses_roam_queries() {
        assert_eq!(
            query_block("{{query: {and: [[Books]] #toread}}}"),
            Some(Ok(Expr::And(vec![r("Books"), r("toread")])))
        );
        assert_eq!(
            query_block("{{[[query]]: {or: [[A]] {not: [[B]]}}}}"),
            Some(Ok(Expr::Or(vec![r("A"), Expr::Not(Box::new(r("B")))])))
        );
        assert_eq!(
            query_block("{{query: [[TODO]] #[[Project Atlas]]}}"),
            Some(Ok(Expr::And(vec![Expr::Todo, r("Project Atlas")])))
        );
        assert_eq!(
            parse("{between: [[November 1st, 2026]] [[November 30th, 2026]]}"),
            Ok(Expr::Between(
                "November 1st, 2026".into(),
                "November 30th, 2026".into()
            ))
        );
        assert_eq!(
            parse("{attr: Status [[done]]} \"ship it\""),
            Ok(Expr::And(vec![
                Expr::Attr {
                    key: "Status".into(),
                    value: Some("done".into())
                },
                Expr::Text("ship it".into())
            ]))
        );
        assert_eq!(
            parse("{attr: Author}"),
            Ok(Expr::Attr {
                key: "Author".into(),
                value: None
            })
        );
        assert_eq!(parse("[[a [[b]]]]"), Ok(r("a [[b]]")));
        assert_eq!(query_block("plain text"), None);
        assert_eq!(query_block("{{embed: ((abc))}}"), None);
    }

    #[test]
    fn reports_errors() {
        for bad in [
            "",
            "{and: [[A]]",
            "{xor: [[A]]}",
            "[[A",
            "{between: [[A]]}",
            "{not:}",
            "bare words",
            "\"open",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should fail");
        }
        assert!(matches!(query_block("{{query: {and: }}}"), Some(Err(_))));
    }

    #[test]
    fn display_round_trips() {
        let src =
            "{and: [[A]] {or: [[B]] \"x y\"} {not: [[TODO]]} {attr: k v} {between: [[d1]] [[d2]]}}";
        let e = parse(src).unwrap();
        assert_eq!(parse(&e.to_string()).unwrap(), e);
    }
}
