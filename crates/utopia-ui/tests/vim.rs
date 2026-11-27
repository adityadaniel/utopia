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
use utopia_ui::vim::{Mode, Resume};
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
