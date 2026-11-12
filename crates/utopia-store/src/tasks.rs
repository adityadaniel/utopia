//! The Tasks view's read (PRD §9.2.2): every live to-do on a live page, grouped by page,
//! with the context the view draws around it. Read-only over the projections (§11.1); the
//! view writes through the same command a page uses (rule 13).

use std::collections::HashMap;

use chrono::NaiveDate;
use utopia_core::{inline::plain_text, BlockId, PageId};

use crate::{Result, Store};

/// One to-do block (TK-2).
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub block: BlockId,
    pub page: PageId,
    /// The block's stored text (markup included); the view renders it like a reference.
    pub text: String,
    pub checked: bool,
    /// Plain text of the nearest heading block above the task.
    pub heading: Option<String>,
    /// Plain text of the parent, when the parent is a plain block (not a heading or a to-do).
    pub context: Option<String>,
    /// The nearest to-do above the task; the view nests the task under it when it shows both.
    pub parent_task: Option<BlockId>,
    /// The earliest day the task links to (TK-5). Read from stored resolutions, so an
    /// ambiguous numeric date, which never resolves (§5.5), never counts.
    pub due: Option<NaiveDate>,
    pub updated_at: i64,
}

/// The to-dos of one page, in outline order.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskGroup {
    pub page: PageId,
    pub title: String,
    /// Set for a daily page, whose weekday the view shows above the title.
    pub daily: Option<NaiveDate>,
    pub tasks: Vec<Task>,
}

impl TaskGroup {
    pub fn open(&self) -> usize {
        self.tasks.iter().filter(|t| !t.checked).count()
    }

    fn latest(&self) -> i64 {
        self.tasks.iter().map(|t| t.updated_at).max().unwrap_or(0)
    }
}

struct Row {
    task: Task,
    position: String,
    title: String,
    daily: Option<NaiveDate>,
}

/// An ancestor of a to-do, `lvl` steps up (1 is the parent).
struct Up {
    lvl: i64,
    id: String,
    kind: String,
    text: String,
    position: String,
}

fn date(s: Option<String>) -> Option<NaiveDate> {
    s.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
}

impl Store {
    /// Every live to-do on a live page (TK-2), done or not: the view filters, so a box
    /// ticked under Open can stay on screen (TK-4). Groups come most recently changed
    /// first, ties by title; tasks keep outline order.
    pub fn tasks(&self) -> Result<Vec<TaskGroup>> {
        self.read_barrier()?;
        let mut rows: Vec<Row> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT b.id, b.page_id, b.text, b.checked, b.updated_at,
                        b.position, pg.title, pg.daily_date
                 FROM block b JOIN page pg ON pg.id = b.page_id
                 WHERE b.block_type = 'todo' AND b.deleted_at IS NULL
                   AND pg.deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(Row {
                    task: Task {
                        block: BlockId(r.get(0)?),
                        page: PageId(r.get(1)?),
                        text: r.get(2)?,
                        checked: r.get::<_, i64>(3)? != 0,
                        heading: None,
                        context: None,
                        parent_task: None,
                        due: None,
                        updated_at: r.get(4)?,
                    },
                    position: r.get(5)?,
                    title: r.get(6)?,
                    daily: date(r.get(7)?),
                })
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        // Every ancestor of every to-do, for its heading, context, nesting and outline path.
        let mut ups: HashMap<String, Vec<Up>> = HashMap::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "WITH RECURSIVE up(task, anc, lvl) AS (
                   SELECT id, parent_id, 1 FROM block
                   WHERE block_type = 'todo' AND deleted_at IS NULL AND parent_id IS NOT NULL
                   UNION ALL
                   SELECT up.task, b.parent_id, up.lvl + 1
                   FROM up JOIN block b ON b.id = up.anc
                   WHERE b.parent_id IS NOT NULL
                 )
                 SELECT up.task, up.lvl, b.id, b.block_type, b.text, b.position
                 FROM up JOIN block b ON b.id = up.anc",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Up {
                        lvl: r.get(1)?,
                        id: r.get(2)?,
                        kind: r.get(3)?,
                        text: r.get(4)?,
                        position: r.get(5)?,
                    },
                ))
            })?;
            for row in rows {
                let (task, up) = row?;
                ups.entry(task).or_default().push(up);
            }
        }

        let mut due: HashMap<String, NaiveDate> = HashMap::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT r.src_block_id, MIN(pg.daily_date)
                 FROM ref r
                 JOIN block b ON b.id = r.src_block_id
                 JOIN page pg ON pg.id = r.dst_page_id
                 WHERE b.block_type = 'todo' AND b.deleted_at IS NULL
                   AND r.dst_kind = 'page' AND pg.kind = 'daily' AND pg.deleted_at IS NULL
                 GROUP BY r.src_block_id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })?;
            for row in rows {
                let (id, d) = row?;
                if let Some(d) = date(d) {
                    due.insert(id, d);
                }
            }
        }

        let mut paths: HashMap<String, Vec<String>> = HashMap::new();
        for row in &mut rows {
            let id = row.task.block.0.clone();
            let mut chain = ups.remove(&id).unwrap_or_default();
            chain.sort_by_key(|u| u.lvl);
            row.task.heading = chain
                .iter()
                .find(|u| u.kind == "heading")
                .map(|u| plain_text(&u.text));
            row.task.context = chain
                .first()
                .filter(|u| u.kind != "heading" && u.kind != "todo")
                .map(|u| plain_text(&u.text))
                .filter(|t| !t.trim().is_empty());
            row.task.parent_task = chain
                .iter()
                .find(|u| u.kind == "todo")
                .map(|u| BlockId(u.id.clone()));
            row.task.due = due.get(&id).copied();
            let mut path: Vec<String> = chain.iter().rev().map(|u| u.position.clone()).collect();
            path.push(row.position.clone());
            paths.insert(id, path);
        }

        let mut groups: Vec<TaskGroup> = Vec::new();
        let mut at: HashMap<PageId, usize> = HashMap::new();
        for row in rows {
            let i = *at.entry(row.task.page.clone()).or_insert_with(|| {
                groups.push(TaskGroup {
                    page: row.task.page.clone(),
                    title: row.title.clone(),
                    daily: row.daily,
                    tasks: Vec::new(),
                });
                groups.len() - 1
            });
            groups[i].tasks.push(row.task);
        }
        let empty = Vec::new();
        for g in &mut groups {
            g.tasks.sort_by(|a, b| {
                let pa = paths.get(&a.block.0).unwrap_or(&empty);
                let pb = paths.get(&b.block.0).unwrap_or(&empty);
                pa.cmp(pb)
            });
        }
        groups.sort_by(|a, b| {
            b.latest()
                .cmp(&a.latest())
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        Ok(groups)
    }

    /// The number the sidebar shows next to Tasks (TK-1): open to-dos on live pages.
    pub fn open_task_count(&self) -> Result<usize> {
        self.read_barrier()?;
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM block b JOIN page pg ON pg.id = b.page_id
             WHERE b.block_type = 'todo' AND b.deleted_at IS NULL AND b.checked = 0
               AND pg.deleted_at IS NULL",
            [],
            |r| r.get(0),
        )?;
        Ok(usize::try_from(n).unwrap_or(0))
    }
}
