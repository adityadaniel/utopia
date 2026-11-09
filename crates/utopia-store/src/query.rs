//! Read queries over the projections: backlinks (§7), the reference inspector (FR-4.6),
//! block-ref status (FR-4.3, FR-4.7), quick jump (§9.1 Cmd+K), journal (FR-1.3) and
//! block search for `((` autocomplete (FR-4.2).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use utopia_core::inline::{extract_refs, plain_text, RefKind};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, BlockKind, PageId, PageKind};

use crate::{Result, Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageRow {
    pub id: PageId,
    pub kind: PageKind,
    pub title: String,
    pub title_key: String,
    pub daily_date: Option<NaiveDate>,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

impl PageRow {
    pub fn is_daily(&self) -> bool {
        self.kind == PageKind::Daily
    }

    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }
}

/// One reference edge into a page or block, with what breadcrumbs and ordering need.
struct HitRow {
    block: String,
    page: String,
    a0: i64,
    a1: i64,
    text: String,
    updated_at: i64,
    parent: Option<String>,
    position: String,
}

fn hit_row(r: &Row<'_>) -> rusqlite::Result<HitRow> {
    Ok(HitRow {
        block: r.get(0)?,
        page: r.get(1)?,
        a0: r.get(2)?,
        a1: r.get(3)?,
        text: r.get(4)?,
        updated_at: r.get(5)?,
        parent: r.get(6)?,
        position: r.get(7)?,
    })
}
/// `(src_block, dst_kind, dst_page, dst_block, dst_title_key)`.
type OutRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

const PAGE_COLS: &str =
    "id, kind, title, title_key, daily_date, created_at, updated_at, deleted_at";

fn page_row(r: &Row<'_>) -> rusqlite::Result<PageRow> {
    let kind: String = r.get(1)?;
    let date: Option<String> = r.get(4)?;
    Ok(PageRow {
        id: PageId(r.get(0)?),
        kind: if kind == "daily" {
            PageKind::Daily
        } else {
            PageKind::Page
        },
        title: r.get(2)?,
        title_key: r.get(3)?,
        daily_date: date.and_then(|d| utopia_core::date::parse_iso(&d)),
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        deleted_at: r.get(7)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockRow {
    pub id: BlockId,
    pub page_id: PageId,
    pub parent_id: Option<BlockId>,
    pub depth: i64,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefSort {
    /// Daily pages newest first, then other pages by title (FR-3.3 default).
    #[default]
    Title,
    Recency,
}

#[derive(Debug, Clone, Default)]
pub struct RefQuery {
    pub sort: RefSort,
    /// Case-insensitive text filter within results (FR-3.3).
    pub text: Option<String>,
    /// Co-reference filters: keep only sources that also reference every listed page
    /// (FR-3.10). A source matches through the block, any ancestor, or its page.
    pub also: Vec<PageId>,
    /// Date filter (v1.1 "date filters"): keep sources dated on or after this day — the
    /// daily page's date, or the block's last edit for other pages.
    pub since: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefHit {
    pub block: BlockId,
    pub page: PageId,
    pub text: String,
    /// Ancestors of the referencing block, root first: `(id, plain text)`.
    pub breadcrumb: Vec<(BlockId, String)>,
    /// Byte ranges of the matching tokens in `text` (FR-4.9).
    pub anchors: Vec<(usize, usize)>,
    pub updated_at: i64,
    #[serde(skip)]
    order: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefGroup {
    pub page: PageRow,
    pub hits: Vec<RefHit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct LinkedRefs {
    /// Hits before filtering.
    pub total: usize,
    pub groups: Vec<RefGroup>,
}

/// A page the current hits co-reference: the chip a reference panel offers, with the number
/// of hits selecting it would keep (FR-3.10, LR-1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoReference {
    pub page: PageId,
    pub title: String,
    pub hits: usize,
}

/// How many co-reference chips a panel offers.
pub const CO_REFERENCE_LIMIT: usize = 6;

/// A referencing block's children for an inline preview (LR-4): the ones shown and how many
/// the block has in total, so the view can fold the rest behind its chip.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RefChildren {
    pub shown: Vec<(BlockId, String)>,
    pub total: usize,
}

impl LinkedRefs {
    pub fn shown(&self) -> usize {
        self.groups.iter().map(|g| g.hits.len()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BlockRefStatus {
    /// No block with that id exists anywhere (FR-4.7).
    Missing,
    /// The block exists as a tombstone (FR-3.7, AC-4.5).
    Deleted { page: PageId, text: String },
    Live {
        page: PageId,
        page_title: String,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RefTarget {
    Page { id: PageId, title: String },
    UnresolvedPage { key: String },
    Block { id: BlockId, status: BlockRefStatus },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutgoingRef {
    pub target: RefTarget,
    pub count: usize,
    /// One source block per target, for navigation.
    pub from_block: BlockId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TitleHit {
    pub page: PageId,
    pub title: String,
    /// Set when the match came through an alias.
    pub alias: Option<String>,
    pub daily: bool,
    pub score: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockHit {
    pub block: BlockId,
    pub page: PageId,
    pub page_title: String,
    pub text: String,
    /// Byte ranges of the matched terms in the plain text the UI draws (SR-1).
    pub anchors: Vec<(usize, usize)>,
}

/// Search results for one page or day (SR-2): the header's name and count, then its blocks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockGroup {
    pub page: PageId,
    pub title: String,
    pub daily: bool,
    pub hits: Vec<BlockHit>,
}

/// One child of a previewed block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviewChild {
    pub block: BlockId,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
}

/// The selected search result, in full (SR-4): its source text, path from the root, state
/// and first few children. Read in the palette's handler, never in `render` (rule 10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlockPreview {
    pub block: BlockId,
    pub page: PageId,
    pub page_title: String,
    pub daily: bool,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
    pub updated_at: i64,
    /// Ancestors, root first. The page title is `page_title`, not part of this list.
    pub path: Vec<(BlockId, String)>,
    pub children: Vec<PreviewChild>,
    pub child_count: usize,
}

/// Where a search may look (SR-3). The chips under the field: All · Journal · Pages · To-dos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum SearchScope {
    #[default]
    All,
    /// Blocks on daily pages.
    Journal,
    /// Blocks on ordinary pages.
    Pages,
    /// To-do blocks, done and not done.
    Todos,
}

impl SearchScope {
    pub const ALL: &'static [SearchScope] = &[
        SearchScope::All,
        SearchScope::Journal,
        SearchScope::Pages,
        SearchScope::Todos,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchScope::All => "All",
            SearchScope::Journal => "Journal",
            SearchScope::Pages => "Pages",
            SearchScope::Todos => "To-dos",
        }
    }

    /// Whether a page title (or alias) may be listed in this scope (SR-3).
    pub fn keeps_page(self, daily: bool) -> bool {
        match self {
            SearchScope::All => true,
            SearchScope::Journal => daily,
            SearchScope::Pages => !daily,
            SearchScope::Todos => false,
        }
    }

    /// The SQL condition on `block b` / `page p` for this scope.
    fn condition(self) -> &'static str {
        match self {
            SearchScope::All => "1",
            SearchScope::Journal => "p.kind='daily'",
            SearchScope::Pages => "p.kind='page'",
            SearchScope::Todos => "b.block_type='todo'",
        }
    }
}

/// How far back a search looks (SR-3's "Any time" menu). Daily hits are dated by their
/// day; other blocks by the local date of their last edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum SearchAge {
    #[default]
    AnyTime,
    Today,
    Week,
    Month,
    Year,
}

impl SearchAge {
    pub const ALL: &'static [SearchAge] = &[
        SearchAge::AnyTime,
        SearchAge::Today,
        SearchAge::Week,
        SearchAge::Month,
        SearchAge::Year,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchAge::AnyTime => "Any time",
            SearchAge::Today => "Today",
            SearchAge::Week => "Past week",
            SearchAge::Month => "Past month",
            SearchAge::Year => "Past year",
        }
    }

    /// First day that still counts, `today - n` for the windows (both ends included).
    pub fn since(self, today: NaiveDate) -> Option<NaiveDate> {
        let back = |n: u64| today.checked_sub_days(chrono::Days::new(n));
        match self {
            SearchAge::AnyTime => None,
            SearchAge::Today => Some(today),
            SearchAge::Week => back(6),
            SearchAge::Month => back(29),
            SearchAge::Year => back(364),
        }
    }

    /// Last day that still counts: today for every window, `None` for Any time. A future
    /// daily page is reachable by navigation, so it must not pass "Today" or "Past week".
    pub fn until(self, today: NaiveDate) -> Option<NaiveDate> {
        match self {
            SearchAge::AnyTime => None,
            _ => Some(today),
        }
    }
}

/// Byte ranges in `plain` matched by `query`'s terms, for search highlighting (SR-1).
///
/// FTS5 picks *which* blocks match; this locates the same prefix terms in the plain text
/// the result row shows. Matching the drawn text rather than the source markup means a
/// range can never point into `[[brackets]]` the renderer drops. Terms start at a word
/// boundary and match case-insensitively, like the index's `unicode61` tokenizer; a term
/// longer than the word it starts in does not match. The range covers the whole word, not
/// just the typed prefix, because the wash marks matched words (SR-1). Diacritics are not
/// folded here.
pub fn match_anchors(plain: &str, query: &str) -> Vec<(usize, usize)> {
    let terms: Vec<Vec<char>> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().collect())
        .collect();
    let chars: Vec<(usize, char)> = plain.char_indices().collect();
    let fold = |a: char, b: char| a.to_lowercase().eq(b.to_lowercase());
    let mut out: Vec<(usize, usize)> = Vec::new();
    for term in &terms {
        for (i, &(start, first)) in chars.iter().enumerate() {
            if i > 0 && chars[i - 1].1.is_alphanumeric() {
                continue; // must start a word, as the tokenizer does
            }
            if !fold(first, term[0]) {
                continue;
            }
            let fits = term
                .iter()
                .enumerate()
                .all(|(k, &c)| chars.get(i + k).is_some_and(|&(_, h)| fold(h, c)));
            if !fits {
                continue;
            }
            let end = {
                let mut k = i + term.len();
                while chars.get(k).is_some_and(|&(_, c)| c.is_alphanumeric()) {
                    k += 1;
                }
                chars.get(k).map_or(plain.len(), |&(off, _)| off)
            };
            out.push((start, end));
        }
    }
    out.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in out {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JournalEntry {
    pub page: PageRow,
    pub preview: Vec<String>,
    /// Live blocks with text: what the sidebar's muted count says (SB-1). A day that only
    /// exists because a link pointed at it has 0 here.
    pub blocks: usize,
}

/// Subsequence fuzzy score of `needle` in `hay` (both normalized); higher is better.
pub fn fuzzy_score(needle: &str, hay: &str) -> Option<i64> {
    if needle.is_empty() {
        return Some(1);
    }
    if hay == needle {
        return Some(10_000);
    }
    if hay.starts_with(needle) {
        return Some(5_000 - hay.len() as i64);
    }
    if let Some(i) = hay.find(needle) {
        let word_start = i == 0 || hay[..i].ends_with(' ');
        return Some(if word_start { 3_000 } else { 2_000 } - i as i64 - hay.len() as i64);
    }
    let mut score = 1_000i64;
    let mut it = hay.char_indices();
    let mut last: Option<usize> = None;
    for nc in needle.chars() {
        loop {
            let (i, hc) = it.next()?;
            if hc == nc {
                if let Some(l) = last {
                    score -= (i - l - 1) as i64;
                }
                last = Some(i);
                break;
            }
        }
    }
    Some(score - hay.len() as i64)
}

/// How many children the search preview lists before "+N more" (SR-4).
pub const PREVIEW_CHILDREN: usize = 6;

/// A raw search row before grouping or truncation: the hit plus the day an age window
/// is measured against.
struct SearchRow {
    hit: BlockHit,
    daily: Option<NaiveDate>,
}

impl Store {
    pub fn page_row(&self, id: &PageId) -> Result<Option<PageRow>> {
        self.read_barrier()?;
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {PAGE_COLS} FROM page WHERE id=?1"),
                [id.as_str()],
                page_row,
            )
            .optional()?)
    }

    /// Resolve a title (or date, or alias) to a live page without creating anything.
    pub fn find_page(&self, title: &str) -> Result<Option<PageId>> {
        self.read_barrier()?;
        crate::resolve_page_key(&self.conn, &normalize_title(title), &self.date_format)
    }

    pub fn block_row(&self, id: &BlockId) -> Result<Option<BlockRow>> {
        self.read_barrier()?;
        Ok(self
            .conn
            .query_row(
                "SELECT id, page_id, parent_id, depth, text, block_type, heading_level, checked,
                        updated_at, deleted_at FROM block WHERE id=?1",
                [id.as_str()],
                |r| {
                    Ok(BlockRow {
                        id: BlockId(r.get(0)?),
                        page_id: PageId(r.get(1)?),
                        parent_id: r.get::<_, Option<String>>(2)?.map(BlockId),
                        depth: r.get(3)?,
                        text: r.get(4)?,
                        kind: BlockKind::from_parts(
                            &r.get::<_, String>(5)?,
                            r.get::<_, Option<i64>>(6)?.map(|l| l as u8),
                        ),
                        checked: r.get(7)?,
                        updated_at: r.get(8)?,
                        deleted_at: r.get(9)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn block_page(&self, id: &BlockId) -> Result<PageId> {
        self.read_barrier()?;
        self.block_row(id)?
            .map(|b| b.page_id)
            .ok_or_else(|| StoreError::NotFound(format!("block {id}")))
    }

    /// Live pages. Normal pages by title; daily pages newest first (FR-1.8).
    pub fn list_pages(&self, include_daily: bool) -> Result<Vec<PageRow>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PAGE_COLS} FROM page WHERE deleted_at IS NULL
             ORDER BY kind='daily', CASE WHEN kind='daily' THEN '' ELSE title_key END,
                      daily_date DESC"
        ))?;
        let rows = stmt
            .query_map([], page_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter(|p| include_daily || !p.is_daily())
            .collect())
    }

    /// First `n` live top-level blocks of a page as plain text (previews, peek cards).
    pub fn page_preview(&self, page: &PageId, n: usize) -> Result<Vec<String>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT text FROM block WHERE page_id=?1 AND parent_id IS NULL AND deleted_at IS NULL
             ORDER BY position LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![page.as_str(), n as i64], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows.iter().map(|t| plain_text(t)).collect())
    }

    /// Pages that exist only because of a `#tag` (SB-2, FR-2.15): no blocks of their own,
    /// and every incoming reference to them is a hashtag (`#tag` / `#[[tag]]`), never a
    /// `[[link]]`. Display-only: the sidebar draws a `#` beside these, there is no tags
    /// subsystem. Ref spans are re-read from the source blocks because `ref` stores the
    /// resolved edge only; this is an index-time query, never paint-time work (rule 3).
    pub fn hashtag_only_pages(&self) -> Result<HashSet<PageId>> {
        self.read_barrier()?;
        self.tag_page_scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        hashtag_only_pages(&self.conn)
    }

    /// The last `limit` journal pages up to `today`, with a 2-line preview (FR-1.3) and
    /// the number of blocks on the day (SB-1).
    pub fn journal(&self, limit: usize) -> Result<Vec<JournalEntry>> {
        self.journal_pages(limit)?
            .into_iter()
            .map(|page| {
                let preview = self
                    .page_preview(&page.id, 2)?
                    .into_iter()
                    .filter(|s| !s.trim().is_empty())
                    .collect();
                let blocks = self.page_block_count(&page.id)?;
                Ok(JournalEntry {
                    page,
                    preview,
                    blocks,
                })
            })
            .collect()
    }

    /// Live blocks with text on a page. An empty block does not count: the sidebar's
    /// count says how much was written, not how many blank rows the outliner holds.
    pub fn page_block_count(&self, page: &PageId) -> Result<usize> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT count(*) FROM block
             WHERE page_id=?1 AND deleted_at IS NULL AND trim(text) <> ''",
        )?;
        let n: i64 = stmt.query_row(params![page.as_str()], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Daily pages up to today with something on them, newest first: the Daily Notes
    /// feed. A day counts once it has a non-empty block or a linked reference (a date
    /// pre-created by a link, FR-1.10); days that were only visited are skipped (FR-1.2).
    pub fn journal_pages(&self, limit: usize) -> Result<Vec<PageRow>> {
        self.read_barrier()?;
        let today = utopia_core::date::iso(self.today());
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {PAGE_COLS} FROM page WHERE kind='daily' AND deleted_at IS NULL
               AND daily_date <= ?1
               AND (EXISTS(SELECT 1 FROM block b WHERE b.page_id=page.id
                             AND b.deleted_at IS NULL AND trim(b.text) <> '')
                    OR EXISTS(SELECT 1 FROM ref r JOIN block b ON b.id=r.src_block_id
                               WHERE r.dst_page_id=page.id AND b.deleted_at IS NULL))
             ORDER BY daily_date DESC LIMIT ?2"
        ))?;
        let pages = stmt
            .query_map(params![today, limit as i64], page_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(pages)
    }

    /// Quick jump (Cmd+K): fuzzy match over live titles and aliases (v1: no bodies).
    pub fn search_titles(&self, query: &str, limit: usize) -> Result<Vec<TitleHit>> {
        self.search_titles_scoped(query, SearchScope::All, limit)
    }

    /// Title search narrowed by the scope chips before the result limit (SR-3): excluded
    /// pages must not crowd eligible matches out of the candidate window.
    pub fn search_titles_scoped(
        &self,
        query: &str,
        scope: SearchScope,
        limit: usize,
    ) -> Result<Vec<TitleHit>> {
        self.read_barrier()?;
        let q = normalize_title(query);
        let mut best: HashMap<PageId, TitleHit> = HashMap::new();
        let mut consider = |hit: TitleHit| {
            let keep = best.get(&hit.page).is_none_or(|b| hit.score > b.score);
            if keep {
                best.insert(hit.page.clone(), hit);
            }
        };
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, title, title_key, kind FROM page WHERE deleted_at IS NULL")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, title, key, kind) = row?;
                if let Some(score) = fuzzy_score(&q, &key) {
                    consider(TitleHit {
                        page: PageId(id),
                        title,
                        alias: None,
                        daily: kind == "daily",
                        score,
                    });
                }
            }
        }
        {
            let mut stmt = self.conn.prepare(
                "SELECT a.page_id, p.title, a.alias, a.alias_key, p.kind FROM alias a
                 JOIN page p ON p.id=a.page_id WHERE p.deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?;
            for row in rows {
                let (id, title, alias, key, kind) = row?;
                if let Some(score) = fuzzy_score(&q, &key) {
                    consider(TitleHit {
                        page: PageId(id),
                        title,
                        alias: Some(alias),
                        daily: kind == "daily",
                        score: score - 1,
                    });
                }
            }
        }
        let mut hits: Vec<TitleHit> = best
            .into_values()
            .filter(|h| scope.keeps_page(h.daily))
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(a.daily.cmp(&b.daily))
                .then(a.title.len().cmp(&b.title.len()))
                .then(a.title.cmp(&b.title))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    /// Block search for `((` autocomplete (FR-4.2): FTS5 prefix match over live bodies.
    pub fn search_blocks(&self, query: &str, limit: usize) -> Result<Vec<BlockHit>> {
        self.search_blocks_filtered(query, SearchScope::All, SearchAge::AnyTime, limit)
    }

    /// Block search through the scope and age chips (SR-3). The scope narrows the SQL
    /// query; the age window is applied to the hit's day (a daily page's date, or the
    /// local date of the block's last edit) before the limit cuts the list.
    pub fn search_blocks_filtered(
        &self,
        query: &str,
        scope: SearchScope,
        age: SearchAge,
        limit: usize,
    ) -> Result<Vec<BlockHit>> {
        Ok(self
            .search_raw(query, scope, age, limit)?
            .into_iter()
            .map(|r| r.hit)
            .collect())
    }

    fn search_raw(
        &self,
        query: &str,
        scope: SearchScope,
        age: SearchAge,
        limit: usize,
    ) -> Result<Vec<SearchRow>> {
        self.read_barrier()?;
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect();
        let map = |r: &Row<'_>| {
            let text: String = r.get(3)?;
            Ok(SearchRow {
                hit: BlockHit {
                    block: BlockId(r.get(0)?),
                    page: PageId(r.get(1)?),
                    page_title: r.get(2)?,
                    anchors: match_anchors(&plain_text(&text), query),
                    text,
                },
                daily: r
                    .get::<_, Option<String>>(4)?
                    .and_then(|d| utopia_core::date::parse_iso(&d)),
            })
        };
        // The age window is part of the query, not a filter applied after the limit: a
        // handful of old matches must never push an eligible one out of the candidates.
        // Daily hits are dated by their day, other blocks by the local date of their last
        // edit (the same day `RefQuery::since` uses).
        let today = self.today();
        let since = age.since(today).map(|d| d.to_string());
        let until = age.until(today).map(|d| d.to_string());
        let fetch = (limit.max(1) * 10) as i64;
        let window = "AND (:since IS NULL
                        OR COALESCE(p.daily_date,
                             date(b.updated_at / 1000, 'unixepoch', 'localtime')) >= :since)
                     AND (:until IS NULL
                        OR COALESCE(p.daily_date,
                             date(b.updated_at / 1000, 'unixepoch', 'localtime')) <= :until)";
        let mut rows = if terms.is_empty() {
            let sql = format!(
                "SELECT b.id, b.page_id, p.title, b.text, p.daily_date
                 FROM block b JOIN page p ON p.id=b.page_id
                 WHERE b.deleted_at IS NULL AND p.deleted_at IS NULL AND b.text <> ''
                   AND {} {window} ORDER BY b.updated_at DESC LIMIT :limit",
                scope.condition()
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt
                .query_map(
                    rusqlite::named_params! {
                        ":since": since,
                        ":until": until,
                        ":limit": fetch,
                    },
                    map,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        } else {
            // Newest matches first, straight off the FTS doclist (no scoring of every
            // match, which is what made common terms slow); then prefer blocks containing
            // the whole query as typed.
            let sql = format!(
                "SELECT f.block_id, b.page_id, p.title, b.text, p.daily_date
                 FROM block_fts f
                 JOIN block b ON b.rowid=f.rowid JOIN page p ON p.id=b.page_id
                 WHERE block_fts MATCH :q AND b.deleted_at IS NULL AND p.deleted_at IS NULL
                   AND {} {window} ORDER BY f.rowid DESC LIMIT :limit",
                scope.condition()
            );
            let mut stmt = self.conn.prepare_cached(&sql)?;
            let mut rows = stmt
                .query_map(
                    rusqlite::named_params! {
                        ":q": terms.join(" "),
                        ":since": since,
                        ":until": until,
                        ":limit": fetch,
                    },
                    map,
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let needle = query.trim().to_lowercase();
            rows.sort_by_key(|h| !h.hit.text.to_lowercase().contains(&needle));
            rows
        };
        rows.truncate(limit);
        Ok(rows)
    }

    /// Block search grouped under the page or day each hit lives on (SR-2, SR-3). Groups
    /// keep the order of their best hit, so the newest page stays first; hits inside a
    /// group keep the flat search order.
    pub fn search_groups(&self, query: &str, limit: usize) -> Result<Vec<BlockGroup>> {
        self.search_groups_filtered(query, SearchScope::All, SearchAge::AnyTime, limit)
    }

    /// [`Store::search_groups`] through the scope and age chips (SR-3).
    pub fn search_groups_filtered(
        &self,
        query: &str,
        scope: SearchScope,
        age: SearchAge,
        limit: usize,
    ) -> Result<Vec<BlockGroup>> {
        let rows = self.search_raw(query, scope, age, limit)?;
        let mut groups: Vec<BlockGroup> = Vec::new();
        let mut index: HashMap<PageId, usize> = HashMap::new();
        for row in rows {
            let hit = row.hit;
            if let Some(&i) = index.get(&hit.page) {
                groups[i].hits.push(hit);
                continue;
            }
            index.insert(hit.page.clone(), groups.len());
            groups.push(BlockGroup {
                page: hit.page.clone(),
                title: hit.page_title.clone(),
                daily: row.daily.is_some(),
                hits: vec![hit],
            });
        }
        Ok(groups)
    }

    /// The block behind a search result, with its path, state and first children (SR-4).
    /// Returns `None` for a tombstone or a block that is not in the index.
    pub fn preview_block(&self, block: &BlockId) -> Result<Option<BlockPreview>> {
        let Some(row) = self.block_row(block)? else {
            return Ok(None);
        };
        let Some(page) = self.page_row(&row.page_id)? else {
            return Ok(None);
        };
        if row.deleted_at.is_some() || page.deleted_at.is_some() {
            return Ok(None);
        }
        let path = match &row.parent_id {
            Some(parent) => self
                .ancestry(&mut HashMap::new(), parent.as_str())?
                .into_iter()
                .map(|(id, text, _)| (BlockId(id), plain_text(&text)))
                .collect(),
            None => Vec::new(),
        };
        let map_child = |r: &Row<'_>| {
            Ok(PreviewChild {
                block: BlockId(r.get(0)?),
                text: r.get(1)?,
                kind: BlockKind::from_parts(
                    &r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?.map(|l| l as u8),
                ),
                checked: r.get(4)?,
            })
        };
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, text, block_type, heading_level, checked FROM block
             WHERE parent_id=?1 AND deleted_at IS NULL ORDER BY position LIMIT ?2",
        )?;
        let children = stmt
            .query_map(params![block.as_str(), PREVIEW_CHILDREN as i64], map_child)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let child_count: i64 = self.conn.query_row(
            "SELECT count(*) FROM block WHERE parent_id=?1 AND deleted_at IS NULL",
            [block.as_str()],
            |r| r.get(0),
        )?;
        let daily = page.is_daily();
        Ok(Some(BlockPreview {
            block: block.clone(),
            page: page.id.clone(),
            page_title: page.title,
            daily,
            text: row.text,
            kind: row.kind,
            checked: row.checked,
            updated_at: row.updated_at,
            path,
            children,
            child_count: child_count as usize,
        }))
    }

    /// Status of a `((uid))` target, including tombstones (§10.3.1).
    pub fn block_ref_status(&self, uid: &str) -> Result<BlockRefStatus> {
        self.read_barrier()?;
        let row = self
            .conn
            .query_row(
                "SELECT b.page_id, b.text, b.deleted_at, p.title FROM block b JOIN page p ON p.id=b.page_id
                 WHERE b.id=?1",
                [uid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(match row {
            None => BlockRefStatus::Missing,
            Some((page, text, Some(_), _)) => BlockRefStatus::Deleted {
                page: PageId(page),
                text,
            },
            Some((page, text, None, title)) => BlockRefStatus::Live {
                page: PageId(page),
                page_title: title,
                text,
            },
        })
    }

    /// Ancestors of `parent` (inclusive) as `(id, text, position)`, root first.
    fn ancestry(
        &self,
        cache: &mut HashMap<String, Vec<(String, String, String)>>,
        parent: &str,
    ) -> Result<Vec<(String, String, String)>> {
        if let Some(v) = cache.get(parent) {
            return Ok(v.clone());
        }
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE anc(id, parent_id, text, position, lvl) AS (
               SELECT id, parent_id, text, position, 0 FROM block WHERE id=?1
               UNION ALL
               SELECT b.id, b.parent_id, b.text, b.position, anc.lvl + 1
               FROM block b JOIN anc ON b.id = anc.parent_id WHERE anc.lvl < 256
             ) SELECT id, text, position FROM anc ORDER BY lvl DESC",
        )?;
        let rows = stmt
            .query_map([parent], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(String, String, String)>>>()?;
        cache.insert(parent.to_string(), rows.clone());
        Ok(rows)
    }

    fn hits_from(&self, rows: Vec<HitRow>) -> Result<Vec<RefHit>> {
        let mut cache = HashMap::new();
        let mut by_block: Vec<RefHit> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for r in rows {
            if let Some(&i) = index.get(&r.block) {
                by_block[i].anchors.push((r.a0 as usize, r.a1 as usize));
                continue;
            }
            // Flat blocks (the common case) need no ancestor query at all.
            let anc = match &r.parent {
                Some(p) => self.ancestry(&mut cache, p)?,
                None => Vec::new(),
            };
            let order = anc
                .iter()
                .map(|(_, _, p)| p.as_str())
                .chain(std::iter::once(r.position.as_str()))
                .collect::<Vec<_>>()
                .join("/");
            index.insert(r.block.clone(), by_block.len());
            by_block.push(RefHit {
                block: BlockId(r.block),
                page: PageId(r.page),
                text: r.text,
                breadcrumb: anc
                    .into_iter()
                    .map(|(id, t, _)| (BlockId(id), plain_text(&t)))
                    .collect(),
                anchors: vec![(r.a0 as usize, r.a1 as usize)],
                updated_at: r.updated_at,
                order,
            });
        }
        Ok(by_block)
    }

    /// Linked References for a page (§7, FR-3.1–3.4, FR-3.10). Includes self-references.
    pub fn linked_references(&self, page: &PageId, q: &RefQuery) -> Result<LinkedRefs> {
        self.read_barrier()?;
        let rows: Vec<HitRow> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT r.src_block_id, r.src_page_id, r.anchor_start, r.anchor_end, b.text, b.updated_at,
                        b.parent_id, b.position
                 FROM ref r JOIN block b ON b.id=r.src_block_id
                 WHERE r.dst_page_id=?1 AND r.dst_kind='page' AND b.deleted_at IS NULL
                 ORDER BY r.src_block_id, r.anchor_start",
            )?;
            let rows = stmt
                .query_map([page.as_str()], hit_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let hits = self.hits_from(rows)?;
        let total = hits.len();
        let mut filters: Vec<(PageId, HashSet<String>)> = Vec::new();
        for f in &q.also {
            let mut stmt = self.conn.prepare_cached(
                "SELECT src_block_id FROM ref WHERE dst_page_id=?1 AND dst_kind='page'",
            )?;
            let set = stmt
                .query_map([f.as_str()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            filters.push((f.clone(), set));
        }
        let needle = q
            .text
            .as_ref()
            .map(|t| t.to_lowercase())
            .filter(|t| !t.is_empty());
        let mut titles: HashMap<PageId, PageRow> = HashMap::new();
        let mut groups: HashMap<PageId, Vec<RefHit>> = HashMap::new();
        for hit in hits {
            if !titles.contains_key(&hit.page) {
                if let Some(row) = self.page_row(&hit.page)? {
                    titles.insert(hit.page.clone(), row);
                }
            }
            let Some(src) = titles.get(&hit.page) else {
                continue;
            };
            let passes_filters = filters.iter().all(|(f, set)| {
                hit.page == *f
                    || set.contains(hit.block.as_str())
                    || hit.breadcrumb.iter().any(|(a, _)| set.contains(a.as_str()))
            });
            if !passes_filters {
                continue;
            }
            if let Some(since) = q.since {
                let day = src.daily_date.or_else(|| {
                    chrono::DateTime::from_timestamp_millis(hit.updated_at)
                        .map(|t| t.with_timezone(&chrono::Local).date_naive())
                });
                if day.is_none_or(|d| d < since) {
                    continue;
                }
            }
            if let Some(n) = &needle {
                let in_text = plain_text(&hit.text).to_lowercase().contains(n);
                let in_title = src.title.to_lowercase().contains(n);
                let in_crumbs = hit
                    .breadcrumb
                    .iter()
                    .any(|(_, t)| t.to_lowercase().contains(n));
                if !(in_text || in_title || in_crumbs) {
                    continue;
                }
            }
            groups.entry(hit.page.clone()).or_default().push(hit);
        }
        let mut out: Vec<RefGroup> = groups
            .into_iter()
            .map(|(p, mut hits)| {
                hits.sort_by(|a, b| a.order.cmp(&b.order));
                RefGroup {
                    page: titles[&p].clone(),
                    hits,
                }
            })
            .collect();
        match q.sort {
            RefSort::Title => out.sort_by(|a, b| {
                b.page
                    .is_daily()
                    .cmp(&a.page.is_daily())
                    .then(b.page.daily_date.cmp(&a.page.daily_date))
                    .then(a.page.title_key.cmp(&b.page.title_key))
            }),
            RefSort::Recency => out.sort_by(|a, b| {
                let ma = a.hits.iter().map(|h| h.updated_at).max();
                let mb = b.hits.iter().map(|h| h.updated_at).max();
                mb.cmp(&ma).then(a.page.title_key.cmp(&b.page.title_key))
            }),
        }
        Ok(LinkedRefs { total, groups: out })
    }

    /// The first `limit` live children of each referencing block, in outline order, and how
    /// many live children it has in total (LR-4). Read from the `block` projection, so a
    /// reference panel can show them without doing the work at paint time (rule 3).
    /// `usize::MAX` means every child (the "+N more" chip was opened).
    pub fn ref_children(
        &self,
        parents: &[BlockId],
        limit: usize,
    ) -> Result<HashMap<BlockId, RefChildren>> {
        self.read_barrier()?;
        let mut out = HashMap::with_capacity(parents.len());
        let mut count = self.conn.prepare_cached(
            "SELECT count(*) FROM block WHERE parent_id=?1 AND deleted_at IS NULL",
        )?;
        let mut rows = self.conn.prepare_cached(
            "SELECT id, text FROM block WHERE parent_id=?1 AND deleted_at IS NULL
             ORDER BY position LIMIT ?2",
        )?;
        // SQLite reads a negative LIMIT as "no limit".
        let sql_limit: i64 = if limit == usize::MAX {
            -1
        } else {
            limit as i64
        };
        for parent in parents {
            let total: i64 = count.query_row([parent.as_str()], |r| r.get(0))?;
            let shown: Vec<(BlockId, String)> = if limit == 0 {
                Vec::new()
            } else {
                rows.query_map(rusqlite::params![parent.as_str(), sql_limit], |r| {
                    Ok((BlockId(r.get(0)?), r.get(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            out.insert(
                parent.clone(),
                RefChildren {
                    shown,
                    total: total as usize,
                },
            );
        }
        Ok(out)
    }

    /// The pages the current hits co-reference, each with the number of hits that selecting
    /// it would keep (FR-3.10, LR-1). Candidates come from the index — the referencing
    /// block, its ancestors and their aliases all resolve to pages there — and the count
    /// uses the very predicate a co-reference filter applies, so a chip's number is exactly
    /// the number of results it produces.
    pub fn co_references(
        &self,
        hits: &[RefHit],
        page: &PageId,
        applied: &[PageId],
    ) -> Result<Vec<CoReference>> {
        self.read_barrier()?;
        let mut candidates: HashSet<String> = HashSet::new();
        {
            let mut refs = self.conn.prepare_cached(
                "SELECT DISTINCT dst_page_id FROM ref
                 WHERE src_block_id=?1 AND dst_kind='page' AND dst_page_id IS NOT NULL",
            )?;
            for h in hits {
                let blocks = std::iter::once(&h.block).chain(h.breadcrumb.iter().map(|(b, _)| b));
                for block in blocks {
                    let rows = refs.query_map([block.as_str()], |r| r.get::<_, String>(0))?;
                    for id in rows {
                        candidates.insert(id?);
                    }
                }
            }
        }
        let mut filter = self.conn.prepare_cached(
            "SELECT src_block_id FROM ref WHERE dst_page_id=?1 AND dst_kind='page'",
        )?;
        let mut out: Vec<(String, CoReference)> = Vec::new();
        for id in candidates {
            let candidate = PageId(id);
            if candidate == *page || applied.contains(&candidate) {
                continue;
            }
            let set: HashSet<String> = filter
                .query_map([candidate.as_str()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            // The `RefQuery::also` predicate: the hit's block, an ancestor, or its page.
            let kept = hits
                .iter()
                .filter(|h| {
                    h.page == candidate
                        || set.contains(h.block.as_str())
                        || h.breadcrumb.iter().any(|(a, _)| set.contains(a.as_str()))
                })
                .count();
            if kept == 0 {
                continue;
            }
            let Some(row) = self.page_row(&candidate)? else {
                continue;
            };
            if row.deleted_at.is_some() {
                continue;
            }
            out.push((
                row.title_key,
                CoReference {
                    page: candidate,
                    title: row.title,
                    hits: kept,
                },
            ));
        }
        out.sort_by(|a, b| b.1.hits.cmp(&a.1.hits).then(a.0.cmp(&b.0)));
        out.truncate(CO_REFERENCE_LIMIT);
        Ok(out.into_iter().map(|(_, c)| c).collect())
    }

    /// Number of live blocks referencing a page.
    pub fn backlink_count(&self, page: &PageId) -> Result<usize> {
        self.read_barrier()?;
        Ok(self.conn.query_row(
            "SELECT count(DISTINCT r.src_block_id) FROM ref r JOIN block b ON b.id=r.src_block_id
             WHERE r.dst_page_id=?1 AND r.dst_kind='page' AND b.deleted_at IS NULL",
            [page.as_str()],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Backlink counts for every page that has one, in a single grouped query. The
    /// sidebar's PAGES list shows a count per row (DESIGN.md §3.2); asking
    /// [`Store::backlink_count`] once per page would be one query per row on every index
    /// update.
    pub fn page_backlink_counts(&self) -> Result<HashMap<PageId, usize>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.dst_page_id, count(DISTINCT r.src_block_id)
             FROM ref r JOIN block b ON b.id=r.src_block_id
             WHERE r.dst_kind='page' AND r.dst_page_id IS NOT NULL AND b.deleted_at IS NULL
             GROUP BY r.dst_page_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                PageId(row.get::<_, String>(0)?),
                row.get::<_, i64>(1)? as usize,
            ))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (page, count) = row?;
            out.insert(page, count);
        }
        Ok(out)
    }

    /// "Referenced by (N)" for a block (FR-3.5).
    pub fn block_backlinks(&self, block: &BlockId) -> Result<Vec<RefHit>> {
        self.read_barrier()?;
        let rows: Vec<HitRow> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT r.src_block_id, r.src_page_id, r.anchor_start, r.anchor_end, b.text, b.updated_at,
                        b.parent_id, b.position
                 FROM ref r JOIN block b ON b.id=r.src_block_id
                 WHERE r.dst_block_id=?1 AND r.dst_kind='block' AND b.deleted_at IS NULL
                 ORDER BY r.src_page_id, r.src_block_id, r.anchor_start",
            )?;
            let rows = stmt
                .query_map([block.as_str()], hit_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        self.hits_from(rows)
    }

    /// Inspector "Outgoing" (FR-4.6): edges from a page's live blocks, or from one block.
    pub fn outgoing_refs(
        &self,
        page: &PageId,
        block: Option<&BlockId>,
    ) -> Result<Vec<OutgoingRef>> {
        self.read_barrier()?;
        let rows: Vec<OutRow> = {
            let (sql, arg) = match block {
                Some(b) => (
                    "SELECT r.src_block_id, r.dst_kind, r.dst_page_id, r.dst_block_id, r.dst_title_key
                     FROM ref r WHERE r.src_block_id=?1 ORDER BY r.anchor_start",
                    b.as_str(),
                ),
                None => (
                    "SELECT r.src_block_id, r.dst_kind, r.dst_page_id, r.dst_block_id, r.dst_title_key
                     FROM ref r JOIN block b ON b.id=r.src_block_id
                     WHERE r.src_page_id=?1 AND b.deleted_at IS NULL ORDER BY b.position, r.anchor_start",
                    page.as_str(),
                ),
            };
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt
                .query_map([arg], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let mut out: Vec<OutgoingRef> = Vec::new();
        for (src, kind, dst_page, dst_block, key) in rows {
            let target = if kind == "block" {
                let id = dst_block.unwrap_or_default();
                RefTarget::Block {
                    status: self.block_ref_status(&id)?,
                    id: BlockId(id),
                }
            } else {
                match dst_page {
                    Some(p) => {
                        let title = self
                            .page_row(&PageId(p.clone()))?
                            .map(|r| r.title)
                            .unwrap_or_default();
                        RefTarget::Page {
                            id: PageId(p),
                            title,
                        }
                    }
                    None => RefTarget::UnresolvedPage {
                        key: key.unwrap_or_default(),
                    },
                }
            };
            match out.iter_mut().find(|o| o.target == target) {
                Some(o) => o.count += 1,
                None => out.push(OutgoingRef {
                    target,
                    count: 1,
                    from_block: BlockId(src),
                }),
            }
        }
        Ok(out)
    }

    /// Inspector "Unresolved" (FR-4.6, FR-4.14): dangling page intents and missing
    /// block refs among a page's outgoing edges.
    pub fn unresolved_refs(&self, page: &PageId) -> Result<Vec<OutgoingRef>> {
        self.read_barrier()?;
        Ok(self
            .outgoing_refs(page, None)?
            .into_iter()
            .filter(|o| {
                matches!(
                    o.target,
                    RefTarget::UnresolvedPage { .. }
                        | RefTarget::Block {
                            status: BlockRefStatus::Missing,
                            ..
                        }
                )
            })
            .collect())
    }

    /// Aliases of a page with their display text (AC-3.3 tooltips).
    pub fn aliases(&self, page: &PageId) -> Result<Vec<String>> {
        self.read_barrier()?;
        let mut stmt = self
            .conn
            .prepare_cached("SELECT alias FROM alias WHERE page_id=?1 ORDER BY alias_key")?;
        let rows = stmt
            .query_map([page.as_str()], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    }
}

/// Whether `page` is a hashtag-only page, classified on its own (SB-2). `None` means the
/// page cannot be one at all (it has blocks, is not a page, or is deleted); `Some(true)` is
/// a tag page. Used by the writer to update only the entries a commit can change.
pub(crate) fn classify_tag_page(conn: &Connection, page: &PageId) -> Result<Option<bool>> {
    let title_key: Option<String> = conn
        .query_row(
            "SELECT p.title_key FROM page p
             WHERE p.id=?1 AND p.deleted_at IS NULL AND p.kind = 'page'
               AND NOT EXISTS (SELECT 1 FROM block b WHERE b.page_id = p.id
                                 AND b.deleted_at IS NULL AND trim(b.text) <> '')",
            [page.as_str()],
            |r| r.get(0),
        )
        .optional()?;
    let Some(title_key) = title_key else {
        return Ok(None);
    };
    let mut keys = HashSet::from([title_key]);
    let mut aliases = conn.prepare_cached("SELECT alias_key FROM alias WHERE page_id=?1")?;
    for row in aliases.query_map([page.as_str()], |r| r.get::<_, String>(0))? {
        keys.insert(row?);
    }
    let mut texts = conn.prepare_cached(
        "SELECT b.text FROM ref r JOIN block b ON b.id = r.src_block_id
         WHERE r.dst_page_id=?1 AND b.deleted_at IS NULL",
    )?;
    let mut any = false;
    let mut all_tags = true;
    for row in texts.query_map([page.as_str()], |r| r.get::<_, String>(0))? {
        let text = row?;
        for r in extract_refs(&text) {
            if r.is_page_like() && keys.contains(&normalize_title(&r.target)) {
                any = true;
                all_tags &= r.kind == RefKind::Tag;
            }
        }
    }
    Ok(Some(any && all_tags))
}

/// The classification itself, over any connection: the writer publishes it after every
/// commit, so the UI thread never runs it (SB-2, §15.1). The body re-reads reference spans
/// from the source blocks because `ref` stores the resolved edge only.
pub(crate) fn hashtag_only_pages(conn: &Connection) -> Result<HashSet<PageId>> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.title_key FROM page p
         WHERE p.deleted_at IS NULL AND p.kind = 'page'
           AND NOT EXISTS (SELECT 1 FROM block b WHERE b.page_id = p.id
                             AND b.deleted_at IS NULL AND trim(b.text) <> '')",
    )?;
    let candidates = stmt
        .query_map([], |r| {
            Ok((PageId(r.get::<_, String>(0)?), r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }
    // The source text of every reference into a blockless page, in one query rather
    // than one per page: the sidebar re-reads this whenever the index moves.
    let mut sources = conn.prepare(
        "SELECT r.dst_page_id, b.text FROM ref r
         JOIN block b ON b.id = r.src_block_id
         JOIN page p ON p.id = r.dst_page_id
         WHERE b.deleted_at IS NULL AND p.deleted_at IS NULL AND p.kind = 'page'
           AND NOT EXISTS (SELECT 1 FROM block own WHERE own.page_id = p.id
                             AND own.deleted_at IS NULL AND trim(own.text) <> '')",
    )?;
    let mut texts: HashMap<PageId, Vec<String>> = HashMap::new();
    for row in sources.query_map([], |r| {
        Ok((PageId(r.get::<_, String>(0)?), r.get::<_, String>(1)?))
    })? {
        let (page, text) = row?;
        texts.entry(page).or_default().push(text);
    }
    // A reference through an alias reaches the page under the alias's key.
    let mut aliases = conn.prepare("SELECT page_id, alias_key FROM alias")?;
    let mut alias_keys: HashMap<PageId, HashSet<String>> = HashMap::new();
    for row in aliases.query_map([], |r| {
        Ok((PageId(r.get::<_, String>(0)?), r.get::<_, String>(1)?))
    })? {
        let (page, key) = row?;
        alias_keys.entry(page).or_default().insert(key);
    }
    let mut out = HashSet::new();
    for (page, title_key) in candidates {
        let mut keys = HashSet::from([title_key]);
        if let Some(aliases) = alias_keys.get(&page) {
            keys.extend(aliases.iter().cloned());
        }
        let mut any = false;
        let mut all_tags = true;
        for text in texts.get(&page).map(Vec::as_slice).unwrap_or_default() {
            for r in extract_refs(text) {
                if r.is_page_like() && keys.contains(&normalize_title(&r.target)) {
                    any = true;
                    all_tags &= r.kind == RefKind::Tag;
                }
            }
        }
        if any && all_tags {
            out.insert(page);
        }
    }
    Ok(out)
}
