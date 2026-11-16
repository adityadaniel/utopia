//! `Op::MoveBlock` and `Op::DeleteBlock` (PRD §13.8 MC-8, MC-9; `docs/mcp-design.md` §4),
//! on a real store: the undo stack, the projections and `doctor` are the ones the app and
//! the MCP server use.
//!
//! The ops are built from JSON where the wire shape matters, as `/v1/op` and a plugin send
//! them.

use serde_json::{json, Value};
use utopia_api::{handle, ApiError, Op, Outcome};
use utopia_core::{BlockId, PageId};
use utopia_store::Store;

fn store() -> Store {
    let mut s = Store::open_in_memory().expect("an in-memory store");
    s.create_page("Project X").expect("a page");
    s.create_page("Reading").expect("a page");
    s
}

fn op(body: Value) -> Op {
    serde_json::from_value(body).expect("an op")
}

fn call(s: &mut Store, body: Value) -> Result<Outcome, ApiError> {
    handle(s, op(body))
}

/// Append one block on `title` under `parent` and give its id back.
fn append(s: &mut Store, title: &str, text: &str, parent: Option<&str>) -> String {
    handle(
        s,
        Op::AppendBlock {
            title: title.into(),
            text: text.into(),
            parent: parent.map(str::to_string),
            create: false,
        },
    )
    .expect("append a block")
    .body["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

fn page(s: &mut Store, title: &str) -> PageId {
    s.find_page(title).expect("a lookup").expect("the page")
}

/// One page's `(text, depth)` rows, in document order.
fn rows(s: &mut Store, title: &str) -> Vec<(String, usize)> {
    let page = page(s, title);
    let st = s.page_state(&page).expect("the page state").clone();
    st.live_preorder()
        .into_iter()
        .map(|(id, d)| (st.block(&id).expect("a live row").text.clone(), d))
        .collect()
}

/// Whether the block is live on the page its row names: what a `((reference))` reads to
/// show the block or "deleted block" (`rich::ChipState::Deleted`, `page_view`'s embeds).
fn live(s: &mut Store, id: &str) -> bool {
    let id = BlockId(id.to_string());
    let Some(row) = s.block_row(&id).expect("a block lookup") else {
        return false;
    };
    s.page_state(&row.page_id)
        .expect("the page state")
        .is_live(&id)
}

/// A chain of `n` blocks, each under the one before it, on a page of its own. The ids come
/// back root first.
fn chain(s: &mut Store, title: &str, n: usize) -> Vec<String> {
    s.create_page(title).expect("a page");
    let mut ids: Vec<String> = Vec::new();
    for i in 0..n {
        let id = append(
            s,
            title,
            &format!("level {i}"),
            ids.last().map(String::as_str),
        );
        ids.push(id);
    }
    ids
}

/// MC-8: a move into another page carries the whole subtree, in shape, and the blocks keep
/// their ids. `doctor` is clean afterwards (rule 2), so the projections agree with a rebuild.
#[test]
fn mc_8_a_move_to_another_page_keeps_the_subtree() {
    let mut s = store();
    let root = append(&mut s, "Project X", "root", None);
    let a = append(&mut s, "Project X", "kid a", Some(&root));
    let b = append(&mut s, "Project X", "kid b", Some(&root));
    let grand = append(&mut s, "Project X", "grand", Some(&b));
    append(&mut s, "Reading", "already here", None);

    let out = call(
        &mut s,
        json!({"op": "move_block", "id": root, "page": "Reading"}),
    )
    .expect("the move");

    assert_eq!(out.body["id"], json!(root));
    assert_eq!(out.body["title"], json!("Reading"));
    assert_eq!(
        out.body["blocks"],
        json!(4),
        "the block and the three under it"
    );
    assert_eq!(
        rows(&mut s, "Reading"),
        vec![
            ("root".to_string(), 0),
            ("kid a".to_string(), 1),
            ("kid b".to_string(), 1),
            ("grand".to_string(), 2),
            ("already here".to_string(), 0),
        ],
        "the subtree keeps its shape, and lands ahead of what was there"
    );
    assert_eq!(
        rows(&mut s, "Project X"),
        Vec::<(String, usize)>::new(),
        "the source page lost the subtree"
    );
    // The same ids, on the other page.
    for id in [&root, &a, &b, &grand] {
        assert_eq!(
            s.block_row(&BlockId(id.clone()))
                .expect("a block lookup")
                .expect("the row")
                .page_id,
            page(&mut s, "Reading")
        );
    }
    assert!(out.changed.contains(&page(&mut s, "Reading")));
    assert!(out.changed.contains(&page(&mut s, "Project X")));
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8, rule 5's neighbour: a `((reference))` to a moved block still resolves, because the
/// move keeps the id. The reference is written before the move and read after it.
#[test]
fn mc_8_a_reference_to_a_moved_block_still_resolves() {
    let mut s = store();
    let target = append(&mut s, "Project X", "the moved words", None);
    append(&mut s, "Reading", &format!("see (({target})) here"), None);

    let before = call(&mut s, json!({"op": "get_block", "id": target}))
        .expect("the block before the move")
        .body;
    assert_eq!(before["text"], json!("the moved words"));

    call(
        &mut s,
        json!({"op": "move_block", "id": target, "page": "Reading"}),
    )
    .expect("the move");

    let after = call(&mut s, json!({"op": "get_block", "id": target}))
        .expect("the block after the move: the id still resolves")
        .body;
    assert_eq!(after["text"], json!("the moved words"));
    assert_eq!(after["page"], json!(page(&mut s, "Reading").as_str()));
    assert!(live(&mut s, &target), "and it is live where it landed");

    // The referring block was not touched: the move rewrote the tree, not the text.
    let reading = call(&mut s, json!({"op": "get_page", "title": "Reading"}))
        .expect("the page")
        .body;
    let texts: Vec<&str> = reading["blocks"]
        .as_array()
        .expect("the page's blocks")
        .iter()
        .filter_map(|b| b["text"].as_str())
        .collect();
    assert!(
        texts.contains(&format!("see (({target})) here").as_str()),
        "the referring block is untouched: {texts:?}"
    );
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8, rule 5: a delete is a tombstone. The block keeps its row and its id, so the id a
/// reference names still resolves and reads as deleted, and one undo brings the subtree back
/// (MC-9). `doctor` is clean on both sides.
#[test]
fn mc_8_a_delete_makes_a_tombstone_and_a_reference_reads_deleted() {
    let mut s = store();
    let gone = append(&mut s, "Project X", "gone", None);
    let child = append(&mut s, "Project X", "gone too", Some(&gone));
    append(&mut s, "Reading", &format!("(({gone}))"), None);
    let before_depth = s.undo_depth();

    let out = call(&mut s, json!({"op": "delete_block", "id": gone})).expect("the delete");
    assert_eq!(out.body["blocks"], json!(2), "the block and its child");
    assert_eq!(out.body["id"], json!(gone));
    assert_eq!(out.body["title"], json!("Project X"));

    assert_eq!(out.changed, vec![page(&mut s, "Project X")]);
    assert_eq!(
        rows(&mut s, "Project X"),
        Vec::<(String, usize)>::new(),
        "nothing live is left"
    );
    // Rule 5: the row and the id stay, so the reference still resolves and reads deleted.
    assert!(
        !live(&mut s, &gone),
        "the reference reads \"deleted block\""
    );
    assert!(!live(&mut s, &child), "and so does the subtree");
    assert!(
        s.block_row(&BlockId(gone.clone()))
            .expect("a block lookup")
            .is_some(),
        "the id is still a row: a reference to it is not \"missing\""
    );
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");

    // MC-9: one undo restores the whole subtree, ids and all.
    s.undo().expect("an undo");
    assert_eq!(
        rows(&mut s, "Project X"),
        vec![("gone".to_string(), 0), ("gone too".to_string(), 1)]
    );
    assert!(live(&mut s, &gone), "live again");
    assert!(live(&mut s, &child));
    assert_eq!(s.undo_depth(), before_depth, "one group, reversed");
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor after the undo: {report:?}");
}

/// MC-8, MC-9: a move is one step on the undo stack, and one undo puts the subtree back on
/// its own page exactly as it was.
#[test]
fn mc_9_one_undo_reverses_a_move() {
    let mut s = store();
    let root = append(&mut s, "Project X", "root", None);
    let kid = append(&mut s, "Project X", "kid", Some(&root));
    append(&mut s, "Project X", "after the subtree", Some(&root));
    let before_project = rows(&mut s, "Project X");
    let before_reading = rows(&mut s, "Reading");
    let depth = s.undo_depth();

    call(
        &mut s,
        json!({"op": "move_block", "id": root, "page": "Reading"}),
    )
    .expect("the move");
    assert_eq!(s.undo_depth(), depth + 1, "one group for the whole move");

    s.undo().expect("an undo");
    assert_eq!(rows(&mut s, "Project X"), before_project, "back in place");
    assert_eq!(
        rows(&mut s, "Reading"),
        before_reading,
        "and gone from here"
    );
    assert!(live(&mut s, &root) && live(&mut s, &kid));
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8, FR-2.9: a move into the block's own subtree is refused, and nothing is written —
/// not even the positions that were valid in the same call.
#[test]
fn mc_8_a_move_into_its_own_subtree_is_refused() {
    let mut s = store();
    let root = append(&mut s, "Project X", "root", None);
    let kid = append(&mut s, "Project X", "kid", Some(&root));
    let grand = append(&mut s, "Project X", "grand", Some(&kid));
    let before = rows(&mut s, "Project X");
    let depth = s.undo_depth();

    for body in [
        json!({"op": "move_block", "id": root, "parent": grand}),
        json!({"op": "move_block", "id": root, "parent": kid}),
        // `after` names the level too, so a target inside the subtree is the same cycle.
        json!({"op": "move_block", "id": root, "after": grand}),
        json!({"op": "move_block", "id": kid, "page": "Project X", "parent": grand}),
    ] {
        let err = call(&mut s, body.clone()).expect_err("a cycle is refused");
        assert_eq!(err.status, 400, "{body}: {}", err.message);
        assert!(
            err.message.contains("own subtree"),
            "{body}: {}",
            err.message
        );
    }
    assert_eq!(rows(&mut s, "Project X"), before, "the tree is untouched");
    assert_eq!(s.undo_depth(), depth, "nothing reached the undo stack");
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8, FR-2.9: a move that would push a descendant past the depth ceiling fails, on the
/// same page and into another one, and writes nothing.
#[test]
fn mc_8_a_move_below_the_depth_ceiling_is_refused() {
    let mut s = store();
    // Twelve levels, the deepest FR-2.9 allows. Moving the last one under any of them would
    // make it the thirteenth.
    let deep = chain(&mut s, "Deep", utopia_core::outline::MAX_DROP_DEPTH - 1);
    let mover = append(&mut s, "Project X", "mover", None);
    append(&mut s, "Project X", "mover's child", Some(&mover));
    // The same shape on the deep page, for the move that stays there.
    let local = append(&mut s, "Deep", "local mover", None);
    append(&mut s, "Deep", "local mover's child", Some(&local));
    let deepest = deep.last().expect("a deepest block");
    let before = rows(&mut s, "Deep");

    for body in [
        json!({"op": "move_block", "id": mover, "page": "Deep", "parent": deepest}),
        json!({"op": "move_block", "id": local, "page": "Deep", "parent": deepest}),
        json!({"op": "move_block", "id": local, "parent": deepest}),
    ] {
        let err = call(&mut s, body.clone()).expect_err("too deep is refused");
        assert_eq!(err.status, 400, "{body}: {}", err.message);
        assert!(
            err.message.to_lowercase().contains("depth"),
            "{body}: {}",
            err.message
        );
    }
    assert_eq!(rows(&mut s, "Deep"), before, "the chain is untouched");
    assert_eq!(
        rows(&mut s, "Project X"),
        vec![("mover".to_string(), 0), ("mover's child".to_string(), 1)],
        "and so is the block that was to move"
    );
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8: a move within the page reorders there, and a move after a block on another page
/// lands behind it — the two ends of `parent`/`after` that `outline::move_to` and
/// `Store::move_block_to_page` take.
#[test]
fn mc_8_a_move_places_the_block_where_it_is_asked_to() {
    let mut s = store();
    let a = append(&mut s, "Project X", "a", None);
    let b = append(&mut s, "Project X", "b", None);
    let c = append(&mut s, "Project X", "c", None);

    // Same page: `c` goes behind `a`, on `a`'s level (derived from `after`).
    call(&mut s, json!({"op": "move_block", "id": c, "after": a})).expect("the move");
    assert_eq!(
        rows(&mut s, "Project X"),
        vec![
            ("a".to_string(), 0),
            ("c".to_string(), 0),
            ("b".to_string(), 0)
        ]
    );

    // Into another page, under a block that is there, behind the last of its children.
    let host = append(&mut s, "Reading", "host", None);
    let host_kid = append(&mut s, "Reading", "host kid", Some(&host));
    call(
        &mut s,
        json!({"op": "move_block", "id": b, "page": "Reading", "parent": host, "after": host_kid}),
    )
    .expect("the move");
    assert_eq!(
        rows(&mut s, "Reading"),
        vec![
            ("host".to_string(), 0),
            ("host kid".to_string(), 1),
            ("b".to_string(), 1)
        ]
    );
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8: without `parent` or `after` the block becomes the first block of the target page,
/// as `outline::move_to` and `Store::move_block_to_page` place it; the answer says where it
/// landed, so an agent that wanted the end of the page learns where to send it.
#[test]
fn mc_8_a_move_without_a_position_goes_first() {
    let mut s = store();
    append(&mut s, "Reading", "already here", None);
    let moved = append(&mut s, "Project X", "moved", None);
    let out = call(
        &mut s,
        json!({"op": "move_block", "id": moved, "page": "Reading"}),
    )
    .expect("the move");
    assert_eq!(out.body["parent"], Value::Null);
    assert_eq!(out.body["after"], Value::Null);
    assert_eq!(
        rows(&mut s, "Reading"),
        vec![("moved".to_string(), 0), ("already here".to_string(), 0)]
    );
}

/// MC-8: a position that is not a place on the target page is refused with no write, and a
/// `parent` and an `after` that are not parent and child are refused rather than guessed at.
#[test]
fn mc_8_a_position_that_does_not_exist_is_refused() {
    let mut s = store();
    let root = append(&mut s, "Project X", "root", None);
    let sibling = append(&mut s, "Project X", "sibling", None);
    let kid = append(&mut s, "Project X", "kid", Some(&root));
    let mover = append(&mut s, "Project X", "mover", None);
    let before = rows(&mut s, "Project X");
    let depth = s.undo_depth();
    let missing = "01J8Z3K9V2AAAAAAAAAAAAAAAA";

    for (body, status, want) in [
        (
            json!({"op": "move_block", "id": mover, "parent": missing}),
            404,
            "no block",
        ),
        (
            json!({"op": "move_block", "id": mover, "after": missing}),
            404,
            "no block",
        ),
        (
            // `kid` is not a child of `sibling`, so this is not one place.
            json!({"op": "move_block", "id": mover, "parent": sibling, "after": kid}),
            400,
            "not a child",
        ),
        (
            // A block that is not there cannot be moved.
            json!({"op": "move_block", "id": missing, "page": "Reading"}),
            404,
            "no block",
        ),
        (
            json!({"op": "move_block", "id": mover, "page": "No such page"}),
            404,
            "no page titled",
        ),
        (
            json!({"op": "delete_block", "id": missing}),
            404,
            "no block",
        ),
    ] {
        let err = call(&mut s, body.clone()).expect_err("refused");
        assert_eq!(err.status, status, "{body}: {}", err.message);
        assert!(
            err.message.contains(want),
            "{body}: {} in {want}",
            err.message
        );
    }
    assert_eq!(rows(&mut s, "Project X"), before);
    assert_eq!(s.undo_depth(), depth);
    let report = s.doctor().expect("doctor runs");
    assert!(report.is_ok(), "doctor: {report:?}");
}

/// MC-8: a block that is already a tombstone cannot be deleted again or moved, and the call
/// says so instead of writing a second tombstone.
#[test]
fn mc_8_a_deleted_block_cannot_be_deleted_or_moved_again() {
    let mut s = store();
    let gone = append(&mut s, "Project X", "gone", None);
    call(&mut s, json!({"op": "delete_block", "id": gone})).expect("the delete");
    let depth = s.undo_depth();
    for body in [
        json!({"op": "delete_block", "id": gone}),
        json!({"op": "move_block", "id": gone, "page": "Reading"}),
        json!({"op": "move_block", "id": gone}),
    ] {
        let err = call(&mut s, body.clone()).expect_err("nothing to act on");
        assert_eq!(err.status, 404, "{body}: {}", err.message);
    }
    assert_eq!(s.undo_depth(), depth, "nothing was written");
    assert!(!live(&mut s, &gone));
}
