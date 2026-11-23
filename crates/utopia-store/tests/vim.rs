//! Vim's linewise block commands against a real store (docs/vim-mode-plan.md §2C; VM-8,
//! VM-10, VM-11). The UI drives these through `EditorEvent`s; here the planners and the
//! store method are called directly, because the point is what the graph looks like
//! afterwards: every id kept, every `((reference))` still resolving, one undo step per
//! command, and `doctor` clean.

use chrono::NaiveDate;
use utopia_core::outline::{self, Plan};
use utopia_core::{BlockId, BlockKind, Command, PageId};
use utopia_store::{BlockRefStatus, Store};

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A file-backed store, so every script runs through the writer thread. It is dropped
/// (flushing and joining the writer) before its directory is removed.
struct TestStore {
    store: Store,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for TestStore {
    type Target = Store;
    fn deref(&self) -> &Store {
        &self.store
    }
}

impl std::ops::DerefMut for TestStore {
    fn deref_mut(&mut self) -> &mut Store {
        &mut self.store
    }
}

fn store() -> TestStore {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("graph.utopia")).unwrap();
    store.set_today(Some(date(2026, 11, 2)));
    TestStore { store, _dir: dir }
}

fn run(s: &mut Store, page: &PageId, plan: Plan) -> Option<BlockId> {
    s.execute_plan(page, plan, None).unwrap().map(|f| f.block)
}

/// Append a block at the end of a page, type `text` and commit it.
fn write(s: &mut Store, page: &PageId, text: &str) -> BlockId {
    let plan = outline::append_block(s.page_state(page).unwrap(), None);
    let id = run(s, page, plan).unwrap();
    s.set_block_text(page, &id, text, None, None).unwrap();
    s.commit_block(page, &id).unwrap();
    id
}

/// Append a block under `parent`, before `after` (None = first child).
fn write_under(
    s: &mut Store,
    page: &PageId,
    parent: Option<&BlockId>,
    text: &str,
    kind: BlockKind,
) -> BlockId {
    let id = BlockId::new();
    s.execute(
        page,
        vec![Command::InsertBlock {
            id: id.clone(),
            parent: parent.cloned(),
            after: None,
            text: text.to_string(),
            kind,
        }],
        Default::default(),
    )
    .unwrap();
    id
}

fn text(s: &mut Store, page: &PageId, block: &BlockId) -> String {
    s.page_state(page)
        .unwrap()
        .block(block)
        .unwrap()
        .text
        .clone()
}

/// Live rows of a page as `(text, depth)`.
fn rows(s: &mut Store, page: &PageId) -> Vec<(String, usize)> {
    let st = s.page_state(page).unwrap();
    st.live_preorder()
        .into_iter()
        .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
        .collect()
}

/// Every id a page's shard knows, live or tombstoned: rule 5 keeps the rows.
fn all_ids(s: &mut Store, page: &PageId) -> Vec<BlockId> {
    let st = s.page_state(page).unwrap();
    let mut ids: Vec<BlockId> = st.blocks.keys().cloned().collect();
    ids.sort();
    ids
}

fn deleted(s: &mut Store, page: &PageId, id: &BlockId) -> bool {
    s.page_state(page)
        .unwrap()
        .block(id)
        .is_some_and(|b| b.deleted_at.is_some())
}

fn doctor_clean(s: &mut Store) {
    let report = s.doctor().unwrap();
    assert!(report.is_ok(), "rule 2: doctor is clean\n{report:?}");
}

/// VM-10, the headline: `dd` then `p` MOVES the blocks, so every id survives and a
/// `((reference))` to one of them still resolves — which a copy with new ids would break.
#[test]
fn vm_10_dd_then_p_keeps_every_block_id_and_the_reference() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let a = write(&mut s, &src, "alpha");
    let a1 = write_under(&mut s, &src, Some(&a), "alpha child", BlockKind::Bullet);
    let b = write(&mut s, &src, "beta");
    write(&mut s, &src, "gamma");
    // Someone refers to the block that is about to move.
    let other = s.create_page("Other").unwrap();
    write(&mut s, &other, &format!("see (({a1}))"));
    assert_eq!(s.block_backlinks(&a1).unwrap().len(), 1);

    let cut = outline::block_ids(s.page_state(&src).unwrap(), &a, 1).unwrap();
    assert_eq!(cut, vec![a.clone(), a1.clone()], "the subtree comes too");
    let before_ids = all_ids(&mut s, &src);
    let depth0 = s.undo_depth();

    // `dd`: one plan, one undo step.
    let deleted_at = s.now();
    let plan = outline::delete_blocks(
        s.page_state(&src).unwrap(),
        &a,
        1,
        deleted_at,
        None,
        &|_| false,
    )
    .unwrap();
    assert_eq!(
        run(&mut s, &src, plan),
        Some(b.clone()),
        "the cursor goes to the NEXT block, as Vim's does"
    );
    assert_eq!(
        rows(&mut s, &src),
        vec![("beta".into(), 0), ("gamma".into(), 0)]
    );
    assert_eq!(s.undo_depth(), depth0 + 1, "`dd` is one undo step");
    assert!(
        deleted(&mut s, &src, &a),
        "rule 5: a tombstone, not a purge"
    );
    assert!(deleted(&mut s, &src, &a1));

    // `p` after `beta`: restore and move in ONE plan.
    let plan = outline::restore_after(s.page_state(&src).unwrap(), &cut, &b, false).unwrap();
    assert_eq!(run(&mut s, &src, plan), Some(a.clone()));
    assert_eq!(
        rows(&mut s, &src),
        vec![
            ("beta".into(), 0),
            ("alpha".into(), 0),
            ("alpha child".into(), 1),
            ("gamma".into(), 0)
        ]
    );
    assert_eq!(
        s.undo_depth(),
        depth0 + 2,
        "`p` is one undo step of its own"
    );

    // The ids are the same ones, so nothing that pointed at them broke.
    let after_ids = all_ids(&mut s, &src);
    assert_eq!(
        before_ids, after_ids,
        "every block id survived the move (VM-10)"
    );
    assert!(
        s.block_backlinks(&a1).unwrap().len() == 1,
        "the ((reference)) still resolves"
    );
    assert!(matches!(
        s.block_ref_status(a1.as_str()).unwrap(),
        BlockRefStatus::Live { text, .. } if text == "alpha child"
    ));
    assert_eq!(text(&mut s, &src, &a), "alpha", "the text came with it");
    // And one undo takes just the paste back, leaving the blocks tombstoned where they were.
    s.undo().unwrap();
    assert_eq!(
        rows(&mut s, &src),
        vec![("beta".into(), 0), ("gamma".into(), 0)]
    );
    assert!(deleted(&mut s, &src, &a));
    doctor_clean(&mut s);
}

/// VM-10: `dd` then `p` on ANOTHER page restores and moves in one undo group, keeping the
/// ids, so a `((reference))` written on the first page still resolves. The subtree has a
/// tombstone between two live children, which is what Enter-then-Backspace leaves: the paste
/// must still put the children back in order (F1), and `doctor` is clean in the moved state,
/// not only after the undo.
#[test]
fn vm_10_dd_then_p_on_another_page_keeps_every_block_id() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let a = write(&mut s, &src, "alpha");
    let a1 = write_under(&mut s, &src, Some(&a), "alpha child", BlockKind::Todo);
    let gone = write_last(&mut s, &src, Some(&a), "a tombstoned child");
    write_last(&mut s, &src, Some(&a), "alpha last child");
    let now = s.now();
    s.execute(
        &src,
        vec![Command::SetDeleted {
            ids: vec![gone.clone()],
            deleted_at: Some(now),
        }],
        Default::default(),
    )
    .unwrap();
    write(&mut s, &src, "kept");
    let dst = s.create_page("Destination").unwrap();
    let target = write(&mut s, &dst, "here");
    let referrer = s.create_page("Referrer").unwrap();
    write(&mut s, &referrer, &format!("see (({a}))"));

    let cut = outline::block_ids(s.page_state(&src).unwrap(), &a, 1).unwrap();
    let now = s.now();
    let plan =
        outline::delete_blocks(s.page_state(&src).unwrap(), &a, 1, now, None, &|_| false).unwrap();
    run(&mut s, &src, plan);
    assert_eq!(rows(&mut s, &src), vec![("kept".into(), 0)]);
    let depth = s.undo_depth();

    s.restore_and_move_to_page(&src, &cut, &dst, None, Some(&target))
        .unwrap();
    assert_eq!(s.undo_depth(), depth + 1, "one undo group for the paste");
    assert_eq!(
        rows(&mut s, &dst),
        vec![
            ("here".into(), 0),
            ("alpha".into(), 0),
            ("alpha child".into(), 1),
            ("alpha last child".into(), 1)
        ],
        "restored live, in order, under the target block (F1: the tombstone between them \
         stays behind)"
    );
    doctor_clean(&mut s);
    assert_eq!(
        rows(&mut s, &src),
        vec![("kept".into(), 0)],
        "and gone from the source"
    );
    let arrived = s.page_state(&dst).unwrap().block(&a1).cloned();
    let arrived = arrived.expect("the child arrived on the other page");
    assert_eq!(arrived.deleted_at, None, "restored, live");
    assert_eq!(arrived.kind, BlockKind::Todo, "its kind came with it");
    assert!(matches!(
        s.block_ref_status(a.as_str()).unwrap(),
        BlockRefStatus::Live { text, .. } if text == "alpha"
    ));
    assert_eq!(s.block_backlinks(&a).unwrap().len(), 1);
    // The move is undoable as one step.
    s.undo().unwrap();
    assert_eq!(rows(&mut s, &dst), vec![("here".into(), 0)]);
    assert!(
        deleted(&mut s, &src, &a),
        "back to a tombstone on the source"
    );
    doctor_clean(&mut s);
}

/// VM-10: a live block is not a cut. The store refuses it rather than moving a block that
/// another command owns (`Store::move_block_to_page` is the way to move a live one).
#[test]
fn vm_10_restore_and_move_refuses_a_live_block() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let a = write(&mut s, &src, "alpha");
    let dst = s.create_page("Destination").unwrap();
    let err = s.restore_and_move_to_page(&src, &[a], &dst, None, None);
    assert!(err.is_err(), "a live block is a move, not a restore");
}

/// VM-8: `3dd` is ONE undo step and tombstones nine blocks when each root has two children
/// — and they are tombstones, not purged rows (rule 5).
#[test]
fn vm_8_3dd_is_one_undo_step_and_never_hard_deletes() {
    let mut s = store();
    let page = s.create_page("Counted").unwrap();
    let mut ids = Vec::new();
    for r in 0..3 {
        let root = write(&mut s, &page, &format!("root {r}"));
        ids.push(root.clone());
        for k in 0..2 {
            ids.push(write_under(
                &mut s,
                &page,
                Some(&root),
                &format!("kid {r}.{k}"),
                BlockKind::Bullet,
            ));
        }
    }
    let depth0 = s.undo_depth();
    let now = s.now();
    let plan = outline::delete_blocks(s.page_state(&page).unwrap(), &ids[0], 3, now, None, &|_| {
        false
    })
    .unwrap();
    assert_eq!(plan.cmds.len(), 1, "one plan, so one undo step");
    run(&mut s, &page, plan);

    assert_eq!(s.undo_depth(), depth0 + 1, "`3dd` is ONE undo step");
    assert!(rows(&mut s, &page).is_empty(), "all three roots went");
    for id in &ids {
        assert!(deleted(&mut s, &page, id), "rule 5: id {id} is a tombstone");
    }
    assert_eq!(all_ids(&mut s, &page).len(), 9, "nine rows, none purged");
    s.undo().unwrap();
    assert_eq!(s.undo_depth(), depth0);
    assert_eq!(
        rows(&mut s, &page).len(),
        9,
        "one undo brings all nine back"
    );
    assert!(!deleted(&mut s, &page, &ids[0]));
    doctor_clean(&mut s);
}

/// VM-10: a yank of a to-do, pasted, is still a to-do with its done state.
#[test]
fn vm_10_a_yanked_todo_is_still_a_todo() {
    let mut s = store();
    let page = s.create_page("Kinds").unwrap();
    let todo = write(&mut s, &page, "done thing");
    write_under(&mut s, &page, Some(&todo), "child", BlockKind::Bullet);
    s.set_block_kind(&page, &todo, BlockKind::Todo, true)
        .unwrap();
    let anchor = write(&mut s, &page, "anchor");

    let tree = outline::block_tree(s.page_state(&page).unwrap(), &todo, 1).unwrap();
    assert_eq!(tree[0].kind, BlockKind::Todo);
    assert!(tree[0].checked, "the register carries the done state");
    let plan =
        outline::insert_tree_after(s.page_state(&page).unwrap(), &anchor, &tree, false, 1).unwrap();
    let copy = run(&mut s, &page, plan).unwrap();
    assert_ne!(copy, todo, "a copy gets new ids");
    let copied = s.page_state(&page).unwrap().block(&copy).unwrap().clone();
    assert_eq!(copied.kind, BlockKind::Todo);
    assert!(copied.checked, "a checked to-do stays checked (VM-10)");
    let child = s.page_state(&page).unwrap().live_children(Some(&copy))[0].clone();
    assert_eq!(text(&mut s, &page, &child), "child");
    assert!(!deleted(&mut s, &page, &todo), "the original is untouched");
    doctor_clean(&mut s);
}

/// VM-8: `J` takes the next visible block's text, re-parents its children here and
/// tombstones it — the joined block is never hard-deleted (rule 5).
#[test]
fn vm_8_join_next_tombstones_and_reparents() {
    let mut s = store();
    let page = s.create_page("Join").unwrap();
    let a = write(&mut s, &page, "one");
    let b = write(&mut s, &page, "two");
    let b1 = write_under(&mut s, &page, Some(&b), "under two", BlockKind::Bullet);
    let c = write(&mut s, &page, "three");
    let depth0 = s.undo_depth();

    let now = s.now();
    let plan =
        outline::join_next(s.page_state(&page).unwrap(), &a, 1, None, &|_| false, now).unwrap();
    run(&mut s, &page, plan);
    assert_eq!(
        rows(&mut s, &page),
        vec![
            ("one two".into(), 0),
            ("under two".into(), 1),
            ("three".into(), 0)
        ]
    );
    assert_eq!(s.undo_depth(), depth0 + 1, "`J` is one undo step");
    assert!(
        deleted(&mut s, &page, &b),
        "the joined block is a tombstone"
    );
    assert_eq!(text(&mut s, &page, &b), "two", "and its text was not lost");
    assert_eq!(
        s.page_state(&page).unwrap().parent_of(&b1),
        Some(a.clone()),
        "its child re-parented to the block it joined"
    );
    assert!(s.page_state(&page).unwrap().contains(&c));
    doctor_clean(&mut s);
}

/// The whole set of commands one after another, as a session would run them: the graph must
/// stay self-consistent (rule 2) and every id must stay reachable (rule 5).
#[test]
fn vm_8_a_scripted_run_of_block_commands_leaves_doctor_clean() {
    let mut s = store();
    let page = s.create_page("Scripted").unwrap();
    let a = write(&mut s, &page, "a");
    let a1 = write_under(&mut s, &page, Some(&a), "a1", BlockKind::Bullet);
    let b = write(&mut s, &page, "b");
    let c = write(&mut s, &page, "c");
    let referrer = s.create_page("Referrer").unwrap();
    write(&mut s, &referrer, &format!("(({a1})) and (({b}))"));
    let ids_before = all_ids(&mut s, &page);
    // Every column of the page's rows, not only text and depth: an undo that restored the
    // text but moved a block, flipped its kind or dropped a tombstone would still compare
    // equal on `(text, depth)` alone.
    let projection = |s: &mut Store| -> Vec<String> {
        let st = s.page_state(&page).unwrap();
        let mut out: Vec<String> = st
            .blocks
            .iter()
            .map(|(id, b)| {
                format!(
                    "{id}|{:?}|{}|{:?}|{}|{:?}|{:?}|{:?}",
                    b.parent, b.live, b.kind, b.checked, b.deleted_at, b.moved_to, b.text
                )
            })
            .collect();
        out.sort();
        out
    };
    let start_projection = projection(&mut s);

    // `yy` then `p`: a copy with new ids.
    let tree = outline::block_tree(s.page_state(&page).unwrap(), &a, 1).unwrap();
    let plan =
        outline::insert_tree_after(s.page_state(&page).unwrap(), &b, &tree, false, 1).unwrap();
    run(&mut s, &page, plan);
    doctor_clean(&mut s);

    // `dd` then `p`: the same ids move.
    let cut = outline::block_ids(s.page_state(&page).unwrap(), &a, 1).unwrap();
    let now = s.now();
    let plan =
        outline::delete_blocks(s.page_state(&page).unwrap(), &a, 1, now, None, &|_| false).unwrap();
    run(&mut s, &page, plan);
    let plan = outline::restore_after(s.page_state(&page).unwrap(), &cut, &c, false).unwrap();
    run(&mut s, &page, plan);
    doctor_clean(&mut s);

    // `J`, then `>>` and `<<`.
    let now = s.now();
    let plan =
        outline::join_next(s.page_state(&page).unwrap(), &c, 1, None, &|_| false, now).unwrap();
    run(&mut s, &page, plan);
    let plan = outline::indent(s.page_state(&page).unwrap(), &b).unwrap_or_else(|_| {
        outline::move_to(s.page_state(&page).unwrap(), &b, Some(&a), None).unwrap()
    });
    run(&mut s, &page, plan);
    doctor_clean(&mut s);

    // Everything that was there is still there, now that the moves are over.
    let ids_after = all_ids(&mut s, &page);
    for id in &ids_before {
        assert!(ids_after.contains(id), "id {id} left the graph");
    }
    assert_eq!(
        s.block_backlinks(&b).unwrap().len(),
        1,
        "the reference to the moved block still resolves"
    );
    // And undoing back to the start returns the page to where it started.
    let start = vec![
        ("a".to_string(), 0),
        ("a1".to_string(), 1),
        ("b".to_string(), 0),
        ("c".to_string(), 0),
    ];
    for _ in 0..200 {
        if rows(&mut s, &page) == start || !s.can_undo() {
            break;
        }
        s.undo().unwrap();
    }
    assert_eq!(
        rows(&mut s, &page),
        vec![
            ("a".into(), 0),
            ("a1".into(), 1),
            ("b".into(), 0),
            ("c".into(), 0)
        ],
        "a full undo restores the starting outline"
    );
    assert_eq!(
        projection(&mut s),
        start_projection,
        "every row — parent, kind, done state, tombstone, moved_to, text — is byte-identical \
         after the 200-undo round trip"
    );
    doctor_clean(&mut s);
}

/// Append `text` as the last child of `parent` (or the last root), so a test can build
/// children in the order it wants to assert on.
fn write_last(s: &mut Store, page: &PageId, parent: Option<&BlockId>, text: &str) -> BlockId {
    let after = s.page_state(page).unwrap().last_child(parent, false);
    let id = BlockId::new();
    s.execute(
        page,
        vec![Command::InsertBlock {
            id: id.clone(),
            parent: parent.cloned(),
            after,
            text: text.to_string(),
            kind: BlockKind::Bullet,
        }],
        Default::default(),
    )
    .unwrap();
    id
}

/// `dd` in the store: the plan from `block_ids`/`delete_blocks`, run as one step. Returns the
/// ids the cut took, which is what the register's `cut` holds.
fn delete(s: &mut Store, page: &PageId, block: &BlockId) -> Vec<BlockId> {
    let cut = outline::block_ids(s.page_state(page).unwrap(), block, 1).unwrap();
    let now = s.now();
    let plan = outline::delete_blocks(s.page_state(page).unwrap(), block, 1, now, None, &|_| false)
        .unwrap();
    run(s, page, plan);
    cut
}

/// VM-10, F1: a child that comes after a TOMBSTONE keeps its place across a cross-page
/// paste. The cut is the live subtree, so the tombstone between two live children is not in
/// it; anchoring the second child on that tombstone left `InsertBlock` with an anchor the
/// destination does not have, and the child came back FIRST (c, a).
#[test]
fn vm_10_a_cross_page_paste_keeps_the_order_around_a_tombstone() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let parent = write(&mut s, &src, "parent");
    let a = write_last(&mut s, &src, Some(&parent), "a");
    let b = write_last(&mut s, &src, Some(&parent), "b");
    let c = write_last(&mut s, &src, Some(&parent), "c");
    // Enter then Backspace leaves exactly this: a tombstone between two live children.
    let now = s.now();
    s.execute(
        &src,
        vec![Command::SetDeleted {
            ids: vec![b.clone()],
            deleted_at: Some(now),
        }],
        Default::default(),
    )
    .unwrap();
    let dst = s.create_page("Destination").unwrap();
    let here = write(&mut s, &dst, "here");

    let cut = outline::block_ids(s.page_state(&src).unwrap(), &parent, 1).unwrap();
    assert_eq!(
        cut,
        vec![parent.clone(), a.clone(), c.clone()],
        "b is not in the cut"
    );
    let cut = delete(&mut s, &src, &parent);
    s.restore_and_move_to_page(&src, &cut, &dst, None, Some(&here))
        .unwrap();

    assert_eq!(
        rows(&mut s, &dst),
        vec![
            ("here".into(), 0),
            ("parent".into(), 0),
            ("a".into(), 1),
            ("c".into(), 1),
        ],
        "the children kept the order they had on the page they came from (F1)"
    );
    doctor_clean(&mut s);
}

/// VM-10, F3: `dd` A on one page and `p` on another is a MOVE, and doing it back moves the
/// same block again. The page a block left keeps its row as `moved_to` history, so
/// `InsertBlock` refuses the id — the way back restores the destination's own row instead
/// (F3), and the text typed on that page before the paste is still there, because the
/// refusal path that used to drop the shard is never reached.
#[test]
fn vm_10_a_move_back_restores_the_destinations_own_row() {
    let mut s = store();
    let monday = s.create_page("Monday").unwrap();
    let a = write(&mut s, &monday, "alpha");
    let anchor = write(&mut s, &monday, "beta");
    let tuesday = s.create_page("Tuesday").unwrap();
    let here = write(&mut s, &tuesday, "here");
    let referrer = s.create_page("Referrer").unwrap();
    write(&mut s, &referrer, &format!("see (({a}))"));
    s.sync_index().unwrap();
    s.flush().unwrap();

    // `dd` A on Monday, `p` on Tuesday: the move, with the same id.
    let cut = delete(&mut s, &monday, &a);
    s.restore_and_move_to_page(&monday, &cut, &tuesday, None, Some(&here))
        .unwrap();
    assert_eq!(s.page_state(&tuesday).unwrap().parent_of(&a), None);

    // `dd` A on Tuesday, and the owner types on Monday without a flush.
    let cut = delete(&mut s, &tuesday, &a);
    s.set_debounced(true).unwrap();
    s.set_block_text(&monday, &anchor, "beta and more", None, None)
        .unwrap();

    // `p` on Monday: the way back, refused before it restores the destination's row (F3).
    let landed = s.restore_and_move_to_page(&tuesday, &cut, &monday, None, Some(&anchor));
    assert!(
        landed.is_ok(),
        "the way back is a move, not a refusal: {landed:?}"
    );
    assert_eq!(
        rows(&mut s, &monday),
        vec![("beta and more".into(), 0), ("alpha".into(), 0)],
        "alpha is back on Monday, live and after its anchor"
    );
    assert!(
        !deleted(&mut s, &monday, &a),
        "and its own row is live again"
    );
    assert!(
        s.page_state(&monday)
            .unwrap()
            .block(&a)
            .is_some_and(|b| b.moved_to.is_none()),
        "with no `moved_to` left behind"
    );
    assert_eq!(
        s.page_state(&tuesday).unwrap().block(&a).unwrap().moved_to,
        Some(monday.clone()),
        "Tuesday keeps its row as history"
    );
    assert_eq!(
        text(&mut s, &monday, &anchor),
        "beta and more",
        "the text typed before the paste was not dropped with a shard"
    );
    assert_eq!(
        s.block_backlinks(&a).unwrap().len(),
        1,
        "the ((reference)) still resolves (VM-10)"
    );
    s.flush().unwrap();
    doctor_clean(&mut s);
}

/// VM-10, F11: a cut block a drag has already taken out of the page is not restorable — its
/// row went with its parent — so the store refuses, and the owner copies instead of
/// resurrecting half of the subtree on the page it left.
#[test]
fn vm_10_a_cut_a_drag_took_away_is_refused() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    let parent = write(&mut s, &src, "parent");
    let child = write_last(&mut s, &src, Some(&parent), "child");
    let dst = s.create_page("Destination").unwrap();
    let here = write(&mut s, &dst, "here");

    let cut = delete(&mut s, &src, &child);
    let third = s.create_page("Third").unwrap();
    s.move_block_to_page(&src, &parent, &third, None, None)
        .unwrap();
    assert!(
        s.page_state(&src)
            .unwrap()
            .block(&child)
            .is_some_and(|b| b.moved_to.is_some()),
        "the drag took the tombstone's row with its parent"
    );

    let r = s.restore_and_move_to_page(&src, &cut, &dst, None, Some(&here));
    assert!(r.is_err(), "a block that left the page is not a cut: {r:?}");
    assert_eq!(
        rows(&mut s, &dst),
        vec![("here".into(), 0)],
        "and nothing was written"
    );
    doctor_clean(&mut s);
}

/// VM-10, F11: a live block a sync put under a tombstoned one is inside the cut's subtree, so
/// `p` on it cannot move the cut there (`MoveBlock` into its own subtree). The restore is
/// refused as a whole, before the first command, so no shard is dropped and the typing on
/// that page survives.
#[test]
fn vm_10_a_paste_under_a_cut_block_is_refused_and_drops_no_shard() {
    let mut s = store();
    let page = s.create_page("Page").unwrap();
    let a = write(&mut s, &page, "alpha");
    let keep = write(&mut s, &page, "keep");
    s.sync_index().unwrap();
    s.flush().unwrap();

    let cut = delete(&mut s, &page, &a);
    // What a sync can bring: a live child under the tombstone (§12.4).
    let arrived = write_last(&mut s, &page, Some(&a), "from another device");
    s.sync_index().unwrap();
    s.flush().unwrap();

    s.set_debounced(true).unwrap();
    s.set_block_text(&page, &keep, "keep and more", None, None)
        .unwrap();
    let r = (|| {
        let st = s.page_state(&page)?;
        let plan = outline::restore_after(st, &cut, &arrived, false)?;
        s.execute_plan(&page, plan, None)
    })();
    assert!(r.is_err(), "a paste under the cut cannot move it: {r:?}");
    assert_eq!(
        text(&mut s, &page, &keep),
        "keep and more",
        "the refused paste dropped no shard, so the typing is still there"
    );
    assert!(
        s.page_state(&page)
            .unwrap()
            .block(&a)
            .is_some_and(|b| b.deleted_at.is_some()),
        "and the cut is untouched"
    );
    doctor_clean(&mut s);
}

/// VM-10, F3: a cut that would pass FR-2.9's ceiling is refused before anything is written —
/// no shard is dropped, so the text typed on the page a moment before is still there.
#[test]
fn vm_10_a_cut_that_is_too_deep_writes_nothing_and_drops_no_shard() {
    let mut s = store();
    let src = s.create_page("Source").unwrap();
    // A chain deep enough that pasting it under a deep block cannot fit.
    let mut chain: Vec<BlockId> = Vec::new();
    let mut parent: Option<BlockId> = None;
    for d in 0..outline::MAX_DROP_DEPTH {
        let id = write_last(&mut s, &src, parent.as_ref(), &format!("d{d}"));
        parent = Some(id.clone());
        chain.push(id);
    }
    let root = chain[0].clone();
    let dst = s.create_page("Destination").unwrap();
    // A destination chain nearly as deep as the ceiling, so a subtree of eleven levels
    // cannot fit under its last block.
    let mut dchain: Vec<BlockId> = Vec::new();
    let mut parent: Option<BlockId> = None;
    for d in 0..(outline::MAX_DROP_DEPTH - 1) {
        let id = write_last(&mut s, &dst, parent.as_ref(), &format!("t{d}"));
        parent = Some(id.clone());
        dchain.push(id);
    }
    let deep = dchain.last().cloned().unwrap();
    let mid = dchain[dchain.len() - 2].clone();
    s.sync_index().unwrap();
    s.flush().unwrap();

    let cut = delete(&mut s, &src, &root);
    s.set_debounced(true).unwrap();
    s.set_block_text(&dst, &mid, "mid and more", None, None)
        .unwrap();
    let r = s.restore_and_move_to_page(&src, &cut, &dst, Some(&deep), None);
    assert!(
        matches!(r, Err(utopia_store::StoreError::Domain(_))),
        "too deep is refused before anything is written: {r:?}"
    );
    assert_eq!(
        text(&mut s, &dst, &mid),
        "mid and more",
        "the refused paste dropped no shard, so the typing is still there"
    );
    assert_eq!(
        s.page_state(&dst).unwrap().live_count(),
        outline::MAX_DROP_DEPTH - 1,
        "and nothing was pasted"
    );
    assert!(
        s.page_state(&src)
            .unwrap()
            .block(&root)
            .is_some_and(|b| b.deleted_at.is_some()),
        "the cut is still a cut, so a later `p` somewhere shallower moves it"
    );
    doctor_clean(&mut s);
}
