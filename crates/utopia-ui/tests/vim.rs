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

fn setup_seeded(
    cx: &mut TestAppContext,
    vim: bool,
    seed: impl FnOnce(&mut Store),
) -> (Entity<AppState>, Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        if vim {
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

fn today_page(app: &Entity<AppState>, cx: &mut VisualTestContext) -> PageId {
    app.update(cx, |app, _| app.store.draft_daily(today()).unwrap())
}

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

fn target_block(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<BlockId> {
    ws.update(cx, |w, cx| {
        w.editor().read(cx).target().map(|t| t.block.clone())
    })
}

fn marked(
    editor: &Entity<BlockEditor>,
    cx: &mut VisualTestContext,
) -> Option<std::ops::Range<usize>> {
    editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx))
}

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

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), None, "the edit ended");

    let selector: &'static str = Box::leak(format!("text-{}", block.0).into_boxed_str());
    let text = cx
        .debug_bounds(selector)
        .expect("the row's text, now that it is not being edited");
    cx.simulate_click(text.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(block), "VM-4: after a click");
    assert_eq!(mode(&ws, cx), Mode::Normal, "VM-4: after a click");
}

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

#[gpui::test]
fn vm_5_an_unknown_normal_key_does_nothing(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let block = live_blocks(&app, &page, cx)[0].clone();
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

#[allow(non_snake_case)]
#[gpui::test]
fn vm_3_i_a_I_and_A_each_enter_insert_mode(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);

    for (keys, at) in [("i", 'i'), ("a", 'a'), ("I", 'I'), ("A", 'A')] {
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

#[gpui::test]
fn vm_13_escape_ladder_ends_at_rest_and_a_key_wakes_it(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let block = live_blocks(&app, &page, cx)[0].clone();

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

    cx.simulate_keystrokes("0 w");
    let resume_at = cursor(&ws, cx);
    assert_eq!(resume_at, 6, "on `beta`, not at an end");

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

    cx.simulate_input("Q");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("one".to_string(), 0), ("two".to_string(), 0)],
        "the click did not leave Insert behind (F2)"
    );
}

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

    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "[[Al]]",
        "Tab added no completion (F3)"
    );
}

#[gpui::test]
fn f3_entering_insert_inside_a_trigger_opens_the_menu(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["[[Alp]]"]);
        s.create_page("Alphabet").unwrap();
    });
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

#[gpui::test]
fn f4_a_vertical_move_rests_on_the_shorter_line(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["abcd\nx"]);
    });
    let page = today_page(&app, cx);
    assert_eq!(field_text(&ws, cx), "abcd\nx");

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

#[gpui::test]
fn f4_a_row_click_past_the_end_of_the_line_rests_on_its_last_character(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    cx.simulate_keystrokes("escape escape");
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), None);

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

#[gpui::test]
fn f4_a_goal_resolved_at_paint_rests_on_a_short_last_line(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["abcd\nx", "abcdefghij"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);

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

#[gpui::test]
fn f5_a_dead_resume_point_wakes_the_first_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one", "two"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);

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

const KEYS: &[&str] = &[
    "i",
    "a",
    "A",
    "o",
    "escape",
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
    "enter",
    "backspace",
    "left",
    "right",
    "up",
    "down",
    "tab",
    "space",
    "z",
    "q",
    "m",
    "t",
    "c",
    "n",
];

#[gpui::test]
fn rules_2_5_11_a_seeded_random_session_leaves_the_graph_recoverable(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["alpha beta gamma", "a [[Link]] and a #tag"]);
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

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut keys: Vec<&str> = Vec::with_capacity(3000);
    let mut digits = 0;
    while keys.len() < 3000 {
        let mut key = KEYS[rng.pick(KEYS.len())];
        let digit = key.len() == 1 && key.as_bytes()[0].is_ascii_digit();
        if digit {
            digits += 1;
            if digits > 2 {
                key = "h";
                digits = 0;
            }
        } else {
            digits = 0;
        }
        keys.push(key);
    }
    for batch in keys.chunks(100) {
        cx.simulate_keystrokes(&batch.join(" "));
    }
    cx.run_until_parked();
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

fn clipboard(cx: &mut VisualTestContext) -> String {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

#[gpui::test]
fn vm_12_an_insert_run_with_long_pauses_is_one_undo_group(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["x"]);
    });
    let page = today_page(&app, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("A");
    for (i, ch) in ["a", "b", "c"].iter().enumerate() {
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
    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "x", "one undo, the whole run");
    assert_eq!(undo_depth(&app, cx), depth);
    assert_eq!(mode(&ws, cx), Mode::Normal);
}

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

#[gpui::test]
fn vm_12_an_undo_during_an_open_run_closes_it_first(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
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

    let editor = editor(&ws, cx);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "x",
        "the undo took the whole run, not one keystroke of it"
    );
    assert!(app.update(cx, |app, _| app.store.in_undo_run()));
    assert!(
        editor.read_with(cx, |e, _| e.vim().insert_run),
        "the editor's flag agrees with the store after the undo (F7)"
    );
    assert_eq!(undo_depth(&app, cx), depth);
}

fn setup_page(
    cx: &mut TestAppContext,
    seed: impl FnOnce(&mut Store) -> PageId,
) -> (
    Entity<AppState>,
    Entity<Workspace>,
    &mut VisualTestContext,
    PageId,
) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::set_vim(cx, true, std::path::Path::new("")).unwrap();
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today()));
    let page = seed(&mut store);
    let app = cx.new(|_| AppState::new(store));
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    let opened = page.clone();
    ws.update_in(cx, |w, window, cx| {
        w.open(opened, None, None, false, window, cx)
    });
    cx.run_until_parked();
    (app, ws, cx, page)
}

fn click_block(cx: &mut VisualTestContext, block: &BlockId) {
    let selector: String = format!("text-{}", block.0);
    click_selector(cx, &selector);
}

fn click_selector(cx: &mut VisualTestContext, selector: &str) {
    let selector: &'static str = Box::leak(selector.to_string().into_boxed_str());
    let row = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} was not rendered"));
    cx.simulate_click(row.center(), Modifiers::default());
    cx.run_until_parked();
}

fn seed_chain(store: &mut Store, page: &PageId, depth: usize) -> Vec<BlockId> {
    let mut ids = Vec::new();
    let mut parent: Option<BlockId> = None;
    let mut after: Option<BlockId> = None;
    for d in 0..depth {
        let id = BlockId::new();
        store
            .execute(
                page,
                vec![Command::InsertBlock {
                    id: id.clone(),
                    parent: parent.clone(),
                    after: after.clone(),
                    text: format!("d{d}"),
                    kind: BlockKind::Bullet,
                }],
                Default::default(),
            )
            .unwrap();
        after = if parent.is_none() {
            Some(id.clone())
        } else {
            None
        };
        parent = Some(id.clone());
        ids.push(id);
    }
    ids
}

#[gpui::test]
fn vm_8_block_commands_leave_the_properties_bar_alone(cx: &mut TestAppContext) {
    let (app, ws, cx, page) = setup_page(cx, |s| {
        let page = s.create_page("Trellis").unwrap();
        let ids: Vec<BlockId> = (0..3).map(|_| BlockId::new()).collect();
        s.execute(
            &page,
            vec![
                Command::InsertBlock {
                    id: ids[0].clone(),
                    parent: None,
                    after: None,
                    text: "status:: dogfooding".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: ids[1].clone(),
                    parent: None,
                    after: Some(ids[0].clone()),
                    text: "owner:: [[Ana]]".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: ids[2].clone(),
                    parent: None,
                    after: Some(ids[1].clone()),
                    text: "a body note".into(),
                    kind: BlockKind::Bullet,
                },
            ],
            Default::default(),
        )
        .unwrap();
        s.create_page("Ana").unwrap();
        s.sync_index().unwrap();
        page
    });
    let ids = live_blocks(&app, &page, cx);
    assert_eq!(ids.len(), 3);

    click_block(cx, &ids[2]);
    cx.simulate_keystrokes("y y");
    cx.run_until_parked();
    click_selector(cx, "property-0");
    assert_eq!(target_block(&ws, cx), Some(ids[0].clone()));
    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("status:: dogfooding".to_string(), 0),
            ("owner:: [[Ana]]".to_string(), 0),
            ("a body note".to_string(), 0)
        ],
        "`dd` on a property row writes nothing (F12)"
    );
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[0].clone()),
        "and the bar keeps its row"
    );

    cx.simulate_keystrokes("p");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("status:: dogfooding".to_string(), 0),
            ("owner:: [[Ana]]".to_string(), 0),
            ("a body note".to_string(), 0)
        ],
        "a paste in the properties bar is refused too (F12)"
    );
}

#[gpui::test]
fn vm_8_dd_in_a_zoom_focuses_a_visible_row(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx, page) = setup_page(cx, |s| {
        let page = s.create_page("Zoomed").unwrap();
        let z = BlockId::new();
        s.execute(
            &page,
            vec![Command::InsertBlock {
                id: z.clone(),
                parent: None,
                after: None,
                text: "zoom root".into(),
                kind: BlockKind::Bullet,
            }],
            Default::default(),
        )
        .unwrap();
        let a = BlockId::new();
        let b = BlockId::new();
        let a1 = BlockId::new();
        s.execute(
            &page,
            vec![
                Command::InsertBlock {
                    id: a.clone(),
                    parent: Some(z.clone()),
                    after: None,
                    text: "a".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: b.clone(),
                    parent: Some(z.clone()),
                    after: Some(a.clone()),
                    text: "b".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: a1.clone(),
                    parent: Some(a.clone()),
                    after: None,
                    text: "a1".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: BlockId::new(),
                    parent: None,
                    after: Some(z.clone()),
                    text: "outside".into(),
                    kind: BlockKind::Bullet,
                },
            ],
            Default::default(),
        )
        .unwrap();
        s.set_collapsed(&a, true).unwrap();
        s.sync_index().unwrap();
        seeded.replace(Some((z.clone(), a.clone(), b.clone())));
        page
    });
    let (z, a, b) = seeded.into_inner().expect("seeded");

    let (z2, b2) = (z.clone(), b.clone());
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), Some(z2), Some(b2), false, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(b.clone()));

    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert_eq!(
        target_block(&ws, cx),
        Some(a.clone()),
        "the caret went to the row above on screen, inside the zoom (F12)"
    );
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("zoom root".to_string(), 0),
            ("a".to_string(), 1),
            ("a1".to_string(), 2),
            ("outside".to_string(), 0)
        ]
    );
}

#[gpui::test]
fn vm_10_a_cut_that_is_too_deep_here_is_not_spent(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx, src) = setup_page(cx, |s| {
        let src = s.create_page("Source").unwrap();
        let chain = seed_chain(s, &src, utopia_core::outline::MAX_DROP_DEPTH);
        let other = seed_chain(s, &src, utopia_core::outline::MAX_DROP_DEPTH - 1);
        let dst = s.create_page("Destination").unwrap();
        let deep = seed_chain(s, &dst, utopia_core::outline::MAX_DROP_DEPTH - 1);
        let shelf = seed_chain(s, &dst, 1);
        seeded.replace(Some((chain, other, dst, deep, shelf)));
        src
    });
    let (chain, other, dst, deep, shelf) = seeded.into_inner().expect("seeded");
    let root = chain[0].clone();

    click_block(cx, &root);
    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert!(!app.update(cx, |app, _| app
        .store
        .page_state(&src)
        .unwrap()
        .is_live(&root)));
    let before = all_block_ids(&app, &src, cx);

    click_block(cx, &other[other.len() - 1]);
    cx.simulate_keystrokes("p");
    cx.run_until_parked();
    assert_eq!(
        all_block_ids(&app, &src, cx),
        before,
        "the too-deep same-page paste wrote nothing"
    );
    assert!(
        matches!(
            utopia_ui::vim::last(),
            Register::Blocks { cut: Some(_), .. }
        ),
        "and the cut is still a cut (F3)"
    );

    let dst2 = dst.clone();
    ws.update_in(cx, |w, window, cx| {
        w.open(dst2, None, None, false, window, cx)
    });
    cx.run_until_parked();
    let dst_before = all_block_ids(&app, &dst, cx);
    click_block(cx, &deep[deep.len() - 1]);
    cx.simulate_keystrokes("p");
    cx.run_until_parked();
    assert_eq!(
        all_block_ids(&app, &dst, cx),
        dst_before,
        "the too-deep cross-page paste wrote nothing"
    );
    assert!(
        matches!(
            utopia_ui::vim::last(),
            Register::Blocks { cut: Some(_), .. }
        ),
        "and the cut survived that refusal too (F3)"
    );

    click_block(cx, &shelf[0]);
    cx.simulate_keystrokes("p");
    cx.run_until_parked();
    for id in &chain {
        assert!(
            app.update(cx, |app, _| app
                .store
                .page_state(&dst)
                .unwrap()
                .contains(id)),
            "block {id} arrived with its id (VM-10)"
        );
    }
    assert!(
        app.update(cx, |app, _| app
            .store
            .page_state(&src)
            .unwrap()
            .contains(&root)),
        "and the source keeps its rows as history"
    );
    app.update(cx, |app, _| {
        let report = app.store.doctor().unwrap();
        assert!(report.is_ok(), "rule 2: {report:?}");
    });
}

#[gpui::test]
fn vm_12_each_charwise_edit_is_one_undo_step(cx: &mut TestAppContext) {
    let cases: &[(&str, usize, &str, &str)] = &[
        ("abc", 1, "x", "ac"),
        ("abc", 1, "X", "bc"),
        ("abc def", 4, "D", "abc "),
        ("abc", 0, "~", "Abc"),
        ("abc", 0, "r x", "xbc"),
        ("alpha beta", 0, "d w", "beta"),
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

#[gpui::test]
fn vm_12_each_change_types_inside_one_run(cx: &mut TestAppContext) {
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

#[gpui::test]
fn vm_12_ciw_typing_and_escape_undo_as_one_step(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta"]);
    });
    let page = today_page(&app, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("0 w");
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

#[gpui::test]
fn vm_12_ci_bracket_inside_a_link_opens_the_menu(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["[[Alpha]]"]);
        s.create_page("Alphabet").unwrap();
    });
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

    cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".into()));
    cx.simulate_keystrokes("\" _ y y");
    cx.run_until_parked();
    assert_eq!(clipboard(cx), "sentinel", "`\"_` discards");
}

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

fn all_block_ids(
    app: &Entity<AppState>,
    page: &PageId,
    cx: &mut VisualTestContext,
) -> Vec<BlockId> {
    app.update(cx, |app, _| {
        let st = app.store.page_state(page).unwrap();
        let mut ids: Vec<BlockId> = st.blocks.keys().cloned().collect();
        ids.sort();
        ids
    })
}

fn kind_of(
    app: &Entity<AppState>,
    page: &PageId,
    block: &BlockId,
    cx: &mut VisualTestContext,
) -> Option<(BlockKind, bool)> {
    app.update(cx, |app, _| {
        app.store
            .page_state(page)
            .unwrap()
            .block(block)
            .map(|b| (b.kind, b.checked))
    })
}

fn seed_child(store: &mut Store, page: &PageId, parent: &BlockId, text: &str) -> BlockId {
    let id = BlockId::new();
    store
        .execute(
            page,
            vec![Command::InsertBlock {
                id: id.clone(),
                parent: Some(parent.clone()),
                after: None,
                text: text.to_string(),
                kind: BlockKind::Bullet,
            }],
            Default::default(),
        )
        .unwrap();
    id
}

#[gpui::test]
fn vm_8_dd_deletes_the_subtree_in_one_undo_step(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["parent", "next", "last"]);
        let child = seed_child(s, &page, &ids[0], "child");
        seeded.replace(Some((page, ids, child)));
    });
    let (page, ids, child) = seeded.into_inner().expect("seeded");
    let before = all_block_ids(&app, &page, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("d d");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![("next".to_string(), 0), ("last".to_string(), 0)],
        "the block and its subtree went"
    );
    assert_eq!(undo_depth(&app, cx), depth + 1, "`dd` is one undo step");
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[1].clone()),
        "the cursor is on the NEXT block, as Vim's is (VM-8)"
    );
    assert_eq!(
        all_block_ids(&app, &page, cx),
        before,
        "rule 5: the ids are still there, as tombstones"
    );
    assert_eq!(
        kind_of(&app, &page, &child, cx),
        Some((BlockKind::Bullet, false)),
        "the child is a row, not a purge"
    );

    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("parent".to_string(), 0),
            ("child".to_string(), 1),
            ("next".to_string(), 0),
            ("last".to_string(), 0)
        ],
        "one undo brings the whole subtree back"
    );
}

#[gpui::test]
fn vm_10_dd_then_p_moves_the_blocks_and_keeps_every_id(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["parent", "next", "last"]);
        let child = seed_child(s, &page, &ids[0], "child");
        let other = s.create_page("Other").unwrap();
        let ref_block = BlockId::new();
        s.execute(
            &other,
            vec![Command::InsertBlock {
                id: ref_block,
                parent: None,
                after: None,
                text: format!("see (({child}))"),
                kind: BlockKind::Bullet,
            }],
            Default::default(),
        )
        .unwrap();
        seeded.replace(Some((page, ids, child, other)));
    });
    let (page, ids, child, other) = seeded.into_inner().expect("seeded");
    let before = all_block_ids(&app, &page, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("d d p");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("next".to_string(), 0),
            ("parent".to_string(), 0),
            ("child".to_string(), 1),
            ("last".to_string(), 0)
        ],
        "the blocks moved, subtree and all (VM-10)"
    );
    assert_eq!(
        all_block_ids(&app, &page, cx),
        before,
        "every block id survived the move"
    );
    assert_eq!(
        undo_depth(&app, cx),
        depth + 2,
        "`dd` and `p` are one undo step each"
    );
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[0].clone()),
        "the cursor is on the block that moved"
    );
    let backlinks = app.update(cx, |app, _| {
        app.store.block_backlinks(&child).unwrap().len()
    });
    assert_eq!(backlinks, 1, "the ((reference)) still resolves");
    let _ = other;

    let before_second = all_block_ids(&app, &page, cx);
    cx.simulate_keystrokes("p");
    cx.run_until_parked();
    let after_second = all_block_ids(&app, &page, cx);
    assert_eq!(
        after_second.len(),
        before_second.len() + 2,
        "two new blocks: the first paste moved, the second copies"
    );
    assert!(
        before_second.iter().all(|id| after_second.contains(id)),
        "and the copy did not take the originals' ids"
    );
}

#[allow(non_snake_case)]
#[gpui::test]
fn vm_10_dd_then_P_puts_the_blocks_above(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha", "beta"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    let before = all_block_ids(&app, &page, cx);

    cx.simulate_keystrokes("d d P");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![("alpha".to_string(), 0), ("beta".to_string(), 0)],
        "`P` restores it above the block the cursor is on"
    );
    assert_eq!(all_block_ids(&app, &page, cx), before, "same ids");
    assert_eq!(target_block(&ws, cx), Some(ids[0].clone()));
}

#[gpui::test]
fn vm_10_yy_then_p_copies_a_todo(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["done thing", "anchor"]);
        s.set_block_kind(&page, &ids[0], BlockKind::Todo, true)
            .unwrap();
        seed_child(s, &page, &ids[0], "child");
        seeded.replace(Some((page, ids)));
    });
    let (page, ids) = seeded.into_inner().expect("seeded");

    cx.simulate_keystrokes("y y j j p");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("done thing".to_string(), 0),
            ("child".to_string(), 1),
            ("anchor".to_string(), 0),
            ("done thing".to_string(), 0),
            ("child".to_string(), 1)
        ],
        "a copy, new ids, after the anchor"
    );
    let copy = live_blocks(&app, &page, cx)[3].clone();
    assert_ne!(copy, ids[0], "a copy, not the original");
    assert_eq!(
        kind_of(&app, &page, &copy, cx),
        Some((BlockKind::Todo, true)),
        "a checked to-do is still a checked to-do (VM-10)"
    );
    let original = live_blocks(&app, &page, cx)[0].clone();
    assert_eq!(original, ids[0], "the original did not move or change");
}

#[gpui::test]
fn vm_8_3dd_is_one_undo_step_and_tombstones_nine_blocks(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["r0", "r1", "r2", "kept"]);
        for root in &ids[..3] {
            seed_child(s, &page, root, "kid a");
            seed_child(s, &page, root, "kid b");
        }
        seeded.replace(Some((page, ids)));
    });
    let (page, ids) = seeded.into_inner().expect("seeded");
    assert_eq!(rows(&app, &page, cx).len(), 10, "3 x (1 + 2) + 1");
    let before = all_block_ids(&app, &page, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("3 d d");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![("kept".to_string(), 0)],
        "the three roots and their six children went"
    );
    assert_eq!(
        undo_depth(&app, cx),
        depth + 1,
        "`3dd` is ONE undo step, not three (VM-8)"
    );
    assert_eq!(
        all_block_ids(&app, &page, cx),
        before,
        "nine tombstones, none hard-deleted (rule 5)"
    );
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[3].clone()),
        "the next block"
    );

    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx).len(),
        10,
        "one undo brings all nine back"
    );
    assert_eq!(undo_depth(&app, cx), depth);
}

#[gpui::test]
fn vm_8_join_tombstones_the_joined_block(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, _ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["one", "two", "three"]);
        let kid = seed_child(s, &page, &ids[1], "under two");
        seeded.replace(Some((page, ids, kid)));
    });
    let (page, ids, kid) = seeded.into_inner().expect("seeded");
    let before = all_block_ids(&app, &page, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("J");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("one two".to_string(), 0),
            ("under two".to_string(), 1),
            ("three".to_string(), 0)
        ],
        "the next block's text joined on with a space, its child re-parented"
    );
    assert_eq!(undo_depth(&app, cx), depth + 1, "`J` is one undo step");
    assert_eq!(
        all_block_ids(&app, &page, cx),
        before,
        "the joined block is a tombstone, not a purge (rule 5)"
    );
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        assert!(
            st.block(&ids[1]).unwrap().deleted_at.is_some(),
            "tombstoned"
        );
        assert_eq!(
            st.block(&ids[1]).unwrap().text,
            "two",
            "its text is not lost"
        );
        assert_eq!(
            st.parent_of(&kid),
            Some(ids[0].clone()),
            "its child moved up"
        );
    });
}

#[gpui::test]
fn vm_8_indent_and_outdent_act_on_the_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["a", "b"]);
    });
    let page = today_page(&app, cx);
    let ids = live_blocks(&app, &page, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("j > >");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("a".to_string(), 0), ("b".to_string(), 1)],
        "`>>` indents the block under the caret"
    );
    assert_eq!(undo_depth(&app, cx), depth + 1, "one undo step");
    assert_eq!(
        target_block(&ws, cx),
        Some(ids[1].clone()),
        "the caret stayed"
    );

    cx.simulate_keystrokes("< <");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("a".to_string(), 0), ("b".to_string(), 0)],
        "`<<` outdents it again"
    );
    assert_eq!(undo_depth(&app, cx), depth + 2, "one undo step each");
}

#[gpui::test]
fn vm_11_a_block_yank_writes_the_clipboard_and_p_prefers_it_when_it_changed(
    cx: &mut TestAppContext,
) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["parent", "anchor"]);
        seed_child(s, &page, &ids[0], "child");
    });
    let page = today_page(&app, cx);

    cx.simulate_keystrokes("y y");
    cx.run_until_parked();
    assert_eq!(
        clipboard(cx),
        "- parent\n  - child\n",
        "a yank puts the blocks on the clipboard as nested Markdown (VM-11)"
    );
    assert!(
        matches!(utopia_ui::vim::last(), Register::Blocks { ref markdown, .. } if markdown == "- parent\n  - child\n"),
        "and in the block register"
    );

    cx.simulate_keystrokes("j j p");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("parent".to_string(), 0),
            ("child".to_string(), 1),
            ("anchor".to_string(), 0),
            ("parent".to_string(), 0),
            ("child".to_string(), 1)
        ]
    );

    cx.write_to_clipboard(gpui::ClipboardItem::new_string("XYZ".to_string()));
    cx.simulate_keystrokes("0 p");
    cx.run_until_parked();
    assert_eq!(
        field_text(&ws, cx),
        "pXYZarent",
        "the changed clipboard is what `p` pastes in"
    );
    assert_eq!(
        rows(&app, &page, cx)[3].0,
        "pXYZarent",
        "and it reached the block"
    );
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".to_string()));
    cx.simulate_keystrokes("j 0 d d");
    cx.run_until_parked();
    assert_eq!(clipboard(cx), "sentinel", "`dd` writes the register only");
    assert!(
        matches!(utopia_ui::vim::last(), Register::Blocks { .. }),
        "but the register has the blocks"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_11_x_then_p_pastes_the_register_even_with_an_external_clipboard(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["ab"]);
    });
    let page = today_page(&app, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("elsewhere".to_string()));
    cx.simulate_keystrokes("x p");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "ba",
        "`x` then `p` is a character move, not the other app's text (F2)"
    );
    assert_eq!(
        clipboard(cx),
        "elsewhere",
        "and `x` did not touch the clipboard"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_11_dw_and_cw_then_p_paste_the_register(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta", "alpha beta"]);
    });
    let page = today_page(&app, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("external".to_string()));

    cx.simulate_keystrokes("d w P");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "alpha beta",
        "`dw` then `P` puts `alpha` back where it was, not the clipboard"
    );

    cx.simulate_keystrokes("j 0 c w X escape 0 P");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[1].0,
        "alphaX beta",
        "`cw` then `P` restores what the change took (F2)"
    );
    assert_eq!(clipboard(cx), "external", "neither one wrote the clipboard");
    let _ = ws;
}

#[gpui::test]
fn vm_10_dd_then_p_is_a_move_even_with_an_external_clipboard(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha", "beta"]);
    });
    let page = today_page(&app, cx);
    let before = all_block_ids(&app, &page, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
        "from a browser".to_string(),
    ));

    cx.simulate_keystrokes("d d P");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &page, cx),
        vec![("alpha".to_string(), 0), ("beta".to_string(), 0)],
        "the block came back above `beta`"
    );
    assert_eq!(
        all_block_ids(&app, &page, cx),
        before,
        "with its own id: a move, not a copy (VM-10, F2)"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_11_a_copy_made_after_a_cut_beats_the_cut(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha", "beta"]);
    });
    let page = today_page(&app, cx);
    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx), vec![("beta".to_string(), 0)]);

    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
        "https://example.com".to_string(),
    ));
    cx.simulate_keystrokes("$ p");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("betahttps://example.com".to_string(), 0)],
        "the clipboard that changed after the cut is what `p` pastes (F6), and the cut did \
         not come back as a move"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_11_a_cut_undone_then_an_external_copy_pastes_the_copy(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha", "beta"]);
    });
    let page = today_page(&app, cx);
    let before = all_block_ids(&app, &page, cx);
    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx), vec![("beta".to_string(), 0)]);
    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("alpha".to_string(), 0), ("beta".to_string(), 0)],
        "the undo brought the block back"
    );

    cx.write_to_clipboard(gpui::ClipboardItem::new_string("later".to_string()));
    cx.simulate_keystrokes("0 $ p");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("alphalater".to_string(), 0), ("beta".to_string(), 0)],
        "the newer clipboard is what `p` pastes (F6)"
    );
    assert_eq!(
        all_block_ids(&app, &page, cx).len(),
        before.len(),
        "and no copy of the cut was made: one new *block* would show here"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_5_cw_on_a_words_last_character_takes_only_that_word(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha beta", "alpha beta", "alpha beta"]);
    });
    let page = today_page(&app, cx);
    for (row, col) in [(0usize, 0usize), (1, 2), (2, 4)] {
        let mut keys = String::new();
        if row > 0 {
            keys.push_str("j ");
        }
        keys.push_str("0 ");
        for _ in 0..col {
            keys.push_str("l ");
        }
        cx.simulate_keystrokes(&keys);
        cx.run_until_parked();
        assert_eq!(cursor(&ws, cx), col, "the caret is on character {col}");
        cx.simulate_keystrokes("c w");
        cx.run_until_parked();
        cx.simulate_input("X");
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
    }
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("X beta".to_string(), 0),
            ("alX beta".to_string(), 0),
            ("alphX beta".to_string(), 0),
        ],
        "`cw` takes the rest of the word under the cursor and leaves `beta` (F5)"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_12_cmd_z_in_insert_takes_the_run_and_reopens_it(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["x"]);
    });
    let page = today_page(&app, cx);
    let depth = undo_depth(&app, cx);
    let editor = editor(&ws, cx);

    cx.simulate_keystrokes("A");
    app.update(cx, |app, _| app.store.set_clock(Some(1_000)));
    cx.simulate_input("a");
    app.update(cx, |app, _| app.store.set_clock(Some(2_000)));
    cx.simulate_input("b");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "xab");

    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "x",
        "the undo took the whole run, not one keystroke of it"
    );
    assert_eq!(undo_depth(&app, cx), depth);
    assert!(
        app.update(cx, |app, _| app.store.in_undo_run()),
        "a run is open again for the rest of the Insert session (F7)"
    );
    assert!(
        editor.read_with(cx, |e, _| e.vim().insert_run),
        "and the editor's flag agrees with the store"
    );

    app.update(cx, |app, _| app.store.set_clock(Some(5_000)));
    cx.simulate_input("c");
    app.update(cx, |app, _| app.store.set_clock(Some(9_000)));
    cx.simulate_input("d");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "xcd");
    assert_eq!(
        undo_depth(&app, cx),
        depth + 1,
        "the second part of the session is one step (VM-12, F7)"
    );
}

#[gpui::test]
fn vm_11_a_delete_names_its_register(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["ab", "cd"]);
    });
    let page = today_page(&app, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("keep".to_string()));

    cx.simulate_keystrokes("\" + x");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "b");
    assert_eq!(
        clipboard(cx),
        "a",
        "`\"+x` writes the clipboard register (F9)"
    );

    cx.simulate_keystrokes("\" _ x");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "");
    assert!(
        matches!(utopia_ui::vim::last(), Register::Chars(ref t) if t == "a"),
        "`\"_x` writes no register (VM-11)"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_8_a_cut_names_its_register(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["parent", "anchor"]);
        seed_child(s, &page, &ids[0], "child");
    });
    let page = today_page(&app, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("keep".to_string()));

    cx.simulate_keystrokes("\" + d d");
    cx.run_until_parked();
    assert_eq!(
        clipboard(cx),
        "- parent\n  - child\n",
        "`\"+dd` writes the clipboard register (F9)"
    );
    assert_eq!(rows(&app, &page, cx), vec![("anchor".to_string(), 0)]);

    cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".to_string()));
    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert_eq!(
        clipboard(cx),
        "sentinel",
        "`dd` on its own leaves the clipboard alone (VM-11)"
    );
    let _ = ws;
}

#[allow(non_snake_case)]
#[gpui::test]
fn vm_8_3D_takes_three_lines(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["one\ntwo\nthree\nfour"]);
    });
    let page = today_page(&app, cx);
    cx.simulate_keystrokes("3 D");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "\nfour",
        "`3D` takes the first three lines and leaves the fourth (F9)"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_11_pasting_multiline_clipboard_text_makes_blocks(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["anchor"]);
    });
    let page = today_page(&app, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
        "one\n  two\nthree".to_string(),
    ));
    cx.simulate_keystrokes("p");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("anchor".to_string(), 0),
            ("one".to_string(), 0),
            ("two".to_string(), 1),
            ("three".to_string(), 0)
        ],
        "line breaks paste as an outline, as Cmd+V does"
    );
    let _ = ws;
}

#[gpui::test]
fn vm_8_a_scripted_block_session_leaves_doctor_clean(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["a", "b", "c", "d"]);
        seed_child(s, &page, &ids[0], "a1");
        let other = s.create_page("Other").unwrap();
        s.execute(
            &other,
            vec![Command::InsertBlock {
                id: BlockId::new(),
                parent: None,
                after: None,
                text: format!("(({})) and (({}))", ids[0], ids[1]),
                kind: BlockKind::Todo,
            }],
            Default::default(),
        )
        .unwrap();
        seeded.replace(Some((page, ids, other)));
    });
    let (page, ids, other) = seeded.into_inner().expect("seeded");
    let before = all_block_ids(&app, &page, cx);

    for keys in [
        "y y", "j p", "d d", "p", "J", "j > >", "< <", "3 d d", "p", "j p",
    ] {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        app.update(cx, |app, _| {
            let report = app.store.doctor().unwrap();
            assert!(report.is_ok(), "rule 2 after `{keys}`\n{report:?}");
        });
    }
    cx.simulate_keystrokes("escape escape");
    cx.run_until_parked();

    let after = all_block_ids(&app, &page, cx);
    for id in &before {
        assert!(
            after.contains(id),
            "id {id} left the graph: nothing here hard-deletes (rule 5)"
        );
    }
    let refs = app.update(cx, |app, _| {
        (
            app.store.block_backlinks(&ids[0]).unwrap().len(),
            app.store.block_backlinks(&ids[1]).unwrap().len(),
        )
    });
    assert_eq!(refs, (1, 1), "both ((references)) still resolve");
    let _ = (ws, other);
    app.update(cx, |app, _| {
        let report = app.store.doctor().unwrap();
        assert!(report.is_ok(), "rule 2 at the end\n{report:?}");
    });
}

#[gpui::test]
fn vm_8_a_reference_text_object_is_pr_4_and_writes_nothing(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["see [[Alpha]] now", "see #tag now"]);
        s.create_page("Alpha").unwrap();
    });
    let page = today_page(&app, cx);
    let before = rows(&app, &page, cx);
    let depth = undo_depth(&app, cx);

    cx.simulate_keystrokes("0 5 l d i l");
    cx.run_until_parked();
    cx.simulate_keystrokes("d a l");
    cx.run_until_parked();

    assert_eq!(rows(&app, &page, cx), before, "no text changed");
    assert_eq!(undo_depth(&app, cx), depth, "and no undo step");
    assert_eq!(mode(&ws, cx), Mode::Normal);

    cx.simulate_keystrokes("c i l");
    cx.run_until_parked();
    assert_eq!(
        mode(&ws, cx),
        Mode::Normal,
        "`cil` stays in Normal until PR 4"
    );
    assert_eq!(rows(&app, &page, cx), before);
}

#[gpui::test]
fn vm_10_dd_then_p_on_another_page_keeps_the_ids(cx: &mut TestAppContext) {
    let seeded = std::cell::RefCell::new(None);
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        let (page, ids) = seed_roots(s, &["alpha", "beta"]);
        let child = seed_child(s, &page, &ids[0], "alpha child");
        let gone = seed_child(s, &page, &ids[0], "a tombstoned child");
        let last = seed_child(s, &page, &ids[0], "alpha last child");
        s.execute(
            &page,
            vec![
                Command::MoveBlock {
                    id: gone.clone(),
                    parent: Some(ids[0].clone()),
                    after: Some(child.clone()),
                },
                Command::MoveBlock {
                    id: last.clone(),
                    parent: Some(ids[0].clone()),
                    after: Some(gone.clone()),
                },
                Command::SetDeleted {
                    ids: vec![gone.clone()],
                    deleted_at: Some(1),
                },
            ],
            Default::default(),
        )
        .unwrap();
        let other = s.create_page("Other").unwrap();
        let anchor = BlockId::new();
        s.execute(
            &other,
            vec![Command::InsertBlock {
                id: anchor.clone(),
                parent: None,
                after: None,
                text: "here".to_string(),
                kind: BlockKind::Bullet,
            }],
            Default::default(),
        )
        .unwrap();
        seeded.replace(Some((page, ids, child, other, anchor)));
    });
    let (page, ids, child, other, anchor) = seeded.into_inner().expect("seeded");
    let before = all_block_ids(&app, &page, cx);

    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx), vec![("beta".to_string(), 0)]);

    let other2 = other.clone();
    ws.update_in(cx, |w, window, cx| {
        w.open(other2, None, None, false, window, cx)
    });
    cx.run_until_parked();
    let selector: &'static str = Box::leak(format!("text-{}", anchor.0).into_boxed_str());
    let row = cx.debug_bounds(selector).expect("the destination row");
    cx.simulate_click(row.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(target_block(&ws, cx), Some(anchor.clone()));

    cx.simulate_keystrokes("p");
    cx.run_until_parked();

    assert_eq!(
        rows(&app, &other, cx),
        vec![
            ("here".to_string(), 0),
            ("alpha".to_string(), 0),
            ("alpha child".to_string(), 1),
            ("alpha last child".to_string(), 1)
        ],
        "the blocks were restored and moved into this page, children in order (VM-10, F1)"
    );
    app.update(cx, |app, _| {
        let report = app.store.doctor().unwrap();
        assert!(report.is_ok(), "rule 2 after the paste: {report:?}");
    });
    assert_eq!(
        rows(&app, &page, cx),
        vec![("beta".to_string(), 0)],
        "and left the page they were on"
    );
    assert_eq!(
        all_block_ids(&app, &page, cx),
        before,
        "every id is still in the source shard, and the moved ones kept theirs"
    );
    assert_eq!(
        app.update(cx, |app, _| app.store.block_page(&ids[0]).unwrap()),
        other,
        "the moved block resolves to the page it moved to"
    );
    assert_eq!(
        app.update(cx, |app, _| app.store.block_page(&child).unwrap()),
        other,
        "the subtree came too"
    );
}

#[gpui::test]
fn vm_10_p_after_an_undone_cut_copies(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup_seeded(cx, true, |s| {
        seed_roots(s, &["alpha", "beta"]);
    });
    let page = today_page(&app, cx);
    let before = all_block_ids(&app, &page, cx);

    cx.simulate_keystrokes("d d");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx), vec![("beta".to_string(), 0)]);
    cx.simulate_keystrokes("u");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![("alpha".to_string(), 0), ("beta".to_string(), 0)],
        "the undo brought the block back"
    );

    cx.simulate_keystrokes("j p");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx),
        vec![
            ("alpha".to_string(), 0),
            ("beta".to_string(), 0),
            ("alpha".to_string(), 0)
        ],
        "a copy, not a move"
    );
    let after = all_block_ids(&app, &page, cx);
    assert_eq!(after.len(), before.len() + 1, "a new id was minted");
    assert!(before.iter().all(|id| after.contains(id)));
    let _ = ws;
}
