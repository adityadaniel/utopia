use crate::command::{BlockKind, Command};
use crate::error::DomainError;
use crate::ids::BlockId;
use crate::markdown::parse_outline;
use crate::text::{diff, floor_char_boundary};
use crate::tree::PageState;

pub const MAX_DROP_DEPTH: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focus {
    pub block: BlockId,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub cmds: Vec<Command>,
    pub focus: Option<Focus>,
}

impl Plan {
    fn focus(mut self, block: &BlockId, offset: usize) -> Self {
        self.focus = Some(Focus {
            block: block.clone(),
            offset,
        });
        self
    }
}

fn node<'a>(st: &'a PageState, id: &BlockId) -> Result<&'a crate::tree::BlockNode, DomainError> {
    st.block(id)
        .ok_or_else(|| DomainError::BlockNotFound(id.clone()))
}

pub fn split_block(
    st: &PageState,
    id: &BlockId,
    caret: usize,
    collapsed: bool,
) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let caret = floor_char_boundary(&n.text, caret);
    let new_id = BlockId::new();
    if caret == 0 && !n.text.is_empty() {
        let plan = Plan {
            cmds: vec![Command::InsertBlock {
                id: new_id,
                parent: n.parent.clone(),
                after: st.prev_sibling(id, false),
                text: String::new(),
                kind: n.kind,
            }],
            focus: None,
        };
        return Ok(plan.focus(id, 0));
    }
    let tail = n.text[caret..].to_string();
    let mut cmds = Vec::new();
    if !tail.is_empty() {
        cmds.push(Command::SetBlockText {
            id: id.clone(),
            edit: crate::command::TextEdit {
                range: caret..n.text.len(),
                insert: String::new(),
            },
        });
    }
    let (parent, after) = if st.has_live_children(id) && !collapsed {
        (Some(id.clone()), None)
    } else {
        (n.parent.clone(), Some(id.clone()))
    };
    cmds.push(Command::InsertBlock {
        id: new_id.clone(),
        parent,
        after,
        text: tail,
        kind: n.kind,
    });
    Ok(Plan { cmds, focus: None }.focus(&new_id, 0))
}

pub fn insert_above(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let new_id = BlockId::new();
    Ok(Plan {
        cmds: vec![Command::InsertBlock {
            id: new_id.clone(),
            parent: n.parent.clone(),
            after: st.prev_sibling(id, false),
            text: String::new(),
            kind: n.kind,
        }],
        focus: None,
    }
    .focus(&new_id, 0))
}

pub fn append_block(st: &PageState, parent: Option<&BlockId>) -> Plan {
    let new_id = BlockId::new();
    Plan {
        cmds: vec![Command::InsertBlock {
            id: new_id.clone(),
            parent: parent.cloned(),
            after: st.last_child(parent, false),
            text: String::new(),
            kind: BlockKind::Bullet,
        }],
        focus: None,
    }
    .focus(&new_id, 0)
}

pub fn indent(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    node(st, id)?;
    let prev = st
        .prev_sibling(id, true)
        .ok_or_else(|| DomainError::NoPreviousSibling(id.clone()))?;
    Ok(Plan {
        cmds: vec![Command::MoveBlock {
            id: id.clone(),
            parent: Some(prev.clone()),
            after: st.last_child(Some(&prev), false),
        }],
        focus: None,
    })
}

pub fn outdent(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let parent = n
        .parent
        .clone()
        .ok_or_else(|| DomainError::AlreadyRoot(id.clone()))?;
    Ok(Plan {
        cmds: vec![Command::MoveBlock {
            id: id.clone(),
            parent: st.parent_of(&parent),
            after: Some(parent),
        }],
        focus: None,
    })
}

pub fn merge_into_previous(
    st: &PageState,
    id: &BlockId,
    zoom: Option<&BlockId>,
    collapsed: &dyn Fn(&BlockId) -> bool,
    now: i64,
) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let rows = st.visible_rows(zoom, collapsed);
    let ix = rows
        .iter()
        .position(|(r, _)| r == id)
        .ok_or_else(|| DomainError::BlockNotFound(id.clone()))?;
    if ix == 0 {
        return match &n.parent {
            Some(p) if Some(p) != zoom => {
                let mut plan = outdent(st, id)?;
                plan.focus = Some(Focus {
                    block: id.clone(),
                    offset: 0,
                });
                Ok(plan)
            }
            _ => Err(DomainError::NoOp),
        };
    }
    let prev = rows[ix - 1].0.clone();
    let prev_node = node(st, &prev)?;
    let prev_len = prev_node.text.len();
    let mut cmds = Vec::new();
    if !n.text.is_empty() {
        cmds.push(Command::SetBlockText {
            id: prev.clone(),
            edit: crate::command::TextEdit {
                range: prev_len..prev_len,
                insert: n.text.clone(),
            },
        });
    }
    let mut after = if n.parent.as_ref() == Some(&prev) {
        st.prev_sibling(id, false)
    } else {
        st.last_child(Some(&prev), false)
    };
    for c in &n.children {
        if st.block(c).is_some_and(|b| b.present) {
            cmds.push(Command::MoveBlock {
                id: c.clone(),
                parent: Some(prev.clone()),
                after: after.clone(),
            });
            after = Some(c.clone());
        }
    }
    cmds.push(Command::SetDeleted {
        ids: vec![id.clone()],
        deleted_at: Some(now),
    });
    Ok(Plan { cmds, focus: None }.focus(&prev, prev_len))
}

pub fn move_up(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let cmd = match st.prev_sibling(id, true) {
        Some(p) => Command::MoveBlock {
            id: id.clone(),
            parent: n.parent.clone(),
            after: st.prev_sibling(&p, false),
        },
        None => {
            let parent = n.parent.clone().ok_or(DomainError::NoOp)?;
            Command::MoveBlock {
                id: id.clone(),
                parent: st.parent_of(&parent),
                after: st.prev_sibling(&parent, false),
            }
        }
    };
    Ok(Plan {
        cmds: vec![cmd],
        focus: None,
    })
}

pub fn move_down(st: &PageState, id: &BlockId) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let cmd = match st.next_sibling(id, true) {
        Some(next) => Command::MoveBlock {
            id: id.clone(),
            parent: n.parent.clone(),
            after: Some(next),
        },
        None => {
            let parent = n.parent.clone().ok_or(DomainError::NoOp)?;
            Command::MoveBlock {
                id: id.clone(),
                parent: st.parent_of(&parent),
                after: Some(parent),
            }
        }
    };
    Ok(Plan {
        cmds: vec![cmd],
        focus: None,
    })
}

pub fn delete_subtree(st: &PageState, id: &BlockId, now: i64) -> Result<Plan, DomainError> {
    node(st, id)?;
    let ids = st.subtree(id, true);
    if ids.is_empty() {
        return Err(DomainError::NoOp);
    }
    let rows = st.live_preorder();
    let ix = rows.iter().position(|(r, _)| r == id);
    let focus = ix.and_then(|i| {
        rows[..i]
            .iter()
            .rev()
            .chain(rows[i + 1..].iter())
            .find(|(r, _)| !ids.contains(r))
            .map(|(r, _)| r.clone())
    });
    let plan = Plan {
        cmds: vec![Command::SetDeleted {
            ids,
            deleted_at: Some(now),
        }],
        focus: None,
    };
    Ok(match focus {
        Some(f) => {
            let len = st.block(&f).map_or(0, |b| b.text.len());
            plan.focus(&f, len)
        }
        None => plan,
    })
}

pub fn set_text(st: &PageState, id: &BlockId, new_text: &str) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let edit = diff(&n.text, new_text).ok_or(DomainError::NoOp)?;
    Ok(Plan {
        cmds: vec![Command::SetBlockText {
            id: id.clone(),
            edit,
        }],
        focus: None,
    })
}

pub fn move_to(
    st: &PageState,
    id: &BlockId,
    parent: Option<&BlockId>,
    after: Option<&BlockId>,
) -> Result<Plan, DomainError> {
    node(st, id)?;
    if let Some(p) = parent {
        node(st, p)?;
        if p == id || st.is_ancestor(id, p) {
            return Err(DomainError::CycleDetected(id.clone()));
        }
    }
    let base = parent.map_or(0, |p| st.depth(p) + 1);
    let own = st.depth(id);
    let max_rel = st
        .subtree(id, true)
        .iter()
        .map(|b| st.depth(b) - own)
        .max()
        .unwrap_or(0);
    if base + max_rel >= MAX_DROP_DEPTH {
        return Err(DomainError::TooDeep);
    }
    if after == Some(id) {
        return Err(DomainError::NoOp);
    }
    Ok(Plan {
        cmds: vec![Command::MoveBlock {
            id: id.clone(),
            parent: parent.cloned(),
            after: after.cloned(),
        }],
        focus: None,
    })
}

pub fn paste(
    st: &PageState,
    id: &BlockId,
    selection: std::ops::Range<usize>,
    pasted: &str,
) -> Result<Plan, DomainError> {
    let n = node(st, id)?;
    let start = floor_char_boundary(&n.text, selection.start);
    let end = floor_char_boundary(&n.text, selection.end.max(start));
    let pasted = pasted.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = pasted.trim_end_matches('\n');
    if !trimmed.contains('\n') {
        let line = trimmed.to_string();
        if line.is_empty() && start == end {
            return Err(DomainError::NoOp);
        }
        let len = line.len();
        return Ok(Plan {
            cmds: vec![Command::SetBlockText {
                id: id.clone(),
                edit: crate::command::TextEdit {
                    range: start..end,
                    insert: line,
                },
            }],
            focus: None,
        }
        .focus(id, start + len));
    }
    let items: Vec<(usize, String, BlockKind)> = parse_outline(trimmed)
        .into_iter()
        .map(|(depth, text)| (depth, text, BlockKind::Bullet))
        .collect();
    if items.is_empty() {
        return Err(DomainError::NoOp);
    }
    let mut cmds = Vec::new();
    let mut rest = &items[..];
    let reuse_current = n.text.is_empty() && n.children.is_empty();
    if reuse_current {
        cmds.push(Command::SetBlockText {
            id: id.clone(),
            edit: crate::command::TextEdit {
                range: 0..0,
                insert: items[0].1.clone(),
            },
        });
        rest = &items[1..];
    }
    let (tail, inserted) = insert_after(st, id, n.parent.clone(), rest);
    cmds.extend(tail);
    let focus = inserted.unwrap_or(Focus {
        block: id.clone(),
        offset: if reuse_current {
            items[0].1.len()
        } else {
            n.text.len()
        },
    });
    Ok(Plan { cmds, focus: None }.focus(&focus.block, focus.offset))
}

pub fn append_outline(
    st: &PageState,
    parent: Option<&BlockId>,
    anchor: Option<&BlockId>,
    items: &[(usize, String)],
) -> Result<Plan, DomainError> {
    let items: Vec<(usize, String, BlockKind)> = items
        .iter()
        .map(|(depth, text)| (*depth, text.clone(), BlockKind::Bullet))
        .collect();
    append_outline_kinds(st, parent, anchor, &items)
}

pub fn append_outline_kinds(
    st: &PageState,
    parent: Option<&BlockId>,
    anchor: Option<&BlockId>,
    items: &[(usize, String, BlockKind)],
) -> Result<Plan, DomainError> {
    let Some((first, rest)) = items.split_first() else {
        return Err(DomainError::NoOp);
    };
    let mut cmds = Vec::new();
    let (anchor, anchor_parent) = match anchor {
        Some(id) => (id.clone(), st.parent_of(id)),
        None => {
            if let Some(p) = parent {
                node(st, p)?;
            }
            let id = BlockId::new();
            cmds.push(Command::InsertBlock {
                id: id.clone(),
                parent: parent.cloned(),
                after: st.last_child(parent, false),
                text: String::new(),
                kind: BlockKind::Bullet,
            });
            (id, parent.cloned())
        }
    };
    let base = anchor_parent.as_ref().map_or(0, |p| st.depth(p) + 1);
    if items
        .iter()
        .any(|(depth, _, _)| base + depth >= MAX_DROP_DEPTH)
    {
        return Err(DomainError::TooDeep);
    }
    if first.2 != BlockKind::Bullet {
        cmds.push(Command::SetBlockKind {
            id: anchor.clone(),
            kind: first.2,
            checked: false,
        });
    }
    cmds.push(Command::SetBlockText {
        id: anchor.clone(),
        edit: crate::command::TextEdit {
            range: 0..0,
            insert: first.1.clone(),
        },
    });
    let (tail, inserted) = insert_after(st, &anchor, anchor_parent, rest);
    cmds.extend(tail);
    Ok(Plan {
        cmds,
        focus: Some(inserted.unwrap_or(Focus {
            block: anchor,
            offset: first.1.len(),
        })),
    })
}

pub fn too_deep(items: &[(usize, String)], base: usize) -> bool {
    items
        .iter()
        .any(|(depth, _)| base + depth >= MAX_DROP_DEPTH)
}

fn insert_after(
    st: &PageState,
    anchor: &BlockId,
    anchor_parent: Option<BlockId>,
    items: &[(usize, String, BlockKind)],
) -> (Vec<Command>, Option<Focus>) {
    let mut cmds = Vec::new();
    let mut last: Vec<BlockId> = vec![anchor.clone()];
    let mut parents: Vec<Option<BlockId>> = vec![anchor_parent];
    let mut first_child_after: Option<Option<BlockId>> = Some(st.last_child(Some(anchor), false));
    let mut focus = None;
    for (depth, text, kind) in items {
        let d = (*depth).min(last.len());
        let new_id = BlockId::new();
        let (parent, after) = if d < last.len() {
            (parents[d].clone(), Some(last[d].clone()))
        } else {
            let after = if last[d - 1] == *anchor {
                first_child_after.take().flatten()
            } else {
                None
            };
            (Some(last[d - 1].clone()), after)
        };
        cmds.push(Command::InsertBlock {
            id: new_id.clone(),
            parent: parent.clone(),
            after,
            text: text.clone(),
            kind: *kind,
        });
        last.truncate(d);
        parents.truncate(d);
        last.push(new_id.clone());
        parents.push(parent);
        focus = Some(Focus {
            block: new_id,
            offset: text.len(),
        });
    }
    (cmds, focus)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub depth: usize,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
}

pub fn block_roots(
    st: &PageState,
    id: &BlockId,
    count: usize,
) -> Result<Vec<BlockId>, DomainError> {
    node(st, id)?;
    let mut roots = vec![id.clone()];
    let mut cur = id.clone();
    for _ in 1..count.max(1) {
        let Some(next) = st.next_sibling(&cur, true) else {
            break;
        };
        roots.push(next.clone());
        cur = next;
    }
    Ok(roots)
}

pub fn block_ids(st: &PageState, id: &BlockId, count: usize) -> Result<Vec<BlockId>, DomainError> {
    let mut ids = Vec::new();
    for r in block_roots(st, id, count)? {
        ids.extend(st.subtree(&r, true));
    }
    Ok(ids)
}

pub fn block_tree(st: &PageState, id: &BlockId, count: usize) -> Result<Vec<TreeRow>, DomainError> {
    let mut rows = Vec::new();
    for r in block_roots(st, id, count)? {
        let base = st.depth(&r);
        for b in st.subtree(&r, true) {
            let Some(n) = st.block(&b) else { continue };
            rows.push(TreeRow {
                depth: st.depth(&b) - base,
                text: n.text.clone(),
                kind: n.kind,
                checked: n.checked,
            });
        }
    }
    Ok(rows)
}

pub fn delete_blocks(
    st: &PageState,
    id: &BlockId,
    count: usize,
    now: i64,
    zoom: Option<&BlockId>,
    collapsed: &dyn Fn(&BlockId) -> bool,
) -> Result<Plan, DomainError> {
    let ids = block_ids(st, id, count)?;
    if ids.is_empty() {
        return Err(DomainError::NoOp);
    }
    let rows = st.visible_rows(zoom, collapsed);
    let find = |rows: &[(BlockId, usize)]| {
        let ix = rows.iter().position(|(r, _)| r == id)?;
        rows[ix..]
            .iter()
            .find(|(r, _)| !ids.contains(r))
            .map(|(r, _)| (r.clone(), 0))
            .or_else(|| {
                rows[..ix]
                    .iter()
                    .rev()
                    .find(|(r, _)| !ids.contains(r))
                    .map(|(r, _)| (r.clone(), st.block(r).map_or(0, |b| b.text.len())))
            })
    };
    let focus = find(&rows).or_else(|| find(&st.live_preorder()));
    let plan = Plan {
        cmds: vec![Command::SetDeleted {
            ids,
            deleted_at: Some(now),
        }],
        focus: None,
    };
    Ok(match focus {
        Some((block, offset)) => plan.focus(&block, offset),
        None => plan,
    })
}

pub fn join_next(
    st: &PageState,
    id: &BlockId,
    count: usize,
    zoom: Option<&BlockId>,
    collapsed: &dyn Fn(&BlockId) -> bool,
    now: i64,
) -> Result<Plan, DomainError> {
    let start = node(st, id)?;
    let mut sim = st.clone();
    let mut cmds: Vec<Command> = Vec::new();
    let mut caret = None;
    for _ in 0..count.max(1) {
        let rows = sim.visible_rows(zoom, collapsed);
        let Some(ix) = rows.iter().position(|(r, _)| r == id) else {
            break;
        };
        let Some(next) = rows.get(ix + 1).map(|(r, _)| r.clone()) else {
            break;
        };
        let Some(n) = sim.block(&next).cloned() else {
            break;
        };
        let text = sim.block(id).map(|b| b.text.clone()).unwrap_or_default();
        let mut join = Vec::new();
        if !n.text.is_empty() {
            let space = !text.is_empty()
                && !text.ends_with(char::is_whitespace)
                && !n.text.starts_with(char::is_whitespace);
            caret.get_or_insert(text.len() + usize::from(space));
            join.push(Command::SetBlockText {
                id: id.clone(),
                edit: crate::command::TextEdit {
                    range: text.len()..text.len(),
                    insert: format!("{}{}", if space { " " } else { "" }, n.text),
                },
            });
        }
        let mut after = if n.parent.as_ref() == Some(id) {
            sim.prev_sibling(&next, false)
        } else {
            sim.last_child(Some(id), false)
        };
        for c in &n.children {
            if sim.block(c).is_none_or(|b| !b.present) {
                continue;
            }
            let deepest = sim
                .subtree(c, false)
                .into_iter()
                .filter(|d| sim.block(d).is_some_and(|b| b.present))
                .map(|d| sim.depth(&d).saturating_sub(sim.depth(c)))
                .max()
                .unwrap_or(0);
            if sim.depth(id) + 1 + deepest >= MAX_DROP_DEPTH {
                return Err(DomainError::TooDeep);
            }
        }
        for c in &n.children {
            if sim.block(c).is_some_and(|b| b.present) {
                join.push(Command::MoveBlock {
                    id: c.clone(),
                    parent: Some(id.clone()),
                    after: after.clone(),
                });
                after = Some(c.clone());
            }
        }
        join.push(Command::SetDeleted {
            ids: vec![next.clone()],
            deleted_at: Some(now),
        });
        for c in &join {
            sim.apply(c, now)?;
        }
        cmds.extend(join);
    }
    if cmds.is_empty() {
        return Err(DomainError::NoOp);
    }
    let caret = caret.unwrap_or(start.text.len());
    Ok(Plan { cmds, focus: None }.focus(id, caret))
}

pub fn insert_tree_after(
    st: &PageState,
    id: &BlockId,
    tree: &[TreeRow],
    before: bool,
    count: usize,
) -> Result<Plan, DomainError> {
    node(st, id)?;
    if tree.is_empty() {
        return Err(DomainError::NoOp);
    }
    let deepest = tree.iter().map(|r| r.depth).max().unwrap_or(0);
    if st.depth(id) + deepest >= MAX_DROP_DEPTH {
        return Err(DomainError::TooDeep);
    }
    let parent = st.parent_of(id);
    let anchor = if before {
        st.prev_sibling(id, false)
    } else {
        Some(id.clone())
    };
    let mut last: Vec<Option<BlockId>> = vec![anchor];
    let mut parents: Vec<Option<BlockId>> = vec![parent];
    let mut cmds: Vec<Command> = Vec::new();
    let mut focus: Option<Focus> = None;
    for _ in 0..count.max(1) {
        for row in tree {
            let d = row.depth.min(last.len());
            let (parent, after) = if d < last.len() {
                (parents[d].clone(), last[d].clone())
            } else {
                (last[d - 1].clone(), None)
            };
            let new_id = BlockId::new();
            cmds.push(Command::InsertBlock {
                id: new_id.clone(),
                parent: parent.clone(),
                after,
                text: row.text.clone(),
                kind: row.kind,
            });
            if row.checked {
                cmds.push(Command::SetBlockKind {
                    id: new_id.clone(),
                    kind: row.kind,
                    checked: true,
                });
            }
            last.truncate(d);
            parents.truncate(d);
            last.push(Some(new_id.clone()));
            parents.push(parent);
            focus.get_or_insert(Focus {
                block: new_id,
                offset: 0,
            });
        }
    }
    Ok(Plan { cmds, focus })
}

pub fn restore_after(
    st: &PageState,
    ids: &[BlockId],
    target: &BlockId,
    before: bool,
) -> Result<Plan, DomainError> {
    node(st, target)?;
    if ids.is_empty() {
        return Err(DomainError::NoOp);
    }
    for id in ids {
        match st.block(id) {
            None => return Err(DomainError::BlockNotFound(id.clone())),
            Some(b) if b.deleted_at.is_none() || b.moved_to.is_some() => {
                return Err(DomainError::NoOp)
            }
            Some(_) => {}
        }
    }
    if ids.contains(target) || ids.iter().any(|id| st.is_ancestor(id, target)) {
        return Err(DomainError::NoOp);
    }
    let roots: Vec<BlockId> = ids
        .iter()
        .filter(|id| !st.parent_of(id).is_some_and(|p| ids.contains(&p)))
        .cloned()
        .collect();
    let Some(first) = roots.first().cloned() else {
        return Err(DomainError::NoOp);
    };
    let root_depth = st.depth(&first);
    let deepest = ids
        .iter()
        .map(|id| st.depth(id))
        .max()
        .unwrap_or(root_depth);
    if st.depth(target) + deepest.saturating_sub(root_depth) >= MAX_DROP_DEPTH {
        return Err(DomainError::TooDeep);
    }
    let parent = st.parent_of(target);
    let mut after = if before {
        let mut prev = st.prev_sibling(target, false);
        while let Some(p) = prev.clone() {
            if !ids.contains(&p) {
                break;
            }
            prev = st.prev_sibling(&p, false);
        }
        prev
    } else {
        Some(target.clone())
    };
    let mut cmds = vec![Command::SetDeleted {
        ids: ids.to_vec(),
        deleted_at: None,
    }];
    for root in &roots {
        cmds.push(Command::MoveBlock {
            id: root.clone(),
            parent: parent.clone(),
            after,
        });
        after = Some(root.clone());
    }
    Ok(Plan {
        cmds,
        focus: Some(Focus {
            block: first,
            offset: 0,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::PageMeta;

    fn apply(st: &mut PageState, plan: &Plan) {
        for c in &plan.cmds {
            st.apply(c, 10).unwrap();
        }
    }

    fn rows(st: &PageState) -> Vec<(String, usize)> {
        st.live_preorder()
            .into_iter()
            .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
            .collect()
    }

    fn page_with(lines: &[(&str, usize)]) -> (PageState, Vec<BlockId>) {
        let mut st = PageState::new("p".into(), PageMeta::default());
        let items: Vec<(usize, String)> = lines.iter().map(|(t, d)| (*d, t.to_string())).collect();
        let root = BlockId::new();
        st.apply(
            &Command::InsertBlock {
                id: root.clone(),
                parent: None,
                after: None,
                text: String::new(),
                kind: BlockKind::Bullet,
            },
            0,
        )
        .unwrap();
        let text: String = items
            .iter()
            .map(|(d, t)| format!("{}{}\n", "  ".repeat(*d), t))
            .collect();
        let plan = paste(&st, &root, 0..0, &text).unwrap();
        apply(&mut st, &plan);
        let ids = st.live_preorder().into_iter().map(|(id, _)| id).collect();
        (st, ids)
    }

    #[test]
    fn enter_splits_and_places() {
        let (mut st, ids) = page_with(&[("hello world", 0), ("child", 1)]);
        let plan = split_block(&st, &ids[0], 5, false).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("hello".into(), 0),
                (" world".into(), 1),
                ("child".into(), 1)
            ]
        );
        let plan = split_block(&st, &ids[0], 0, false).unwrap();
        assert_eq!(plan.focus.as_ref().unwrap().block, ids[0]);
        apply(&mut st, &plan);
        assert_eq!(rows(&st)[0], ("".into(), 0));
    }

    #[test]
    fn enter_on_collapsed_parent_creates_sibling() {
        let (mut st, ids) = page_with(&[("parent", 0), ("child", 1)]);
        let plan = split_block(&st, &ids[0], 6, true).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![("parent".into(), 0), ("child".into(), 1), ("".into(), 0)]
        );
    }

    #[test]
    fn tab_and_shift_tab() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1)]);
        assert!(matches!(
            indent(&st, &ids[0]),
            Err(DomainError::NoPreviousSibling(_))
        ));
        {
            let p = indent(&st, &ids[1]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("a".into(), 0), ("b".into(), 1), ("b1".into(), 2)]
        );
        {
            let p = outdent(&st, &ids[1]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("a".into(), 0), ("b".into(), 0), ("b1".into(), 1)]
        );
        assert!(matches!(
            outdent(&st, &ids[0]),
            Err(DomainError::AlreadyRoot(_))
        ));
    }

    #[test]
    fn backspace_merges_text_and_children() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1), ("c", 0)]);
        let plan = merge_into_previous(&st, &ids[1], None, &|_| false, 99).unwrap();
        assert_eq!(
            plan.focus,
            Some(Focus {
                block: ids[0].clone(),
                offset: 1
            })
        );
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![("ab".into(), 0), ("b1".into(), 1), ("c".into(), 0)]
        );
        assert_eq!(st.block(&ids[1]).unwrap().deleted_at, Some(99));
        let (mut st, ids) = page_with(&[("p", 0), ("x", 1), ("x1", 2), ("y", 1)]);
        {
            let p = merge_into_previous(&st, &ids[1], None, &|_| false, 1).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("px".into(), 0), ("x1".into(), 1), ("y".into(), 1)]
        );
        assert_eq!(
            merge_into_previous(&st, &ids[0], None, &|_| false, 1),
            Err(DomainError::NoOp)
        );
    }

    #[test]
    fn move_up_down_edges() {
        let (mut st, ids) = page_with(&[("p", 0), ("x", 1), ("y", 1)]);
        {
            let p = move_up(&st, &ids[2]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("p".into(), 0), ("y".into(), 1), ("x".into(), 1)]
        );
        {
            let p = move_up(&st, &ids[2]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("y".into(), 0), ("p".into(), 0), ("x".into(), 1)]
        );
        {
            let p = move_down(&st, &ids[1]).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(
            rows(&st),
            vec![("y".into(), 0), ("p".into(), 0), ("x".into(), 0)]
        );
        assert_eq!(move_down(&st, &ids[1]), Err(DomainError::NoOp));
    }

    #[test]
    fn drop_validation() {
        let (st, ids) = page_with(&[("a", 0), ("b", 1)]);
        assert!(matches!(
            move_to(&st, &ids[0], Some(&ids[1]), None),
            Err(DomainError::CycleDetected(_))
        ));
        let mut lines: Vec<(String, usize)> = (0..12).map(|d| (format!("d{d}"), d)).collect();
        lines.push(("leaf".into(), 0));
        let lines_ref: Vec<(&str, usize)> = lines.iter().map(|(t, d)| (t.as_str(), *d)).collect();
        let (st, ids) = page_with(&lines_ref);
        let leaf = &ids[12];
        assert_eq!(
            move_to(&st, leaf, Some(&ids[11]), None),
            Err(DomainError::TooDeep)
        );
        assert!(move_to(&st, leaf, Some(&ids[10]), None).is_ok());
        assert_eq!(move_to(&st, leaf, None, Some(leaf)), Err(DomainError::NoOp));
    }

    #[test]
    fn paste_preserves_nesting() {
        let (st, _) = page_with(&[("one", 0), ("two", 1), ("three", 2), ("four", 0)]);
        assert_eq!(
            rows(&st),
            vec![
                ("one".into(), 0),
                ("two".into(), 1),
                ("three".into(), 2),
                ("four".into(), 0)
            ]
        );
        let (mut st, ids) = page_with(&[("keep", 0)]);
        let plan = paste(&st, &ids[0], 4..4, "x\n  y\nz").unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("keep".into(), 0),
                ("x".into(), 0),
                ("y".into(), 1),
                ("z".into(), 0)
            ]
        );
        let plan = paste(&st, &ids[0], 2..2, "--").unwrap();
        apply(&mut st, &plan);
        assert_eq!(st.block(&ids[0]).unwrap().text, "ke--ep");
    }

    fn chain(n: usize) -> Vec<(usize, String)> {
        (0..n).map(|d| (d, format!("d{d}"))).collect()
    }

    fn empty_page() -> (PageState, BlockId) {
        let mut st = PageState::new("p".into(), PageMeta::default());
        let root = BlockId::new();
        st.apply(
            &Command::InsertBlock {
                id: root.clone(),
                parent: None,
                after: None,
                text: String::new(),
                kind: BlockKind::Bullet,
            },
            0,
        )
        .unwrap();
        (st, root)
    }

    #[test]
    fn mc_8_append_outline_goes_last_and_keeps_nesting() {
        let (mut st, ids) = page_with(&[("keep", 0), ("kid", 1)]);
        let plan =
            append_outline(&st, None, None, &parse_outline("- one\n  - two\n- three\n")).unwrap();
        let last = plan.focus.as_ref().expect("a caret").block.clone();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("keep".into(), 0),
                ("kid".into(), 1),
                ("one".into(), 0),
                ("two".into(), 1),
                ("three".into(), 0)
            ]
        );
        assert_eq!(st.block(&last).unwrap().text, "three");

        let plan = append_outline(
            &st,
            Some(&ids[0]),
            None,
            &parse_outline("- under\n  - deeper\n"),
        )
        .unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("keep".into(), 0),
                ("kid".into(), 1),
                ("under".into(), 1),
                ("deeper".into(), 2),
                ("one".into(), 0),
                ("two".into(), 1),
                ("three".into(), 0)
            ]
        );
    }

    #[test]
    fn mc_8_an_anchor_block_takes_the_first_line() {
        let (mut st, root) = empty_page();
        let plan =
            append_outline(&st, None, Some(&root), &parse_outline("- one\n  - two\n")).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![("one".into(), 0), ("two".into(), 1)],
            "no empty block is left above the outline"
        );
        assert_eq!(st.block(&root).unwrap().text, "one");
        assert_eq!(
            append_outline(&st, None, Some(&root), &[]),
            Err(DomainError::NoOp)
        );
    }

    #[test]
    fn mc_8_an_outline_past_twelve_levels_is_refused() {
        let (st, ids) = page_with(&[("root", 0)]);
        assert!(append_outline(&st, None, None, &chain(MAX_DROP_DEPTH)).is_ok());
        assert_eq!(
            append_outline(&st, None, None, &chain(MAX_DROP_DEPTH + 1)),
            Err(DomainError::TooDeep)
        );
        assert_eq!(
            append_outline(&st, Some(&ids[0]), None, &chain(MAX_DROP_DEPTH)),
            Err(DomainError::TooDeep),
            "one level down, twelve more would be thirteen"
        );
    }

    #[test]
    fn delete_tombstones_live_subtree() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1)]);
        let plan = delete_subtree(&st, &ids[1], 7).unwrap();
        assert_eq!(plan.focus.as_ref().unwrap().block, ids[0]);
        apply(&mut st, &plan);
        assert_eq!(rows(&st), vec![("a".into(), 0)]);
        assert!(st.block(&ids[2]).unwrap().deleted_at.is_some());
    }

    fn siblings(roots: usize, kids: usize) -> (PageState, Vec<BlockId>) {
        let mut lines: Vec<(String, usize)> = Vec::new();
        for r in 0..roots {
            lines.push((format!("r{r}"), 0));
            for k in 0..kids {
                lines.push((format!("r{r}k{k}"), 1));
            }
        }
        let refs: Vec<(&str, usize)> = lines.iter().map(|(t, d)| (t.as_str(), *d)).collect();
        page_with(&refs)
    }

    #[test]
    fn vm_8_delete_blocks_takes_siblings_with_their_subtrees() {
        let (mut st, ids) = siblings(3, 2);
        let plan = delete_blocks(&st, &ids[0], 3, 9, None, &|_| false).unwrap();
        assert_eq!(plan.cmds.len(), 1, "one SetDeleted, so one undo step");
        assert_eq!(plan.focus, None, "the whole page went with it");
        apply(&mut st, &plan);
        assert_eq!(st.live_count(), 0);
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(
                st.block(id).unwrap().deleted_at,
                Some(9),
                "block {i} is a tombstone, not gone"
            );
        }
        assert_eq!(st.blocks.len(), 9, "nine rows, none purged");
    }

    #[test]
    fn vm_8_delete_blocks_focuses_the_next_then_the_previous() {
        let (mut st, ids) = siblings(3, 2);
        let plan = delete_blocks(&st, &ids[3], 1, 1, None, &|_| false).unwrap();
        assert_eq!(
            plan.focus,
            Some(Focus {
                block: ids[6].clone(),
                offset: 0
            })
        );
        apply(&mut st, &plan);
        let plan = delete_blocks(&st, &ids[6], 1, 1, None, &|_| false).unwrap();
        assert_eq!(
            plan.focus,
            Some(Focus {
                block: ids[2].clone(),
                offset: "r0k1".len()
            })
        );
    }

    #[test]
    fn vm_10_the_roots_ids_and_rows_agree() {
        let (mut st, ids) = siblings(3, 2);
        assert_eq!(
            block_roots(&st, &ids[6], 9).unwrap(),
            vec![ids[6].clone()],
            "the last root has no sibling below"
        );
        assert_eq!(
            block_roots(&st, &ids[0], 2).unwrap(),
            vec![ids[0].clone(), ids[3].clone()]
        );
        let cut = block_ids(&st, &ids[0], 2).unwrap();
        assert_eq!(cut.len(), 6, "two roots, each with two children");
        assert_eq!(cut[0], ids[0]);
        let tree = block_tree(&st, &ids[0], 2).unwrap();
        assert_eq!(tree.len(), 6);
        assert_eq!(tree[0].depth, 0);
        assert_eq!(tree[1].depth, 1);
        assert_eq!(tree[3].depth, 0, "the second root is back at the top");
        st.apply(
            &Command::SetBlockKind {
                id: ids[1].clone(),
                kind: BlockKind::Todo,
                checked: true,
            },
            1,
        )
        .unwrap();
        let tree = block_tree(&st, &ids[0], 1).unwrap();
        assert_eq!(tree[1].kind, BlockKind::Todo);
        assert!(tree[1].checked);
    }

    #[test]
    fn vm_8_join_next_keeps_the_text_and_the_children() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1), ("c", 0)]);
        let plan = join_next(&st, &ids[0], 1, None, &|_| false, 4).unwrap();
        assert_eq!(
            plan.focus,
            Some(Focus {
                block: ids[0].clone(),
                offset: 2
            })
        );
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![("a b".into(), 0), ("b1".into(), 1), ("c".into(), 0)],
            "the joined block's child moved up, the joined block is a tombstone"
        );
        assert_eq!(st.block(&ids[1]).unwrap().deleted_at, Some(4));
        assert_eq!(st.block(&ids[1]).unwrap().text, "b", "its text is not lost");

        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("c", 0), ("d", 0)]);
        let plan = join_next(&st, &ids[0], 3, None, &|_| false, 4).unwrap();
        apply(&mut st, &plan);
        assert_eq!(rows(&st), vec![("a b c d".into(), 0)]);
        assert_eq!(st.live_count(), 1);
        assert!(plan
            .cmds
            .iter()
            .all(|c| !matches!(c, Command::PurgeBlock { .. })));
    }

    #[test]
    fn vm_8_join_next_refuses_a_join_that_would_pass_the_ceiling() {
        let mut lines: Vec<(String, usize)> = vec![("a".into(), 0)];
        lines.extend((0..(MAX_DROP_DEPTH - 1)).map(|d| (format!("c{d}"), d + 1)));
        lines.push(("b".into(), 0));
        lines.push(("b1".into(), 1));
        let refs: Vec<(&str, usize)> = lines.iter().map(|(t, d)| (t.as_str(), *d)).collect();
        let (mut st, ids) = page_with(&refs);
        let deepest = ids[MAX_DROP_DEPTH - 1].clone();
        let before = rows(&st);
        let r = join_next(&st, &deepest, 1, None, &|_| false, 7);
        assert!(
            matches!(r, Err(DomainError::TooDeep)),
            "`b1` would land at depth {MAX_DROP_DEPTH}: {r:?}"
        );
        assert_eq!(rows(&st), before, "and nothing was planned");

        let shallow = ids[MAX_DROP_DEPTH - 2].clone();
        let plan = join_next(&st, &shallow, 1, None, &|_| false, 7).unwrap();
        apply(&mut st, &plan);
        assert!(
            st.live_preorder().iter().all(|(_, d)| *d < MAX_DROP_DEPTH),
            "the outline stays inside the ceiling: {:?}",
            st.live_preorder()
        );
    }

    #[test]
    fn vm_10_restore_after_refuses_a_target_inside_the_cut() {
        let (mut st, ids) = page_with(&[("a", 0), ("a1", 1), ("b", 0)]);
        let cut = block_ids(&st, &ids[0], 1).unwrap();
        let plan = delete_blocks(&st, &ids[0], 1, 3, None, &|_| false).unwrap();
        apply(&mut st, &plan);
        let arrived = BlockId::new();
        st.apply(
            &Command::InsertBlock {
                id: arrived.clone(),
                parent: Some(ids[0].clone()),
                after: None,
                text: "from another device".into(),
                kind: BlockKind::Bullet,
            },
            4,
        )
        .unwrap();
        let before = st.clone();
        assert!(matches!(
            restore_after(&st, &cut, &arrived, false),
            Err(DomainError::NoOp)
        ));
        assert_eq!(st.canonical(), before.canonical(), "nothing was written");
    }

    #[test]
    fn vm_10_restore_after_refuses_a_block_that_moved_away() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0)]);
        let cut = block_ids(&st, &ids[0], 1).unwrap();
        let plan = delete_blocks(&st, &ids[0], 1, 3, None, &|_| false).unwrap();
        apply(&mut st, &plan);
        st.apply(
            &Command::SetMoved {
                ids: vec![ids[0].clone()],
                to: Some("elsewhere".into()),
            },
            5,
        )
        .unwrap();
        assert!(matches!(
            restore_after(&st, &cut, &ids[1], false),
            Err(DomainError::NoOp)
        ));
    }

    #[test]
    fn vm_8_delete_blocks_focus_ignores_rows_a_collapse_hides() {
        let (st, ids) = page_with(&[("a", 0), ("a1", 1), ("b", 0)]);
        let collapsed = |b: &BlockId| b == &ids[0];
        let plan = delete_blocks(&st, &ids[2], 1, 9, None, &collapsed).unwrap();
        assert_eq!(
            plan.focus,
            Some(Focus {
                block: ids[0].clone(),
                offset: "a".len()
            }),
            "the row above is `a`, not its hidden child"
        );
    }
    #[test]
    fn vm_8_join_next_takes_the_next_visible_row() {
        let (mut st, ids) = page_with(&[("a", 0), ("a1", 1), ("b", 0)]);
        let collapsed = |_b: &BlockId| false;
        let plan = join_next(&st, &ids[0], 1, None, &collapsed, 1).unwrap();
        apply(&mut st, &plan);
        assert_eq!(rows(&st), vec![("a a1".into(), 0), ("b".into(), 0)]);

        let (mut st, ids) = page_with(&[("a", 0), ("a1", 1), ("b", 0)]);
        let collapsed = |b: &BlockId| b == &ids[0];
        let plan = join_next(&st, &ids[0], 1, None, &collapsed, 1).unwrap();
        apply(&mut st, &plan);
        assert_eq!(rows(&st), vec![("a b".into(), 0), ("a1".into(), 1)]);
    }

    #[test]
    fn vm_10_insert_tree_keeps_kind_and_done_state() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0)]);
        let tree = vec![
            TreeRow {
                depth: 0,
                text: "todo".into(),
                kind: BlockKind::Todo,
                checked: true,
            },
            TreeRow {
                depth: 1,
                text: "heading".into(),
                kind: BlockKind::Heading(2),
                checked: false,
            },
        ];
        let plan = insert_tree_after(&st, &ids[0], &tree, false, 1).unwrap();
        let first = plan.focus.as_ref().unwrap().block.clone();
        assert_ne!(
            first, ids[0],
            "new ids, so the copy does not take the original's place"
        );
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("a".into(), 0),
                ("todo".into(), 0),
                ("heading".into(), 1),
                ("b".into(), 0)
            ]
        );
        let inserted = st.block(&first).unwrap();
        assert_eq!(inserted.kind, BlockKind::Todo);
        assert!(inserted.checked, "a checked to-do stays checked");
        let child = st.live_children(Some(&first))[0].clone();
        assert_eq!(st.block(&child).unwrap().kind, BlockKind::Heading(2));

        let plan = insert_tree_after(&st, &ids[1], &tree, true, 1).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st)[3..].to_vec(),
            vec![("todo".into(), 0), ("heading".into(), 1), ("b".into(), 0)]
        );
        let plan = insert_tree_after(&st, &ids[0], &tree[..1], false, 2).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st)[1..3].to_vec(),
            vec![("todo".into(), 0), ("todo".into(), 0)]
        );
    }

    #[test]
    fn vm_10_insert_tree_refuses_past_the_depth_ceiling() {
        let mut lines: Vec<(String, usize)> =
            (0..MAX_DROP_DEPTH).map(|d| (format!("d{d}"), d)).collect();
        lines.push(("leaf".into(), 0));
        let refs: Vec<(&str, usize)> = lines.iter().map(|(t, d)| (t.as_str(), *d)).collect();
        let (st, ids) = page_with(&refs);
        let deep = ids[MAX_DROP_DEPTH - 1].clone();
        let shallow = ids[MAX_DROP_DEPTH].clone();
        let tree = vec![
            TreeRow {
                depth: 0,
                text: "x".into(),
                kind: BlockKind::Bullet,
                checked: false,
            },
            TreeRow {
                depth: 1,
                text: "y".into(),
                kind: BlockKind::Bullet,
                checked: false,
            },
        ];
        assert_eq!(
            insert_tree_after(&st, &deep, &tree, false, 1),
            Err(DomainError::TooDeep)
        );
        assert!(insert_tree_after(&st, &shallow, &tree, false, 1).is_ok());
    }

    #[test]
    fn vm_10_restore_after_keeps_the_ids() {
        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("b1", 1), ("c", 0)]);
        let cut = block_ids(&st, &ids[1], 1).unwrap();
        {
            let p = delete_blocks(&st, &ids[1], 1, 5, None, &|_| false).unwrap();
            apply(&mut st, &p);
        }
        assert_eq!(rows(&st), vec![("a".into(), 0), ("c".into(), 0)]);
        let plan = restore_after(&st, &cut, &ids[3], false).unwrap();
        assert!(matches!(
            plan.cmds[0],
            Command::SetDeleted {
                deleted_at: None,
                ..
            }
        ));
        assert_eq!(
            plan.focus,
            Some(Focus {
                block: ids[1].clone(),
                offset: 0
            })
        );
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![
                ("a".into(), 0),
                ("c".into(), 0),
                ("b".into(), 0),
                ("b1".into(), 1)
            ]
        );
        assert_eq!(
            st.block(&ids[2]).unwrap().deleted_at,
            None,
            "the subtree came back too"
        );

        let (mut st, ids) = page_with(&[("a", 0), ("b", 0), ("c", 0)]);
        let cut = block_ids(&st, &ids[1], 1).unwrap();
        {
            let p = delete_blocks(&st, &ids[1], 1, 5, None, &|_| false).unwrap();
            apply(&mut st, &p);
        }
        let plan = restore_after(&st, &cut, &ids[2], true).unwrap();
        apply(&mut st, &plan);
        assert_eq!(
            rows(&st),
            vec![("a".into(), 0), ("b".into(), 0), ("c".into(), 0)]
        );
        assert_eq!(
            st.live_roots(),
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()]
        );
    }

    #[test]
    fn vm_10_restore_after_refuses_live_and_missing_blocks() {
        let (st, ids) = page_with(&[("a", 0), ("b", 0)]);
        assert_eq!(
            restore_after(&st, &[ids[1].clone()], &ids[0], false),
            Err(DomainError::NoOp)
        );
        assert_eq!(
            restore_after(&st, &[], &ids[0], false),
            Err(DomainError::NoOp)
        );
        let missing = BlockId::new();
        assert_eq!(
            restore_after(&st, std::slice::from_ref(&missing), &ids[0], false),
            Err(DomainError::BlockNotFound(missing))
        );
    }
}
