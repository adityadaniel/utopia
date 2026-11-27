//! v1.1 (P1) features from §4.2: unlinked references and promote-to-link (FR-3.9,
//! FR-3.11), aliases UI, journal and page templates, extract block to page (FR-2.18),
//! cross-page moves, block types, quick capture (FR-2.19), Roam JSON import, OPML export.

use std::collections::HashSet;
use std::ops::Range;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use utopia_core::command::PageMetaPatch;
use utopia_core::date::parse_unambiguous_date;
use utopia_core::inline::{extract_refs, find_mentions, page_link_text};
use utopia_core::title::{clean_title, normalize_title};
use utopia_core::{outline, BlockId, BlockKind, Command, PageId, PageState};

use crate::project::{display_title, resolve_page_key};
use crate::{EditOpts, Result, Store, StoreError};

/// Prefix of template pages: `templates/journal` is applied to new daily pages; any
/// `templates/<name>` can be inserted from the slash menu.
pub const TEMPLATE_PREFIX: &str = "templates/";

/// An unlinked mention of a page's title in another page's block (FR-3.9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mention {
    pub block: BlockId,
    pub page: PageId,
    pub page_title: String,
    pub text: String,
    pub range: Range<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub pages: usize,
    pub blocks: usize,
}

fn fts_phrase(title: &str) -> Option<String> {
    let words: Vec<String> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect();
    (!words.is_empty()).then(|| format!("\"{}\"", words.join(" ")))
}

impl Store {
    // ---------------------------------------------------------------- unlinked refs

    /// Plain-text mentions of the page's title or aliases outside any reference, in
    /// other pages' live blocks (FR-3.9). Found through FTS, confirmed by the parser.
    /// This form makes pending edits durable first, so it must not run in a render path:
    /// use [`Store::unlinked_references_now`] for that.
    pub fn unlinked_references(&mut self, page: &PageId) -> Result<Vec<Mention>> {
        self.sync_index()?;
        self.unlinked_mentions(page)
    }

    /// The same mentions, read from the index as it stands: no flush and no wait for the
    /// writer (LR-5, rule 10). The answer can lag one commit; the caller runs it again when
    /// the index reports it is durable.
    pub fn unlinked_references_now(&mut self, page: &PageId) -> Result<Vec<Mention>> {
        self.read_barrier()?;
        self.unlinked_mentions(page)
    }

    fn unlinked_mentions(&mut self, page: &PageId) -> Result<Vec<Mention>> {
        self.unlinked_scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let st = self.page_state(page)?.clone();
        let mut names = vec![display_title(&st, &self.date_format)];
        names.extend(st.meta.aliases.iter().cloned());
        let mut out = Vec::new();
        let mut seen: HashSet<(BlockId, usize)> = HashSet::new();
        for name in names {
            let Some(q) = fts_phrase(&name) else { continue };
            let rows: Vec<(String, String, String, String)> = {
                let mut stmt = self.conn.prepare_cached(
                    "SELECT f.block_id, b.page_id, p.title, b.text FROM block_fts f
                     JOIN block b ON b.rowid=f.rowid JOIN page p ON p.id=b.page_id
                     WHERE block_fts MATCH ?1 AND b.page_id<>?2 ORDER BY f.rowid DESC LIMIT 2000",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![q, page.as_str()], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (block, src_page, title, text) in rows {
                for range in find_mentions(&text, &name) {
                    if seen.insert((BlockId(block.clone()), range.start)) {
                        out.push(Mention {
                            block: BlockId(block.clone()),
                            page: PageId(src_page.clone()),
                            page_title: title.clone(),
                            text: text.clone(),
                            range,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    /// "Link" on an unlinked mention (FR-3.11, AC-3.7): rewrite exactly that span to a
    /// link to `page`, as one undo step.
    pub fn promote_mention(
        &mut self,
        block: &BlockId,
        range: Range<usize>,
        page: &PageId,
    ) -> Result<()> {
        self.sync_index()?;
        let src_page = self.block_page(block)?;
        let target = self.page_state(page)?.clone();
        let title = display_title(&target, &self.date_format);
        let mut names = vec![title.clone()];
        names.extend(target.meta.aliases.iter().cloned());
        let text = self
            .page_state(&src_page)?
            .block(block)
            .map(|b| b.text.clone())
            .ok_or_else(|| StoreError::NotFound(format!("block {block}")))?;
        let valid = names
            .iter()
            .any(|n| find_mentions(&text, n).contains(&range));
        if !valid {
            return Err(StoreError::Invalid("not an unlinked mention".into()));
        }
        let insert = page_link_text(&title, false);
        self.seal_undo();
        self.execute(
            &src_page,
            vec![Command::SetBlockText {
                id: block.clone(),
                edit: utopia_core::TextEdit { range, insert },
            }],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(())
    }

    /// "Link all" (LR-5): rewrite every unlinked mention of `page` as a link, in one undo
    /// step and one transaction. Mentions inside one block are rewritten from the end back,
    /// so the byte offsets of earlier ones stay valid.
    pub fn promote_all_mentions(&mut self, page: &PageId) -> Result<usize> {
        self.sync_index()?;
        let target = self.page_state(page)?.clone();
        let title = display_title(&target, &self.date_format);
        let insert = page_link_text(&title, false);
        let mentions = self.unlinked_references(page)?;
        if mentions.is_empty() {
            return Ok(0);
        }
        let mut steps: std::collections::BTreeMap<
            PageId,
            std::collections::BTreeMap<BlockId, Vec<Range<usize>>>,
        > = std::collections::BTreeMap::new();
        for m in &mentions {
            steps
                .entry(m.page.clone())
                .or_default()
                .entry(m.block.clone())
                .or_default()
                .push(m.range.clone());
        }
        let mut page_steps = Vec::with_capacity(steps.len());
        let mut linked = 0;
        for (src_page, blocks) in steps {
            let mut cmds = Vec::new();
            for (block, mut ranges) in blocks {
                // A title and one of its aliases can match the same words ("New York" and
                // "York"), so the list holds overlapping ranges. Keep the earliest, longest
                // span and drop anything that overlaps it: two edits over the same bytes
                // would rewrite each other's replacements.
                ranges.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
                let mut kept: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
                for range in ranges {
                    if kept.last().is_none_or(|last| range.start >= last.end) {
                        kept.push(range);
                    }
                }
                // Apply from the end back so earlier byte offsets stay valid.
                for range in kept.into_iter().rev() {
                    linked += 1;
                    cmds.push(Command::SetBlockText {
                        id: block.clone(),
                        edit: utopia_core::TextEdit {
                            range,
                            insert: insert.clone(),
                        },
                    });
                }
            }
            page_steps.push((src_page, cmds));
        }
        self.execute_multi(page_steps, EditOpts::default())?;
        Ok(linked)
    }

    // ---------------------------------------------------------------- aliases UI

    /// Add an alias (P1 "aliases UI"). Refused when date-shaped or taken by another page
    /// (§10.4 invariant 8).
    pub fn add_alias(&mut self, page: &PageId, alias: &str) -> Result<()> {
        self.sync_index()?;
        let alias = clean_title(alias);
        let key = normalize_title(&alias);
        if key.is_empty() {
            return Err(StoreError::Invalid("alias is empty".into()));
        }
        if parse_unambiguous_date(&key, &self.date_format).is_some() {
            return Err(StoreError::Invalid("aliases cannot be dates".into()));
        }
        if let Some(other) = resolve_page_key(&self.conn, &key, &self.date_format)? {
            if other != *page {
                return Err(StoreError::Invalid(format!(
                    "\"{alias}\" already names another page"
                )));
            }
        }
        let mut aliases = self.page_state(page)?.meta.aliases.clone();
        aliases.push(alias);
        self.set_aliases(page, aliases)
    }

    pub fn remove_alias(&mut self, page: &PageId, alias: &str) -> Result<()> {
        let key = normalize_title(alias);
        let aliases: Vec<String> = self
            .page_state(page)?
            .meta
            .aliases
            .iter()
            .filter(|a| normalize_title(a) != key)
            .cloned()
            .collect();
        self.set_aliases(page, aliases)
    }

    fn set_aliases(&mut self, page: &PageId, aliases: Vec<String>) -> Result<()> {
        self.seal_undo();
        self.execute(
            page,
            vec![Command::SetPageMeta(PageMetaPatch {
                aliases: Some(aliases),
                ..Default::default()
            })],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(())
    }

    // ---------------------------------------------------------------- block types

    /// Switch a block's type (FR-2.13): bullet, numbered, todo, quote, code, heading.
    pub fn set_block_kind(
        &mut self,
        page: &PageId,
        block: &BlockId,
        kind: BlockKind,
        checked: bool,
    ) -> Result<()> {
        self.seal_undo();
        self.execute(
            page,
            vec![Command::SetBlockKind {
                id: block.clone(),
                kind,
                checked,
            }],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(())
    }

    // ---------------------------------------------------------------- templates

    /// Template pages: `(page, name)` for every live `templates/<name>` page.
    pub fn template_pages(&mut self) -> Result<Vec<(PageId, String)>> {
        Ok(self
            .list_pages(false)?
            .into_iter()
            .filter_map(|p| {
                p.title_key
                    .strip_prefix(TEMPLATE_PREFIX)
                    .filter(|n| !n.is_empty())
                    .map(|n| (p.id.clone(), n.to_string()))
            })
            .collect())
    }

    /// Commands copying `template`'s live blocks (fresh ids) under `parent` after `after`.
    pub(crate) fn template_cmds(
        &mut self,
        template: &PageId,
        parent: Option<&BlockId>,
        after: Option<&BlockId>,
    ) -> Result<Vec<Command>> {
        let t = self.page_state(template)?.clone();
        let mut cmds = Vec::new();
        let mut last_root = after.cloned();
        fn copy(
            t: &PageState,
            id: &BlockId,
            parent: Option<BlockId>,
            after: Option<BlockId>,
            cmds: &mut Vec<Command>,
        ) -> BlockId {
            let b = t.block(id).expect("live child exists");
            let new = BlockId::new();
            cmds.push(Command::InsertBlock {
                id: new.clone(),
                parent,
                after,
                text: b.text.clone(),
                kind: b.kind,
            });
            if b.checked {
                cmds.push(Command::SetBlockKind {
                    id: new.clone(),
                    kind: b.kind,
                    checked: true,
                });
            }
            let mut prev = None;
            for c in t.live_children(Some(id)) {
                prev = Some(copy(t, &c, Some(new.clone()), prev, cmds));
            }
            new
        }
        for r in t.live_roots() {
            last_root = Some(copy(&t, &r, parent.cloned(), last_root, &mut cmds));
        }
        Ok(cmds)
    }

    /// Insert a page template after `after` (slash menu). One undo step; returns the first
    /// inserted block.
    pub fn apply_template(
        &mut self,
        page: &PageId,
        parent: Option<&BlockId>,
        after: Option<&BlockId>,
        template: &PageId,
    ) -> Result<Option<BlockId>> {
        let cmds = self.template_cmds(template, parent, after)?;
        let first = cmds.iter().find_map(|c| c.block_id().cloned());
        if cmds.is_empty() {
            return Ok(None);
        }
        self.seal_undo();
        self.execute(page, cmds, EditOpts::default())?;
        self.seal_undo();
        Ok(first)
    }

    /// Opening a date fills a brand-new daily page from `templates/journal` when it
    /// exists (P1 journal template); otherwise one empty block (§9.3).
    pub(crate) fn fill_new_daily(&mut self, page: &PageId) -> Result<Option<BlockId>> {
        if !self.page_state(page)?.live_roots().is_empty() {
            return Ok(None);
        }
        let template = self.find_page(&format!("{TEMPLATE_PREFIX}journal"))?;
        let Some(template) = template.filter(|t| t != page) else {
            return Ok(None);
        };
        let cmds = self.template_cmds(&template, None, None)?;
        if cmds.is_empty() {
            return Ok(None);
        }
        let first = cmds.iter().find_map(|c| c.block_id().cloned());
        self.execute(
            page,
            cmds,
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )?;
        Ok(first)
    }

    // ---------------------------------------------------------------- moves across pages

    /// Commands moving `block` (and its subtree) from `src` to `dst` under `parent` after
    /// `after`, keeping every block id (cross-page drag, P1; extract, FR-2.18).
    fn cross_page_move(
        &mut self,
        src: &PageId,
        block: &BlockId,
        dst: &PageId,
        parent: Option<&BlockId>,
        after: Option<&BlockId>,
    ) -> Result<(Vec<Command>, Vec<Command>)> {
        let s = self.page_state(src)?.clone();
        if !s.is_live(block) {
            return Err(StoreError::NotFound(format!("block {block}")));
        }
        let ids = s.subtree(block, false);
        // FR-2.9's ceiling applies to a drop into another page too, exactly as
        // `outline::move_to` applies it on one page: the subtree keeps its shape, so it lands
        // `own` levels lower than it is now. It is measured on the blocks that are live — the
        // same set `move_to` measures — because a subtree that already left this page keeps
        // its rows as `moved_to` history (the loop below skips them) and a tombstone has no
        // depth to push: counting them refused a one-block subtree that would land two levels
        // down because the chain it used to hang from was still in the shard.
        let d = self.page_state(dst)?.clone();
        let base = parent.map_or(0, |p| d.depth(p) + 1);
        let own = s.depth(block);
        let max_rel = ids
            .iter()
            .filter(|b| s.is_live(b))
            .map(|b| s.depth(b).saturating_sub(own))
            .max()
            .unwrap_or(0);
        if base + max_rel >= outline::MAX_DROP_DEPTH {
            return Err(StoreError::Domain(utopia_core::DomainError::TooDeep));
        }
        let mut dst_cmds = Vec::new();
        for id in &ids {
            let b = s.block(id).expect("subtree member");
            if b.moved_to.is_some() {
                continue;
            }
            let (p, a) = if id == block {
                (parent.cloned(), after.cloned())
            } else {
                let p = b.parent.clone();
                let a = s.prev_sibling(id, false).filter(|x| ids.contains(x));
                (p, a)
            };
            dst_cmds.push(Command::InsertBlock {
                id: id.clone(),
                parent: p,
                after: a,
                text: b.text.clone(),
                kind: b.kind,
            });
            if b.checked {
                dst_cmds.push(Command::SetBlockKind {
                    id: id.clone(),
                    kind: b.kind,
                    checked: true,
                });
            }
            if let Some(d) = b.deleted_at {
                dst_cmds.push(Command::SetDeleted {
                    ids: vec![id.clone()],
                    deleted_at: Some(d),
                });
            }
        }
        let src_cmds = vec![Command::SetMoved {
            ids,
            to: Some(dst.clone()),
        }];
        Ok((src_cmds, dst_cmds))
    }

    /// Drag a block to another page (P1 "drag across pages"). One undo step.
    pub fn move_block_to_page(
        &mut self,
        src: &PageId,
        block: &BlockId,
        dst: &PageId,
        parent: Option<&BlockId>,
        after: Option<&BlockId>,
    ) -> Result<()> {
        if src == dst {
            return Err(StoreError::Invalid("use a same-page move".into()));
        }
        let (src_cmds, dst_cmds) = self.cross_page_move(src, block, dst, parent, after)?;
        self.seal_undo();
        self.execute_multi(
            vec![(dst.clone(), dst_cmds), (src.clone(), src_cmds)],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(())
    }

    pub fn restore_and_move_to_page(
        &mut self,
        src: &PageId,
        blocks: &[BlockId],
        dst: &PageId,
        parent: Option<&BlockId>,
        after: Option<&BlockId>,
    ) -> Result<()> {
        if src == dst {
            return Err(StoreError::Invalid("use a same-page restore".into()));
        }
        if blocks.is_empty() {
            return Err(StoreError::Invalid("nothing to restore".into()));
        }
        let s = self.page_state(src)?.clone();
        for b in blocks {
            match s.block(b) {
                None => return Err(StoreError::NotFound(format!("block {b}"))),
                Some(n) if n.moved_to.is_some() => {
                    return Err(utopia_core::DomainError::NoOp.into())
                }
                Some(n) if n.deleted_at.is_some() => {}
                Some(_) => {
                    return Err(StoreError::Invalid(format!(
                        "block {b} is live, not a tombstone: copy it instead"
                    )))
                }
            }
        }
        let roots: Vec<BlockId> = blocks
            .iter()
            .filter(|b| !s.parent_of(b).is_some_and(|p| blocks.contains(&p)))
            .cloned()
            .collect();
        let d = self.page_state(dst)?.clone();
        if let Some(p) = parent {
            d.block(p)
                .ok_or_else(|| StoreError::NotFound(format!("block {p}")))?;
        }
        let own = roots.first().map(|r| s.depth(r)).unwrap_or(0);
        let deepest = blocks.iter().map(|b| s.depth(b)).max().unwrap_or(own);
        let base = parent.map_or(0, |p| d.depth(p) + 1);
        if base + deepest.saturating_sub(own) >= outline::MAX_DROP_DEPTH {
            return Err(StoreError::Domain(utopia_core::DomainError::TooDeep));
        }
        let mut dst_cmds: Vec<Command> = Vec::new();
        let mut after = after.cloned();
        for root in &roots {
            for id in s.subtree(root, false) {
                if !blocks.contains(&id) {
                    continue;
                }
                let Some(n) = s.block(&id) else { continue };
                let p = if &id == root {
                    parent.cloned()
                } else if n.parent.as_ref().is_some_and(|p| blocks.contains(p)) {
                    n.parent.clone()
                } else {
                    s.ancestors(&id)
                        .into_iter()
                        .rev()
                        .find(|a| blocks.contains(a))
                        .or_else(|| parent.cloned())
                };
                let a = if &id == root {
                    after.clone()
                } else {
                    let mut prev = s.prev_sibling(&id, false);
                    while let Some(prev_id) = prev.clone() {
                        if blocks.contains(&prev_id) {
                            break;
                        }
                        prev = s.prev_sibling(&prev_id, false);
                    }
                    prev
                };
                if d.block(&id).is_some() {
                    if d.is_live(&id) {
                        return Err(StoreError::Invalid(format!(
                            "block {id} is already live on this page"
                        )));
                    }
                    dst_cmds.push(Command::SetMoved {
                        ids: vec![id.clone()],
                        to: None,
                    });
                    dst_cmds.push(Command::SetDeleted {
                        ids: vec![id.clone()],
                        deleted_at: None,
                    });
                    dst_cmds.push(Command::MoveBlock {
                        id: id.clone(),
                        parent: p,
                        after: a,
                    });
                } else {
                    dst_cmds.push(Command::InsertBlock {
                        id: id.clone(),
                        parent: p,
                        after: a,
                        text: n.text.clone(),
                        kind: n.kind,
                    });
                    if n.checked {
                        dst_cmds.push(Command::SetBlockKind {
                            id: id.clone(),
                            kind: n.kind,
                            checked: true,
                        });
                    }
                }
            }
            after = Some(root.clone());
        }
        let src_cmds = vec![Command::SetMoved {
            ids: blocks.to_vec(),
            to: Some(dst.clone()),
        }];
        self.seal_undo();
        self.execute_multi(
            vec![(dst.clone(), dst_cmds), (src.clone(), src_cmds)],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(())
    }

    /// Move one block and its live subtree within its own page (PRD §13.8 MC-8, MC-9;
    /// FR-2.9): `Op::MoveBlock` when the target page is the page the block is already on.
    /// One undo step, like the editor's drag. A drop into the block's own subtree
    /// ([`utopia_core::DomainError::CycleDetected`]) or one that would push a descendant past
    /// [`outline::MAX_DROP_DEPTH`] fails before anything is written. Returns the number of
    /// live blocks that moved.
    pub fn move_block(
        &mut self,
        page: &PageId,
        block: &BlockId,
        parent: Option<&BlockId>,
        after: Option<&BlockId>,
    ) -> Result<usize> {
        let (plan, moved) = {
            let st = self.page_state(page)?;
            let plan = outline::move_to(st, block, parent, after)?;
            let moved = st.subtree(block, true).len();
            (plan, moved)
        };
        self.seal_undo();
        self.execute_plan(page, plan, None)?;
        self.seal_undo();
        Ok(moved)
    }

    /// Tombstone a block and its live subtree (PRD §13.8 MC-8; rule 5, §6.5):
    /// `Op::DeleteBlock`. The rows keep their ids, so a `((reference))` to any of them still
    /// resolves and reads "deleted block", and one undo puts the subtree back. Returns the
    /// number of live blocks that became tombstones.
    pub fn delete_block(&mut self, page: &PageId, block: &BlockId) -> Result<usize> {
        let now = self.now();
        let (plan, deleted) = {
            let st = self.page_state(page)?;
            let plan = outline::delete_subtree(st, block, now)?;
            let deleted = st.subtree(block, true).len();
            (plan, deleted)
        };
        self.seal_undo();
        self.execute_plan(page, plan, None)?;
        self.seal_undo();
        Ok(deleted)
    }

    /// Extract block to page (FR-2.18): the block and its subtree move (same ids) to the
    /// page `title` (created if needed); a `[[title]]` link takes its place. One undo step.
    pub fn extract_block_to_page(
        &mut self,
        src: &PageId,
        block: &BlockId,
        title: &str,
    ) -> Result<PageId> {
        let dst = self.create_page(title)?;
        if dst == *src {
            return Err(StoreError::Invalid(
                "cannot extract a block into its own page".into(),
            ));
        }
        let after = self.page_state(&dst)?.last_child(None, false);
        let (mut src_cmds, dst_cmds) =
            self.cross_page_move(src, block, &dst, None, after.as_ref())?;
        let s = self.page_state(src)?.clone();
        let node = s.block(block).expect("checked live");
        let dst_title = self
            .page_row(&dst)?
            .map(|r| r.title)
            .unwrap_or_else(|| clean_title(title));
        src_cmds.push(Command::InsertBlock {
            id: BlockId::new(),
            parent: node.parent.clone(),
            after: Some(block.clone()),
            text: page_link_text(&dst_title, false),
            kind: BlockKind::Bullet,
        });
        self.seal_undo();
        self.execute_multi(
            vec![(dst.clone(), dst_cmds), (src.clone(), src_cmds)],
            EditOpts::default(),
        )?;
        self.seal_undo();
        Ok(dst)
    }

    // ---------------------------------------------------------------- quick capture

    /// Global quick capture (FR-2.19): append `text` to today's daily note and commit it.
    pub fn capture(&mut self, text: &str) -> Result<(PageId, BlockId)> {
        self.end_undo_run();
        let (page, first) = self.open_daily(self.today())?;
        let st = self.page_state(&page)?;
        let roots = st.live_roots();
        let reuse = roots.len() == 1 && st.block(&first).is_some_and(|b| b.text.is_empty());
        let block = if reuse {
            first
        } else {
            let plan = utopia_core::outline::append_block(st, None);
            self.execute_plan(&page, plan, None)?
                .map(|f| f.block)
                .ok_or_else(|| StoreError::Invalid("capture failed".into()))?
        };
        self.set_block_text(&page, &block, text, None, None)?;
        self.commit_block(&page, &block)?;
        self.seal_undo();
        Ok((page, block))
    }

    // ---------------------------------------------------------------- Roam import

    /// Import a Roam JSON export (§4.1, adoption path). Pages are matched by title (dates
    /// become daily pages); block uids are kept so `((refs))` keep working; `{{[[TODO]]}}`
    /// and `{{[[DONE]]}}` become todo blocks; Roam's `__italic__` becomes `_italic_`.
    pub fn import_roam_json(&mut self, json: &str) -> Result<ImportReport> {
        let pages: Vec<Value> = serde_json::from_str(json)?;
        let mut report = ImportReport::default();
        let mut used: HashSet<String> = HashSet::new();
        for p in &pages {
            let Some(title) = p.get("title").and_then(Value::as_str) else {
                continue;
            };
            let page = self.create_page(title)?;
            report.pages += 1;
            let mut cmds = Vec::new();
            let mut after = self.page_state(&page)?.last_child(None, false);
            for child in p
                .get("children")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                after =
                    Some(self.roam_block(child, None, after, &mut cmds, &mut used, &mut report));
            }
            if !cmds.is_empty() {
                self.execute(
                    &page,
                    cmds,
                    EditOpts {
                        no_undo: true,
                        ..Default::default()
                    },
                )?;
            }
        }
        // Pages referenced but not exported (Roam omits some) are created like on commit.
        self.sync_index()?;
        let unresolved: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT DISTINCT b.text FROM ref r JOIN block b ON b.id=r.src_block_id
                 WHERE r.dst_kind='page' AND r.dst_page_id IS NULL",
            )?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        for text in unresolved {
            for r in extract_refs(&text).into_iter().filter(|r| r.is_page_like()) {
                if self.find_page(&r.target)?.is_none() {
                    self.create_page(&r.target)?;
                }
            }
        }
        self.sync_index()?;
        Ok(report)
    }

    fn roam_block(
        &mut self,
        v: &Value,
        parent: Option<BlockId>,
        after: Option<BlockId>,
        cmds: &mut Vec<Command>,
        used: &mut HashSet<String>,
        report: &mut ImportReport,
    ) -> BlockId {
        let raw = v.get("string").and_then(Value::as_str).unwrap_or_default();
        let uid = v.get("uid").and_then(Value::as_str).unwrap_or_default();
        let taken = !BlockId::is_valid_ref(uid)
            || used.contains(uid)
            || self
                .block_row(&BlockId(uid.to_string()))
                .ok()
                .flatten()
                .is_some();
        let id = if taken {
            BlockId::new()
        } else {
            BlockId(uid.to_string())
        };
        used.insert(id.0.clone());
        let (mut kind, mut checked, mut text) = (BlockKind::Bullet, false, raw.to_string());
        for (marker, done) in [
            ("{{[[TODO]]}}", false),
            ("{{TODO}}", false),
            ("{{[[DONE]]}}", true),
            ("{{DONE}}", true),
        ] {
            if let Some(rest) = text.strip_prefix(marker) {
                kind = BlockKind::Todo;
                checked = done;
                text = rest.trim_start().to_string();
                break;
            }
        }
        if let Some(h) = v
            .get("heading")
            .and_then(Value::as_u64)
            .filter(|h| (1..=3).contains(h))
        {
            kind = BlockKind::Heading(h as u8);
        }
        text = text.replace("__", "_");
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent,
            after,
            text: utopia_core::text::nfc(&text),
            kind,
        });
        if checked {
            cmds.push(Command::SetBlockKind {
                id: id.clone(),
                kind,
                checked: true,
            });
        }
        report.blocks += 1;
        let mut prev = None;
        for c in v
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            prev = Some(self.roam_block(c, Some(id.clone()), prev, cmds, used, report));
        }
        id
    }

    // ---------------------------------------------------------------- OPML

    /// OPML export of every live page (v1.1).
    pub fn export_opml(&mut self, path: &Path) -> Result<()> {
        let snapshot = self.export_snapshot()?;
        crate::export::write_opml(&snapshot, path)
    }
}
