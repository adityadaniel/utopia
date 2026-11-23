//! The CRDT boundary (PRD §13.2): one Loro document per page ("shard").
//!
//! Layout of a shard:
//! - map `page`: `title`, `kind`, `daily_date`, `created_at`, `deleted_at`,
//!   `merged_into`, and a nested map `titles` (title_key → display) holding the current
//!   title plus every alias. Concurrent renames therefore keep the losing title as an
//!   alias (§12.4).
//! - tree `blocks` (fractional index enabled): one node per block, whose meta map holds
//!   `id` (ULID), `text` (LoroText), `kind`, `level`, `checked`, `deleted_at`,
//!   `moved_to`, `created_at`, `updated_at`.
//!
//! Loro is deliberately confined to this crate so it can be swapped (§11.4).

use std::collections::HashMap;

use chrono::NaiveDate;
use loro::{
    Container, ExportMode, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue, TreeID, TreeParentId,
    ValueOrContainer, VersionVector,
};
use thiserror::Error;
use utopia_core::command::{canonical_aliases, PageMetaPatch};
use utopia_core::title::normalize_title;
use utopia_core::{
    BlockId, BlockKind, BlockNode, Command, DocDelta, DomainError, PageId, PageKind, PageMeta,
    PageState,
};

const PAGE: &str = "page";
const TITLES: &str = "titles";
const TREE: &str = "blocks";

#[derive(Debug, Error)]
pub enum DocError {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("crdt error: {0}")]
    Crdt(String),
    #[error("corrupt shard: {0}")]
    Corrupt(String),
}

impl From<loro::LoroError> for DocError {
    fn from(e: loro::LoroError) -> Self {
        DocError::Crdt(e.to_string())
    }
}

pub type Result<T, E = DocError> = std::result::Result<T, E>;

/// Opaque version identifier (encoded version vector, hex) — `page_doc.frontier`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Frontier(pub String);

impl Frontier {
    pub fn from_vv(vv: &VersionVector) -> Self {
        Frontier(hex(&vv.encode()))
    }

    pub fn to_vv(&self) -> Result<VersionVector> {
        if self.0.is_empty() {
            return Ok(VersionVector::default());
        }
        let bytes = unhex(&self.0).ok_or_else(|| DocError::Corrupt("bad frontier".into()))?;
        VersionVector::decode(&bytes).map_err(DocError::from)
    }
}

/// One peer's changes that wait for earlier changes of a page (§12.3 step 3): counters
/// `start..end` are held, and they need that peer's changes before `start` (and whatever
/// those depend on) to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WaitingRange {
    pub peer: u64,
    pub start: u64,
    pub end: u64,
}

/// The peers and counters in bytes from [`PageDoc::waiting`], for diagnostics. It reads
/// the blob's header only, so it costs nothing like an import.
pub fn waiting_ranges(bytes: &[u8]) -> Result<Vec<WaitingRange>> {
    let meta = LoroDoc::decode_import_blob_meta(bytes, false)
        .map_err(|e| DocError::Corrupt(e.to_string()))?;
    let mut out: Vec<WaitingRange> = meta
        .partial_end_vv
        .iter()
        .map(|(peer, end)| WaitingRange {
            peer: *peer,
            start: meta
                .partial_start_vv
                .get(peer)
                .map_or(0, |c| (*c).max(0) as u64),
            end: (*end).max(0) as u64,
        })
        .filter(|r| r.end > r.start)
        .collect();
    out.sort();
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// The narrow seam between the domain and the CRDT (§13.2).
pub trait PageDoc: Sized {
    fn new(page_id: PageId, peer: u64) -> Self;
    fn from_snapshot(page_id: PageId, bytes: &[u8], peer: u64) -> Result<Self>;
    fn snapshot(&self) -> Result<Vec<u8>>;
    fn frontier(&self) -> Frontier;
    fn export_since(&self, f: &Frontier) -> Result<Vec<u8>>;
    /// Idempotent. Returns whether anything new was applied.
    ///
    /// Remote changes whose causal predecessors this doc does not have are kept, not
    /// applied: [`PageDoc::waiting`] exports them so the caller can store them durably
    /// (§12.3 step 3, §2.3.7). They apply on the import that brings the missing changes.
    fn import(&mut self, bytes: &[u8]) -> Result<bool>;
    /// Remote changes this doc received but cannot apply yet, because changes they depend
    /// on never arrived; `None` when nothing waits. The bytes go back in with
    /// [`PageDoc::import`] and stay waiting until the missing changes come. Snapshots and
    /// [`PageDoc::export_since`] never contain them, so a caller that keeps only a snapshot
    /// loses them (§2.3.7).
    fn waiting(&self) -> Result<Option<Vec<u8>>>;
    /// Commands are the only mutation path.
    fn apply(&mut self, cmd: &Command, now: i64) -> Result<DocDelta>;
    fn state(&self) -> &PageState;
}

pub struct LoroPageDoc {
    doc: LoroDoc,
    state: PageState,
    index: HashMap<BlockId, TreeID>,
}

fn str_of(v: Option<ValueOrContainer>) -> Option<String> {
    match v? {
        ValueOrContainer::Value(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn i64_of(v: Option<ValueOrContainer>) -> Option<i64> {
    match v? {
        ValueOrContainer::Value(LoroValue::I64(i)) => Some(i),
        ValueOrContainer::Value(LoroValue::Double(d)) => Some(d as i64),
        _ => None,
    }
}

fn bool_of(v: Option<ValueOrContainer>) -> Option<bool> {
    match v? {
        ValueOrContainer::Value(LoroValue::Bool(b)) => Some(b),
        _ => None,
    }
}

fn opt_i64(v: Option<i64>) -> LoroValue {
    v.map_or(LoroValue::Null, LoroValue::I64)
}

fn opt_str(v: Option<&str>) -> LoroValue {
    v.map_or(LoroValue::Null, |s| LoroValue::String(s.to_string().into()))
}

fn text_of(meta: &LoroMap) -> Option<LoroText> {
    match meta.get("text")? {
        ValueOrContainer::Container(Container::Text(t)) => Some(t),
        _ => None,
    }
}

impl LoroPageDoc {
    fn tree(&self) -> LoroTree {
        self.doc.get_tree(TREE)
    }

    fn page_map(&self) -> LoroMap {
        self.doc.get_map(PAGE)
    }

    fn titles_map(&self) -> Result<LoroMap> {
        // Mergeable: two peers creating it concurrently converge on one map.
        Ok(self.page_map().ensure_mergeable_map(TITLES)?)
    }

    fn wrap(doc: LoroDoc, page_id: PageId, peer: u64) -> Result<Self> {
        doc.set_peer_id(peer)?;
        doc.get_tree(TREE).enable_fractional_index(0);
        let mut me = Self {
            doc,
            state: PageState::new(page_id, PageMeta::default()),
            index: HashMap::new(),
        };
        me.rebuild();
        Ok(me)
    }

    /// Open a shard from its snapshot plus any later local updates.
    pub fn load(page_id: PageId, snapshot: &[u8], updates: &[Vec<u8>], peer: u64) -> Result<Self> {
        let doc = LoroDoc::from_snapshot(snapshot).map_err(|e| DocError::Corrupt(e.to_string()))?;
        for u in updates {
            doc.import(u)
                .map_err(|e| DocError::Corrupt(e.to_string()))?;
        }
        Self::wrap(doc, page_id, peer)
    }

    pub fn version(&self) -> VersionVector {
        self.doc.oplog_vv()
    }

    pub fn export_updates(&self, from: &VersionVector) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::updates(from))
            .map_err(|e| DocError::Crdt(e.to_string()))
    }

    fn tid(&self, id: &BlockId) -> Result<TreeID> {
        self.index
            .get(id)
            .copied()
            .ok_or_else(|| DocError::Domain(DomainError::BlockNotFound(id.clone())))
    }

    fn read_meta(&self) -> PageMeta {
        let m = self.page_map();
        let title = str_of(m.get("title")).unwrap_or_default();
        let kind = match str_of(m.get("kind")).as_deref() {
            Some("daily") => PageKind::Daily,
            _ => PageKind::Page,
        };
        let daily_date = str_of(m.get("daily_date"))
            .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok());
        let mut titles: Vec<(String, String)> = Vec::new();
        if let Some(ValueOrContainer::Container(Container::Map(tm))) = m.get(TITLES) {
            tm.for_each(|k, v| {
                if let ValueOrContainer::Value(LoroValue::String(s)) = v {
                    titles.push((k.to_string(), s.to_string()));
                }
            });
        }
        titles.sort();
        let title_key = normalize_title(&title);
        let aliases = titles
            .into_iter()
            .filter(|(k, _)| *k != title_key)
            .map(|(_, v)| v)
            .collect();
        PageMeta {
            title,
            kind,
            daily_date,
            created_at: i64_of(m.get("created_at")).unwrap_or(0),
            deleted_at: i64_of(m.get("deleted_at")),
            merged_into: str_of(m.get("merged_into")).map(PageId),
            aliases,
        }
    }

    /// Rebuild the cached [`PageState`] and id index from the Loro tree.
    ///
    /// Two live nodes can carry the same block id when two devices re-merge the same
    /// block concurrently (§8.6). The node with the smallest `TreeID` wins
    /// deterministically; the loser is hidden and its children are adopted by the winner.
    fn rebuild(&mut self) {
        let tree = self.tree();
        let mut order: Vec<(TreeID, Option<TreeID>)> = Vec::new();
        let mut stack: Vec<(TreeID, Option<TreeID>)> =
            tree.roots().into_iter().rev().map(|t| (t, None)).collect();
        while let Some((t, p)) = stack.pop() {
            order.push((t, p));
            if let Some(children) = tree.children(TreeParentId::Node(t)) {
                for c in children.into_iter().rev() {
                    stack.push((c, Some(t)));
                }
            }
        }
        let mut ulid: HashMap<TreeID, BlockId> = HashMap::with_capacity(order.len());
        let mut metas: HashMap<TreeID, LoroMap> = HashMap::with_capacity(order.len());
        let mut winners: HashMap<BlockId, TreeID> = HashMap::with_capacity(order.len());
        for (t, _) in &order {
            let Ok(meta) = tree.get_meta(*t) else {
                continue;
            };
            let id = BlockId(
                str_of(meta.get("id")).unwrap_or_else(|| format!("{}-{}", t.peer, t.counter)),
            );
            winners
                .entry(id.clone())
                .and_modify(|w| *w = (*w).min(*t))
                .or_insert(*t);
            ulid.insert(*t, id);
            metas.insert(*t, meta);
        }
        let mut blocks: HashMap<BlockId, BlockNode> = HashMap::with_capacity(winners.len());
        for (id, t) in &winners {
            let meta = &metas[t];
            let level = i64_of(meta.get("level")).map(|l| l.clamp(1, 3) as u8);
            let kind_s = str_of(meta.get("kind")).unwrap_or_default();
            let fi = tree.fractional_index(*t).unwrap_or_default();
            let created_at = i64_of(meta.get("created_at"))
                .or_else(|| id.timestamp_ms())
                .unwrap_or(0);
            blocks.insert(
                id.clone(),
                BlockNode {
                    id: id.clone(),
                    parent: None,
                    children: Vec::new(),
                    text: text_of(meta).map(|t| t.to_string()).unwrap_or_default(),
                    kind: BlockKind::from_parts(&kind_s, level),
                    checked: bool_of(meta.get("checked")).unwrap_or(false),
                    // '!' sorts below every fractional-index character, so appending the
                    // id keeps index order while making positions unique per sibling list.
                    position: format!("{fi}!{id}"),
                    created_at,
                    updated_at: i64_of(meta.get("updated_at")).unwrap_or(created_at),
                    deleted_at: i64_of(meta.get("deleted_at")),
                    moved_to: str_of(meta.get("moved_to")).map(PageId),
                    present: true,
                    live: true,
                },
            );
        }
        let mut roots = Vec::new();
        for (t, p) in &order {
            let Some(id) = ulid.get(t) else { continue };
            if winners.get(id) != Some(t) {
                continue;
            }
            let parent = p.and_then(|p| ulid.get(&p).cloned()).filter(|p| p != id);
            match &parent {
                Some(pid) if blocks.contains_key(pid) => {
                    blocks
                        .get_mut(pid)
                        .expect("checked")
                        .children
                        .push(id.clone());
                }
                _ => roots.push(id.clone()),
            }
        }
        // Parent pointers follow the children lists exactly.
        let mut parent_of: HashMap<BlockId, BlockId> = HashMap::new();
        for b in blocks.values() {
            for c in &b.children {
                parent_of.insert(c.clone(), b.id.clone());
            }
        }
        for b in blocks.values_mut() {
            b.parent = parent_of.get(&b.id).cloned();
        }
        let version = self.state.version + 1;
        self.state = PageState {
            page_id: self.state.page_id.clone(),
            meta: self.read_meta(),
            roots,
            blocks,
            version,
        };
        self.state.recompute_liveness();
        self.index = winners;
    }

    fn position_of(&self, id: &BlockId) -> Option<String> {
        let t = *self.index.get(id)?;
        Some(format!(
            "{}!{id}",
            self.tree().fractional_index(t).unwrap_or_default()
        ))
    }

    /// Read back fractional positions after an insert/move. Loro may rearrange sibling
    /// indexes when a new one does not fit; if a neighbour moved, refresh the whole list.
    fn refresh_positions(&mut self, cmd: &Command) {
        let id = match cmd {
            Command::InsertBlock { id, .. } | Command::MoveBlock { id, .. } => id,
            _ => return,
        };
        if let Some(pos) = self.position_of(id) {
            if let Some(b) = self.state.blocks.get_mut(id) {
                b.position = pos;
            }
        }
        let parent = self.state.parent_of(id);
        let neighbours: Vec<BlockId> = {
            let siblings = self.state.children_of(parent.as_ref());
            let Some(ix) = siblings.iter().position(|s| s == id) else {
                return;
            };
            [ix.checked_sub(1), Some(ix + 1)]
                .into_iter()
                .flatten()
                .filter_map(|i| siblings.get(i).cloned())
                .collect()
        };
        let stale = neighbours
            .iter()
            .any(|n| self.position_of(n) != self.state.block(n).map(|b| b.position.clone()));
        if stale {
            let siblings = self.state.children_of(parent.as_ref()).to_vec();
            for n in &siblings {
                if let Some(pos) = self.position_of(n) {
                    if let Some(b) = self.state.blocks.get_mut(n) {
                        b.position = pos;
                    }
                }
            }
        }
    }

    /// The state as a full rebuild from the CRDT would produce it (tests, diagnostics).
    pub fn rebuilt_state(&self) -> PageState {
        let mut copy = Self {
            doc: self.doc.fork(),
            state: PageState::new(self.state.page_id.clone(), PageMeta::default()),
            index: HashMap::new(),
        };
        copy.doc.get_tree(TREE).enable_fractional_index(0);
        copy.rebuild();
        copy.state
    }

    fn write_meta(&self, p: &PageMetaPatch) -> Result<()> {
        let m = self.page_map();
        let old_title = str_of(m.get("title")).unwrap_or_default();
        if let Some(t) = &p.title {
            m.insert("title", t.clone())?;
        }
        if let Some(k) = p.kind {
            m.insert("kind", k.as_str())?;
        }
        if let Some(d) = p.daily_date {
            m.insert(
                "daily_date",
                opt_str(d.map(utopia_core::date::iso).as_deref()),
            )?;
        }
        if let Some(c) = p.created_at {
            m.insert("created_at", c)?;
        }
        if let Some(d) = p.deleted_at {
            m.insert("deleted_at", opt_i64(d))?;
        }
        if let Some(mi) = &p.merged_into {
            m.insert("merged_into", opt_str(mi.as_ref().map(|p| p.as_str())))?;
        }
        let title = p.title.clone().unwrap_or(old_title.clone());
        let titles = self.titles_map()?;
        if let Some(aliases) = &p.aliases {
            let mut desired: Vec<(String, String)> = canonical_aliases(&title, aliases)
                .into_iter()
                .map(|a| (normalize_title(&a), a))
                .collect();
            let tk = normalize_title(&title);
            if !tk.is_empty() {
                desired.push((tk, title.clone()));
            }
            let mut existing = Vec::new();
            titles.for_each(|k, _| existing.push(k.to_string()));
            for k in existing {
                if !desired.iter().any(|(d, _)| *d == k) {
                    titles.delete(&k)?;
                }
            }
            for (k, v) in desired {
                titles.insert(&k, v)?;
            }
        } else if p.title.is_some() {
            let (ok, nk) = (normalize_title(&old_title), normalize_title(&title));
            if !ok.is_empty() && ok != nk {
                titles.delete(&ok)?;
            }
            if !nk.is_empty() {
                titles.insert(&nk, title.clone())?;
            }
        }
        Ok(())
    }

    fn set_node(&self, id: &BlockId, key: &str, v: impl Into<LoroValue>, now: i64) -> Result<()> {
        let meta = self.tree().get_meta(self.tid(id)?)?;
        meta.insert(key, v)?;
        meta.insert("updated_at", now)?;
        Ok(())
    }

    fn apply_inner(&mut self, cmd: &Command, now: i64) -> Result<DocDelta> {
        let tree = self.tree();
        let mut delta = DocDelta::default();
        match cmd {
            Command::InsertBlock {
                id,
                parent,
                after,
                text,
                kind,
            } => {
                if self.index.contains_key(id) {
                    return Err(DomainError::NoOp.into());
                }
                let parent_t = parent.as_ref().map(|p| self.tid(p)).transpose()?;
                let siblings = tree.children(parent_t).unwrap_or_default();
                let ix = match after {
                    None => 0,
                    Some(a) => self
                        .index
                        .get(a)
                        .and_then(|at| siblings.iter().position(|s| s == at))
                        .map_or(siblings.len(), |i| i + 1),
                };
                let t = tree.create_at(parent_t, ix)?;
                let meta = tree.get_meta(t)?;
                meta.insert("id", id.0.clone())?;
                let txt = meta.insert_container("text", LoroText::new())?;
                if !text.is_empty() {
                    txt.insert_utf8(0, text)?;
                }
                meta.insert("kind", kind.type_str())?;
                meta.insert("level", opt_i64(kind.heading_level().map(i64::from)))?;
                meta.insert("checked", false)?;
                meta.insert("created_at", id.timestamp_ms().unwrap_or(now))?;
                meta.insert("updated_at", now)?;
                self.index.insert(id.clone(), t);
                delta.inserted.push(id.clone());
                delta.structural = true;
            }
            Command::PurgeBlock { id } => {
                let t = self.tid(id)?;
                delta.removed = self.state.subtree(id, false);
                tree.delete(t)?;
                delta.structural = true;
            }
            Command::MoveBlock { id, parent, after } => {
                let t = self.tid(id)?;
                let parent_t = parent.as_ref().map(|p| self.tid(p)).transpose()?;
                if let Some(p) = parent {
                    if p == id || self.state.is_ancestor(id, p) {
                        return Err(DomainError::CycleDetected(id.clone()).into());
                    }
                }
                let target_parent: TreeParentId = parent_t.into();
                let anchor = after
                    .as_ref()
                    .filter(|a| *a != id)
                    .and_then(|a| self.index.get(a).copied())
                    .filter(|at| tree.parent(*at) == Some(target_parent));
                match (after, anchor) {
                    (_, Some(at)) => tree.mov_after(t, at)?,
                    (None, None) => {
                        let first = tree.children(parent_t).and_then(|c| c.first().copied());
                        if first != Some(t) || tree.parent(t) != Some(target_parent) {
                            tree.mov_to(t, parent_t, 0)?;
                        }
                    }
                    (Some(_), None) => {
                        let last = tree
                            .children(parent_t)
                            .and_then(|c| c.into_iter().rfind(|c| *c != t));
                        match last {
                            Some(l) => tree.mov_after(t, l)?,
                            None => tree.mov_to(t, parent_t, 0)?,
                        }
                    }
                }
                tree.get_meta(t)?.insert("updated_at", now)?;
                delta.changed = self.state.subtree(id, false);
                delta.structural = true;
            }
            Command::SetBlockText { id, edit } => {
                let t = self.tid(id)?;
                let meta = tree.get_meta(t)?;
                let text = match text_of(&meta) {
                    Some(t) => t,
                    None => meta.insert_container("text", LoroText::new())?,
                };
                let current = text.to_string();
                let new = utopia_core::text::apply_edit(&current, edit).ok_or(
                    DomainError::InvalidTextEdit {
                        start: edit.range.start,
                        end: edit.range.end,
                    },
                )?;
                let len = edit.range.end - edit.range.start;
                if len > 0 {
                    text.delete_utf8(edit.range.start, len)?;
                }
                if !edit.insert.is_empty() {
                    text.insert_utf8(edit.range.start, &edit.insert)?;
                }
                meta.insert("updated_at", now)?;
                if let Some(b) = self.state.blocks.get_mut(id) {
                    b.text = new;
                    b.updated_at = now;
                }
                delta.changed.push(id.clone());
            }
            Command::SetBlockKind { id, kind, checked } => {
                self.set_node(id, "kind", kind.type_str(), now)?;
                self.set_node(
                    id,
                    "level",
                    opt_i64(kind.heading_level().map(i64::from)),
                    now,
                )?;
                self.set_node(id, "checked", *checked, now)?;
                if let Some(b) = self.state.blocks.get_mut(id) {
                    b.kind = *kind;
                    b.checked = *checked;
                    b.updated_at = now;
                }
                delta.changed.push(id.clone());
            }
            Command::SetDeleted { ids, deleted_at } => {
                for id in ids {
                    self.set_node(id, "deleted_at", opt_i64(*deleted_at), now)?;
                    delta.changed.push(id.clone());
                }
                delta.structural = true;
            }
            Command::SetMoved { ids, to } => {
                for id in ids {
                    self.set_node(
                        id,
                        "moved_to",
                        opt_str(to.as_ref().map(|p| p.as_str())),
                        now,
                    )?;
                    delta.changed.push(id.clone());
                }
                delta.structural = true;
            }
            Command::SetPageMeta(p) => {
                self.write_meta(p)?;
                self.state.meta = self.read_meta();
                delta.meta_changed = true;
            }
        }
        Ok(delta)
    }
}

impl PageDoc for LoroPageDoc {
    fn new(page_id: PageId, peer: u64) -> Self {
        Self::wrap(LoroDoc::new(), page_id, peer).expect("fresh doc accepts a peer id")
    }

    fn from_snapshot(page_id: PageId, bytes: &[u8], peer: u64) -> Result<Self> {
        Self::load(page_id, bytes, &[], peer)
    }

    fn snapshot(&self) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::Snapshot)
            .map_err(|e| DocError::Crdt(e.to_string()))
    }

    fn frontier(&self) -> Frontier {
        Frontier::from_vv(&self.doc.oplog_vv())
    }

    fn export_since(&self, f: &Frontier) -> Result<Vec<u8>> {
        self.export_updates(&f.to_vv()?)
    }

    fn waiting(&self) -> Result<Option<Vec<u8>>> {
        self.doc
            .export_pending_updates()
            .map_err(|e| DocError::Crdt(e.to_string()))
    }

    fn import(&mut self, bytes: &[u8]) -> Result<bool> {
        let before = self.doc.oplog_vv();
        self.doc
            .import(bytes)
            .map_err(|e| DocError::Corrupt(e.to_string()))?;
        let changed = self.doc.oplog_vv() != before;
        if changed {
            self.rebuild();
        }
        Ok(changed)
    }

    fn apply(&mut self, cmd: &Command, now: i64) -> Result<DocDelta> {
        let result = self.apply_inner(cmd, now);
        self.doc.commit();
        match result {
            Ok(delta) => {
                if delta.structural {
                    // Mirror the command on the cached tree (reference semantics, proven
                    // equal to a CRDT rebuild by the fuzz tests) instead of rebuilding the
                    // whole page: batches stay linear (AC-4.7, AC-2.5).
                    for id in &delta.removed {
                        self.index.remove(id);
                    }
                    match self.state.apply(cmd, now) {
                        Ok(_) => self.refresh_positions(cmd),
                        Err(_) => self.rebuild(),
                    }
                } else {
                    self.state.version += 1;
                }
                Ok(delta)
            }
            Err(e) => {
                // A failed command may have partially written; resync the cache.
                self.rebuild();
                Err(e)
            }
        }
    }

    fn state(&self) -> &PageState {
        &self.state
    }
}

#[cfg(test)]
mod tests;
