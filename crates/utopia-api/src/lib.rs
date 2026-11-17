//! Local automation API (P2; the row "In-app automation API" in §4.1 and Appendix A).
//!
//! The server only accepts loopback connections, requires a bearer token, and refuses
//! browser requests (any `Origin` header, or a `Host` that is not loopback — DNS
//! rebinding), so a web page cannot drive the graph. It never touches the store itself:
//! each request becomes an [`ApiCall`] that the owner of the [`Store`] (the app's main
//! thread, or `utopia serve-api`) runs with [`handle`], so the API sees exactly the
//! state the UI does and every write goes through the normal command path.
//!
//! Routes (JSON in and out):
//!
//! | Method | Path | Body / query |
//! |---|---|---|
//! | GET | `/v1/pages` | `?daily=true` to include journal pages |
//! | GET | `/v1/pages/{title}` | the page and its outline |
//! | GET | `/v1/pages/{title}/backlinks` | Linked References |
//! | POST | `/v1/pages/{title}/blocks` | `{"text", "parent"?, "create"?}` |
//! | GET | `/v1/blocks/{id}` | one block |
//! | PATCH | `/v1/blocks/{id}` | `{"text"}` |
//! | GET | `/v1/journal/{date}` | `today` or `YYYY-MM-DD` |
//! | GET | `/v1/search` | `?q=&limit=` |
//! | POST | `/v1/query` | `{"query", "limit"?}` — query-block syntax |
//! | POST | `/v1/capture` | `{"text"}` — appended to today's page |
//! | POST | `/v1/op` | one [`Op`] as JSON (`{"op": "search", "q": …}`), at the graph's access level |
//!
//! `/v1/op` is the bridge for the MCP server (PRD §13.8, docs/mcp-design.md §2.4). It is the
//! only route the assistant access level ([`access::Level`], MC-1, MC-2) controls: when
//! `!level.permits(op.kind())` it answers 403 with a text for the agent to pass on, and the
//! op is never queued. A new server is at [`access::Level::Off`]; the owner sets the level
//! from the graph's access file (`<config>/mcp/<graph_key>.json`, [`access::access_file`])
//! with [`ApiServer::set_level`]. A body that is not an [`Op`] answers 400.
//!
//! [`Op::UpdateBlock`] also takes `kind`, `checked` and `if_rev` for the MCP bridge (PRD
//! §13.8 MC-8, MC-10); the REST route above keeps sending `text` and nothing else, so its
//! answer does not change.
//!
//! The owner writes `{"url", "token"}` to `<config>/api/<graph_key>.json`
//! ([`discovery_file`], mode 0600), one file for each graph. [`Client`] reads it and calls
//! the owner from another process (`utopia capture`, decision D10).
//!
//! A request becomes a [`ApiCall`] with one explicit state ([`CallState`]): `Pending` until
//! the store's owner claims it (`Running` → `Done`) or the request gives up (`Expired`,
//! terminal). Exactly one side wins that race, so a call that answered 504 can never run
//! afterwards and a retry cannot write twice. The queue is bounded ([`Limits::queue`]), so a
//! stalled owner answers 429 instead of accumulating work; see
//! `docs/state-machines/api-call-inbox.md`.
//!
//! Answers for a call that produced no result, each of which says what happened to the store:
//!
//! | Status | Cause | Store |
//! |---|---|---|
//! | 429 | the owner's queue is full, so the call was never queued | unchanged; retry later |
//! | 503 | the app is not running: the queue was gone, or it stopped before running the call | unchanged; retry later |
//! | 504 | the request gave up waiting: "nothing was written", or "it may be applied" when the owner had already started the call | as the message says |
//!
//! An [`ApiError`] status (400, 404, 409, 500) comes from the operation itself. A 409 is
//! `Op::UpdateBlock`'s `if_rev` refusing to overwrite a block that changed after it was read.

use std::future::Future as _;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Bytes,
    extract::{Path as UrlPath, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use utopia_core::{outline, title::normalize_title, BlockId, BlockKind, PageId};
use utopia_store::{resolve_page_key, RefQuery, Store, StoreError};

pub mod access;
mod client;
mod url;
pub use access::{Access, Level};
pub use client::{discovery_file, read_discovery, Client, ClientError, Discovery};
pub use url::{parse_url, UrlAction};

/// One API operation. Also the JSON request plugins send (`{"op": "search", "q": …}`), and
/// the body of `POST /v1/op`. It serializes to the same shape it reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    ListPages {
        #[serde(default)]
        daily: bool,
    },
    GetPage {
        title: String,
    },
    Backlinks {
        title: String,
    },
    AppendBlock {
        title: String,
        text: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        create: bool,
    },
    /// Append an indented Markdown outline in one transaction and one undo step (PRD §13.8
    /// MC-8, MC-9). `title` is a page title, an alias or a date; `create` makes the page
    /// (a date makes its daily page, FR-1.2) when nothing resolves. The id markers of the
    /// outline format are removed from `markdown` first, so an outline read from a page can
    /// be sent back as it was shown.
    AppendOutline {
        title: String,
        markdown: String,
        #[serde(default)]
        parent: Option<String>,
        #[serde(default)]
        create: bool,
    },
    GetBlock {
        id: String,
        /// Also return the block's live children, nested as `children` (the `read_block`
        /// tool, PRD §13.8 MC-7). The default keeps the result every existing caller gets.
        #[serde(default)]
        subtree: bool,
    },
    /// Change one block (PRD §13.8 MC-8, MC-9, MC-10; FR-2.13): its text, its kind and
    /// whether a to-do is done, in one call and one undo step. Every part is optional, so
    /// the JSON plugins and `PATCH /v1/blocks/{id}` send today — `{"op": "update_block",
    /// "id", "text"}` — keeps its meaning, and an absent part leaves that part of the block
    /// alone.
    UpdateBlock {
        id: String,
        /// The block's new text. Absent leaves the text alone; the empty string still
        /// empties the block, as it does through the REST route (the MCP tool refuses an
        /// empty one, because wiping a block is never what an agent's turn is for).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// The new kind: `bullet`, `numbered`, `todo`, `quote`, `code`, `heading`, or
        /// `h1`/`h2`/`h3` to pick a heading's level. `heading` keeps the level of a block
        /// that already is a heading and makes any other block a heading 1. An unknown
        /// name is refused, never read as a bullet.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// Whether a to-do is done (FR-2.13). Only a to-do has this state, so the block must
        /// be one already or become one in the same call.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checked: Option<bool>,
        /// The short hash [`block_rev`] gives the text the caller read, as `read_block`
        /// returns it. If the block's text is not that, the call fails and writes nothing
        /// (MC-10, §2.3.7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        if_rev: Option<String>,
    },
    Journal {
        date: String,
    },
    Search {
        q: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Query {
        query: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    Capture {
        text: String,
    },
}

fn default_limit() -> usize {
    20
}

/// How an outline line names the block it came from, e.g. `{id:01J8Z3K9V2}` — the marker
/// `read_page` and `read_block` write at the end of each line (`docs/mcp-design.md` §4,
/// `utopia-mcp`'s outline format). It is deliberately not Utopia markup: `((id))` is a block
/// reference and `[[id]]` a page link, so a line an agent sends back would make one. A
/// single `{` is literal text, so the marker is inert even where it survives.
pub const ID_MARKER_PREFIX: &str = "{id:";
pub const ID_MARKER_SUFFIX: &str = "}";

/// The most blocks one `Op::AppendOutline` call may add (PRD §13.8 MC-8). A page of notes is
/// a few hundred lines and the AC-2.5 paste budget is measured on 500; this is four of
/// those, one commit's worth of work. Above it the call fails and writes nothing.
pub const MAX_OUTLINE_BLOCKS: usize = 2000;

/// `markdown` with the id marker of [`ID_MARKER_PREFIX`] taken off the end of each line, so
/// the outline format round-trips: `read_page` → `append_blocks` lands the text the agent
/// read, not the marker text (PRD §13.8 MC-8, design §4). Everything else is left alone;
/// a `{id:…}` in the middle of a line is text, not a marker.
pub fn strip_id_markers(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    for (i, line) in markdown.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(strip_id_marker(line));
    }
    out
}

fn strip_id_marker(line: &str) -> &str {
    let trimmed = line.trim_end_matches([' ', '\t', '\r']);
    let Some(at) = trimmed.rfind(ID_MARKER_PREFIX) else {
        return line;
    };
    let Some(id) = trimmed[at + ID_MARKER_PREFIX.len()..].strip_suffix(ID_MARKER_SUFFIX) else {
        return line;
    };
    if id.is_empty() || id.contains(|c: char| c.is_whitespace() || c == '{' || c == '}') {
        return line;
    }
    // The marker sits after the text, so the space that separated them goes with it.
    line[..at].trim_end()
}

/// What an operation does to the graph. The plugin permission check reads this one
/// classification and [`handle`] is the only implementation, so a new op cannot run with
/// read permission by being forgotten in a second list
/// (`docs/state-machines/plugin-run-lifecycle.md`, issue 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// Reads only; [`handle`] commits nothing.
    Read,
    /// Commits through the store and names the pages it touched in [`Outcome::changed`].
    Write,
}

impl Op {
    /// Exhaustive on purpose: adding an [`Op`] variant does not compile until it is
    /// classified here, and `ops_classify_reads_and_writes_consistently` checks the
    /// classification against what [`handle`] actually does.
    pub const fn kind(&self) -> OpKind {
        match self {
            Op::ListPages { .. }
            | Op::GetPage { .. }
            | Op::Backlinks { .. }
            | Op::GetBlock { .. }
            | Op::Journal { .. }
            | Op::Search { .. }
            | Op::Query { .. } => OpKind::Read,
            Op::AppendBlock { .. }
            | Op::AppendOutline { .. }
            | Op::UpdateBlock { .. }
            | Op::Capture { .. } => OpKind::Write,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        let status = match &e {
            StoreError::NotFound(_) => 404,
            StoreError::Invalid(_) | StoreError::Domain(_) => 400,
            _ => 500,
        };
        Self::new(status, e.to_string())
    }
}

/// The result of an operation: the JSON body and the pages a write touched (so the UI
/// can refresh them).
#[derive(Debug, Clone)]
pub struct Outcome {
    pub body: Value,
    pub changed: Vec<PageId>,
}

fn read(body: Value) -> Result<Outcome, ApiError> {
    Ok(Outcome {
        body,
        changed: Vec::new(),
    })
}

/// The page a title names, through the same resolver as `[[links]]` (dates, titles,
/// aliases).
fn page_by_title(store: &Store, title: &str) -> Result<PageId, ApiError> {
    resolve_page_key(store.conn(), &normalize_title(title), store.date_format())?
        .ok_or_else(|| ApiError::new(404, format!("no page titled {title:?}")))
}

fn block_json(
    store: &mut Store,
    page: &PageId,
    id: &BlockId,
    subtree: bool,
) -> Result<Value, ApiError> {
    let st = store.page_state(page)?;
    let b = st
        .block(id)
        .filter(|_| st.is_live(id))
        .ok_or_else(|| ApiError::new(404, format!("no block {id}")))?;
    let mut body = json!({
        "id": id.as_str(),
        "page": page.as_str(),
        "text": b.text,
        "kind": b.kind.type_str(),
        "heading": b.kind.heading_level(),
        "checked": b.checked,
    });
    if subtree {
        body["children"] = Value::Array(
            st.live_children(Some(id))
                .iter()
                .map(|c| block_node(st, c))
                .collect(),
        );
    }
    Ok(body)
}

/// One block and its live descendants, without the owning page (which the caller already
/// knows: a subtree never crosses a page).
fn block_node(st: &utopia_core::tree::PageState, id: &BlockId) -> Value {
    let b = st.block(id).expect("live row exists");
    json!({
        "id": id.as_str(),
        "text": b.text,
        "kind": b.kind.type_str(),
        "heading": b.kind.heading_level(),
        "checked": b.checked,
        "children": st
            .live_children(Some(id))
            .iter()
            .map(|c| block_node(st, c))
            .collect::<Vec<Value>>(),
    })
}

/// The short hash of a block's text that `read_block` returns as `rev` and that
/// `update_block` checks as `if_rev` (PRD §13.8 MC-10, §2.3.7). One function, so the two
/// ends of the check cannot disagree. 12 hex digits: enough to notice an edit, short
/// enough to read.
pub fn block_rev(text: &str) -> String {
    utopia_store::sha256_hex(text.as_bytes())[..12].to_string()
}

/// The `kind` argument of [`Op::UpdateBlock`] as a [`BlockKind`]. The names are the ones
/// [`block_json`] writes for `kind`, plus `h1`/`h2`/`h3` for the `heading` field it gives
/// beside a heading, and they are matched without case. An unknown name is refused instead
/// of read as a bullet: a write that changes a block to something other than what it was
/// asked to change is worse than no write at all (rule 5).
fn parse_kind(kind: &str, current: BlockKind) -> Result<BlockKind, ApiError> {
    Ok(match kind.trim().to_ascii_lowercase().as_str() {
        "bullet" => BlockKind::Bullet,
        "numbered" => BlockKind::Numbered,
        "todo" => BlockKind::Todo,
        "quote" => BlockKind::Quote,
        "code" => BlockKind::Code,
        "heading" => BlockKind::Heading(match current {
            BlockKind::Heading(level) => level,
            _ => 1,
        }),
        "h1" => BlockKind::Heading(1),
        "h2" => BlockKind::Heading(2),
        "h3" => BlockKind::Heading(3),
        _ => {
            return Err(ApiError::new(
                400,
                format!(
                    "unknown block kind {kind:?}: use bullet, numbered, todo, quote, code, \
                     heading, h1, h2 or h3"
                ),
            ))
        }
    })
}

fn page_json(store: &mut Store, page: &PageId) -> Result<Value, ApiError> {
    let fmt = store.date_format().clone();
    let st = store.page_state(page)?.clone();
    if st.meta.deleted_at.is_some() {
        return Err(ApiError::new(404, "page is in the trash"));
    }
    // Rebuild the tree from the preorder walk: each row's parent is the nearest earlier
    // row one level up.
    fn node(st: &utopia_core::tree::PageState, id: &BlockId) -> Value {
        let b = st.block(id).expect("live row exists");
        json!({
            "id": id.as_str(),
            "text": b.text,
            "kind": b.kind.type_str(),
            "heading": b.kind.heading_level(),
            "checked": b.checked,
            "children": [],
        })
    }
    let mut stack: Vec<Value> = Vec::new();
    let mut roots = Vec::new();
    let rows = st.live_preorder();
    let close = |stack: &mut Vec<Value>, roots: &mut Vec<Value>, depth: usize| {
        while stack.len() > depth {
            let done = stack.pop().expect("non-empty");
            match stack.last_mut() {
                Some(parent) => parent["children"]
                    .as_array_mut()
                    .expect("children array")
                    .push(done),
                None => roots.push(done),
            }
        }
    };
    for (id, depth) in rows {
        close(&mut stack, &mut roots, depth);
        stack.push(node(&st, &id));
    }
    close(&mut stack, &mut roots, 0);
    Ok(json!({
        "id": page.as_str(),
        "title": utopia_store::display_title(&st, &fmt),
        "kind": if st.meta.daily_date.is_some() { "daily" } else { "page" },
        "date": st.meta.daily_date.map(|d| d.to_string()),
        "aliases": st.meta.aliases,
        "blocks": roots,
    }))
}

fn parse_date(store: &Store, s: &str) -> Result<NaiveDate, ApiError> {
    if s == "today" {
        return Ok(store.today());
    }
    utopia_core::date::parse_iso(s)
        .or_else(|| store.date_format().parse(s))
        .ok_or_else(|| ApiError::new(400, format!("not a date: {s:?}")))
}

/// Run one operation against the store. Writes commit before returning and name the pages
/// they touched in [`Outcome::changed`]; [`Op::kind`] says which ops those are.
pub fn handle(store: &mut Store, op: Op) -> Result<Outcome, ApiError> {
    match op {
        Op::ListPages { daily } => {
            let pages = store.list_pages(daily)?;
            read(json!({ "pages": pages }))
        }
        Op::GetPage { title } => {
            let page = page_by_title(store, &title)?;
            read(page_json(store, &page)?)
        }
        Op::Backlinks { title } => {
            let page = page_by_title(store, &title)?;
            let refs = store.linked_references(&page, &RefQuery::default())?;
            read(serde_json::to_value(refs).map_err(|e| ApiError::new(500, e.to_string()))?)
        }
        Op::Journal { date } => {
            let d = parse_date(store, &date)?;
            let key = normalize_title(&store.date_format().format(d));
            let page = resolve_page_key(store.conn(), &key, store.date_format())?
                .ok_or_else(|| ApiError::new(404, format!("no journal page for {d}")))?;
            read(page_json(store, &page)?)
        }
        Op::GetBlock { id, subtree } => {
            let id = BlockId(id);
            let page = store
                .block_row(&id)?
                .ok_or_else(|| ApiError::new(404, format!("no block {id}")))?
                .page_id;
            read(block_json(store, &page, &id, subtree)?)
        }
        Op::Search { q, limit } => {
            let titles = store.search_titles(&q, limit)?;
            let blocks = store.search_blocks(&q, limit)?;
            read(json!({ "pages": titles, "blocks": blocks }))
        }
        Op::Query { query, limit } => {
            let expr = utopia_core::query::parse(&query).map_err(|e| ApiError::new(400, e))?;
            let r = store.run_query(&expr, limit)?;
            read(serde_json::to_value(r).map_err(|e| ApiError::new(500, e.to_string()))?)
        }
        Op::Capture { text } => {
            if text.trim().is_empty() {
                return Err(ApiError::new(400, "text is empty"));
            }
            let (page, block) = store.capture(&text)?;
            store.flush()?;
            Ok(Outcome {
                body: block_json(store, &page, &block, false)?,
                changed: vec![page],
            })
        }
        Op::UpdateBlock {
            id,
            text,
            kind,
            checked,
            if_rev,
        } => {
            let id = BlockId(id);
            // Rule 2: the block's row is read to learn its page, and in the app the store is
            // debounced, so an index read does not wait for the writer. Without this the agent's
            // `append_blocks` and then `update_block` on an id it was just given answers "no
            // block" while the append is still being committed.
            store.sync_index()?;
            let page = store
                .block_row(&id)?
                .ok_or_else(|| ApiError::new(404, format!("no block {id}")))?
                .page_id;
            let st = store.page_state(&page)?;
            let Some(block) = st.block(&id).filter(|_| st.is_live(&id)) else {
                return Err(ApiError::new(404, format!("no block {id}")));
            };
            let (text_now, kind_now) = (block.text.clone(), block.kind);
            // MC-10: the check reads the block before the first write, so a rev that does not
            // agree leaves the graph exactly as it was.
            if let Some(want) = &if_rev {
                let have = block_rev(&text_now);
                // Case and stray whitespace are forgiven: a model that pasted the rev with
                // its own spelling still means the text it read.
                if want.trim().to_ascii_lowercase() != have {
                    return Err(ApiError::new(
                        409,
                        format!(
                            "the block changed since it was read: its rev is {have}, not \
                             {want}. Read the block again with read_block, then send the \
                             text and rev it gives you. Nothing was written."
                        ),
                    ));
                }
            }
            let kind = match &kind {
                Some(k) => Some(parse_kind(k, kind_now)?),
                None => None,
            };
            if text.is_none() && kind.is_none() && checked.is_none() {
                return Err(ApiError::new(
                    400,
                    "nothing to change: send `text`, `kind` or `checked`",
                ));
            }
            let after = kind.unwrap_or(kind_now);
            if checked.is_some() && after != BlockKind::Todo {
                return Err(ApiError::new(
                    400,
                    "`checked` applies to a to-do: send kind \"todo\" in the same call to \
                     make the block one",
                ));
            }
            // One command group for the text, the kind and `checked` (MC-9), so one undo
            // takes the whole call off. Only new text can name a link that a page must be
            // made for, and `update_block` already shaped that text the way `commit_block`
            // would: committing a kind or `checked` change would shape text the agent never
            // sent (the owner's own, still uncommitted and not NFC or not healed yet) in a
            // second, coalescing group, which is not one undo step and can join the group
            // the owner is typing into. A kind change alone leaves links alone, as the
            // editor's own kind change does.
            store.update_block(&page, &id, text.as_deref(), kind, checked)?;
            if text.is_some() {
                store.commit_block(&page, &id)?;
            }
            store.flush()?;
            Ok(Outcome {
                body: block_json(store, &page, &id, false)?,
                changed: vec![page],
            })
        }
        Op::AppendOutline {
            title,
            markdown,
            parent,
            create,
        } => {
            // Design §4: the outline format's id markers come off first, so an outline an
            // agent read can be sent straight back.
            let markdown = strip_id_markers(&markdown);
            let items = utopia_core::markdown::parse_outline(&markdown);
            // MC-8: the limits are checked before anything is written, so a refused call
            // does not even leave a page behind.
            if items.is_empty() {
                return Err(ApiError::new(400, "the outline has no blocks"));
            }
            if items.len() > MAX_OUTLINE_BLOCKS {
                return Err(ApiError::new(
                    400,
                    format!(
                        "the outline has {} blocks; at most {MAX_OUTLINE_BLOCKS} can be \
                         appended in one call",
                        items.len()
                    ),
                ));
            }
            let too_deep = |base: usize| -> Option<ApiError> {
                utopia_core::outline::too_deep(&items, base).then(|| {
                    ApiError::new(
                        400,
                        format!(
                            "an outline can be at most {} levels deep (FR-2.9); nothing was \
                             written",
                            utopia_core::outline::MAX_DROP_DEPTH
                        ),
                    )
                })
            };
            // A title that names a date resolves to that day, and `create_page` makes it a
            // daily page on this first write (FR-1.2). The check reads the index (rule 2).
            store.sync_index()?;
            let parent = parent.map(BlockId);
            let page = match page_by_title(store, &title) {
                Ok(p) => p,
                // `parent` is a block id, and a page that does not exist has no blocks: the
                // call could only fail after creating the page, so it fails without one.
                Err(e) if e.status == 404 && create && parent.is_none() => {
                    // A page that does not exist yet has no parent to be deep under.
                    if let Some(e) = too_deep(0) {
                        return Err(e);
                    }
                    store.create_page(&title)?
                }
                Err(e) => return Err(e),
            };
            let base = match &parent {
                Some(p) => {
                    let st = store.page_state(&page)?;
                    if !st.is_live(p) {
                        return Err(ApiError::new(404, format!("no block {p} on {title:?}")));
                    }
                    st.depth(p) + 1
                }
                None => 0,
            };
            if let Some(e) = too_deep(base) {
                return Err(e);
            }
            let appended = store.append_outline(&page, parent.as_ref(), &items)?;
            let fmt = store.date_format().clone();
            let page_title = utopia_store::display_title(store.page_state(&page)?, &fmt);
            store.flush()?;
            Ok(Outcome {
                body: json!({
                    "page": page.as_str(),
                    "title": page_title,
                    "blocks": appended.len(),
                    "first": appended.first().map(|b| b.as_str()),
                    "last": appended.last().map(|b| b.as_str()),
                }),
                changed: vec![page],
            })
        }
        Op::AppendBlock {
            title,
            text,
            parent,
            create,
        } => {
            let page = match page_by_title(store, &title) {
                Ok(p) => p,
                Err(e) if e.status == 404 && create => store.create_page(&title)?,
                Err(e) => return Err(e),
            };
            let parent = parent.map(BlockId);
            let st = store.page_state(&page)?;
            if let Some(p) = &parent {
                if !st.is_live(p) {
                    return Err(ApiError::new(404, format!("no block {p} on {title:?}")));
                }
            }
            // A page whose only block is empty (a fresh page or journal day) fills it.
            let roots = st.live_roots();
            let reuse = match (&parent, roots.as_slice()) {
                (None, [only]) if st.block(only).is_some_and(|b| b.text.is_empty()) => {
                    !st.has_live_children(only)
                }
                _ => false,
            };
            store.seal_undo();
            let block = if reuse {
                roots[0].clone()
            } else {
                let plan = outline::append_block(store.page_state(&page)?, parent.as_ref());
                store
                    .execute_plan(&page, plan, None)?
                    .ok_or_else(|| ApiError::new(500, "insert produced no block"))?
                    .block
            };
            store.set_block_text(&page, &block, &text, None, None)?;
            store.commit_block(&page, &block)?;
            store.flush()?;
            Ok(Outcome {
                body: block_json(store, &page, &block, false)?,
                changed: vec![page],
            })
        }
    }
}

/// Bounds on the calls an [`ApiServer`] accepts and how long a request waits for one.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How many calls may wait for the store's owner before new requests are refused with
    /// 429. A stalled UI thread must not let the queue grow without bound. At least one
    /// slot always exists, so the queue cannot be turned into a refuse-everything channel.
    pub queue: usize,
    /// How long a request waits for its answer before its call is cancelled (504). A call
    /// older than this never runs, so a client that gave up and retried cannot write twice.
    pub wait: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        // Long enough for a slow write on a big day, short enough that a wedged UI thread
        // does not hold a client forever; 64 calls is far more than the app drains in a
        // frame, and each is a few hundred bytes.
        Self {
            queue: 64,
            wait: Duration::from_secs(30),
        }
    }
}

/// Where a queued call is in its life. It is shared (behind an `Arc`) by the HTTP task that
/// waits for the answer and by the store's owner that runs it, so exactly one of them
/// decides whether the operation happens:
///
/// ```text
/// Pending ──claim (owner)──▶ Running ──▶ Done
///    └────expire (waiter)───▶ Expired    (terminal: the operation never runs)
/// ```
///
/// Both edges out of `Pending` are compare-and-swaps, so a call the client has given up on
/// can never be executed afterwards — which is what made a retry after a 504 duplicate the
/// write (docs/state-machines/api-call-inbox.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallState {
    /// Queued; nothing has touched the store yet.
    Pending,
    /// The owner took it: the store may already have changed.
    Running,
    /// It ran and its answer was sent (or its receiver was already gone).
    Done,
    /// The waiter gave up before the owner took it.
    Expired,
}

impl CallState {
    fn code(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::Running => 1,
            Self::Done => 2,
            Self::Expired => 3,
        }
    }

    fn of(code: u8) -> Self {
        match code {
            0 => Self::Pending,
            1 => Self::Running,
            2 => Self::Done,
            _ => Self::Expired,
        }
    }
}

/// [`CallState`] in an atomic, so the owner and the waiter can race for a pending call.
#[derive(Debug)]
struct CallSlot(AtomicU8);

impl CallSlot {
    fn new() -> Self {
        Self(AtomicU8::new(CallState::Pending.code()))
    }

    fn get(&self) -> CallState {
        CallState::of(self.0.load(Ordering::SeqCst))
    }

    /// `Pending → Running`: the owner takes the call. False once it expired.
    fn claim(&self) -> bool {
        self.swap(CallState::Pending, CallState::Running)
    }

    /// `Pending → Expired`: the waiter stops waiting. False if the owner already took it,
    /// in which case the operation may still land.
    fn expire(&self) -> bool {
        self.swap(CallState::Pending, CallState::Expired)
    }

    fn done(&self) {
        self.0.store(CallState::Done.code(), Ordering::SeqCst);
    }

    fn swap(&self, from: CallState, to: CallState) -> bool {
        self.0
            .compare_exchange(from.code(), to.code(), Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

/// A request waiting for the store's owner.
pub struct ApiCall {
    pub op: Op,
    /// When the client stops waiting for its answer.
    deadline: Instant,
    state: Arc<CallSlot>,
    reply: oneshot::Sender<Result<Value, ApiError>>,
}

impl ApiCall {
    fn new(op: Op, reply: oneshot::Sender<Result<Value, ApiError>>, wait: Duration) -> Self {
        Self {
            op,
            deadline: Instant::now() + wait,
            state: Arc::new(CallSlot::new()),
            reply,
        }
    }

    /// Run the operation and answer the HTTP request. Returns the pages a write changed.
    ///
    /// A call that nobody waits for any more — the request timed out, the client
    /// disconnected, or the call sat in the queue past its deadline — is dropped without
    /// touching the store (§4.1, P2 local automation API).
    pub fn run(self, store: &mut Store) -> Vec<PageId> {
        if self.reply.is_closed() || Instant::now() >= self.deadline || !self.state.claim() {
            self.state.expire();
            // A caller that is still waiting (one with a longer wait than the deadline)
            // gets an answer instead of a hang; a client that gave up ignores it.
            let _ = self.reply.send(Err(ApiError::new(
                504,
                "the call expired before it ran; nothing was written",
            )));
            return Vec::new();
        }
        let r = handle(store, self.op);
        let (reply, changed) = match r {
            Ok(o) => (Ok(o.body), o.changed),
            Err(e) => (Err(e), Vec::new()),
        };
        self.state.done();
        let _ = self.reply.send(reply);
        changed
    }
}

/// A running API server. Dropping it stops the server.
pub struct ApiServer {
    pub addr: SocketAddr,
    pub token: String,
    level: Arc<AtomicU8>,
    calls: std::sync::Mutex<mpsc::Receiver<ApiCall>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ApiServer {
    pub fn try_recv(&self) -> Option<ApiCall> {
        self.calls.lock().ok()?.try_recv().ok()
    }

    pub fn recv_timeout(&self, d: Duration) -> Option<ApiCall> {
        self.calls.lock().ok()?.recv_timeout(d).ok()
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The assistant access level `/v1/op` enforces (MC-2). Off until the owner sets it.
    pub fn level(&self) -> Level {
        Level::of(self.level.load(Ordering::SeqCst))
    }

    /// Change the level for every `/v1/op` request from now on, without a restart. Ops
    /// already queued were checked at the level they arrived under.
    pub fn set_level(&self, level: Level) {
        self.level.store(level.code(), Ordering::SeqCst);
    }

    /// Write `{"url", "token"}` for scripts to discover the server, readable by the user
    /// only. The token is a per-launch secret and lives outside the graph file (§15.6).
    pub fn write_discovery(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = serde_json::to_vec_pretty(&json!({ "url": self.url(), "token": self.token }))
            .map_err(std::io::Error::other)?;
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::remove_file(path);
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)?;
            f.write_all(&body)
        }
        #[cfg(not(unix))]
        std::fs::write(path, body)
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// 256 random bits, hex.
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("system randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Clone)]
struct Ctx {
    calls: mpsc::SyncSender<ApiCall>,
    token: String,
    port: u16,
    wait: Duration,
    level: Arc<AtomicU8>,
}

/// Start the server on a loopback address (`127.0.0.1:0` picks a free port).
pub fn serve(addr: &str, token: String) -> std::io::Result<ApiServer> {
    serve_with(addr, token, Limits::default())
}

/// [`serve`] with explicit [`Limits`] (a test drives the timeout with a short one).
pub fn serve_with(addr: &str, token: String, limits: Limits) -> std::io::Result<ApiServer> {
    let addr: SocketAddr = addr
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the automation API only listens on loopback",
        ));
    }
    let listener = std::net::TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let (calls_tx, calls) = mpsc::sync_channel(limits.queue.max(1));
    let (shutdown, stop) = tokio::sync::oneshot::channel::<()>();
    let level = Arc::new(AtomicU8::new(Level::Off.code()));
    let ctx = Ctx {
        calls: calls_tx,
        token: token.clone(),
        port: addr.port(),
        wait: limits.wait,
        level: level.clone(),
    };
    let thread = std::thread::Builder::new()
        .name("utopia-api".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
                let _ = axum::serve(listener, router(ctx))
                    .with_graceful_shutdown(async {
                        let _ = stop.await;
                    })
                    .await;
            });
        })?;
    Ok(ApiServer {
        addr,
        token,
        level,
        calls: std::sync::Mutex::new(calls),
        shutdown: Some(shutdown),
        thread: Some(thread),
    })
}

fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/v1/pages", get(list_pages))
        .route("/v1/pages/{title}", get(get_page))
        .route("/v1/pages/{title}/backlinks", get(backlinks))
        .route("/v1/pages/{title}/blocks", post(append_block))
        .route("/v1/blocks/{id}", get(get_block).patch(update_block))
        .route("/v1/journal/{date}", get(journal))
        .route("/v1/search", get(search))
        .route("/v1/query", post(query))
        .route("/v1/capture", post(capture))
        .route("/v1/op", post(op_route))
        .with_state(ctx)
}

fn err(status: u16, message: &str) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, Json(json!({ "error": message }))).into_response()
}

/// Constant-time token comparison.
fn token_ok(given: &str, want: &str) -> bool {
    given.len() == want.len()
        && given
            .bytes()
            .zip(want.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn check(ctx: &Ctx, headers: &HeaderMap) -> Result<(), (u16, &'static str)> {
    if headers.contains_key("origin") {
        return Err((403, "browser requests are not accepted"));
    }
    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let loopback = [
        format!("127.0.0.1:{}", ctx.port),
        format!("localhost:{}", ctx.port),
        format!("[::1]:{}", ctx.port),
    ];
    if !loopback.iter().any(|h| h == host) {
        return Err((403, "host must be loopback"));
    }
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if !token_ok(auth, &ctx.token) {
        return Err((401, "missing or wrong bearer token"));
    }
    Ok(())
}

async fn call(ctx: Ctx, headers: HeaderMap, op: Op) -> Response {
    if let Err((status, msg)) = check(&ctx, &headers) {
        return err(status, msg);
    }
    enqueue(ctx, op).await
}

/// Queue one checked call for the store's owner and wait for its answer.
async fn enqueue(ctx: Ctx, op: Op) -> Response {
    let (reply, mut rx) = oneshot::channel();
    let call = ApiCall::new(op, reply, ctx.wait);
    let state = call.state.clone();
    if let Err(e) = ctx.calls.try_send(call) {
        return match e {
            // The owner is behind; refuse rather than let the queue grow. Nothing ran, so
            // the client can try again later.
            mpsc::TrySendError::Full(_) => {
                err(429, "the app is busy; nothing was written, try again")
            }
            mpsc::TrySendError::Disconnected(_) => not_running(),
        };
    }
    // Keep `rx` across the wait (a plain `timeout` would consume it): the answer can land
    // at the very moment the wait ends.
    let waited = tokio::time::timeout(
        ctx.wait,
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut rx).poll(cx)),
    )
    .await;
    match waited {
        Ok(reply) => answer(reply),
        Err(_) => expired(&state, rx),
    }
}

/// The owner's answer, or the news that it stopped without giving one.
fn answer(reply: Result<Result<Value, ApiError>, oneshot::error::RecvError>) -> Response {
    match reply {
        Ok(Ok(body)) => Json(body).into_response(),
        Ok(Err(e)) => err(e.status, &e.message),
        // The owner dropped the call without running it (shutdown), so nothing changed.
        Err(_) => not_running(),
    }
}

/// One answer for "the app is not running", whether the queue was gone before the call was
/// accepted or the app stopped before it ran: both mean nothing touched the store, so a
/// client has one fact to act on instead of two readings of "graph is closed".
fn not_running() -> Response {
    err(503, "the app is not running; nothing was written")
}

/// The request gave up waiting. [`CallState`] says whether its call can still run, so the
/// client learns whether a retry is safe instead of guessing (a 504 used to mean both).
fn expired(state: &CallSlot, mut rx: oneshot::Receiver<Result<Value, ApiError>>) -> Response {
    if let Ok(reply) = rx.try_recv() {
        return answer(Ok(reply));
    }
    if state.expire() {
        err(504, "the app did not answer in time; nothing was written")
    } else {
        match state.get() {
            CallState::Expired => err(504, "the call was cancelled; nothing was written"),
            _ => err(
                504,
                "the app was still running the call when the request gave up; it may be applied",
            ),
        }
    }
}

#[derive(Deserialize)]
struct PagesQ {
    #[serde(default)]
    daily: bool,
}

async fn list_pages(State(c): State<Ctx>, h: HeaderMap, Query(q): Query<PagesQ>) -> Response {
    call(c, h, Op::ListPages { daily: q.daily }).await
}

async fn get_page(State(c): State<Ctx>, h: HeaderMap, UrlPath(title): UrlPath<String>) -> Response {
    call(c, h, Op::GetPage { title }).await
}

async fn backlinks(
    State(c): State<Ctx>,
    h: HeaderMap,
    UrlPath(title): UrlPath<String>,
) -> Response {
    call(c, h, Op::Backlinks { title }).await
}

#[derive(Deserialize)]
struct AppendBody {
    text: String,
    parent: Option<String>,
    #[serde(default)]
    create: bool,
}

async fn append_block(
    State(c): State<Ctx>,
    h: HeaderMap,
    UrlPath(title): UrlPath<String>,
    Json(b): Json<AppendBody>,
) -> Response {
    call(
        c,
        h,
        Op::AppendBlock {
            title,
            text: b.text,
            parent: b.parent,
            create: b.create,
        },
    )
    .await
}

async fn get_block(State(c): State<Ctx>, h: HeaderMap, UrlPath(id): UrlPath<String>) -> Response {
    call(c, h, Op::GetBlock { id, subtree: false }).await
}

#[derive(Deserialize)]
struct TextBody {
    text: String,
}

async fn update_block(
    State(c): State<Ctx>,
    h: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(b): Json<TextBody>,
) -> Response {
    let op = Op::UpdateBlock {
        id,
        text: Some(b.text),
        kind: None,
        checked: None,
        if_rev: None,
    };
    call(c, h, op).await
}

async fn journal(State(c): State<Ctx>, h: HeaderMap, UrlPath(date): UrlPath<String>) -> Response {
    call(c, h, Op::Journal { date }).await
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
    limit: Option<usize>,
}

async fn search(State(c): State<Ctx>, h: HeaderMap, Query(q): Query<SearchQ>) -> Response {
    let limit = q.limit.unwrap_or(20).min(500);
    call(c, h, Op::Search { q: q.q, limit }).await
}

#[derive(Deserialize)]
struct QueryBody {
    query: String,
    limit: Option<usize>,
}

async fn query(State(c): State<Ctx>, h: HeaderMap, Json(b): Json<QueryBody>) -> Response {
    let limit = b.limit.unwrap_or(100).min(1000);
    call(
        c,
        h,
        Op::Query {
            query: b.query,
            limit,
        },
    )
    .await
}

async fn capture(State(c): State<Ctx>, h: HeaderMap, Json(b): Json<TextBody>) -> Response {
    call(c, h, Op::Capture { text: b.text }).await
}

/// `POST /v1/op` (docs/mcp-design.md §2.4): the request checks of every route, then the
/// body as an [`Op`] (400 if it is not one), then the access level (MC-2): a refused op is
/// answered 403 here and never reaches the queue, so the store's owner never sees it.
async fn op_route(State(c): State<Ctx>, h: HeaderMap, body: Bytes) -> Response {
    if let Err((status, msg)) = check(&c, &h) {
        return err(status, msg);
    }
    let op: Op = match serde_json::from_slice(&body) {
        Ok(op) => op,
        Err(e) => return err(400, &format!("the body is not an op: {e}")),
    };
    let level = Level::of(c.level.load(Ordering::SeqCst));
    if let Some(refusal) = level.refusal(op.kind()) {
        return err(403, refusal);
    }
    enqueue(c, op).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("g.utopia")).unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        (dir, store)
    }

    /// The text of every live block, in preorder.
    fn texts(store: &mut Store) -> Vec<String> {
        let pages = store.list_pages(true).unwrap();
        let mut out = Vec::new();
        for page in &pages {
            let st = store.page_state(&page.id).unwrap().clone();
            for (id, _) in st.live_preorder() {
                out.push(st.block(&id).unwrap().text.clone());
            }
        }
        out
    }

    fn pending(op: Op, wait: Duration) -> (ApiCall, oneshot::Receiver<Result<Value, ApiError>>) {
        let (reply, rx) = oneshot::channel();
        (ApiCall::new(op, reply, wait), rx)
    }

    fn capture(text: &str) -> Op {
        Op::Capture { text: text.into() }
    }

    /// A client that gave up (its 504 receiver is gone) must not be served later.
    #[test]
    fn a_call_whose_client_left_never_runs() {
        let (_dir, mut store) = store();
        let (call, rx) = pending(capture("one"), Duration::from_secs(30));
        drop(rx);
        assert!(
            call.run(&mut store).is_empty(),
            "a call nobody waits for changes nothing"
        );
        assert_eq!(texts(&mut store), Vec::<String>::new());
    }

    /// A call that sat in the queue past its deadline must not run either.
    #[test]
    fn a_call_past_its_deadline_never_runs() {
        let (_dir, mut store) = store();
        let (call, mut rx) = pending(capture("one"), Duration::ZERO);
        assert!(call.run(&mut store).is_empty());
        assert_eq!(texts(&mut store), Vec::<String>::new());
        match rx.try_recv() {
            Ok(Err(e)) => assert_eq!(e.status, 504, "the caller is told it did not run"),
            other => panic!("expected a 504 answer, got {other:?}"),
        }
    }

    #[test]
    fn an_expired_call_never_starts() {
        let slot = CallSlot::new();
        assert_eq!(slot.get(), CallState::Pending);
        assert!(slot.expire(), "the waiter cancels a pending call");
        assert_eq!(slot.get(), CallState::Expired);
        assert!(!slot.claim(), "an expired call is terminal");
        assert!(!slot.expire(), "expiring twice changes nothing");
    }

    /// Plugin run lifecycle, issue 2: the permission a plugin needs comes from
    /// `Op::kind`, so the classification must say what `handle` does. Every op is built
    /// from its JSON form (what a plugin sends) and run against a small graph; a write
    /// must name the page it touched, a read must touch none.
    #[test]
    fn ops_classify_reads_and_writes_consistently() {
        let (_dir, mut store) = store();
        let page = store.create_page("Test Page").unwrap();
        let block = store.ensure_first_block(&page).unwrap();
        store
            .set_block_text(&page, &block, "one", None, None)
            .unwrap();
        store.commit_block(&page, &block).unwrap();
        store
            .open_daily(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap())
            .unwrap();
        store.sync_index().unwrap();
        let op =
            |v: Value| -> Op { serde_json::from_value(v).expect("an op as a plugin sends it") };
        let cases: Vec<(Op, OpKind)> = vec![
            (op(json!({"op": "list_pages"})), OpKind::Read),
            (
                op(json!({"op": "get_page", "title": "Test Page"})),
                OpKind::Read,
            ),
            (
                op(json!({"op": "backlinks", "title": "Test Page"})),
                OpKind::Read,
            ),
            (
                op(json!({"op": "get_block", "id": block.as_str()})),
                OpKind::Read,
            ),
            (op(json!({"op": "journal", "date": "today"})), OpKind::Read),
            (op(json!({"op": "search", "q": "one"})), OpKind::Read),
            (op(json!({"op": "query", "query": "\"one\""})), OpKind::Read),
            (
                op(json!({"op": "append_block", "title": "Test Page", "text": "two"})),
                OpKind::Write,
            ),
            (
                op(json!({"op": "append_outline", "title": "Test Page",
                          "markdown": "- three\n  - three point one\n"})),
                OpKind::Write,
            ),
            (
                op(json!({"op": "update_block", "id": block.as_str(), "text": "one edited"})),
                OpKind::Write,
            ),
            (
                op(json!({"op": "capture", "text": "captured"})),
                OpKind::Write,
            ),
        ];
        for (op, want) in cases {
            let name = format!("{op:?}");
            assert_eq!(
                op.kind(),
                want,
                "{name}: classified against what handle does"
            );
            let out = handle(&mut store, op).expect(&name);
            match want {
                OpKind::Write => assert!(!out.changed.is_empty(), "{name}: a write names its page"),
                OpKind::Read => assert!(out.changed.is_empty(), "{name}: a read changes nothing"),
            }
        }
    }

    /// An append of `markdown` to `title`, as the MCP bridge and a plugin send it.
    fn append_outline(title: &str, markdown: &str, parent: Option<&BlockId>, create: bool) -> Op {
        Op::AppendOutline {
            title: title.into(),
            markdown: markdown.into(),
            parent: parent.map(|p| p.as_str().to_string()),
            create,
        }
    }

    /// The `(text, depth)` rows of one page, in document order.
    fn rows(store: &mut Store, page: &PageId) -> Vec<(String, usize)> {
        let st = store.page_state(page).unwrap().clone();
        st.live_preorder()
            .into_iter()
            .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
            .collect()
    }

    /// MC-3, MC-8: one `append_blocks` call appends the whole outline in one commit and
    /// leaves the projections equal to a rebuild (`doctor`, rule 2).
    #[test]
    fn mc_3_append_outline_commits_the_whole_outline_and_doctor_is_clean() {
        let (_dir, mut store) = store();
        let out = handle(
            &mut store,
            append_outline(
                "Reading list",
                "- Dune\n  - chapter one\n- Emma\n",
                None,
                true,
            ),
        )
        .unwrap();
        assert_eq!(out.changed.len(), 1, "the page it wrote");
        assert_eq!(out.body["blocks"], json!(3));
        assert_eq!(out.body["title"], json!("Reading list"));
        let page = PageId(out.body["page"].as_str().expect("a page id").to_string());
        assert_eq!(
            rows(&mut store, &page),
            vec![
                ("Dune".to_string(), 0),
                ("chapter one".to_string(), 1),
                ("Emma".to_string(), 0)
            ]
        );
        // The body names the first and last block, not their text.
        let ids: Vec<BlockId> = store
            .page_state(&page)
            .unwrap()
            .live_preorder()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(out.body["first"].as_str(), Some(ids[0].as_str()));
        assert_eq!(out.body["last"].as_str(), Some(ids[2].as_str()));
        store.sync_index().unwrap();
        assert!(store.doctor().unwrap().is_ok(), "incremental == rebuild");

        // A second call appends after the first without touching it.
        handle(
            &mut store,
            append_outline("Reading list", "- Piranesi\n", None, false),
        )
        .unwrap();
        let st = store.page_state(&page).unwrap().clone();
        let texts: Vec<&str> = st
            .live_preorder()
            .iter()
            .map(|(id, _)| st.block(id).unwrap().text.as_str())
            .collect();
        assert_eq!(texts, vec!["Dune", "chapter one", "Emma", "Piranesi"]);
        store.sync_index().unwrap();
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-9: the whole outline is one step on the undo stack, so one Cmd+Z in the app takes
    /// all of it off (FR-2.12) and one redo puts it back.
    #[test]
    fn mc_9_one_undo_removes_the_whole_outline() {
        let (_dir, mut store) = store();
        let page = store.create_page("Notes").unwrap();
        let first = store.ensure_first_block(&page).unwrap();
        store
            .set_block_text(&page, &first, "kept", None, None)
            .unwrap();
        store.commit_block(&page, &first).unwrap();
        store.sync_index().unwrap();
        let before = rows(&mut store, &page);
        let steps = store.undo_depth();

        handle(
            &mut store,
            append_outline("Notes", "- a\n  - b\n    - c\n- d\n", None, false),
        )
        .unwrap();
        assert_eq!(store.undo_depth(), steps + 1, "one step, not one per block");
        assert_eq!(rows(&mut store, &page).len(), before.len() + 4);

        store.undo().unwrap();
        assert_eq!(rows(&mut store, &page), before, "the whole outline is gone");
        store.redo().unwrap();
        assert_eq!(rows(&mut store, &page).len(), before.len() + 4);
        store.sync_index().unwrap();
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-8, rule 2: the outline and the pages its links make are one transaction. When the
    /// dependent write fails, no part of the outline is durable, so a retry cannot duplicate
    /// it. The failure comes from SQLite itself (a trigger on the linked page's row), so the
    /// writer path runs unchanged.
    #[test]
    fn mc_8_a_failed_link_page_write_leaves_no_outline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.utopia");
        let mut store = Store::open(&path).unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let page = store.create_page("Log").unwrap();
        store.sync_index().unwrap();
        store
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_linked_page BEFORE INSERT ON page
                 WHEN NEW.title_key = 'ana'
                 BEGIN SELECT RAISE(ABORT, 'injected linked-page failure'); END",
            )
            .unwrap();

        let _ = handle(
            &mut store,
            append_outline("Log", "- met [[Ana]] there\n", None, false),
        );
        // The writer may not have run yet: the barrier is what reports its failure.
        let err = store
            .sync_index()
            .expect_err("the linked page write failed");
        assert!(err.to_string().contains("injected"), "{err}");
        drop(store);

        // A fresh store sees what the database holds: no outline, no page it named.
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store.page_state(&page).unwrap().live_count(),
            0,
            "a failed call must not leave the outline durable"
        );
        assert!(store.find_page("Ana").unwrap().is_none());
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-8, rule 2: the outline and the page its link makes are *one* transaction, so
    /// neither can be durable alone. The failure is on the outline's own block, after the
    /// page row was written in the same job: a separate earlier commit for the page would
    /// survive, and this test would see it.
    #[test]
    fn mc_8_the_outline_and_its_pages_are_one_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.utopia");
        let mut store = Store::open(&path).unwrap();
        let page = store.create_page("Log").unwrap();
        store.sync_index().unwrap();
        store
            .conn()
            .execute_batch(
                "CREATE TRIGGER fail_outline_block BEFORE INSERT ON block
                 WHEN NEW.text = 'met [[Ana]] there'
                 BEGIN SELECT RAISE(ABORT, 'injected outline failure'); END",
            )
            .unwrap();

        let _ = handle(
            &mut store,
            append_outline("Log", "- met [[Ana]] there\n", None, false),
        );
        let err = store.sync_index().expect_err("the outline write failed");
        assert!(err.to_string().contains("injected"), "{err}");
        drop(store);

        let mut store = Store::open(&path).unwrap();
        assert!(
            store.find_page("Ana").unwrap().is_none(),
            "the page the link named must not survive without the outline"
        );
        assert_eq!(store.page_state(&page).unwrap().live_count(), 0);
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-8, W3/FR-1.10: a `[[link]]` and a tag in an appended outline make their pages and
    /// their backlinks, as the same text typed into a block does.
    #[test]
    fn mc_8_a_link_in_an_outline_makes_its_page_and_backlink() {
        let (_dir, mut store) = store();
        let out = handle(
            &mut store,
            append_outline(
                "Log",
                "- met [[Ana]] about #launch\n- #[[two words]]\n",
                None,
                true,
            ),
        )
        .unwrap();
        let page = PageId(out.body["page"].as_str().expect("a page id").to_string());
        assert_eq!(rows(&mut store, &page).len(), 2);
        store.sync_index().unwrap();
        let ana = store.find_page("Ana").unwrap().expect("the link made it");
        assert!(
            store.find_page("launch").unwrap().is_some(),
            "a tag is a page"
        );
        assert!(store.find_page("two words").unwrap().is_some());
        assert_eq!(store.backlink_count(&ana).unwrap(), 1, "one backlink");
        let launch = store.find_page("launch").unwrap().expect("the tag page");
        assert_eq!(
            store.backlink_count(&launch).unwrap(),
            1,
            "a tag backlinks too"
        );
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-8, design §4: the id markers of the outline format are taken off every line, so
    /// `read_page`'s own output can be appended as it was read. A `{id:…}` that is not the
    /// marker of a whole line stays text.
    #[test]
    fn mc_8_the_id_markers_of_an_outline_are_stripped() {
        let (_dir, mut store) = store();
        let out = handle(
            &mut store,
            append_outline(
                "Copy",
                "- one {id:01J8Z3K9V2}\n  - two\n    continued {id:01J8Z3K9W0}\n",
                None,
                true,
            ),
        )
        .unwrap();
        let page = PageId(out.body["page"].as_str().unwrap().to_string());
        assert_eq!(
            rows(&mut store, &page)
                .into_iter()
                .map(|(t, _)| t)
                .collect::<Vec<_>>(),
            vec!["one", "two", "continued"]
        );
        assert_eq!(
            strip_id_markers("- keep {id:x} mid\n- {id:}\n- {id:a b}\n- { id:01A }\n"),
            "- keep {id:x} mid\n- {id:}\n- {id:a b}\n- { id:01A }\n",
            "only a marker at the end of a line is one"
        );
        assert_eq!(strip_id_markers("  text {id:01A}  \n"), "  text\n");
    }

    /// MC-8, FR-1.2: a title that names a date makes that day's page on this first write,
    /// and it is the daily page, not a page titled with the date text.
    #[test]
    fn mc_8_a_date_target_makes_the_daily_page() {
        let (_dir, mut store) = store();
        let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let out = handle(
            &mut store,
            append_outline("2026-11-02", "- wrote the brief\n", None, true),
        )
        .unwrap();
        let page = PageId(out.body["page"].as_str().unwrap().to_string());
        let st = store.page_state(&page).unwrap().clone();
        assert_eq!(st.meta.daily_date, Some(today), "the daily page");
        assert_eq!(
            out.body["title"],
            json!(store.date_format().format(today)),
            "the day's display title, not the text of the date it was asked for"
        );
        assert_eq!(
            rows(&mut store, &page),
            vec![("wrote the brief".to_string(), 0)]
        );
        store.sync_index().unwrap();
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-8: a page whose only block is empty (a fresh page or a day opened for its first
    /// write) takes the outline's first line in that block, as `AppendBlock` fills it.
    #[test]
    fn mc_8_an_empty_page_takes_the_outline_in_its_block() {
        let (_dir, mut store) = store();
        let (page, first) = store
            .open_daily(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap())
            .unwrap();
        let out = handle(
            &mut store,
            append_outline("2026-11-02", "- first line\n  - child\n", None, false),
        )
        .unwrap();
        assert_eq!(out.body["blocks"], json!(2));
        assert_eq!(out.body["first"].as_str(), Some(first.as_str()));
        assert_eq!(
            rows(&mut store, &page),
            vec![("first line".to_string(), 0), ("child".to_string(), 1)],
            "the empty block was filled, not left above the outline"
        );
    }

    /// MC-8: `parent` puts the outline last under that block; a parent that is not on the
    /// page is 404 and writes nothing.
    #[test]
    fn mc_8_a_parent_puts_the_outline_under_it() {
        let (_dir, mut store) = store();
        let page = store.create_page("Plan").unwrap();
        let root = store.ensure_first_block(&page).unwrap();
        store
            .set_block_text(&page, &root, "root", None, None)
            .unwrap();
        store.commit_block(&page, &root).unwrap();
        handle(
            &mut store,
            append_outline("Plan", "- under\n  - deeper\n", Some(&root), false),
        )
        .unwrap();
        assert_eq!(
            rows(&mut store, &page),
            vec![
                ("root".to_string(), 0),
                ("under".to_string(), 1),
                ("deeper".to_string(), 2)
            ]
        );
        let missing = BlockId::new();
        let before = rows(&mut store, &page);
        let err = handle(
            &mut store,
            append_outline("Plan", "- nowhere\n", Some(&missing), false),
        )
        .unwrap_err();
        assert_eq!(err.status, 404, "{}", err.message);
        assert_eq!(rows(&mut store, &page), before);
    }

    /// MC-8: `create` off on a page that does not resolve is 404 and writes nothing.
    #[test]
    fn mc_8_a_missing_page_is_not_created_without_create() {
        let (_dir, mut store) = store();
        let pages = store.list_pages(true).unwrap().len();
        let err = handle(
            &mut store,
            append_outline("Nowhere", "- a note\n", None, false),
        )
        .unwrap_err();
        assert_eq!(err.status, 404, "{}", err.message);
        assert!(store.find_page("Nowhere").unwrap().is_none());
        assert_eq!(store.list_pages(true).unwrap().len(), pages);

        // With `create` on but a parent given, the block cannot be on a page that does not
        // exist yet, so the call fails before it makes one.
        let missing = BlockId::new();
        let err = handle(
            &mut store,
            append_outline("Elsewhere", "- a note\n", Some(&missing), true),
        )
        .unwrap_err();
        assert_eq!(err.status, 404, "{}", err.message);
        assert!(store.find_page("Elsewhere").unwrap().is_none());
        assert_eq!(store.list_pages(true).unwrap().len(), pages);
    }

    /// MC-8, FR-2.9: an outline deeper than 12 levels is refused and writes nothing — not
    /// even the page, when the call would have created it.
    #[test]
    fn mc_8_an_outline_deeper_than_twelve_levels_writes_nothing() {
        let (_dir, mut store) = store();
        let too_deep: String = (0..13)
            .map(|d| format!("{}- d{d}\n", "  ".repeat(d)))
            .collect();
        let pages = store.list_pages(true).unwrap().len();
        let err = handle(&mut store, append_outline("Deep", &too_deep, None, true)).unwrap_err();
        assert_eq!(err.status, 400, "{}", err.message);
        assert!(err.message.contains("12 levels"), "{}", err.message);
        assert!(store.find_page("Deep").unwrap().is_none(), "no page either");
        assert_eq!(store.list_pages(true).unwrap().len(), pages);

        // Twelve levels are allowed where the ceiling leaves room for them.
        let page = store.create_page("Deep enough").unwrap();
        let body: String = (0..12)
            .map(|d| format!("{}- d{d}\n", "  ".repeat(d)))
            .collect();
        handle(
            &mut store,
            append_outline("Deep enough", &body, None, false),
        )
        .unwrap();
        assert_eq!(rows(&mut store, &page).len(), 12);
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-8: an outline with more blocks than one call may add is refused and writes
    /// nothing, again without leaving a page behind.
    #[test]
    fn mc_8_an_outline_above_the_block_limit_writes_nothing() {
        let (_dir, mut store) = store();
        let many: String = (0..=MAX_OUTLINE_BLOCKS)
            .map(|i| format!("- {i}\n"))
            .collect();
        let pages = store.list_pages(true).unwrap().len();
        let steps = store.undo_depth();
        let err = handle(&mut store, append_outline("Big", &many, None, true)).unwrap_err();
        assert_eq!(err.status, 400, "{}", err.message);
        assert!(
            err.message.contains(&MAX_OUTLINE_BLOCKS.to_string()),
            "{}",
            err.message
        );
        assert!(store.find_page("Big").unwrap().is_none());
        assert_eq!(store.list_pages(true).unwrap().len(), pages);
        assert_eq!(store.undo_depth(), steps, "a refusal records no step");

        // The limit itself is allowed.
        let at_limit: String = (0..MAX_OUTLINE_BLOCKS)
            .map(|i| format!("- {i}\n"))
            .collect();
        let out = handle(&mut store, append_outline("Big", &at_limit, None, true)).unwrap();
        assert_eq!(out.body["blocks"], json!(MAX_OUTLINE_BLOCKS));
        assert_eq!(store.undo_depth(), steps + 1);
        store.sync_index().unwrap();
        assert!(store.doctor().unwrap().is_ok());
    }

    /// MC-7: `read_block` needs a block's subtree. The default answer stays the shape
    /// every existing caller gets (no `children` key).
    #[test]
    fn mc_7_get_block_returns_a_subtree_only_when_asked() {
        let (_dir, mut store) = store();
        let append = |store: &mut Store, text: &str, parent: Option<String>| {
            handle(
                store,
                Op::AppendBlock {
                    title: "Test Page".into(),
                    text: text.into(),
                    parent,
                    create: true,
                },
            )
            .unwrap()
            .body
        };
        let root = append(&mut store, "root", None);
        let root_id = root["id"].as_str().unwrap().to_string();
        let child = append(&mut store, "child", Some(root_id.clone()));
        let child_id = child["id"].as_str().unwrap().to_string();
        let leaf = append(&mut store, "leaf", Some(child_id.clone()));
        let leaf_id = leaf["id"].as_str().unwrap().to_string();
        assert_eq!(child["page"], root["page"], "same page");

        let plain = handle(
            &mut store,
            Op::GetBlock {
                id: root_id.clone(),
                subtree: false,
            },
        )
        .unwrap()
        .body;
        assert!(plain.get("children").is_none(), "no subtree unless asked");
        assert_eq!(plain["text"], "root");

        let sub = handle(
            &mut store,
            Op::GetBlock {
                id: root_id.clone(),
                subtree: true,
            },
        )
        .unwrap()
        .body;
        assert_eq!(sub["children"][0]["id"], child_id.as_str());
        assert_eq!(sub["children"][0]["text"], "child");
        assert_eq!(sub["children"][0]["children"][0]["id"], leaf_id.as_str());
        assert_eq!(
            sub["children"][0]["children"][0]["children"],
            json!([]),
            "a leaf has an empty children list"
        );
        // A child read on its own is just its own subtree.
        let from_child = handle(
            &mut store,
            Op::GetBlock {
                id: child_id.clone(),
                subtree: true,
            },
        )
        .unwrap()
        .body;
        assert_eq!(from_child["children"][0]["text"], "leaf");
        assert_eq!(from_child["children"][0]["children"], json!([]));
    }

    /// MC-10: `rev` is a short hash of the text, so an edit changes it and the same text
    /// always gives the same value.
    #[test]
    fn mc_10_the_block_rev_tracks_the_text() {
        assert_eq!(block_rev("one"), block_rev("one"));
        assert_ne!(block_rev("one"), block_rev("one "));
        assert_eq!(block_rev("one").len(), 12);
        assert!(block_rev("one").chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(block_rev(""), block_rev("\n"));
    }

    /// MC-3: the MCP bridge sends `Op` values over `/v1/op`, so each one must serialize to
    /// the JSON shape it reads, and read back as itself.
    #[test]
    fn mc_3_every_op_round_trips_through_its_json() {
        let all = vec![
            (
                Op::ListPages { daily: true },
                json!({"op": "list_pages", "daily": true}),
            ),
            (
                Op::GetPage { title: "A".into() },
                json!({"op": "get_page", "title": "A"}),
            ),
            (
                Op::Backlinks { title: "A".into() },
                json!({"op": "backlinks", "title": "A"}),
            ),
            (
                Op::AppendBlock {
                    title: "A".into(),
                    text: "t".into(),
                    parent: Some("b1".into()),
                    create: true,
                },
                json!({"op": "append_block", "title": "A", "text": "t", "parent": "b1", "create": true}),
            ),
            (
                Op::AppendBlock {
                    title: "A".into(),
                    text: "t".into(),
                    parent: None,
                    create: false,
                },
                json!({"op": "append_block", "title": "A", "text": "t", "parent": null, "create": false}),
            ),
            (
                Op::GetBlock {
                    id: "b1".into(),
                    subtree: false,
                },
                json!({"op": "get_block", "id": "b1", "subtree": false}),
            ),
            (
                Op::AppendOutline {
                    title: "A".into(),
                    markdown: "- t\n".into(),
                    parent: Some("b1".into()),
                    create: true,
                },
                json!({"op": "append_outline", "title": "A", "markdown": "- t\n", "parent": "b1", "create": true}),
            ),
            (
                Op::GetBlock {
                    id: "b1".into(),
                    subtree: true,
                },
                json!({"op": "get_block", "id": "b1", "subtree": true}),
            ),
            (
                Op::UpdateBlock {
                    id: "b1".into(),
                    text: Some("t".into()),
                    kind: None,
                    checked: None,
                    if_rev: None,
                },
                json!({"op": "update_block", "id": "b1", "text": "t"}),
            ),
            (
                Op::UpdateBlock {
                    id: "b1".into(),
                    text: None,
                    kind: Some("todo".into()),
                    checked: Some(true),
                    if_rev: Some("0123456789ab".into()),
                },
                json!({"op": "update_block", "id": "b1", "kind": "todo", "checked": true, "if_rev": "0123456789ab"}),
            ),
            (
                Op::Journal {
                    date: "today".into(),
                },
                json!({"op": "journal", "date": "today"}),
            ),
            (
                Op::Search {
                    q: "x".into(),
                    limit: 5,
                },
                json!({"op": "search", "q": "x", "limit": 5}),
            ),
            (
                Op::Query {
                    query: "{and: [[A]]}".into(),
                    limit: 7,
                },
                json!({"op": "query", "query": "{and: [[A]]}", "limit": 7}),
            ),
            (
                Op::Capture { text: "t".into() },
                json!({"op": "capture", "text": "t"}),
            ),
        ];
        for (op, want) in all {
            let v = serde_json::to_value(&op).unwrap();
            assert_eq!(v, want, "{op:?} serializes to the shape it reads");
            let back: Op = serde_json::from_value(v).unwrap();
            assert_eq!(back, op);
            // Exhaustive: a new variant does not compile until this match names it, which is
            // the prompt to add a row to the table above.
            match op {
                Op::ListPages { .. }
                | Op::GetPage { .. }
                | Op::Backlinks { .. }
                | Op::AppendBlock { .. }
                | Op::AppendOutline { .. }
                | Op::GetBlock { .. }
                | Op::UpdateBlock { .. }
                | Op::Journal { .. }
                | Op::Search { .. }
                | Op::Query { .. }
                | Op::Capture { .. } => {}
            }
        }
    }

    #[test]
    fn a_running_call_cannot_be_cancelled() {
        let slot = CallSlot::new();
        assert!(slot.claim(), "the owner takes a pending call");
        assert!(!slot.claim(), "it is taken once");
        assert!(!slot.expire(), "so the waiter learns the write may land");
        slot.done();
        assert_eq!(slot.get(), CallState::Done);
    }
}
