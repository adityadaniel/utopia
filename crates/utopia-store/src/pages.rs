//! Page lifecycle: creation (§10.3), daily pages (§5), commit-time normalization (§6.5),
//! rename and merge (§8.6), trash (AC-1.5), reference rewrites (FR-3.6, FR-4.14), and the
//! import-time convergence rules (dedupe, re-merge).

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use rusqlite::{params, OptionalExtension};
use utopia_core::command::PageMetaPatch;
use utopia_core::date::{iso, parse_unambiguous_date};
use utopia_core::inline::{extract_refs, heal_tags, rewrite_page_refs, unlink_page_refs};
use utopia_core::text::{diff, nfc};
use utopia_core::title::{clean_title, normalize_title};
use utopia_core::{BlockId, BlockKind, Command, DomainError, PageId, PageKind, PageState};
use utopia_doc::PageDoc;

use crate::project::resolve_page_key;
use crate::{DraftDay, EditOpts, Result, Store, StoreError};

/// The page lifecycle as one value (§6.5, AC-1.5; docs/state-machines/page-lifecycle-trash.md).
///
/// The CRDT side is the only source of truth: `PageMeta.deleted_at` says whether the page is
/// tombstoned and `merged_into` says why (a rename onto an existing title, rule 8). The local
/// `trash` row and the `tombstone` memory are bookkeeping derived from it, never a second
/// opinion about the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageFate {
    /// In the index: its title or date is live.
    Live,
    /// Tombstoned with no merge pointer (AC-1.5). Listed in the trash while restorable.
    Trashed { at: i64 },
    /// Tombstoned with a pointer to the page its blocks moved into (rule 8). Never restored:
    /// the content lives in the survivor.
    Merged { into: PageId, at: i64 },
    /// Hard-deleted by Empty Trash: no shard and no page row, only a local `tombstone` row.
    /// The id is never issued again (rule 4).
    Purged { at: i64 },
}

impl PageFate {
    /// The fate of a page that still has its shard. A page with a shard is never `Purged`.
    pub fn of(state: &PageState) -> Self {
        match (state.meta.deleted_at, state.meta.merged_into.clone()) {
            (None, _) => PageFate::Live,
            (Some(at), Some(into)) => PageFate::Merged { into, at },
            (Some(at), None) => PageFate::Trashed { at },
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, PageFate::Live)
    }

    /// When the page was tombstoned; `None` while it is live.
    pub fn tombstoned_at(&self) -> Option<i64> {
        match self {
            PageFate::Live => None,
            PageFate::Trashed { at } | PageFate::Merged { at, .. } | PageFate::Purged { at } => {
                Some(*at)
            }
        }
    }

    /// The local `trash` row this fate owns: `Some(at)` only for `Trashed`. A merged page's
    /// content is not in the trash, and a live page has no row at all.
    pub fn trashed_at(&self) -> Option<i64> {
        match self {
            PageFate::Trashed { at } => Some(*at),
            _ => None,
        }
    }
}

impl Store {
    fn live_daily(&self, date: NaiveDate) -> Result<Option<PageId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM page WHERE kind='daily' AND daily_date=?1 AND deleted_at IS NULL",
                [iso(date)],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(PageId))
    }

    /// Whether `id` was ever used by a page on this device: live, tombstoned, purged by Empty
    /// Trash (the local `tombstone` memory), an in-memory shard that is queued or written, a
    /// durable shard (`page_doc`), or an id that is still a session alias for the page a
    /// merged promotion moved to (FR-1.2). An alias is reserved: reusing it would make every
    /// read of the new page follow the old survivor.
    fn id_taken(&self, id: &PageId) -> Result<bool> {
        Ok(self.promotions.contains_key(id)
            || self.claims_id(id)
            || self.query_exists("SELECT EXISTS(SELECT 1 FROM page_doc WHERE page_id=?1)", id)?
            || self.query_exists("SELECT EXISTS(SELECT 1 FROM page WHERE id=?1)", id)?
            // Empty Trash leaves no page row behind; the memory is what keeps the id reserved
            // (rule 4, `PageFate::Purged`).
            || self.query_exists("SELECT EXISTS(SELECT 1 FROM tombstone WHERE id=?1)", id)?)
    }

    fn query_exists(&self, sql: &str, id: &PageId) -> Result<bool> {
        Ok(self.conn.query_row(sql, [id.as_str()], |r| r.get(0))?)
    }

    /// This device's view of a page's lifecycle
    /// (docs/state-machines/page-lifecycle-trash.md). `Purged` is the state Empty Trash
    /// leaves behind: no shard and no page row, only the local memory that reserves the id.
    /// `None` for an id this graph has never seen.
    pub fn page_fate(&mut self, page: &PageId) -> Result<Option<PageFate>> {
        if self.page_exists(page)? {
            return Ok(Some(PageFate::of(self.page_state(page)?)));
        }
        self.read_barrier()?;
        let at: Option<i64> = self
            .conn
            .query_row(
                "SELECT deleted_at FROM tombstone WHERE id=?1",
                [page.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.map(|at| PageFate::Purged { at }))
    }

    /// Whether an in-memory shard for `id` counts as the page existing. A shard that was
    /// built but never queued claims nothing: a page whose identity never applied must not
    /// make the next create issue a fresh ULID for a title no page ever used (§10.3).
    fn claims_id(&self, id: &PageId) -> bool {
        self.docs.get(id).is_some_and(|e| e.has_snapshot)
            || self.pending.iter().any(|(p, _, _)| p == id)
    }

    /// Build a page's shard with its identity metadata and put it in `docs`, ready to be
    /// queued. Nothing enters `docs` before the metadata applied, so a failed build cannot
    /// leave a shard that looks like an existing page (§10.3, `id_taken`).
    fn install_new_page(&mut self, id: &PageId, meta: PageMetaPatch) -> Result<()> {
        let now = self.now();
        let mut entry = self.fresh_entry(id);
        entry.doc.apply(&Command::SetPageMeta(meta), now)?;
        self.docs.insert(id.clone(), entry);
        Ok(())
    }

    /// Resolve `title` to a page, creating it if nothing matches (`CreatePage`, App. C).
    /// Never duplicates: dates go to their daily page, existing titles and aliases win, and a
    /// tombstoned deterministic id is never resurrected (§10.3).
    pub fn create_page(&mut self, title: &str) -> Result<PageId> {
        self.sync_index()?;
        let title = clean_title(title);
        if title.is_empty() {
            return Err(DomainError::EmptyTitle.into());
        }
        let key = normalize_title(&title);
        if let Some(d) = parse_unambiguous_date(&key, &self.date_format) {
            return self.ensure_daily_page(d);
        }
        if let Some(p) = resolve_page_key(&self.conn, &key, &self.date_format)? {
            return Ok(p);
        }
        let mut id = PageId::for_title_key(&key);
        if self.id_taken(&id)? {
            id = PageId::fresh();
        }
        let now = self.now();
        self.install_new_page(
            &id,
            PageMetaPatch {
                title: Some(title),
                kind: Some(PageKind::Page),
                created_at: Some(now),
                ..Default::default()
            },
        )?;
        self.queue(&[(id.clone(), None)], true)?;
        self.flush()?;
        Ok(id)
    }

    /// The live daily page for `date`, created empty if absent (FR-1.5, FR-1.10). A draft
    /// of that date is written out instead, so the date never gets two pages.
    pub fn ensure_daily_page(&mut self, date: NaiveDate) -> Result<PageId> {
        self.sync_index()?;
        if let Some(p) = self.live_daily(date)? {
            // A draft of a day that now has a page is stale (FR-1.2): retire it for the day.
            self.retire_stale_draft(date, &p);
            return Ok(p);
        }
        if let Some(p) = self.draft_page(date) {
            self.queue(&[(p.clone(), None)], true)?;
            return Ok(p);
        }
        let mut id = PageId::for_date(date);
        if self.id_taken(&id)? {
            id = PageId::fresh();
        }
        let now = self.now();
        self.install_new_page(
            &id,
            PageMetaPatch {
                kind: Some(PageKind::Daily),
                daily_date: Some(Some(date)),
                created_at: Some(now),
                ..Default::default()
            },
        )?;
        self.queue(&[(id.clone(), None)], true)?;
        self.flush()?;
        Ok(id)
    }

    /// Visiting a date (FR-1.2): its live page, or else a draft held only in memory — one
    /// empty block, or the journal template — that is written to the graph (and synced)
    /// by its first edit. A day that is only looked at never becomes a page.
    pub fn draft_daily(&mut self, date: NaiveDate) -> Result<PageId> {
        self.sync_index()?;
        if let Some(p) = self.live_daily(date)? {
            // A draft of a day that now has a page is stale (FR-1.2): retire it for the day.
            self.retire_stale_draft(date, &p);
            return Ok(p);
        }
        if let Some(p) = self.draft_page(date) {
            return Ok(p);
        }
        let mut id = PageId::for_date(date);
        if self.id_taken(&id)? {
            id = PageId::fresh();
        }
        let template = self.find_page(&format!("{}journal", crate::TEMPLATE_PREFIX))?;
        let mut cmds = match template {
            Some(t) => self.template_cmds(&t, None, None)?,
            None => Vec::new(),
        };
        if cmds.is_empty() {
            cmds.push(Command::InsertBlock {
                id: BlockId::new(),
                parent: None,
                after: None,
                text: String::new(),
                kind: BlockKind::Bullet,
            });
        }
        let now = self.now();
        let mut entry = self.fresh_entry(&id);
        entry.doc.apply(
            &Command::SetPageMeta(PageMetaPatch {
                kind: Some(PageKind::Daily),
                daily_date: Some(Some(date)),
                created_at: Some(now),
                ..Default::default()
            }),
            now,
        )?;
        for cmd in &cmds {
            // Nothing was inserted into `drafts` yet, so a failure leaves no trace.
            entry.doc.apply(cmd, now)?;
        }
        self.drafts.insert(id.clone(), DraftDay { date, entry });
        Ok(id)
    }

    /// Append an empty root block to a draft day, in memory only (FR-1.2): the click that
    /// starts writing must not write the day, even when a journal template already gave it
    /// blocks. Nothing is queued, projected or flushed; the first keystroke promotes the
    /// whole draft, this block included, in one write.
    pub fn draft_append_block(&mut self, page: &PageId) -> Result<BlockId> {
        if !self.is_draft(page) {
            return Err(StoreError::Invalid("not a draft day".into()));
        }
        let after = self.page_state(page)?.last_child(None, false);
        let id = BlockId::new();
        let now = self.now();
        let entry = self
            .drafts
            .get_mut(page)
            .ok_or_else(|| StoreError::NotFound(format!("draft {page}")))?;
        entry.entry.doc.apply(
            &Command::InsertBlock {
                id: id.clone(),
                parent: None,
                after,
                text: String::new(),
                kind: BlockKind::Bullet,
            },
            now,
        )?;
        Ok(id)
    }

    /// The oldest daily page that exists on this device: the day the Daily Notes feed
    /// finishes loading at (FR-1.2). Drafts are excluded on purpose: a day that was only
    /// visited is not a page, so it must not extend the feed (a visit to 2019 would
    /// otherwise re-open the feed for the whole session). The first write promotes the
    /// draft and the next feed rebuild picks it up. Trashed days are excluded too.
    pub fn oldest_daily(&self) -> Result<Option<NaiveDate>> {
        self.read_barrier()?;
        Ok(self
            .conn
            .query_row(
                "SELECT MIN(daily_date) FROM page WHERE kind='daily' AND deleted_at IS NULL",
                [],
                |r| r.get::<_, Option<String>>(0),
            )?
            .and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()))
    }

    /// Daily pages dated `from..=to`, drafts included: what the Daily Notes feed shows for
    /// those days (the rest are blank days, written only if you type on them).
    pub fn days_between(
        &self,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<std::collections::BTreeMap<NaiveDate, PageId>> {
        self.read_barrier()?;
        let mut out = std::collections::BTreeMap::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT daily_date, id FROM page WHERE kind='daily' AND deleted_at IS NULL
               AND daily_date BETWEEN ?1 AND ?2",
        )?;
        let rows = stmt
            .query_map(params![iso(from), iso(to)], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (d, id) in rows {
            if let Ok(d) = NaiveDate::parse_from_str(&d, "%Y-%m-%d") {
                out.insert(d, PageId(id));
            }
        }
        for (p, d) in &self.drafts {
            if (from..=to).contains(&d.date) {
                out.entry(d.date).or_insert_with(|| p.clone());
            }
        }
        Ok(out)
    }

    /// Whether `page` is a draft day (shown, never written). A draft always has its shard:
    /// it lives inside the draft entry (FR-1.2).
    pub fn is_draft(&self, page: &PageId) -> bool {
        self.drafts.contains_key(page)
    }

    /// The draft for `date`, if the day has been looked at but never written (FR-1.2).
    pub fn draft_page(&self, date: NaiveDate) -> Option<PageId> {
        self.drafts
            .iter()
            .find(|(_, d)| d.date == date)
            .map(|(p, _)| p.clone())
    }

    /// Promote a draft (FR-1.2): the shard moves from the draft entry into `docs`, so the
    /// next commit writes the whole page. Returns the day it belonged to. This is the only
    /// Draft → Live transition.
    pub(crate) fn promote_draft(&mut self, page: &PageId) -> Option<NaiveDate> {
        let draft = self.drafts.remove(page)?;
        self.docs.insert(page.clone(), draft.entry);
        Some(draft.date)
    }

    /// Forget a draft without writing it: its shard was only ever in memory.
    pub(crate) fn discard_draft(&mut self, page: &PageId) {
        self.drafts.remove(page);
    }

    /// Retire a stale draft of `date` for the page that already owns the day (FR-1.2): its
    /// shard was never written, so it is dropped, its id becomes a session alias for the
    /// survivor, and views still showing it are told to retarget.
    fn retire_stale_draft(&mut self, date: NaiveDate, survivor: &PageId) {
        let Some(draft) = self.draft_page(date) else {
            return;
        };
        if draft == *survivor {
            return;
        }
        self.discard_draft(&draft);
        self.promotions.insert(draft.clone(), survivor.clone());
        self.replacements.push((draft, survivor.clone()));
    }

    /// Replacements the views have not been told about yet: `(from, to)`. The UI announces
    /// them as page replacements so a view showing `from` follows `to` (FR-1.2).
    pub fn take_replacements(&mut self) -> Vec<(PageId, PageId)> {
        std::mem::take(&mut self.replacements)
    }

    /// Where a promoted draft's page id went when another page already owned its day
    /// (FR-1.2). The id was never durable, so it is a session alias: views and the editor
    /// that still hold it follow the day instead of a shard that no longer exists (rule 5).
    /// `None` for every page that is not such an alias.
    pub fn redirect(&self, page: &PageId) -> Option<PageId> {
        let mut at = page.clone();
        for _ in 0..8 {
            match self.promotions.get(&at) {
                Some(to) => at = to.clone(),
                None => return (at != *page).then_some(at),
            }
        }
        None
    }

    /// A promoted draft (FR-1.2) may belong to a day another page already owns: a remote
    /// device wrote the date under a fresh id because this device's uuidv5 is tombstoned
    /// there (rule 4). Writing the draft as a second live daily page would violate
    /// `page_daily_uq` and fail the commit, so its blocks move into the page that has the
    /// day instead (§8.6).
    ///
    /// The draft's shard is dropped, not written as a `merged_into` tombstone: a draft was
    /// never durable, so it has no identity to merge away, and a tombstone for the same id
    /// would let `reconcile_merged` drag the remote's still-trashed blocks into the live day
    /// (rule 5). The discarded id is remembered as a session alias ([`Store::redirect`]) so
    /// the editor and the views editing it follow the day. Returns the surviving page, which
    /// the caller must commit instead.
    pub(crate) fn resolve_promotion(
        &mut self,
        page: &PageId,
        date: NaiveDate,
    ) -> Result<Option<PageId>> {
        let Some(live) = self.live_daily(date)? else {
            return Ok(None);
        };
        if live == *page {
            return Ok(None);
        }
        let from = self.page_state(page)?.clone();
        let into = self.page_state(&live)?.clone();
        let (cmds, _moved) = Self::plan_block_moves(&from, &into);
        let now = self.now();
        if !cmds.is_empty() {
            self.apply_cmds(&live, &cmds, now, false)?;
        }
        self.docs.remove(page);
        self.promotions.insert(page.clone(), live.clone());
        self.replacements.push((page.clone(), live.clone()));
        Ok(Some(live))
    }

    /// Visiting a page: make sure there is a block to type into (§9.3) and return the
    /// first visible one. Not an undoable edit.
    pub fn ensure_first_block(&mut self, page: &PageId) -> Result<BlockId> {
        let st = self.page_state(page)?;
        if let Some(first) = st.live_roots().first() {
            return Ok(first.clone());
        }
        let id = BlockId::new();
        let after = st.last_child(None, false);
        self.execute(
            page,
            vec![Command::InsertBlock {
                id: id.clone(),
                parent: None,
                after,
                text: String::new(),
                kind: BlockKind::Bullet,
            }],
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )?;
        Ok(id)
    }

    /// Open a date: create its page lazily and put a block in it (FR-1.2, AC-1.1).
    pub fn open_daily(&mut self, date: NaiveDate) -> Result<(PageId, BlockId)> {
        let page = self.ensure_daily_page(date)?;
        if let Some(first) = self.fill_new_daily(&page)? {
            return Ok((page, first));
        }
        let block = self.ensure_first_block(&page)?;
        Ok((page, block))
    }

    fn is_live_title(&self, phrase: &str) -> bool {
        let key = normalize_title(phrase);
        parse_unambiguous_date(&key, &self.date_format).is_none()
            && resolve_page_key(&self.conn, &key, &self.date_format)
                .ok()
                .flatten()
                .is_some()
    }

    /// Commit a block (blur, Enter, or idle; AC-3.2): NFC-normalize, apply the hashtag
    /// longest-match rule, and create pages for new `[[links]]`, tags and dates so they
    /// exist immediately (W3, FR-1.10). Returns whether the text was rewritten.
    pub fn commit_block(&mut self, page: &PageId, block: &BlockId) -> Result<bool> {
        self.sync_index()?;
        let Some(text) = self
            .page_state(page)?
            .block(block)
            .filter(|b| b.live)
            .map(|b| b.text.clone())
        else {
            return Ok(false);
        };
        let mut new = nfc(&text);
        if let Some(healed) = heal_tags(&new, |p| self.is_live_title(p)) {
            new = healed;
        }
        let rewritten = new != text && self.set_block_text(page, block, &new, None, None)?;
        let kind = self.page_state(page)?.block(block).map(|b| b.kind);
        if kind != Some(BlockKind::Code) {
            for r in extract_refs(&new) {
                if !r.is_page_like() {
                    continue;
                }
                let key = normalize_title(&r.target);
                if resolve_page_key(&self.conn, &key, &self.date_format)?.is_none() {
                    self.create_page(&r.target)?;
                }
            }
        }
        Ok(rewritten)
    }

    /// Rename (FR-3.6, FR-4.12, FR-4.13, §5.5). Returns the surviving page: `page` itself,
    /// or the page it merged into when the new title collides or is a date.
    pub fn rename_page(&mut self, page: &PageId, new_title: &str) -> Result<PageId> {
        self.sync_index()?;
        let meta = self.page_state(page)?.meta.clone();
        if meta.kind == PageKind::Daily {
            return Err(DomainError::DailyTitleReadOnly.into());
        }
        let new = clean_title(new_title);
        if new.is_empty() {
            return Err(DomainError::EmptyTitle.into());
        }
        let key = normalize_title(&new);
        if let Some(d) = parse_unambiguous_date(&key, &self.date_format) {
            let daily = self.ensure_daily_page(d)?;
            self.merge_pages(page, &daily)?;
            return Ok(daily);
        }
        let old_key = normalize_title(&meta.title);
        if key != old_key {
            if let Some(other) = resolve_page_key(&self.conn, &key, &self.date_format)? {
                if other != *page {
                    self.merge_pages(page, &other)?;
                    return Ok(other);
                }
            }
        }
        if new == meta.title {
            return Ok(page.clone());
        }
        let mut aliases = meta.aliases.clone();
        if key != old_key && !meta.title.is_empty() {
            aliases.push(meta.title.clone());
        }
        self.seal_undo();
        self.execute(
            page,
            vec![Command::SetPageMeta(PageMetaPatch {
                title: Some(new),
                aliases: Some(aliases),
                ..Default::default()
            })],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(page.clone())
    }

    /// Commands that move every block of `from` not yet moved into `into`, preserving ids
    /// and subtrees (§8.6 step 1). Blocks whose parent already lives in `into` attach to
    /// it; others become roots appended after `into`'s last root.
    fn plan_block_moves(from: &PageState, into: &PageState) -> (Vec<Command>, Vec<BlockId>) {
        let mut cmds = Vec::new();
        let mut moved: Vec<BlockId> = Vec::new();
        let mut in_batch: HashSet<BlockId> = HashSet::new();
        let mut last_under: HashMap<Option<BlockId>, Option<BlockId>> = HashMap::new();
        let mut order = Vec::new();
        for r in &from.roots {
            order.extend(from.subtree(r, false));
        }
        for x in order {
            let Some(node) = from.block(&x) else { continue };
            if node.moved_to.is_some() {
                continue;
            }
            moved.push(x.clone());
            if into.contains(&x) {
                // Already there (a concurrent re-merge won); only hide it here.
                in_batch.insert(x.clone());
                continue;
            }
            let parent = node
                .parent
                .clone()
                .filter(|p| in_batch.contains(p) || into.contains(p));
            let after = last_under
                .entry(parent.clone())
                .or_insert_with(|| match &parent {
                    Some(p) if in_batch.contains(p) && !into.contains(p) => None,
                    p => into.last_child(p.as_ref(), false),
                })
                .clone();
            cmds.push(Command::InsertBlock {
                id: x.clone(),
                parent: parent.clone(),
                after,
                text: node.text.clone(),
                kind: node.kind,
            });
            if node.checked {
                cmds.push(Command::SetBlockKind {
                    id: x.clone(),
                    kind: node.kind,
                    checked: true,
                });
            }
            if let Some(d) = node.deleted_at {
                cmds.push(Command::SetDeleted {
                    ids: vec![x.clone()],
                    deleted_at: Some(d),
                });
            }
            last_under.insert(parent, Some(x.clone()));
            in_batch.insert(x);
        }
        (cmds, moved)
    }

    /// The full merge plan (§8.6): blocks move, aliases transfer, `from` is tombstoned
    /// with a pointer to the survivor. Emits only ordinary commands, so sync needs no
    /// special merge message (step 6).
    fn plan_merge(&mut self, from: &PageId, into: &PageId) -> Result<Vec<(PageId, Vec<Command>)>> {
        if from == into {
            return Err(DomainError::NoOp.into());
        }
        let b = self.page_state(from)?.clone();
        let a = self.page_state(into)?.clone();
        let now = self.now();
        let (mut cmds_a, moved) = Self::plan_block_moves(&b, &a);
        let mut aliases = a.meta.aliases.clone();
        aliases.extend(b.meta.aliases.iter().cloned());
        if b.meta.kind == PageKind::Page && !b.meta.title.is_empty() {
            aliases.push(b.meta.title.clone());
        }
        cmds_a.push(Command::SetPageMeta(PageMetaPatch {
            aliases: Some(aliases),
            ..Default::default()
        }));
        let mut cmds_b = Vec::new();
        if !moved.is_empty() {
            cmds_b.push(Command::SetMoved {
                ids: moved,
                to: Some(into.clone()),
            });
        }
        cmds_b.push(Command::SetPageMeta(PageMetaPatch {
            deleted_at: Some(Some(b.meta.deleted_at.unwrap_or(now))),
            merged_into: Some(Some(into.clone())),
            ..Default::default()
        }));
        Ok(vec![(from.clone(), cmds_b), (into.clone(), cmds_a)])
    }

    /// `MergePages` (§8.6): one transaction, one undo step.
    pub fn merge_pages(&mut self, from: &PageId, into: &PageId) -> Result<()> {
        self.sync_index()?;
        let steps = self.plan_merge(from, into)?;
        self.seal_undo();
        self.execute_multi(steps, EditOpts::default())?;
        self.seal_undo();
        Ok(())
    }

    /// Delete a page to the trash (AC-1.5). Undoable; restorable for 30 days. The trash row
    /// is part of the commit, not a second write: the writer puts it beside the tombstone
    /// (docs/state-machines/page-lifecycle-trash.md issues 1 and 2).
    pub fn delete_page(&mut self, page: &PageId) -> Result<()> {
        let st = self.page_state(page)?.clone();
        if !PageFate::of(&st).is_live() {
            return Err(DomainError::NoOp.into());
        }
        let now = self.now();
        // This device owns the page's trash row from here on, including across a failed
        // commit: the repair rewrites the shard and the writer puts the row beside it.
        self.own_trash.insert(page.clone());
        self.seal_undo();
        self.execute(
            page,
            vec![Command::SetPageMeta(PageMetaPatch {
                deleted_at: Some(Some(now)),
                ..Default::default()
            })],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(())
    }

    /// Restore a trashed page (AC-1.5). If its title or date is taken by a live page
    /// meanwhile, it merges into that page instead of creating a duplicate. Returns the
    /// surviving page: the page itself, or the one it merged into.
    pub fn restore_page(&mut self, page: &PageId) -> Result<PageId> {
        self.sync_index()?;
        let meta = self.page_state(page)?.meta.clone();
        match PageFate::of(self.page_state(page)?) {
            PageFate::Live => {
                // A row that outlived its tombstone (a graph written before the row rode the
                // tombstone): queue the shard so the writer drops it. No behaviour change for
                // a page that never had one.
                self.clear_stale_trash_row(page)?;
                return Ok(page.clone());
            }
            // The blocks already live in the survivor (rule 8). Restoring a merged page means
            // going there, never resurrecting a tile with nothing under it.
            PageFate::Merged { .. } => return self.merge_survivor(page),
            // A purged page has no shard, so the read above already failed; keeping the arm
            // makes the four states exhaustive here.
            PageFate::Purged { .. } => return Err(StoreError::NotFound(format!("page {page}"))),
            PageFate::Trashed { .. } => {}
        }
        let existing = match (meta.kind, meta.daily_date) {
            (PageKind::Daily, Some(d)) => self.live_daily(d)?,
            _ => resolve_page_key(&self.conn, &normalize_title(&meta.title), &self.date_format)?,
        };
        let survivor = match existing {
            Some(other) if other != *page => {
                self.merge_pages(page, &other)?;
                other
            }
            _ => {
                // Clearing both fields is a normal commit: the writer drops the trash row in
                // the same transaction.
                self.execute(
                    page,
                    vec![Command::SetPageMeta(PageMetaPatch {
                        deleted_at: Some(None),
                        merged_into: Some(None),
                        ..Default::default()
                    })],
                    EditOpts::default(),
                )?;
                page.clone()
            }
        };
        Ok(survivor)
    }

    /// The end of a chain of merges (§8.6): `merged_into` may point at a page that was merged
    /// away later (A onto B, then B onto C), and only the last one holds the blocks. Bounded
    /// like `reconcile_merged`, so a cycle in the pointers can never hang a read.
    fn merge_survivor(&mut self, page: &PageId) -> Result<PageId> {
        let mut current = page.clone();
        for _ in 0..8 {
            match PageFate::of(self.page_state(&current)?) {
                PageFate::Merged { into, .. } => current = into,
                _ => break,
            }
        }
        Ok(current)
    }

    /// Rewrite `page` even though its shard did not change, so the writer can drop a trash
    /// row that no longer belongs to its fate. Only a page this device put in the trash has a
    /// row at all, so this is a no-op for every other page.
    fn clear_stale_trash_row(&mut self, page: &PageId) -> Result<()> {
        if !self.own_trash.contains(page) {
            return Ok(());
        }
        self.push_pending(page, None, false);
        self.flush()
    }

    /// Trashed pages that are still tombstoned: `(id, title, deleted_at)`. A row exists only
    /// for the `Trashed` fate, so this list is the trash and nothing else (the join keeps a
    /// row that outlived its tombstone out of the UI).
    pub fn list_trash(&self) -> Result<Vec<(PageId, String, i64)>> {
        self.read_barrier()?;
        let mut stmt = self.conn.prepare(
            "SELECT t.id, p.title, t.deleted_at FROM trash t JOIN page p ON p.id=t.id
             WHERE p.deleted_at IS NOT NULL ORDER BY t.deleted_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((PageId(r.get(0)?), r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Hard-delete trashed pages deleted before `cutoff_ms` (all when `None`). The only
    /// operation that permanently removes data (§6.5).
    pub fn empty_trash(&mut self, cutoff_ms: Option<i64>) -> Result<usize> {
        self.sync_index()?;
        let ids: Vec<PageId> = self
            .list_trash()?
            .into_iter()
            .filter(|(_, _, at)| cutoff_ms.is_none_or(|c| *at < c))
            .map(|(id, _, _)| id)
            .collect();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for id in &ids {
            tx.execute("DELETE FROM ref WHERE src_page_id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM block WHERE page_id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM page_update WHERE page_id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM page WHERE id=?1", [id.as_str()])?;
            tx.execute("DELETE FROM trash WHERE id=?1", [id.as_str()])?;
        }
        tx.commit()?;
        for id in &ids {
            self.docs.remove(id);
            // The page is gone: it can own no trash row again (its id is never reissued).
            self.own_trash.remove(id);
        }
        // The purge removed rows the classification reads, without a commit to publish the
        // set the remaining index implies (SB-2).
        self.republish_tag_pages()?;
        Ok(ids.len())
    }

    fn rewrite_blocks(
        &mut self,
        blocks: Vec<(PageId, BlockId)>,
        rewrite: impl Fn(&str) -> Option<String>,
    ) -> Result<usize> {
        let mut steps: Vec<(PageId, Vec<Command>)> = Vec::new();
        let mut n = 0;
        for (page, block) in blocks {
            let Some(text) = self
                .page_state(&page)?
                .block(&block)
                .map(|b| b.text.clone())
            else {
                continue;
            };
            let Some(new) = rewrite(&text) else { continue };
            let Some(edit) = diff(&text, &new) else {
                continue;
            };
            n += 1;
            let cmd = Command::SetBlockText { id: block, edit };
            match steps.iter_mut().find(|(p, _)| *p == page) {
                Some((_, v)) => v.push(cmd),
                None => steps.push((page, vec![cmd])),
            }
        }
        if n > 0 {
            self.seal_undo();
            self.execute_multi(steps, EditOpts::default())?;
            self.seal_undo();
        }
        Ok(n)
    }

    /// "Rewrite references to new title" (FR-3.6): every link that reaches this page
    /// through an alias is rewritten to the current title, as one undo step.
    pub fn rewrite_references(&mut self, page: &PageId) -> Result<usize> {
        self.sync_index()?;
        let meta = self.page_state(page)?.meta.clone();
        if meta.kind == PageKind::Daily {
            return Ok(0);
        }
        let alias_keys: HashSet<String> = meta.aliases.iter().map(|a| normalize_title(a)).collect();
        let blocks: Vec<(PageId, BlockId)> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT b.page_id, b.id FROM ref r JOIN block b ON b.id = r.src_block_id
                 WHERE r.dst_page_id=?1 AND r.dst_kind='page' AND b.deleted_at IS NULL",
            )?;
            let rows = stmt
                .query_map([page.as_str()], |r| {
                    Ok((PageId(r.get(0)?), BlockId(r.get(1)?)))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        let title = meta.title.clone();
        self.rewrite_blocks(blocks, |text| {
            rewrite_page_refs(text, |t| alias_keys.contains(&normalize_title(t)), &title)
        })
    }

    /// Unresolved intent → "Redirect to an existing page" (FR-4.14), within one block.
    pub fn redirect_intent(&mut self, block: &BlockId, key: &str, to: &PageId) -> Result<()> {
        self.sync_index()?;
        let page = self.block_page(block)?;
        let to_title = self
            .page_row(to)?
            .ok_or_else(|| StoreError::NotFound(format!("page {to}")))?
            .title;
        let key = normalize_title(key);
        self.rewrite_blocks(vec![(page, block.clone())], |text| {
            rewrite_page_refs(text, |t| normalize_title(t) == key, &to_title)
        })?;
        Ok(())
    }

    /// Unresolved intent → "Convert to plain text" (FR-4.14), within one block.
    pub fn unlink_intent(&mut self, block: &BlockId, key: &str) -> Result<()> {
        self.sync_index()?;
        let page = self.block_page(block)?;
        let key = normalize_title(key);
        self.rewrite_blocks(vec![(page, block.clone())], |text| {
            unlink_page_refs(text, |t| normalize_title(t) == key)
        })?;
        Ok(())
    }

    /// Import-time deduplication (§8.6): if `page` is live and its title (or date) matches
    /// another live page, merge the larger id into the smaller one so every device picks
    /// the same survivor. Applies commands in memory; returns the other pages touched.
    pub(crate) fn dedupe_collisions(&mut self, page: &PageId) -> Result<Vec<PageId>> {
        let st = self.page_state(page)?.clone();
        if !PageFate::of(&st).is_live() {
            return Ok(Vec::new());
        }
        let other: Option<PageId> = match (st.meta.kind, st.meta.daily_date) {
            (PageKind::Daily, Some(d)) => self
                .conn
                .query_row(
                    "SELECT id FROM page WHERE kind='daily' AND daily_date=?1 AND deleted_at IS NULL AND id<>?2",
                    params![iso(d), page.as_str()],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(PageId),
            _ => self
                .conn
                .query_row(
                    "SELECT id FROM page WHERE kind='page' AND title_key=?1 AND deleted_at IS NULL AND id<>?2",
                    params![normalize_title(&st.meta.title), page.as_str()],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .map(PageId),
        };
        let Some(other) = other else {
            return Ok(Vec::new());
        };
        let (from, into) = if *page < other {
            (other.clone(), page.clone())
        } else {
            (page.clone(), other.clone())
        };
        let now = self.now();
        for (p, cmds) in self.plan_merge(&from, &into)? {
            self.apply_cmds(&p, &cmds, now, false)?;
        }
        Ok(vec![other])
    }

    /// Convergent merge rule (§8.6): blocks that arrive in a merged-away page after the
    /// merge are moved into the survivor (following chains of merges).
    pub(crate) fn reconcile_merged(&mut self, page: &PageId) -> Result<bool> {
        let mut current = page.clone();
        let mut changed = false;
        for _ in 0..8 {
            let st = self.page_state(&current)?.clone();
            let PageFate::Merged { into, .. } = PageFate::of(&st) else {
                break;
            };
            let has_orphans = st.blocks.values().any(|b| b.moved_to.is_none());
            if has_orphans && self.page_exists(&into)? {
                let a = self.page_state(&into)?.clone();
                let (cmds_a, moved) = Self::plan_block_moves(&st, &a);
                let steps = vec![
                    (
                        current.clone(),
                        vec![Command::SetMoved {
                            ids: moved,
                            to: Some(into.clone()),
                        }],
                    ),
                    (into.clone(), cmds_a),
                ];
                self.execute_multi(
                    steps,
                    EditOpts {
                        no_undo: true,
                        ..Default::default()
                    },
                )?;
                changed = true;
            }
            current = into;
        }
        Ok(changed)
    }

    /// Import a batch of remote updates (§12.3 step 3): import every shard, then dedupe
    /// and project page by page, then re-merge orphans. Idempotent.
    pub fn import_batch(&mut self, batch: &[(PageId, Vec<u8>)]) -> Result<Vec<PageId>> {
        self.sync_index()?;
        let mut changed: Vec<PageId> = Vec::new();
        for (page, bytes) in batch {
            // Another device wrote this day first: drop the local draft so its unsent
            // empty block never merges into theirs.
            self.discard_draft(page);
            // A shard for the id of a draft that promotion merged away makes that id a page
            // of its own again: drop the session alias, or identity logic (dedupe, restore)
            // would act on the day instead of the page this shard describes (rule 4).
            self.promotions.remove(page);
            self.replacements.retain(|(from, _)| from != page);
            let did = if self.page_exists(page)? {
                self.load_doc(page)?.doc.import(bytes)
            } else {
                self.new_doc(page).doc.import(bytes)
            };
            let did = match did {
                Ok(did) => did,
                Err(e) => {
                    // The shards imported so far were never committed (the loop below did
                    // not run), so they exist only in memory. Drop every page of the batch
                    // so a retry re-imports its payload instead of seeing an unchanged
                    // version vector and skipping it (§12.3 steps 3–4).
                    for (p, _) in batch {
                        self.docs.remove(p);
                    }
                    return Err(e.into());
                }
            };
            if did && !changed.contains(page) {
                changed.push(page.clone());
            }
        }
        for page in &changed {
            let others = self.dedupe_collisions(page)?;
            let local_ops = !others.is_empty();
            let mut pages = vec![(page.clone(), None)];
            pages.extend(others.into_iter().map(|p| (p, None)));
            // Merge commands generated here are new local ops and must reach the relay.
            // Each page is committed before the next one's dedupe check reads the index.
            self.queue(&pages, local_ops)?;
            self.sync_index()?;
        }
        for page in &changed {
            self.reconcile_merged(page)?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/state-machines/page-lifecycle-trash.md`: restore on a merged page is now an
    /// explicit no-op that points at the survivor. It used to clear both tombstone fields
    /// and leave a live page whose every block had `moved_to` the survivor: a page with
    /// nothing to draw. Rule 5 keeps its tiles; it does not resurrect them.
    #[test]
    fn ac_4_6_restoring_a_merged_page_returns_the_survivor() {
        let mut s = Store::open_in_memory().unwrap();
        let from = s.create_page("From").unwrap();
        let into = s.create_page("Into").unwrap();
        s.merge_pages(&from, &into).unwrap();
        assert!(
            matches!(
                PageFate::of(s.page_state(&from).unwrap()),
                PageFate::Merged { .. }
            ),
            "a rename onto an existing title leaves a merge tombstone"
        );

        assert_eq!(s.restore_page(&from).unwrap(), into);
        assert!(
            matches!(
                PageFate::of(s.page_state(&from).unwrap()),
                PageFate::Merged { .. }
            ),
            "restore must not resurrect a merged page (rule 8)"
        );
    }

    /// Review: merge chains exist (A onto B, then B onto C, §8.6), so A's `merged_into`
    /// points at a page that is itself a tombstone. Restore must resolve the *end* of the
    /// chain — where the blocks actually live — not the intermediate hop.
    #[test]
    fn ac_4_6_restoring_a_page_merged_twice_returns_the_final_survivor() {
        let mut s = Store::open_in_memory().unwrap();
        let a = s.create_page("A").unwrap();
        let b = s.create_page("B").unwrap();
        let c = s.create_page("C").unwrap();
        let block = s.ensure_first_block(&a).unwrap();
        s.set_block_text(&a, &block, "moved twice", None, None)
            .unwrap();
        s.merge_pages(&a, &b).unwrap();
        s.merge_pages(&b, &c).unwrap();

        let texts: Vec<String> = {
            let st = s.page_state(&c).unwrap();
            st.live_preorder()
                .into_iter()
                .map(|(id, _)| st.block(&id).unwrap().text.clone())
                .collect()
        };
        assert!(
            texts.iter().any(|t| t == "moved twice"),
            "the blocks reached the end of the chain: {texts:?}"
        );
        assert_eq!(
            s.restore_page(&a).unwrap(),
            c,
            "the survivor is the end of the merge chain, not the hop A points at"
        );
        assert!(
            matches!(
                PageFate::of(s.page_state(&a).unwrap()),
                PageFate::Merged { .. }
            ),
            "restore must not resurrect a merged page (rule 8)"
        );
    }

    /// `docs/state-machines/draft-daily-page.md` issue 4: a shard built for a page whose
    /// metadata never applied stayed in `docs`, so `id_taken` reported the title's
    /// deterministic id taken and the next create issued a fresh ULID for a page that was
    /// never tombstoned (rule 4). An in-memory shard that was never written claims nothing.
    #[test]
    fn fr_1_2_unwritten_shard_does_not_claim_its_id() {
        let mut s = Store::open_in_memory().unwrap();
        let id = PageId::for_title_key(&normalize_title("Ana"));
        // The shape the error path used to leave behind: a built shard that was never
        // queued for writing.
        s.new_doc(&id);
        assert_eq!(s.create_page("Ana").unwrap(), id);
    }
}
