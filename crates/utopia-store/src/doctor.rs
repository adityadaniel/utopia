//! `utopia doctor` (§10.4, §15.2): invariant checks and index rebuild.

use std::collections::{BTreeSet, HashMap};

use rusqlite::Connection;
use serde::Serialize;
use utopia_core::date::parse_unambiguous_date;

use crate::project::{Projector, ResolutionWork};
use crate::{Result, Store};

#[derive(Debug, Clone, Default, Serialize)]
pub struct DoctorReport {
    pub pages: usize,
    pub blocks: usize,
    pub refs: usize,
    pub problems: Vec<String>,
}

impl DoctorReport {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every projection row as text, excluding fields that legitimately differ between an
/// incremental and a full projection (`page.updated_at`, `ref.id`).
fn dump(conn: &Connection) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    for (prefix, sql) in [
        ("page", "SELECT id||'|'||kind||'|'||title||'|'||title_key||'|'||ifnull(daily_date,'')||'|'||ifnull(deleted_at,'') FROM page"),
        ("alias", "SELECT page_id||'|'||alias_key||'|'||alias FROM alias"),
        ("block", "SELECT id||'|'||page_id||'|'||ifnull(parent_id,'')||'|'||position||'|'||depth||'|'||text||'|'||block_type||'|'||ifnull(heading_level,'')||'|'||checked||'|'||created_at||'|'||updated_at||'|'||ifnull(deleted_at,'') FROM block"),
        ("ref", "SELECT src_block_id||'|'||src_page_id||'|'||dst_kind||'|'||ifnull(dst_page_id,'')||'|'||ifnull(dst_block_id,'')||'|'||ifnull(dst_title_key,'')||'|'||anchor_start||'|'||anchor_end FROM ref"),
        ("fts", "SELECT block_id||'|'||text FROM block_fts"),
        ("attr", "SELECT block_id||'|'||page_id||'|'||key_norm||'|'||value_key FROM attr"),
    ] {
        for s in strings(conn, sql)? {
            out.insert(format!("{prefix}|{s}"));
        }
    }
    Ok(out)
}

impl Store {
    /// Re-project every shard from scratch inside the current transaction.
    fn reproject_all(&mut self, conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "DELETE FROM ref; DELETE FROM block_fts; DELETE FROM attr; DELETE FROM block; DELETE FROM alias;",
        )?;
        let projector = Projector {
            conn,
            fmt: &self.date_format,
            now: self.clock.unwrap_or_else(utopia_core::now_ms),
        };
        let mut work = ResolutionWork::default();
        // Tombstoned pages first, as in a normal commit.
        // Draft days (FR-1.2) live only in memory and have nothing to project.
        let mut states: Vec<_> = self
            .docs
            .iter()
            .filter(|(p, _)| !self.is_draft(p))
            .map(|(_, e)| e.doc_state())
            .collect();
        states.sort_by_key(|s| s.meta.deleted_at.is_none());
        for st in &states {
            projector.project(st, None, &mut work)?;
        }
        // Everything may resolve differently once all pages exist again.
        let keys = {
            let mut stmt =
                conn.prepare("SELECT DISTINCT dst_title_key FROM ref WHERE dst_kind='page'")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        work.keys.extend(keys);
        projector.reresolve(&mut work)?;
        Ok(())
    }

    fn load_all_docs(&mut self) -> Result<()> {
        for p in self.all_page_ids()? {
            self.load_doc(&p)?;
        }
        Ok(())
    }

    /// Rebuild all derived tables from the shards (principle 6: derived data is
    /// disposable). The local `tombstone` memory is not derived data (§10.3, rule 4): a
    /// purged page has no shard to rebuild it from, so it is never dropped here.
    pub fn rebuild_index(&mut self) -> Result<()> {
        self.sync_index()?;
        self.load_all_docs()?;
        let conn = std::mem::replace(&mut self.conn, Connection::open_in_memory()?);
        let result = (|| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            match self.reproject_all(&conn) {
                Ok(()) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })();
        self.conn = conn;
        result?;
        // The repair replaced the projections the last publish classified, so the set it
        // left describes an index that no longer exists (§10.4, SB-2).
        self.republish_tag_pages()?;
        Ok(())
    }

    /// Check the §10.4 invariants. Read-only: the rebuild-equality check runs in a
    /// transaction that is rolled back.
    pub fn doctor(&mut self) -> Result<DoctorReport> {
        self.sync_index()?;
        self.load_all_docs()?;
        let mut r = DoctorReport::default();
        let c = &self.conn;
        r.pages = c.query_row(
            "SELECT count(*) FROM page WHERE deleted_at IS NULL",
            [],
            |x| x.get::<_, i64>(0),
        )? as usize;
        r.blocks = c.query_row(
            "SELECT count(*) FROM block WHERE deleted_at IS NULL",
            [],
            |x| x.get::<_, i64>(0),
        )? as usize;
        r.refs = c.query_row("SELECT count(*) FROM ref", [], |x| x.get::<_, i64>(0))? as usize;
        let checks: [(&str, &str); 8] = [
            ("1: block in a different page than its parent",
             "SELECT c.id FROM block c JOIN block p ON p.id=c.parent_id WHERE c.page_id<>p.page_id"),
            ("2: depth is not parent depth + 1",
             "SELECT c.id FROM block c JOIN block p ON p.id=c.parent_id WHERE c.depth<>p.depth+1
              UNION SELECT id FROM block WHERE parent_id IS NULL AND depth<>0"),
            ("4: duplicate sibling position",
             "SELECT min(id) FROM block WHERE deleted_at IS NULL
              GROUP BY page_id, ifnull(parent_id,''), position HAVING count(*)>1"),
            ("5: live block without a shard",
             "SELECT b.id FROM block b LEFT JOIN page_doc d ON d.page_id=b.page_id
              WHERE d.page_id IS NULL AND b.deleted_at IS NULL"),
            ("7: duplicate live title",
             "SELECT title_key FROM page WHERE deleted_at IS NULL AND kind='page'
              GROUP BY title_key HAVING count(*)>1"),
            ("8: alias maps to several live pages or shadows a title",
             "SELECT a.alias_key FROM alias a JOIN page p ON p.id=a.page_id WHERE p.deleted_at IS NULL
              GROUP BY a.alias_key HAVING count(*)>1
              UNION SELECT a.alias_key FROM alias a JOIN page t ON t.title_key=a.alias_key
              WHERE t.kind='page' AND t.deleted_at IS NULL AND t.id<>a.page_id"),
            ("9: live block in a tombstoned page",
             "SELECT b.id FROM block b JOIN page p ON p.id=b.page_id
              WHERE p.deleted_at IS NOT NULL AND b.deleted_at IS NULL"),
            // Local bookkeeping, not a projection (docs/state-machines/page-lifecycle-trash.md):
            // the trash row exists only for a trashed page.
            ("local: trash row for a page that is not trashed",
             "SELECT t.id FROM trash t LEFT JOIN page p ON p.id=t.id
              WHERE p.id IS NULL OR p.deleted_at IS NULL"),
        ];
        for (name, sql) in checks {
            for id in strings(c, sql)? {
                r.problems.push(format!("invariant {name}: {id}"));
            }
        }
        // 3: no cycles in parent_id.
        let parents: HashMap<String, Option<String>> = {
            let mut stmt = c.prepare("SELECT id, parent_id FROM block")?;
            let rows = stmt
                .query_map([], |x| Ok((x.get(0)?, x.get(1)?)))?
                .collect::<rusqlite::Result<HashMap<_, _>>>()?;
            rows
        };
        for start in parents.keys() {
            let mut cur = parents.get(start).cloned().flatten();
            let mut steps = 0;
            while let Some(p) = cur {
                steps += 1;
                if p == *start || steps > parents.len() {
                    r.problems
                        .push(format!("invariant 3: cycle through {start}"));
                    break;
                }
                cur = parents.get(&p).cloned().flatten();
            }
        }
        // 8 (date-shaped aliases).
        for key in strings(c, "SELECT alias_key FROM alias")? {
            if parse_unambiguous_date(&key, &self.date_format).is_some() {
                r.problems
                    .push(format!("invariant 8: date-shaped alias {key}"));
            }
        }
        // 6 and 10: the incremental projection equals a full rebuild.
        let before = dump(&self.conn)?;
        let conn = std::mem::replace(&mut self.conn, Connection::open_in_memory()?);
        let rebuilt = (|| {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let res = self.reproject_all(&conn).and_then(|_| dump(&conn));
            let _ = conn.execute_batch("ROLLBACK");
            res
        })();
        self.conn = conn;
        let after = rebuilt?;
        for row in before.symmetric_difference(&after).take(50) {
            let side = if before.contains(row) {
                "index only"
            } else {
                "rebuild only"
            };
            r.problems.push(format!(
                "invariant 6: projection differs from rebuild ({side}): {row}"
            ));
        }
        Ok(r)
    }
}
