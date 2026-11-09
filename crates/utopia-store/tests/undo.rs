//! Undo history acceptance scripts (FR-2.12): a Cmd+Z either applies its group and moves it
//! to the other stack, or leaves the history exactly as it was. See
//! `docs/state-machines/undo-group-coalescing.md`.

use chrono::NaiveDate;
use utopia_core::outline;
use utopia_core::{BlockId, Command, PageId, TextEdit};
use utopia_store::{EditOpts, Store};

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn store() -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(date(2026, 11, 2)));
    store
}

/// Append a block, type `text` and commit it: three steps, all heading for the history.
fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let block = s
        .execute_plan(page, plan, None)
        .unwrap()
        .map(|f| f.block)
        .unwrap();
    s.set_block_text(page, &block, text, None, None).unwrap();
    s.commit_block(page, &block).unwrap();
    block
}

fn text(s: &mut Store, page: &PageId, block: &BlockId) -> String {
    s.page_state(page)
        .unwrap()
        .block(block)
        .unwrap()
        .text
        .clone()
}

/// Insert `text` at the start of `block`, as a hand-built command group.
fn insert(block: &BlockId, text: &str) -> Command {
    Command::SetBlockText {
        id: block.clone(),
        edit: TextEdit {
            range: 0..0,
            insert: text.into(),
        },
    }
}

/// Issue 1: `undo` popped the group before anything fallible ran, so a step whose page had
/// no shard left (Empty Trash) took the whole group with it — after the earlier steps'
/// inverses had already been applied in memory.
#[test]
fn fr_2_12_undo_keeps_the_group_when_a_page_is_gone() {
    let mut s = store();
    let alpha = s.create_page("Alpha").unwrap();
    let beta = s.create_page("Beta").unwrap();
    let a_block = write(&mut s, &alpha, "alpha");
    let b_block = write(&mut s, &beta, "beta");

    // Beta goes to the trash, then one group edits both pages. Undo walks the steps in
    // reverse, so the live page comes first and the purged one fails second.
    s.delete_page(&beta).unwrap();
    s.execute_multi(
        vec![
            (beta.clone(), vec![insert(&b_block, "!")]),
            (alpha.clone(), vec![insert(&a_block, ">")]),
        ],
        EditOpts::default(),
    )
    .unwrap();
    assert_eq!(s.empty_trash(None).unwrap(), 1, "Beta is gone for good");

    let undone = s.undo();
    assert!(
        undone.is_ok(),
        "a purged page no longer stops the undo: {undone:?}"
    );
    assert_eq!(
        text(&mut s, &alpha, &a_block),
        "alpha",
        "the live page's edit came off"
    );
    assert!(
        s.can_redo(),
        "the group moved to the redo stack instead of being lost"
    );

    // The group is intact on the other stack: redo puts the live page's edit back and
    // skips the purged page the same way.
    let redone = s.redo();
    assert!(redone.is_ok(), "redo skips the purged page: {redone:?}");
    assert_eq!(text(&mut s, &alpha, &a_block), ">alpha");
    assert!(s.can_undo(), "the group came back to the undo stack");
}

/// Issue 1, the queue branch: the inverses were applied and the group popped, then the
/// commit failed — so the in-memory shards held the undo while neither stack did.
#[test]
fn fr_2_12_undo_keeps_the_group_when_its_commit_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(dir.path().join("graph.utopia")).unwrap();
    s.set_today(Some(date(2026, 11, 2)));
    let page = s.create_page("Alpha").unwrap();
    let block = write(&mut s, &page, "one");
    s.set_block_text(&page, &block, "one two", None, None)
        .unwrap();
    s.seal_undo();
    s.sync_index().unwrap();

    // A writer that aborts every commit (block projections included), discovered by the
    // barrier so the undo's own flush is the call that reports it.
    s.conn()
        .execute_batch(
            "CREATE TRIGGER fail_commit BEFORE INSERT ON block
             BEGIN SELECT RAISE(ABORT, 'injected commit failure'); END",
        )
        .unwrap();
    s.set_block_text(&page, &block, "one two three", None, None)
        .unwrap();
    assert!(s.sync_index().is_err(), "the injected failure is reported");

    let undone = s.undo();
    assert!(
        undone.is_err(),
        "a failed commit is reported, not swallowed"
    );
    assert_eq!(
        text(&mut s, &page, &block),
        "one two",
        "the shard has the undo (rule 5)"
    );
    assert!(
        s.can_redo(),
        "the group moved to the redo stack instead of into the void"
    );
}
