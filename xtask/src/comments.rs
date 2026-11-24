use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BASELINE: &str = "xtask/comment-baseline.txt";
const EXTENSIONS: [&str; 3] = ["rs", "swift", "ts"];
const EXEMPT_PREFIXES: [&str; 1] = ["SAFETY:"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Rust,
    Swift,
    TypeScript,
}

impl Lang {
    pub fn for_path(path: &Path) -> Option<Lang> {
        match path.extension()?.to_str()? {
            "rs" => Some(Lang::Rust),
            "swift" => Some(Lang::Swift),
            "ts" => Some(Lang::TypeScript),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub line: usize,
    pub end_line: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    Code {
        closer: Option<char>,
        depth: usize,
    },
    Str {
        raw_hashes: Option<usize>,
        multiline: bool,
    },
    Template,
}

pub fn scan(src: &str, lang: Lang) -> Vec<Comment> {
    let chars: Vec<char> = src.chars().collect();
    let mut comments = Vec::new();
    let mut stack = vec![Frame::Code {
        closer: None,
        depth: 0,
    }];
    let mut line = 1;
    let mut i = 0;
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    while i < chars.len() {
        let c = chars[i];
        let top = *stack.last().expect("the code frame is never popped");
        match top {
            Frame::Code { closer, depth } => {
                if c == '/' && at(i + 1) == '/' {
                    let start = i;
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                    comments.push(Comment {
                        line,
                        end_line: line,
                        start,
                        end: i,
                        text: chars[start..i].iter().collect(),
                    });
                    continue;
                }
                if c == '/' && at(i + 1) == '*' {
                    let start = i;
                    let first = line;
                    let mut nest = 0usize;
                    while i < chars.len() {
                        if chars[i] == '/' && at(i + 1) == '*' {
                            nest += 1;
                            i += 2;
                            if lang == Lang::TypeScript {
                                nest = 1;
                            }
                            continue;
                        }
                        if chars[i] == '*' && at(i + 1) == '/' {
                            nest -= 1;
                            i += 2;
                            if nest == 0 {
                                break;
                            }
                            continue;
                        }
                        if chars[i] == '\n' {
                            line += 1;
                        }
                        i += 1;
                    }
                    let end = i.min(chars.len());
                    comments.push(Comment {
                        line: first,
                        end_line: line,
                        start,
                        end,
                        text: chars[start..end].iter().collect(),
                    });
                    continue;
                }
                if let Some(close) = closer {
                    let open = if close == ')' { '(' } else { '{' };
                    if c == open {
                        stack.pop();
                        stack.push(Frame::Code {
                            closer,
                            depth: depth + 1,
                        });
                    } else if c == close {
                        stack.pop();
                        if depth > 0 {
                            stack.push(Frame::Code {
                                closer,
                                depth: depth - 1,
                            });
                        }
                        i += 1;
                        continue;
                    }
                }
                let ident_before = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
                match (lang, c) {
                    (Lang::Rust, 'r') | (Lang::Rust, 'b') | (Lang::Rust, 'c') if !ident_before => {
                        let mut j = i + 1;
                        if c != 'r' && at(j) == 'r' {
                            j += 1;
                        } else if c != 'r' {
                            i += 1;
                            continue;
                        }
                        let hashes_start = j;
                        while at(j) == '#' {
                            j += 1;
                        }
                        if at(j) == '"' {
                            stack.push(Frame::Str {
                                raw_hashes: Some(j - hashes_start),
                                multiline: false,
                            });
                            i = j + 1;
                            continue;
                        }
                    }
                    (Lang::Rust, '\'') => {
                        if at(i + 1) == '\\' {
                            let mut j = i + 3;
                            while j < chars.len() && chars[j] != '\'' && chars[j] != '\n' {
                                j += 1;
                            }
                            i = j + 1;
                            continue;
                        }
                        if at(i + 2) == '\'' {
                            i += 3;
                            continue;
                        }
                    }
                    (Lang::Swift, '#') if at(i + 1) == '#' || at(i + 1) == '"' => {
                        let mut j = i;
                        while at(j) == '#' {
                            j += 1;
                        }
                        if at(j) == '"' {
                            let multiline = at(j + 1) == '"' && at(j + 2) == '"';
                            stack.push(Frame::Str {
                                raw_hashes: Some(j - i),
                                multiline,
                            });
                            i = j + if multiline { 3 } else { 1 };
                            continue;
                        }
                    }
                    (_, '"') => {
                        let multiline = lang == Lang::Swift && at(i + 1) == '"' && at(i + 2) == '"';
                        stack.push(Frame::Str {
                            raw_hashes: None,
                            multiline,
                        });
                        i += if multiline { 3 } else { 1 };
                        continue;
                    }
                    (Lang::TypeScript, '\'') => {
                        let mut j = i + 1;
                        while j < chars.len() && chars[j] != '\'' && chars[j] != '\n' {
                            j += if chars[j] == '\\' { 2 } else { 1 };
                        }
                        i = j + 1;
                        continue;
                    }
                    (Lang::TypeScript, '`') => {
                        stack.push(Frame::Template);
                        i += 1;
                        continue;
                    }
                    _ => {}
                }
                if c == '\n' {
                    line += 1;
                }
                i += 1;
            }
            Frame::Str {
                raw_hashes,
                multiline,
            } => {
                if c == '\n' {
                    line += 1;
                    i += 1;
                    continue;
                }
                let escapes = match (lang, raw_hashes) {
                    (Lang::Rust, Some(_)) => false,
                    (Lang::Swift, Some(n)) => c == '\\' && (0..n).all(|k| at(i + 1 + k) == '#'),
                    _ => c == '\\',
                };
                if escapes {
                    let skip = raw_hashes.unwrap_or(0);
                    let next = at(i + 1 + skip);
                    if lang == Lang::Swift && next == '(' {
                        stack.push(Frame::Code {
                            closer: Some(')'),
                            depth: 0,
                        });
                        i += 2 + skip;
                        continue;
                    }
                    if next == '\n' {
                        line += 1;
                    }
                    i += 2 + skip;
                    continue;
                }
                if c == '"' {
                    let quotes = if multiline { 3 } else { 1 };
                    let closes_quotes = (0..quotes).all(|k| at(i + k) == '"');
                    let hashes = raw_hashes.unwrap_or(0);
                    let closes_hashes = (0..hashes).all(|k| at(i + quotes + k) == '#');
                    if closes_quotes && closes_hashes {
                        stack.pop();
                        i += quotes + hashes;
                        continue;
                    }
                }
                i += 1;
            }
            Frame::Template => {
                match c {
                    '\\' => {
                        if at(i + 1) == '\n' {
                            line += 1;
                        }
                        i += 2;
                        continue;
                    }
                    '`' => {
                        stack.pop();
                    }
                    '$' if at(i + 1) == '{' => {
                        stack.push(Frame::Code {
                            closer: Some('}'),
                            depth: 0,
                        });
                        i += 2;
                        continue;
                    }
                    '\n' => line += 1,
                    _ => {}
                }
                i += 1;
            }
        }
    }
    comments
}

pub fn exempt(comments: &[Comment]) -> Vec<bool> {
    let mut flags = Vec::with_capacity(comments.len());
    let mut run_end = None;
    for c in comments {
        let body = c.text.trim_start_matches(['/', '*', '!']).trim_start();
        let continues_run = run_end == Some(c.line.saturating_sub(1))
            && c.text.starts_with("//")
            && !c.text.starts_with("///");
        let is_exempt = EXEMPT_PREFIXES.iter().any(|p| body.starts_with(p)) || continues_run;
        run_end = is_exempt.then_some(c.end_line);
        flags.push(is_exempt);
    }
    flags
}

pub fn counted_lines(comments: &[Comment]) -> usize {
    let mut lines = std::collections::BTreeSet::new();
    for (c, is_exempt) in comments.iter().zip(exempt(comments)) {
        if !is_exempt {
            lines.extend(c.line..=c.end_line);
        }
    }
    lines.len()
}

pub fn strip(src: &str, lang: Lang) -> String {
    let chars: Vec<char> = src.chars().collect();
    let comments = scan(src, lang);
    let mut cut = vec![false; chars.len()];
    for (c, is_exempt) in comments.iter().zip(exempt(&comments)) {
        if is_exempt {
            continue;
        }
        let mut from = c.start;
        while from > 0 && matches!(chars[from - 1], ' ' | '\t') {
            from -= 1;
        }
        let mut to = c.end;
        while to < chars.len() && matches!(chars[to], ' ' | '\t') {
            to += 1;
        }
        let owns_line =
            (from == 0 || chars[from - 1] == '\n') && (to == chars.len() || chars[to] == '\n');
        if owns_line && to < chars.len() {
            to += 1;
        } else if !owns_line {
            to = c.end;
        }
        cut[from..to].iter_mut().for_each(|x| *x = true);
    }
    chars
        .iter()
        .zip(cut)
        .filter(|(_, gone)| !gone)
        .map(|(c, _)| *c)
        .collect()
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits one level below the repo root")
        .to_path_buf()
}

pub fn tracked_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).args(["ls-files", "-z", "--"]);
    for ext in EXTENSIONS {
        cmd.arg(format!("*.{ext}"));
    }
    let out = cmd.output().context("git ls-files")?;
    if !out.status.success() {
        bail!("git ls-files: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8(out.stdout)?
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| root.join(p).is_file())
        .collect())
}

pub fn measure(root: &Path) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    for rel in tracked_files(root)? {
        let Some(lang) = Lang::for_path(&rel) else {
            continue;
        };
        let src = std::fs::read_to_string(root.join(&rel))
            .with_context(|| format!("read {}", rel.display()))?;
        let n = counted_lines(&scan(&src, lang));
        if n > 0 {
            counts.insert(rel.to_string_lossy().replace('\\', "/"), n);
        }
    }
    Ok(counts)
}

pub fn read_baseline(root: &Path) -> Result<BTreeMap<String, usize>> {
    let text = match std::fs::read_to_string(root.join(BASELINE)) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e).context(BASELINE),
    };
    let mut map = BTreeMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (count, path) = line
            .split_once(char::is_whitespace)
            .with_context(|| format!("{BASELINE}:{}: expected `<count> <path>`", n + 1))?;
        let count = count
            .parse()
            .with_context(|| format!("{BASELINE}:{}: bad count", n + 1))?;
        map.insert(path.trim().to_string(), count);
    }
    Ok(map)
}

pub fn render_baseline(counts: &BTreeMap<String, usize>) -> String {
    let total: usize = counts.values().sum();
    let mut out = format!(
        "# Comment ratchet: comment lines each file may still hold (docs/adr/0001-no-code-comments.md).\n\
         # A count may only go down. Regenerate with `cargo run -p xtask -- comments --update`.\n\
         # total {total}\n"
    );
    for (path, n) in counts {
        out.push_str(&format!("{n} {path}\n"));
    }
    out
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub grown: Vec<(String, usize, usize)>,
    pub shrunk: Vec<(String, usize, usize)>,
}

pub fn compare(baseline: &BTreeMap<String, usize>, now: &BTreeMap<String, usize>) -> Report {
    let mut report = Report::default();
    for (path, &n) in now {
        let allowed = baseline.get(path).copied().unwrap_or(0);
        if n > allowed {
            report.grown.push((path.clone(), allowed, n));
        } else if n < allowed {
            report.shrunk.push((path.clone(), allowed, n));
        }
    }
    for (path, &allowed) in baseline {
        if !now.contains_key(path) && allowed > 0 {
            report.shrunk.push((path.clone(), allowed, 0));
        }
    }
    report.shrunk.sort();
    report
}

pub fn check(root: &Path) -> Result<()> {
    let report = compare(&read_baseline(root)?, &measure(root)?);
    let mut msg = String::new();
    for (path, allowed, n) in &report.grown {
        msg.push_str(&format!(
            "  {path}: {n} comment lines, the ratchet allows {allowed}\n"
        ));
    }
    if !msg.is_empty() {
        bail!(
            "new code comments (docs/adr/0001-no-code-comments.md). Put the reason in a \
             name, a type, a test or an ADR under docs/adr/, then delete the comment. \
             List them with `cargo run -p xtask -- comments --list <path>`.\n{msg}"
        );
    }
    for (path, allowed, n) in &report.shrunk {
        msg.push_str(&format!("  {path}: {allowed} -> {n}\n"));
    }
    if !msg.is_empty() {
        bail!(
            "fewer comments than the ratchet records, so tighten it: run \
             `cargo run -p xtask -- comments --update` and commit {BASELINE}.\n{msg}"
        );
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<()> {
    let root = repo_root();
    match args.first().map(String::as_str) {
        None | Some("--check") => {
            check(&root)?;
            let total: usize = measure(&root)?.values().sum();
            println!("comment ratchet holds: {total} comment lines left");
            Ok(())
        }
        Some("--update") => {
            let now = measure(&root)?;
            let base = read_baseline(&root)?;
            let report = compare(&base, &now);
            if !report.grown.is_empty() && !args.iter().any(|a| a == "--allow-growth") {
                bail!(
                    "refusing to raise the ratchet for {} file(s), first {:?}; pass \
                     --allow-growth only for a file that moved or was renamed",
                    report.grown.len(),
                    report.grown[0]
                );
            }
            std::fs::write(root.join(BASELINE), render_baseline(&now))?;
            println!(
                "{BASELINE}: {} -> {} comment lines",
                base.values().sum::<usize>(),
                now.values().sum::<usize>()
            );
            Ok(())
        }
        Some("--list") => {
            let paths: Vec<PathBuf> = if args.len() > 1 {
                args[1..].iter().map(PathBuf::from).collect()
            } else {
                tracked_files(&root)?
            };
            for rel in paths {
                let files = if root.join(&rel).is_dir() {
                    tracked_files(&root)?
                        .into_iter()
                        .filter(|p| p.starts_with(&rel))
                        .collect()
                } else {
                    vec![rel]
                };
                for file in files {
                    let Some(lang) = Lang::for_path(&file) else {
                        continue;
                    };
                    let src = std::fs::read_to_string(root.join(&file))?;
                    for c in scan(&src, lang) {
                        let first = c.text.lines().next().unwrap_or("");
                        println!("{}:{}: {}", file.display(), c.line, first.trim());
                    }
                }
            }
            Ok(())
        }
        Some("--strip") => {
            if args.len() < 2 {
                bail!("comments --strip <path…>: name the files or folders to strip");
            }
            let tracked = tracked_files(&root)?;
            let mut stripped = 0;
            for arg in &args[1..] {
                let rel = PathBuf::from(arg);
                for file in tracked.iter().filter(|p| p.starts_with(&rel)) {
                    let Some(lang) = Lang::for_path(file) else {
                        continue;
                    };
                    let path = root.join(file);
                    let src = std::fs::read_to_string(&path)?;
                    let out = strip(&src, lang);
                    if out != src {
                        std::fs::write(&path, out)?;
                        stripped += 1;
                        println!("stripped {}", file.display());
                    }
                }
            }
            println!("{stripped} file(s) changed; run `cargo fmt --all`, then `comments --update`");
            Ok(())
        }
        Some("--total") => {
            let now = measure(&root)?;
            println!("{}", now.values().sum::<usize>());
            Ok(())
        }
        Some(other) => bail!("comments: unknown flag {other}; use --check, --update, --list [path…], --strip <path…> or --total"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str, lang: Lang) -> Vec<usize> {
        scan(src, lang).iter().map(|c| c.line).collect()
    }

    #[test]
    fn rust_line_doc_and_block_comments_are_found() {
        let src =
            "//! module\n/// item\nfn a() {} // tail\n/* one\n /* nested */\n two */\nfn b() {}\n";
        let found = scan(src, Lang::Rust);
        assert_eq!(
            found.iter().map(|c| c.line).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(found[3].end_line, 6);
        assert_eq!(counted_lines(&found), 6);
    }

    #[test]
    fn rust_strings_hide_slashes() {
        let src = r####"let a = "http://x"; let b = r#"// not "a" comment"#; let c = b"//";
let d = '/'; let e = '\''; fn f<'a>(x: &'a str) {} let g = "\"//";
"####;
        assert!(
            scan(src, Lang::Rust).is_empty(),
            "{:?}",
            scan(src, Lang::Rust)
        );
    }

    #[test]
    fn rust_identifiers_ending_in_r_are_not_raw_strings() {
        let src = "let br = 1; let x = for_r#\"\"; // real\n";
        assert_eq!(lines(src, Lang::Rust), vec![1]);
    }

    #[test]
    fn safety_comments_are_exempt_with_their_continuation() {
        let src =
            "// SAFETY: the pointer is valid\n// for the whole call.\nunsafe { f() }\n// counted\n";
        assert_eq!(counted_lines(&scan(src, Lang::Rust)), 1);
    }

    #[test]
    fn swift_interpolation_and_multiline_strings_hide_slashes() {
        let src = "let a = \"\\(f(\"//\")) //\"\nlet b = \"\"\"\n// inside\n\"\"\"\nlet c = #\"//\"# // real\n";
        assert_eq!(lines(src, Lang::Swift), vec![5]);
    }

    #[test]
    fn typescript_templates_and_quotes_hide_slashes() {
        let src = "const a = `//${b + `//`}//`; const c = '//'; // real\n/** doc */\n";
        assert_eq!(lines(src, Lang::TypeScript), vec![1, 2]);
    }

    #[test]
    fn strip_removes_comment_lines_and_tails_but_keeps_safety() {
        let src = "//! module\nfn a() {} // tail\n    /// doc\n    let s = \"// kept\";\n// SAFETY: why\nunsafe {}\nx /* mid */ + y\n";
        assert_eq!(
            strip(src, Lang::Rust),
            "fn a() {}\n    let s = \"// kept\";\n// SAFETY: why\nunsafe {}\nx + y\n"
        );
    }

    #[test]
    fn ratchet_reports_growth_and_shrinkage() {
        let base = BTreeMap::from([("a.rs".to_string(), 3), ("gone.rs".to_string(), 2)]);
        let now = BTreeMap::from([("a.rs".to_string(), 4), ("new.rs".to_string(), 1)]);
        let r = compare(&base, &now);
        assert_eq!(
            r.grown,
            vec![("a.rs".to_string(), 3, 4), ("new.rs".to_string(), 0, 1)]
        );
        assert_eq!(r.shrunk, vec![("gone.rs".to_string(), 2, 0)]);
    }

    #[test]
    fn baseline_round_trips() {
        let counts = BTreeMap::from([("x/y.rs".to_string(), 7)]);
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("xtask")).unwrap();
        std::fs::write(dir.path().join(BASELINE), render_baseline(&counts)).unwrap();
        assert_eq!(read_baseline(dir.path()).unwrap(), counts);
    }

    #[test]
    fn the_repository_holds_the_comment_ratchet() {
        check(&repo_root()).unwrap();
    }
}
