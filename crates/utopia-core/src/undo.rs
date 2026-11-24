use crate::command::Command;
use crate::ids::{BlockId, PageId};

pub const COALESCE_MS: i64 = 500;
const LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoStep {
    pub page: PageId,
    pub forward: Vec<Command>,
    pub inverse: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caret {
    pub page: PageId,
    pub block: BlockId,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Coalesce {
    #[default]
    Sealed,
    Open {
        block: BlockId,
        at_ms: i64,
    },
}

impl Coalesce {
    pub fn open(block: BlockId, at_ms: i64) -> Self {
        Coalesce::Open { block, at_ms }
    }

    fn joins(&self, top: &Coalesce) -> bool {
        match (self, top) {
            (
                Coalesce::Open { block, at_ms },
                Coalesce::Open {
                    block: top_block,
                    at_ms: top_at,
                },
            ) => block == top_block && (0..=COALESCE_MS).contains(&(at_ms - top_at)),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoGroup {
    pub steps: Vec<UndoStep>,
    pub coalesce: Coalesce,
    pub caret_before: Option<Caret>,
    pub caret_after: Option<Caret>,
}

impl Default for UndoGroup {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            coalesce: Coalesce::Sealed,
            caret_before: None,
            caret_after: None,
        }
    }
}

impl UndoGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.iter().all(|s| s.forward.is_empty())
    }

    pub fn pages(&self) -> Vec<PageId> {
        let mut v: Vec<PageId> = self.steps.iter().map(|s| s.page.clone()).collect();
        v.sort();
        v.dedup();
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Undo,
    Redo,
}

#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<UndoGroup>,
    redo: Vec<UndoGroup>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, mut group: UndoGroup) {
        if group.is_empty() {
            return;
        }
        self.redo.clear();
        if let Some(top) = self.undo.last_mut() {
            if group.coalesce.joins(&top.coalesce) {
                top.steps.append(&mut group.steps);
                top.coalesce = group.coalesce;
                top.caret_after = group.caret_after;
                return;
            }
        }
        self.undo.push(group);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    pub fn seal(&mut self) {
        if let Some(top) = self.undo.last_mut() {
            top.coalesce = Coalesce::Sealed;
        }
    }

    pub fn begin(&mut self, dir: Direction) -> Option<UndoGroup> {
        match dir {
            Direction::Undo => self.undo.pop(),
            Direction::Redo => self.redo.pop(),
        }
    }

    pub fn finish(&mut self, dir: Direction, mut group: UndoGroup) {
        group.coalesce = Coalesce::Sealed;
        match dir {
            Direction::Undo => self.redo.push(group),
            Direction::Redo => self.undo.push(group),
        }
    }

    pub fn abort(&mut self, dir: Direction, group: UndoGroup) {
        match dir {
            Direction::Undo => self.undo.push(group),
            Direction::Redo => self.redo.push(group),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::TextEdit;

    fn text_group(block: &str, at: i64) -> UndoGroup {
        let cmd = Command::SetBlockText {
            id: block.into(),
            edit: TextEdit {
                range: 0..0,
                insert: "x".into(),
            },
        };
        UndoGroup {
            steps: vec![UndoStep {
                page: "p".into(),
                forward: vec![cmd.clone()],
                inverse: vec![cmd],
            }],
            coalesce: Coalesce::open(block.into(), at),
            caret_before: None,
            caret_after: None,
        }
    }

    #[test]
    fn coalesces_within_window_only() {
        let mut s = UndoStack::new();
        s.push(text_group("a", 0));
        s.push(text_group("a", 400));
        s.push(text_group("a", 800));
        assert_eq!(
            s.len(),
            1,
            "each edit within 500 ms of the previous coalesces"
        );
        s.push(text_group("a", 1400));
        assert_eq!(s.len(), 2);
        s.push(text_group("b", 1450));
        assert_eq!(s.len(), 3, "different block never coalesces");
        s.seal();
        s.push(text_group("b", 1460));
        assert_eq!(s.len(), 4, "sealed group never coalesces");
    }

    #[test]
    fn a_clock_that_goes_backwards_seals_the_group() {
        let mut s = UndoStack::new();
        s.push(text_group("a", 1000));
        s.push(text_group("a", 900));
        assert_eq!(s.len(), 2, "an earlier timestamp is not an adjacent edit");
    }

    #[test]
    fn redo_cleared_by_new_edit() {
        let mut s = UndoStack::new();
        s.push(text_group("a", 0));
        let g = s.begin(Direction::Undo).unwrap();
        s.finish(Direction::Undo, g);
        assert!(s.can_redo());
        s.push(text_group("b", 5000));
        assert!(!s.can_redo());
    }

    #[test]
    fn an_aborted_transition_restores_the_group_as_it_was() {
        let mut s = UndoStack::new();
        s.push(text_group("a", 0));
        let g = s.begin(Direction::Undo).unwrap();
        s.abort(Direction::Undo, g);
        assert_eq!(s.len(), 1);
        assert!(!s.can_redo());
        s.push(text_group("a", 100));
        assert_eq!(s.len(), 1, "the run of keystrokes is unchanged");
    }
}
