//! Vim mode under the real workspace (docs/vim-mode-plan.md §1G): VM-1 to VM-7, VM-13
//! and VM-14, plus the seeded random session of rule 11.
//!
//! The harness is the one in `tests/workspace.rs`, copied: a real store, a real window and
//! the real [`Workspace`], with Vim turned on before the window is built the way a launch
//! reads `settings.json`. Every assertion is on store state, `Store::undo_depth`, the
//! editor's mode, or its target and cursor. None is on a missing `debug_bounds`: GPUI's
//! test frames never clear debug bounds, so "it stopped being drawn" is not observable
//! that way (AGENTS.md).

use chrono::NaiveDate;
use gpui::{
    px, AppContext as _, Entity, EntityInputHandler as _, Modifiers, TestAppContext,
    VisualTestContext,
};
use utopia_core::text::next_grapheme;
use utopia_core::undo::Caret;
use utopia_core::vim::motion;
use utopia_core::{BlockId, BlockKind, Command, PageId};
use utopia_store::Store;
use utopia_ui::actions::install_keymap;
use utopia_ui::app_state::AppState;
use utopia_ui::editor::BlockEditor;
use utopia_ui::theme::Theme;
use utopia_ui::vim::{Mode, Register, Resume};
use utopia_ui::workspace::Workspace;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
}

/// A workspace on an in-memory graph with the Daily Notes feed open. `seed` writes the
/// graph through the store before the window exists, so the first frame already shows it
/// (a scripted keystroke would race the UI and read worse in a screenshot).
fn setup_seeded(
    cx: &mut TestAppContext,
    vim: bool,
    seed: impl FnOnce(&mut Store),
) -> (Entity<AppState>, Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        if vim {
            // VM-1: the preference is `settings.json`'s and is read at launch. The empty
            // directory keeps it in memory, so a test never writes a real config dir.
            utopia_ui::appearance::set_vim(cx, true, std::path::Path::new("")).unwrap();
        }
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today()));
    seed(&mut store);
    let app = cx.new(|_| AppState::new(store));
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    (app, ws, cx)
}

fn setup(
    cx: &mut TestAppContext,
    vim: bool,
) -> (Entity<AppState>, Entity<Workspace>, &mut VisualTestContext) {
    setup_seeded(cx, vim, |_| {})
}

/// Today's page as shown: live once written, a draft before that (FR-1.2).
fn today_page(app: &Entity<AppState>, cx: &mut VisualTestContext) -> PageId {
    app.update(cx, |app, _| app.store.draft_daily(today()).unwrap())
}

/// Live rows of a page as `(text, depth)`.
fn rows(app: &Entity<AppState>, page: &PageId, cx: &mut VisualTestContext) -> Vec<(String, usize)> {
    app.update(cx, |app, _| {
        let st = app.store.page_state(page).unwrap();
        st.live_preorder()
            .into_iter()
            .map(|(id, d)| (st.block(&id).unwrap().text.clone(), d))
            .collect()
    })
}

fn live_blocks(app: &Entity<AppState>, page: &PageId, cx: &mut VisualTestContext) -> Vec<BlockId> {
    app.update(cx, |app, _| {
        app.store
            .page_state(page)
            .unwrap()
            .live_preorder()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    })
}

fn undo_depth(app: &Entity<AppState>, cx: &mut VisualTestContext) -> usize {
    app.update(cx, |app, _| app.store.undo_depth())
}

/// Today's page with one root block per entry, written through the store.
fn seed_roots(store: &mut Store, texts: &[&str]) -> (PageId, Vec<BlockId>) {
    let (page, first) = store.open_daily(today()).unwrap();
    store
        .set_block_text(&page, &first, texts[0], None, None)
        .unwrap();
    store.commit_block(&page, &first).unwrap();
    let mut ids = vec![first.clone()];
    let mut after = first;
    for text in &texts[1..] {
        let id = BlockId::new();
        store
            .execute(
                &page,
                vec![Command::InsertBlock {
                    id: id.clone(),
                    parent: None,
                    after: Some(after.clone()),
                    text: (*text).to_string(),
                    kind: BlockKind::Bullet,
                }],
                Default::default(),
            )
            .unwrap();
        after = id.clone();
        ids.push(id);
    }
    (page, ids)
}

/// A named page with one root block per entry, written through the store (a sidebar page,
/// which cannot be the launch view).
fn seed_page(store: &mut Store, title: &str, texts: &[&str]) -> (PageId, Vec<BlockId>) {
    let page = store.create_page(title).unwrap();
    let mut ids = Vec::new();
    let mut after: Option<BlockId> = None;
    for text in texts {
        let id = BlockId::new();
        store
            .execute(
                &page,
                vec![Command::InsertBlock {
                    id: id.clone(),
                    parent: None,
                    after: after.clone(),
                    text: (*text).to_string(),
                    kind: BlockKind::Bullet,
                }],
                Default::default(),
            )
            .unwrap();
        after = Some(id.clone());
        ids.push(id);
    }
    (page, ids)
}

fn editor(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Entity<BlockEditor> {
    ws.update(cx, |w, _| w.editor())
}

fn mode(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Mode {
    ws.update(cx, |w, cx| w.editor().read(cx).vim().mode)
}

fn pending(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
    ws.update(cx, |w, cx| w.editor().read(cx).vim().parser.pending())
}

fn cursor(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> usize {
    ws.update(cx, |w, cx| w.editor().read(cx).cursor())
}

fn field_text(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
    ws.update(cx, |w, cx| w.editor().read(cx).field_text().to_string())
}

/// The block the caret is in, if the editor is editing one.
fn target_block(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<BlockId> {
    ws.update(cx, |w, cx| {
        w.editor().read(cx).target().map(|t| t.block.clone())
    })
}

/// Where the input method's composition is, in UTF-16 offsets: `Some` while one is live.
fn marked(
    editor: &Entity<BlockEditor>,
    cx: &mut VisualTestContext,
) -> Option<std::ops::Range<usize>> {
    editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx))
}

/// VM-2: with Vim off nothing about typing changed — a letter lands in the block and the
/// caret keys still move it, so the interceptor is not quietly eating keystrokes.
#[gpui::test]
fn vm_2_with_vim_off_a_letter_still_types(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx, false);
    let page = today_page(&app, cx);
    cx.simulate_input("abc");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "abc");
    cx.simulate_keystrokes("left left");
    cx.simulate_input("X");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "aXbc",
        "the arrow keys and the input handler are today's (VM-2)"
    );
}

/// VM-4: a block in edit is in Normal at entry. It is Normal at launch, and it is Normal
/// again when a click enters it.
#[gpui::test]
fn vm_4_a_block_in_edit_starts_in_normal_mode(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let block = live_blocks(&app, &page, cx)[0].clone();
    assert_eq!(
        target_block(&ws, cx),
        Some(block.clone()),
        "the launch page puts the caret in today's first block (AC-1.1)"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal, "VM-4: at launch");

    // Escape in Normal ends the edit; the row is drawn as text again.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), None, "the edit ended");

    // `debug_bounds` takes a `&'static str`; leaking one test-built selector is fine.
    let selector: &'static str = Box::leak(format!("text-{}", block.0).into_boxed_str());
    let text = cx
        .debug_bounds(selector)
        .expect("the row's text, now that it is not being edited");
    cx.simulate_click(text.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(block), "VM-4: after a click");
    assert_eq!(mode(&ws, cx), Mode::Normal, "VM-4: after a click");
}

/// VM-5: in Normal no key puts a character in the block. Motions, a count and a
/// repeat-find are all keys the grammar knows, and none of them writes: the text and the
/// undo stack are untouched.
#[gpui::test]
fn vm_5_normal_mode_types_nothing(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let before = rows(&app, &page, cx);
    let depth = undo_depth(&app, cx);
    assert_eq!(mode(&ws, cx), Mode::Normal);

    cx.simulate_keystrokes("h l w b e 0 $ ^ 1 2 3 h l");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        before,
        "no key types a character in Normal mode (VM-5)"
    );
    assert_eq!(
        field_text(&ws, cx),
        "alpha beta",
        "and the buffer the editor would write is untouched too"
    );
    assert_eq!(
        undo_depth(&app, cx),
        depth,
        "and none of them writes, so the undo stack does not grow"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
}

/// VM-5: a key the grammar does not know does nothing at all — no text, no cursor move,
/// no pending count left over, no undo step.
#[gpui::test]
fn vm_5_an_unknown_normal_key_does_nothing(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let block = live_blocks(&app, &page, cx)[0].clone();
    // Somewhere not at either end, so a stray motion would show.
    cx.simulate_keystrokes("0 w");
    let before = rows(&app, &page, cx);
    let at = cursor(&ws, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("@ Q");
    cx.run_until_parked();

    assert_eq!(rows(&app, &page, cx), before, "no text");
    assert_eq!(cursor(&ws, cx), at, "the cursor did not move");
    assert_eq!(undo_depth(&app, cx), depth, "no undo step");
    assert_eq!(target_block(&ws, cx), Some(block), "still the same block");
    assert_eq!(mode(&ws, cx), Mode::Normal);
    assert_eq!(
        pending(&ws, cx),
        "",
        "a rejected key leaves nothing pending in the parser"
    );
}

/// VM-3: `i`, `a`, `I` and `A` each enter Insert, and a letter typed after each lands in
/// the block at the offset the command chose. Regression: a build moved the caret and
/// left the mode in Normal, so the letter went to the parser and vanished.
// The requirement's own name spells the keys, so it is not snake case.
#[allow(non_snake_case)]
#[gpui::test]
fn vm_3_i_a_I_and_A_each_enter_insert_mode(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);

    for (keys, at) in [("i", 'i'), ("a", 'a'), ("I", 'I'), ("A", 'A')] {
        // Normal, with the cursor parked at 0 and the block committed.
        cx.simulate_keystrokes("escape 0");
        cx.run_until_parked();
        assert_eq!(mode(&ws, cx), Mode::Normal, "{keys}: start from Normal");
        let before = field_text(&ws, cx);
        let caret = cursor(&ws, cx);
        let expect = match at {
            'i' => caret,
            'a' => next_grapheme(&before, caret),
            'I' => motion::line_first_non_blank(&before, caret),
            'A' => motion::line_end_exclusive(&before, caret),
            _ => unreachable!(),
        };

        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        assert_eq!(
            mode(&ws, cx),
            Mode::Insert,
            "VM-3: `{keys}` enters Insert, not just a moved caret"
        );
        assert_eq!(cursor(&ws, cx), expect, "`{keys}` puts the caret here");

        // The letter must land in the block, at the caret the command left.
        cx.simulate_input("X");
        cx.run_until_parked();
        let mut want = before.clone();
        want.insert(expect, 'X');
        assert_eq!(
            field_text(&ws, cx),
            want,
            "`{keys}` then a letter types into the block"
        );
        assert_eq!(
            rows(&app, &page, cx)[0].0,
            want,
            "and the store has it: the letter did not go to the parser"
        );
        assert_eq!(cursor(&ws, cx), expect + 1, "the caret is after the letter");
    }
}

/// VM-6: charwise motions stay inside the block that owns the caret. `w` at the end of the
/// block rests on the last character; it neither leaves the block nor moves the caret past
/// the text the Normal cursor may sit on.
#[gpui::test]
fn vm_6_w_at_the_end_of_a_block_stays_in_the_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta gamma"]);
    });
    let page = today_page(&app, cx);
    let block = live_blocks(&app, &page, cx)[0].clone();
    let text = "alpha beta gamma";
    let last = motion::line_end_inclusive(text, text.len());
    assert_eq!(
        last,
        text.len() - 1,
        "the last character is the resting place"
    );

    cx.simulate_keystrokes("0 w w");
    cx.run_until_parked();
    let two_words = motion::word_forward(text, motion::word_forward(text, 0, false), false);
    assert_eq!(cursor(&ws, cx), two_words, "two words on");
    cx.simulate_keystrokes("w w w w");
    cx.run_until_parked();

    assert_eq!(
        cursor(&ws, cx),
        last,
        "`w` at the end of the block stops on the last character (VM-6)"
    );
    assert_eq!(
        target_block(&ws, cx),
        Some(block),
        "still in the same block"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
    assert_eq!(
        rows(&app, &page, cx),
        vec![(text.to_string(), 0)],
        "no new block was made and no character moved"
    );
}

/// VM-7: `j` runs out of wrapped lines and crosses into the next block, and `k` comes
/// back. The mode comes with it, so the next key is still a Normal key.
#[gpui::test]
fn vm_7_j_crosses_into_the_next_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    assert_eq!(ids.len(), 2);
    assert_eq!(target_block(&ws, cx), Some(ids[0].clone()));

    cx.simulate_keystrokes("j");
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[1].clone()),
        "j crosses into the next block (VM-7)"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal, "and stays in Normal");
    assert_eq!(cursor(&ws, cx), 0, "at the start of the block it entered");

    cx.simulate_keystrokes("k");
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[0].clone()),
        "k crosses back"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
    assert_eq!(
        rows(&app, &page, cx),
        vec![("one".to_string(), 0), ("two".to_string(), 0)],
        "travel is not an edit"
    );
}

/// VM-13: Escape is a ladder. The first rung leaves Insert for Normal on the same block,
/// the second ends the edit and remembers where the caret rested, and a Normal key then
/// wakes the cursor at that offset. Waking is not an edit, so the undo stack is unchanged.
#[gpui::test]
fn vm_13_escape_ladder_ends_at_rest_and_a_key_wakes_it(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let block = live_blocks(&app, &page, cx)[0].clone();

    // Rung one: out of Insert, the block committed, the caret on the same row.
    cx.simulate_keystrokes("A");
    cx.simulate_input(" gamma");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "alpha beta gamma");
    assert_eq!(
        mode(&ws, cx),
        Mode::Normal,
        "the first Escape leaves Insert"
    );
    assert_eq!(
        target_block(&ws, cx),
        Some(block.clone()),
        "and keeps the block"
    );

    // Park the caret mid-block, where the wake must bring it back to.
    cx.simulate_keystrokes("0 w");
    let resume_at = cursor(&ws, cx);
    assert_eq!(resume_at, 6, "on `beta`, not at an end");

    // Rung two: out of the edit, at rest, with the place remembered — including the view
    // that hosted it, because the same editor serves the sidebar's pages (F5).
    let host = ws.update(cx, |w, cx| {
        w.editor()
            .read(cx)
            .target()
            .expect("the block is in edit")
            .host
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        None,
        "the second Escape ends the edit"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
    let want = Resume {
        host,
        caret: Caret {
            page: page.clone(),
            block: block.clone(),
            offset: resume_at,
        },
    };
    assert_eq!(
        ws.update(cx, |w, cx| w.editor().read(cx).vim().resume.clone()),
        Some(want),
        "the rest point is remembered for the waking key"
    );
    let depth = undo_depth(&app, cx);
    let before = rows(&app, &page, cx);

    // A Normal key wakes the cursor where it was. `@` is unknown to the grammar, so it
    // moves nothing and the offset it woke at is still there to read.
    cx.simulate_keystrokes("@");
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(block),
        "a Normal key wakes the cursor"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
    assert_eq!(
        cursor(&ws, cx),
        resume_at,
        "at the offset it rested at, not the start of the row"
    );
    assert_eq!(rows(&app, &page, cx), before, "waking is not an edit");
    assert_eq!(undo_depth(&app, cx), depth, "and leaves no undo step");
}

/// VM-1: Vim owns characters, not the whole keyboard. Tab still indents and a Cmd chord
/// still reaches the keymap while a block is in Normal mode.
#[gpui::test]
fn vm_1_tab_and_cmd_chords_still_reach_the_keymap(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    cx.simulate_keystrokes("j");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(ids[1].clone()));
    assert_eq!(mode(&ws, cx), Mode::Normal);

    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("one".to_string(), 0), ("two".to_string(), 1)],
        "Tab indented the block in Normal mode (VM-1)"
    );

    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("one".to_string(), 0), ("two".to_string(), 0)],
        "Cmd+Z reached the keymap in Normal mode and undid the indent"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
}

/// VM-14: in Insert the input method behaves as it does with Vim off (§13.5). Escape with
/// a live composition neither leaves Insert nor commits the provisional glyphs.
#[gpui::test]
fn vm_14_marked_text_in_insert_survives_escape(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["ab"]);
    });
    let page = today_page(&app, cx);
    let editor = editor(&ws, cx);
    cx.simulate_keystrokes("A");
    assert_eq!(mode(&ws, cx), Mode::Insert);
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(field_text(&ws, cx), "abにほ");
    assert_eq!(marked(&editor, cx), Some(2..4));

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert_eq!(
        mode(&ws, cx),
        Mode::Insert,
        "Escape does not climb the ladder while the input method owns the buffer"
    );
    assert_eq!(
        marked(&editor, cx),
        Some(2..4),
        "the composition is untouched"
    );
    assert_eq!(field_text(&ws, cx), "abにほ");
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "ab",
        "nothing provisional was committed"
    );
}

/// VM-14: Normal takes no marked text. Offering a composition neither changes the buffer
/// nor starts one, so the composition flag stays off and the platform has nothing to
/// update.
#[gpui::test]
fn vm_14_normal_takes_no_marked_text(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["ab"]);
    });
    let page = today_page(&app, cx);
    let editor = editor(&ws, cx);
    assert_eq!(mode(&ws, cx), Mode::Normal);
    let at = cursor(&ws, cx);

    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();

    assert_eq!(field_text(&ws, cx), "ab", "the buffer did not change");
    assert_eq!(marked(&editor, cx), None, "and no composition started");
    assert!(
        !editor.read_with(cx, |e, _| e.composing()),
        "the editor is not composing"
    );
    assert_eq!(cursor(&ws, cx), at, "the caret did not move");
    assert_eq!(mode(&ws, cx), Mode::Normal);
    assert_eq!(rows(&app, &page, cx)[0].0, "ab");
}

/// VM-14: committed text that arrives in Normal is a key. It goes to the parser — `3` is a
/// count there — and never into the block.
#[gpui::test]
fn vm_14_committed_text_in_normal_reaches_the_parser_not_the_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["ab"]);
    });
    let page = today_page(&app, cx);
    let editor = editor(&ws, cx);
    assert_eq!(mode(&ws, cx), Mode::Normal);

    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "3", window, cx)
    });
    cx.run_until_parked();

    assert_eq!(field_text(&ws, cx), "ab", "the block did not take the text");
    assert_eq!(rows(&app, &page, cx)[0].0, "ab");
    assert_eq!(
        pending(&ws, cx),
        "3",
        "the parser got the key: it is waiting for a motion to count (VM-14)"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
}

/// VM-14 (§13.5): the caret and the buffer belong to the input method while it composes.
/// A click cannot move the caret under it, and the characters it composed are what the
/// commit persists.
#[gpui::test]
fn vm_14_a_click_during_a_composition_keeps_the_composed_characters(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["ab"]);
    });
    let page = today_page(&app, cx);
    let editor = editor(&ws, cx);
    cx.simulate_keystrokes("A");
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(field_text(&ws, cx), "abにほ");
    assert_eq!(cursor(&ws, cx), 8, "the caret is after the composed glyphs");

    // A click at the left edge, which would put the caret at offset 0 if it were ours.
    let bounds = cx.debug_bounds("block-editor").expect("the editor");
    let mut at = bounds.origin;
    at.x += px(2.);
    at.y += bounds.size.height / 2.;
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();

    assert_eq!(
        cursor(&ws, cx),
        8,
        "the input method owns the caret: a click does not move it under a composition"
    );
    assert_eq!(field_text(&ws, cx), "abにほ");
    assert_eq!(marked(&editor, cx), Some(2..4), "the composition is intact");
    assert_eq!(mode(&ws, cx), Mode::Insert);
    assert_eq!(rows(&app, &page, cx)[0].0, "ab", "nothing provisional");

    // The input method commits; the characters survive the click.
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "にほ", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "abにほ",
        "the composed characters are what persists"
    );
}

/// VM-4 (F1): one editor serves the sidebar's pages, and a sidebar page stays editable
/// while the main column shows something else. Normal keys must be intercepted there too:
/// before, the main-content test ran first, so `enter` split the block and `backspace`
/// edited text in a block that was in Normal mode.
#[gpui::test]
fn f1_normal_keys_reach_a_sidebar_block_while_the_main_pane_shows_the_graph(
    cx: &mut TestAppContext,
) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seeded.replace(Some(seed_page(s, "Sidebar", &["one", "two"])));
    });
    let (page, ids) = seeded.into_inner().expect("seeded");

    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, true, window, cx)
    });
    cx.run_until_parked();
    // The main column leaves the page for the graph, which is the state the gate got
    // wrong (F1).
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();

    let view = ws
        .update(cx, |w, _| w.sidebar_view(0))
        .expect("the page is open in the sidebar");
    view.update_in(cx, |v, window, cx| v.focus_first(window, cx));
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[0].clone()),
        "the sidebar's first block is in edit"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
    let before = rows(&app, &page, cx);

    // Both are ordinary keymap keys and neither is in the Normal grammar, so in Normal
    // they must do nothing at all.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        before,
        "enter did not split the block (F1)"
    );
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        before,
        "backspace changed no text (F1)"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
}

/// VM-4 (F2): both rendered rows share one focus handle, so no focus-loss reset fires
/// between them. A click into a different rendered block enters Normal; before, the mode
/// stayed Insert and the next key typed a character into the block it landed on.
#[gpui::test]
fn f2_a_click_into_another_block_enters_normal_mode(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    cx.simulate_keystrokes("A");
    cx.run_until_parked();
    assert_eq!(
        mode(&ws, cx),
        Mode::Insert,
        "typing at the end of block one"
    );

    // The second row is drawn as text, so its rendered bounds are there to click.
    let selector: &'static str = Box::leak(format!("text-{}", ids[1].0).into_boxed_str());
    let text = cx.debug_bounds(selector).expect("the second row");
    cx.simulate_click(text.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[1].clone()),
        "the click entered the second block"
    );
    assert_eq!(
        mode(&ws, cx),
        Mode::Normal,
        "a click is a block entry, so it enters Normal (F2)"
    );

    // A letter in Normal is a key the grammar does not know: it types nothing.
    cx.simulate_input("Q");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("one".to_string(), 0), ("two".to_string(), 0)],
        "the click did not leave Insert behind (F2)"
    );
}

/// VM-3 (F3): the menu is Insert's. Escape closes it as it leaves Insert, so the
/// `autocomplete` key context goes too and Tab indents instead of accepting a suggestion.
#[gpui::test]
fn f3_escape_from_insert_closes_the_autocomplete_menu(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        s.create_page("Alphabet").unwrap();
    });
    let page = today_page(&app, cx);
    cx.simulate_keystrokes("i");
    cx.simulate_input("[[Al");
    cx.run_until_parked();
    assert_eq!(mode(&ws, cx), Mode::Insert);
    assert!(
        ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .autocomplete()
            .captures_keys()),
        "`[[Al` offers the page it names"
    );

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        mode(&ws, cx),
        Mode::Normal,
        "the first Escape leaves Insert"
    );
    assert!(
        !ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .autocomplete()
            .captures_keys()),
        "no menu captures keys in Normal (F3)"
    );

    // Tab is the indent key again, not `AcceptCompletion`: the text still says what was
    // typed, and no `[[Alphabet]]` link was written over the query.
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    // The commit normalises the half-typed link to `[[Al]]`; accepting the suggestion
    // would have written `[[Alphabet]]` over the query instead.
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "[[Al]]",
        "Tab added no completion (F3)"
    );
}

/// VM-3 (F3, plan §5): entering Insert where the caret sits inside a trigger opens the
/// menu, so `i` inside `[[Alp` offers the pages the text already asked for.
#[gpui::test]
fn f3_entering_insert_inside_a_trigger_opens_the_menu(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["[[Alp]]"]);
        s.create_page("Alphabet").unwrap();
    });
    // Normal, with the caret after `Alp`, where the trigger already covers the text.
    cx.simulate_keystrokes("0 5 l");
    cx.run_until_parked();
    assert_eq!(cursor(&ws, cx), 5);
    assert!(
        !ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .autocomplete()
            .captures_keys()),
        "Normal has no menu"
    );

    cx.simulate_keystrokes("i");
    cx.run_until_parked();
    assert_eq!(mode(&ws, cx), Mode::Insert);
    let menu = ws.update(cx, |w, cx| {
        w.editor().read(cx).autocomplete().menu().cloned()
    });
    let menu = menu.expect("`i` inside `[[Alp` opens its menu (F3)");
    assert_eq!(menu.query, "Alp");
    assert_eq!(menu.start, 0);
}

/// 1E.3 (F4): `G` enters the last block at its end, so the caret has to be clamped to the
/// block cursor's resting place before a command acts on it. `i` then inserts on the
/// block's last character, not after it.
#[gpui::test]
fn f4_g_enters_the_last_block_on_its_last_character(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);

    cx.simulate_keystrokes("G");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(ids[1].clone()));
    assert_eq!(
        cursor(&ws, cx),
        "two".len() - 1,
        "the block cursor rests on the last character (F4)"
    );

    cx.simulate_keystrokes("i");
    cx.simulate_input("X");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[1].0,
        "twXo",
        "`i` inserted before the last character, not after it (F4)"
    );
}

/// 1E.3 (F4): a click puts the caret in the block, so in Normal it lands on a grapheme and
/// never past the last one of its line — a click to the right of the text included.
#[gpui::test]
fn f4_a_click_past_the_end_of_the_line_rests_on_its_last_character(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["ab"]);
    });
    let page = today_page(&app, cx);
    assert_eq!(mode(&ws, cx), Mode::Normal);

    let bounds = cx.debug_bounds("block-editor").expect("the editor");
    let mut at = bounds.origin;
    at.x += bounds.size.width - px(2.);
    at.y += bounds.size.height / 2.;
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        cursor(&ws, cx),
        1,
        "the click's insertion point is clamped to the block cursor (F4)"
    );

    cx.simulate_keystrokes("i");
    cx.simulate_input("X");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "aXb");
}

/// 1E.3 (F4): `j` lands on an insertion point one wrapped line down. When that line is
/// shorter than the caret's column, Normal's cursor steps back onto its last grapheme
/// instead of resting after it.
#[gpui::test]
fn f4_a_vertical_move_rests_on_the_shorter_line(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["abcd\nx"]);
    });
    let page = today_page(&app, cx);
    assert_eq!(field_text(&ws, cx), "abcd\nx");

    // `d` on the first line, then down a line: the caret's column is past the `x`.
    cx.simulate_keystrokes("0 3 l j");
    cx.run_until_parked();
    assert_eq!(
        cursor(&ws, cx),
        5,
        "the cursor rests on the `x`, not after it (F4)"
    );

    cx.simulate_keystrokes("i");
    cx.simulate_input("X");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "abcd\nXx");
}

/// 1E.3 (F4): a click on a rendered row goes through block entry with the caret its text
/// laid out, which at the right edge is one past the last character. Normal's cursor
/// rests on a grapheme, so entry clamps it.
#[gpui::test]
fn f4_a_row_click_past_the_end_of_the_line_rests_on_its_last_character(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    // At rest: the click opens the block from nothing, so no earlier edit is committed and
    // the position it places is only ever seen by block entry.
    cx.simulate_keystrokes("escape escape");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), None);

    // The second row is drawn as text: a click past its last character still lands in it.
    let selector: &'static str = Box::leak(format!("text-{}", ids[1].0).into_boxed_str());
    let text = cx.debug_bounds(selector).expect("the second row");
    let mut at = text.origin;
    at.x += text.size.width - px(2.);
    at.y += text.size.height / 2.;
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(ids[1].clone()));
    assert_eq!(
        cursor(&ws, cx),
        2,
        "block entry clamps the click's caret onto the last character (F4)"
    );

    cx.simulate_keystrokes("i");
    cx.simulate_input("X");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[1].0, "twXo");
}

/// 1E.3 (F4): `k` into the block above resolves its `Goal::LastLine` at the next paint,
/// which aims for the column the caret came from. When the line it lands on is shorter,
/// the position it resolves is one past the line's end; Normal's cursor steps back onto
/// its last grapheme.
#[gpui::test]
fn f4_a_goal_resolved_at_paint_rests_on_a_short_last_line(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["abcd\nx", "abcdefghij"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);

    // The second block, at its last character: `k` then aims for that column above.
    cx.simulate_keystrokes("G");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(ids[1].clone()));
    assert_eq!(cursor(&ws, cx), 9);

    cx.simulate_keystrokes("k");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(ids[0].clone()));
    assert_eq!(
        cursor(&ws, cx),
        5,
        "the goal's column is clamped onto the `x` (F4)"
    );

    cx.simulate_keystrokes("i");
    cx.simulate_input("X");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "abcd\nXx");
}

/// VM-7 (F8): an upward count larger than the rows above clamps at the first body block
/// when there is no properties bar to spend the rest of it on. Before, it returned from
/// the bar branch and nothing moved, while `9j` clamped at the other edge.
#[gpui::test]
fn f8_an_upward_count_past_the_first_block_stops_at_it(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two", "three"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    cx.simulate_keystrokes("j j");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(ids[2].clone()));

    cx.simulate_keystrokes("9 k");
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[0].clone()),
        "a bigger count clamps at the first block (F8)"
    );
}

/// VM-13 (F5): a resume point whose block is gone (a sync or an assistant tombstoned it)
/// is dropped and the waking key falls back to the page's first block. Before, the key
/// was eaten and no block entered edit, so the editor was stuck at rest.
#[gpui::test]
fn f5_a_dead_resume_point_wakes_the_first_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);

    // Rest on the second block, with its place remembered, then lose it to a remote write.
    cx.simulate_keystrokes("j escape escape");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), None, "at rest");
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).vim().resume.is_some()),
        "at rest with a resume point"
    );
    app.update(cx, |app, _| {
        let now = app.store.now();
        app.store
            .execute(
                &page,
                vec![Command::SetDeleted {
                    ids: vec![ids[1].clone()],
                    deleted_at: Some(now),
                }],
                Default::default(),
            )
            .unwrap();
    });
    cx.run_until_parked();

    cx.simulate_keystrokes("@");
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[0].clone()),
        "the waking key falls back to the first block (F5)"
    );
    assert_eq!(mode(&ws, cx), Mode::Normal);
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).vim().resume.is_none()),
        "the unusable resume point is dropped (F5)"
    );
}

/// A deterministic xorshift64, so the random session is the same on every run and a
/// failure can be reproduced key for key.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The key pool for the random session. Each entry is a keystroke `simulate_keystrokes`
/// parses. The mix is deliberate: the four insert entries are a ninth of the pool, so most
/// keys are Normal-mode motions that write nothing and the session pushes far fewer undo
/// groups than the stack's 1,000-group cap — a full undo must still reach the start.
const KEYS: &[&str] = &[
    // Entering Insert and leaving it.
    "i",
    "a",
    "A",
    "o",
    "escape",
    // Motions, operators and prefixes: keys that write nothing.
    "h",
    "j",
    "k",
    "l",
    "w",
    "b",
    "e",
    "0",
    "$",
    "^",
    "d",
    "y",
    "g",
    "f",
    "r",
    "\"",
    "v",
    "V",
    "~",
    ".",
    ";",
    ",",
    // Commands that write through the store, and counts to drive them. `u` is left out on
    // purpose: an undo inside the session drops the redo groups, so the seeded graph would
    // no longer be a state this history can reach and "undo back to the start" would stop
    // meaning anything.
    "x",
    "X",
    "D",
    "p",
    "P",
    "J",
    "1",
    "2",
    "5",
    "9",
    // Named keys: caret moves, block splits and merges, the indent that reaches the keymap.
    "enter",
    "backspace",
    "left",
    "right",
    "up",
    "down",
    "tab",
    "space",
    // Ordinary letters: the case a Normal key must never type.
    "z",
    "q",
    "m",
    "t",
    "c",
    "n",
];

/// Rules 2, 5 and 11: a few thousand random keys against a small graph. Nothing panics,
/// the projections still agree with the CRDT (`doctor`), and undoing everything brings the
/// starting text back — the session lost no character it could not put back.
#[gpui::test]
fn rules_2_5_11_a_seeded_random_session_leaves_the_graph_recoverable(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["alpha beta gamma", "a [[Link]] and a #tag"]);
        // One block with a child, so the session has a tree to walk into.
        let child = BlockId::new();
        s.execute(
            &page,
            vec![Command::InsertBlock {
                id: child,
                parent: Some(ids[1].clone()),
                after: None,
                text: "nested".to_string(),
                kind: BlockKind::Todo,
            }],
            Default::default(),
        )
        .unwrap();
    });
    let page = today_page(&app, cx);
    let start = rows(&app, &page, cx);
    let depth0 = undo_depth(&app, cx);
    assert_eq!(start.len(), 3, "the start is the seeded graph: {start:?}");

    // 3,000 keys from a fixed seed.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut keys: Vec<&str> = Vec::with_capacity(3000);
    let mut digits = 0;
    while keys.len() < 3000 {
        let mut key = KEYS[rng.pick(KEYS.len())];
        let digit = key.len() == 1 && key.as_bytes()[0].is_ascii_digit();
        if digit {
            digits += 1;
            if digits > 2 {
                // Two digits are a count of at most 99. Three would let a counted `u`
                // run for thousands of undos; long counts are not this test's subject.
                key = "h";
                digits = 0;
            }
        } else {
            digits = 0;
        }
        keys.push(key);
    }
    // Batches of 100: one paint per batch, not one per key.
    for batch in keys.chunks(100) {
        cx.simulate_keystrokes(&batch.join(" "));
    }
    cx.run_until_parked();
    // Leave whatever mode the session ended in, so the pending text is committed.
    cx.simulate_keystrokes("escape escape");
    cx.run_until_parked();

    app.update(cx, |app, _| {
        let report = app.store.doctor().unwrap();
        assert!(report.is_ok(), "rule 2: doctor is clean\\n{report:?}");
    });
    let depth1 = undo_depth(&app, cx);
    assert!(
        depth1 > depth0,
        "the session wrote something, or this test proves nothing"
    );

    // Undo back to the start. It is reached part-way through the stack: the seeded graph
    // is where the session began, not the empty graph the day was made from.
    let mut restored = false;
    for _ in 0..1000 {
        if rows(&app, &page, cx) == start {
            restored = true;
            break;
        }
        let undone = app.update(cx, |app, _| {
            if app.store.can_undo() {
                app.store.undo().unwrap();
                true
            } else {
                false
            }
        });
        if !undone {
            break;
        }
    }
    assert!(
        restored,
        "rule 5: a full undo restores the starting text\\nstart: {start:?}\\nnow: {:?}",
        rows(&app, &page, cx)
    );
    app.update(cx, |app, _| {
        let report = app.store.doctor().unwrap();
        assert!(report.is_ok(), "rule 2 after the undos\\n{report:?}");
    });
}

// ------------------------------------------------------------------------ PR 2 — 2A
// Undo runs (VM-12; PRD §9.1.1, the FR-2.12 note): one Insert run is one undo group.

/// The system clipboard's text, as the app would paste it.
fn clipboard(cx: &mut VisualTestContext) -> String {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

/// 2A.4 (VM-12): pauses longer than the 500 ms coalesce window do not split an Insert run.
/// The store's clock is settable, so this jumps it instead of sleeping.
#[gpui::test]
fn vm_12_an_insert_run_with_long_pauses_is_one_undo_group(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["x"]);
    });
    let page = today_page(&app, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("A");
    for (i, ch) in ["a", "b", "c"].iter().enumerate() {
        // 600 ms apart: three groups outside a run, one inside it.
        app.update(cx, |app, _| {
            app.store.set_clock(Some(1_000 + 600 * i as i64))
        });
        cx.simulate_input(ch);
    }
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert_eq!(rows(&app, &page, cx)[0].0, "xabc");
    assert_eq!(
        undo_depth(&app, cx),
        depth + 1,
        "one Insert run is one undo group, whatever the pauses (VM-12)"
    );
    // And one `u` takes the whole run back.
    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "x", "one undo, the whole run");
    assert_eq!(undo_depth(&app, cx), depth);
    assert_eq!(mode(&ws, cx), Mode::Normal);
}

/// 2A.3 (VM-12): a blur closes the run, so `i`, two typed characters and a click elsewhere
/// are still one undo step.
#[gpui::test]
fn vm_12_a_run_closed_by_a_blur_is_one_group(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["x"]);
    });
    let page = today_page(&app, cx);
    let editor = editor(&ws, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("A");
    app.update(cx, |app, _| app.store.set_clock(Some(1_000)));
    cx.simulate_input("a");
    app.update(cx, |app, _| app.store.set_clock(Some(2_000)));
    cx.simulate_input("b");
    cx.run_until_parked();
    assert!(
        app.update(cx, |app, _| app.store.in_undo_run()),
        "typing in Insert holds the run open"
    );

    // The keyboard goes somewhere else without an Escape.
    editor.update(cx, |e, cx| e.focus_lost(cx));
    cx.run_until_parked();

    assert!(
        !app.update(cx, |app, _| app.store.in_undo_run()),
        "the blur closed the run (2A.3)"
    );
    assert_eq!(
        undo_depth(&app, cx),
        depth + 1,
        "the whole run is one step even when the blur closed it"
    );
    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "x");
}

/// 2A.1 (VM-12): an undo that starts while a run is open closes the run first, so it takes
/// the whole Insert run and not the edit before it.
#[gpui::test]
fn vm_12_an_undo_during_an_open_run_closes_it_first(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["x"]);
    });
    let page = today_page(&app, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("A");
    app.update(cx, |app, _| app.store.set_clock(Some(1_000)));
    cx.simulate_input("a");
    app.update(cx, |app, _| app.store.set_clock(Some(2_000)));
    cx.simulate_input("b");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "xab");

    // An undo with the run still open: the run is folded and undone as one step.
    app.update(cx, |app, _| app.store.undo().unwrap());
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "x",
        "the undo took the whole run, not one keystroke of it"
    );
    assert!(!app.update(cx, |app, _| app.store.in_undo_run()));
    assert_eq!(undo_depth(&app, cx), depth);
}

// ------------------------------------------------------------------------ PR 2 — 2B
// Charwise operators: one command is one undo step, and `c`/`s`/`S`/`C` type inside it.

/// 2B (VM-12): each charwise edit is ONE undo step. One block per case, so each starts from
/// its own text; `j` reaches the next one and `0` plus `l` place the Normal cursor.
#[gpui::test]
fn vm_12_each_charwise_edit_is_one_undo_step(cx: &mut TestAppContext) {
    // (text, cursor offset, keys, text after)
    let cases: &[(&str, usize, &str, &str)] = &[
        ("abc", 1, "x", "ac"),
        ("abc", 1, "X", "bc"),
        ("abc def", 4, "D", "abc "),
        ("abc", 0, "~", "Abc"),
        ("abc", 0, "r x", "xbc"),
        ("alpha beta", 0, "d w", "beta"),
        // The last word of the block: no stray character and no other block joined.
        ("alpha beta", 6, "d w", "alpha "),
        ("alpha beta", 0, "d e", " beta"),
        ("alpha beta", 6, "d $", "alpha "),
        ("alpha beta", 6, "d 0", "beta"),
        ("alpha beta", 7, "d i w", "alpha "),
        ("alpha beta", 7, "d a w", "alpha"),
        ("f(a b)c", 3, "d i (", "f()c"),
        ("a b-c d", 0, "d W", "b-c d"),
        ("alpha beta", 7, "d i W", "alpha "),
        ("a [x] b", 3, "d i [", "a [] b"),
        ("a {x} b", 3, "d i {", "a {} b"),
        ("say \"hi\" now", 6, "d i \"", "say \"\" now"),
        ("say 'hi' now", 6, "d a '", "say  now"),
        ("run `now` ok", 6, "d i `", "run `` ok"),
    ];
    let texts: Vec<&str> = cases.iter().map(|(text, ..)| *text).collect();
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &texts);
    });
    let page = today_page(&app, cx);

    for (i, (text, at, keys, want)) in cases.iter().enumerate() {
        let mut strokes = String::new();
        if i > 0 {
            strokes.push_str("j ");
        }
        strokes.push_str("0 ");
        for _ in 0..*at {
            strokes.push_str("l ");
        }
        strokes.push_str(keys);
        let depth = undo_depth(&app, cx);
        cx.simulate_keystrokes(&strokes);
        cx.run_until_parked();

        assert_eq!(
            rows(&app, &page, cx)[i].0,
            *want,
            "`{keys}` on {text:?} (block {i})"
        );
        assert_eq!(
            undo_depth(&app, cx),
            depth + 1,
            "`{keys}` on {text:?} is one undo step (VM-12)"
        );
    }
}

/// 2B (VM-12): `c`, `s`, `S` and `C` delete and then type inside one run, so the whole
/// change is one undo step.
#[gpui::test]
fn vm_12_each_change_types_inside_one_run(cx: &mut TestAppContext) {
    // (text, cursor offset, keys, text after)
    let cases: &[(&str, usize, &str, &str)] = &[
        ("abc def", 4, "C", "abc X"),
        ("abc", 1, "s", "aXc"),
        ("abc def", 0, "S", "X"),
        ("abc def", 0, "c c", "X"),
        ("say \"hi\" now", 5, "c i \"", "say \"X\" now"),
        ("alpha beta", 0, "c w", "X beta"),
        ("alpha beta", 7, "c i w", "alpha X"),
        ("alpha beta", 0, "c a w", "Xbeta"),
    ];
    let texts: Vec<&str> = cases.iter().map(|(text, ..)| *text).collect();
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &texts);
    });
    let page = today_page(&app, cx);

    for (i, (text, at, keys, want)) in cases.iter().enumerate() {
        let mut strokes = String::new();
        if i > 0 {
            strokes.push_str("j ");
        }
        strokes.push_str("0 ");
        for _ in 0..*at {
            strokes.push_str("l ");
        }
        strokes.push_str(keys);
        let depth = undo_depth(&app, cx);
        cx.simulate_keystrokes(&strokes);
        cx.run_until_parked();
        cx.simulate_input("X");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();

        assert_eq!(
            rows(&app, &page, cx)[i].0,
            *want,
            "`{keys}` then typing on {text:?} (block {i})"
        );
        assert_eq!(
            undo_depth(&app, cx),
            depth + 1,
            "`{keys}` and its typing are one undo step (VM-12)"
        );
    }
}

/// 2A.4 + 2B, the headline acceptance: `ciw`, a typed word and Escape are one undo step, and
/// `u` brings the original word back in one more.
#[gpui::test]
fn vm_12_ciw_typing_and_escape_undo_as_one_step(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("0 w"); // on `beta`
    assert_eq!(cursor(&ws, cx), 6);
    cx.simulate_keystrokes("c i w");
    cx.run_until_parked();
    assert_eq!(mode(&ws, cx), Mode::Insert, "`ciw` enters Insert");
    assert_eq!(field_text(&ws, cx), "alpha ", "the word is gone first");
    cx.simulate_input("gamma");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert_eq!(rows(&app, &page, cx)[0].0, "alpha gamma");
    assert_eq!(
        undo_depth(&app, cx),
        depth + 1,
        "`ciw` and its typing are one undo step (2A.4)"
    );

    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "alpha beta",
        "`u` restores the original word in one step"
    );
    assert_eq!(undo_depth(&app, cx), depth);
    assert_eq!(mode(&ws, cx), Mode::Normal);
    assert!(
        cursor(&ws, cx) <= "alpha beta".len(),
        "the cursor is still in the restored block"
    );
}

/// 2B (VM-8): `cc` changes the block's own text and leaves its children alone.
#[gpui::test]
fn vm_12_cc_changes_the_text_and_keeps_the_children(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["parent", "sibling"]);
        let child = BlockId::new();
        s.execute(
            &page,
            vec![Command::InsertBlock {
                id: child,
                parent: Some(ids[0].clone()),
                after: None,
                text: "child".to_string(),
                kind: BlockKind::Bullet,
            }],
            Default::default(),
        )
        .unwrap();
        seeded.replace(Some(page));
    });
    let page = seeded.into_inner().expect("seeded");
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("0 c c");
    cx.run_until_parked();
    cx.simulate_input("new");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("new".to_string(), 0),
            ("child".to_string(), 1),
            ("sibling".to_string(), 0),
        ],
        "`cc` took the block's text, not the blocks below it (VM-8)"
    );
    assert_eq!(undo_depth(&app, cx), depth + 1, "one undo step");
    assert_eq!(mode(&ws, cx), Mode::Normal);
}

/// 2B: `ci[` inside `[[Alpha]]` leaves the caret inside the link, and entering Insert
/// re-reads the trigger, so the page menu opens (PR 1's rule, relied on here).
#[gpui::test]
fn vm_12_ci_bracket_inside_a_link_opens_the_menu(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["[[Alpha]]"]);
        s.create_page("Alphabet").unwrap();
    });
    // On the `h` of `Alpha`, inside the brackets.
    cx.simulate_keystrokes("0 3 l");
    assert_eq!(cursor(&ws, cx), 3);
    assert_eq!(field_text(&ws, cx), "[[Alpha]]");

    cx.simulate_keystrokes("c i [");
    cx.run_until_parked();

    assert_eq!(mode(&ws, cx), Mode::Insert, "`ci[` enters Insert");
    assert_eq!(
        field_text(&ws, cx),
        "[[]]",
        "the link's text is what was changed"
    );
    assert_eq!(
        cursor(&ws, cx),
        2,
        "the caret is inside the brackets, where the menu is"
    );
    let menu = ws
        .update(cx, |w, cx| {
            w.editor().read(cx).autocomplete().menu().cloned()
        })
        .expect("`ci[` inside `[[…]]` opens the link's menu");
    assert_eq!(menu.start, 0, "the trigger is the link's opening `[[`");
}

/// 2B (VM-11): `y` writes the register *and* the system clipboard; `d`, `c` and `x` write
/// the register only. `"_` discards.
#[gpui::test]
fn vm_11_a_yank_writes_the_clipboard_and_a_delete_does_not(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta", "gamma delta", "one two"]);
    });
    let page = today_page(&app, cx);

    let depth = undo_depth(&app, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".into()));
    cx.simulate_keystrokes("0 y w");
    cx.run_until_parked();
    assert_eq!(
        clipboard(cx),
        "alpha ",
        "a yank puts what it took on the system clipboard (VM-11)"
    );
    assert_eq!(
        undo_depth(&app, cx),
        depth,
        "`yw` is a yank, not an edit: it takes nothing and leaves no undo step"
    );
    assert!(
        matches!(utopia_ui::vim::last(), Register::Chars(t) if t == "alpha "),
        "and in the register"
    );
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "alpha beta",
        "yanking writes no text"
    );

    // `d`, `c` and `x` leave the clipboard as it was.
    for (keys, typed) in [("0 d w", false), ("0 x", false), ("j 0 c w", true)] {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".into()));
        cx.simulate_keystrokes(keys);
        if typed {
            cx.simulate_input("Z");
            cx.simulate_keystrokes("escape");
        }
        cx.run_until_parked();
        assert_eq!(
            clipboard(cx),
            "sentinel",
            "`{keys}` writes the register only"
        );
    }

    // `"_` writes neither: the black hole register.
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".into()));
    cx.simulate_keystrokes("\" _ y y");
    cx.run_until_parked();
    assert_eq!(clipboard(cx), "sentinel", "`\"_` discards");
}

/// 2B (VM-12): a text object with no object under the cursor writes nothing and leaves no
/// undo step — `di(` outside any bracket.
#[gpui::test]
fn vm_12_an_object_that_is_not_there_writes_nothing(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["plain text"]);
    });
    let page = today_page(&app, cx);
    let depth = undo_depth(&app, cx);
    let before = rows(&app, &page, cx);

    cx.simulate_keystrokes("0 d i (");
    cx.run_until_parked();

    assert_eq!(rows(&app, &page, cx), before, "no text changed");
    assert_eq!(undo_depth(&app, cx), depth, "no undo step");
    assert_eq!(mode(&ws, cx), Mode::Normal);
}
