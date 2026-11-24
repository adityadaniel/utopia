use std::ops::Range;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::ids::{BlockId, PageId};
use crate::tree::PageState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum BlockKind {
    #[default]
    Bullet,
    Numbered,
    Todo,
    Quote,
    Code,
    Heading(u8),
}

impl BlockKind {
    pub fn type_str(self) -> &'static str {
        match self {
            BlockKind::Bullet => "bullet",
            BlockKind::Numbered => "numbered",
            BlockKind::Todo => "todo",
            BlockKind::Quote => "quote",
            BlockKind::Code => "code",
            BlockKind::Heading(_) => "heading",
        }
    }

    pub fn heading_level(self) -> Option<u8> {
        match self {
            BlockKind::Heading(l) => Some(l.clamp(1, 3)),
            _ => None,
        }
    }

    pub fn from_parts(type_str: &str, level: Option<u8>) -> Self {
        match type_str {
            "numbered" => BlockKind::Numbered,
            "todo" => BlockKind::Todo,
            "quote" => BlockKind::Quote,
            "code" => BlockKind::Code,
            "heading" => BlockKind::Heading(level.unwrap_or(1).clamp(1, 3)),
            _ => BlockKind::Bullet,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum PageKind {
    #[default]
    Page,
    Daily,
}

impl PageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PageKind::Page => "page",
            PageKind::Daily => "daily",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PageMeta {
    pub title: String,
    pub kind: PageKind,
    pub daily_date: Option<NaiveDate>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
    pub merged_into: Option<PageId>,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PageMetaPatch {
    pub title: Option<String>,
    pub kind: Option<PageKind>,
    pub daily_date: Option<Option<NaiveDate>>,
    pub created_at: Option<i64>,
    pub deleted_at: Option<Option<i64>>,
    pub merged_into: Option<Option<PageId>>,
    pub aliases: Option<Vec<String>>,
}

impl PageMetaPatch {
    pub fn apply_to(&self, m: &mut PageMeta) {
        if let Some(v) = &self.title {
            m.title = v.clone();
        }
        if let Some(v) = self.kind {
            m.kind = v;
        }
        if let Some(v) = self.daily_date {
            m.daily_date = v;
        }
        if let Some(v) = self.created_at {
            m.created_at = v;
        }
        if let Some(v) = self.deleted_at {
            m.deleted_at = v;
        }
        if let Some(v) = &self.merged_into {
            m.merged_into = v.clone();
        }
        if let Some(v) = &self.aliases {
            m.aliases = v.clone();
        }
        if self.title.is_some() || self.aliases.is_some() {
            m.aliases = canonical_aliases(&m.title, &m.aliases);
        }
    }

    pub fn inverse_against(&self, m: &PageMeta) -> PageMetaPatch {
        PageMetaPatch {
            title: self.title.as_ref().map(|_| m.title.clone()),
            kind: self.kind.map(|_| m.kind),
            daily_date: self.daily_date.map(|_| m.daily_date),
            created_at: self.created_at.map(|_| m.created_at),
            deleted_at: self.deleted_at.map(|_| m.deleted_at),
            merged_into: self.merged_into.as_ref().map(|_| m.merged_into.clone()),
            aliases: self.aliases.as_ref().map(|_| m.aliases.clone()),
        }
    }
}

pub fn canonical_aliases(title: &str, aliases: &[String]) -> Vec<String> {
    let title_key = crate::title::normalize_title(title);
    let mut seen = std::collections::BTreeMap::new();
    for a in aliases {
        let k = crate::title::normalize_title(a);
        if k.is_empty() || k == title_key {
            continue;
        }
        seen.entry(k)
            .or_insert_with(|| crate::title::clean_title(a));
    }
    seen.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub insert: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    InsertBlock {
        id: BlockId,
        parent: Option<BlockId>,
        after: Option<BlockId>,
        text: String,
        kind: BlockKind,
    },
    PurgeBlock {
        id: BlockId,
    },
    MoveBlock {
        id: BlockId,
        parent: Option<BlockId>,
        after: Option<BlockId>,
    },
    SetBlockText {
        id: BlockId,
        edit: TextEdit,
    },
    SetBlockKind {
        id: BlockId,
        kind: BlockKind,
        checked: bool,
    },
    SetDeleted {
        ids: Vec<BlockId>,
        deleted_at: Option<i64>,
    },
    SetMoved {
        ids: Vec<BlockId>,
        to: Option<PageId>,
    },
    SetPageMeta(PageMetaPatch),
}

impl Command {
    pub fn invert(&self, before: &PageState) -> Result<Vec<Command>, DomainError> {
        let node = |id: &BlockId| {
            before
                .block(id)
                .ok_or_else(|| DomainError::BlockNotFound(id.clone()))
        };
        Ok(match self {
            Command::InsertBlock { id, .. } => vec![Command::PurgeBlock { id: id.clone() }],
            Command::PurgeBlock { id } => {
                node(id)?;
                let mut out = Vec::new();
                for bid in before.subtree(id, false) {
                    let n = node(&bid)?;
                    out.push(Command::InsertBlock {
                        id: bid.clone(),
                        parent: n.parent.clone(),
                        after: before.prev_sibling(&bid, false),
                        text: n.text.clone(),
                        kind: n.kind,
                    });
                    if n.checked {
                        out.push(Command::SetBlockKind {
                            id: bid.clone(),
                            kind: n.kind,
                            checked: true,
                        });
                    }
                    if n.deleted_at.is_some() {
                        out.push(Command::SetDeleted {
                            ids: vec![bid.clone()],
                            deleted_at: n.deleted_at,
                        });
                    }
                    if n.moved_to.is_some() {
                        out.push(Command::SetMoved {
                            ids: vec![bid.clone()],
                            to: n.moved_to.clone(),
                        });
                    }
                }
                out
            }
            Command::MoveBlock { id, .. } => {
                let n = node(id)?;
                vec![Command::MoveBlock {
                    id: id.clone(),
                    parent: n.parent.clone(),
                    after: before.prev_sibling(id, false),
                }]
            }
            Command::SetBlockText { id, edit } => {
                let n = node(id)?;
                let old = n
                    .text
                    .get(edit.range.clone())
                    .ok_or(DomainError::InvalidTextEdit {
                        start: edit.range.start,
                        end: edit.range.end,
                    })?;
                vec![Command::SetBlockText {
                    id: id.clone(),
                    edit: TextEdit {
                        range: edit.range.start..edit.range.start + edit.insert.len(),
                        insert: old.to_string(),
                    },
                }]
            }
            Command::SetBlockKind { id, .. } => {
                let n = node(id)?;
                vec![Command::SetBlockKind {
                    id: id.clone(),
                    kind: n.kind,
                    checked: n.checked,
                }]
            }
            Command::SetDeleted { ids, .. } => {
                let mut groups: Vec<(Option<i64>, Vec<BlockId>)> = Vec::new();
                for id in ids {
                    let old = node(id)?.deleted_at;
                    match groups.iter_mut().find(|(v, _)| *v == old) {
                        Some((_, g)) => g.push(id.clone()),
                        None => groups.push((old, vec![id.clone()])),
                    }
                }
                groups
                    .into_iter()
                    .map(|(deleted_at, ids)| Command::SetDeleted { ids, deleted_at })
                    .collect()
            }
            Command::SetMoved { ids, .. } => {
                let mut groups: Vec<(Option<PageId>, Vec<BlockId>)> = Vec::new();
                for id in ids {
                    let old = node(id)?.moved_to.clone();
                    match groups.iter_mut().find(|(v, _)| *v == old) {
                        Some((_, g)) => g.push(id.clone()),
                        None => groups.push((old, vec![id.clone()])),
                    }
                }
                groups
                    .into_iter()
                    .map(|(to, ids)| Command::SetMoved { ids, to })
                    .collect()
            }
            Command::SetPageMeta(p) => vec![Command::SetPageMeta(p.inverse_against(&before.meta))],
        })
    }

    pub fn block_id(&self) -> Option<&BlockId> {
        match self {
            Command::InsertBlock { id, .. }
            | Command::PurgeBlock { id }
            | Command::MoveBlock { id, .. }
            | Command::SetBlockText { id, .. }
            | Command::SetBlockKind { id, .. } => Some(id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocDelta {
    pub changed: Vec<BlockId>,
    pub inserted: Vec<BlockId>,
    pub removed: Vec<BlockId>,
    pub meta_changed: bool,
    pub structural: bool,
}

impl DocDelta {
    pub fn merge(&mut self, other: DocDelta) {
        self.changed.extend(other.changed);
        self.inserted.extend(other.inserted);
        self.removed.extend(other.removed);
        self.meta_changed |= other.meta_changed;
        self.structural |= other.structural;
    }

    pub fn touched(&self) -> Vec<BlockId> {
        let mut v: Vec<BlockId> = self
            .changed
            .iter()
            .chain(&self.inserted)
            .chain(&self.removed)
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
            && self.inserted.is_empty()
            && self.removed.is_empty()
            && !self.meta_changed
    }
}
