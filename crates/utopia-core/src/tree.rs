use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::command::{BlockKind, Command, DocDelta, PageMeta};
use crate::error::DomainError;
use crate::ids::{BlockId, PageId};
use crate::text::apply_edit;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockNode {
    pub id: BlockId,
    pub parent: Option<BlockId>,
    pub children: Vec<BlockId>,
    pub text: String,
    pub kind: BlockKind,
    pub checked: bool,
    pub position: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
    pub moved_to: Option<PageId>,
    pub present: bool,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageState {
    pub page_id: PageId,
    pub meta: PageMeta,
    pub roots: Vec<BlockId>,
    pub blocks: HashMap<BlockId, BlockNode>,
    pub version: u64,
}

impl PageState {
    pub fn new(page_id: PageId, meta: PageMeta) -> Self {
        Self {
            page_id,
            meta,
            roots: Vec::new(),
            blocks: HashMap::new(),
            version: 0,
        }
    }

    pub fn block(&self, id: &BlockId) -> Option<&BlockNode> {
        self.blocks.get(id)
    }

    pub fn contains(&self, id: &BlockId) -> bool {
        self.blocks.contains_key(id)
    }

    pub fn is_live(&self, id: &BlockId) -> bool {
        self.blocks.get(id).is_some_and(|b| b.live)
    }

    pub fn children_of(&self, parent: Option<&BlockId>) -> &[BlockId] {
        match parent {
            None => &self.roots,
            Some(p) => self.blocks.get(p).map_or(&[], |b| b.children.as_slice()),
        }
    }

    fn siblings_filtered(&self, parent: Option<&BlockId>, live_only: bool) -> Vec<BlockId> {
        self.children_of(parent)
            .iter()
            .filter(|c| !live_only || self.is_live(c))
            .cloned()
            .collect()
    }

    pub fn live_children(&self, parent: Option<&BlockId>) -> Vec<BlockId> {
        self.siblings_filtered(parent, true)
    }

    pub fn live_roots(&self) -> Vec<BlockId> {
        self.live_children(None)
    }

    pub fn has_live_children(&self, id: &BlockId) -> bool {
        self.children_of(Some(id)).iter().any(|c| self.is_live(c))
    }

    pub fn parent_of(&self, id: &BlockId) -> Option<BlockId> {
        self.blocks.get(id).and_then(|b| b.parent.clone())
    }

    pub fn index_in_parent(&self, id: &BlockId, live_only: bool) -> Option<usize> {
        let parent = self.blocks.get(id)?.parent.clone();
        self.siblings_filtered(parent.as_ref(), live_only)
            .iter()
            .position(|c| c == id)
    }

    pub fn prev_sibling(&self, id: &BlockId, live_only: bool) -> Option<BlockId> {
        let parent = self.blocks.get(id)?.parent.clone();
        let sibs = self.siblings_filtered(parent.as_ref(), live_only);
        let ix = sibs.iter().position(|c| c == id)?;
        ix.checked_sub(1).map(|i| sibs[i].clone())
    }

    pub fn next_sibling(&self, id: &BlockId, live_only: bool) -> Option<BlockId> {
        let parent = self.blocks.get(id)?.parent.clone();
        let sibs = self.siblings_filtered(parent.as_ref(), live_only);
        let ix = sibs.iter().position(|c| c == id)?;
        sibs.get(ix + 1).cloned()
    }

    pub fn last_child(&self, parent: Option<&BlockId>, live_only: bool) -> Option<BlockId> {
        self.siblings_filtered(parent, live_only).last().cloned()
    }

    pub fn depth(&self, id: &BlockId) -> usize {
        self.ancestors(id).len()
    }

    pub fn ancestors(&self, id: &BlockId) -> Vec<BlockId> {
        let mut out = Vec::new();
        let mut cur = self.parent_of(id);
        let mut guard = 0;
        while let Some(p) = cur {
            guard += 1;
            if guard > self.blocks.len() {
                break;
            }
            cur = self.parent_of(&p);
            out.push(p);
        }
        out.reverse();
        out
    }

    pub fn is_ancestor(&self, a: &BlockId, b: &BlockId) -> bool {
        self.ancestors(b).iter().any(|x| x == a)
    }

    pub fn subtree(&self, id: &BlockId, live_only: bool) -> Vec<BlockId> {
        let mut out = Vec::new();
        let mut stack = vec![id.clone()];
        while let Some(cur) = stack.pop() {
            if live_only && !self.is_live(&cur) {
                continue;
            }
            if let Some(b) = self.blocks.get(&cur) {
                out.push(cur.clone());
                for c in b.children.iter().rev() {
                    stack.push(c.clone());
                }
            }
        }
        out
    }

    pub fn live_preorder(&self) -> Vec<(BlockId, usize)> {
        self.visible_rows(None, &|_| false)
    }

    pub fn visible_rows(
        &self,
        zoom: Option<&BlockId>,
        collapsed: &dyn Fn(&BlockId) -> bool,
    ) -> Vec<(BlockId, usize)> {
        let mut out = Vec::new();
        let mut stack: Vec<(BlockId, usize)> = self
            .live_children(zoom)
            .into_iter()
            .rev()
            .map(|c| (c, 0))
            .collect();
        while let Some((id, d)) = stack.pop() {
            let expand = !collapsed(&id);
            if expand {
                for c in self.live_children(Some(&id)).into_iter().rev() {
                    stack.push((c, d + 1));
                }
            }
            out.push((id, d));
        }
        out
    }

    pub fn live_count(&self) -> usize {
        self.blocks.values().filter(|b| b.live).count()
    }

    fn live_of(&self, id: &BlockId) -> bool {
        let Some(b) = self.blocks.get(id) else {
            return false;
        };
        b.present
            && (b.deleted_at.is_none()
                || b.children
                    .iter()
                    .any(|c| self.blocks.get(c).is_some_and(|c| c.live)))
    }

    fn refresh_liveness_at(&mut self, id: &BlockId) {
        let order = self.subtree(id, false);
        for b in &order {
            let parent_present = self
                .parent_of(b)
                .is_none_or(|p| self.blocks.get(&p).is_some_and(|p| p.present));
            if let Some(n) = self.blocks.get_mut(b) {
                n.present = parent_present && n.moved_to.is_none();
            }
        }
        for b in order.iter().rev() {
            let live = self.live_of(b);
            if let Some(n) = self.blocks.get_mut(b) {
                n.live = live;
            }
        }
        self.refresh_ancestors(id);
    }

    fn refresh_ancestors(&mut self, id: &BlockId) {
        for a in self.ancestors(id).into_iter().rev() {
            let live = self.live_of(&a);
            if let Some(n) = self.blocks.get_mut(&a) {
                n.live = live;
            }
        }
    }

    pub fn recompute_liveness(&mut self) {
        fn visit(
            st: &mut HashMap<BlockId, BlockNode>,
            id: &BlockId,
            parent_present: bool,
            seen: &mut HashSet<BlockId>,
        ) -> bool {
            if !seen.insert(id.clone()) {
                return false;
            }
            let (children, present, deleted) = match st.get(id) {
                Some(b) => (
                    b.children.clone(),
                    parent_present && b.moved_to.is_none(),
                    b.deleted_at.is_some(),
                ),
                None => return false,
            };
            let mut any_live_child = false;
            for c in &children {
                any_live_child |= visit(st, c, present, seen);
            }
            let live = present && (!deleted || any_live_child);
            if let Some(b) = st.get_mut(id) {
                b.present = present;
                b.live = live;
            }
            live
        }
        let mut seen = HashSet::new();
        for r in self.roots.clone() {
            visit(&mut self.blocks, &r, true, &mut seen);
        }
    }

    fn remove_from_parent(&mut self, id: &BlockId) {
        let parent = self.parent_of(id);
        let list = match &parent {
            None => &mut self.roots,
            Some(p) => match self.blocks.get_mut(p) {
                Some(b) => &mut b.children,
                None => return,
            },
        };
        list.retain(|c| c != id);
    }

    fn insert_into_parent(
        &mut self,
        id: &BlockId,
        parent: Option<&BlockId>,
        after: Option<&BlockId>,
    ) {
        let list = match parent {
            None => &mut self.roots,
            Some(p) => {
                &mut self
                    .blocks
                    .get_mut(p)
                    .expect("parent checked by caller")
                    .children
            }
        };
        let ix = match after {
            None => 0,
            Some(a) => list
                .iter()
                .position(|c| c == a)
                .map_or(list.len(), |i| i + 1),
        };
        list.insert(ix, id.clone());
    }

    fn node_mut(&mut self, id: &BlockId) -> Result<&mut BlockNode, DomainError> {
        self.blocks
            .get_mut(id)
            .ok_or_else(|| DomainError::BlockNotFound(id.clone()))
    }

    pub fn apply(&mut self, cmd: &Command, now: i64) -> Result<DocDelta, DomainError> {
        let mut delta = DocDelta::default();
        let mut local_liveness = false;
        match cmd {
            Command::InsertBlock {
                id,
                parent,
                after,
                text,
                kind,
            } => {
                if self.blocks.contains_key(id) {
                    return Err(DomainError::NoOp);
                }
                if let Some(p) = parent {
                    if !self.blocks.contains_key(p) {
                        return Err(DomainError::BlockNotFound(p.clone()));
                    }
                }
                self.blocks.insert(
                    id.clone(),
                    BlockNode {
                        id: id.clone(),
                        parent: parent.clone(),
                        children: Vec::new(),
                        text: text.clone(),
                        kind: *kind,
                        checked: false,
                        position: String::new(),
                        created_at: id.timestamp_ms().unwrap_or(now),
                        updated_at: now,
                        deleted_at: None,
                        moved_to: None,
                        present: true,
                        live: true,
                    },
                );
                let after = after.as_ref().filter(|a| *a != id);
                self.insert_into_parent(id, parent.as_ref(), after);
                delta.inserted.push(id.clone());
                delta.structural = true;
                self.refresh_liveness_at(id);
                local_liveness = true;
            }
            Command::PurgeBlock { id } => {
                if !self.blocks.contains_key(id) {
                    return Err(DomainError::BlockNotFound(id.clone()));
                }
                let sub = self.subtree(id, false);
                self.remove_from_parent(id);
                for b in &sub {
                    self.blocks.remove(b);
                }
                delta.removed = sub;
                delta.structural = true;
            }
            Command::MoveBlock { id, parent, after } => {
                if !self.blocks.contains_key(id) {
                    return Err(DomainError::BlockNotFound(id.clone()));
                }
                if let Some(p) = parent {
                    if !self.blocks.contains_key(p) {
                        return Err(DomainError::BlockNotFound(p.clone()));
                    }
                    if p == id || self.is_ancestor(id, p) {
                        return Err(DomainError::CycleDetected(id.clone()));
                    }
                }
                let old_parent = self.parent_of(id);
                self.remove_from_parent(id);
                let after = after.as_ref().filter(|a| *a != id);
                self.insert_into_parent(id, parent.as_ref(), after);
                let n = self.node_mut(id)?;
                n.parent = parent.clone();
                n.updated_at = now;
                delta.changed = self.subtree(id, false);
                delta.structural = true;
                self.refresh_liveness_at(id);
                if let Some(op) = old_parent {
                    if self.blocks.contains_key(&op) {
                        let live = self.live_of(&op);
                        if let Some(n) = self.blocks.get_mut(&op) {
                            n.live = live;
                        }
                        self.refresh_ancestors(&op);
                    }
                }
                local_liveness = true;
            }
            Command::SetBlockText { id, edit } => {
                let n = self.node_mut(id)?;
                n.text = apply_edit(&n.text, edit).ok_or(DomainError::InvalidTextEdit {
                    start: edit.range.start,
                    end: edit.range.end,
                })?;
                n.updated_at = now;
                delta.changed.push(id.clone());
            }
            Command::SetBlockKind { id, kind, checked } => {
                let n = self.node_mut(id)?;
                n.kind = *kind;
                n.checked = *checked;
                n.updated_at = now;
                delta.changed.push(id.clone());
            }
            Command::SetDeleted { ids, deleted_at } => {
                for id in ids {
                    let n = self.node_mut(id)?;
                    n.deleted_at = *deleted_at;
                    n.updated_at = now;
                    delta.changed.push(id.clone());
                }
                delta.structural = true;
            }
            Command::SetMoved { ids, to } => {
                for id in ids {
                    let n = self.node_mut(id)?;
                    n.moved_to = to.clone();
                    n.updated_at = now;
                    delta.changed.push(id.clone());
                }
                delta.structural = true;
            }
            Command::SetPageMeta(p) => {
                p.apply_to(&mut self.meta);
                delta.meta_changed = true;
            }
        }
        if delta.structural && !local_liveness {
            self.recompute_liveness();
        }
        self.version += 1;
        Ok(delta)
    }

    pub fn canonical(&self) -> Vec<String> {
        let mut out = vec![format!(
            "meta|{}|{:?}|{:?}|{:?}|{:?}|{:?}",
            self.meta.title,
            self.meta.kind,
            self.meta.daily_date,
            self.meta.deleted_at.is_some(),
            self.meta.merged_into,
            self.meta.aliases
        )];
        fn walk(st: &PageState, list: &[BlockId], depth: usize, out: &mut Vec<String>) {
            for id in list {
                if let Some(b) = st.blocks.get(id) {
                    out.push(format!(
                        "{}{}|{}|{}|{}|{}|{:?}",
                        "  ".repeat(depth),
                        b.id,
                        b.text,
                        b.kind.type_str(),
                        b.checked,
                        b.deleted_at.is_some(),
                        b.moved_to
                    ));
                    walk(st, &b.children, depth + 1, out);
                }
            }
        }
        walk(self, &self.roots, 0, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::TextEdit;

    pub(crate) fn ins(
        st: &mut PageState,
        id: &str,
        parent: Option<&str>,
        after: Option<&str>,
        text: &str,
    ) {
        st.apply(
            &Command::InsertBlock {
                id: id.into(),
                parent: parent.map(Into::into),
                after: after.map(Into::into),
                text: text.into(),
                kind: BlockKind::Bullet,
            },
            1,
        )
        .unwrap();
    }

    fn page() -> PageState {
        PageState::new("p".into(), PageMeta::default())
    }

    #[test]
    fn insert_order_and_rows() {
        let mut st = page();
        ins(&mut st, "a", None, None, "A");
        ins(&mut st, "c", None, Some("a"), "C");
        ins(&mut st, "b", None, Some("a"), "B");
        ins(&mut st, "a1", Some("a"), None, "A1");
        let rows: Vec<_> = st
            .live_preorder()
            .into_iter()
            .map(|(id, d)| (id.0, d))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("a".into(), 0),
                ("a1".into(), 1),
                ("b".into(), 0),
                ("c".into(), 0)
            ]
        );
        let collapsed = |id: &BlockId| id.0 == "a";
        assert_eq!(st.visible_rows(None, &collapsed).len(), 3);
        assert_eq!(st.visible_rows(Some(&"a".into()), &|_| false).len(), 1);
    }

    #[test]
    fn move_rejects_cycles() {
        let mut st = page();
        ins(&mut st, "a", None, None, "");
        ins(&mut st, "b", Some("a"), None, "");
        let err = st.apply(
            &Command::MoveBlock {
                id: "a".into(),
                parent: Some("b".into()),
                after: None,
            },
            1,
        );
        assert_eq!(err, Err(DomainError::CycleDetected("a".into())));
    }

    #[test]
    fn tombstone_revives_with_live_child() {
        let mut st = page();
        ins(&mut st, "a", None, None, "");
        ins(&mut st, "b", Some("a"), None, "");
        st.apply(
            &Command::SetDeleted {
                ids: vec!["a".into(), "b".into()],
                deleted_at: Some(5),
            },
            2,
        )
        .unwrap();
        assert!(!st.is_live(&"a".into()));
        ins(&mut st, "c", Some("a"), None, "");
        assert!(st.is_live(&"a".into()), "parent revived");
        assert!(!st.is_live(&"b".into()));
    }

    #[test]
    fn every_command_inverts() {
        let mut st = page();
        ins(&mut st, "a", None, None, "hello");
        ins(&mut st, "b", None, Some("a"), "world");
        ins(&mut st, "b1", Some("b"), None, "child");
        let cmds = vec![
            Command::SetBlockText {
                id: "a".into(),
                edit: TextEdit {
                    range: 5..5,
                    insert: " there".into(),
                },
            },
            Command::MoveBlock {
                id: "b".into(),
                parent: Some("a".into()),
                after: None,
            },
            Command::SetDeleted {
                ids: vec!["b1".into()],
                deleted_at: Some(9),
            },
            Command::SetMoved {
                ids: vec!["b".into(), "b1".into()],
                to: Some("q".into()),
            },
            Command::SetBlockKind {
                id: "a".into(),
                kind: BlockKind::Todo,
                checked: true,
            },
            Command::PurgeBlock { id: "a".into() },
            Command::SetPageMeta(crate::command::PageMetaPatch {
                title: Some("T".into()),
                ..Default::default()
            }),
        ];
        for cmd in cmds {
            let before = st.canonical();
            let inv = cmd.invert(&st).unwrap();
            st.apply(&cmd, 3).unwrap();
            assert_ne!(st.canonical(), before, "{cmd:?} changed nothing");
            let mut undone = st.clone();
            for c in &inv {
                undone.apply(c, 4).unwrap();
            }
            assert_eq!(undone.canonical(), before, "inverse of {cmd:?}");
        }
    }
}
