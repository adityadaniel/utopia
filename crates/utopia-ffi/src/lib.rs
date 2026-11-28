//! The mobile core (PRD §14, Option A): the same store, CRDT and sync engine as the
//! desktop, behind a small UniFFI surface. The surface covers the minimal mobile scope
//! (§14.3): today's page, bullet capture, references (with title search for `[[`),
//! backlinks, full offline editing and sync. Nothing here knows about any UI toolkit.
//!
//! Swift bindings: `cargo run -p utopia-ffi --bin uniffi-bindgen -- generate
//! --library <libutopia_ffi> --language swift --out-dir <dir>` (see `xtask mobile`).

use std::sync::{Arc, Mutex, MutexGuard};

use chrono::NaiveDate;
use utopia_core::{outline, BlockId, PageId};
use utopia_store::{RefQuery, Store, StoreError};
use utopia_sync::{StoreDone, StoreStep, SyncEngine, SyncError, SyncReport, SyncStatus, Transport};

uniffi::setup_scaffolding!();

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    #[error("this graph is encrypted; the passphrase is missing or wrong")]
    Locked,
    #[error("not found: {msg}")]
    NotFound { msg: String },
    #[error("{msg}")]
    Invalid { msg: String },
    #[error("sync: {msg}")]
    Sync { msg: String },
    #[error("{msg}")]
    Internal { msg: String },
}

impl From<StoreError> for FfiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Encrypted => FfiError::Locked,
            StoreError::NotFound(msg) => FfiError::NotFound { msg },
            StoreError::Invalid(msg) => FfiError::Invalid { msg },
            StoreError::Domain(d) => FfiError::Invalid { msg: d.to_string() },
            other => FfiError::Internal {
                msg: other.to_string(),
            },
        }
    }
}

impl From<SyncError> for FfiError {
    fn from(e: SyncError) -> Self {
        // A store fault keeps its own answer; everything else can only happen while the
        // transport is being built (a round reports its failures as a `SyncOutcome`).
        match e {
            SyncError::Store(e) => FfiError::from(e),
            other => FfiError::Sync {
                msg: other.to_string(),
            },
        }
    }
}

type Result<T, E = FfiError> = std::result::Result<T, E>;

/// One visible block of a page, in outline order.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BlockView {
    pub id: String,
    pub text: String,
    /// Plain text with markup removed (for list rows).
    pub plain: String,
    pub depth: u32,
    /// `bullet`, `numbered`, `todo`, `quote`, `code` or `heading`.
    pub kind: String,
    /// The level of a heading; 0 for every other kind.
    pub heading_level: u32,
    /// Whether a to-do is done (ios-prd AC-5.1).
    pub checked: bool,
    /// When the block was made, Unix ms UTC: the day log's time column (§4.4).
    pub created_at_ms: i64,
    /// The text split into what to draw, already resolved by the core (C1). Concatenating
    /// the part texts gives `plain`.
    pub parts: Vec<TextPart>,
    /// A block the phone cannot draw in text, for example a query (§4.4, AC-7.5).
    pub placeholder: Option<String>,
    /// The key of a leading `key:: value` block: it is drawn as a row above the outline, not
    /// as a block (§4.4, AC-7.3).
    pub property_key: Option<String>,
}

/// What one piece of a block's text is (ios-prd §4.4). The core decides it: the app never
/// parses a block itself (C1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PartKind {
    Plain,
    /// A `[[page]]` or `[[tag]]` link.
    Page,
    /// A `#tag`.
    Tag,
    /// A link that names a day.
    Date,
    /// A `((block reference))`, shown as the text of the block it names.
    Block,
    /// An inline link or attachment name: `[name](url)`.
    Link,
    /// An inline code span.
    Code,
}

/// One drawn piece of a block's text. `start`/`end` are UTF-16 offsets into the text the
/// parts join into, which is what a `Text` view and VoiceOver measure in.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TextPart {
    pub kind: PartKind,
    pub text: String,
    /// The page this piece opens, when it opens one.
    pub page_id: Option<String>,
    pub start: u32,
    pub end: u32,
}

/// A title the core found in a draft line, with the range it covers (ios-prd §4.6): the
/// whole-word, any-case match of a page title or alias, two characters or longer.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TitleMatch {
    /// The page's own spelling of its title, which is what a link stores.
    pub title: String,
    pub page_id: String,
    pub start: u32,
    pub end: u32,
}

/// A link a reader accepted in a draft line: the title, and the UTF-16 range of the words
/// that carry it. The core checks the words again before it writes the link, so a range
/// the reader has since changed is written as plain text (AC-4.11).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AcceptedLink {
    pub title: String,
    pub start: u32,
    pub end: u32,
    pub day: Option<String>,
    pub words: Option<String>,
    pub typed: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AbsorbedDraft {
    pub text: String,
    pub links: Vec<AcceptedLink>,
    pub caret: Option<u32>,
}

fn rebase_links_in(
    text_before: &str,
    edit_start: u32,
    edit_removed_len: u32,
    replacement: &str,
    links: Vec<AcceptedLink>,
) -> Vec<AcceptedLink> {
    let edit_end = edit_start.saturating_add(edit_removed_len);
    let Some(range) = utf16_range(text_before, edit_start, edit_end) else {
        return Vec::new();
    };
    let new_text = format!(
        "{}{}{}",
        &text_before[..range.start],
        replacement,
        &text_before[range.end..]
    );
    let replacement_len = utf16_len(replacement);
    let delta = replacement_len as i64 - edit_removed_len as i64;
    let mut out = Vec::with_capacity(links.len());
    for link in links {
        if link.end <= edit_start {
            out.push(link);
            continue;
        }
        if link.start >= edit_end {
            let start = (link.start as i64 + delta).max(0) as u32;
            let end = (link.end as i64 + delta).max(0) as u32;
            if start < end {
                out.push(AcceptedLink { start, end, ..link });
            }
            continue;
        }
        if link.typed || (link.day.is_none() && link.words.is_some()) {
            let inside = if edit_removed_len == 0 {
                link.start < edit_start && edit_start < link.end
            } else {
                link.start <= edit_start
                    && edit_end <= link.end
                    && !(edit_start == link.start && edit_end == link.end)
            };
            if !inside {
                continue;
            }
            let end = (link.end as i64 + delta).max(0) as u32;
            if link.start >= end {
                continue;
            }
            let Some(words_range) = utf16_range(&new_text, link.start, end) else {
                continue;
            };
            let words = &new_text[words_range];
            if words.trim().is_empty() {
                continue;
            }
            out.push(AcceptedLink {
                title: words.to_string(),
                start: link.start,
                end,
                day: None,
                words: Some(words.to_string()),
                typed: true,
            });
            continue;
        }
        let Some(stored) = link.words.as_deref().filter(|_| link.day.is_some()) else {
            continue;
        };
        let inside = if edit_removed_len == 0 {
            link.start < edit_start && edit_start < link.end
        } else {
            link.start <= edit_start && edit_end <= link.end
        };
        if !inside {
            continue;
        }
        let end = (link.end as i64 + delta).max(0) as u32;
        if link.start >= end {
            continue;
        }
        let Some(words_range) = utf16_range(&new_text, link.start, end) else {
            continue;
        };
        let words = &new_text[words_range];
        if words.trim().is_empty() || words.to_lowercase() != stored.to_lowercase() {
            continue;
        }
        out.push(AcceptedLink { end, ..link });
    }
    out
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TextEdit {
    pub start: u32,
    pub removed: u32,
    pub replacement: String,
}

fn edit_between_in(old: &str, new: &str, links: &[AcceptedLink]) -> TextEdit {
    let before: Vec<char> = old.chars().collect();
    let after: Vec<char> = new.chars().collect();
    let mut prefix = 0;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < before.len() - prefix
        && suffix < after.len() - prefix
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed_run = &before[prefix..before.len() - suffix];
    let inserted_run = &after[prefix..after.len() - suffix];
    let insertion = removed_run.is_empty() && !inserted_run.is_empty();
    let deletion = !removed_run.is_empty() && inserted_run.is_empty();
    let (position, replacement) = if insertion || deletion {
        let slide_run = if insertion { inserted_run } else { removed_run };
        let (position, run) = equivalent_edit(&before, prefix, slide_run, insertion, links);
        (position, if insertion { run } else { Vec::new() })
    } else {
        (prefix, inserted_run.to_vec())
    };
    let start = before[..position]
        .iter()
        .map(|c| c.len_utf16() as u32)
        .sum();
    let removed = removed_run.iter().map(|c| c.len_utf16() as u32).sum();
    let replacement = replacement.iter().collect();
    TextEdit {
        start,
        removed,
        replacement,
    }
}

fn equivalent_edit(
    before: &[char],
    original: usize,
    original_run: &[char],
    insertion: bool,
    links: &[AcceptedLink],
) -> (usize, Vec<char>) {
    let mut candidates: Vec<(i64, usize, Vec<char>)> = vec![(0, original, original_run.to_vec())];
    let mut position = original;
    let mut run = original_run.to_vec();
    while position > 0 && before[position - 1] == *run.last().expect("a nonempty run") {
        let last = run.pop().expect("a nonempty run");
        run.insert(0, last);
        position -= 1;
        candidates.push((-((original - position) as i64), position, run.clone()));
    }
    position = original;
    run = original_run.to_vec();
    loop {
        let next = if insertion {
            position
        } else {
            position + run.len()
        };
        if next >= before.len() || before[next] != run[0] {
            break;
        }
        let first = run.remove(0);
        run.push(first);
        position += 1;
        candidates.push(((position - original) as i64, position, run.clone()));
    }
    let offsets: Vec<u32> = (0..=before.len())
        .scan(0u32, |units, index| {
            let here = *units;
            if index < before.len() {
                *units += before[index].len_utf16() as u32;
            }
            Some(here)
        })
        .collect();
    candidates.sort_by_key(|(step, _, _)| (step.unsigned_abs(), *step > 0));
    for (_, position, run) in candidates {
        let offset = offsets[position];
        if !links
            .iter()
            .any(|link| link.start < offset && offset < link.end)
        {
            return (position, run);
        }
    }
    (original, original_run.to_vec())
}

#[uniffi::export]
pub fn edit_between(old: String, new: String, links: Vec<AcceptedLink>) -> TextEdit {
    edit_between_in(&old, &new, &links)
}

#[uniffi::export]
pub fn rebase_links(
    text_before: String,
    edit_start: u32,
    edit_removed_len: u32,
    replacement: String,
    links: Vec<AcceptedLink>,
) -> Vec<AcceptedLink> {
    rebase_links_in(
        &text_before,
        edit_start,
        edit_removed_len,
        &replacement,
        links,
    )
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DateMatch {
    pub title: String,
    pub day: String,
    pub page_id: String,
    pub words: String,
    pub start: u32,
    pub end: u32,
}

/// One line of a draft outline (ios-prd §4.2): its text, its depth and whether it is a
/// to-do. `depth` 0 is a top-level block.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct OutlineLine {
    pub text: String,
    pub depth: u32,
    pub todo: bool,
}

/// A `key:: value` block of a page, shown as a row above the outline (AC-7.3).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PropertyView {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PageView {
    pub id: String,
    pub title: String,
    /// `YYYY-MM-DD` for journal pages.
    pub date: Option<String>,
    pub blocks: Vec<BlockView>,
    /// The page's leading `key:: value` blocks, in outline order (AC-7.3).
    pub properties: Vec<PropertyView>,
    /// Whether anything is on the page. A day that was only visited is empty and has no
    /// page in the graph (FR-1.2).
    pub exists: bool,
}

/// A page by name, without opening it: a search result or a recent page (AC-8.1, AC-8.2).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PageRef {
    pub id: String,
    pub title: String,
    /// `YYYY-MM-DD` for journal pages.
    pub date: Option<String>,
}

/// One block found by search, with the page it is on (AC-8.1).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchHit {
    pub page_id: String,
    pub page_title: String,
    pub block_id: String,
    pub text: String,
}

/// What [`Graph::search`] found: pages first, then blocks (AC-8.1).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SearchResults {
    pub pages: Vec<PageRef>,
    pub blocks: Vec<SearchHit>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RefView {
    pub page_id: String,
    pub page_title: String,
    pub block_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct SyncSummary {
    pub pushed: u32,
    pub pulled_pages: u32,
    pub imported_pages: u32,
    /// Pages that failed while the rest of the round applied (§12.6). The shell shows
    /// the counts as a successful sync and this as a warning, instead of one error.
    pub failed: u32,
}

/// What one [`Graph::sync`] call did, in the terms §12.6 asks the shell to show.
///
/// One variant per answer the shell has to render, so a wrong token, a relay speaking
/// another protocol and an unreachable relay cannot collapse into one "Offline"
/// (`docs/state-machines/ffi-mobile-sync.md`, issue 3). `SyncSummary` is unchanged: it is
/// still the counts, and the outcome says which counts mean what.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum SyncOutcome {
    /// The relay was reached and every page went through.
    Synced { summary: SyncSummary },
    /// The relay was reached but at least one page failed, or holds another device's
    /// changes that cannot apply yet; the rest of the round applied (§12.6). `queued`
    /// counts this device's unsent updates afterwards. `blocked` counts pages that wait
    /// for changes another device has not sent to this relay: they are kept, not lost,
    /// and apply when that device sends them (§2.3.7). Neither is "Synced".
    Partial {
        summary: SyncSummary,
        queued: u32,
        blocked: u32,
    },
    /// The relay was not reached: everything is still queued locally (§12.6).
    Offline { queued: u32 },
    /// The round failed on this device (the store, a shard, a projection), so the network
    /// is not blamed for it.
    LocalError { message: String },
    /// 401/403: the token is wrong or revoked. Sync halts until it is replaced (§12.6).
    NeedsAuth,
    /// The relay speaks a protocol version this client does not; nothing was pushed
    /// (§12.6). `version` is the relay's; 0 means the client never learned it (it is only
    /// unknown when the mismatch did not come from the round, which cannot happen today).
    Unsupported { version: u32 },
    /// A round was already running on this graph, so this call did not start a second one
    /// (issue 2). The running round's own outcome is the one to show.
    Busy,
}

/// Where a graph's sync engine is, which is also whether a round is running: the one fact
/// the old surface kept only as "the store mutex is held".
///
/// ```text
/// Idle(engine) ──take──▶ Running ──put back──▶ Idle(engine)
/// ```
///
/// `Idle` is the only state a round starts from, and a round owns the engine for its whole
/// length (it is not needed on the wire), so a second round cannot start behind the first
/// and the queued compaction snapshots survive from one call to the next (issues 2 and 4).
enum SyncSlot {
    /// Home between rounds.
    Idle(SyncEngine),
    /// A round is in flight and holds the engine.
    Running,
}

/// An open graph. All calls are serialized on one store.
#[derive(uniffi::Object)]
pub struct Graph {
    store: Mutex<Store>,
    /// The sync machine: never "no engine", which is what made every call build a new one.
    sync: Mutex<SyncSlot>,
}

impl Graph {
    fn with_store(store: Store) -> Result<Self> {
        let engine = SyncEngine::new(&store)?;
        Ok(Self {
            store: Mutex::new(store),
            sync: Mutex::new(SyncSlot::Idle(engine)),
        })
    }

    fn guard(&self) -> std::result::Result<MutexGuard<'_, Store>, StoreError> {
        self.store
            .lock()
            .map_err(|_| StoreError::Invalid("graph lock poisoned".into()))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
        let mut s = self.guard()?;
        f(&mut s)
    }

    /// Take the engine for one round, or `None` when a round already holds it.
    fn take_engine(&self) -> Result<Option<SyncEngine>> {
        let mut slot = self.sync.lock().map_err(|_| FfiError::Internal {
            msg: "sync lock poisoned".into(),
        })?;
        Ok(match std::mem::replace(&mut *slot, SyncSlot::Running) {
            SyncSlot::Idle(engine) => Some(engine),
            // Another round is in flight and `Running` is already the state.
            SyncSlot::Running => None,
        })
    }

    /// Hand the engine back at the end of a round, whatever the round did.
    fn put_engine(&self, engine: SyncEngine) -> Result<()> {
        let mut slot = self.sync.lock().map_err(|_| FfiError::Internal {
            msg: "sync lock poisoned".into(),
        })?;
        // Invariant: only `take_engine` moves the engine out, and only this puts it back.
        debug_assert!(matches!(*slot, SyncSlot::Running));
        *slot = SyncSlot::Idle(engine);
        Ok(())
    }

    /// The round itself, over whatever transport is in force: the HTTP relay for the shell,
    /// an in-memory one for tests. The store lock is taken for `prepare` and `apply` only;
    /// the exchange runs outside it (issue 1).
    fn sync_via(&self, mut transport: Box<dyn Transport>) -> Result<SyncOutcome> {
        // One round at a time: a second call while this one is on the wire does nothing
        // rather than queueing a whole round behind it (issue 2).
        let Some(mut engine) = self.take_engine()? else {
            return Ok(SyncOutcome::Busy);
        };
        let result = utopia_sync::sync_round(&mut engine, transport.as_mut(), |engine, step| {
            let mut store = self.guard()?;
            match step {
                StoreStep::Prepare => engine.prepare(&mut store).map(StoreDone::Prepared),
                StoreStep::Apply(inc) => engine.apply(&mut store, *inc).map(StoreDone::Applied),
                StoreStep::Queued => store
                    .outbox_count()
                    .map(|n| StoreDone::Queued(n > 0))
                    .map_err(SyncError::from),
            }
        });
        // The engine goes home whatever the round did: the next call starts from it and
        // keeps the compaction snapshots this one queued (issue 4).
        let restored = self.put_engine(engine);
        let outcome = self.with(|store| Ok(outcome_of(&result, store)));
        restored?;
        outcome
    }
}

/// The page's display title and, for a journal page, the day it names.
fn page_label(store: &Store, page: &PageId) -> Option<(String, Option<String>)> {
    let row = store.page_row(page).ok().flatten()?;
    let fmt = store.date_format();
    match row.daily_date {
        Some(d) => Some((fmt.format(d), Some(d.to_string()))),
        None => Some((row.title, None)),
    }
}

/// UTF-16 length, the unit a `Text` view and VoiceOver measure a string in.
fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// The byte offset of UTF-16 offset `at`, when it lands on a character boundary.
fn utf16_to_byte(s: &str, at: u32) -> Option<usize> {
    let mut units = 0u32;
    for (i, c) in s.char_indices() {
        if units == at {
            return Some(i);
        }
        units += c.len_utf16() as u32;
        if units > at {
            return None;
        }
    }
    (units == at).then_some(s.len())
}

/// The byte range that UTF-16 range `start..end` covers.
fn utf16_range(s: &str, start: u32, end: u32) -> Option<std::ops::Range<usize>> {
    let a = utf16_to_byte(s, start)?;
    let b = utf16_to_byte(s, end)?;
    (a <= b).then_some(a..b)
}

/// Walk an inline tree and give each piece its display text and the stored resolution it
/// points at (rule 3), so the phone draws a link without parsing anything itself (C1).
fn walk_parts(
    store: &mut Store,
    nodes: &[utopia_core::inline::Inline],
    spans: &[utopia_store::BlockRefSpan],
    out: &mut Vec<TextPart>,
) {
    use utopia_core::inline::Inline;
    let push = |out: &mut Vec<TextPart>, kind, text: String, page: Option<String>| {
        let start = out.last().map_or(0, |p| p.end);
        let end = start + utf16_len(&text);
        out.push(TextPart {
            kind,
            text,
            page_id: page,
            start,
            end,
        });
    };
    for node in nodes {
        match node {
            Inline::Text(t) => {
                if !t.is_empty() {
                    push(out, PartKind::Plain, t.clone(), None);
                }
            }
            Inline::Emph { children, .. } => walk_parts(store, children, spans, out),
            Inline::Code { text, .. } => push(out, PartKind::Code, text.clone(), None),
            Inline::PageRef { title, range } => {
                let span = spans
                    .iter()
                    .find(|s| s.kind == utopia_core::inline::RefKind::Page && s.range == *range);
                let (text, kind, page) = match span.and_then(|s| s.page.clone()) {
                    Some(p) => match page_label(store, &p) {
                        Some((label, Some(_))) => {
                            (label, PartKind::Date, Some(p.as_str().to_string()))
                        }
                        Some((label, None)) => {
                            (label, PartKind::Page, Some(p.as_str().to_string()))
                        }
                        None => (title.clone(), PartKind::Page, None),
                    },
                    None => (title.clone(), PartKind::Page, None),
                };
                push(out, kind, text, page);
            }
            Inline::Tag {
                title,
                bracketed,
                range,
            } => {
                let span = spans
                    .iter()
                    .find(|s| s.kind == utopia_core::inline::RefKind::Tag && s.range == *range);
                let target = span.and_then(|s| s.page.clone());
                let label = target
                    .as_ref()
                    .and_then(|p| page_label(store, p))
                    .map(|(t, _)| t)
                    .unwrap_or_else(|| title.clone());
                let text = if *bracketed {
                    label
                } else {
                    format!("#{label}")
                };
                push(
                    out,
                    PartKind::Tag,
                    text,
                    target.map(|p| p.as_str().to_string()),
                );
            }
            Inline::BlockRef { uid, range } | Inline::Embed { uid, range } => {
                let span = spans
                    .iter()
                    .find(|s| s.kind == utopia_core::inline::RefKind::Block && s.range == *range);
                let target = span
                    .and_then(|s| s.block.clone())
                    .or_else(|| Some(BlockId(uid.clone())));
                let (text, page) = match target.and_then(|b| store.block_row(&b).ok().flatten()) {
                    Some(row) if row.deleted_at.is_none() => (
                        utopia_core::inline::plain_text(&row.text),
                        Some(row.page_id.as_str().to_string()),
                    ),
                    _ => ("Deleted block".to_string(), None),
                };
                push(out, PartKind::Block, text, page);
            }
            Inline::Link { text, .. } => push(out, PartKind::Link, text.clone(), None),
        }
    }
}

/// The drawn pieces of a block's text, as the plain text and the parts that join into it.
fn parts_of(store: &mut Store, block: Option<&BlockId>, text: &str) -> (String, Vec<TextPart>) {
    let spans = block
        .and_then(|b| store.block_ref_spans(b).ok())
        .unwrap_or_default();
    let mut parts = Vec::new();
    walk_parts(store, &utopia_core::inline::parse(text), &spans, &mut parts);
    // A block with no text still needs a row to focus; an empty plain string keeps that.
    for p in &mut parts {
        if p.kind == PartKind::Plain && p.text.is_empty() {
            p.text.clear();
        }
    }
    let plain: String = parts.iter().map(|p| p.text.as_str()).collect();
    (plain, parts)
}

fn kind_name(kind: utopia_core::BlockKind) -> (&'static str, u32) {
    match kind {
        utopia_core::BlockKind::Bullet => ("bullet", 0),
        utopia_core::BlockKind::Numbered => ("numbered", 0),
        utopia_core::BlockKind::Todo => ("todo", 0),
        utopia_core::BlockKind::Quote => ("quote", 0),
        utopia_core::BlockKind::Code => ("code", 0),
        utopia_core::BlockKind::Heading(level) => ("heading", level as u32),
    }
}

fn page_view(store: &mut Store, page: &PageId) -> Result<PageView> {
    let fmt = store.date_format().clone();
    let exists = store.page_exists(page)?;
    if !exists {
        return Ok(PageView {
            id: page.as_str().to_string(),
            title: String::new(),
            date: None,
            blocks: Vec::new(),
            properties: Vec::new(),
            exists: false,
        });
    }
    let st = store.page_state(page)?.clone();
    let times = store.block_times(page)?;
    // Leading `key:: value` blocks are shown once, as rows above the outline (AC-7.3,
    // DESIGN §3.8). The shape comes from the core, not from the app.
    let mut properties = Vec::new();
    let mut property_blocks: Vec<BlockId> = Vec::new();
    for id in st.live_roots() {
        let Some(b) = st.block(&id) else { break };
        match utopia_core::inline::attribute(&b.text) {
            Some((key, range)) => {
                properties.push(PropertyView {
                    key,
                    value: utopia_core::inline::plain_text(b.text[range].trim()),
                });
                property_blocks.push(id);
            }
            None => break,
        }
    }
    let mut blocks = Vec::with_capacity(times.len());
    for (id, depth) in st.live_preorder() {
        let Some(b) = st.block(&id) else { continue };
        let (plain, parts) = parts_of(store, Some(&id), &b.text);
        let (kind, heading_level) = kind_name(b.kind);
        let placeholder =
            utopia_core::query::query_block(&b.text).map(|_| "Query — open on Mac".to_string());
        let property_key = property_blocks
            .contains(&id)
            .then(|| utopia_core::inline::attribute(&b.text).map(|(key, _)| key))
            .flatten();
        blocks.push(BlockView {
            id: id.as_str().to_string(),
            text: b.text.clone(),
            plain,
            depth: depth as u32,
            kind: kind.to_string(),
            heading_level,
            checked: b.checked,
            created_at_ms: times.get(&id).copied().unwrap_or(0),
            parts,
            placeholder,
            property_key,
        });
    }
    Ok(PageView {
        id: page.as_str().to_string(),
        title: utopia_store::display_title(&st, &fmt),
        date: st.meta.daily_date.map(|d| d.to_string()),
        blocks,
        properties,
        exists: true,
    })
}

/// A day that has no page yet: the day's identity and title, and nothing to write (FR-1.2).
fn blank_day(store: &Store, date: NaiveDate) -> PageView {
    PageView {
        id: PageId::for_date(date).as_str().to_string(),
        title: store.date_format().format(date),
        date: Some(date.to_string()),
        blocks: Vec::new(),
        properties: Vec::new(),
        exists: false,
    }
}

fn parse_day(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| FfiError::Invalid { msg: e.to_string() })
}

/// The title a page's words name, with the page's own spelling, when the words are still
/// that title or one of its aliases (AC-4.1, AC-4.11). `None` means the words changed.
fn link_title_for(store: &Store, words: &str) -> Option<(String, String)> {
    let key = utopia_core::title::normalize_title(words);
    if key.is_empty() {
        return None;
    }
    let page = utopia_store::resolve_page_key(store.conn(), &key, store.date_format()).ok()??;
    let (title, date) = page_label(store, &page)?;
    date.is_none().then(|| (page.as_str().to_string(), title))
}

fn written_link(
    store: &Store,
    text: &str,
    range: std::ops::Range<usize>,
    link: &AcceptedLink,
) -> Option<(Option<String>, String)> {
    let words = &text[range.clone()];
    if let Some(day) = &link.day {
        let typed = link.words.as_deref()?;
        let boundaries = link.typed
            || (text[..range.start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
                && text[range.end..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric()));
        if !boundaries || words.to_lowercase() != typed.to_lowercase() {
            return None;
        }
        let day = parse_day(day).ok()?;
        let label = store.date_format().format(day);
        let title = if utopia_core::date::parse_unambiguous_date(&label, store.date_format())
            == Some(day)
        {
            label
        } else {
            day.format("%Y-%m-%d").to_string()
        };
        return Some((Some(PageId::for_date(day).as_str().to_string()), title));
    }
    if let Some(typed) = &link.words {
        if words != typed.as_str() || typed.trim().is_empty() {
            return None;
        }
        let page = utopia_store::resolve_page_key(
            store.conn(),
            &utopia_core::title::normalize_title(typed),
            store.date_format(),
        )
        .ok()
        .flatten();
        return Some((page.map(|p| p.as_str().to_string()), typed.clone()));
    }
    let accepted = &link.title;
    let key = utopia_core::title::normalize_title(words);
    let accepted_key = utopia_core::title::normalize_title(accepted);
    if key.is_empty() {
        return None;
    }
    if let Some(page) = utopia_store::resolve_page_key(store.conn(), &key, store.date_format())
        .ok()
        .flatten()
    {
        let (title, _) = page_label(store, &page)?;
        let page_key = utopia_core::title::normalize_title(&title);
        if key == accepted_key || page_key == accepted_key {
            return Some((Some(page.as_str().to_string()), title));
        }
        return None;
    }
    (key == accepted_key).then(|| (None, words.trim().to_string()))
}

fn on_word_boundary(text: &str, range: &std::ops::Range<usize>) -> bool {
    text[..range.start]
        .chars()
        .next_back()
        .is_none_or(|c| !c.is_alphanumeric())
        && text[range.end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
}

fn typed_relative_day(
    inner: &str,
    today: NaiveDate,
    exists: impl Fn(&str) -> bool,
) -> Option<NaiveDate> {
    let phrase = inner.trim();
    if phrase.is_empty() || exists(phrase) {
        return None;
    }
    let found = utopia_core::date::find_date_phrases(phrase, today);
    (found.len() == 1 && found[0].range == (0..phrase.len()))
        .then(|| utopia_core::date::relative_date(phrase, today))
        .flatten()
}

fn absorb_typed_links_in(
    text: &str,
    links: Vec<AcceptedLink>,
    today: NaiveDate,
    format: &utopia_core::date::DateFormat,
    caret: Option<u32>,
    exists: impl Fn(&str) -> bool,
) -> AbsorbedDraft {
    let refs: Vec<std::ops::Range<usize>> = utopia_core::inline::extract_refs(text)
        .into_iter()
        .filter(|r| r.kind == utopia_core::inline::RefKind::Page)
        .map(|r| r.range)
        .collect();
    if refs.is_empty() {
        return AbsorbedDraft {
            text: text.to_string(),
            links,
            caret,
        };
    }
    let existing: Vec<(std::ops::Range<usize>, AcceptedLink)> = links
        .iter()
        .filter_map(|link| {
            utf16_range(text, link.start, link.end).map(|range| (range, link.clone()))
        })
        .collect();
    let typed_ranges: Vec<std::ops::Range<usize>> = refs
        .iter()
        .map(|range| range.start + 2..range.end.saturating_sub(2))
        .collect();
    let mut removals: Vec<std::ops::Range<usize>> = Vec::with_capacity(refs.len() * 2);
    for range in &refs {
        let open_end = (range.start + 2).min(range.end);
        let open = range.start..open_end;
        let close = range.end.saturating_sub(2)..range.end;
        if !open.is_empty() {
            removals.push(open);
        }
        if close.start > open_end && !close.is_empty() {
            removals.push(close);
        }
    }
    removals.sort_by_key(|r| r.start);
    let removed_before = |at: usize| -> usize {
        removals
            .iter()
            .take_while(|r| r.end <= at)
            .map(|r| r.end - r.start)
            .sum()
    };
    let mut new_text = String::with_capacity(text.len());
    let mut at = 0usize;
    for r in &removals {
        if r.start < at {
            continue;
        }
        new_text.push_str(&text[at..r.start]);
        at = r.end;
    }
    new_text.push_str(&text[at..]);
    let utf16_at = |byte: usize| utf16_len(&new_text[..byte]);
    let mapped_caret = caret.and_then(|c| utf16_to_byte(text, c)).map(|byte| {
        let at = removals
            .iter()
            .find(|r| r.start < byte && byte < r.end)
            .map_or(byte, |r| r.start);
        utf16_at(at - removed_before(at))
    });
    let mut out = Vec::with_capacity(existing.len() + refs.len());
    for (range, link) in &existing {
        if removals
            .iter()
            .any(|r| r.start < range.end && range.start < r.end)
            || typed_ranges
                .iter()
                .any(|r| r.start < range.end && range.start < r.end)
        {
            continue;
        }
        out.push(AcceptedLink {
            start: utf16_at(range.start - removed_before(range.start)),
            end: utf16_at(range.end - removed_before(range.end)),
            ..link.clone()
        });
    }
    for range in &refs {
        let inner_range = range.start + 2..range.end.saturating_sub(2);
        let inner = &text[inner_range.clone()];
        let start = utf16_at(inner_range.start - removed_before(inner_range.start));
        let end = utf16_at(inner_range.end - removed_before(inner_range.end));
        if let Some(day) =
            typed_relative_day(inner, today, &exists).filter(|_| on_word_boundary(text, range))
        {
            out.push(AcceptedLink {
                title: format.format(day),
                start,
                end,
                day: Some(day.to_string()),
                words: Some(inner.to_string()),
                typed: true,
            });
        } else {
            out.push(AcceptedLink {
                title: inner.to_string(),
                start,
                end,
                day: None,
                words: Some(inner.to_string()),
                typed: true,
            });
        }
    }
    out.sort_by_key(|l| l.start);
    AbsorbedDraft {
        text: new_text,
        links: out,
        caret: mapped_caret,
    }
}

const MAX_SUGGESTION_WORDS: usize = 6;

#[uniffi::export]
impl Graph {
    /// Open (or create) the graph file. `passphrase` unlocks a graph encrypted at rest.
    #[uniffi::constructor]
    pub fn open(path: String, passphrase: Option<String>) -> Result<Arc<Self>> {
        let store = Store::open_with_key(&path, passphrase.as_deref())?;
        Ok(Arc::new(Self::with_store(store)?))
    }

    /// An in-memory graph (previews and tests).
    #[uniffi::constructor]
    pub fn in_memory() -> Result<Arc<Self>> {
        Ok(Arc::new(Self::with_store(Store::open_in_memory()?)?))
    }

    /// Pin "today" (tests, screenshots); `None` follows the clock.
    pub fn set_today(&self, date: Option<String>) -> Result<()> {
        let d = match date {
            Some(s) => Some(
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(|e| FfiError::Invalid { msg: e.to_string() })?,
            ),
            None => None,
        };
        self.with(|s| {
            s.set_today(d);
            Ok(())
        })
    }

    /// Today's journal page (FR-1.1). A visit writes nothing: a day that has no page yet is
    /// answered empty, and the first Send creates it (FR-1.2, AC-1.3, AC-1.4).
    pub fn today(&self) -> Result<PageView> {
        self.with(|s| {
            let date = s.today();
            let page = s.days_between(date, date)?.get(&date).cloned();
            match page {
                Some(p) => page_view(s, &p),
                None => Ok(blank_day(s, date)),
            }
        })
    }

    /// One day, read-only, whether or not it has a page (the day log's earlier days, §4.4).
    pub fn day(&self, date: String) -> Result<PageView> {
        let d = parse_day(&date)?;
        self.with(|s| match s.days_between(d, d)?.get(&d).cloned() {
            Some(p) => page_view(s, &p),
            None => Ok(blank_day(s, d)),
        })
    }

    /// The days before `date` that have notes, newest first (scrolling up in the day log).
    /// A day that was only visited is not a page and is not among them (FR-1.2).
    pub fn days_before(&self, date: String, count: u32) -> Result<Vec<PageView>> {
        let d = parse_day(&date)?;
        self.with(|s| {
            let from = s.oldest_daily()?.unwrap_or(d);
            if from >= d {
                return Ok(Vec::new());
            }
            let days = s.days_between(from, d.pred_opt().unwrap_or(d))?;
            let mut found = Vec::new();
            for (day, page) in days.into_iter().rev() {
                if found.len() >= count as usize {
                    break;
                }
                let st = s.page_state(&page)?.clone();
                let has_content = st
                    .live_roots()
                    .iter()
                    .any(|id| st.block(id).is_some_and(|b| !b.text.trim().is_empty()));
                if !has_content {
                    continue;
                }
                let _ = day;
                found.push(page_view(s, &page)?);
            }
            Ok(found)
        })
    }

    pub fn page(&self, page_id: String) -> Result<PageView> {
        self.with(|s| page_view(s, &PageId(page_id)))
    }

    /// Append a draft outline to today's note (ios-prd C3, K11, AC-3.1): new top-level
    /// blocks at the end of the day, in the order written, in one plan and one commit, so
    /// one Send is one write and the whole capture arrives or none of it does.
    ///
    /// Empty lines are dropped first, then the depth rule (§4.5 rule 1) is applied again, so
    /// emptying a middle line makes the line below it a child of the line above (AC-3.5). A
    /// line that is a to-do becomes a to-do block that is not done (AC-5.1). Links are
    /// written by [`Graph::text_with_links`] before the call; their pages are made by the
    /// store on the same commit (C1).
    pub fn append_outline(&self, lines: Vec<OutlineLine>) -> Result<PageView> {
        let mut shaped: Vec<(usize, String, utopia_core::BlockKind)> = Vec::new();
        for line in lines {
            let text = line.text.trim();
            if text.is_empty() {
                continue;
            }
            let depth = match shaped.last() {
                None => 0,
                Some((d, _, _)) => (line.depth as usize).min(d + 1),
            };
            if depth >= utopia_core::outline::MAX_DROP_DEPTH {
                return Err(FfiError::Invalid {
                    msg: format!(
                        "a line can be at most {} levels deep",
                        utopia_core::outline::MAX_DROP_DEPTH - 1
                    ),
                });
            }
            let kind = if line.todo {
                utopia_core::BlockKind::Todo
            } else {
                utopia_core::BlockKind::Bullet
            };
            shaped.push((depth, text.to_string(), kind));
        }
        if shaped.is_empty() {
            return Err(FfiError::Invalid {
                msg: "the draft has no text".into(),
            });
        }
        self.with(|s| {
            let date = s.today();
            s.seal_undo();
            let page = s.ensure_daily_page(date)?;
            s.append_outline_kinds(&page, None, &shaped)?;
            s.flush()?;
            page_view(s, &page)
        })
    }

    /// Set whether a to-do is done (K9, AC-5.1, AC-5.6): one command, the same one the Mac's
    /// tick makes, and one undo step. A block that is not a to-do becomes one in the same
    /// call, because only a to-do has that state.
    pub fn set_todo_done(&self, block_id: String, done: bool) -> Result<()> {
        self.with(|s| {
            let block = BlockId(block_id);
            let row = s.block_row(&block)?.ok_or_else(|| FfiError::NotFound {
                msg: format!("block {block}"),
            })?;
            let kind =
                (row.kind != utopia_core::BlockKind::Todo).then_some(utopia_core::BlockKind::Todo);
            s.update_block(&row.page_id, &block, None, kind, Some(done))?;
            s.flush()?;
            Ok(())
        })
    }

    /// The pages that match `words`, in the day log's order, then the blocks that contain
    /// them (AC-8.1). Pages first, then blocks, each with its page's title.
    pub fn search(&self, words: String, limit: u32) -> Result<SearchResults> {
        let n = limit as usize;
        self.with(|s| {
            let mut pages = Vec::new();
            for hit in s.search_titles(&words, n)? {
                let (title, date) = page_label(s, &hit.page).unwrap_or((hit.title, None));
                pages.push(PageRef {
                    id: hit.page.as_str().to_string(),
                    title,
                    date,
                });
            }
            let mut blocks = Vec::new();
            for hit in s.search_blocks(&words, n)? {
                blocks.push(SearchHit {
                    page_id: hit.page.as_str().to_string(),
                    page_title: hit.page_title,
                    block_id: hit.block.as_str().to_string(),
                    text: utopia_core::inline::plain_text(&hit.text),
                });
            }
            Ok(SearchResults { pages, blocks })
        })
    }

    /// The pages changed most recently (AC-8.2), newest first.
    pub fn recent_pages(&self, limit: u32) -> Result<Vec<PageRef>> {
        self.with(|s| {
            Ok(s.recent_pages(limit as usize)?
                .into_iter()
                .map(|row| {
                    let date = row.daily_date.map(|d| d.to_string());
                    let title = match row.daily_date {
                        Some(d) => s.date_format().format(d),
                        None => row.title.clone(),
                    };
                    PageRef {
                        id: row.id.as_str().to_string(),
                        title,
                        date,
                    }
                })
                .collect())
        })
    }

    /// How many of this device's changes have not reached the relay yet (AC-9.1: "Offline —
    /// 3 changes queued").
    pub fn queued_changes(&self) -> Result<u32> {
        self.with(|s| Ok(s.outbox_count()? as u32))
    }

    /// The pages whose title or alias is in a draft line, as a whole phrase and in any letter
    /// case (AC-4.1, AC-4.3, AC-4.10). The ranges are UTF-16 offsets into `text`. Titles of
    /// one or two characters are not suggested (§4.6). `accepted` ranges are skipped.
    pub fn title_suggestions(
        &self,
        text: String,
        accepted: Vec<AcceptedLink>,
        limit: u32,
    ) -> Result<Vec<TitleMatch>> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let words = utopia_core::text::word_ranges(&text);
        // Longest window that starts here, so "Atlas Launch" wins over "Launch" (AC-4.3).
        let mut candidates: Vec<(std::ops::Range<usize>, String, String)> = Vec::new();
        self.with(|s| {
            for i in 0..words.len() {
                for take in (1..=MAX_SUGGESTION_WORDS.min(words.len() - i)).rev() {
                    let range = words[i].start..words[i + take - 1].end;
                    let phrase = &text[range.clone()];
                    if phrase.trim().chars().count() < 3 {
                        continue;
                    }
                    let Some((page_id, title)) = link_title_for(s, phrase) else {
                        continue;
                    };
                    candidates.push((range, page_id, title));
                    break;
                }
            }
            Ok(())
        })?;
        candidates.sort_by_key(|(r, _, _)| (r.start, std::cmp::Reverse(r.end)));
        let mut out: Vec<TitleMatch> = Vec::new();
        let mut taken: Vec<std::ops::Range<usize>> = accepted
            .iter()
            .filter_map(|a| utf16_range(&text, a.start, a.end))
            .collect();
        for (range, page_id, title) in candidates {
            if out.len() >= limit as usize {
                break;
            }
            if taken
                .iter()
                .any(|t| t.start < range.end && range.start < t.end)
            {
                continue;
            }
            out.push(TitleMatch {
                title,
                page_id,
                start: utf16_len(&text[..range.start]),
                end: utf16_len(&text[..range.end]),
            });
            taken.push(range);
        }
        Ok(out)
    }

    pub fn date_suggestions(
        &self,
        text: String,
        accepted: Vec<AcceptedLink>,
        limit: u32,
    ) -> Result<Vec<DateMatch>> {
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let taken: Vec<std::ops::Range<usize>> = accepted
            .iter()
            .filter_map(|a| utf16_range(&text, a.start, a.end))
            .collect();
        self.with(|s| {
            let today = s.today();
            let format = s.date_format().clone();
            Ok(utopia_core::date::find_date_phrases(&text, today)
                .into_iter()
                .filter(|phrase| {
                    !taken
                        .iter()
                        .any(|t| t.start < phrase.range.end && phrase.range.start < t.end)
                })
                .take(limit as usize)
                .map(|phrase| DateMatch {
                    title: format.format(phrase.date),
                    day: phrase.date.to_string(),
                    page_id: PageId::for_date(phrase.date).as_str().to_string(),
                    words: text[phrase.range.clone()].to_string(),
                    start: utf16_len(&text[..phrase.range.start]),
                    end: utf16_len(&text[..phrase.range.end]),
                })
                .collect())
        })
    }

    /// The text to store for a draft line, with each accepted link written as `[[Title]]` in
    /// the page's own spelling (AC-4.1, AC-4.11). A link whose words no longer name its page
    /// is written as plain text, so a changed word never leaves a stale link behind.
    pub fn text_with_links(&self, text: String, links: Vec<AcceptedLink>) -> Result<String> {
        let mut spans: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        self.with(|s| {
            for link in &links {
                let Some(range) = utf16_range(&text, link.start, link.end) else {
                    continue;
                };
                if let Some((_, title)) = written_link(s, &text, range.clone(), link) {
                    spans.push((range, title));
                }
            }
            Ok(())
        })?;
        spans.sort_by_key(|(r, _)| r.start);
        let mut out = String::with_capacity(text.len() + 4 * spans.len());
        let mut at = 0usize;
        for (range, title) in spans {
            if range.start < at {
                continue;
            }
            out.push_str(&text[at..range.start]);
            out.push_str("[[");
            out.push_str(&title);
            out.push_str("]]");
            at = range.end;
        }
        out.push_str(&text[at..]);
        Ok(out)
    }

    pub fn absorb_typed_links(
        &self,
        text: String,
        links: Vec<AcceptedLink>,
        caret: Option<u32>,
    ) -> Result<AbsorbedDraft> {
        self.with(|s| {
            let today = s.today();
            let format = s.date_format().clone();
            let exists = |title: &str| s.find_page(title).ok().flatten().is_some();
            Ok(absorb_typed_links_in(
                &text, links, today, &format, caret, exists,
            ))
        })
    }

    /// The drawn pieces of a draft line: plain runs and the links the reader accepted, in
    /// link colour (AC-4.1). A link whose words changed is plain again (AC-4.11).
    pub fn draft_parts(&self, text: String, links: Vec<AcceptedLink>) -> Result<Vec<TextPart>> {
        let mut spans: Vec<(std::ops::Range<usize>, PartKind, Option<String>)> = Vec::new();
        self.with(|s| {
            for link in &links {
                let Some(range) = utf16_range(&text, link.start, link.end) else {
                    continue;
                };
                if let Some((page_id, _)) = written_link(s, &text, range.clone(), link) {
                    let kind = if link.day.is_some() {
                        PartKind::Date
                    } else {
                        PartKind::Page
                    };
                    spans.push((range, kind, page_id));
                }
            }
            Ok(())
        })?;
        spans.sort_by_key(|(r, _, _)| r.start);
        let mut out: Vec<TextPart> = Vec::new();
        let mut at = 0usize;
        let mut offset = 0u32;
        let mut push = |out: &mut Vec<TextPart>, kind, text: &str, page: Option<String>| {
            if text.is_empty() {
                return;
            }
            let start = offset;
            offset += utf16_len(text);
            out.push(TextPart {
                kind,
                text: text.to_string(),
                page_id: page,
                start,
                end: offset,
            });
        };
        for (range, kind, page_id) in spans {
            if range.start < at {
                continue;
            }
            push(&mut out, PartKind::Plain, &text[at..range.start], None);
            push(&mut out, kind, &text[range.clone()], page_id);
            at = range.end;
        }
        push(&mut out, PartKind::Plain, &text[at..], None);
        Ok(out)
    }

    /// The page a title names (dates and aliases resolve like links), if any.
    pub fn find_page(&self, title: String) -> Result<Option<PageView>> {
        self.with(|s| {
            let key = utopia_core::title::normalize_title(&title);
            match utopia_store::resolve_page_key(s.conn(), &key, s.date_format())? {
                Some(p) => page_view(s, &p).map(Some),
                None => Ok(None),
            }
        })
    }

    /// Append a top-level block (fills a lone empty block instead). Links in the text
    /// create their pages and references on commit.
    pub fn append_block(&self, page_id: String, text: String) -> Result<BlockView> {
        self.with(|s| {
            let page = PageId(page_id);
            let st = s.page_state(&page)?.clone();
            let roots = st.live_roots();
            let reuse = match roots.as_slice() {
                [only] => {
                    st.block(only).is_some_and(|b| b.text.is_empty()) && !st.has_live_children(only)
                }
                _ => false,
            };
            s.seal_undo();
            let block = if reuse {
                roots[0].clone()
            } else {
                let plan = outline::append_block(&st, None);
                s.execute_plan(&page, plan, None)?
                    .ok_or_else(|| FfiError::Internal {
                        msg: "insert produced no block".into(),
                    })?
                    .block
            };
            s.set_block_text(&page, &block, &text, None, None)?;
            s.commit_block(&page, &block)?;
            s.flush()?;
            let (plain, parts) = parts_of(s, Some(&block), &text);
            let (kind, heading_level) = kind_name(utopia_core::BlockKind::Bullet);
            Ok(BlockView {
                id: block.as_str().to_string(),
                plain,
                text,
                depth: 0,
                kind: kind.to_string(),
                heading_level,
                checked: false,
                created_at_ms: s.block_row(&block)?.map_or(0, |r| r.updated_at),
                parts,
                placeholder: None,
                property_key: None,
            })
        })
    }

    pub fn set_block_text(&self, block_id: String, text: String) -> Result<()> {
        self.with(|s| {
            let block = BlockId(block_id);
            let row = s.block_row(&block)?.ok_or_else(|| FfiError::NotFound {
                msg: format!("block {block}"),
            })?;
            s.seal_undo();
            s.set_block_text(&row.page_id, &block, &text, None, None)?;
            s.commit_block(&row.page_id, &block)?;
            s.flush()?;
            Ok(())
        })
    }

    /// Linked References of a page, newest source pages first.
    pub fn backlinks(&self, page_id: String) -> Result<Vec<RefView>> {
        self.with(|s| {
            let page = PageId(page_id);
            let refs = s.linked_references(
                &page,
                &RefQuery {
                    sort: utopia_store::RefSort::Recency,
                    ..Default::default()
                },
            )?;
            Ok(refs
                .groups
                .into_iter()
                .flat_map(|g| {
                    let title = g.page.title.clone();
                    let pid = g.page.id.as_str().to_string();
                    g.hits.into_iter().map(move |h| RefView {
                        page_id: pid.clone(),
                        page_title: title.clone(),
                        block_id: h.block.as_str().to_string(),
                        // A row the phone draws is readable text, never raw markup (§4.4).
                        text: utopia_core::inline::plain_text(&h.text),
                    })
                })
                .collect())
        })
    }

    /// Page titles for `[[` autocomplete.
    pub fn search_titles(&self, query: String, limit: u32) -> Result<Vec<String>> {
        self.with(|s| {
            Ok(s.search_titles(&query, limit as usize)?
                .into_iter()
                .map(|h| h.title)
                .collect())
        })
    }

    /// One sync round with a relay (§12). `e2e_passphrase` turns on end-to-end
    /// encryption; the same passphrase must be used on every device.
    ///
    /// The network half runs without the store lock (§14.3, rule 10), so `today`, `page`
    /// and `append_block` keep working while the relay is slow, and a call made while a
    /// round is in flight answers [`SyncOutcome::Busy`] instead of queueing a second round.
    pub fn sync(
        &self,
        relay_url: String,
        graph_id: String,
        token: Option<String>,
        e2e_passphrase: Option<String>,
    ) -> Result<SyncOutcome> {
        let transport = relay_transport(&relay_url, &graph_id, token, e2e_passphrase)?;
        self.sync_via(transport)
    }
}

/// The relay transport for a sync call, with end-to-end encryption when a passphrase is
/// given. Building it touches no store, so it happens before the round starts.
fn relay_transport(
    relay_url: &str,
    graph_id: &str,
    token: Option<String>,
    e2e_passphrase: Option<String>,
) -> Result<Box<dyn Transport>> {
    use utopia_sync::http::HttpTransport;
    let http = HttpTransport::new(relay_url, graph_id, token)?;
    Ok(match e2e_passphrase {
        Some(p) => {
            let key = utopia_sync::e2e::GraphKey::derive(&p, graph_id)?;
            Box::new(utopia_sync::e2e::Encrypted::new(
                http,
                key,
                graph_id.to_string(),
            ))
        }
        None => Box::new(http),
    })
}

/// The round's answer for the shell: [`SyncStatus`] (the desktop's one reading of a round,
/// so mobile and desktop cannot drift) plus the counts the shell shows.
fn outcome_of(result: &std::result::Result<SyncReport, SyncError>, store: &Store) -> SyncOutcome {
    // A refusal is read from the error, because only the error knows the relay's protocol
    // version (§12.6).
    match result {
        Err(SyncError::Unauthorized) => return SyncOutcome::NeedsAuth,
        Err(SyncError::ProtocolVersion(version)) => {
            return SyncOutcome::Unsupported { version: *version }
        }
        _ => {}
    }
    // `SyncStatus::from_result` answers `Synced`/`Partial` exactly for an `Ok` report, so a
    // summary is always there; the fallback keeps this total without a panic (rule 11).
    let summary = || result.as_ref().map(summary_of).unwrap_or_default();
    match SyncStatus::from_result(result, store) {
        SyncStatus::Synced { .. } => SyncOutcome::Synced { summary: summary() },
        SyncStatus::Partial {
            queued, blocked, ..
        } => SyncOutcome::Partial {
            summary: summary(),
            queued: queued as u32,
            blocked: blocked as u32,
        },
        SyncStatus::Offline { queued } => SyncOutcome::Offline {
            queued: queued as u32,
        },
        SyncStatus::LocalError { message } => SyncOutcome::LocalError { message },
        SyncStatus::NeedsAuth => SyncOutcome::NeedsAuth,
        // Returned above; kept total without a panic (rule 11) and without inventing a
        // version the client never saw.
        SyncStatus::Unsupported => SyncOutcome::Unsupported { version: 0 },
    }
}

/// The counts the shell shows. One failed page can appear twice in a report (a push and a
/// pull), so this counts pages, not failures.
fn summary_of(r: &SyncReport) -> SyncSummary {
    SyncSummary {
        pushed: r.pushed as u32,
        pulled_pages: r.pulled_pages as u32,
        imported_pages: r.imported.len() as u32,
        failed: r
            .failed
            .iter()
            .map(|f| f.page.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use utopia_sync::wire::{Manifest, ManifestPage, PullResponse, PushRequest, PushResponse};
    use utopia_sync::{CursorKind, CursorSpace, PROTOCOL_VERSION};

    type Result<T> = std::result::Result<T, SyncError>;

    type ManifestStep = Box<dyn FnMut() -> Result<Option<Manifest>> + Send>;
    type PushStep = Box<dyn FnMut(&PageId, PushRequest) -> Result<PushResponse> + Send>;
    type PullStep = Box<dyn FnMut(&PageId, u64) -> Result<PullResponse> + Send>;
    type SnapshotStep = Box<dyn FnMut(&PageId, u64) -> Result<()> + Send>;

    /// A transport with one closure per step, so a test can put a round in a known state
    /// (on the wire, refused, refused for one page) without a server.
    struct Scripted {
        /// The cursor space this "relay" speaks; a snapshot is only valid in the one that
        /// measured it.
        space: CursorSpace,
        on_manifest: ManifestStep,
        on_push: PushStep,
        on_pull: PullStep,
        on_snapshot: SnapshotStep,
    }

    impl Scripted {
        /// A reached, empty relay: no pages to pull, every push accepted.
        fn quiet() -> Self {
            Self {
                space: CursorSpace::new("relay:scripted#g", CursorKind::Seq),
                on_manifest: Box::new(|| Ok(Some(empty_manifest()))),
                on_push: Box::new(|_page, req| {
                    Ok(PushResponse {
                        accepted: req.updates.len(),
                        head_seq: 1,
                        duplicate: 0,
                    })
                }),
                on_pull: Box::new(|_page, _since| {
                    Ok(PullResponse {
                        snapshot: None,
                        updates: Vec::new(),
                        head_seq: 0,
                    })
                }),
                on_snapshot: Box::new(|_page, _covers| Ok(())),
            }
        }

        /// Every step answers `error` (once; a step asked again says the script is spent).
        fn failing(error: SyncError) -> Self {
            fn spent() -> SyncError {
                SyncError::Transport("scripted: spent".into())
            }
            let err = std::sync::Arc::new(std::sync::Mutex::new(Some(error)));
            let step = |err: &std::sync::Arc<std::sync::Mutex<Option<SyncError>>>| {
                err.lock()
                    .expect("the script is not poisoned")
                    .take()
                    .unwrap_or_else(spent)
            };
            Self {
                space: CursorSpace::new("relay:scripted#g", CursorKind::Seq),
                on_manifest: Box::new({
                    let err = err.clone();
                    move || Err(step(&err))
                }),
                on_push: Box::new({
                    let err = err.clone();
                    move |_page, _req| Err(step(&err))
                }),
                on_pull: Box::new({
                    let err = err.clone();
                    move |_page, _since| Err(step(&err))
                }),
                on_snapshot: Box::new(move |_page, _covers| Err(step(&err))),
            }
        }
    }

    impl Transport for Scripted {
        fn manifest(&mut self, _if_none_match: Option<u64>) -> Result<Option<Manifest>> {
            (self.on_manifest)()
        }
        fn cursor_space(&self) -> CursorSpace {
            self.space.clone()
        }
        fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
            (self.on_push)(page, req)
        }
        fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse> {
            (self.on_pull)(page, since_seq)
        }
        fn put_snapshot(
            &mut self,
            page: &PageId,
            covers_seq: u64,
            _payload: Vec<u8>,
        ) -> Result<()> {
            (self.on_snapshot)(page, covers_seq)
        }
    }

    fn empty_manifest() -> Manifest {
        Manifest {
            rev: 1,
            protocol_version: PROTOCOL_VERSION,
            pages: Vec::new(),
            instance: "scripted".into(),
        }
    }

    /// One page, `head_seq` long, with `snapshot_seq` already compacted.
    fn manifest_with(page: &PageId, head_seq: u64, snapshot_seq: u64) -> Manifest {
        Manifest {
            pages: vec![ManifestPage {
                page_id: page.as_str().to_string(),
                head_seq,
                frontier: String::new(),
                snapshot_seq,
                tail_bytes: 0,
                updated_at: 0,
                deleted: false,
            }],
            ..empty_manifest()
        }
    }

    /// §12.6, §2.3.7 (the 2026-11-23 incident, mobile side): a page holding another
    /// device's changes that cannot apply yet is `Partial` with `blocked`, never "Synced".
    #[test]
    fn ac_12_6_mobile_outcome_names_blocked_pages() {
        use utopia_sync::relay::{Relay, RelayTransport};
        use utopia_sync::SyncClient;
        fn write(s: &mut Store, page: &PageId, text: &str) {
            let plan = utopia_core::outline::append_block(s.page_state(page).unwrap(), None);
            let id = s.execute_plan(page, plan, None).unwrap().unwrap().block;
            s.set_block_text(page, &id, text, None, None).unwrap();
            s.commit_block(page, &id).unwrap();
        }
        let relay = Relay::new();
        // The other device's first change is marked sent but never reaches this relay (an
        // older build that sent it to a mistyped graph id); its next one does, and depends
        // on the first.
        let mut other = Store::open_in_memory().unwrap();
        let mut right =
            SyncClient::new(RelayTransport::new(relay.clone(), "utopia"), &other).unwrap();
        let page = other.create_page("Shared").unwrap();
        write(&mut other, &page, "synced normally");
        right.sync_once(&mut other).unwrap();
        right.sync_once(&mut other).unwrap();
        write(&mut other, &page, "never reached this relay");
        let lost: Vec<i64> = other
            .outbox_pending(100)
            .unwrap()
            .into_iter()
            .map(|(seq, _, _, _)| seq)
            .collect();
        other.outbox_mark_sent(&lost).unwrap();
        write(&mut other, &page, "pushed to the right graph id");
        right.sync_once(&mut other).unwrap();

        let g = Graph::in_memory().unwrap();
        match g
            .sync_via(Box::new(RelayTransport::new(relay.clone(), "utopia")))
            .unwrap()
        {
            SyncOutcome::Partial {
                summary, blocked, ..
            } => {
                assert_eq!(blocked, 1, "the page that waits is named");
                assert_eq!(summary.failed, 0, "nothing failed: it is held");
            }
            other => panic!("expected Partial with a blocked page, got {other:?}"),
        }
    }

    /// §12.3 step 4, §12.7 (the 2026-11-23 incident, mobile side): the phone syncs today's
    /// note to a mistyped graph id, then to the real one with no destination reset. At
    /// first contact it sends what the real graph lacks, so the Mac gets every line.
    #[test]
    fn ac_12_7_mobile_changed_graph_id_sends_earlier_changes() {
        use utopia_sync::relay::{Relay, RelayTransport};
        use utopia_sync::SyncClient;
        let relay = Relay::new();
        let phone = Graph::in_memory().unwrap();
        phone.set_today(Some("2026-11-23".into())).unwrap();
        phone
            .append_outline(vec![line("Now syncing", 0, false)])
            .unwrap();
        phone
            .sync_via(Box::new(RelayTransport::new(relay.clone(), "Utopia")))
            .unwrap();
        phone
            .sync_via(Box::new(RelayTransport::new(relay.clone(), "utopia")))
            .unwrap();
        let day = phone
            .append_outline(vec![line("Testing from mobile", 0, false)])
            .unwrap();
        phone
            .sync_via(Box::new(RelayTransport::new(relay.clone(), "utopia")))
            .unwrap();

        let mut mac = Store::open_in_memory().unwrap();
        let mut client =
            SyncClient::new(RelayTransport::new(relay.clone(), "utopia"), &mac).unwrap();
        client.sync_once(&mut mac).unwrap();
        assert_eq!(mac.waiting_count().unwrap(), 0, "nothing waits on the Mac");
        let page = PageId(day.id.clone());
        let st = mac.page_state(&page).unwrap().clone();
        let mut texts: Vec<String> = st
            .live_preorder()
            .into_iter()
            .map(|(id, _)| st.block(&id).unwrap().text.clone())
            .collect();
        texts.sort();
        assert_eq!(texts, vec!["Now syncing", "Testing from mobile"]);
    }

    fn graph_with_two_days() -> (Arc<Graph>, PageView, PageView) {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.append_outline(vec![line("one", 0, false)]).unwrap();
        g.set_today(Some("2026-11-03".into())).unwrap();
        let next = g.append_outline(vec![line("two", 0, false)]).unwrap();
        (g, day, next)
    }

    /// Issue 1: other FFI calls keep working while a round is on the wire. The store lock
    /// is taken for `prepare`/`apply`, never for the network.
    #[test]
    fn a_round_does_not_hold_the_store_lock_on_the_wire() {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.append_outline(vec![line("queued", 0, false)]).unwrap();
        let free = Arc::new(AtomicBool::new(true));
        let probes = Arc::new(AtomicU32::new(0));
        let probe = |free: &Arc<AtomicBool>, probes: &Arc<AtomicU32>, g: &Arc<Graph>| {
            probes.fetch_add(1, Ordering::SeqCst);
            if g.store.try_lock().is_err() {
                free.store(false, Ordering::SeqCst);
            }
        };
        let page = PageId(day.id.clone());
        let mut t = Scripted::quiet();
        t.on_manifest = Box::new({
            let (free, probes, g) = (free.clone(), probes.clone(), g.clone());
            move || {
                probe(&free, &probes, &g);
                Ok(Some(manifest_with(&page, 0, 0)))
            }
        });
        t.on_push = Box::new({
            let (free, probes, g) = (free.clone(), probes.clone(), g.clone());
            move |_page, req| {
                probe(&free, &probes, &g);
                Ok(PushResponse {
                    accepted: req.updates.len(),
                    head_seq: 1,
                    duplicate: 0,
                })
            }
        });
        t.on_pull = Box::new({
            let (free, probes, g) = (free.clone(), probes.clone(), g.clone());
            move |_page, _since| {
                probe(&free, &probes, &g);
                Ok(PullResponse {
                    snapshot: None,
                    updates: Vec::new(),
                    head_seq: 1,
                })
            }
        });
        let outcome = g.sync_via(Box::new(t)).unwrap();
        assert!(matches!(outcome, SyncOutcome::Synced { .. }), "{outcome:?}");
        assert!(
            probes.load(Ordering::SeqCst) >= 3,
            "the round must actually reach the wire"
        );
        assert!(
            free.load(Ordering::SeqCst),
            "the store lock was held during the exchange, so `today`/`append_block` block"
        );
    }

    /// Issue 2: a second tap cannot queue a second round behind the first.
    #[test]
    fn a_second_round_while_one_runs_is_busy() {
        let g = Graph::in_memory().unwrap();
        let (reached, on_wire) = mpsc::channel();
        let (release, held) = mpsc::channel::<()>();
        let mut blocking = Scripted::quiet();
        blocking.on_manifest = Box::new(move || {
            reached.send(()).expect("the test is listening");
            held.recv_timeout(Duration::from_secs(10))
                .expect("the test releases the round");
            Ok(Some(empty_manifest()))
        });
        let round = {
            let g = g.clone();
            std::thread::spawn(move || g.sync_via(Box::new(blocking)).unwrap())
        };
        on_wire
            .recv_timeout(Duration::from_secs(10))
            .expect("the first round reached the wire");
        assert_eq!(
            g.sync_via(Box::new(Scripted::quiet())).unwrap(),
            SyncOutcome::Busy,
            "a second round must not start while the first holds the engine"
        );
        release.send(()).expect("the round is still waiting");
        assert!(matches!(round.join().unwrap(), SyncOutcome::Synced { .. }));
        // The engine came home, so the next tap runs a round again.
        assert!(!matches!(
            g.sync_via(Box::new(Scripted::quiet())).unwrap(),
            SyncOutcome::Busy
        ));
    }

    /// The engine goes home on every path, including a round that fails before it applies:
    /// a stuck `Running` would answer `Busy` forever.
    #[test]
    fn the_engine_comes_home_after_a_failed_round() {
        let g = Graph::in_memory().unwrap();
        let outcome = g
            .sync_via(Box::new(Scripted::failing(SyncError::Transport(
                "offline".into(),
            ))))
            .unwrap();
        assert!(
            matches!(outcome, SyncOutcome::Offline { .. }),
            "{outcome:?}"
        );
        assert!(
            matches!(
                g.sync_via(Box::new(Scripted::quiet())).unwrap(),
                SyncOutcome::Synced { .. }
            ),
            "a failed round must not leave the graph busy"
        );
    }

    /// Issue 3: the shell can tell a refusal from a bad network, and neither blames the
    /// other.
    #[test]
    fn a_refusal_is_not_offline() {
        let g = Graph::in_memory().unwrap();
        let outcome = |error: SyncError| {
            g.sync_via(Box::new(Scripted::failing(error)))
                .expect("a round reports its answer, it does not fail the call")
        };
        assert_eq!(
            outcome(SyncError::Unauthorized),
            SyncOutcome::NeedsAuth,
            "§12.6: 401 halts sync for a re-auth, it is not Offline"
        );
        assert_eq!(
            outcome(SyncError::ProtocolVersion(9)),
            SyncOutcome::Unsupported { version: 9 },
            "§12.6: another protocol version is its own answer"
        );
        assert!(
            matches!(
                outcome(SyncError::Transport("refused".into())),
                SyncOutcome::Offline { .. }
            ),
            "a transport failure is Offline"
        );
        let local = outcome(SyncError::Store(StoreError::Invalid("disk".into())));
        assert!(
            matches!(&local, SyncOutcome::LocalError { message } if message.contains("disk")),
            "{local:?}"
        );
    }

    /// §12.6: a relay that took some pages and refused one is not "Offline" and not a clean
    /// sync either; the shell shows the counts and the warning.
    #[test]
    fn a_page_that_fails_while_the_rest_moves_is_partial() {
        let (g, refused_day, other_day) = graph_with_two_days();
        let refused = refused_day.id.clone();
        assert_ne!(refused, other_day.id);
        let mut t = Scripted::quiet();
        t.on_push = Box::new(move |page, req| {
            if page.as_str() == refused {
                Err(SyncError::Transport("relay refused this page".into()))
            } else {
                Ok(PushResponse {
                    accepted: req.updates.len(),
                    head_seq: 1,
                    duplicate: 0,
                })
            }
        });
        match g.sync_via(Box::new(t)).unwrap() {
            SyncOutcome::Partial {
                summary, queued, ..
            } => {
                assert_eq!(summary.failed, 1, "one page failed");
                assert!(summary.pushed > 0, "the other page's push was accepted");
                assert!(queued >= 1, "the refused page's rows stay queued");
            }
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    /// Issue 4: a compaction snapshot queued by one round is uploaded by the next, because
    /// the engine (which holds the queue) lives on the graph, not on the round.
    #[test]
    fn a_queued_snapshot_rides_the_next_round() {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.append_outline(vec![line("long tail", 0, false)]).unwrap();
        let page = PageId(day.id.clone());

        // Round 1: the relay's tail for the page looks 1000 updates long, so applying it
        // queues a snapshot; a refused push stops the round before that snapshot goes out.
        let mut first = Scripted::quiet();
        first.on_manifest = Box::new({
            let page = page.clone();
            move || Ok(Some(manifest_with(&page, 1000, 0)))
        });
        first.on_push =
            Box::new(|_page, _req| Err(SyncError::Transport("refused for the test".into())));
        first.on_pull = Box::new(|_page, _since| {
            Ok(PullResponse {
                snapshot: None,
                updates: Vec::new(),
                head_seq: 1000,
            })
        });
        let _ = g.sync_via(Box::new(first)).unwrap();

        // Round 2: the tail now looks compacted, so nothing asks for a new snapshot; the
        // only way one is uploaded is if round 1's queue outlived it.
        let sent = Arc::new(AtomicU32::new(0));
        let mut second = Scripted::quiet();
        second.on_manifest = Box::new(move || Ok(Some(manifest_with(&page, 1000, 1000))));
        second.on_snapshot = Box::new({
            let sent = sent.clone();
            move |_page, _covers| {
                sent.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        let _ = g.sync_via(Box::new(second)).unwrap();
        assert_eq!(
            sent.load(Ordering::SeqCst),
            1,
            "the snapshot queued by the first round must ride the second"
        );
    }

    /// The round's answer keeps the desktop's wording (`SyncStatus`) so the two surfaces
    /// cannot drift: the same report maps to the same counts.
    #[test]
    fn the_summary_counts_pages_not_failures() {
        let page = PageId("p".into());
        let report = SyncReport {
            pushed: 2,
            duplicates: 1,
            pulled_pages: 3,
            imported: vec![page.clone()],
            failed: vec![
                utopia_sync::Failure {
                    page: Some(page.clone()),
                    kind: utopia_sync::FailureKind::Transport("push".into()),
                },
                utopia_sync::Failure {
                    page: Some(page.clone()),
                    kind: utopia_sync::FailureKind::Transport("pull".into()),
                },
            ],
            ..Default::default()
        };
        let summary = summary_of(&report);
        assert_eq!(summary.pushed, 2);
        assert_eq!(summary.imported_pages, 1);
        assert_eq!(summary.failed, 1, "one page, two failures");
        assert_eq!(
            report
                .failed
                .iter()
                .map(|f| f.page.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            1
        );
    }

    /// Retaining the engine must not carry a compaction snapshot to another destination:
    /// the snapshot is only this device's page, and a relay's log is not ours to truncate
    /// (AGENTS.md rule 5). A snapshot measured on one relay must not be uploaded to a
    /// graph id or relay url the next call points at.
    #[test]
    fn a_snapshot_queued_for_one_relay_is_not_uploaded_to_another() {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        let day = g.append_outline(vec![line("long tail", 0, false)]).unwrap();
        let page = PageId(day.id.clone());

        // Round 1 against the first relay: the tail looks 1000 updates long, so applying
        // it queues a snapshot, and a refused push stops the round before it goes out.
        let mut first = Scripted::quiet();
        first.space = CursorSpace::new("relay:first#g", CursorKind::Seq);
        first.on_manifest = Box::new({
            let page = page.clone();
            move || Ok(Some(manifest_with(&page, 1000, 0)))
        });
        first.on_push =
            Box::new(|_page, _req| Err(SyncError::Transport("refused for the test".into())));
        first.on_pull = Box::new(|_page, _since| {
            Ok(PullResponse {
                snapshot: None,
                updates: Vec::new(),
                head_seq: 1000,
            })
        });
        let _ = g.sync_via(Box::new(first));

        // Round 2 goes to another relay that already holds its own history for the page.
        let uploaded = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let mut second = Scripted::quiet();
        second.space = CursorSpace::new("relay:second#g", CursorKind::Seq);
        second.on_manifest = Box::new({
            let page = page.clone();
            move || Ok(Some(manifest_with(&page, 5000, 5000)))
        });
        second.on_pull = Box::new(|_page, _since| {
            Ok(PullResponse {
                snapshot: None,
                updates: Vec::new(),
                head_seq: 5000,
            })
        });
        second.on_snapshot = Box::new({
            let uploaded = uploaded.clone();
            move |_page, covers| {
                uploaded.lock().expect("not poisoned").push(covers);
                Ok(())
            }
        });
        let _ = g.sync_via(Box::new(second));
        assert_eq!(
            uploaded.lock().expect("not poisoned").as_slice(),
            &[] as &[u64],
            "a snapshot covering the first relay's head was uploaded to the second"
        );
    }

    /// The store is not locked while the exchange runs, so a capture can land after
    /// `prepare` gathered nothing. A round that stopped there would answer `Synced` with
    /// the capture still queued; it must make another pass instead.
    #[test]
    fn an_edit_made_during_the_exchange_is_pushed_before_the_round_reports_synced() {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-02".into())).unwrap();
        g.append_outline(vec![line("first", 0, false)]).unwrap();
        // The append queued its first write; send it so the round under test really starts
        // with nothing to push.
        g.sync_via(Box::new(Scripted::quiet())).unwrap();
        assert_eq!(
            g.guard().expect("not poisoned").outbox_count().unwrap(),
            0,
            "the round under test starts with an empty outbox"
        );
        let captured = Arc::new(AtomicBool::new(false));
        let pushed = Arc::new(AtomicU32::new(0));
        let mut t = Scripted::quiet();
        t.on_manifest = Box::new({
            let g = g.clone();
            let captured = captured.clone();
            move || {
                if !captured.swap(true, Ordering::SeqCst) {
                    // A capture lands while the manifest request is on the wire.
                    g.append_outline(vec![line("typed during the round", 0, false)])
                        .expect("the store is free during the exchange");
                }
                Ok(Some(empty_manifest()))
            }
        });
        t.on_push = Box::new({
            let pushed = pushed.clone();
            move |_page, req| {
                pushed.fetch_add(req.updates.len() as u32, Ordering::SeqCst);
                Ok(PushResponse {
                    accepted: req.updates.len(),
                    head_seq: 1,
                    duplicate: 0,
                })
            }
        });
        let outcome = g.sync_via(Box::new(t)).unwrap();
        assert!(
            matches!(&outcome, SyncOutcome::Synced { summary } if summary.pushed >= 1),
            "the capture made during the exchange was left unsent behind {outcome:?}"
        );
        assert!(
            pushed.load(Ordering::SeqCst) >= 1,
            "the round pushed nothing"
        );
    }

    // ---- The phone's contract (docs/ios-prd.md §4, §6, C1-C3) -------------------------

    fn phone() -> Arc<Graph> {
        let g = Graph::in_memory().unwrap();
        g.set_today(Some("2026-11-21".into())).unwrap();
        g
    }

    fn line(text: &str, depth: u32, todo: bool) -> OutlineLine {
        OutlineLine {
            text: text.into(),
            depth,
            todo,
        }
    }

    /// Rows of the last write, as `(text, depth, kind)`.
    fn rows(day: &PageView) -> Vec<(String, u32, String)> {
        day.blocks
            .iter()
            .map(|b| (b.text.clone(), b.depth, b.kind.clone()))
            .collect()
    }

    /// FR-1.2, AC-1.3: looking at a day never makes it a page; the first Send does.
    #[test]
    fn ac_1_3_a_visit_creates_no_page() {
        let g = phone();
        let day = g.today().unwrap();
        assert!(!day.exists, "a visit must not create the day");
        assert!(day.blocks.is_empty());
        assert!(
            !g.with(|s| Ok(s.page_exists(&PageId(day.id.clone()))?))
                .unwrap(),
            "no shard may be written for a day that was only visited"
        );
        let after = g
            .append_outline(vec![line("Call the bank", 0, false)])
            .unwrap();
        assert!(after.exists, "the first Send makes the page");
        assert_eq!(
            rows(&after),
            vec![("Call the bank".into(), 0, "bullet".into())]
        );
    }

    /// AC-3.1: a nested draft arrives as a parent and its child.
    #[test]
    fn ac_3_1_a_nested_draft_lands_as_a_parent_and_child() {
        let g = phone();
        let day = g
            .append_outline(vec![
                line("Meeting with Dana", 0, false),
                line("Budget is approved", 1, false),
            ])
            .unwrap();
        assert_eq!(
            rows(&day),
            vec![
                ("Meeting with Dana".into(), 0, "bullet".into()),
                ("Budget is approved".into(), 1, "bullet".into()),
            ]
        );
    }

    /// AC-3.5: emptying the middle line makes the third line the first line's child.
    #[test]
    fn ac_3_5_empty_middle_line_reparents() {
        let g = phone();
        let day = g
            .append_outline(vec![
                line("first", 0, false),
                line("   ", 1, false),
                line("third", 2, false),
            ])
            .unwrap();
        assert_eq!(
            rows(&day),
            vec![
                ("first".into(), 0, "bullet".into()),
                ("third".into(), 1, "bullet".into()),
            ]
        );
    }

    /// AC-3.6: two lines at depth 0 are two entries, in the order written.
    #[test]
    fn ac_3_6_two_top_level_lines_are_two_entries() {
        let g = phone();
        let day = g
            .append_outline(vec![line("one", 0, false), line("two", 0, false)])
            .unwrap();
        assert_eq!(
            rows(&day),
            vec![
                ("one".into(), 0, "bullet".into()),
                ("two".into(), 0, "bullet".into()),
            ]
        );
    }

    /// AC-3.7, FR-2.9: no line may sit at block depth 12.
    #[test]
    fn ac_3_7_a_line_can_be_eleven_levels_deep_at_most() {
        let g = phone();
        let mut lines = vec![line("level 0", 0, false)];
        for d in 1..=11u32 {
            lines.push(line(&format!("level {d}"), d, false));
        }
        assert!(g.append_outline(lines).is_ok(), "depth 11 is allowed");
        let g = phone();
        let mut too_deep = vec![line("level 0", 0, false)];
        for d in 1..=12u32 {
            too_deep.push(line(&format!("level {d}"), d, false));
        }
        assert!(
            g.append_outline(too_deep).is_err(),
            "a line at depth 12 is refused"
        );
    }

    /// K11: each Send starts new top-level blocks at the end of the note, never below an
    /// entry sent before.
    #[test]
    fn k11_a_second_send_is_a_new_top_level_block() {
        let g = phone();
        g.append_outline(vec![
            line("first entry", 0, false),
            line("under it", 1, false),
        ])
        .unwrap();
        let day = g
            .append_outline(vec![line("second entry", 0, false)])
            .unwrap();
        assert_eq!(
            rows(&day),
            vec![
                ("first entry".into(), 0, "bullet".into()),
                ("under it".into(), 1, "bullet".into()),
                ("second entry".into(), 0, "bullet".into()),
            ]
        );
    }

    /// AC-4.1: the words stay as typed and the stored text carries the page's spelling.
    #[test]
    fn ac_4_1_accepted_link_written_with_page_spelling() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        let text = "Lunch with dana".to_string();
        let found = g.title_suggestions(text.clone(), vec![], 5).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].title, "Dana");
        let stored = g
            .text_with_links(
                text,
                vec![AcceptedLink {
                    title: found[0].title.clone(),
                    start: found[0].start,
                    end: found[0].end,
                    day: None,
                    words: None,
                    typed: false,
                }],
            )
            .unwrap();
        assert_eq!(stored, "Lunch with [[Dana]]");
        let day = g.append_outline(vec![line(&stored, 0, false)]).unwrap();
        assert_eq!(day.blocks[0].text, "Lunch with [[Dana]]");
        let dana = g.find_page("dana".into()).unwrap().unwrap();
        let refs = g.backlinks(dana.id.clone()).unwrap();
        assert_eq!(refs.len(), 1, "the link is a reference the page sees");
    }

    /// AC-4.3: a two-word title is suggested one word at a time, and one tap links it all.
    #[test]
    fn ac_4_3_a_two_word_title_links_as_one_phrase() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Atlas Launch").map(|_| ())?))
            .unwrap();
        assert!(
            g.title_suggestions("Atlas".into(), vec![], 5)
                .unwrap()
                .is_empty(),
            "half a title is not a suggestion"
        );
        let found = g
            .title_suggestions("Ship the Atlas Launch deck".into(), vec![], 5)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Atlas Launch");
        assert_eq!(
            &"Ship the Atlas Launch deck"[found[0].start as usize..found[0].end as usize],
            "Atlas Launch"
        );
    }

    /// AC-4.4: a suggestion never changes the text by itself.
    #[test]
    fn ac_4_4_suggestions_never_change_the_text() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        let text = "Lunch with dana".to_string();
        let _ = g.title_suggestions(text.clone(), vec![], 5).unwrap();
        assert_eq!(
            g.text_with_links(text.clone(), vec![]).unwrap(),
            text,
            "no accepted link, no change"
        );
    }

    /// AC-4.11: a word changed after the chip was tapped is plain text after Send.
    #[test]
    fn ac_4_11_changed_word_drops_the_link() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        let accepted = vec![AcceptedLink {
            title: "Dana".into(),
            start: 11,
            end: 15,
            day: None,
            words: None,
            typed: false,
        }];
        assert_eq!(
            g.text_with_links("Lunch with dana".into(), accepted.clone())
                .unwrap(),
            "Lunch with [[Dana]]"
        );
        assert_eq!(
            g.text_with_links("Lunch with Dan".into(), accepted.clone())
                .unwrap(),
            "Lunch with Dan",
            "a word that is no longer the title is plain text"
        );
        let parts = g.draft_parts("Lunch with Dan".into(), accepted).unwrap();
        assert!(
            parts.iter().all(|p| p.kind == PartKind::Plain),
            "the word draws as plain text again: {parts:?}"
        );
    }

    #[test]
    fn ac_4_14_a_pasted_bracketed_link_becomes_an_accepted_link() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Utopia").map(|_| ())?))
            .unwrap();
        let absorbed = g
            .absorb_typed_links("see [[utopia]] today".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.text, "see utopia today");
        assert_eq!(absorbed.links.len(), 1, "{absorbed:?}");
        assert_eq!(absorbed.links[0].title, "utopia");
        assert_eq!(absorbed.links[0].words.as_deref(), Some("utopia"));
        assert_eq!(absorbed.links[0].day, None);
        assert_eq!((absorbed.links[0].start, absorbed.links[0].end), (4, 10));
        assert_eq!(
            g.text_with_links(absorbed.text.clone(), absorbed.links.clone())
                .unwrap(),
            "see [[utopia]] today"
        );
        let parts = g.draft_parts(absorbed.text, absorbed.links).unwrap();
        assert!(
            parts
                .iter()
                .any(|p| p.kind == PartKind::Page && p.text == "utopia"),
            "{parts:?}"
        );
    }

    #[test]
    fn ac_4_14_a_typed_link_keeps_the_text_the_reader_wrote() {
        let g = phone();
        g.with(|s| {
            let page = s.create_page("Utopia")?;
            s.add_alias(&page, "UT")?;
            Ok(())
        })
        .unwrap();
        for (written, inner) in [
            ("[[utopia]]", "utopia"),
            ("[[UT]]", "UT"),
            ("[[2026-11-26]]", "2026-11-26"),
            ("[[Nov 26]]", "Nov 26"),
        ] {
            let absorbed = g.absorb_typed_links(written.into(), vec![], None).unwrap();
            assert_eq!(absorbed.text, inner, "{written}");
            assert_eq!(absorbed.links.len(), 1, "{written}: {absorbed:?}");
            assert_eq!(absorbed.links[0].day, None, "{written}");
            assert_eq!(absorbed.links[0].words.as_deref(), Some(inner), "{written}");
            assert_eq!(
                g.text_with_links(absorbed.text, absorbed.links).unwrap(),
                written,
                "a typed link is written exactly as typed"
            );
        }
    }

    #[test]
    fn ac_4_14_a_typed_link_to_a_new_page_is_made_by_send() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("[[Fresh Page]]".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.text, "Fresh Page");
        assert_eq!(absorbed.links[0].title, "Fresh Page");
        assert_eq!(absorbed.links[0].words.as_deref(), Some("Fresh Page"));
        assert!(
            g.find_page("Fresh Page".into()).unwrap().is_none(),
            "a typed link does not make the page before Send"
        );
        let stored = g.text_with_links(absorbed.text, absorbed.links).unwrap();
        assert_eq!(stored, "[[Fresh Page]]");
        let day = g.append_outline(vec![line(&stored, 0, false)]).unwrap();
        assert_eq!(day.blocks[0].text, "[[Fresh Page]]");
        assert!(
            g.find_page("Fresh Page".into()).unwrap().is_some(),
            "Send makes the page the typed link names"
        );
    }

    #[test]
    fn ac_4_14_a_typed_link_keeps_the_links_and_offsets_around_it() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        let existing = vec![AcceptedLink {
            title: "Dana".into(),
            start: 11,
            end: 15,
            day: None,
            words: None,
            typed: false,
        }];
        let absorbed = g
            .absorb_typed_links("Lunch with dana 🙂 [[Atlas]]".into(), existing, None)
            .unwrap();
        assert_eq!(absorbed.text, "Lunch with dana 🙂 Atlas");
        assert_eq!(absorbed.links.len(), 2, "{absorbed:?}");
        assert_eq!((absorbed.links[0].start, absorbed.links[0].end), (11, 15));
        assert_eq!((absorbed.links[1].start, absorbed.links[1].end), (19, 24));
        assert_eq!(
            g.text_with_links(absorbed.text, absorbed.links).unwrap(),
            "Lunch with [[Dana]] 🙂 [[Atlas]]"
        );
    }

    #[test]
    fn ac_4_14_a_typed_link_before_an_accepted_link_keeps_its_offsets() {
        let g = phone();
        let existing = vec![AcceptedLink {
            title: "Dana".into(),
            start: 15,
            end: 19,
            day: None,
            words: None,
            typed: false,
        }];
        let absorbed = g
            .absorb_typed_links("[[Atlas]] with dana".into(), existing, None)
            .unwrap();
        assert_eq!(absorbed.text, "Atlas with dana");
        assert_eq!(absorbed.links.len(), 2, "{absorbed:?}");
        assert_eq!((absorbed.links[0].start, absorbed.links[0].end), (0, 5));
        assert_eq!((absorbed.links[1].start, absorbed.links[1].end), (11, 15));
    }

    #[test]
    fn ac_4_14_two_typed_links_are_both_absorbed() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("[[Atlas]] and [[Utopia]]".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.text, "Atlas and Utopia");
        assert_eq!(absorbed.links.len(), 2, "{absorbed:?}");
        assert_eq!((absorbed.links[0].start, absorbed.links[0].end), (0, 5));
        assert_eq!((absorbed.links[1].start, absorbed.links[1].end), (10, 16));
    }

    #[test]
    fn ac_4_14_a_link_overlapping_the_brackets_is_dropped() {
        let g = phone();
        let existing = vec![AcceptedLink {
            title: "Atlas".into(),
            start: 0,
            end: 3,
            day: None,
            words: None,
            typed: false,
        }];
        let absorbed = g
            .absorb_typed_links("[[Atlas]]".into(), existing, None)
            .unwrap();
        assert_eq!(absorbed.text, "Atlas");
        assert_eq!(absorbed.links.len(), 1, "{absorbed:?}");
        assert_eq!(absorbed.links[0].words.as_deref(), Some("Atlas"));
    }

    #[test]
    fn ac_4_14_a_relative_date_in_brackets_is_a_date_link() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("call [[tomorrow]]".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.text, "call tomorrow");
        assert_eq!(absorbed.links[0].day.as_deref(), Some("2026-11-22"));
        assert_eq!(absorbed.links[0].words.as_deref(), Some("tomorrow"));
        assert_eq!(
            g.text_with_links(absorbed.text.clone(), absorbed.links.clone())
                .unwrap(),
            "call [[November 22nd, 2026]]"
        );
        assert!(g
            .draft_parts(absorbed.text, absorbed.links)
            .unwrap()
            .iter()
            .any(|p| p.kind == PartKind::Date));
        let next = g
            .absorb_typed_links("[[next friday]]".into(), vec![], None)
            .unwrap();
        assert_eq!(next.links[0].day.as_deref(), Some("2026-11-27"));
        assert_eq!(
            g.text_with_links(next.text, next.links).unwrap(),
            "[[November 27th, 2026]]"
        );
        let ago = g
            .absorb_typed_links("[[2 weeks ago]]".into(), vec![], None)
            .unwrap();
        assert_eq!(ago.links[0].day.as_deref(), Some("2026-11-07"));
    }

    #[test]
    fn ac_4_14_absorption_moves_the_caret_left_by_the_brackets_before_it() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("see [[Utopia]] now".into(), vec![], Some(18))
            .unwrap();
        assert_eq!(absorbed.text, "see Utopia now");
        assert_eq!(absorbed.caret, Some(14));
        let before = g
            .absorb_typed_links("see [[Utopia]] now".into(), vec![], Some(4))
            .unwrap();
        assert_eq!(before.caret, Some(4));
    }

    #[test]
    fn ac_4_14_lines_without_typed_brackets_are_unchanged() {
        let g = phone();
        let links = vec![AcceptedLink {
            title: "Dana".into(),
            start: 11,
            end: 15,
            day: None,
            words: None,
            typed: false,
        }];
        let absorbed = g
            .absorb_typed_links("Lunch with dana today".into(), links.clone(), Some(5))
            .unwrap();
        assert_eq!(absorbed.text, "Lunch with dana today");
        assert_eq!(absorbed.links, links);
        assert_eq!(absorbed.caret, Some(5));
    }

    #[test]
    fn ac_4_14_incomplete_or_code_brackets_stay_as_typed() {
        let g = phone();
        let incomplete = g.absorb_typed_links("a [[b".into(), vec![], None).unwrap();
        assert_eq!(incomplete.text, "a [[b");
        assert!(incomplete.links.is_empty());
        let code = g
            .absorb_typed_links("`[[Utopia]]`".into(), vec![], None)
            .unwrap();
        assert_eq!(code.text, "`[[Utopia]]`");
        assert!(code.links.is_empty());
    }

    #[test]
    fn ac_4_14_an_insert_at_the_start_of_a_typed_link_shifts_it() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("[[Utopia]] sync".into(), vec![], None)
            .unwrap();
        let rebased = rebase_links("Utopia sync".into(), 0, 0, "U".into(), absorbed.links);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (1, 7));
        assert_eq!(
            g.text_with_links("UUtopia sync".into(), rebased).unwrap(),
            "U[[Utopia]] sync"
        );
        let absorbed = g
            .absorb_typed_links("[[Utopia]] sync".into(), vec![], None)
            .unwrap();
        let rebased = rebase_links("Utopia sync".into(), 0, 0, "Use ".into(), absorbed.links);
        assert_eq!((rebased[0].start, rebased[0].end), (4, 10));
        assert_eq!(
            g.text_with_links("Use Utopia sync".into(), rebased)
                .unwrap(),
            "Use [[Utopia]] sync"
        );
    }

    #[test]
    fn ac_4_14_an_insert_at_the_end_of_a_typed_link_leaves_it() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("[[Utopia]] sync".into(), vec![], None)
            .unwrap();
        let rebased = rebase_links("Utopia sync".into(), 6, 0, "!".into(), absorbed.links);
        assert_eq!((rebased[0].start, rebased[0].end), (0, 6));
        assert_eq!(
            g.text_with_links("Utopia! sync".into(), rebased).unwrap(),
            "[[Utopia]]! sync"
        );
    }

    #[test]
    fn ac_4_14_a_delete_before_a_typed_link_keeps_it() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("U[[Utopia]] now".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.text, "UUtopia now");
        assert_eq!((absorbed.links[0].start, absorbed.links[0].end), (1, 7));
        let rebased = rebase_links("UUtopia now".into(), 0, 1, "".into(), absorbed.links);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (0, 6));
        assert_eq!(
            g.text_with_links("Utopia now".into(), rebased).unwrap(),
            "[[Utopia]] now"
        );
    }

    #[test]
    fn ac_4_14_a_typo_inside_a_typed_link_stays_a_verbatim_link() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("[[Utopai]]".into(), vec![], None)
            .unwrap();
        let rebased = rebase_links("Utopai".into(), 4, 2, "ia".into(), absorbed.links);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (0, 6));
        assert_eq!(rebased[0].words.as_deref(), Some("Utopia"));
        assert_eq!(
            g.text_with_links("Utopia".into(), rebased).unwrap(),
            "[[Utopia]]"
        );
    }

    #[test]
    fn ac_4_14_a_covering_edit_drops_a_typed_link() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("see [[Utopia]] now".into(), vec![], None)
            .unwrap();
        let rebased = rebase_links(
            "see Utopia now".into(),
            0,
            14,
            "Buy milk".into(),
            absorbed.links,
        );
        assert!(rebased.is_empty(), "{rebased:?}");
        assert_eq!(
            g.text_with_links("Buy milk".into(), rebased).unwrap(),
            "Buy milk"
        );
        let absorbed = g
            .absorb_typed_links("see [[Utopia]] now".into(), vec![], None)
            .unwrap();
        let left = rebase_links("see Utopia now".into(), 2, 4, "X".into(), absorbed.links);
        assert!(left.is_empty(), "{left:?}");
        let absorbed = g
            .absorb_typed_links("see [[Utopia]] now".into(), vec![], None)
            .unwrap();
        let right = rebase_links("see Utopia now".into(), 8, 4, "Y".into(), absorbed.links);
        assert!(right.is_empty(), "{right:?}");
    }

    #[test]
    fn ac_4_14_an_edit_across_two_typed_links_drops_both() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("[[Atlas]] and [[Utopia]]".into(), vec![], None)
            .unwrap();
        let rebased = rebase_links(
            "Atlas and Utopia".into(),
            3,
            10,
            "or".into(),
            absorbed.links,
        );
        assert!(rebased.is_empty(), "{rebased:?}");
    }

    #[test]
    fn ac_4_14_a_typed_relative_date_becomes_a_typed_page_link_when_edited() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("[[next friday]]".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.links[0].day.as_deref(), Some("2026-11-27"));
        let rebased = rebase_links("next friday".into(), 5, 6, "fri".into(), absorbed.links);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!(rebased[0].day, None);
        assert_eq!(rebased[0].words.as_deref(), Some("next fri"));
        assert_eq!(
            g.text_with_links("next fri".into(), rebased).unwrap(),
            "[[next fri]]"
        );
    }

    #[test]
    fn ac_4_14_an_edit_before_a_chip_link_shifts_its_range() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        let links = vec![AcceptedLink {
            title: "Dana".into(),
            start: 11,
            end: 15,
            day: None,
            words: None,
            typed: false,
        }];
        let rebased = rebase_links("Lunch with dana".into(), 0, 0, "Plan ".into(), links);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (16, 20));
        assert_eq!(
            g.text_with_links("Plan Lunch with dana".into(), rebased)
                .unwrap(),
            "Plan Lunch with [[Dana]]"
        );
    }

    #[test]
    fn ac_4_14_an_edit_that_does_not_fit_drops_the_links() {
        phone();
        let links = vec![AcceptedLink {
            title: "Dana".into(),
            start: 11,
            end: 15,
            day: None,
            words: None,
            typed: false,
        }];
        assert!(rebase_links("abc".into(), 10, 0, "x".into(), links).is_empty());
    }

    #[test]
    fn ac_4_14_a_relative_date_without_a_boundary_stays_verbatim() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("a[[tomorrow]]".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.text, "atomorrow");
        assert_eq!(absorbed.links[0].day, None);
        assert_eq!(absorbed.links[0].words.as_deref(), Some("tomorrow"));
        assert_eq!(
            g.text_with_links(absorbed.text, absorbed.links).unwrap(),
            "a[[tomorrow]]"
        );
    }

    #[test]
    fn ac_4_14_adjacent_pasted_links_both_survive_send() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let absorbed = g
            .absorb_typed_links("[[tomorrow]][[Dana]]".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.text, "tomorrowDana");
        assert_eq!(absorbed.links.len(), 2, "{absorbed:?}");
        assert_eq!((absorbed.links[0].start, absorbed.links[0].end), (0, 8));
        assert_eq!(absorbed.links[0].day.as_deref(), Some("2026-11-28"));
        assert!(absorbed.links[0].typed);
        assert_eq!((absorbed.links[1].start, absorbed.links[1].end), (8, 12));
        assert_eq!(absorbed.links[1].day, None);
        assert!(absorbed.links[1].typed);
        let stored = g
            .text_with_links(absorbed.text.clone(), absorbed.links.clone())
            .unwrap();
        assert!(
            stored.contains("]][[") && stored.ends_with("[[Dana]]"),
            "{stored}"
        );
        let parts = g.draft_parts(absorbed.text, absorbed.links).unwrap();
        assert_eq!(
            parts
                .iter()
                .filter(|p| p.kind == PartKind::Date || p.kind == PartKind::Page)
                .count(),
            2,
            "{parts:?}"
        );
    }

    #[test]
    fn ac_4_14_an_empty_bracket_pair_stays_as_typed() {
        let g = phone();
        let absorbed = g.absorb_typed_links("[[ ]]".into(), vec![], None).unwrap();
        assert_eq!(absorbed.text, "[[ ]]");
        assert!(absorbed.links.is_empty());
    }

    #[test]
    fn ac_4_14_rebase_keeps_utf16_offsets_with_emoji_and_cjk() {
        let g = phone();
        let absorbed = g
            .absorb_typed_links("🙂 [[Utopia]]".into(), vec![], None)
            .unwrap();
        assert_eq!((absorbed.links[0].start, absorbed.links[0].end), (3, 9));
        let rebased = rebase_links("🙂 Utopia".into(), 0, 0, "界".into(), absorbed.links);
        assert_eq!((rebased[0].start, rebased[0].end), (4, 10));
        assert_eq!(rebased[0].words.as_deref(), Some("Utopia"));
        let absorbed = g
            .absorb_typed_links("[[Utopia]]".into(), vec![], None)
            .unwrap();
        let rebased = rebase_links("Utopia".into(), 1, 2, "日本".into(), absorbed.links);
        assert_eq!((rebased[0].start, rebased[0].end), (0, 6));
        assert_eq!(rebased[0].words.as_deref(), Some("U日本pia"));
    }

    #[test]
    fn ac_4_14_an_insert_at_the_start_of_a_chip_link_shifts_it() {
        phone();
        let links = vec![AcceptedLink {
            title: "Dana".into(),
            start: 11,
            end: 15,
            day: None,
            words: None,
            typed: false,
        }];
        let rebased = rebase_links("Lunch with dana".into(), 11, 0, "X".into(), links);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (12, 16));
    }

    #[test]
    fn ac_4_14_a_relative_date_gives_way_to_a_real_page_named_tomorrow() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Tomorrow").map(|_| ())?))
            .unwrap();
        let absorbed = g
            .absorb_typed_links("[[Tomorrow]]".into(), vec![], None)
            .unwrap();
        assert_eq!(absorbed.links[0].day, None);
        assert_eq!(absorbed.links[0].words.as_deref(), Some("Tomorrow"));
        assert_eq!(
            g.text_with_links(absorbed.text, absorbed.links).unwrap(),
            "[[Tomorrow]]"
        );
    }

    /// AC-4.10: chips belong to the line, so another line finds other titles.
    #[test]
    fn ac_4_10_suggestions_follow_the_active_line() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        assert_eq!(
            g.title_suggestions("Ask Dana about it".into(), vec![], 5)
                .unwrap()
                .len(),
            1
        );
        assert!(g
            .title_suggestions("Ask about it".into(), vec![], 5)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn io_4_short_titles_and_days_are_not_title_suggestions() {
        let g = phone();
        g.with(|s| Ok(s.create_page("On").map(|_| ())?)).unwrap();
        assert!(g
            .title_suggestions("On the way".into(), vec![], 5)
            .unwrap()
            .is_empty());
        assert!(
            g.title_suggestions("November 21st, 2026".into(), vec![], 5)
                .unwrap()
                .is_empty(),
            "a day is a date chip, not a title chip (AC-4.12)"
        );
        assert!(
            !g.date_suggestions("November 21st, 2026".into(), vec![], 5)
                .unwrap()
                .is_empty(),
            "it is a date chip instead (AC-4.12)"
        );
    }

    #[test]
    fn ac_4_12_a_date_phrase_is_a_chip_and_writes_the_day() {
        let g = phone();
        let text = "call Ana tomorrow".to_string();
        let found = g.date_suggestions(text.clone(), vec![], 5).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].title, "November 22nd, 2026");
        assert_eq!(found[0].day, "2026-11-22");
        assert_eq!(found[0].words, "tomorrow");
        let range = utf16_range(&text, found[0].start, found[0].end).unwrap();
        assert_eq!(&text[range], "tomorrow");
        let link = AcceptedLink {
            title: found[0].title.clone(),
            start: found[0].start,
            end: found[0].end,
            day: Some(found[0].day.clone()),
            words: Some(found[0].words.clone()),
            typed: false,
        };
        let stored = g.text_with_links(text.clone(), vec![link.clone()]).unwrap();
        assert_eq!(stored, "call Ana [[November 22nd, 2026]]");
        let day = g.append_outline(vec![line(&stored, 0, false)]).unwrap();
        assert_eq!(day.blocks[0].text, "call Ana [[November 22nd, 2026]]");
        let page = PageId::for_date(parse_day("2026-11-22").unwrap());
        assert_eq!(
            g.backlinks(page.as_str().to_string()).unwrap().len(),
            1,
            "the link is a reference to the day"
        );
        let parts = g.draft_parts(text, vec![link]).unwrap();
        let date = parts
            .iter()
            .find(|p| p.kind == PartKind::Date)
            .expect("a date part");
        assert_eq!(date.text, "tomorrow", "the words stay as typed");
        assert_eq!(date.page_id.as_deref(), Some(page.as_str()));
    }

    #[test]
    fn ac_4_12_non_ascii_before_a_date_keeps_utf16_ranges() {
        let g = phone();
        let text = "🙂 tomorrow".to_string();
        let found = g.date_suggestions(text.clone(), vec![], 5).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].start, found[0].end), (3, 11));
        let range = utf16_range(&text, found[0].start, found[0].end).unwrap();
        assert_eq!(&text[range], "tomorrow");
        let link = AcceptedLink {
            title: found[0].title.clone(),
            start: found[0].start,
            end: found[0].end,
            day: Some(found[0].day.clone()),
            words: Some(found[0].words.clone()),
            typed: false,
        };
        assert_eq!(
            g.text_with_links(text, vec![link]).unwrap(),
            "🙂 [[November 22nd, 2026]]"
        );
    }

    #[test]
    fn ac_4_9_a_date_control_title_after_cjk_still_links() {
        let g = phone();
        let title = "November 22nd, 2026";
        let text = format!("明日は{title} ");
        let start = utf16_len("明日は");
        let end = start + utf16_len(title);
        let link = AcceptedLink {
            title: title.into(),
            start,
            end,
            day: None,
            words: None,
            typed: false,
        };
        assert_eq!(
            g.text_with_links(text, vec![link]).unwrap(),
            "明日は[[November 22nd, 2026]] "
        );
    }

    #[test]
    fn ac_4_6_a_picker_title_after_a_letter_still_links() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        let link = AcceptedLink {
            title: "Dana".into(),
            start: 4,
            end: 8,
            day: None,
            words: None,
            typed: false,
        };
        assert_eq!(
            g.text_with_links("withDana ".into(), vec![link.clone()])
                .unwrap(),
            "with[[Dana]] "
        );
        let parts = g.draft_parts("withDana ".into(), vec![link]).unwrap();
        assert!(
            parts
                .iter()
                .any(|p| p.kind == PartKind::Page && p.text == "Dana"),
            "{parts:?}"
        );
    }

    #[test]
    fn ac_4_13_bare_numbers_and_months_are_no_chip() {
        let g = phone();
        for text in ["26", "26th", "may", "march", "sun", "sat", "11/6/26", "nov"] {
            assert!(
                g.date_suggestions(text.into(), vec![], 5)
                    .unwrap()
                    .is_empty(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn ac_4_13_punctuation_does_not_join_a_month_to_a_day() {
        let g = phone();
        for text in ["in May. 5 of us", "may_5", "May, 5", "Nov. 26"] {
            assert!(
                g.date_suggestions(text.into(), vec![], 5)
                    .unwrap()
                    .is_empty(),
                "{text:?}"
            );
        }
        assert_eq!(
            g.date_suggestions("May 5".into(), vec![], 5).unwrap()[0].words,
            "May 5"
        );
    }

    #[test]
    fn ac_4_12_dates_sit_next_to_title_chips_and_skip_accepted_ranges() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Friday").map(|_| ())?))
            .unwrap();
        let text = "Ana friday and next friday".to_string();
        let dates = g.date_suggestions(text.clone(), vec![], 5).unwrap();
        assert_eq!(
            dates.iter().map(|d| d.words.as_str()).collect::<Vec<_>>(),
            vec!["friday", "next friday"]
        );
        assert!(
            g.title_suggestions(text.clone(), vec![], 5)
                .unwrap()
                .iter()
                .any(|t| t.title == "Friday"),
            "the page title is still offered"
        );
        let accepted = vec![AcceptedLink {
            title: dates[0].title.clone(),
            start: dates[0].start,
            end: dates[0].end,
            day: Some(dates[0].day.clone()),
            words: Some(dates[0].words.clone()),
            typed: false,
        }];
        let after = g.date_suggestions(text, accepted, 5).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].words, "next friday");
    }

    #[test]
    fn ac_4_13_changed_words_drop_the_date_link() {
        let g = phone();
        let text = "call Ana tomorrow".to_string();
        let found = g.date_suggestions(text, vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let changed = "call Ana next fri".to_string();
        assert_eq!(
            g.text_with_links(changed.clone(), vec![link.clone()])
                .unwrap(),
            changed
        );
        let parts = g.draft_parts(changed, vec![link]).unwrap();
        assert!(parts.iter().all(|p| p.kind == PartKind::Plain));
    }

    #[test]
    fn ac_4_13_a_word_that_grows_past_the_range_drops_the_link() {
        let g = phone();
        let found = g
            .date_suggestions("call Ana tomorrow".into(), vec![], 5)
            .unwrap()[0]
            .clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let grown = "call Ana tomorrows".to_string();
        assert_eq!(
            g.text_with_links(grown.clone(), vec![link.clone()])
                .unwrap(),
            grown
        );
        assert!(g
            .draft_parts(grown, vec![link])
            .unwrap()
            .iter()
            .all(|p| p.kind == PartKind::Plain));
    }

    #[test]
    fn ac_4_13_a_date_link_keeps_the_day_it_was_tapped_for() {
        let g = phone();
        let text = "call Ana today".to_string();
        let found = g.date_suggestions(text.clone(), vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        g.set_today(Some("2026-11-28".into())).unwrap();
        assert_eq!(
            g.text_with_links(text.clone(), vec![link.clone()]).unwrap(),
            "call Ana [[November 21st, 2026]]",
            "a weekday or today sent a week later keeps the day"
        );
        assert!(g
            .draft_parts(text, vec![link])
            .unwrap()
            .iter()
            .any(|p| p.kind == PartKind::Date));
        let absolute = AcceptedLink {
            title: "November 22nd, 2026".into(),
            start: 3,
            end: 9,
            day: Some("2026-11-22".into()),
            words: Some("Nov 22".into()),
            typed: false,
        };
        g.set_today(Some("2027-06-01".into())).unwrap();
        assert_eq!(
            g.text_with_links("on Nov 22".into(), vec![absolute])
                .unwrap(),
            "on [[November 22nd, 2026]]",
            "a month and day kept in a draft hold across a new year"
        );
    }

    #[test]
    fn ac_4_12_an_ambiguous_title_writes_the_iso_day() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        g.with(|s| {
            s.set_date_format("MM/dd/yyyy")?;
            Ok(())
        })
        .unwrap();
        let text = "call Ana next week".to_string();
        let found = g.date_suggestions(text.clone(), vec![], 5).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].title, "12/04/2026");
        assert_eq!(found[0].day, "2026-12-04");
        let link = AcceptedLink {
            title: found[0].title.clone(),
            start: found[0].start,
            end: found[0].end,
            day: Some(found[0].day.clone()),
            words: Some(found[0].words.clone()),
            typed: false,
        };
        let stored = g.text_with_links(text, vec![link.clone()]).unwrap();
        assert_eq!(stored, "call Ana [[2026-12-04]]");
        g.append_outline(vec![line(&stored, 0, false)]).unwrap();
        let page = PageId::for_date(parse_day("2026-12-04").unwrap());
        assert_eq!(
            g.backlinks(page.as_str().to_string()).unwrap().len(),
            1,
            "the written link resolves to the chosen day"
        );
        let parts = g
            .draft_parts("call Ana next week".into(), vec![link])
            .unwrap();
        let date = parts
            .iter()
            .find(|p| p.kind == PartKind::Date)
            .expect("a date part");
        assert_eq!(date.page_id.as_deref(), Some(page.as_str()));
    }

    #[test]
    fn ac_4_12_editing_before_a_date_link_keeps_the_day() {
        let g = phone();
        let text = "next week".to_string();
        let found = g.date_suggestions(text.clone(), vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let edited = "Call Ana next week".to_string();
        let rebased = rebase_links(text, 0, 0, "Call Ana ".into(), vec![link]);
        assert_eq!(rebased.len(), 1);
        assert_eq!((rebased[0].start, rebased[0].end), (9, 18));
        assert_eq!(
            g.text_with_links(edited.clone(), rebased.clone()).unwrap(),
            "Call Ana [[November 28th, 2026]]"
        );
        assert!(g
            .draft_parts(edited, rebased)
            .unwrap()
            .iter()
            .any(|p| p.kind == PartKind::Date));
    }

    #[test]
    fn ac_4_12_a_prefix_sharing_a_letter_keeps_the_date() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let found = g.date_suggestions("next week".into(), vec![], 5).unwrap()[0].clone();
        assert_eq!((found.start, found.end), (0, 9));
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let rebased = rebase_links("next week".into(), 0, 0, "note: ".into(), vec![link]);
        assert_eq!(rebased.len(), 1);
        assert_eq!((rebased[0].start, rebased[0].end), (6, 15));
        assert_eq!(
            g.text_with_links("note: next week".into(), rebased)
                .unwrap(),
            "note: [[December 4th, 2026]]"
        );
    }

    #[test]
    fn ac_4_12_a_case_change_keeps_the_chosen_day() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let found = g.date_suggestions("next week".into(), vec![], 5).unwrap()[0].clone();
        assert_eq!(found.day, "2026-12-04");
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day.clone()),
            words: Some(found.words),
            typed: false,
        };
        let rebased = rebase_links("next week".into(), 0, 1, "N".into(), vec![link]);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!(rebased[0].day.as_deref(), Some("2026-12-04"));
        g.set_today(Some("2026-12-10".into())).unwrap();
        assert_eq!(
            g.text_with_links("Next week".into(), rebased).unwrap(),
            "[[December 4th, 2026]]"
        );
    }

    #[test]
    fn ac_4_12_a_picker_insert_before_a_date_keeps_both() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let found = g.date_suggestions("next week".into(), vec![], 5).unwrap()[0].clone();
        let date = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let mut rebased = rebase_links("next week".into(), 0, 0, "Dana ".into(), vec![date]);
        rebased.push(AcceptedLink {
            title: "Dana".into(),
            start: 0,
            end: 4,
            day: None,
            words: None,
            typed: false,
        });
        assert_eq!(rebased.len(), 2);
        assert_eq!(
            g.text_with_links("Dana next week".into(), rebased).unwrap(),
            "[[Dana]] [[December 4th, 2026]]"
        );
    }

    #[test]
    fn ac_4_12_a_normalization_change_before_a_date_shifts_it() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let before = "caf\u{e9} next week";
        let found = g.date_suggestions(before.into(), vec![], 5).unwrap()[0].clone();
        assert_eq!((found.start, found.end), (5, 14));
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let rebased = rebase_links(before.into(), 3, 1, "e\u{301}".into(), vec![link]);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (6, 15));
        let after = "cafe\u{301} next week";
        assert_eq!(
            g.text_with_links(after.into(), rebased).unwrap(),
            "cafe\u{301} [[December 4th, 2026]]"
        );
    }

    #[test]
    fn ac_4_12_an_emoji_edit_before_a_date_keeps_it() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let before = "\u{1f600} next week";
        let after = "\u{1f601} next week";
        assert_eq!(
            edit_between_in(before, after, &[]),
            TextEdit {
                start: 0,
                removed: 2,
                replacement: "\u{1f601}".into(),
            }
        );
        let prefixed = edit_between_in("next week", "note: next week", &[]);
        assert_eq!(
            format!(
                "{}{}{}",
                &"next week"[..1],
                prefixed.replacement,
                &"next week"[1..]
            ),
            "note: next week"
        );
        let found = g.date_suggestions(before.into(), vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        assert_eq!((link.start, link.end), (3, 12));
        let rebased = rebase_links(before.into(), 0, 2, "\u{1f601}".into(), vec![link]);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (3, 12));
        assert_eq!(
            g.text_with_links(after.into(), rebased).unwrap(),
            "\u{1f601} [[December 4th, 2026]]"
        );
    }

    #[test]
    fn ac_4_12_a_fallback_prefix_insert_keeps_the_date() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let found = g.date_suggestions("next week".into(), vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let edit = edit_between_in("next week", "note: next week", std::slice::from_ref(&link));
        assert_eq!(
            edit,
            TextEdit {
                start: 0,
                removed: 0,
                replacement: "note: ".into(),
            }
        );
        let rebased = rebase_links(
            "next week".into(),
            edit.start,
            edit.removed,
            edit.replacement,
            vec![link],
        );
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (6, 15));
        assert_eq!(
            g.text_with_links("note: next week".into(), rebased)
                .unwrap(),
            "note: [[December 4th, 2026]]"
        );
    }

    #[test]
    fn ac_4_12_a_fallback_suffix_insert_keeps_the_date() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let found = g.date_suggestions("next week".into(), vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let edit = edit_between_in("next week", "next week week", std::slice::from_ref(&link));
        assert_eq!(
            edit,
            TextEdit {
                start: 9,
                removed: 0,
                replacement: " week".into(),
            }
        );
        let rebased = rebase_links(
            "next week".into(),
            edit.start,
            edit.removed,
            edit.replacement,
            vec![link],
        );
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!((rebased[0].start, rebased[0].end), (0, 9));
        assert_eq!(
            g.text_with_links("next week week".into(), rebased).unwrap(),
            "[[December 4th, 2026]] week"
        );
    }

    #[test]
    fn ac_4_12_replacing_the_whole_phrase_with_a_case_change_keeps_the_day() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let found = g.date_suggestions("next week".into(), vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let rebased = rebase_links("next week".into(), 0, 9, "Next week".into(), vec![link]);
        assert_eq!(rebased.len(), 1, "{rebased:?}");
        assert_eq!(rebased[0].day.as_deref(), Some("2026-12-04"));
        g.set_today(Some("2026-12-10".into())).unwrap();
        assert_eq!(
            g.text_with_links("Next week".into(), rebased).unwrap(),
            "[[December 4th, 2026]]"
        );
    }

    #[test]
    fn ac_4_12_replacing_the_whole_phrase_with_other_words_drops_the_link() {
        let g = phone();
        g.set_today(Some("2026-11-27".into())).unwrap();
        let found = g.date_suggestions("next week".into(), vec![], 5).unwrap()[0].clone();
        let link = AcceptedLink {
            title: found.title,
            start: found.start,
            end: found.end,
            day: Some(found.day),
            words: Some(found.words),
            typed: false,
        };
        let rebased = rebase_links("next week".into(), 0, 9, "last week".into(), vec![link]);
        assert!(rebased.is_empty(), "{rebased:?}");
        assert_eq!(
            g.text_with_links("last week".into(), rebased).unwrap(),
            "last week"
        );
    }

    #[test]
    fn ac_4_12_no_chip_inside_inline_code() {
        let g = phone();
        let dates = g
            .date_suggestions("`next week` and next week".into(), vec![], 5)
            .unwrap();
        assert_eq!(dates.len(), 1, "{dates:?}");
        assert_eq!(dates[0].words, "next week");
        assert_eq!((dates[0].start, dates[0].end), (16, 25));
    }

    /// AC-5.1: a sent to-do is not done.
    #[test]
    fn ac_5_1_a_sent_todo_is_not_done() {
        let g = phone();
        let day = g.append_outline(vec![line("Pay rent", 0, true)]).unwrap();
        assert_eq!(rows(&day), vec![("Pay rent".into(), 0, "todo".into())]);
        assert!(!day.blocks[0].checked);
    }

    /// AC-5.3, AC-5.6: a tick is one command on the same block, and a second tap removes it.
    #[test]
    fn ac_5_3_a_tick_and_its_removal_keep_the_block() {
        let g = phone();
        let day = g
            .append_outline(vec![line("Pay rent", 0, true), line("child", 1, false)])
            .unwrap();
        let todo = day.blocks[0].id.clone();
        g.set_todo_done(todo.clone(), true).unwrap();
        let day = g.today().unwrap();
        assert!(day.blocks[0].checked, "the tick shows at once");
        assert_eq!(day.blocks[0].text, "Pay rent", "the text does not change");
        assert_eq!(day.blocks[1].text, "child", "the children do not change");
        g.set_todo_done(todo, false).unwrap();
        assert!(
            !g.today().unwrap().blocks[0].checked,
            "a second tap removes it"
        );
    }

    /// AC-7.3, AC-7.5: attributes are rows above the outline; a block type the phone cannot
    /// draw in text gets a label instead of raw markup.
    #[test]
    fn ac_7_3_properties_and_ac_7_5_placeholders() {
        let g = phone();
        let page = g
            .with(|s| {
                let p = s.create_page("Atlas Launch")?;
                s.append_outline(
                    &p,
                    None,
                    &[
                        (0, "Status:: Active".to_string()),
                        (0, "{{query: [[TODO]]}}".to_string()),
                    ],
                )?;
                s.flush()?;
                Ok(p)
            })
            .unwrap();
        let view = g.page(page.as_str().to_string()).unwrap();
        assert_eq!(view.properties.len(), 1);
        assert_eq!(view.properties[0].key, "Status");
        assert_eq!(view.properties[0].value, "Active");
        let query = view
            .blocks
            .iter()
            .find(|b| b.placeholder.is_some())
            .expect("the query block gets a label");
        assert_eq!(query.placeholder.as_deref(), Some("Query — open on Mac"));
    }

    /// AC-8.1, AC-8.2: search lists pages first, then blocks; recents are the changed pages.
    #[test]
    fn ac_8_1_search_lists_pages_then_blocks_and_ac_8_2_recents() {
        let g = phone();
        g.with(|s| {
            let p = s.create_page("Atlas Launch")?;
            s.append_outline(&p, None, &[(0, "the atlas is late".to_string())])?;
            s.create_page("Budget")?;
            s.flush()?;
            Ok(())
        })
        .unwrap();
        let found = g.search("atl".into(), 10).unwrap();
        assert_eq!(found.pages.len(), 1);
        assert_eq!(found.pages[0].title, "Atlas Launch");
        assert!(found.blocks.iter().any(|b| b.page_title == "Atlas Launch"));
        let recents = g.recent_pages(8).unwrap();
        assert!(recents.len() >= 2);
        assert!(recents.iter().any(|p| p.title == "Atlas Launch"));
    }

    /// AC-7.1, §4.4: a link draws in link colour with no brackets, and names its page.
    #[test]
    fn ac_7_1_a_link_part_names_the_page_it_opens() {
        let g = phone();
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        let day = g
            .append_outline(vec![line("Lunch with [[Dana]]", 0, false)])
            .unwrap();
        let block = &day.blocks[0];
        let link = block
            .parts
            .iter()
            .find(|p| p.kind == PartKind::Page)
            .expect("the link is a part");
        assert_eq!(link.text, "Dana", "no brackets in the drawn text");
        assert!(link.page_id.is_some());
        assert_eq!(
            block.plain, "Lunch with Dana",
            "the parts join into the plain text"
        );
    }

    /// §4.4: the time column comes from the block, and a day that exists is a page.
    #[test]
    fn io_4_the_day_log_has_a_creation_time() {
        let g = phone();
        let day = g
            .append_outline(vec![line("Call the bank", 0, false)])
            .unwrap();
        assert!(day.blocks[0].created_at_ms > 0, "a block carries its time");
    }

    /// AC-4.6: a title that no page has is written as a link by the Send, and the page is
    /// made then — not when the picker inserts it.
    #[test]
    fn ac_4_6_a_created_page_exists_after_send_only() {
        let g = phone();
        let accepted = vec![AcceptedLink {
            title: "Quarterly plan".into(),
            start: 14,
            end: 28,
            day: None,
            words: None,
            typed: false,
        }];
        let text = g
            .text_with_links("Ask about the Quarterly plan".into(), accepted.clone())
            .unwrap();
        assert_eq!(text, "Ask about the [[Quarterly plan]]");
        assert!(
            g.find_page("Quarterly plan".into()).unwrap().is_none(),
            "the picker makes no page (AC-4.6)"
        );
        let day = g.append_outline(vec![line(&text, 0, false)]).unwrap();
        assert_eq!(day.blocks[0].text, "Ask about the [[Quarterly plan]]");
        assert!(
            g.find_page("Quarterly plan".into()).unwrap().is_some(),
            "the Send makes the page"
        );
        // A link whose words changed to another existing title does not silently retarget.
        g.with(|s| Ok(s.create_page("Dana").map(|_| ())?)).unwrap();
        assert_eq!(
            g.text_with_links(
                "Ask Dana".into(),
                vec![AcceptedLink {
                    title: "Quarterly plan".into(),
                    start: 4,
                    end: 8,
                    day: None,
                    words: None,
                    typed: false,
                }]
            )
            .unwrap(),
            "Ask Dana"
        );
        let _ = accepted;
    }
}
