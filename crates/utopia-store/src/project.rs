//! Projections (§11.1): `page`, `alias`, `block`, `ref` and `block_fts` rows derived
//! from a page shard, plus the resolver (§10.3.1). Everything here runs inside the
//! caller's transaction and is a pure function of shard state + other pages' rows.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, Connection, OptionalExtension};
use utopia_core::date::{iso, parse_unambiguous_date};
use utopia_core::inline::{extract_refs, RefKind};
use utopia_core::title::normalize_title;
use utopia_core::{BlockId, BlockKind, DateFormat, DocDelta, PageId, PageKind, PageState};

use crate::Result;

/// Title shown for a page: daily titles are a rendering of the date (FR-1.4, FR-1.11).
pub fn display_title(st: &PageState, fmt: &DateFormat) -> String {
    match (st.meta.kind, st.meta.daily_date) {
        (PageKind::Daily, Some(d)) => fmt.format(d),
        _ => st.meta.title.clone(),
    }
}

/// Resolution of a page-like reference target (§10.3.1). Dates win; daily pages are
/// reachable only through the date step. Takes the normalized key.
pub fn resolve_page_key(conn: &Connection, key: &str, fmt: &DateFormat) -> Result<Option<PageId>> {
    if let Some(d) = parse_unambiguous_date(key, fmt) {
        return Ok(conn
            .query_row(
                "SELECT id FROM page WHERE kind='daily' AND daily_date=?1 AND deleted_at IS NULL",
                [iso(d)],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(PageId));
    }
    if let Some(id) = conn
        .query_row(
            "SELECT id FROM page WHERE title_key=?1 AND kind='page' AND deleted_at IS NULL",
            [key],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(Some(PageId(id)));
    }
    // Aliases may live on daily pages (a normal page merged into a date, §5.5); they are
    // explicit names, never date renderings, so they stay reachable.
    Ok(conn
        .query_row(
            "SELECT a.page_id FROM alias a JOIN page p ON p.id = a.page_id
             WHERE a.alias_key=?1 AND p.deleted_at IS NULL ORDER BY a.page_id LIMIT 1",
            [key],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(PageId))
}

/// Identity-relevant facts about a page row, compared before/after projection to decide
/// which references need re-resolution (FR-3.12).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageKeys {
    pub live: bool,
    pub title_key: String,
    pub daily_date: Option<String>,
    pub alias_keys: Vec<String>,
}

pub fn page_keys(conn: &Connection, page: &PageId) -> Result<PageKeys> {
    let row = conn
        .query_row(
            "SELECT deleted_at IS NULL, title_key, daily_date FROM page WHERE id=?1",
            [page.as_str()],
            |r| {
                Ok((
                    r.get::<_, bool>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((live, title_key, daily_date)) = row else {
        return Ok(PageKeys::default());
    };
    let mut stmt =
        conn.prepare_cached("SELECT alias_key FROM alias WHERE page_id=?1 ORDER BY alias_key")?;
    let alias_keys = stmt
        .query_map([page.as_str()], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(PageKeys {
        live,
        title_key,
        daily_date,
        alias_keys,
    })
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    page_id: String,
    parent_id: Option<String>,
    position: String,
    depth: i64,
    text: String,
    block_type: String,
    heading_level: Option<i64>,
    checked: bool,
    created_at: i64,
    updated_at: i64,
    deleted_at: Option<i64>,
}

fn load_rows(conn: &Connection, page: &PageId) -> Result<HashMap<String, Row>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, page_id, parent_id, position, depth, text, block_type, heading_level,
                checked, created_at, updated_at, deleted_at
         FROM block WHERE page_id=?1",
    )?;
    let rows = stmt
        .query_map([page.as_str()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Row {
                    page_id: r.get(1)?,
                    parent_id: r.get(2)?,
                    position: r.get(3)?,
                    depth: r.get(4)?,
                    text: r.get(5)?,
                    block_type: r.get(6)?,
                    heading_level: r.get(7)?,
                    checked: r.get(8)?,
                    created_at: r.get(9)?,
                    updated_at: r.get(10)?,
                    deleted_at: r.get(11)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(rows)
}

fn load_row(conn: &Connection, id: &BlockId) -> Result<Option<Row>> {
    let mut stmt = conn.prepare_cached(
        "SELECT page_id, parent_id, position, depth, text, block_type, heading_level,
                checked, created_at, updated_at, deleted_at
         FROM block WHERE id=?1",
    )?;
    Ok(stmt
        .query_row([id.as_str()], |r| {
            Ok(Row {
                page_id: r.get(0)?,
                parent_id: r.get(1)?,
                position: r.get(2)?,
                depth: r.get(3)?,
                text: r.get(4)?,
                block_type: r.get(5)?,
                heading_level: r.get(6)?,
                checked: r.get(7)?,
                created_at: r.get(8)?,
                updated_at: r.get(9)?,
                deleted_at: r.get(10)?,
            })
        })
        .optional()?)
}

/// The projected row for a block, or `None` if the block does not belong to this page's
/// projection (moved away).
fn row_for(st: &PageState, id: &BlockId) -> Option<Row> {
    let b = st.block(id)?;
    if !b.present {
        return None;
    }
    let deleted_at = match st.meta.deleted_at {
        // Invariant 9: no live block belongs to a tombstoned page.
        Some(page_del) => Some(b.deleted_at.unwrap_or(page_del)),
        None if b.live => None,
        None => Some(b.deleted_at.unwrap_or(b.updated_at)),
    };
    Some(Row {
        page_id: st.page_id.0.clone(),
        parent_id: b.parent.as_ref().map(|p| p.0.clone()),
        position: b.position.clone(),
        depth: st.depth(id) as i64,
        text: b.text.clone(),
        block_type: b.kind.type_str().to_string(),
        heading_level: b.kind.heading_level().map(i64::from),
        checked: b.checked,
        created_at: b.created_at,
        updated_at: b.updated_at,
        deleted_at,
    })
}

pub struct Projector<'a> {
    pub conn: &'a Connection,
    pub fmt: &'a DateFormat,
    pub now: i64,
}

/// What changed about page identity during a batch of projections.
#[derive(Debug, Default)]
pub struct ResolutionWork {
    pub keys: HashSet<String>,
    pub pages: HashSet<PageId>,
    /// Destination pages whose reference edges this write changed (SB-2). Only these need
    /// their tag-page classification revisited; the unchanged refs on the same page do not.
    pub changed: HashSet<PageId>,
}

impl Projector<'_> {
    /// Project `st` into the page/alias/block/ref/fts tables. With a delta only the
    /// touched rows are rewritten (FR-3.8); otherwise the page is diffed.
    pub fn project(
        &self,
        st: &PageState,
        delta: Option<&DocDelta>,
        work: &mut ResolutionWork,
    ) -> Result<()> {
        let flip = self.project_page(st, work)?;
        self.project_blocks(st, delta, flip, work)
    }

    /// The page and alias rows. Returns whether the page appeared or was (un)tombstoned,
    /// which changes every block row's effective deletion.
    pub fn project_page(&self, st: &PageState, work: &mut ResolutionWork) -> Result<bool> {
        let before = page_keys(self.conn, &st.page_id)?;
        self.project_page_row(st)?;
        let after = page_keys(self.conn, &st.page_id)?;
        if before != after {
            work.pages.insert(st.page_id.clone());
            for k in [&before.title_key, &after.title_key]
                .into_iter()
                .chain(&before.alias_keys)
                .chain(&after.alias_keys)
            {
                if !k.is_empty() {
                    work.keys.insert(k.clone());
                }
            }
            for d in [before.daily_date, after.daily_date].into_iter().flatten() {
                work.keys.insert(d);
            }
        }
        Ok(before.live != after.live)
    }

    /// The block rows (and their refs/FTS). Imports carry no delta and a page flip changes
    /// every row: diff the whole page. Otherwise only the touched rows, their subtrees
    /// (depth, presence) and ancestors (revived tombstones) can change.
    pub fn project_blocks(
        &self,
        st: &PageState,
        delta: Option<&DocDelta>,
        page_flip: bool,
        work: &mut ResolutionWork,
    ) -> Result<()> {
        match delta.filter(|_| !page_flip) {
            None => self.project_all_blocks(st, work),
            Some(d) => {
                let mut ids: Vec<BlockId> = Vec::new();
                let mut seen: HashSet<BlockId> = HashSet::new();
                for id in d.touched() {
                    let mut group = vec![id.clone()];
                    if d.structural && st.contains(&id) {
                        group.extend(st.ancestors(&id));
                        group.extend(st.subtree(&id, false));
                    }
                    for g in group {
                        if seen.insert(g.clone()) {
                            ids.push(g);
                        }
                    }
                }
                self.write_rows(st, ids, work)
            }
        }
    }

    /// Write rows in an order that keeps foreign keys satisfied: deletions deepest first,
    /// then upserts parents first.
    fn write_rows(
        &self,
        st: &PageState,
        ids: Vec<BlockId>,
        work: &mut ResolutionWork,
    ) -> Result<()> {
        let mut deletes = Vec::new();
        let mut upserts = Vec::new();
        for id in ids {
            let old = load_row(self.conn, &id)?;
            match row_for(st, &id) {
                Some(new) => upserts.push((id, old, new)),
                None => {
                    if let Some(old) = old {
                        deletes.push((id, old));
                    }
                }
            }
        }
        deletes.sort_by_key(|(_, old)| std::cmp::Reverse(old.depth));
        for (id, old) in deletes {
            self.write_row(st, &id, Some(&old), None, work)?;
        }
        upserts.sort_by_key(|(_, _, new)| new.depth);
        for (id, old, new) in upserts {
            self.write_row(st, &id, old.as_ref(), Some(&new), work)?;
        }
        Ok(())
    }

    fn project_page_row(&self, st: &PageState) -> Result<()> {
        let title = display_title(st, self.fmt);
        let title_key = normalize_title(&title);
        let created_at = if st.meta.created_at > 0 {
            st.meta.created_at
        } else {
            self.now
        };
        self.conn.execute(
            "INSERT INTO page(id, kind, title, title_key, daily_date, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET kind=excluded.kind, title=excluded.title,
               title_key=excluded.title_key, daily_date=excluded.daily_date,
               created_at=excluded.created_at, updated_at=excluded.updated_at,
               deleted_at=excluded.deleted_at",
            params![
                st.page_id.as_str(),
                st.meta.kind.as_str(),
                title,
                title_key,
                st.meta.daily_date.map(iso),
                created_at,
                self.now,
                st.meta.deleted_at,
            ],
        )?;
        self.conn
            .execute("DELETE FROM alias WHERE page_id=?1", [st.page_id.as_str()])?;
        // Rule 4: remember that this id was tombstoned. Empty Trash deletes the shard and the
        // page row, so this row is the only thing that stops the deterministic id from being
        // issued again (docs/state-machines/page-lifecycle-trash.md issue 3).
        if let Some(at) = st.meta.deleted_at {
            self.conn.execute(
                "INSERT INTO tombstone(id, deleted_at) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET deleted_at=excluded.deleted_at",
                params![st.page_id.as_str(), at],
            )?;
        }
        if st.meta.deleted_at.is_none() {
            for a in &st.meta.aliases {
                let key = normalize_title(a);
                // Invariant 8: aliases are never date-shaped and never shadow a live title.
                if key.is_empty() || parse_unambiguous_date(&key, self.fmt).is_some() {
                    continue;
                }
                let shadowed: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM page WHERE title_key=?1 AND kind='page'
                       AND deleted_at IS NULL AND id<>?2)",
                    params![key, st.page_id.as_str()],
                    |r| r.get(0),
                )?;
                if shadowed {
                    continue;
                }
                self.conn.execute(
                    "INSERT OR IGNORE INTO alias(page_id, alias_key, alias) VALUES (?1, ?2, ?3)",
                    params![st.page_id.as_str(), key, a],
                )?;
            }
        }
        Ok(())
    }

    fn project_all_blocks(&self, st: &PageState, work: &mut ResolutionWork) -> Result<()> {
        let existing = load_rows(self.conn, &st.page_id)?;
        // Stale rows first, deepest first, so no delete strands a child.
        let mut stale: Vec<(&String, &Row)> = existing
            .iter()
            .filter(|(id, _)| row_for(st, &BlockId((*id).clone())).is_none())
            .collect();
        stale.sort_by_key(|(_, old)| std::cmp::Reverse(old.depth));
        for (id, old) in stale {
            self.write_row(st, &BlockId(id.clone()), Some(old), None, work)?;
        }
        // Then upserts in document pre-order: parents before children.
        for r in &st.roots {
            for id in st.subtree(r, false) {
                let Some(new) = row_for(st, &id) else {
                    continue;
                };
                let old = match existing.get(&id.0) {
                    Some(r) => Some(r.clone()),
                    // The row may currently belong to another page (merge, §8.6).
                    None => load_row(self.conn, &id)?,
                };
                self.write_row(st, &id, old.as_ref(), Some(&new), work)?;
            }
        }
        Ok(())
    }

    fn write_row(
        &self,
        st: &PageState,
        id: &BlockId,
        old: Option<&Row>,
        new: Option<&Row>,
        work: &mut ResolutionWork,
    ) -> Result<()> {
        match (old, new) {
            (None, None) => Ok(()),
            (Some(old), None) => {
                // Only drop rows this page owns; a moved block's row belongs to its new page.
                if old.page_id == st.page_id.0 {
                    // Its outgoing references disappear with the row: the destinations may
                    // stop (or start) being hashtag-only (SB-2).
                    work.changed.extend(self.dst_pages_of_block(id)?);
                    // The FTS row goes with it (trigger `block_fts_on_delete`).
                    self.conn
                        .prepare_cached("DELETE FROM block WHERE id=?1")?
                        .execute([id.as_str()])?;
                }
                Ok(())
            }
            (old, Some(new)) => {
                if old == Some(new) {
                    return Ok(());
                }
                self.conn.prepare_cached(
                    "INSERT INTO block(id, page_id, parent_id, position, depth, text, block_type,
                       heading_level, checked, created_at, updated_at, deleted_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT(id) DO UPDATE SET page_id=excluded.page_id,
                       parent_id=excluded.parent_id, position=excluded.position,
                       depth=excluded.depth, text=excluded.text, block_type=excluded.block_type,
                       heading_level=excluded.heading_level, checked=excluded.checked,
                       created_at=excluded.created_at, updated_at=excluded.updated_at,
                       deleted_at=excluded.deleted_at")?.execute(params![
                        id.as_str(),
                        new.page_id,
                        new.parent_id,
                        new.position,
                        new.depth,
                        new.text,
                        new.block_type,
                        new.heading_level,
                        new.checked,
                        new.created_at,
                        new.updated_at,
                        new.deleted_at,
                    ],
                )?;
                let reparse = old.is_none_or(|o| {
                    o.text != new.text
                        || o.deleted_at.is_some() != new.deleted_at.is_some()
                        || o.block_type != new.block_type
                });
                if reparse {
                    // Compare what each destination's references look like before and after
                    // (SB-2). Only a page whose tag/link class changed needs reclassifying;
                    // ordinary prose edits would otherwise rescan every tag it mentions.
                    let before = match old {
                        Some(o) => self.page_ref_classes(o)?,
                        None => HashMap::new(),
                    };
                    self.index_block(id, new)?;
                    let after = self.page_ref_classes(new)?;
                    for (page, all_tags) in &before {
                        if after.get(page) != Some(all_tags) {
                            work.changed.insert(page.clone());
                        }
                    }
                    for page in after.keys() {
                        if !before.contains_key(page) {
                            work.changed.insert(page.clone());
                        }
                    }
                } else if old.is_some_and(|o| o.page_id != new.page_id) {
                    // Moved to another page unchanged (merge): re-home its ref rows (the FTS row is
                    // keyed by rowid and carries no page).
                    self.conn
                        .prepare_cached("UPDATE ref SET src_page_id=?2 WHERE src_block_id=?1")?
                        .execute(params![id.as_str(), new.page_id])?;
                    self.conn
                        .prepare_cached("UPDATE attr SET page_id=?2 WHERE block_id=?1")?
                        .execute(params![id.as_str(), new.page_id])?;
                }
                Ok(())
            }
        }
    }

    /// Page destinations of `row`'s text, mapped to whether every reference from this block
    /// to that page is a hashtag (SB-2). Used to tell a tag-to-link change from a prose edit.
    fn page_ref_classes(&self, row: &Row) -> Result<HashMap<PageId, bool>> {
        let mut classes: HashMap<PageId, bool> = HashMap::new();
        if row.deleted_at.is_some() || row.block_type == BlockKind::Code.type_str() {
            return Ok(classes);
        }
        for r in extract_refs(&row.text) {
            if !matches!(r.kind, RefKind::Page | RefKind::Tag) {
                continue;
            }
            let key = normalize_title(&r.target);
            if let Some(page) = resolve_page_key(self.conn, &key, self.fmt)? {
                let all_tags = classes.entry(page).or_insert(true);
                if r.kind == RefKind::Page {
                    *all_tags = false;
                }
            }
        }
        Ok(classes)
    }

    /// The page destinations this block currently references, if any.
    fn dst_pages_of_block(&self, id: &BlockId) -> Result<HashSet<PageId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT dst_page_id FROM ref
             WHERE src_block_id=?1 AND dst_kind='page' AND dst_page_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([id.as_str()], |r| r.get::<_, String>(0))?;
        Ok(rows
            .map(|r| r.map(PageId))
            .collect::<rusqlite::Result<HashSet<_>>>()?)
    }

    /// Rewrite the ref edges and FTS row of one block (FR-3.8: only touched rows).
    fn index_block(&self, id: &BlockId, row: &Row) -> Result<()> {
        self.conn
            .prepare_cached("DELETE FROM ref WHERE src_block_id=?1")?
            .execute([id.as_str()])?;
        self.conn
            .prepare_cached(
                "DELETE FROM block_fts WHERE rowid = (SELECT rowid FROM block WHERE id=?1)",
            )?
            .execute([id.as_str()])?;
        self.conn
            .prepare_cached("DELETE FROM attr WHERE block_id=?1")?
            .execute([id.as_str()])?;
        if row.deleted_at.is_some() {
            return Ok(());
        }
        if row.block_type != BlockKind::Code.type_str() {
            if let Some((kn, k, v, vk)) = crate::schema::attr_row(&row.text) {
                self.conn
                    .prepare_cached(
                        "INSERT INTO attr(block_id, page_id, key_norm, key, value, value_key)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    )?
                    .execute(params![id.as_str(), row.page_id, kn, k, v, vk])?;
            }
        }
        self.conn
            .prepare_cached(
                "INSERT INTO block_fts(rowid, text, block_id)
                 SELECT rowid, ?1, id FROM block WHERE id=?2",
            )?
            .execute(params![row.text, id.as_str()])?;
        if row.block_type == BlockKind::Code.type_str() {
            return Ok(());
        }
        for r in extract_refs(&row.text) {
            let (dst_kind, dst_page, dst_block, key) = match r.kind {
                RefKind::Block => ("block", None, Some(r.target.clone()), None),
                RefKind::Page | RefKind::Tag => {
                    let key = normalize_title(&r.target);
                    let page = resolve_page_key(self.conn, &key, self.fmt)?;
                    ("page", page.map(|p| p.0), None, Some(key))
                }
            };
            self.conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO ref(src_block_id, src_page_id, dst_kind, dst_page_id,
                   dst_block_id, dst_title_key, anchor_start, anchor_end)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?
                .execute(params![
                    id.as_str(),
                    row.page_id,
                    dst_kind,
                    dst_page,
                    dst_block,
                    key,
                    r.range.start as i64,
                    r.range.end as i64,
                ])?;
        }
        Ok(())
    }

    /// Re-resolve page refs affected by identity changes (FR-3.6, FR-3.12, AC-2.10).
    /// Bounded by the `dst_title_key` and `dst_page_id` indexes: no other rows are read.
    pub fn reresolve(&self, work: &mut ResolutionWork) -> Result<usize> {
        let mut candidates: Vec<(i64, String, Option<String>)> = Vec::new();
        {
            let mut by_key = self.conn.prepare_cached(
                "SELECT id, dst_title_key, dst_page_id FROM ref
                 WHERE dst_kind='page' AND dst_title_key=?1",
            )?;
            for k in &work.keys {
                candidates.extend(
                    by_key
                        .query_map([k], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                );
            }
            let mut by_page = self.conn.prepare_cached(
                "SELECT id, dst_title_key, dst_page_id FROM ref
                 WHERE dst_kind='page' AND dst_page_id=?1",
            )?;
            for p in &work.pages {
                candidates.extend(
                    by_page
                        .query_map([p.as_str()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?,
                );
            }
        }
        candidates.sort();
        candidates.dedup();
        // Date keys (ISO from `daily_date`) need the stored keys of every form that parses
        // to that date; those rows are found by the page-id pass or by their own key, so
        // here we only need to also scan unresolved rows whose key parses as an affected date.
        let dates: HashSet<&String> = work
            .keys
            .iter()
            .filter(|k| utopia_core::date::parse_iso(k).is_some())
            .collect();
        if !dates.is_empty() {
            let mut stmt = self.conn.prepare_cached(
                "SELECT DISTINCT dst_title_key FROM ref WHERE dst_kind='page' AND dst_page_id IS NULL",
            )?;
            let keys = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut by_key = self.conn.prepare_cached(
                "SELECT id, dst_title_key, dst_page_id FROM ref
                 WHERE dst_kind='page' AND dst_title_key=?1 AND dst_page_id IS NULL",
            )?;
            for k in keys {
                if let Some(d) = parse_unambiguous_date(&k, self.fmt) {
                    if dates.contains(&iso(d)) {
                        candidates.extend(
                            by_key
                                .query_map([&k], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                                .collect::<rusqlite::Result<Vec<_>>>()?,
                        );
                    }
                }
            }
            candidates.sort();
            candidates.dedup();
        }
        let mut changed = 0;
        for (id, key, old) in candidates {
            let new = resolve_page_key(self.conn, &key, self.fmt)?.map(|p| p.0);
            if new != old {
                for p in [old.as_deref(), new.as_deref()].into_iter().flatten() {
                    work.changed.insert(PageId(p.to_string()));
                }
                changed += self.conn.execute(
                    "UPDATE OR IGNORE ref SET dst_page_id=?1 WHERE id=?2",
                    params![new, id],
                )?;
            }
        }
        Ok(changed)
    }
}
