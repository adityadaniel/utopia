//! SQLite schema (PRD §10.2) and migrations.
//!
//! Additions to the PRD schema, all local-only:
//! - `page_update`: per-page Loro updates appended on every commit and folded into
//!   `page_doc.snapshot` periodically, so a keystroke never rewrites a whole snapshot
//!   while the shard and its projections still commit in one transaction (§11.2, §13.3).
//! - `local_state`: device-local UI state such as collapse (FR-2.8, §12.5).

use rusqlite::Connection;

pub const SCHEMA_VERSION: i64 = 9;

const V1: &str = r#"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE page (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL CHECK (kind IN ('page','daily')),
  title       TEXT NOT NULL,
  title_key   TEXT NOT NULL,
  daily_date  TEXT,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL,
  deleted_at  INTEGER
);
CREATE UNIQUE INDEX page_title_uq ON page(title_key)
  WHERE deleted_at IS NULL AND kind = 'page';
CREATE UNIQUE INDEX page_daily_uq ON page(daily_date)
  WHERE kind = 'daily' AND deleted_at IS NULL;
CREATE INDEX page_recent ON page(updated_at DESC) WHERE deleted_at IS NULL;

CREATE TABLE block (
  id            TEXT PRIMARY KEY,
  page_id       TEXT NOT NULL REFERENCES page(id) ON DELETE CASCADE,
  parent_id     TEXT REFERENCES block(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
  position      TEXT NOT NULL,
  depth         INTEGER NOT NULL,
  text          TEXT NOT NULL DEFAULT '',
  block_type    TEXT NOT NULL DEFAULT 'bullet'
                  CHECK (block_type IN ('bullet','numbered','todo','quote','code','heading')),
  heading_level INTEGER CHECK (heading_level BETWEEN 1 AND 3),
  checked       INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  deleted_at    INTEGER
);
CREATE UNIQUE INDEX block_sibling_uq ON block(page_id, ifnull(parent_id,''), position)
  WHERE deleted_at IS NULL;
CREATE INDEX block_page   ON block(page_id, position)   WHERE deleted_at IS NULL;
CREATE INDEX block_parent ON block(parent_id, position) WHERE deleted_at IS NULL;
CREATE INDEX block_updated ON block(updated_at DESC);

CREATE TABLE ref (
  id            INTEGER PRIMARY KEY,
  src_block_id  TEXT NOT NULL REFERENCES block(id) ON DELETE CASCADE,
  src_page_id   TEXT NOT NULL,
  dst_kind      TEXT NOT NULL CHECK (dst_kind IN ('page','block')),
  dst_page_id   TEXT,
  dst_block_id  TEXT,
  dst_title_key TEXT,
  anchor_start  INTEGER NOT NULL,
  anchor_end    INTEGER NOT NULL
);
CREATE UNIQUE INDEX ref_uq ON ref(
  src_block_id, dst_kind,
  ifnull(dst_page_id,''), ifnull(dst_block_id,''), ifnull(dst_title_key,'')
);
CREATE INDEX ref_in_page  ON ref(dst_page_id, src_page_id);
CREATE INDEX ref_in_block ON ref(dst_block_id, src_page_id);
CREATE INDEX ref_dangling ON ref(dst_title_key) WHERE dst_page_id IS NULL AND dst_kind = 'page';
CREATE INDEX ref_key      ON ref(dst_title_key);
CREATE INDEX ref_src      ON ref(src_block_id);
CREATE INDEX ref_src_page ON ref(src_page_id);

CREATE TABLE alias (
  page_id   TEXT NOT NULL REFERENCES page(id) ON DELETE CASCADE,
  alias_key TEXT NOT NULL,
  alias     TEXT NOT NULL,
  PRIMARY KEY (page_id, alias_key)
);
CREATE INDEX alias_key ON alias(alias_key);

CREATE TABLE page_doc (
  page_id   TEXT PRIMARY KEY REFERENCES page(id) ON DELETE CASCADE,
  snapshot  BLOB NOT NULL,
  frontier  TEXT NOT NULL,
  bytes     INTEGER NOT NULL
);

CREATE TABLE page_update (
  seq     INTEGER PRIMARY KEY AUTOINCREMENT,
  page_id TEXT NOT NULL,
  payload BLOB NOT NULL
);
CREATE INDEX page_update_page ON page_update(page_id, seq);

CREATE TABLE outbox (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  page_id    TEXT NOT NULL,
  payload    BLOB NOT NULL,
  client_seq INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  sent_at    INTEGER
);
CREATE INDEX outbox_pending ON outbox(page_id, seq) WHERE sent_at IS NULL;

CREATE TABLE sync_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE VIRTUAL TABLE block_fts USING fts5(
  text, block_id UNINDEXED, page_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE trash (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload BLOB NOT NULL, deleted_at INTEGER NOT NULL
);

CREATE TABLE local_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

/// v2: FTS rows are keyed by the block's rowid (FTS5 cannot index `block_id`, so deleting
/// by it scanned the whole table), and a trigger drops them with their block, including
/// cascaded deletes.
const V2: &str = r#"
CREATE TRIGGER block_fts_on_delete AFTER DELETE ON block BEGIN
  DELETE FROM block_fts WHERE rowid = old.rowid;
END;
DELETE FROM block_fts;
INSERT INTO block_fts(rowid, text, block_id, page_id)
  SELECT rowid, text, id, page_id FROM block WHERE deleted_at IS NULL;
"#;

/// v3: a full (non-partial) index on `block.parent_id`. SQLite's foreign-key checks
/// cannot use the partial `block_parent` index, so deleting a block, or inserting one while
/// a deferred violation is pending, scanned the whole table.
const V3: &str = "CREATE INDEX block_parent_fk ON block(parent_id);";

/// v4: the FTS table drops `page_id` (search joins `block` by rowid), so moving a block
/// between pages (merge) never re-tokenizes its text.
const V4: &str = r#"
DROP TRIGGER block_fts_on_delete;
DROP TABLE block_fts;
CREATE VIRTUAL TABLE block_fts USING fts5(
  text, block_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER block_fts_on_delete AFTER DELETE ON block BEGIN
  DELETE FROM block_fts WHERE rowid = old.rowid;
END;
INSERT INTO block_fts(rowid, text, block_id)
  SELECT rowid, text, id FROM block WHERE deleted_at IS NULL;
"#;

/// v5: drop `block_parent(parent_id, position)`. No query reads it (children come from
/// the page index; FK checks use `block_parent_fk`), and because every root block in the
/// graph shares `parent_id NULL` it scattered writes across the whole index on merges.
const V5: &str = "DROP INDEX block_parent;";

/// v6: `key:: value` attributes as rows (P2 queries: attributes become queryable). One
/// row per attribute block; `value_key` is the normalized plain value, and page refs in
/// the value are already in `ref`. Backfilled from block text by [`backfill_attrs`].
const V6: &str = r#"
CREATE TABLE attr (
  block_id  TEXT PRIMARY KEY REFERENCES block(id) ON DELETE CASCADE,
  page_id   TEXT NOT NULL,
  key_norm  TEXT NOT NULL,
  key       TEXT NOT NULL,
  value     TEXT NOT NULL,
  value_key TEXT NOT NULL
);
CREATE INDEX attr_key ON attr(key_norm, value_key);
CREATE INDEX attr_page ON attr(page_id);
"#;

/// v7: attachments (P2). Bytes live in the blob directory beside the graph, named by
/// SHA-256 (§11.6); this table remembers what this device has and what it has uploaded.
const V7: &str = r#"
CREATE TABLE attachment (
  hash       TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  mime       TEXT NOT NULL,
  size       INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  uploaded   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX attachment_upload ON attachment(uploaded) WHERE uploaded = 0;
"#;

/// v8: the local tombstone memory (docs/state-machines/page-lifecycle-trash.md issue 3).
/// Empty Trash removes the shard and the page row, which used to be the only record that an
/// id had been tombstoned: the next `create_page` reissued the same uuidv5 (rule 4) and a
/// peer that still held the tombstoned shard imported the new page's ops into it, bringing
/// `deleted_at` back. These rows are local bookkeeping, not a projection, so
/// `rebuild_index` never drops them.
///
/// Backfilled from every page that is tombstoned today, and the same migration drops a
/// trash row that outlived its tombstone (the state an undo of a delete used to leave).
const V8: &str = r#"
CREATE TABLE tombstone (
  id         TEXT PRIMARY KEY,
  deleted_at INTEGER NOT NULL
);
INSERT OR IGNORE INTO tombstone(id, deleted_at)
  SELECT id, deleted_at FROM page WHERE deleted_at IS NOT NULL;
DELETE FROM trash WHERE NOT EXISTS (
  SELECT 1 FROM page p WHERE p.id = trash.id AND p.deleted_at IS NOT NULL);
"#;

/// v9: the attachment row's upload flag becomes an explicit state
/// (docs/state-machines/attachment-blob-state.md). Two columns, one fact: `lost_at` is set
/// exactly when `state = 'lost'`, and the table CHECK makes the other combination
/// unrepresentable, so "pending but file gone" cannot be silently skipped any more.
/// `uploaded = 1` rows become `uploaded`; the rest start pending.
///
/// The table is rebuilt (not `ALTER TABLE`ed) because SQLite cannot add a table-level CHECK
/// and v7/v8 files keep opening either way (§14.4). It has no foreign keys to fix up.
const V9: &str = r#"
CREATE TABLE attachment_v9 (
  hash       TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  mime       TEXT NOT NULL,
  size       INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  state      TEXT NOT NULL CHECK (state IN ('pending','uploaded','lost')),
  lost_at    INTEGER,
  CHECK ((state = 'lost') = (lost_at IS NOT NULL))
);
INSERT INTO attachment_v9(hash, name, mime, size, created_at, state, lost_at)
  SELECT hash, name, mime, size, created_at,
         CASE WHEN uploaded <> 0 THEN 'uploaded' ELSE 'pending' END, NULL
  FROM attachment;
DROP TABLE attachment;
ALTER TABLE attachment_v9 RENAME TO attachment;
CREATE INDEX attachment_pending ON attachment(created_at, hash)
  WHERE state <> 'uploaded';
"#;

/// Attribute row values for a block's text, if it is an attribute block.
pub(crate) fn attr_row(text: &str) -> Option<(String, String, String, String)> {
    use utopia_core::{inline, title::normalize_title};
    let (key, range) = inline::attribute(text)?;
    let value = text[range].to_string();
    let value_key = normalize_title(&inline::plain_text(&value));
    Some((normalize_title(&key), key, value, value_key))
}

fn backfill_attrs(tx: &rusqlite::Transaction) -> rusqlite::Result<()> {
    let rows: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, page_id, text FROM block
             WHERE deleted_at IS NULL AND block_type <> 'code' AND instr(text, '::') > 0",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut ins = tx.prepare(
        "INSERT INTO attr(block_id, page_id, key_norm, key, value, value_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (id, page, text) in rows {
        if let Some((kn, k, v, vk)) = attr_row(&text) {
            ins.execute(rusqlite::params![id, page, kn, k, v, vk])?;
        }
    }
    Ok(())
}

pub fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -32768;",
    )
}

pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        let tx = conn.transaction()?;
        tx.execute_batch(V1)?;
        tx.execute(
            "INSERT INTO meta(key, value) VALUES ('title_normalization', ?1)",
            [utopia_core::title::NORMALIZATION_VERSION.to_string()],
        )?;
        tx.execute_batch("PRAGMA user_version = 1")?;
        tx.commit()?;
    }
    if version < 2 {
        let tx = conn.transaction()?;
        tx.execute_batch(V2)?;
        tx.execute_batch("PRAGMA user_version = 2")?;
        tx.commit()?;
    }
    if version < 3 {
        let tx = conn.transaction()?;
        tx.execute_batch(V3)?;
        tx.execute_batch("PRAGMA user_version = 3")?;
        tx.commit()?;
    }
    if version < 4 {
        let tx = conn.transaction()?;
        tx.execute_batch(V4)?;
        tx.execute_batch("PRAGMA user_version = 4")?;
        tx.commit()?;
    }
    if version < 5 {
        let tx = conn.transaction()?;
        tx.execute_batch(V5)?;
        tx.execute_batch("PRAGMA user_version = 5")?;
        tx.commit()?;
    }
    if version < 6 {
        let tx = conn.transaction()?;
        tx.execute_batch(V6)?;
        backfill_attrs(&tx)?;
        tx.execute_batch("PRAGMA user_version = 6")?;
        tx.commit()?;
    }
    if version < 7 {
        let tx = conn.transaction()?;
        tx.execute_batch(V7)?;
        // Stamp the version this migration builds, not the newest one: each block commits
        // on its own, and a crash between two of them must not leave a file that claims a
        // schema it does not have.
        tx.execute_batch("PRAGMA user_version = 7")?;
        tx.commit()?;
    }
    if version < 8 {
        let tx = conn.transaction()?;
        tx.execute_batch(V8)?;
        // The version this block builds, not the newest one: a file that stops here must
        // not claim a schema it does not have yet.
        tx.execute_batch("PRAGMA user_version = 8")?;
        tx.commit()?;
    }
    if version < 9 {
        let tx = conn.transaction()?;
        tx.execute_batch(V9)?;
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        tx.commit()?;
    }
    Ok(())
}
