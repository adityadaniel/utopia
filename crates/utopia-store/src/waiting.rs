//! Remote changes a page holds but cannot apply yet (§12.3 step 3, §2.3.7).
//!
//! A device that pushed its first changes to one destination and its later ones to another
//! (a mistyped graph id, a wiped relay) leaves a gap: the later changes depend on ones this
//! device never receives. Loro keeps such changes pending and leaves them out of every
//! snapshot, so without the `page_pending` row (schema v11) they existed only in memory
//! and the sync cursor moved past them: the other device's text was lost with no error.
//!
//! The row is written after the batch that changed it has committed its shards, and
//! before the caller advances any cursor. A stale row is harmless (importing it again
//! applies nothing new); a missing one is the loss this module exists to prevent.
use rusqlite::{params, OptionalExtension};
use utopia_core::PageId;
use utopia_doc::{waiting_ranges, PageDoc, WaitingRange};

use crate::{Result, Store};

/// A page that waits for changes from another device, for status lines and `doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingPage {
    pub page: PageId,
    /// The waiting changes, per peer. Empty when the stored bytes cannot be read, which
    /// is reported rather than hidden.
    pub ranges: Vec<WaitingRange>,
    /// When the page first started to wait (Unix ms).
    pub since: i64,
}

impl Store {
    /// The stored waiting changes of `page`, if any.
    pub(crate) fn waiting_payload(&self, page: &PageId) -> Result<Option<Vec<u8>>> {
        let page = &self.redirect(page).unwrap_or_else(|| page.clone());
        Ok(self
            .conn
            .query_row(
                "SELECT payload FROM page_pending WHERE page_id=?1",
                [page.as_str()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Record what `page`'s shard now holds waiting, or clear the row when nothing does.
    /// Called after the shard's commit, so a cleared row never hides changes that are not
    /// yet durable in `page_doc`.
    pub(crate) fn save_waiting(&mut self, page: &PageId) -> Result<()> {
        // The shard `load_doc` imported into: a promoted draft's old id reads as the page
        // that owns the day, and so must the row.
        let page = &self.redirect(page).unwrap_or_else(|| page.clone());
        let Some(entry) = self.docs.get(page) else {
            return Ok(());
        };
        match entry.doc.waiting()? {
            Some(bytes) => {
                self.conn.execute(
                    "INSERT INTO page_pending(page_id, payload, since) VALUES (?1, ?2, ?3)
                     ON CONFLICT(page_id) DO UPDATE SET payload = excluded.payload",
                    params![page.as_str(), bytes, self.now()],
                )?;
            }
            None => {
                self.conn
                    .execute("DELETE FROM page_pending WHERE page_id=?1", [page.as_str()])?;
            }
        }
        Ok(())
    }

    /// Whether `page` holds changes that wait for another device's earlier changes.
    pub fn has_waiting(&self, page: &PageId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM page_pending WHERE page_id=?1)",
            [page.as_str()],
            |r| r.get(0),
        )?)
    }

    /// How many pages wait for changes from another device. A count, cheap enough for the
    /// status line on every round (rule 10).
    pub fn waiting_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM page_pending", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Every page that waits, with the peers and counters it waits on.
    pub fn waiting_pages(&self) -> Result<Vec<WaitingPage>> {
        let mut stmt = self
            .conn
            .prepare("SELECT page_id, payload, since FROM page_pending ORDER BY since, page_id")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    PageId(r.get::<_, String>(0)?),
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .map(|(page, payload, since)| WaitingPage {
                page,
                ranges: waiting_ranges(&payload).unwrap_or_default(),
                since,
            })
            .collect())
    }
}
