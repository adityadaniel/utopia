//! The tool catalog (PRD §13.8 MC-7, MC-11; `docs/mcp-design.md` §4, §4.1, §4.2).
//!
//! Every read tool is a thin mapping onto one [`Op`] plus a rendering of the result:
//!
//! | Tool | Arguments | Op |
//! |---|---|---|
//! | `search` | `query`, `limit` | [`Op::Search`] |
//! | `list_pages` | `include_daily` | [`Op::ListPages`] |
//! | `read_page` | `page`, `after` | [`Op::Journal`] for a date, [`Op::GetPage`] for the rest |
//! | `read_block` | `id`, `after` | [`Op::GetBlock`] with `subtree` |
//! | `get_backlinks` | `page` | [`Op::Backlinks`] |
//! | `run_query` | `query`, `limit` | [`Op::Query`] |
//! | `append_blocks` | `page`, `markdown`, `parent`, `create` | [`Op::AppendOutline`] |
//! | `update_block` | `id`, `text`, `kind`, `checked`, `if_rev` | [`Op::UpdateBlock`] |
//!
//! Two rules shape the output. Page text appears only in results, never in a description or
//! in [`INSTRUCTIONS`] (MC-11): those are constants of this file. And one result is cut at
//! [`BUDGET`] bytes, at a block boundary, with a last line that says how to read the rest
//! (§4.2).
//!
//! The outline format: one line for each block, `- ` and a kind prefix, indented two spaces
//! per level, and the line (or the block's last line, when its text runs over several)
//! ends with a marker that carries the block's id:
//!
//! ```text
//! - Project X {id:01J8Z3K9V2}
//!   - [ ] write the brief {id:01J8Z3K9W0}
//! ```
//!
//! The marker is `{id:…}` and deliberately not Utopia markup: `((id))` is a block
//! reference, so an agent that sent a line back would create a reference. A single `{` is
//! literal text to the inline parser, so this marker is inert in a block that an agent
//! writes back (PR 7).

use std::collections::HashMap;

use chrono::NaiveDate;
use serde_json::{json, Map, Value};
use utopia_api::{block_rev, Op, OpKind};
use utopia_core::date::{iso, parse_iso, relative_date};

use crate::backend::{Backend, BackendError};

/// The most text one tool result carries (design §4.2): above the 10,000-token warning of
/// Claude Code, below its 25,000-token limit. Slice 1.4 adjusts it with the real clients.
pub const BUDGET: usize = 24 * 1024;

/// How an outline line names the block it came from, e.g. `{id:01J8Z3K9V2}`. The one
/// definition is in `utopia_api`, because `Op::AppendOutline` takes the marker off the text an
/// agent sends back (PR 7, design §4); [`MARKER_PREFIX`] and [`MARKER_SUFFIX`] are how this
/// file reads and writes it.
pub const MARKER_PREFIX: &str = utopia_api::ID_MARKER_PREFIX;
pub const MARKER_SUFFIX: &str = utopia_api::ID_MARKER_SUFFIX;

/// The guide a client gets from `initialize` and `server/discover` (design §4). It is a
/// constant: no page text, ever (MC-11).
pub const INSTRUCTIONS: &str = "\
Utopia is a Roam-style outliner: every note is a block in a tree under a page.

Syntax in block text:
- [[Page]] links to the page titled Page; an alias of that page resolves too.
- #tag and #[[two words]] are page links as well; a tag is a page.
- ((block-id)) is a reference to one block and shows that block's text where it is written.
- key:: value at the start of a block sets an attribute, which query blocks can match.

An outline from read_page or read_block has one line for each block, indented two spaces
per level, and each line ends with the block's id in the form {id:01J8Z3K9V2}. That marker
is not markup and not a reference: keep it on the line, and use the id with read_block.
append_blocks takes such an outline back, in the same format, and ignores those markers.

A read never creates anything: a day with no page reads as an empty outline. A result
longer than 24 KB is cut at a block boundary and ends with a line that says how many
blocks are missing and gives an `after` id: pass that id back to the same call to read the
next part.

update_block changes one block: its text, its kind (bullet, numbered, to-do, quote, code
or a heading) or whether a to-do is done. read_block gives the block's rev; send it back
as if_rev and the call fails, writing nothing, when the block changed since you read it.
Every update_block call is one undo step in Utopia.";

/// What a tool call answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The tool is not in the catalog: a protocol error, `-32602` (design §3.2), because
    /// there is nothing for the model to do with it.
    Unknown,
    /// The call failed. The text goes to the model with `"isError": true`.
    Failed(String),
}

impl From<BackendError> for ToolError {
    fn from(e: BackendError) -> Self {
        match e {
            BackendError::Failed { message, .. }
            | BackendError::OwnedElsewhere { message }
            | BackendError::NoOwnerApi { message } => ToolError::Failed(message),
        }
    }
}

/// One tool: the name, the description, its kind (which decides the annotations and the
/// `--read-only` filter), the JSON schema of its arguments and the function that maps the
/// arguments to an op and renders the answer.
struct Tool {
    name: &'static str,
    description: &'static str,
    kind: OpKind,
    /// `destructiveHint` (§4.1). The six read tools never destroy anything.
    destructive: bool,
    /// `idempotentHint` (§4.1). `append_blocks` is not: calling it twice appends twice.
    idempotent: bool,
    schema: fn() -> Value,
    run: fn(&dyn Backend, &Args<'_>) -> Result<String, ToolError>,
}

/// The catalog. `tools/list` has the same order for every caller and for every graph: it
/// does not depend on the access file (design §5) or on the contents (MC-11).
const CATALOG: &[Tool] = &[
    Tool {
        name: "search",
        description: "Search page titles and block text. Returns matching pages and matching \
                      blocks; a block line ends with the block's id, which read_block takes.",
        kind: OpKind::Read,
        destructive: false,
        idempotent: true,
        schema: schema_search,
        run: run_search,
    },
    Tool {
        name: "list_pages",
        description: "List the graph's pages. The daily notes are left out unless \
                      include_daily is true.",
        kind: OpKind::Read,
        destructive: false,
        idempotent: true,
        schema: schema_list_pages,
        run: run_list_pages,
    },
    Tool {
        name: "read_page",
        description: "Read a page as an indented Markdown outline. `page` is a title, an \
                      alias, `today`, `yesterday` or an ISO date (2026-11-02). A day with no \
                      page reads as an empty outline: a read never creates a page. A result \
                      cut at 24 KB names an `after` id: pass it back to read the next part.",
        kind: OpKind::Read,
        destructive: false,
        idempotent: true,
        schema: schema_read_page,
        run: run_read_page,
    },
    Tool {
        name: "read_block",
        description: "Read one block and its children as an indented Markdown outline. The \
                      first line gives `rev`, the short hash of the block's text. The \
                      outline ends each line with the block's id. A result cut at 24 KB \
                      names an `after` id: pass it back to read the next part.",
        kind: OpKind::Read,
        destructive: false,
        idempotent: true,
        schema: schema_read_block,
        run: run_read_block,
    },
    Tool {
        name: "get_backlinks",
        description: "List the blocks that link to a page (its Linked References), grouped by \
                      the page each one is written on.",
        kind: OpKind::Read,
        destructive: false,
        idempotent: true,
        schema: schema_get_backlinks,
        run: run_get_backlinks,
    },
    Tool {
        name: "run_query",
        description: "Run a Roam-style query block against the graph, for example \
                      `{and: [[Project X]] [[TODO]]}` (the unchecked to-dos on, under, or \
                      linking Project X), and return the matching blocks grouped by page.",
        kind: OpKind::Read,
        destructive: false,
        idempotent: true,
        schema: schema_run_query,
        run: run_run_query,
    },
    Tool {
        name: "append_blocks",
        description: "Append blocks to a page, as an indented Markdown outline: one line for \
                      each block, two spaces per level. The {id:…} markers of read_page are \
                      ignored, so an outline read from a page can be sent back. `page` is a \
                      title, an alias, `today`, `yesterday` or an ISO date; `create` makes \
                      the page when it does not exist, and a date is that day's note. The \
                      whole outline is one commit and one undo step, and at most 2000 blocks \
                      in one call.",
        kind: OpKind::Write,
        destructive: false,
        idempotent: false,
        schema: schema_append_blocks,
        run: run_append_blocks,
    },
    Tool {
        name: "update_block",
        description: "Change one block: its text, its kind (bullet, to-do, heading, …) or \
                      whether a to-do is done. Send `if_rev` from read_block and the call \
                      fails, writing nothing, when the block changed since you read it. The \
                      whole change is one undo step, and the answer gives the block with its \
                      new rev.",
        kind: OpKind::Write,
        // Design §4.1: it replaces a block's text, which is the only copy of those words.
        destructive: true,
        idempotent: true,
        schema: schema_update_block,
        run: run_update_block,
    },
];

/// The catalog as `tools/list` answers it. `read_only` (`utopia mcp --read-only`, MC-2)
/// removes the write tools for the life of that process; the access file never does
/// (design §5).
pub fn list(read_only: bool) -> Vec<Value> {
    CATALOG
        .iter()
        .filter(|t| !(read_only && t.kind == OpKind::Write))
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": (t.schema)(),
                "annotations": {
                    "readOnlyHint": t.kind == OpKind::Read,
                    "destructiveHint": t.destructive,
                    "idempotentHint": t.idempotent,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

/// Run one `tools/call`. A tool that is not in the catalog is the protocol error (`-32602`,
/// design §3.2); every other failure, including the access level and a missing block, is a
/// text for the model.
pub fn call(
    backend: &dyn Backend,
    read_only: bool,
    name: &str,
    arguments: &Value,
) -> Result<String, ToolError> {
    let Some(tool) = CATALOG.iter().find(|t| t.name == name) else {
        return Err(ToolError::Unknown);
    };
    // MC-2: `--read-only` lowers the level for this process and cannot raise it. A write
    // tool is refused here, before any store is opened.
    if read_only && tool.kind == OpKind::Write {
        return Err(ToolError::Failed(
            "This client runs in read-only mode (utopia mcp --read-only). Nothing was \
             changed."
                .to_string(),
        ));
    }
    let empty = Map::new();
    let map = match arguments {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => {
            return Err(ToolError::Failed(
                "The arguments of a tool call must be a JSON object.".to_string(),
            ))
        }
    };
    (tool.run)(backend, &Args { map })
}

// ---------------------------------------------------------------------------------------
// Arguments

struct Args<'a> {
    map: &'a Map<String, Value>,
}

impl Args<'_> {
    fn string(&self, key: &str) -> Result<String, ToolError> {
        match self.map.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.trim().to_string()),
            Some(Value::String(_)) => {
                Err(ToolError::Failed(format!("The `{key}` argument is empty.")))
            }
            Some(other) => Err(ToolError::Failed(format!(
                "The `{key}` argument must be a string, not {other}."
            ))),
            None => Err(ToolError::Failed(format!(
                "The `{key}` argument is required."
            ))),
        }
    }

    /// A text argument that keeps its whitespace. The markdown of `append_blocks` is
    /// structure: `parse_outline` reads relative indentation as nesting, so trimming the
    /// first line's indentation — the only one `trim` can reach — would turn siblings into a
    /// parent and its child (MC-8). Emptiness is still judged without the whitespace.
    fn text(&self, key: &str) -> Result<String, ToolError> {
        match self.map.get(key) {
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
            Some(Value::String(_)) => {
                Err(ToolError::Failed(format!("The `{key}` argument is empty.")))
            }
            Some(other) => Err(ToolError::Failed(format!(
                "The `{key}` argument must be a string, not {other}."
            ))),
            None => Err(ToolError::Failed(format!(
                "The `{key}` argument is required."
            ))),
        }
    }

    /// An optional string argument: absent and `null` both mean "not given".
    fn optional_string(&self, key: &str) -> Result<Option<String>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
            Some(Value::String(_)) => {
                Err(ToolError::Failed(format!("The `{key}` argument is empty.")))
            }
            Some(other) => Err(ToolError::Failed(format!(
                "The `{key}` argument must be a string, not {other}."
            ))),
        }
    }

    fn boolean(&self, key: &str, default: bool) -> Result<bool, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(Value::Bool(b)) => Ok(*b),
            Some(other) => Err(ToolError::Failed(format!(
                "The `{key}` argument must be true or false, not {other}."
            ))),
        }
    }

    /// An optional true/false argument: absent and `null` both mean "not given".
    fn optional_boolean(&self, key: &str) -> Result<Option<bool>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(other) => Err(ToolError::Failed(format!(
                "The `{key}` argument must be true or false, not {other}."
            ))),
        }
    }

    /// An optional text argument that keeps its whitespace, like [`Args::text`]. An empty
    /// one is refused: emptying a block is not something these tools do, and a model that
    /// meant to write a word should learn that it did not, not lose the block's text.
    fn optional_text(&self, key: &str) -> Result<Option<String>, ToolError> {
        match self.map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
            Some(Value::String(_)) => Err(ToolError::Failed(format!(
                "The `{key}` argument is empty: send the block's new text, or leave `{key}` \
                 out to keep the text it has."
            ))),
            Some(other) => Err(ToolError::Failed(format!(
                "The `{key}` argument must be a string, not {other}."
            ))),
        }
    }

    /// The result limit of a search or a query. Bounded, so one call cannot ask for a list
    /// that is certain to be cut (§4.2).
    fn limit(&self, default: usize) -> Result<usize, ToolError> {
        match self.map.get("limit") {
            None | Some(Value::Null) => Ok(default),
            Some(Value::Number(n)) => match n.as_u64() {
                Some(n @ 1..=100) => Ok(n as usize),
                _ => Err(ToolError::Failed(
                    "The `limit` argument must be a whole number from 1 to 100.".to_string(),
                )),
            },
            Some(_) => Err(ToolError::Failed(
                "The `limit` argument must be a whole number from 1 to 100.".to_string(),
            )),
        }
    }
}

/// The `limit` both `search` and `run_query` use when the argument is absent: the same 20
/// that [`Op::Search`] and [`Op::Query`] default to.
const DEFAULT_LIMIT: usize = 20;

// ---------------------------------------------------------------------------------------
// Schemas

fn limit_property() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 100,
        "description": "How many results to return at most. Default 20.",
    })
}

fn schema_search() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {"type": "string", "description": "The text to search for."},
            "limit": limit_property(),
        },
        "required": ["query"],
        "additionalProperties": false,
    })
}

fn schema_list_pages() -> Value {
    json!({
        "type": "object",
        "properties": {
            "include_daily": {
                "type": "boolean",
                "description": "Also list the daily notes. Default false.",
            },
        },
        "additionalProperties": false,
    })
}

fn after_property() -> Value {
    json!({
        "type": "string",
        "description": "Continue after this block id: only the blocks that follow it are \
                        returned. A cut result ends with the id to pass here.",
    })
}

fn schema_read_page() -> Value {
    json!({
        "type": "object",
        "properties": {
            "page": {
                "type": "string",
                "description": "A page title or alias, `today`, `yesterday`, or a date as \
                                YYYY-MM-DD.",
            },
            "after": after_property(),
        },
        "required": ["page"],
        "additionalProperties": false,
    })
}

fn schema_read_block() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "The block's id, as an outline line's {id:…} marker gives it.",
            },
            "after": after_property(),
        },
        "required": ["id"],
        "additionalProperties": false,
    })
}

fn schema_get_backlinks() -> Value {
    json!({
        "type": "object",
        "properties": {
            "page": {"type": "string", "description": "A page title or alias."},
        },
        "required": ["page"],
        "additionalProperties": false,
    })
}

fn schema_run_query() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "The query, in query-block syntax, without the {{query: …}} \
                                wrapper.",
            },
            "limit": limit_property(),
        },
        "required": ["query"],
        "additionalProperties": false,
    })
}

fn schema_append_blocks() -> Value {
    json!({
        "type": "object",
        "properties": {
            "page": {
                "type": "string",
                "description": "The page to append to: a title or alias, `today`, `yesterday`, \
                                or a date as YYYY-MM-DD.",
            },
            "markdown": {
                "type": "string",
                "description": "The blocks to append, one line each, indented two spaces per \
                                level. `- ` starts a block; the rest of the line is its text. \
                                The {id:…} markers of read_page are ignored.",
            },
            "parent": {
                "type": "string",
                "description": "The id of an existing block to append under. Absent appends at \
                                the end of the page.",
            },
            "create": {
                "type": "boolean",
                "description": "Create the page when it does not exist. Default false.",
            },
        },
        "required": ["page", "markdown"],
        "additionalProperties": false,
    })
}

fn schema_update_block() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "The block's id, as an outline line's {id:…} marker gives it.",
            },
            "text": {
                "type": "string",
                "description": "The block's new text, in Utopia markup ([[Page]], #tag, \
                                ((block-id)), key:: value). Leave it out to keep the text the \
                                block has.",
            },
            "kind": {
                "type": "string",
                "enum": ["bullet", "numbered", "todo", "quote", "code", "heading",
                         "h1", "h2", "h3"],
                "description": "The block's new kind. `heading` keeps the level of a \
                                heading and makes any other block a heading 1; h1, h2 and h3 \
                                pick a level. Leave it out to keep the kind.",
            },
            "checked": {
                "type": "boolean",
                "description": "Whether a to-do is done. Only a to-do has this state: send \
                                kind \"todo\" in the same call if the block is not one yet. \
                                Leave it out to keep it.",
            },
            "if_rev": {
                "type": "string",
                "description": "The rev that read_block gave this block. When the block's \
                                text changed since, the call fails and writes nothing, so a \
                                concurrent edit is not overwritten: read the block again and \
                                send the text and rev it gives you. Leave it out to write \
                                regardless.",
            },
        },
        "required": ["id"],
        "additionalProperties": false,
    })
}

// ---------------------------------------------------------------------------------------
// The six read tools

fn run_search(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let q = args.string("query")?;
    let limit = args.limit(DEFAULT_LIMIT)?;
    let body = backend.call(Op::Search { q, limit })?;
    Ok(render_search(&body))
}

fn run_list_pages(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let daily = args.boolean("include_daily", false)?;
    let body = backend.call(Op::ListPages { daily })?;
    Ok(render_list_pages(&body))
}

fn run_read_page(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let page = args.string("page")?;
    let after = args.optional_string("after")?;
    let body = match day_of(&page) {
        // A day is a page like any other, but its title depends on the graph's date
        // format, so ask for it by date (Op::Journal resolves it, and never creates it).
        Some(day) => match backend.call(Op::Journal { date: iso(day) }) {
            Ok(body) => body,
            // FR-1.2, MC-7: a day without a page is an empty outline, not an error. The
            // op answers 404 for that day; the HTTP route keeps that answer.
            Err(BackendError::Failed { status: 404, .. }) => return Ok(String::new()),
            Err(e) => return Err(e.into()),
        },
        None => backend.call(Op::GetPage { title: page })?,
    };
    render_outline(&body, after.as_deref())
}

fn run_read_block(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let id = args.string("id")?;
    let after = args.optional_string("after")?;
    let body = backend.call(Op::GetBlock {
        id: id.clone(),
        subtree: true,
    })?;
    let text = body["text"].as_str().unwrap_or_default();
    // MC-10: the `if_rev` of `update_block` is this same hash of this same text.
    let rev = block_rev(text);
    let mut doc = Doc::outline(&format!("rev: {rev}\n\n"));
    let mut walk = Cursor::new(after.as_deref());
    outline_node(&body, 0, &mut walk, &mut doc);
    walk.check()?;
    Ok(doc.finish())
}

fn run_get_backlinks(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let page = args.string("page")?;
    let body = backend.call(Op::Backlinks { title: page })?;
    Ok(render_backlinks(&body))
}

fn run_run_query(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let query = args.string("query")?;
    let limit = args.limit(DEFAULT_LIMIT)?;
    let body = backend.call(Op::Query { query, limit })?;
    Ok(render_query(&body))
}

fn run_append_blocks(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let page = args.string("page")?;
    // Not `string()`: the outline's indentation is its nesting (MC-8).
    let markdown = args.text("markdown")?;
    let parent = args.optional_string("parent")?;
    let create = args.boolean("create", false)?;
    // A day goes to the store as its ISO date: the store resolves a date title to that day's
    // page and makes it on this first write (FR-1.2), whatever the graph's date format is.
    let title = day_of(&page).map(iso).unwrap_or(page);
    let body = backend.call(Op::AppendOutline {
        title,
        markdown,
        parent,
        create,
    })?;
    Ok(render_append(&body))
}

fn run_update_block(backend: &dyn Backend, args: &Args<'_>) -> Result<String, ToolError> {
    let id = args.string("id")?;
    let text = args.optional_text("text")?;
    let kind = args.optional_string("kind")?;
    let checked = args.optional_boolean("checked")?;
    let if_rev = args.optional_string("if_rev")?;
    if text.is_none() && kind.is_none() && checked.is_none() {
        return Err(ToolError::Failed(
            "Send `text`, `kind` or `checked`: there is nothing for update_block to change."
                .to_string(),
        ));
    }
    let body = backend.call(Op::UpdateBlock {
        id,
        text,
        kind,
        checked,
        if_rev,
    })?;
    Ok(render_update(&body))
}

/// The day a `page` argument names, if it names one: `today`, `yesterday` or an ISO date.
/// Every other reading is a page title or an alias, through the graph's resolver.
fn day_of(page: &str) -> Option<NaiveDate> {
    if let Some(d) = parse_iso(page) {
        return Some(d);
    }
    let lower = page.to_ascii_lowercase();
    match lower.as_str() {
        "today" | "yesterday" => relative_date(&lower, chrono::Local::now().date_naive()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------
// One block line

/// The `- ` line of one block, mirroring `utopia_core::markdown`'s export: the `[ ] `,
/// `## ` and `> ` prefix is how a reader sees the kind. `append_blocks` reads such a line as
/// a bullet whose text starts with that prefix (`markdown::parse_outline` has no kind
/// parsing), so a kind does not round-trip yet (design §10).
fn block_line(node: &Value, depth: usize, text: &str) -> String {
    let pad = "  ".repeat(depth);
    let prefix = match node["kind"].as_str() {
        Some("todo") if node["checked"] == Value::Bool(true) => "[x] ",
        Some("todo") => "[ ] ",
        Some("quote") => "> ",
        Some("heading") => {
            let level = node["heading"].as_u64().unwrap_or(1).clamp(1, 3) as usize;
            return heading_line(&pad, level, text);
        }
        _ => "",
    };
    let mut s = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i == 0 {
            s.push_str(&format!("{pad}- {prefix}{line}"));
        } else {
            s.push('\n');
            s.push_str(&pad);
            s.push_str("  ");
            s.push_str(line);
        }
    }
    s.push('\n');
    s
}

fn heading_line(pad: &str, level: usize, text: &str) -> String {
    let mut s = format!("{pad}- {} {text}", "#".repeat(level));
    s.push('\n');
    s
}

/// Where a continuation starts: after this block id the blocks are shown, before it they
/// are skipped (MC-7). Blocks are in document order, so the id of the last block a result
/// shows is the cursor that returns the ones it left out: nothing is repeated and nothing
/// is skipped, however deep the cut falls.
struct Cursor<'a> {
    after: Option<&'a str>,
    passed: bool,
    found: bool,
}

impl<'a> Cursor<'a> {
    fn new(after: Option<&'a str>) -> Self {
        Self {
            after,
            passed: after.is_none(),
            found: after.is_none(),
        }
    }

    /// A cursor that names a block this result does not hold is a mistake, not an empty
    /// page: say so instead of answering nothing.
    fn check(&self) -> Result<(), ToolError> {
        if self.found {
            return Ok(());
        }
        // `after` is only `None` when nothing was asked for, and then `found` is true.
        Err(ToolError::Failed(format!(
            "There is no block \"{}\" in this result, so there is nothing to continue \
             after.",
            self.after.unwrap_or_default()
        )))
    }
}

/// Append one block and then its children, depth first: the page's outline in order.
fn outline_node(node: &Value, depth: usize, walk: &mut Cursor<'_>, doc: &mut Doc) {
    let id = node["id"].as_str().unwrap_or_default();
    if walk.passed {
        let text = node["text"].as_str().unwrap_or_default();
        doc.block(block_line(node, depth, text), id);
    } else if walk.after == Some(id) {
        // This is the block the previous result ended at: skip its line, keep its children.
        walk.passed = true;
        walk.found = true;
    }
    for child in node["children"].as_array().into_iter().flatten() {
        outline_node(child, depth + 1, walk, doc);
    }
}

fn render_outline(page: &Value, after: Option<&str>) -> Result<String, ToolError> {
    let mut doc = Doc::outline("");
    let mut walk = Cursor::new(after);
    for node in page["blocks"].as_array().into_iter().flatten() {
        outline_node(node, 0, &mut walk, &mut doc);
    }
    walk.check()?;
    Ok(doc.finish())
}

// ---------------------------------------------------------------------------------------
// Lists

fn render_search(body: &Value) -> String {
    let mut doc = Doc::default();
    let pages = body["pages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let blocks = body["blocks"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if pages.is_empty() && blocks.is_empty() {
        doc.line("(no matches)");
        return doc.finish();
    }
    if !pages.is_empty() {
        doc.line("## Pages");
        for p in pages {
            let title = one_line(p["title"].as_str().unwrap_or_default());
            match p["alias"].as_str() {
                Some(alias) => doc.line(format!("- {title} (alias: {})", one_line(alias))),
                None => doc.line(format!("- {title}")),
            }
        }
    }
    if !blocks.is_empty() {
        doc.line("## Blocks");
        for b in blocks {
            let page = one_line(b["page_title"].as_str().unwrap_or_default());
            let text = one_line(b["text"].as_str().unwrap_or_default());
            doc.block(
                format!("- {page}: {text}\n"),
                b["block"].as_str().unwrap_or(""),
            );
        }
    }
    doc.finish()
}

fn render_list_pages(body: &Value) -> String {
    let mut doc = Doc::default();
    let pages = body["pages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if pages.is_empty() {
        doc.line("(no pages)");
        return doc.finish();
    }
    for p in pages {
        let title = one_line(p["title"].as_str().unwrap_or_default());
        // `PageRow.kind` serializes as `"Daily"` (the plain `PageKind` serde shape), so the
        // day is read from `daily_date`: the same fact, and not a spelling to keep in step.
        if p["daily_date"].is_string() {
            doc.line(format!("- {title} (daily note)"));
        } else {
            doc.line(format!("- {title}"));
        }
    }
    doc.finish()
}

fn render_backlinks(body: &Value) -> String {
    let mut doc = Doc::default();
    let groups = body["groups"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    for g in groups {
        let hits = g["hits"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if hits.is_empty() {
            continue;
        }
        doc.line(format!(
            "## {}",
            one_line(g["page"]["title"].as_str().unwrap_or_default())
        ));
        for h in hits {
            let mut parts: Vec<String> = h["breadcrumb"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c.get(1).and_then(Value::as_str))
                .map(one_line)
                .collect();
            parts.push(one_line(h["text"].as_str().unwrap_or_default()));
            doc.block(
                format!("- {}\n", parts.join(" › ")),
                h["block"].as_str().unwrap_or(""),
            );
        }
    }
    if doc.is_empty() {
        doc.line("(no backlinks)");
    }
    doc.finish()
}

fn render_query(body: &Value) -> String {
    let mut doc = Doc::default();
    let hits = body["hits"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if hits.is_empty() {
        doc.line("(no matches)");
        return doc.finish();
    }
    // One heading for each page, in the order the query returned them.
    let mut grouped: Vec<(String, Vec<&Value>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for h in hits {
        let page = one_line(h["page_title"].as_str().unwrap_or_default());
        match at.get(&page) {
            Some(i) => grouped[*i].1.push(h),
            None => {
                at.insert(page.clone(), grouped.len());
                grouped.push((page, vec![h]));
            }
        }
    }
    for (page, hits) in grouped {
        doc.line(format!("## {page}"));
        for h in hits {
            doc.block(
                format!("- {}\n", one_line(h["text"].as_str().unwrap_or_default())),
                h["block"].as_str().unwrap_or(""),
            );
        }
    }
    let total = body["total"].as_u64().unwrap_or(0) as usize;
    if total > hits.len() {
        doc.line(format!(
            "({} more match{}; call run_query with a larger limit.)",
            total - hits.len(),
            if total - hits.len() == 1 { "" } else { "es" }
        ));
    }
    doc.finish()
}

/// What `append_blocks` answers: how much landed, and the ids at either end, so an agent can
/// read the new blocks back without reading the whole page again.
fn render_append(body: &Value) -> String {
    let title = one_line(body["title"].as_str().unwrap_or_default());
    let blocks = body["blocks"].as_u64().unwrap_or(0);
    let first = body["first"].as_str().unwrap_or_default();
    let last = body["last"].as_str().unwrap_or_default();
    let mut doc = Doc::default();
    doc.line(format!(
        "Appended {blocks} block{} to \"{title}\".",
        if blocks == 1 { "" } else { "s" }
    ));
    match (first.is_empty(), last.is_empty() || last == first) {
        (false, false) => doc.line(format!(
            "The first new block is {first} and the last is {last}; read_block takes either id."
        )),
        (false, true) => doc.line(format!(
            "The new block is {first}; read_block takes that id."
        )),
        _ => {}
    }
    doc.finish()
}

/// What `update_block` answers: the block as the page now shows it, and the `rev` that a
/// second call can send back as `if_rev` without reading the block again (MC-10).
fn render_update(body: &Value) -> String {
    let id = body["id"].as_str().unwrap_or_default();
    let text = body["text"].as_str().unwrap_or_default();
    let mut doc = Doc::outline("Updated the block. It now reads:\n");
    doc.block(block_line(body, 0, text), id);
    doc.line(format!("rev: {}", block_rev(text)));
    doc.line("Send that rev as `if_rev` to change this block again without reading it first.");
    doc.finish()
}

/// One line: block text can hold newlines, and a list entry cannot.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------------------
// The size budget

/// One rendered line of a result. `block` is the id the line came from, if it came from a
/// block, so the cut can fall between blocks and name the first one it left out.
struct Entry {
    text: String,
    block: Option<String>,
}

/// A result under construction: lines, a prefix, and the cut (§4.2).
#[derive(Default)]
struct Doc {
    prefix: String,
    entries: Vec<Entry>,
    /// Whether its entries are blocks in document order, so a cut can name the id to
    /// continue after ([`Cursor`]). Only the two outline tools set this.
    continues: bool,
}

impl Doc {
    /// An outline (`read_page`, `read_block`): blocks in document order, continuable.
    fn outline(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_string(),
            entries: Vec::new(),
            continues: true,
        }
    }

    /// A line that is not a block (a heading, a note).
    fn line(&mut self, text: impl Into<String>) {
        let mut text = text.into();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        self.entries.push(Entry { text, block: None });
    }

    /// A block's line or lines. `text` ends with a newline; the marker is added by
    /// [`Doc::finish`], so a cut never leaves half a marker behind.
    fn block(&mut self, text: String, id: &str) {
        self.entries.push(Entry {
            text,
            block: Some(id.to_string()),
        });
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn finish(self) -> String {
        fit(&self.prefix, self.entries, self.continues)
    }
}

/// Room left for the line that closes a cut result.
const RESERVE: usize = 256;

/// Cut the result at [`BUDGET`] bytes of UTF-8 at a block boundary, and say what was left
/// out and how to read it (§4.2, MC-7). An outline is cut between two blocks and names the
/// id to continue after; a list is cut between two results and says how many were left.
/// A block bigger than the whole budget is the one cut that is not at a block boundary: it
/// keeps its head and its complete id marker, and the result says that it is cut short.
fn fit(prefix: &str, entries: Vec<Entry>, continues: bool) -> String {
    let mut out = String::from(prefix);
    let room = BUDGET.saturating_sub(prefix.len() + RESERVE);
    let mut kept = 0;
    for e in &entries {
        let full = compose(e, None);
        if out.len() - prefix.len() + full.len() <= room {
            out.push_str(&full);
            kept += 1;
        } else {
            break;
        }
    }
    let mut cut_short: Option<&Entry> = None;
    if kept == 0 && !entries.is_empty() {
        out.push_str(&compose(&entries[0], Some(room)));
        cut_short = Some(&entries[0]);
        kept = 1;
    }
    let left = &entries[kept..];
    if left.is_empty() && cut_short.is_none() {
        return out;
    }
    // The last block this result shows. In an outline it is the entry right before the
    // first one left out, so passing it back resumes exactly there.
    let cursor = entries[..kept]
        .iter()
        .rev()
        .find_map(|e| e.block.as_deref());
    let blocks = left.iter().filter(|e| e.block.is_some()).count();
    let first_left = left.iter().find_map(|e| e.block.as_deref());

    out.push('\n');
    let mut note = String::from("(24 KB budget: ");
    if let Some(entry) = cut_short {
        match entry.block.as_deref() {
            Some(id) => note.push_str(&format!(
                "block \"{id}\" is longer than one result, so its text is cut short"
            )),
            None => note.push_str("the first line is cut short"),
        }
        if !left.is_empty() {
            note.push_str(" and ");
        }
    }
    if !left.is_empty() {
        if continues && blocks > 0 {
            if let Some(cursor) = cursor {
                note.push_str(&format!(
                    "{blocks} block{} not shown. Continue with after: \"{cursor}\".",
                    if blocks == 1 { "" } else { "s" }
                ));
            }
        } else if let Some(first) = first_left {
            note.push_str(&format!(
                "{blocks} block{} not shown. Read one of them with read_block(\"{first}\" \
                 to see the rest of that subtree).",
                if blocks == 1 { "" } else { "s" }
            ));
        } else {
            note.push_str(&format!(
                "{} line{} not shown. Ask for fewer results.",
                left.len(),
                if left.len() == 1 { "" } else { "s" }
            ));
        }
    }
    note.push_str(")\n");
    out.push_str(&note);
    out
}

/// One entry, as it is written. `max` cuts the entry's own text to that many bytes, keeping
/// the id marker whole: the marker is what lets a reader name the block it partly sees.
fn compose(e: &Entry, max: Option<usize>) -> String {
    let body = e.text.trim_end_matches('\n');
    match &e.block {
        Some(id) => {
            let marker = format!(" {MARKER_PREFIX}{id}{MARKER_SUFFIX}");
            match max {
                None => format!("{body}{marker}\n"),
                Some(max) => {
                    let room = max.saturating_sub(marker.len() + 2);
                    let at = floor_char_boundary(body, room);
                    format!("{} …{marker}\n", body[..at].trim_end())
                }
            }
        }
        None => match max {
            None => e.text.clone(),
            Some(max) => {
                let at = floor_char_boundary(body, max.saturating_sub(2));
                format!("{} …\n", body[..at].trim_end())
            }
        },
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, text: &str) -> String {
        let mut doc = Doc::default();
        doc.block(format!("- {text}\n"), id);
        doc.finish()
    }

    /// MC-7: one line per block, indented two spaces per level, each ending with the
    /// block's marker, and kinds as the export writes them.
    #[test]
    fn mc_7_the_outline_has_one_marked_line_for_each_block() {
        let page = json!({
            "blocks": [
                {"id": "01A", "text": "root", "kind": "bullet", "heading": null,
                 "checked": null, "children": [
                     {"id": "01B", "text": "kid", "kind": "todo", "heading": null,
                      "checked": true, "children": []},
                     {"id": "01C", "text": "second", "kind": "heading", "heading": 2,
                      "checked": null, "children": []}
                 ]},
            ]
        });
        assert_eq!(
            render_outline(&page, None).unwrap(),
            "- root {id:01A}\n  - [x] kid {id:01B}\n  - ## second {id:01C}\n"
        );
        assert_eq!(render_outline(&json!({"blocks": []}), None).unwrap(), "");
    }

    /// A block whose text runs over several lines keeps one marker, at the end.
    #[test]
    fn mc_7_a_multi_line_block_gets_one_marker() {
        let page = json!({
            "blocks": [{"id": "01A", "text": "one\ntwo", "kind": "bullet", "heading": null,
                        "checked": null, "children": []}]
        });
        assert_eq!(
            render_outline(&page, None).unwrap(),
            "- one\n  two {id:01A}\n"
        );
    }

    /// MC-7: a day with no page is an empty outline, and `read_page` says so instead of
    /// passing the op's 404 on.
    #[test]
    fn mc_7_a_day_without_a_page_is_an_empty_outline() {
        struct Missing;
        impl Backend for Missing {
            fn call(&self, op: Op) -> Result<Value, BackendError> {
                match op {
                    Op::Journal { .. } => Err(BackendError::Failed {
                        status: 404,
                        message: "no journal page".into(),
                    }),
                    _ => panic!("expected a journal op"),
                }
            }
        }
        let args = json!({"page": "today"});
        let map = args.as_object().unwrap();
        assert_eq!(
            run_read_page(&Missing, &Args { map }).unwrap(),
            "",
            "an empty outline, not an error"
        );
    }

    /// MC-7: a large page is cut at the budget, at a block boundary, and names the number
    /// of blocks left out and the call that reads them.
    #[test]
    fn mc_7_a_large_result_is_cut_at_the_budget() {
        let blocks: Vec<Value> = (0..2000)
            .map(|i| {
                json!({"id": format!("01{i:024}"), "text": "x".repeat(40), "kind": "bullet",
                       "heading": null, "checked": null, "children": []})
            })
            .collect();
        let page = json!({"blocks": blocks});
        let text = render_outline(&page, None).unwrap();
        assert!(text.len() <= BUDGET, "{} bytes", text.len());
        assert!(
            text.len() > BUDGET - 1024,
            "fills the budget: {}",
            text.len()
        );
        assert!(text.contains("not shown"), "{text}");
        let last = text.lines().filter(|l| l.starts_with("- ")).count();
        let missing = 2000 - last;
        assert!(
            text.contains(&format!("{missing} blocks not shown")),
            "{missing} missing, text ends: {}",
            text.lines().last().unwrap_or_default()
        );
        // It names the continuation: the id of the last block it shows.
        let cursor = text
            .rsplit_once("after: \"")
            .expect("the continuation")
            .1
            .split('"')
            .next()
            .expect("an id");
        let shown = text
            .lines()
            .take(last)
            .last()
            .and_then(|l| l.rsplit_once("{id:"))
            .map(|(_, rest)| rest.trim_end_matches('}'))
            .expect("a last line");
        assert_eq!(cursor, shown, "the cursor is the last block shown");
        // Every kept line is whole: a marker at the end, text of the seeded length.
        for line in text.lines().take(last) {
            assert!(line.ends_with('}'), "{line}");
        }
    }

    /// MC-7: the one cut that is not at a block boundary keeps the id marker and says so.
    #[test]
    fn a_cut_never_splits_a_marker_or_a_character() {
        let mut doc = Doc::outline("");
        doc.block(format!("- {}\n", "é".repeat(BUDGET)), "01A");
        let text = fit("", doc.entries, true);
        assert!(text.len() <= BUDGET, "{}", text.len());
        assert!(text.ends_with("\n"));
        // The head is valid UTF-8 (it is a &str), and the cut landed on a boundary.
        assert!(text.starts_with("- ééé"));
        assert!(
            text.lines().next().unwrap().ends_with("{id:01A}"),
            "{}",
            text.lines().next().unwrap()
        );
        assert_eq!(text.matches('…').count(), 1, "one cut marker: {text:.120}");
        assert!(text.contains("cut short"), "{}", &text[text.len() - 120..]);
        assert!(
            !text.contains("after:"),
            "a block that big has no continuation"
        );
    }

    /// `search` keeps pages and blocks apart, and a block line carries its id.
    #[test]
    fn mc_7_search_renders_pages_and_blocks() {
        let body = json!({
            "pages": [{"page": "p1", "title": "Project X", "alias": "Proj", "daily": false, "score": 3}],
            "blocks": [{"block": "01B", "page": "p1", "page_title": "Project X", "text": "see [[Y]]", "anchors": []}],
        });
        let text = render_search(&body);
        assert!(
            text.contains("## Pages\n- Project X (alias: Proj)\n"),
            "{text}"
        );
        assert!(
            text.contains("## Blocks\n- Project X: see [[Y]] {id:01B}\n"),
            "{text}"
        );
    }

    #[test]
    fn mc_7_arguments_are_checked_before_the_backend_runs() {
        struct Never;
        impl Backend for Never {
            fn call(&self, _: Op) -> Result<Value, BackendError> {
                panic!("invalid arguments must not reach the backend")
            }
        }
        for (tool, args) in [
            ("search", json!({})),
            ("search", json!({"query": "x", "limit": 0})),
            ("search", json!({"query": "x", "limit": "10"})),
            ("list_pages", json!({"include_daily": "yes"})),
            ("read_page", json!({"page": 3})),
            ("read_block", json!({"id": ""})),
            ("get_backlinks", json!({})),
            ("run_query", json!({"query": "x", "limit": 101})),
            ("append_blocks", json!({"page": "A"})),
            ("append_blocks", json!({"page": "A", "markdown": "   "})),
            (
                "append_blocks",
                json!({"page": "A", "markdown": "- x", "create": "yes"}),
            ),
            (
                "append_blocks",
                json!({"page": "A", "markdown": "- x", "parent": 7}),
            ),
        ] {
            let err = call(&Never, false, tool, &args).unwrap_err();
            assert!(
                matches!(err, ToolError::Failed(_)),
                "{tool} {args}: {err:?}"
            );
        }
        assert_eq!(
            call(&Never, false, "nope", &json!({})).unwrap_err(),
            ToolError::Unknown
        );
        assert_eq!(
            call(&Never, false, "search", &json!([])).unwrap_err(),
            ToolError::Failed("The arguments of a tool call must be a JSON object.".into())
        );
    }

    /// MC-8: the Markdown argument reaches the parser exactly as it was sent. Trimming it
    /// takes the indentation off the first line only, which turns two siblings into a parent
    /// and its child.
    #[test]
    fn mc_8_the_markdown_argument_keeps_its_indentation() {
        struct Captured(std::sync::Mutex<Option<Op>>);
        impl Backend for Captured {
            fn call(&self, op: Op) -> Result<Value, BackendError> {
                *self.0.lock().expect("the captured op") = Some(op);
                Ok(json!({"title": "A", "blocks": 2, "first": "01A", "last": "01B"}))
            }
        }
        let backend = Captured(std::sync::Mutex::new(None));
        let markdown = "  - one\n  - two\n";
        call(
            &backend,
            false,
            "append_blocks",
            &json!({"page": "A", "markdown": markdown}),
        )
        .unwrap();
        match backend.0.lock().expect("the captured op").clone() {
            Some(Op::AppendOutline { markdown: sent, .. }) => {
                assert_eq!(sent, markdown, "the outline as it was written");
            }
            other => panic!("expected an append_outline op, got {other:?}"),
        }
        // Whitespace-only text is still empty.
        let err = call(
            &backend,
            false,
            "append_blocks",
            &json!({"page": "A", "markdown": " \n  "}),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ToolError::Failed(m) if m.contains("`markdown`")),
            "{err:?}"
        );
    }

    /// MC-11: the catalog and the guide are constants. Running a tool on a graph never
    /// changes them, and neither does a flag other than `--read-only`.
    #[test]
    fn mc_11_the_catalog_is_a_constant() {
        let a = list(false);
        assert_eq!(a, list(false));
        assert_eq!(a.len(), 8, "the six read tools and the two write tools");
        for t in &a {
            let name = t["name"].as_str().unwrap();
            // Design §4.1: `destructiveHint` and `openWorldHint` default to true in the
            // 2026-07-28 schema, so every tool sets all four hints.
            let want = match name {
                "append_blocks" => json!({"readOnlyHint": false, "destructiveHint": false,
                                          "idempotentHint": false, "openWorldHint": false}),
                "update_block" => json!({"readOnlyHint": false, "destructiveHint": true,
                                         "idempotentHint": true, "openWorldHint": false}),
                _ => json!({"readOnlyHint": true, "destructiveHint": false,
                            "idempotentHint": true, "openWorldHint": false}),
            };
            assert_eq!(t["annotations"], want, "{name}");
            assert_eq!(t["inputSchema"]["type"], "object", "{name}");
        }
        // MC-2: `--read-only` removes the write tools for this process.
        let readonly = list(true);
        assert_eq!(readonly.len(), 6);
        assert!(
            readonly
                .iter()
                .all(|t| t["annotations"]["readOnlyHint"] == json!(true)),
            "no write tool in a read-only list"
        );
        // The description states the limit the op enforces (MC-8).
        let append = a.iter().find(|t| t["name"] == "append_blocks").unwrap();
        assert!(
            append["description"]
                .as_str()
                .unwrap()
                .contains(&utopia_api::MAX_OUTLINE_BLOCKS.to_string()),
            "{}",
            append["description"]
        );
        assert!(!INSTRUCTIONS.is_empty());
        assert!(INSTRUCTIONS.contains("append_blocks"), "the guide names it");
        assert!(
            INSTRUCTIONS.contains("update_block"),
            "and the other write tool"
        );
        assert!(
            INSTRUCTIONS.contains("if_rev"),
            "and how it avoids an overwrite"
        );
    }

    /// MC-8: what `append_blocks` answers: how much landed and the ids at either end, and
    /// one block reads as one block.
    #[test]
    fn mc_8_the_append_result_names_what_landed() {
        let body = json!({"title": "Project X", "blocks": 3,
                          "first": "01A", "last": "01C"});
        assert_eq!(
            render_append(&body),
            "Appended 3 blocks to \"Project X\".\n\
             The first new block is 01A and the last is 01C; read_block takes either id.\n"
        );
        let one = json!({"title": "A day", "blocks": 1, "first": "01A", "last": "01A"});
        assert_eq!(
            render_append(&one),
            "Appended 1 block to \"A day\".\n\
             The new block is 01A; read_block takes that id.\n"
        );
    }

    /// MC-8, MC-10: what `update_block` answers: the block as the page now shows it, then
    /// the rev a second call can send back as `if_rev` without reading the block again.
    #[test]
    fn mc_8_the_update_result_carries_the_block_and_its_rev() {
        let body = json!({"id": "01A", "text": "buy milk", "kind": "todo",
                          "heading": null, "checked": true});
        let text = render_update(&body);
        assert_eq!(
            text.lines().next(),
            Some("Updated the block. It now reads:")
        );
        assert!(text.contains("\n- [x] buy milk {id:01A}\n"), "{text}");
        assert!(
            text.contains(&format!("\nrev: {}\n", block_rev("buy milk"))),
            "{text}"
        );
        assert!(text.contains("if_rev"), "{text}");

        let heading = json!({"id": "01B", "text": "Plan", "kind": "heading",
                             "heading": 2, "checked": null});
        assert!(
            render_update(&heading).contains("\n- ## Plan {id:01B}\n"),
            "a heading keeps the outline format's prefix"
        );
    }

    #[test]
    fn the_marker_is_not_utopia_markup() {
        // `((id))` is a block reference and `[[id]]` a page link; a single `{` is literal,
        // so the marker an agent sends back makes no reference.
        let line = format!("- text {MARKER_PREFIX}01J8Z3K9V2{MARKER_SUFFIX}");
        let refs = utopia_core::inline::extract_refs(&line);
        assert!(refs.is_empty(), "{refs:?}");
        assert_eq!(block("01A", "text"), "- text {id:01A}\n");
    }
}
