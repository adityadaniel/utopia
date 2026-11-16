//! `Op::UpdateBlock` with `kind`, `checked` and `if_rev` (PRD §13.8 MC-8, MC-9, MC-10;
//! `docs/mcp-design.md` §4). These drive `handle` on a real store, so the undo stack,
//! the projections and `doctor` are the ones the app and the MCP server use.
//!
//! The op is built from JSON, as `/v1/op` and a plugin send it, so the wire shape is under
//! test in each call and not only in the serialization table.

use serde_json::{json, Value};
use utopia_api::{block_rev, handle, ApiError, Op, Outcome};
use utopia_core::{BlockId, BlockKind, PageId};
use utopia_store::Store;

fn store() -> Store {
    let mut s = Store::open_in_memory().expect("an in-memory store");
    s.create_page("Project X").expect("a page");
    s
}

/// The op as `/v1/op` and a plugin send it: one JSON object, absent parts and all.
fn op(body: Value) -> Op {
    serde_json::from_value(body).expect("an op")
}

fn update(s: &mut Store, body: Value) -> Result<Outcome, ApiError> {
    handle(s, op(body))
}

/// A block on "Project X", made through the API as an agent would.
fn block(s: &mut Store, text: &str) -> String {
    handle(
        s,
        Op::AppendBlock {
            title: "Project X".into(),
            text: text.into(),
            parent: None,
            create: false,
        },
    )
    .expect("append a block")
    .body["id"]
        .as_str()
        .expect("an id")
        .to_string()
}

/// The page, its block's text, kind and checked state.
fn fields(s: &mut Store, id: &str) -> (String, BlockKind, bool) {
    let page: PageId = s
        .find_page("Project X")
        .expect("a lookup")
        .expect("the page");
    let st = s.page_state(&page).expect("the page state");
    let b = st.block(&BlockId(id.to_string())).expect("the block");
    (b.text.clone(), b.kind, b.checked)
}

/// MC-10, §2.3.7: a rev that does not agree with the block fails and writes nothing —
/// not the text, not the kind, not the checked state, and no undo step either.
#[test]
fn mc_10_a_rev_that_does_not_agree_fails_and_writes_nothing() {
    let mut s = store();
    let id = block(&mut s, "shipping notes");
    let depth = s.undo_depth();
    let err = update(
        &mut s,
        json!({
            "op": "update_block", "id": id,
            "text": "shipped", "kind": "todo", "checked": true,
            "if_rev": "000000000000",
        }),
    )
    .expect_err("a stale rev fails");
    assert_eq!(err.status, 409, "{}", err.message);
    assert!(
        err.message.contains("read_block"),
        "the error says to read the block again: {}",
        err.message
    );
    assert_eq!(
        fields(&mut s, &id),
        ("shipping notes".to_string(), BlockKind::Bullet, false)
    );
    assert_eq!(s.undo_depth(), depth, "nothing reached the undo stack");
    assert!(s.doctor().expect("doctor runs").is_ok());
}

/// MC-10: a rev that agrees writes, and the caller can read the new one back: the hash is
/// the one `read_block` returns for the new text.
#[test]
fn mc_10_a_rev_that_agrees_writes() {
    let mut s = store();
    let id = block(&mut s, "shipping notes");
    let rev = block_rev("shipping notes");
    // A rev pasted back in another case is the same rev; only a different text fails.
    let body = update(
        &mut s,
        json!({"op": "update_block", "id": id, "text": "shipped", "if_rev": rev.to_uppercase()}),
    )
    .expect("an agreeing rev writes")
    .body;
    assert_eq!(body["text"], "shipped");
    assert_eq!(fields(&mut s, &id).0, "shipped");
    assert_eq!(block_rev(&fields(&mut s, &id).0), block_rev("shipped"));

    // The new rev is the one a second call must send; the old one no longer agrees.
    let err = update(
        &mut s,
        json!({"op": "update_block", "id": id, "text": "shipped again", "if_rev": rev}),
    )
    .expect_err("the old rev is stale now");
    assert_eq!(err.status, 409);
    assert_eq!(fields(&mut s, &id).0, "shipped", "nothing was written");
}

/// MC-8, FR-2.13: one call sets the kind and the checked state of a to-do.
#[test]
fn mc_8_kind_and_checked_change_in_one_call() {
    let mut s = store();
    let id = block(&mut s, "buy milk");
    update(
        &mut s,
        json!({"op": "update_block", "id": id, "kind": "todo", "checked": true}),
    )
    .expect("a to-do, ticked");
    assert_eq!(fields(&mut s, &id).1, BlockKind::Todo);
    assert!(fields(&mut s, &id).2);

    // Ticking and unticking without repeating the kind.
    update(
        &mut s,
        json!({"op": "update_block", "id": id, "checked": false}),
    )
    .expect("unticked");
    assert_eq!(fields(&mut s, &id).1, BlockKind::Todo, "still a to-do");
    assert!(!fields(&mut s, &id).2);

    // A block that stops being a to-do is not left ticked.
    update(
        &mut s,
        json!({"op": "update_block", "id": id, "checked": true}),
    )
    .expect("ticked again");
    update(
        &mut s,
        json!({"op": "update_block", "id": id, "kind": "bullet"}),
    )
    .expect("a bullet");
    assert_eq!(fields(&mut s, &id).1, BlockKind::Bullet);
    assert!(!fields(&mut s, &id).2, "a bullet cannot be done");
    assert!(s.doctor().expect("doctor runs").is_ok());
}

/// MC-8: `kind` sets every kind, and `heading` keeps the level of a block that already is
/// one while `h1`, `h2` and `h3` pick it.
#[test]
fn mc_8_a_heading_keeps_its_level() {
    let mut s = store();
    let id = block(&mut s, "a heading");
    for (sent, want) in [
        ("heading", BlockKind::Heading(1)),
        ("h2", BlockKind::Heading(2)),
        ("heading", BlockKind::Heading(2)),
        ("h3", BlockKind::Heading(3)),
        ("quote", BlockKind::Quote),
        ("code", BlockKind::Code),
        ("numbered", BlockKind::Numbered),
    ] {
        update(
            &mut s,
            json!({"op": "update_block", "id": id, "kind": sent}),
        )
        .unwrap_or_else(|e| panic!("kind {sent:?}: {}", e.message));
        assert_eq!(fields(&mut s, &id).1, want, "kind {sent:?}");
    }
}

/// MC-8, MC-9: the text, the kind and `checked` of one call are one step on the undo
/// stack, and one Cmd+Z takes all three back (FR-2.12).
#[test]
fn mc_9_one_undo_reverses_the_whole_call() {
    let mut s = store();
    let id = block(&mut s, "draft the brief");
    let before = fields(&mut s, &id);
    // The editor's own typing is its own step; the agent's call must not merge into it.
    s.set_block_text(
        &s.find_page("Project X").unwrap().unwrap(),
        &BlockId(id.clone()),
        "draft the brief",
        None,
        None,
    )
    .expect("the owner's edit");
    let depth = s.undo_depth();
    update(
        &mut s,
        json!({
            "op": "update_block", "id": id,
            "text": "send the brief", "kind": "todo", "checked": true,
        }),
    )
    .expect("the full change");
    assert_eq!(s.undo_depth(), depth + 1, "one call is one undo step");
    assert_eq!(
        fields(&mut s, &id),
        ("send the brief".to_string(), BlockKind::Todo, true)
    );
    assert!(s.doctor().expect("doctor runs").is_ok());

    // A group with no caret answers `None`; the undo itself still happened.
    s.undo().expect("undo runs");
    assert_eq!(fields(&mut s, &id), before, "one undo reverses the call");
    assert_eq!(s.undo_depth(), depth);
    assert!(s.doctor().expect("doctor runs").is_ok());
}

/// MC-8, W3/FR-1.10: a `[[link]]` in the new text makes its page, as the same text typed
/// into the block does.
#[test]
fn mc_8_a_link_in_the_new_text_makes_its_page() {
    let mut s = store();
    let id = block(&mut s, "ask for the copy");
    assert!(s.find_page("Ana").expect("a lookup").is_none());
    update(
        &mut s,
        json!({"op": "update_block", "id": id, "text": "ask [[Ana]] for the copy"}),
    )
    .expect("a link");
    assert!(
        s.find_page("Ana").expect("a lookup").is_some(),
        "the link made its page"
    );
    assert!(s.doctor().expect("doctor runs").is_ok());
}

/// MC-8: a kind that is not one of the nine is refused and writes nothing. Reading it as a
/// bullet (which `BlockKind::from_parts` does with an unknown string) would change a block
/// to something the caller did not ask for.
#[test]
fn mc_8_an_unknown_kind_is_refused() {
    let mut s = store();
    let id = block(&mut s, "buy milk");
    let err = update(
        &mut s,
        json!({"op": "update_block", "id": id, "text": "buy oat milk", "kind": "toodo"}),
    )
    .expect_err("an unknown kind fails");
    assert_eq!(err.status, 400, "{}", err.message);
    assert!(err.message.contains("toodo"), "{}", err.message);
    assert_eq!(
        fields(&mut s, &id),
        ("buy milk".to_string(), BlockKind::Bullet, false)
    );
}

/// MC-8: `checked` is a to-do's state. A call that would check a bullet is refused and
/// says how to make the block a to-do in the same call.
#[test]
fn mc_8_checked_needs_a_to_do() {
    let mut s = store();
    let id = block(&mut s, "buy milk");
    let err = update(
        &mut s,
        json!({"op": "update_block", "id": id, "checked": true}),
    )
    .expect_err("a bullet cannot be checked");
    assert_eq!(err.status, 400, "{}", err.message);
    assert!(err.message.contains("todo"), "{}", err.message);
    assert!(!fields(&mut s, &id).2, "nothing was ticked");
}

/// MC-8: an op that names no change is refused, so a call that lost its fields on the way
/// does not answer as if it had written something.
#[test]
fn mc_8_a_call_with_nothing_to_change_is_refused() {
    let mut s = store();
    let id = block(&mut s, "buy milk");
    let err =
        update(&mut s, json!({"op": "update_block", "id": id})).expect_err("nothing to change");
    assert_eq!(err.status, 400, "{}", err.message);
    // A block that does not exist is 404, as it is for every other op.
    let err = update(
        &mut s,
        json!({"op": "update_block", "id": "01J8Z3K9V2AAAAAAAAAAAAAAAA", "text": "x"}),
    )
    .expect_err("no such block");
    assert_eq!(err.status, 404, "{}", err.message);
}

/// MC-8: the JSON that plugins and HTTP clients send today still parses, and the parts it
/// leaves out stay unset — `{"op": "update_block", "id": …, "text": …}` keeps its meaning.
#[test]
fn mc_8_the_old_json_still_parses() {
    let old = op(json!({"op": "update_block", "id": "b1", "text": "t"}));
    assert_eq!(
        old,
        Op::UpdateBlock {
            id: "b1".into(),
            text: Some("t".into()),
            kind: None,
            checked: None,
            if_rev: None,
        }
    );
    // And it round-trips to the shape it read, with no nulls the old readers do not know.
    assert_eq!(
        serde_json::to_value(&old).expect("it serializes"),
        json!({"op": "update_block", "id": "b1", "text": "t"})
    );

    // The text alone still writes, through the op and through the old REST shape.
    let mut s = store();
    let id = block(&mut s, "one");
    update(
        &mut s,
        json!({"op": "update_block", "id": id, "text": "two"}),
    )
    .expect("a text-only call");
    assert_eq!(fields(&mut s, &id).0, "two");
}
