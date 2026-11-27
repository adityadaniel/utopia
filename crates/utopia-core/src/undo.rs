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
    run_start: Option<usize>,
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
        let floor = self.run_start.unwrap_or(0);
        let may_join = self.undo.len() > floor;
        if may_join {
            if let Some(top) = self.undo.last_mut() {
                if group.coalesce.joins(&top.coalesce) {
                    top.steps.append(&mut group.steps);
                    top.coalesce = group.coalesce;
                    top.caret_after = group.caret_after;
                    return;
                }
            }
        }
        self.undo.push(group);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
            if let Some(start) = self.run_start.as_mut() {
                *start = start.saturating_sub(1);
            }
        }
    }

    pub fn begin_run(&mut self) {
        if self.run_start.is_some() {
            return;
        }
        self.seal();
        self.run_start = Some(self.undo.len());
    }

    pub fn end_run(&mut self) {
        let Some(start) = self.run_start.take() else {
            return;
        };
        let start = start.min(self.undo.len());
        if self.undo.len() <= start + 1 {
            self.seal();
            return;
        }
        let mut groups = self.undo.drain(start..);
        let Some(first) = groups.next() else {
            return;
        };
        let mut folded = UndoGroup {
            steps: first.steps,
            coalesce: Coalesce::Sealed,
            caret_before: first.caret_before,
            caret_after: first.caret_after,
        };
        for group in groups {
            folded.steps.extend(group.steps);
            folded.caret_after = group.caret_after;
        }
        self.undo.push(folded);
        while self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
    }

    pub fn in_run(&self) -> bool {
        self.run_start.is_some()
    }

    pub fn seal(&mut self) {
        if let Some(top) = self.undo.last_mut() {
            top.coalesce = Coalesce::Sealed;
        }
    }

    pub fn begin(&mut self, dir: Direction) -> Option<UndoGroup> {
        self.end_run();
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

    #[test]
    fn a_run_that_outlives_the_limit_never_folds_the_edit_before_it() {
        let mut u = UndoStack::new();
        u.push(edit_group("BEFORE", "b0", 0));
        u.seal();
        u.begin_run();
        for i in 0..(LIMIT + 50) {
            u.push(edit_group("run", &format!("b{}", i + 1), i as i64 * 10_000));
            u.seal();
        }
        u.end_run();
        assert!(u.len() <= LIMIT, "the limit still holds: {}", u.len());
        let mut steps = 0;
        while u.can_undo() {
            let Some(g) = u.begin(Direction::Undo) else {
                break;
            };
            u.finish(Direction::Undo, g);
            steps += 1;
            assert!(steps <= LIMIT + 2, "undo did not terminate");
        }
    }

    #[test]
    fn opening_a_run_twice_keeps_the_first_start() {
        let mut u = UndoStack::new();
        u.begin_run();
        u.push(edit_group("one", "b1", 0));
        u.seal();
        u.begin_run();
        u.push(edit_group("two", "b2", 10_000));
        u.seal();
        u.end_run();
        assert_eq!(u.len(), 1, "both groups fold into one step");
    }

    #[test]
    fn closing_a_run_that_was_never_opened_folds_nothing() {
        let mut u = UndoStack::new();
        u.push(edit_group("one", "b1", 0));
        u.seal();
        u.push(edit_group("two", "b2", 10_000));
        u.seal();
        let before = u.len();
        u.end_run();
        assert_eq!(u.len(), before, "the history is untouched");
    }

    #[test]
    fn an_undo_closes_a_run_that_nobody_closed() {
        let mut u = UndoStack::new();
        u.begin_run();
        u.push(edit_group("one", "b1", 0));
        u.seal();
        u.push(edit_group("two", "b2", 10_000));
        u.seal();
        let g = u.begin(Direction::Undo).expect("something to undo");
        u.finish(Direction::Undo, g);
        assert!(!u.in_run(), "the undo closed the run");
        u.push(edit_group("three", "b3", 20_000));
        u.seal();
        assert!(u.can_undo());
    }

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

    fn edit_group(insert: &str, block: &str, at: i64) -> UndoGroup {
        let cmd = Command::SetBlockText {
            id: block.into(),
            edit: TextEdit {
                range: 0..0,
                insert: insert.into(),
            },
        };
        let caret = |offset| Caret {
            page: "p".into(),
            block: block.into(),
            offset,
        };
        UndoGroup {
            steps: vec![UndoStep {
                page: "p".into(),
                forward: vec![cmd.clone()],
                inverse: vec![cmd],
            }],
            coalesce: Coalesce::open(block.into(), at),
            caret_before: Some(caret(0)),
            caret_after: Some(caret(insert.len())),
        }
    }

    fn inserts(group: &UndoGroup) -> Vec<String> {
        group
            .steps
            .iter()
            .map(|s| match &s.forward[0] {
                Command::SetBlockText { edit, .. } => edit.insert.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_run_folds_its_groups_into_one_in_push_order() {
        let mut s = UndoStack::new();
        s.begin_run();
        assert!(s.in_run());
        s.push(edit_group("a", "b", 0));
        s.push(edit_group("b", "b", 5000));
        s.push(edit_group("c", "b", 10000));
        assert_eq!(s.len(), 3, "still three groups until the run ends");
        s.end_run();
        assert!(!s.in_run());
        assert_eq!(s.len(), 1, "one Insert run is one undo step");
        assert_eq!(inserts(&s.undo[0]), ["a", "b", "c"], "in push order");
        assert_eq!(
            s.undo[0].caret_before.as_ref().map(|c| c.offset),
            Some(0),
            "the run undoes to where it started"
        );
        assert_eq!(
            s.undo[0].caret_after.as_ref().map(|c| c.offset),
            Some(1),
            "and redoes to where it ended"
        );
    }

    #[test]
    fn a_run_does_not_coalesce_with_the_group_before_it() {
        let mut s = UndoStack::new();
        s.push(text_group("b", 0));
        s.begin_run();
        s.push(text_group("b", 100));
        s.end_run();
        assert_eq!(s.len(), 2, "the run is a step of its own");
        s.push(text_group("b", 200));
        assert_eq!(s.len(), 3, "and the folded group is sealed");
    }

    #[test]
    fn a_run_that_holds_one_group_keeps_it() {
        let mut s = UndoStack::new();
        s.begin_run();
        s.push(edit_group("a", "b", 0));
        s.end_run();
        assert_eq!(s.len(), 1);
        assert_eq!(inserts(&s.undo[0]), ["a"]);
        s.begin_run();
        s.end_run();
        assert_eq!(s.len(), 1, "an empty run is not a group");
    }

    #[test]
    fn the_limit_holds_after_a_fold() {
        let mut s = UndoStack::new();
        for i in 0..LIMIT {
            s.push(text_group(&format!("b{i}"), 0));
        }
        assert_eq!(s.len(), LIMIT);
        s.begin_run();
        for i in 0..50 {
            s.push(text_group(&format!("run{i}"), 0));
        }
        s.end_run();
        assert!(
            s.len() <= LIMIT,
            "a fold leaves the stack at or under its cap: {}",
            s.len()
        );
        assert_eq!(s.undo.last().unwrap().steps.len(), 50);

        s.begin_run();
        for i in 0..(LIMIT + 100) {
            s.push(text_group(&format!("long{i}"), 0));
        }
        s.end_run();
        assert!(s.len() <= LIMIT, "{}", s.len());
        assert!(!s.in_run());
    }

    #[test]
    fn an_undo_during_a_run_closes_it_first() {
        let mut s = UndoStack::new();
        s.push(edit_group("before", "b", 0));
        s.begin_run();
        s.push(edit_group("one", "b", 5000));
        s.push(edit_group("two", "b", 10000));
        let group = s.begin(Direction::Undo).expect("the run is an undo step");
        s.finish(Direction::Undo, group);
        assert_eq!(s.len(), 1, "the edit before the run is still there");
        assert_eq!(
            inserts(&s.undo[0]),
            ["before"],
            "the undo took the run, not the edit before it"
        );
        assert!(!s.in_run(), "and closed the run");
        let group = s
            .begin(Direction::Redo)
            .expect("the run is on the redo stack");
        assert_eq!(inserts(&group), ["one", "two"]);
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
