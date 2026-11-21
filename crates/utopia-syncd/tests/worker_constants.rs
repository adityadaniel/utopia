//! Constants the Cloudflare Worker (`relay-cloudflare/src`) must share with `utopia-syncd`
//! and `utopia-proto` (PRD §12.3, §12.8). Read from the TypeScript as text, so this runs in
//! every `cargo test` with no Node: a change on one side fails here until the other follows.

use std::path::{Path, PathBuf};

fn relay_cloudflare() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../relay-cloudflare")
}

/// `export const NAME = <expr>;` (an optional `: number` annotation allowed) in exactly one
/// file of `relay-cloudflare/src`: the file, relative to the repository, and the expression.
fn ts_const(name: &str) -> (String, String) {
    let src = relay_cloudflare().join("src");
    let mut found = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&src)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", src.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ts"))
        .collect();
    files.sort();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let needle = format!("const {name}");
        let mut from = 0;
        while let Some(at) = text[from..].find(&needle) {
            let start = from + at;
            from = start + needle.len();
            let before = text[..start].chars().next_back();
            let after = text[from..].chars().next();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if word(before) || word(after) {
                continue;
            }
            let rest = &text[from..];
            let Some(eq) = rest.find('=') else { continue };
            let annotation = rest[..eq].trim();
            if !(annotation.is_empty() || annotation == ": number") {
                continue;
            }
            let end = rest[eq..].find(';').map(|i| eq + i).unwrap_or(rest.len());
            let rel = format!(
                "relay-cloudflare/src/{}",
                path.file_name().unwrap().to_string_lossy()
            );
            found.push((rel, rest[eq + 1..end].trim().to_string()));
        }
    }
    match found.len() {
        1 => found.pop().unwrap(),
        0 => panic!(
            "relay-cloudflare/src defines no `const {name}`: if it moved or was renamed, \
             point crates/utopia-syncd/tests/worker_constants.rs at it"
        ),
        _ => panic!("relay-cloudflare/src defines `const {name}` more than once: {found:?}"),
    }
}

/// A TypeScript integer expression of literals, `( )`, `*`, `+`, `-` and `<<`, evaluated
/// with JavaScript's semantics (`<<` works on 32-bit signed integers).
fn eval(expr: &str) -> Result<i64, String> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }
        fn eat(&mut self, tok: &str) -> bool {
            self.ws();
            if self.s[self.i..].starts_with(tok.as_bytes()) {
                self.i += tok.len();
                true
            } else {
                false
            }
        }
        fn shift(&mut self) -> Result<i64, String> {
            let mut v = self.additive()?;
            while self.eat("<<") {
                let by = self.additive()?;
                v = ((v as i32).wrapping_shl(by as u32 & 31)) as i64;
            }
            Ok(v)
        }
        fn additive(&mut self) -> Result<i64, String> {
            let mut v = self.term()?;
            loop {
                if self.eat("+") {
                    v += self.term()?;
                } else if self.eat("-") {
                    v -= self.term()?;
                } else {
                    return Ok(v);
                }
            }
        }
        fn term(&mut self) -> Result<i64, String> {
            let mut v = self.atom()?;
            while self.eat("*") {
                v *= self.atom()?;
            }
            Ok(v)
        }
        fn atom(&mut self) -> Result<i64, String> {
            if self.eat("(") {
                let v = self.shift()?;
                if !self.eat(")") {
                    return Err("unbalanced parenthesis".into());
                }
                return Ok(v);
            }
            self.ws();
            let start = self.i;
            while self.i < self.s.len()
                && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
            {
                self.i += 1;
            }
            let lit = std::str::from_utf8(&self.s[start..self.i])
                .unwrap()
                .replace('_', "");
            let parsed = match lit.strip_prefix("0x") {
                Some(hex) => i64::from_str_radix(hex, 16),
                None => lit.parse(),
            };
            parsed.map_err(|_| format!("not an integer literal: {lit:?}"))
        }
    }
    let mut p = P {
        s: expr.as_bytes(),
        i: 0,
    };
    let v = p.shift()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(format!("cannot evaluate {expr:?} past byte {}", p.i));
    }
    Ok(v)
}

fn assert_ts_const(name: &str, rust: i64, rust_name: &str) {
    let (file, expr) = ts_const(name);
    let ts = eval(&expr).unwrap_or_else(|e| panic!("{file}: `{name} = {expr}`: {e}"));
    assert_eq!(
        ts, rust,
        "{file} has `{name} = {expr}` ({ts}), but {rust_name} is {rust}: the two relays \
         must agree; change {file} (or {rust_name}, and then both relays) so they match"
    );
}

#[test]
fn the_worker_speaks_the_same_protocol_version() {
    assert_ts_const(
        "PROTOCOL_VERSION",
        utopia_proto::PROTOCOL_VERSION as i64,
        "utopia_proto::PROTOCOL_VERSION (crates/utopia-proto/src/lib.rs)",
    );
}

#[test]
fn the_worker_has_the_same_body_limit() {
    assert_ts_const(
        "MAX_BODY_BYTES",
        utopia_syncd::MAX_BODY_BYTES as i64,
        "utopia_syncd::MAX_BODY_BYTES (crates/utopia-syncd/src/lib.rs)",
    );
}

#[test]
fn the_worker_has_the_same_default_quota() {
    assert_ts_const(
        "DEFAULT_QUOTA_BYTES",
        utopia_syncd::DEFAULT_QUOTA_BYTES as i64,
        "utopia_syncd::DEFAULT_QUOTA_BYTES (crates/utopia-syncd/src/lib.rs)",
    );
    // The value wrangler.jsonc ships, when it ships one, is the same default.
    let path = relay_cloudflare().join("wrangler.jsonc");
    let text = std::fs::read_to_string(&path).unwrap();
    if let Some(at) = text.find("\"QUOTA_BYTES\"") {
        let rest = &text[at + "\"QUOTA_BYTES\"".len()..];
        let value: String = rest
            .trim_start()
            .trim_start_matches(':')
            .trim_start()
            .trim_start_matches('"')
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        assert_eq!(
            value.parse::<u64>().ok(),
            Some(utopia_syncd::DEFAULT_QUOTA_BYTES),
            "relay-cloudflare/wrangler.jsonc sets QUOTA_BYTES to {value:?}, but \
             utopia_syncd::DEFAULT_QUOTA_BYTES is {}: change relay-cloudflare/wrangler.jsonc",
            utopia_syncd::DEFAULT_QUOTA_BYTES
        );
    }
}

#[test]
fn the_expression_reader_follows_javascript() {
    assert_eq!(eval("(64 << 20) + (64 << 10)"), Ok((64 << 20) + (64 << 10)));
    assert_eq!(eval("1 << 30"), Ok(1 << 30));
    assert_eq!(eval("1_073_741_824"), Ok(1 << 30));
    assert_eq!(
        eval("64 * 1024 * 1024 + 0x10000"),
        Ok((64 << 20) + (64 << 10))
    );
    // JavaScript shifts 32-bit signed integers: what the Worker really gets.
    assert_eq!(eval("1 << 31"), Ok(i32::MIN as i64));
    assert_eq!(eval("1 << 32"), Ok(1));
    assert!(eval("Number(env.X)").is_err());
}
