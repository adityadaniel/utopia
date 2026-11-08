//! Attachments (P2): content-addressed files beside the graph (§10.3, §11.6).
//!
//! `notes.utopia` keeps its attachments in `notes.blobs/<sha256>`. A blob is written
//! once (to a temporary name, then renamed), never modified, and verified against its
//! hash whenever it arrives from sync, so a corrupt or malicious copy cannot replace a
//! good one. Blocks reference blobs with the markup in [`utopia_core::attach`].

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use utopia_core::attach::{blob_refs, highlight_markup, highlights, is_hash, sniff_mime};
use utopia_core::{outline, BlockId, BlockKind, PageId};

use crate::{Result, Store, StoreError};

/// Largest attachment accepted (sync payloads and memory use stay bounded).
pub const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Attachment {
    pub hash: String,
    pub name: String,
    pub mime: String,
    pub size: u64,
}

/// Where an attachment row stands in its life (§11.6, rule 12):
/// `docs/state-machines/attachment-blob-state.md`.
///
/// The row and the file are two facts; this is the row's half. A pending row whose bytes
/// are gone is [`BlobState::Lost`], never silently skipped, so it cannot pin the upload
/// queue. `Uploaded` is permanent: the bytes are content-addressed, so a relay that has
/// them never needs them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobState {
    /// Bytes on this device, not yet on a relay: the upload queue.
    Pending,
    /// On this device and on a relay. Never leaves this state.
    Uploaded,
    /// The row outlived its file (`.blobs` not moved with the graph, a `backup()` copy, a
    /// deleted file). Not uploadable until the bytes come back. `noticed_at` is when the
    /// loss was last confirmed, which is what orders the reconciliation rotation.
    Lost { noticed_at: i64 },
}

impl BlobState {
    fn as_str(self) -> &'static str {
        match self {
            BlobState::Pending => "pending",
            BlobState::Uploaded => "uploaded",
            BlobState::Lost { .. } => "lost",
        }
    }

    /// The CHECK constraint keeps the two columns in step; a hand-edited or newer value is
    /// read as [`Pending`], which can only cost an upload, never a byte (rule 11).
    fn from_row(state: &str, lost_at: Option<i64>) -> Self {
        match state {
            "uploaded" => BlobState::Uploaded,
            "lost" => BlobState::Lost {
                noticed_at: lost_at.unwrap_or(0),
            },
            _ => BlobState::Pending,
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Store {
    /// Where this graph's blobs live, `None` for in-memory graphs.
    pub fn blob_dir(&self) -> Option<PathBuf> {
        let p = self.path.as_ref()?;
        let stem = p.file_stem()?.to_string_lossy().to_string();
        Some(p.with_file_name(format!("{stem}.blobs")))
    }

    /// The file holding a blob, if this device has it.
    pub fn blob_path(&self, hash: &str) -> Option<PathBuf> {
        if !is_hash(hash) {
            return None;
        }
        let p = self.blob_dir()?.join(hash);
        p.exists().then_some(p)
    }

    pub fn has_blob(&self, hash: &str) -> bool {
        is_hash(hash) && (self.mem_blobs.contains_key(hash) || self.blob_path(hash).is_some())
    }

    pub fn blob(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        if !is_hash(hash) {
            return Ok(None);
        }
        if let Some(b) = self.mem_blobs.get(hash) {
            return Ok(Some(b.clone()));
        }
        match self.blob_path(hash) {
            Some(p) => Ok(Some(std::fs::read(p)?)),
            None => Ok(None),
        }
    }

    pub fn attachment(&self, hash: &str) -> Result<Option<Attachment>> {
        Ok(self
            .conn
            .query_row(
                "SELECT hash, name, mime, size FROM attachment WHERE hash=?1",
                [hash],
                |r| {
                    Ok(Attachment {
                        hash: r.get(0)?,
                        name: r.get(1)?,
                        mime: r.get(2)?,
                        size: r.get::<_, i64>(3)? as u64,
                    })
                },
            )
            .optional()?)
    }

    fn write_blob(&mut self, hash: &str, bytes: &[u8]) -> Result<()> {
        match self.blob_dir() {
            None => {
                self.mem_blobs.insert(hash.to_string(), bytes.to_vec());
            }
            Some(dir) => {
                let dst = dir.join(hash);
                if dst.exists() {
                    return Ok(());
                }
                std::fs::create_dir_all(&dir)?;
                let tmp = dir.join(format!(".{hash}.{}.tmp", std::process::id()));
                let mut f = std::fs::File::create(&tmp)?;
                f.write_all(bytes)?;
                f.sync_all()?;
                drop(f);
                std::fs::rename(&tmp, &dst)?;
            }
        }
        Ok(())
    }

    /// Record bytes this device now has. `uploaded` is sticky; a restored file revives a
    /// lost row to pending, because the same bytes under the same hash are the same blob.
    fn record(&self, a: &Attachment, state: BlobState) -> Result<()> {
        self.conn.execute(
            "INSERT INTO attachment(hash, name, mime, size, created_at, state, lost_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)
             ON CONFLICT(hash) DO UPDATE SET
                 state = CASE
                     WHEN attachment.state = 'uploaded' OR excluded.state = 'uploaded'
                     THEN 'uploaded' ELSE 'pending' END,
                 lost_at = NULL",
            params![
                a.hash,
                a.name,
                a.mime,
                a.size as i64,
                self.now(),
                state.as_str()
            ],
        )?;
        Ok(())
    }

    /// The row's state, or `None` when this device never recorded the blob.
    pub fn attachment_state(&self, hash: &str) -> Result<Option<BlobState>> {
        let row = self
            .conn
            .query_row(
                "SELECT state, lost_at FROM attachment WHERE hash=?1",
                [hash],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .optional()?;
        Ok(row.map(|(state, lost_at)| BlobState::from_row(&state, lost_at)))
    }

    /// Store bytes as an attachment and return its identity. Adding the same content
    /// twice is a no-op that returns the same hash.
    pub fn add_blob(&mut self, name: &str, bytes: &[u8]) -> Result<Attachment> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(StoreError::Invalid(format!(
                "{name} is larger than {} MB",
                MAX_BLOB_BYTES / (1024 * 1024)
            )));
        }
        let hash = sha256_hex(bytes);
        if let Some(a) = self.attachment(&hash)? {
            // Known bytes on disk: nothing to write. A row that had lost its file is
            // recorded again below, which revives it to pending.
            let lost = matches!(self.attachment_state(&hash)?, Some(BlobState::Lost { .. }));
            if self.has_blob(&hash) && !lost {
                return Ok(a);
            }
        }
        self.write_blob(&hash, bytes)?;
        let a = Attachment {
            hash,
            name: name.to_string(),
            mime: sniff_mime(name, bytes).to_string(),
            size: bytes.len() as u64,
        };
        self.record(&a, BlobState::Pending)?;
        Ok(a)
    }

    pub fn attach_file(&mut self, path: &Path) -> Result<Attachment> {
        let meta = std::fs::metadata(path)?;
        if meta.len() as usize > MAX_BLOB_BYTES {
            return Err(StoreError::Invalid(format!(
                "{} is larger than {} MB",
                path.display(),
                MAX_BLOB_BYTES / (1024 * 1024)
            )));
        }
        let bytes = std::fs::read(path)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".into());
        self.add_blob(&name, &bytes)
    }

    /// A blob that arrived from sync. Rejected unless its content hashes to `hash`.
    pub fn put_synced_blob(&mut self, hash: &str, bytes: &[u8]) -> Result<()> {
        if !is_hash(hash) || sha256_hex(bytes) != hash {
            return Err(StoreError::Invalid(format!(
                "blob {hash} failed verification"
            )));
        }
        self.write_blob(hash, bytes)?;
        let name = self
            .referenced_blob_names()?
            .into_iter()
            .find(|(h, _)| h == hash)
            .map_or_else(|| hash.to_string(), |(_, n)| n);
        self.record(
            &Attachment {
                hash: hash.to_string(),
                mime: sniff_mime(&name, bytes).to_string(),
                name,
                size: bytes.len() as u64,
            },
            BlobState::Uploaded,
        )
    }

    /// `(hash, name)` for every attachment live blocks point at.
    fn referenced_blob_names(&self) -> Result<Vec<(String, String)>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT text FROM block WHERE deleted_at IS NULL AND instr(text, 'blob:') > 0",
        )?;
        let mut out = Vec::new();
        for text in stmt.query_map([], |r| r.get::<_, String>(0))? {
            for r in blob_refs(&text?) {
                out.push((r.hash, r.name));
            }
        }
        Ok(out)
    }

    pub fn referenced_blobs(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .referenced_blob_names()?
            .into_iter()
            .map(|(h, _)| h)
            .collect())
    }

    /// Referenced blobs this device does not have (sync fetches these).
    pub fn missing_blobs(&self) -> Result<Vec<String>> {
        Ok(self
            .referenced_blobs()?
            .into_iter()
            .filter(|h| !self.has_blob(h))
            .collect())
    }

    /// Local blobs not yet uploaded, oldest first (§11.6).
    ///
    /// The queue is the rows whose bytes this device still has. A row whose file is gone is
    /// marked [`BlobState::Lost`] and skipped *before* the limit applies: the previous
    /// version put `LIMIT` in the SQL and only then looked for the file, so a few dead rows
    /// at the head of the queue starved every newer attachment forever
    /// (docs/state-machines/attachment-blob-state.md issue 1).
    ///
    /// This runs in `SyncEngine::prepare` on the UI thread, so the work is bounded per call
    /// (rule 10): the pending rows are read one at a time and the scan stops as soon as the
    /// batch is full. A dead row is met once, marked, and then leaves this scan for good, so
    /// a backlog of dead files costs one pass in total, not one per round. The slots the
    /// batch leaves unused re-check the lost rows ([`Store::revive_lost`]).
    pub fn blobs_to_upload(&self, limit: usize) -> Result<Vec<(String, Vec<u8>)>> {
        let mut out = Vec::new();
        let mut dead = Vec::new();
        {
            let mut stmt = self.conn.prepare_cached(
                "SELECT hash FROM attachment WHERE state = 'pending'
                 ORDER BY created_at, hash",
            )?;
            let mut rows = stmt.query([])?;
            while out.len() < limit {
                let Some(row) = rows.next()? else { break };
                let hash: String = row.get(0)?;
                match self.blob(&hash)? {
                    Some(bytes) => out.push((hash, bytes)),
                    None => dead.push(hash),
                }
            }
        }
        // Marked after the cursor closes: changing the rows a SELECT is walking has no
        // defined order.
        for hash in dead {
            self.mark_blob_lost(&hash)?;
        }
        if out.len() < limit {
            self.revive_lost(limit - out.len(), &mut out)?;
        }
        Ok(out)
    }

    /// Spend the upload slots the queue did not fill on the rows whose file was gone: their
    /// bytes may be back (a restored `.blobs` directory), and a revived row is uploaded in
    /// the same round it is noticed.
    ///
    /// The pass is bounded by `slots` and ordered by `lost_at`, which every still-missing
    /// row refreshes ([`Store::refresh_lost_at`]): without that, a lost set larger than the
    /// slots would retry the same head forever and never reach the rest.
    fn revive_lost(&self, slots: usize, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
        let lost: Vec<String> = {
            let mut stmt = self.conn.prepare_cached(
                "SELECT hash FROM attachment WHERE state = 'lost'
                 ORDER BY lost_at, hash LIMIT ?1",
            )?;
            let rows = stmt.query_map([slots as i64], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for hash in lost {
            match self.blob(&hash)? {
                Some(bytes) => {
                    self.revive_blob(&hash)?;
                    out.push((hash, bytes));
                }
                None => self.refresh_lost_at(&hash)?,
            }
        }
        Ok(())
    }

    /// Notice, once, that a queued row has no bytes. Not fatal: the file may come back
    /// (a `.blobs` directory restored, the same content attached again).
    fn mark_blob_lost(&self, hash: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE attachment SET state = 'lost', lost_at = ?2
             WHERE hash = ?1 AND state = 'pending'",
            params![hash, self.now()],
        )?;
        if changed > 0 {
            eprintln!(
                "utopia-store: attachment {hash} has no file; not uploading it until the bytes are back"
            );
        }
        Ok(())
    }

    /// `lost_at` is when the loss was last confirmed: refresh it so a row that is still
    /// missing goes to the back of the reconciliation rotation.
    fn refresh_lost_at(&self, hash: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE attachment SET lost_at = ?2 WHERE hash = ?1 AND state = 'lost'",
            params![hash, self.now()],
        )?;
        Ok(())
    }

    /// The bytes are back for a row that had lost them.
    fn revive_blob(&self, hash: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE attachment SET state = 'pending', lost_at = NULL
             WHERE hash = ?1 AND state = 'lost'",
            [hash],
        )?;
        Ok(())
    }

    pub fn mark_blobs_uploaded(&self, hashes: &[String]) -> Result<()> {
        let mut stmt = self.conn.prepare_cached(
            "UPDATE attachment SET state = 'uploaded', lost_at = NULL WHERE hash = ?1",
        )?;
        for h in hashes {
            stmt.execute([h])?;
        }
        Ok(())
    }
}

/// A highlight block in the graph.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PdfHighlight {
    pub block: BlockId,
    pub page_id: PageId,
    /// 0-based PDF page.
    pub pdf_page: usize,
    pub rects: Vec<[f32; 4]>,
    /// The block text without the highlight markup (the quote and any note).
    pub text: String,
}

impl Store {
    /// Highlights on a PDF, in PDF page order.
    pub fn pdf_highlights(&self, hash: &str) -> Result<Vec<PdfHighlight>> {
        self.read_barrier()?;
        let needle = format!("{{{{highlight: blob:{hash}");
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, page_id, text FROM block WHERE deleted_at IS NULL AND instr(text, ?1) > 0",
        )?;
        let mut out = Vec::new();
        for row in stmt.query_map([&needle], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (id, page, text) = row?;
            for h in highlights(&text).into_iter().filter(|h| h.hash == hash) {
                let mut plain = text.clone();
                plain.replace_range(h.range.clone(), "");
                out.push(PdfHighlight {
                    block: BlockId(id.clone()),
                    page_id: PageId(page.clone()),
                    pdf_page: h.page,
                    rects: h.rects.clone(),
                    text: plain.trim().to_string(),
                });
            }
        }
        out.sort_by(|a, b| {
            (a.pdf_page, a.rects[0][1])
                .partial_cmp(&(b.pdf_page, b.rects[0][1]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }

    /// Record a highlight as a quote block under the block that shows the PDF (or on
    /// today's page when no block does), so it gets references, search and sync like
    /// any other block.
    pub fn add_pdf_highlight(
        &mut self,
        hash: &str,
        host: Option<&BlockId>,
        pdf_page: usize,
        rects: &[[f32; 4]],
        quote: &str,
    ) -> Result<(PageId, BlockId)> {
        self.sync_index()?;
        // The block that attaches this PDF (not a highlight that merely mentions it).
        let attaches = |text: &str| blob_refs(text).iter().any(|r| r.hash == hash);
        let given = match host {
            Some(h) => self
                .block_row(h)?
                .filter(|r| r.deleted_at.is_none() && attaches(&r.text))
                .map(|r| (r.page_id, r.id)),
            None => None,
        };
        // Without a known host (opened from a link), the oldest block that attaches it.
        let fallback = || -> Result<Option<(PageId, BlockId)>> {
            let mut stmt = self.conn.prepare_cached(
                "SELECT id, page_id, text FROM block WHERE deleted_at IS NULL AND instr(text, ?1) > 0
                 ORDER BY created_at",
            )?;
            let rows = stmt.query_map([format!("blob:{hash}")], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (b, p, text) = row?;
                if attaches(&text) {
                    return Ok(Some((PageId(p), BlockId(b))));
                }
            }
            Ok(None)
        };
        let (page, parent) = match given.map(Ok).or_else(|| fallback().transpose()) {
            Some(r) => {
                let (p, b) = r?;
                (p, Some(b))
            }
            None => (self.ensure_daily_page(self.today())?, None),
        };
        let quote = quote.split_whitespace().collect::<Vec<_>>().join(" ");
        let text = if quote.is_empty() {
            highlight_markup(hash, pdf_page, rects)
        } else {
            format!("{quote} {}", highlight_markup(hash, pdf_page, rects))
        };
        self.seal_undo();
        let plan = outline::append_block(self.page_state(&page)?, parent.as_ref());
        let block = self
            .execute_plan(&page, plan, None)?
            .ok_or_else(|| StoreError::Invalid("could not add the highlight".into()))?
            .block;
        self.set_block_text(&page, &block, &text, None, None)?;
        self.set_block_kind(&page, &block, BlockKind::Quote, false)?;
        self.commit_block(&page, &block)?;
        Ok((page, block))
    }
}

#[cfg(test)]
mod tests {
    use super::BlobState;

    /// The column is the state (§11.6): every value round-trips, and anything else is read
    /// as pending rather than panicking or claiming bytes are on a relay (rule 11).
    #[test]
    fn attachment_state_reads_every_value_and_tolerates_unknown_ones() {
        assert_eq!(BlobState::Pending.as_str(), "pending");
        assert_eq!(BlobState::Uploaded.as_str(), "uploaded");
        assert_eq!(BlobState::Lost { noticed_at: 7 }.as_str(), "lost");
        assert_eq!(BlobState::from_row("pending", None), BlobState::Pending);
        assert_eq!(BlobState::from_row("uploaded", None), BlobState::Uploaded);
        assert_eq!(
            BlobState::from_row("lost", Some(7)),
            BlobState::Lost { noticed_at: 7 }
        );
        // The CHECK constraint keeps these in step; a file that slipped through reads as
        // lost-at-epoch instead of panicking.
        assert_eq!(
            BlobState::from_row("lost", None),
            BlobState::Lost { noticed_at: 0 }
        );
        assert_eq!(BlobState::from_row("pending", Some(7)), BlobState::Pending);
        assert_eq!(BlobState::from_row("bogus", None), BlobState::Pending);
    }
}
