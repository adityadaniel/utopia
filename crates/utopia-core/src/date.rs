use chrono::{Datelike, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

pub const DEFAULT_DATE_FORMAT: &str = "MMMM do, yyyy";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

const BUILTIN_FORMATS: &[&str] = &[
    "MMMM do, yyyy",
    "MMMM d, yyyy",
    "MMM do, yyyy",
    "MMM d, yyyy",
    "MMMM do yyyy",
    "MMMM d yyyy",
    "do MMMM yyyy",
    "d MMMM yyyy",
    "d MMM yyyy",
    "EEEE, MMMM do, yyyy",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Year,
    MonthName,
    MonthAbbr,
    Month2,
    Month,
    DayOrdinal,
    Day2,
    Day,
    Weekday,
    WeekdayAbbr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Tok(Tok),
    Lit(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateFormat {
    pattern: String,
    #[serde(default)]
    previous: Vec<String>,
}

impl Default for DateFormat {
    fn default() -> Self {
        Self::new(DEFAULT_DATE_FORMAT)
    }
}

impl DateFormat {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            previous: Vec::new(),
        }
    }

    pub fn changed_to(&self, pattern: &str) -> Self {
        let mut previous = self.previous.clone();
        previous.push(self.pattern.clone());
        previous.retain(|p| p != pattern);
        previous.dedup();
        Self {
            pattern: pattern.to_string(),
            previous,
        }
    }

    pub fn with_previous(pattern: impl Into<String>, previous: Vec<String>) -> Self {
        Self {
            pattern: pattern.into(),
            previous,
        }
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn previous(&self) -> &[String] {
        &self.previous
    }

    fn pieces(&self) -> Vec<Piece> {
        tokenize(&self.pattern)
    }

    pub fn format(&self, date: NaiveDate) -> String {
        render(&self.pieces(), date)
    }

    pub fn is_unambiguous(&self) -> bool {
        let pieces = self.pieces();
        let toks: Vec<Tok> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Tok(t) => Some(*t),
                Piece::Lit(_) => None,
            })
            .collect();
        let year = toks.iter().position(|t| *t == Tok::Year);
        let month = toks.iter().position(|t| {
            matches!(
                t,
                Tok::MonthName | Tok::MonthAbbr | Tok::Month2 | Tok::Month
            )
        });
        let day = toks
            .iter()
            .position(|t| matches!(t, Tok::DayOrdinal | Tok::Day2 | Tok::Day));
        match (year, month, day) {
            (Some(y), Some(m), Some(d)) => {
                matches!(toks[m], Tok::MonthName | Tok::MonthAbbr) || (y < m && m < d)
            }
            _ => false,
        }
    }

    pub fn parse(&self, s: &str) -> Option<NaiveDate> {
        parse_with(&self.pieces(), s)
    }
}

pub fn parse_unambiguous_date(s: &str, graph_format: &DateFormat) -> Option<NaiveDate> {
    let s = s.trim();
    if let Some(d) = parse_iso(s) {
        return Some(d);
    }
    for f in BUILTIN_FORMATS {
        if let Some(d) = parse_with(&tokenize(f), s) {
            return Some(d);
        }
    }
    if graph_format.is_unambiguous() {
        if let Some(d) = graph_format.parse(s) {
            return Some(d);
        }
    }
    graph_format
        .previous
        .iter()
        .map(|p| DateFormat::new(p.as_str()))
        .filter(DateFormat::is_unambiguous)
        .find_map(|f| f.parse(s))
}

pub fn relative_date(phrase: &str, today: NaiveDate) -> Option<NaiveDate> {
    let lower = phrase.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let days = |n: i64| today.checked_add_signed(chrono::Duration::try_days(n)?);
    let months = |n: i32| {
        if n >= 0 {
            today.checked_add_months(chrono::Months::new(n as u32))
        } else {
            today.checked_sub_months(chrono::Months::new(n.unsigned_abs()))
        }
    };
    let weekday = |w: &str| {
        WEEKDAYS
            .iter()
            .position(|name| {
                let name = name.to_lowercase();
                w == name || (w.len() >= 3 && name.starts_with(w))
            })
            .and_then(|i| Weekday::try_from(i as u8).ok())
    };
    let offset = |n: &str, unit: &str, sign: i64| -> Option<NaiveDate> {
        let n: i64 = match n {
            "a" | "an" | "one" => 1,
            _ => n.parse().ok().filter(|n| (0..=3660).contains(n))?,
        };
        let total = sign.checked_mul(n)?;
        match unit.trim_end_matches('s') {
            "day" => days(total),
            "week" => days(total.checked_mul(7)?),
            "month" => months(i32::try_from(total).ok()?),
            "year" => months(i32::try_from(total.checked_mul(12)?).ok()?),
            _ => None,
        }
    };
    match words.as_slice() {
        ["today"] => Some(today),
        ["tomorrow"] => days(1),
        ["yesterday"] => days(-1),
        ["day", "after", "tomorrow"] => days(2),
        ["day", "before", "yesterday"] => days(-2),
        [dir @ ("next" | "last"), unit] => {
            let sign = if *dir == "next" { 1 } else { -1 };
            match *unit {
                "week" => days(7 * sign),
                "month" => months(sign as i32),
                "year" => months(12 * sign as i32),
                w => {
                    let target = weekday(w)?;
                    let mut d = days(sign)?;
                    while d.weekday() != target {
                        d = d.checked_add_signed(chrono::Duration::try_days(sign)?)?;
                    }
                    Some(d)
                }
            }
        }
        ["in", n, unit] => offset(n, unit, 1),
        [n, unit, "ago"] => offset(n, unit, -1),
        [n, unit, "from", "now"] => offset(n, unit, 1),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatePhrase {
    pub range: std::ops::Range<usize>,
    pub date: NaiveDate,
}

const MAX_DATE_PHRASE_WORDS: usize = 4;

fn month_index(token: &str) -> Option<u32> {
    let token = token.to_lowercase();
    if token.chars().count() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|name| name.to_lowercase().starts_with(&token))
        .map(|i| i as u32 + 1)
}

fn weekday_named(token: &str) -> Option<Weekday> {
    let token = token.to_lowercase();
    WEEKDAYS
        .iter()
        .position(|name| name.to_lowercase() == token)
        .and_then(|i| Weekday::try_from(i as u8).ok())
}

fn day_number(token: &str) -> Option<u32> {
    let token = token.to_lowercase();
    let (digits, suffix) = ["st", "nd", "rd", "th"]
        .iter()
        .find_map(|suffix| {
            token
                .strip_suffix(suffix)
                .map(|digits| (digits, Some(*suffix)))
        })
        .unwrap_or((token.as_str(), None));
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let day = digits
        .parse::<u32>()
        .ok()
        .filter(|d| (1..=31).contains(d))?;
    if suffix.is_some_and(|suffix| suffix != ordinal_suffix(day)) {
        return None;
    }
    Some(day)
}

fn year_number(token: &str) -> Option<i32> {
    if token.len() == 4 && token.bytes().all(|b| b.is_ascii_digit()) {
        token.parse().ok().filter(|y| (1000..=9999).contains(y))
    } else {
        None
    }
}

fn month_then_day(month: &str, day: &str, year: i32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month_index(month)?, day_number(day)?)
}

fn absolute_tokens(phrase: &str) -> Option<Vec<&str>> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < phrase.len() {
        let start = i;
        while let Some(c) = phrase[i..].chars().next() {
            if !c.is_alphanumeric() {
                break;
            }
            i += c.len_utf8();
        }
        if i == start {
            return None;
        }
        tokens.push(&phrase[start..i]);
        if i == phrase.len() {
            break;
        }
        let gap = i;
        while let Some(c) = phrase[i..].chars().next() {
            if !c.is_whitespace() {
                break;
            }
            i += c.len_utf8();
        }
        if i < phrase.len() && phrase[i..].starts_with(',') {
            if tokens.len() != 2 {
                return None;
            }
            i += 1;
            let after_comma = i;
            while let Some(c) = phrase[i..].chars().next() {
                if !c.is_whitespace() {
                    break;
                }
                i += c.len_utf8();
            }
            if i == after_comma {
                return None;
            }
        }
        if i == gap || i >= phrase.len() {
            return None;
        }
    }
    Some(tokens)
}

fn absolute_date(phrase: &str, today: NaiveDate) -> Option<NaiveDate> {
    if let Some(d) = parse_iso(phrase) {
        return Some(d);
    }
    let tokens = absolute_tokens(phrase)?;
    let year = today.year();
    match tokens.as_slice() {
        [a, b] => month_then_day(a, b, year).or_else(|| month_then_day(b, a, year)),
        [a, b, c] => {
            year_number(c).and_then(|y| month_then_day(a, b, y).or_else(|| month_then_day(b, a, y)))
        }
        _ => None,
    }
}

pub fn date_phrase(phrase: &str, today: NaiveDate) -> Option<NaiveDate> {
    let phrase = phrase.trim();
    if phrase.is_empty() {
        return None;
    }
    if let Some(d) = relative_date(phrase, today) {
        return Some(d);
    }
    if let Some(target) = weekday_named(phrase) {
        let ahead = (i64::from(target.num_days_from_monday())
            - i64::from(today.weekday().num_days_from_monday()))
        .rem_euclid(7);
        return today.checked_add_signed(chrono::Duration::days(ahead));
    }
    absolute_date(phrase, today)
}

pub fn find_date_phrases(text: &str, today: NaiveDate) -> Vec<DatePhrase> {
    let words = crate::text::word_ranges(text);
    let code = crate::inline::code_ranges(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut found = None;
        for take in (1..=MAX_DATE_PHRASE_WORDS.min(words.len() - i)).rev() {
            let range = words[i].start..words[i + take - 1].end;
            if code
                .iter()
                .any(|c| c.start < range.end && range.start < c.end)
            {
                continue;
            }
            if let Some(date) = date_phrase(&text[range.clone()], today) {
                found = Some((range, date, take));
                break;
            }
        }
        match found {
            Some((range, date, take)) => {
                out.push(DatePhrase { range, date });
                i += take;
            }
            None => i += 1,
        }
    }
    out
}

pub fn expand_relative_date_links(
    text: &str,
    caret: usize,
    today: NaiveDate,
    format: &DateFormat,
    exists: impl Fn(&str) -> bool,
) -> Option<(String, usize)> {
    expand_links(text, caret, true, today, format, exists)
}

pub fn finish_relative_date_links(
    text: &str,
    caret: usize,
    today: NaiveDate,
    format: &DateFormat,
    exists: impl Fn(&str) -> bool,
) -> Option<(String, usize)> {
    expand_links(text, caret, false, today, format, exists)
}

fn expand_links(
    text: &str,
    caret: usize,
    skip_inside: bool,
    today: NaiveDate,
    format: &DateFormat,
    exists: impl Fn(&str) -> bool,
) -> Option<(String, usize)> {
    let mut out = String::with_capacity(text.len());
    let mut new_caret = caret;
    let mut last = 0;
    for r in crate::inline::extract_refs(text) {
        let Some(span) = text.get(r.range.clone()) else {
            continue;
        };
        let prefix = if span.starts_with("#[[") {
            "#[["
        } else if span.starts_with("[[") {
            "[["
        } else {
            continue;
        };
        if !span.ends_with("]]") || r.target.contains("[[") {
            continue;
        }
        let inside = r.range.start < caret && caret < r.range.end;
        if (inside && skip_inside) || r.range.start < last {
            continue;
        }
        let Some(d) = relative_date(&r.target, today) else {
            continue;
        };
        if exists(&r.target) {
            continue;
        }
        let link = format!("{prefix}{}]]", format.format(d));
        if r.range.end <= caret {
            new_caret = (new_caret + link.len()).saturating_sub(span.len());
        }
        out.push_str(&text[last..r.range.start]);
        out.push_str(&link);
        if inside {
            new_caret = out.len();
        }
        last = r.range.end;
    }
    if last == 0 {
        return None;
    }
    out.push_str(&text[last..]);
    Some((out, new_caret))
}

pub fn parse_iso(s: &str) -> Option<NaiveDate> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        if b[r.clone()].iter().all(u8::is_ascii_digit) {
            s[r].parse().ok()
        } else {
            None
        }
    };
    NaiveDate::from_ymd_opt(digits(0..4)? as i32, digits(5..7)?, digits(8..10)?)
}

pub fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

pub fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

fn tokenize(pattern: &str) -> Vec<Piece> {
    const TABLE: &[(&str, Tok)] = &[
        ("yyyy", Tok::Year),
        ("MMMM", Tok::MonthName),
        ("MMM", Tok::MonthAbbr),
        ("MM", Tok::Month2),
        ("M", Tok::Month),
        ("do", Tok::DayOrdinal),
        ("dd", Tok::Day2),
        ("d", Tok::Day),
        ("EEEE", Tok::Weekday),
        ("EEE", Tok::WeekdayAbbr),
    ];
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut rest = pattern;
    'outer: while !rest.is_empty() {
        for (pat, tok) in TABLE {
            if rest.starts_with(pat) {
                if !lit.is_empty() {
                    out.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                out.push(Piece::Tok(*tok));
                rest = &rest[pat.len()..];
                continue 'outer;
            }
        }
        let ch = rest.chars().next().expect("non-empty");
        lit.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    if !lit.is_empty() {
        out.push(Piece::Lit(lit));
    }
    out
}

fn render(pieces: &[Piece], d: NaiveDate) -> String {
    let mut s = String::new();
    for p in pieces {
        match p {
            Piece::Lit(l) => s.push_str(l),
            Piece::Tok(t) => match t {
                Tok::Year => s.push_str(&format!("{:04}", d.year())),
                Tok::MonthName => s.push_str(MONTHS[d.month0() as usize]),
                Tok::MonthAbbr => s.push_str(&MONTHS[d.month0() as usize][..3]),
                Tok::Month2 => s.push_str(&format!("{:02}", d.month())),
                Tok::Month => s.push_str(&d.month().to_string()),
                Tok::DayOrdinal => s.push_str(&format!("{}{}", d.day(), ordinal_suffix(d.day()))),
                Tok::Day2 => s.push_str(&format!("{:02}", d.day())),
                Tok::Day => s.push_str(&d.day().to_string()),
                Tok::Weekday => s.push_str(weekday_name(d.weekday())),
                Tok::WeekdayAbbr => s.push_str(&weekday_name(d.weekday())[..3]),
            },
        }
    }
    s
}

fn weekday_name(w: Weekday) -> &'static str {
    WEEKDAYS[w.num_days_from_monday() as usize]
}

pub fn weekday_name_of(date: NaiveDate) -> &'static str {
    weekday_name(date.weekday())
}

fn take_digits(s: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    if n < min {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn take_name<'a>(s: &'a str, names: &[&str], abbr: bool) -> Option<(usize, &'a str)> {
    for (i, name) in names.iter().enumerate() {
        let cand = if abbr { &name[..3] } else { name };
        if s.len() >= cand.len()
            && s.is_char_boundary(cand.len())
            && s[..cand.len()].eq_ignore_ascii_case(cand)
        {
            return Some((i, &s[cand.len()..]));
        }
    }
    None
}

fn parse_with(pieces: &[Piece], input: &str) -> Option<NaiveDate> {
    let (mut y, mut m, mut d) = (None, None, None);
    let mut rest = input.trim();
    let mut has_weekday = false;
    for p in pieces {
        match p {
            Piece::Lit(l) => {
                let l_trim = l.trim();
                if l.chars().any(char::is_whitespace) {
                    let before = rest;
                    rest = rest.trim_start();
                    if !l_trim.is_empty() {
                        rest = rest.strip_prefix(l_trim)?;
                        rest = rest.trim_start();
                    } else if before.len() == rest.len() {
                        return None;
                    }
                } else {
                    rest = rest.strip_prefix(l.as_str())?;
                }
            }
            Piece::Tok(t) => match t {
                Tok::Year => {
                    let (v, r) = take_digits(rest, 4, 4)?;
                    y = Some(v as i32);
                    rest = r;
                }
                Tok::Month2 => {
                    let (v, r) = take_digits(rest, 2, 2)?;
                    m = Some(v);
                    rest = r;
                }
                Tok::Month => {
                    let (v, r) = take_digits(rest, 1, 2)?;
                    m = Some(v);
                    rest = r;
                }
                Tok::MonthName => {
                    let (i, r) = take_name(rest, &MONTHS, false)?;
                    m = Some(i as u32 + 1);
                    rest = r;
                }
                Tok::MonthAbbr => {
                    let (i, r) = take_name(rest, &MONTHS, true)?;
                    m = Some(i as u32 + 1);
                    rest = r;
                }
                Tok::Day2 => {
                    let (v, r) = take_digits(rest, 2, 2)?;
                    d = Some(v);
                    rest = r;
                }
                Tok::Day => {
                    let (v, r) = take_digits(rest, 1, 2)?;
                    d = Some(v);
                    rest = r;
                }
                Tok::DayOrdinal => {
                    let (v, r) = take_digits(rest, 1, 2)?;
                    let suffix = r.get(..2)?;
                    if !["st", "nd", "rd", "th"]
                        .iter()
                        .any(|s| suffix.eq_ignore_ascii_case(s))
                    {
                        return None;
                    }
                    d = Some(v);
                    rest = &r[2..];
                }
                Tok::Weekday => {
                    let (_, r) = take_name(rest, &WEEKDAYS, false)?;
                    has_weekday = true;
                    rest = r;
                }
                Tok::WeekdayAbbr => {
                    let (_, r) = take_name(rest, &WEEKDAYS, true)?;
                    has_weekday = true;
                    rest = r;
                }
            },
        }
    }
    if !rest.trim().is_empty() {
        return None;
    }
    let _ = has_weekday;
    NaiveDate::from_ymd_opt(y?, m?, d?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn ac_4_12_out_of_range_offsets_are_no_date() {
        let t = d(2026, 11, 3);
        for phrase in [
            "-99999999999 days ago",
            "in 99999999999999 weeks",
            "in -3 days",
        ] {
            assert_eq!(relative_date(phrase, t), None, "{phrase:?}");
            assert_eq!(date_phrase(phrase, t), None, "{phrase:?}");
            assert!(find_date_phrases(phrase, t).is_empty(), "{phrase:?}");
        }
    }

    #[test]
    fn relative_phrases() {
        let t = d(2026, 11, 3);
        let r = |p: &str| relative_date(p, t);
        assert_eq!(r("Today"), Some(t));
        assert_eq!(r("yesterday"), Some(d(2026, 11, 2)));
        assert_eq!(r("  Tomorrow "), Some(d(2026, 11, 4)));
        assert_eq!(r("day after tomorrow"), Some(d(2026, 11, 5)));
        assert_eq!(r("day before yesterday"), Some(d(2026, 11, 1)));
        assert_eq!(r("next week"), Some(d(2026, 11, 10)));
        assert_eq!(r("last week"), Some(d(2026, 10, 27)));
        assert_eq!(r("next month"), Some(d(2026, 12, 3)));
        assert_eq!(r("last year"), Some(d(2025, 11, 3)));
        assert_eq!(r("next Sunday"), Some(d(2026, 11, 8)));
        assert_eq!(r("next tuesday"), Some(d(2026, 11, 10)));
        assert_eq!(r("next wed"), Some(d(2026, 11, 4)));
        assert_eq!(r("last friday"), Some(d(2026, 10, 30)));
        assert_eq!(r("last tuesday"), Some(d(2026, 10, 27)));
        assert_eq!(r("in 3 days"), Some(d(2026, 11, 6)));
        assert_eq!(r("in a week"), Some(d(2026, 11, 10)));
        assert_eq!(r("2 weeks ago"), Some(d(2026, 10, 20)));
        assert_eq!(r("1 day from now"), Some(d(2026, 11, 4)));
        assert_eq!(
            relative_date("next month", d(2026, 1, 31)),
            Some(d(2026, 2, 28))
        );
        for no in [
            "Monday",
            "next",
            "next thing",
            "in 3 parsecs",
            "tomorrow's plan",
            "today and tomorrow",
            "",
            "in 99999999 days",
        ] {
            assert_eq!(r(no), None, "{no:?}");
        }
    }

    #[test]
    fn expands_relative_links_outside_the_caret() {
        let t = d(2026, 11, 3);
        let f = DateFormat::default();
        let none = |_: &str| false;
        let text = "call [[Yesterday]] and #[[next sunday]]";
        let (out, caret) = expand_relative_date_links(text, text.len(), t, &f, none).unwrap();
        assert_eq!(
            out,
            "call [[November 2nd, 2026]] and #[[November 8th, 2026]]"
        );
        assert_eq!(caret, out.len());
        let text = "see [[tomorrow]]";
        assert_eq!(expand_relative_date_links(text, 10, t, &f, none), None);
        let (out, caret) = expand_relative_date_links(text, 2, t, &f, none).unwrap();
        assert_eq!((out.as_str(), caret), ("see [[November 4th, 2026]]", 2));
        assert_eq!(
            expand_relative_date_links("[[Today]]", 9, t, &f, |p| p == "Today"),
            None
        );
        for text in [
            "tomorrow",
            "`[[tomorrow]]`",
            "[[Project]] #tomorrow",
            "[[a [[b]]]]",
        ] {
            assert_eq!(
                expand_relative_date_links(text, text.len(), t, &f, none),
                None,
                "{text:?}"
            );
        }
    }

    #[test]
    fn ac_4_12_the_phrases_a_phone_offers() {
        let t = d(2026, 11, 3);
        let yes = [
            ("today", t),
            ("Tomorrow", d(2026, 11, 4)),
            ("yesterday", d(2026, 11, 2)),
            ("friday", d(2026, 11, 6)),
            ("Tuesday", t),
            ("next friday", d(2026, 11, 6)),
            ("last sunday", d(2026, 11, 1)),
            ("in 3 days", d(2026, 11, 6)),
            ("2 weeks ago", d(2026, 10, 20)),
            ("Nov 26", d(2026, 11, 26)),
            ("26 November", d(2026, 11, 26)),
            ("November 26th", d(2026, 11, 26)),
            ("nov 26th", d(2026, 11, 26)),
            ("Nov 26, 2027", d(2027, 11, 26)),
            ("26 sep 2027", d(2027, 9, 26)),
            ("2026-11-26", d(2026, 11, 26)),
            ("Nov 21st", d(2026, 11, 21)),
            ("Nov 22nd", d(2026, 11, 22)),
            ("Nov 23rd", d(2026, 11, 23)),
            ("Nov 11th", d(2026, 11, 11)),
            ("Nov 12th", d(2026, 11, 12)),
            ("Nov 13th", d(2026, 11, 13)),
        ];
        for (phrase, day) in yes {
            assert_eq!(date_phrase(phrase, t), Some(day), "{phrase:?}");
        }
        for no in [
            "26",
            "may",
            "march",
            "sun",
            "sat",
            "monday 26",
            "11/6/26",
            "Nov",
            "2026",
            "in 3 parsecs",
            "Nov 32",
            "26 November 26",
            "May. 5",
            "may_5",
            "Nov, 26",
            "Nov. 26",
            "26nd",
            "1th",
            "11st",
            "22th",
            "26th of November",
        ] {
            assert_eq!(date_phrase(no, t), None, "{no:?}");
        }
    }

    #[test]
    fn ac_4_12_phrases_are_found_whole_in_a_line() {
        let t = d(2026, 11, 3);
        let found = find_date_phrases("call Ana next friday, then 2 weeks ago", t);
        let seen: Vec<(String, String)> = found
            .iter()
            .map(|p| {
                (
                    "call Ana next friday, then 2 weeks ago"[p.range.clone()].to_string(),
                    p.date.to_string(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("next friday".to_string(), "2026-11-06".to_string()),
                ("2 weeks ago".to_string(), "2026-10-20".to_string()),
            ]
        );
        assert!(find_date_phrases("the 26th of it", t).is_empty());
        assert!(find_date_phrases("26", t).is_empty());
        assert!(find_date_phrases("a sunny day", t).is_empty());
        assert!(find_date_phrases("in May. 5 of us", t).is_empty());
        assert!(find_date_phrases("may_5", t).is_empty());
        let one = find_date_phrases("Ship November 26th to Ana", t);
        assert_eq!(one.len(), 1);
        assert_eq!(
            &"Ship November 26th to Ana"[one[0].range.clone()],
            "November 26th"
        );
        let year = find_date_phrases("due 26 November 2027, no later", t);
        assert_eq!(year.len(), 1);
        assert_eq!(
            &"due 26 November 2027, no later"[year[0].range.clone()],
            "26 November 2027"
        );
    }

    #[test]
    fn finishing_expands_the_link_at_the_caret() {
        let t = d(2026, 11, 3);
        let f = DateFormat::default();
        let none = |_: &str| false;
        let text = "see [[tomorrow]] then [[yesterday]]";
        let (out, caret) = finish_relative_date_links(text, 14, t, &f, none).unwrap();
        assert_eq!(
            out,
            "see [[November 4th, 2026]] then [[November 2nd, 2026]]"
        );
        assert_eq!(&out[..caret], "see [[November 4th, 2026]]");
        let (out, caret) = finish_relative_date_links("[[today]] x", 11, t, &f, none).unwrap();
        assert_eq!((out.as_str(), caret), ("[[November 3rd, 2026]] x", 24));
        assert_eq!(
            finish_relative_date_links("[[Today]]", 4, t, &f, |p| p == "Today"),
            None,
            "a real page still wins"
        );
    }

    #[test]
    fn default_format_renders_roam_style() {
        let f = DateFormat::default();
        assert_eq!(f.format(d(2026, 11, 2)), "November 2nd, 2026");
        assert_eq!(f.format(d(2026, 11, 11)), "November 11th, 2026");
        assert_eq!(f.format(d(2026, 12, 1)), "December 1st, 2026");
        assert_eq!(f.format(d(2026, 12, 23)), "December 23rd, 2026");
        assert_eq!(
            DateFormat::new("yyyy-MM-dd").format(d(2026, 1, 6)),
            "2026-01-06"
        );
    }

    #[test]
    fn weekday_names_follow_the_calendar() {
        assert_eq!(weekday_name_of(d(2026, 11, 2)), "Monday");
        assert_eq!(weekday_name_of(d(2026, 11, 5)), "Thursday");
        assert_eq!(weekday_name_of(d(2026, 11, 8)), "Sunday");
    }

    #[test]
    fn ac_1_7_three_forms_resolve_identically() {
        let f = DateFormat::new("dd.MMM.yyyy");
        let want = Some(d(2026, 12, 1));
        assert_eq!(parse_unambiguous_date("2026-12-01", &f), want);
        assert_eq!(parse_unambiguous_date("December 1st, 2026", &f), want);
        assert_eq!(parse_unambiguous_date("01.Dec.2026", &f), want);
        assert_eq!(parse_unambiguous_date("december 1, 2026", &f), want);
    }

    #[test]
    fn ambiguous_numeric_dates_never_resolve() {
        let us = DateFormat::new("MM/dd/yy");
        assert!(!us.is_unambiguous());
        assert_eq!(parse_unambiguous_date("11/6/26", &us), None);
        let us4 = DateFormat::new("MM/dd/yyyy");
        assert!(!us4.is_unambiguous());
        assert_eq!(parse_unambiguous_date("11/06/2026", &us4), None);
        assert!(DateFormat::new("yyyy/MM/dd").is_unambiguous());
        assert_eq!(
            parse_unambiguous_date("2026/11/06", &DateFormat::new("yyyy/MM/dd")),
            Some(d(2026, 11, 6))
        );
    }

    #[test]
    fn previous_formats_keep_parsing() {
        let f = DateFormat::new("yyyy/MM/dd").changed_to("yyyy-MM-dd");
        assert_eq!(f.pattern(), "yyyy-MM-dd");
        assert_eq!(
            parse_unambiguous_date("2026/12/01", &f),
            Some(d(2026, 12, 1))
        );
        let back = f.changed_to("yyyy/MM/dd");
        assert_eq!(back.previous(), &["yyyy-MM-dd".to_string()]);
    }

    #[test]
    fn yearless_and_invalid_are_not_dates() {
        let f = DateFormat::default();
        assert_eq!(
            parse_unambiguous_date("Nov 2", &DateFormat::new("MMM d")),
            None
        );
        assert_eq!(parse_unambiguous_date("February 30th, 2026", &f), None);
        assert_eq!(parse_unambiguous_date("2026-13-01", &f), None);
        assert_eq!(parse_unambiguous_date("Modern Religion", &f), None);
        assert_eq!(
            parse_unambiguous_date("2026", &DateFormat::new("yyyy")),
            None
        );
    }

    #[test]
    fn round_trip_formats() {
        for pat in [
            "MMMM do, yyyy",
            "yyyy-MM-dd",
            "EEEE, MMMM do, yyyy",
            "d MMM yyyy",
        ] {
            let f = DateFormat::new(pat);
            let date = d(2027, 1, 6);
            assert_eq!(f.parse(&f.format(date)), Some(date), "{pat}");
        }
    }
}
