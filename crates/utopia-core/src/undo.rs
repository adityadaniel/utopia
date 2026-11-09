//! Undo/redo (FR-2.12). Each group stores the forward commands and their inverses per
//! page, so undo and redo are ordinary command applications (and therefore sync like any
//! other edit). Consecutive text edits to the same block within 500 ms coalesce.
//!
//! The stack is global rather than per page: a merge (§8.6) or an edit made through a
//! transclusion (AC-4.2) touches several pages, and a single linear history guarantees
//! every inverse is applied to exactly the state it was computed against.

use crate::command::Command;
use crate::ids::{BlockId, PageId};

pub const COALESCE_MS: i64 = 500;
const LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoStep {
    pub page: PageId,
    pub forward: Vec<Command>,
    /// Applied in order to undo this step.
    pub inverse: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caret {
    pub page: PageId,
    pub block: BlockId,
    pub offset: usize,
}

/// Whether the newest group is still open for coalescing (FR-2.12).
///
/// The time a group opened lives only in [`Coalesce::Open`], so "sealed, but at a time" —
/// a combination that decided nothing — cannot be constructed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Coalesce {
    /// The next text edit starts its own group.
    #[default]
    Sealed,
    /// A text edit of `block` at most [`COALESCE_MS`] after `at_ms` joins this group.
    Open { block: BlockId, at_ms: i64 },
}

impl Coalesce {
    /// Open for a text edit of `block` made at `at_ms`.
    pub fn open(block: BlockId, at_ms: i64) -> Self {
        Coalesce::Open { block, at_ms }
    }

    /// Whether a group in this state may merge into `top`: only two open states for the same
    /// block, at most [`COALESCE_MS`] apart. A clock that went backwards (`at_ms` before the
    /// top's) starts a new group instead of merging edits that are not adjacent in time.
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
    /// Whether the next edit may join this group; only a plain text edit ever opens it.
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

/// The direction of one history transition: a Cmd+Z or a Cmd+Shift+Z.
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

    /// Record a freshly performed group. Clears redo.
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

    /// Stop the current group from absorbing further edits (e.g. on blur or navigation).
    pub fn seal(&mut self) {
        if let Some(top) = self.undo.last_mut() {
            top.coalesce = Coalesce::Sealed;
        }
    }

    /// Take the group `dir` applies out of the history.
    ///
    /// From here the caller owns it and must hand it back: [`UndoStack::finish`] when the
    /// commands were applied, [`UndoStack::abort`] when they were not. That is what makes
    /// a group that sits in neither stack — history that vanished because something after
    /// the pop failed — impossible to produce.
    pub fn begin(&mut self, dir: Direction) -> Option<UndoGroup> {
        match dir {
            Direction::Undo => self.undo.pop(),
            Direction::Redo => self.redo.pop(),
        }
    }

    /// The transition happened: the group is now the other stack's top. It never coalesces,
    /// because applying a group ends the run of text edits it belonged to.
    pub fn finish(&mut self, dir: Direction, mut group: UndoGroup) {
        group.coalesce = Coalesce::Sealed;
        match dir {
            Direction::Undo => self.redo.push(group),
            Direction::Redo => self.undo.push(group),
        }
    }

    /// The transition did not happen: the group goes back where it came from, unchanged —
    /// its coalescing state included, so a failed Cmd+Z does not split a run of keystrokes.
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
        // Still open for coalescing: a failed undo is not an edit boundary.
        s.push(text_group("a", 100));
        assert_eq!(s.len(), 1, "the run of keystrokes is unchanged");
    }
}
