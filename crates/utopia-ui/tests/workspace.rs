//! End-to-end UI scripts (§4.3 workflows) driving the real workspace through GPUI's
//! headless test platform: keystrokes in, store state out.

use chrono::NaiveDate;
use gpui::{
    point, px, size, AppContext as _, Entity, Modifiers, MouseButton, TestAppContext,
    VisualTestContext,
};
use utopia_core::PageId;
use utopia_store::Store;
use utopia_ui::actions::install_keymap;
use utopia_ui::app_state::{AppState, IndexUpdated};
use utopia_ui::pane_stack::{Click, Origin};
use utopia_ui::theme::Theme;
use utopia_ui::workspace::Workspace;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
}

fn setup(cx: &mut TestAppContext) -> (Entity<AppState>, Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today()));
    let app = cx.new(|_| AppState::new(store));
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    (app, ws, cx)
}

/// Today's page as shown: live once written, a draft before that (FR-1.2).
fn today_page(app: &Entity<AppState>, cx: &mut VisualTestContext) -> PageId {
    app.update(cx, |app, _| app.store.draft_daily(today()).unwrap())
}

fn daily_count(app: &Entity<AppState>, cx: &mut VisualTestContext) -> usize {
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        app.store
            .list_pages(true)
            .unwrap()
            .into_iter()
            .filter(|p| p.is_daily())
            .count()
    })
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

fn rows_str(r: &[(String, usize)]) -> Vec<(&str, usize)> {
    r.iter().map(|(t, d)| (t.as_str(), *d)).collect()
}

#[gpui::test]
fn w1_capture_into_the_day(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    // AC-1.1: the caret is already in today's first block.
    cx.simulate_input("Met [[Dana]] about #project");
    // No page matches "project" yet, so no suggestion is open and Enter splits.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    app.update(cx, |app, _| {
        assert!(
            app.store.find_page("Dana").unwrap().is_some(),
            "pages exist on commit"
        );
        assert!(app.store.find_page("project").unwrap().is_some());
        let dana = app.store.find_page("Dana").unwrap().unwrap();
        assert_eq!(app.store.backlink_count(&dana).unwrap(), 1);
    });
}

#[gpui::test]
fn outliner_keyboard_ergonomics(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("b");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("a", 0), ("b", 1)]);
    cx.simulate_keystrokes("shift-tab");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("a", 0), ("b", 0)]);
    // Enter mid-block splits; Backspace at offset 0 merges back (FR-2.5).
    cx.simulate_keystrokes("left");
    cx.simulate_input("c");
    cx.simulate_keystrokes("left enter");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("a", 0), ("", 0), ("cb", 0)]
    );
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("a", 0), ("cb", 0)]);
    // Up moves to the previous block; typing lands there.
    cx.simulate_keystrokes("up");
    cx.simulate_input("!");
    cx.run_until_parked();
    let r = rows(&app, &page, cx);
    assert!(r[0].0.contains('!'), "{r:?}");
    // Cmd+Shift+Up/Down reorders.
    cx.simulate_keystrokes("cmd-shift-down");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[1].0, r[0].0);
}

#[gpui::test]
fn undo_redo_from_the_editor(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("one");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("two");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("one", 0), ("", 0)]);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("one", 0)]);
    cx.simulate_keystrokes("cmd-shift-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    // The caret came back after the undos: typing still lands in the page.
    cx.simulate_input("x");
    cx.run_until_parked();
    let r = rows(&app, &page, cx);
    assert!(r.iter().any(|(t, _)| t.contains('x')), "{r:?}");
}

#[gpui::test]
fn tag_suggestion_accepts_then_enter_splits(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("mindfulness").unwrap();
    });
    cx.simulate_input("practice #mind");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("practice #mindfulness", 0)]
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx).len(),
        2,
        "the accepted tag does not reopen the menu"
    );
}

#[gpui::test]
fn autocomplete_inserts_canonical_links(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("Alpha Prime").unwrap();
        app.store.create_page("Modern Religion").unwrap();
    });
    cx.simulate_input("see [[alp");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.simulate_input(" and #mod");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let text = rows(&app, &page, cx)[0].0.clone();
    assert_eq!(text, "see [[Alpha Prime]] and #[[Modern Religion]]");
    // Escape dismisses and leaves the typed text intact (FR-2.17), auto-closed `]]` too.
    cx.simulate_input(" [[Nov");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(rows(&app, &page, cx)[0].0.ends_with("[[Nov]]"));
}

#[gpui::test]
fn cmd_enter_creates_page_from_query(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("[[Brand New");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Brand New]]");
    app.update(cx, |app, _| {
        assert!(app.store.find_page("Brand New").unwrap().is_some())
    });
}

#[gpui::test]
fn quick_jump_creates_and_opens(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    cx.simulate_input("Reading log");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let created = app.update(cx, |app, _| app.store.find_page("Reading log").unwrap());
    assert!(created.is_some());
    let shown = ws.update(cx, |w, cx| w.main_page(cx));
    assert_eq!(shown, created);
    // The caret waits in the new page's first block (found by screenshot: typing was lost).
    cx.simulate_input("first thought");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let page = created.unwrap();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("first thought", 0)]);
}

/// QJ-1: with nothing typed the palette offers today, yesterday and the pages opened
/// last, and picking one opens it. The list is local UI state (rule 9), not the index.
#[gpui::test]
fn quick_jump_empty_state_lists_days_and_recent_pages(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-icon-0-day").is_some(),
        "today is the first row"
    );
    assert!(
        cx.debug_bounds("qj-icon-1-day").is_some(),
        "yesterday is the second"
    );
    assert!(
        cx.debug_bounds("qj-row-2").is_none(),
        "nothing has been opened yet"
    );
    // Create a page through the palette, then leave it: it becomes the first recent page.
    cx.simulate_input("Reading log");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    let reading = app
        .update(cx, |app, _| app.store.find_page("Reading log").unwrap())
        .unwrap();
    app.update(cx, |app, _| {
        let stored = app
            .store
            .local_get("recent_pages")
            .unwrap()
            .unwrap_or_default();
        assert_eq!(
            stored.lines().next(),
            Some(reading.as_str()),
            "newest first: {stored:?}"
        );
    });
    // Go elsewhere, then reopen the palette: the page is offered by name, not by query.
    cx.simulate_keystrokes("cmd-shift-t");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-icon-2-page").is_some(),
        "the recent page follows today and yesterday"
    );
    cx.simulate_keystrokes("down down enter");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(reading),
        "Enter opens the highlighted recent page"
    );
}

/// QJ-2: a page that only a `#tag` ever reached is labelled a tag page.
#[gpui::test]
fn quick_jump_marks_tag_pages(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_input("reading #atlas");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    assert!(app
        .update(cx, |app, _| app.store.find_page("atlas").unwrap())
        .is_some());
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    cx.simulate_input("atlas");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-icon-0-tag").is_some(),
        "a page reached only through #atlas is a tag page"
    );
    // A page that also has a `[[…]]` link is a page, not a tag (FR-2.15).
    app.update(cx, |app, _| {
        app.store.create_page("Atlas launch").unwrap();
    });
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    cx.simulate_input("atlas launch");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-icon-0-page").is_some(),
        "a page nobody tagged is a plain page"
    );
}

/// QJ-5: `>` lists the actions in the catalog, and picking one runs it as its key would.
#[gpui::test]
fn quick_jump_runs_commands(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // Somewhere to come back from: today's own page, then the feed behind the palette.
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    cx.simulate_input("Reading log");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    let reading = app
        .update(cx, |app, _| app.store.find_page("Reading log").unwrap())
        .unwrap();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(reading));
    // " > " lists commands instead of pages; "Today" runs GoToday.
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    cx.simulate_input(">Today");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-icon-0-command").is_some(),
        "the first match is a command row"
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let shown = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    let date = app.update(cx, |app, _| {
        app.store.page_state(&shown).unwrap().meta.daily_date
    });
    assert_eq!(date, Some(today()), "GoToday ran from the palette");
    // The palette is gone and focus is back on the page: typing lands in today's block.
    cx.simulate_input("ran from the palette");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let page = today_page(&app, cx);
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("ran from the palette", 0)]
    );
}

/// QJ-5: a command bound to the editor acts on the block being edited. The palette takes
/// the focus, but the block stays in edit, so the command is dispatched back to it.
#[gpui::test]
fn quick_jump_runs_editor_commands(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("b");
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    cx.simulate_input(">Indent");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("a", 0), ("b", 1)],
        "Indent ran on the block that was being edited"
    );
}

/// QJ-5: an editor command with no block in edit cannot act, so it is shown as unavailable
/// rather than offered and silently dropped.
#[gpui::test]
fn quick_jump_marks_editor_commands_unavailable(cx: &mut TestAppContext) {
    let (_app, _ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    cx.simulate_input(">Indent");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-command-0-Indent-off").is_some(),
        "Indent needs a block in edit, and none is"
    );
    // A command that works anywhere stays available.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input(">Today");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-command-0-GoToday").is_some(),
        "GoToday is a window action"
    );
}

/// QJ-1: a page that was opened and then deleted (or merged away, which tombstones the
/// source) is skipped, and its entry does not use up one of the eight recent slots.
#[gpui::test]
fn quick_jump_skips_deleted_recent_pages(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    fn open(ws: &Entity<Workspace>, cx: &mut VisualTestContext, page: PageId) {
        ws.update_in(cx, |w, window, cx| {
            w.open(page, None, None, false, window, cx)
        });
    }
    let (reading, trips) = app.update(cx, |app, _| {
        (
            app.store.create_page("Reading log").unwrap(),
            app.store.create_page("Trips").unwrap(),
        )
    });
    open(&ws, cx, reading.clone());
    open(&ws, cx, trips);
    cx.run_until_parked();
    app.update(cx, |app, _| app.store.delete_page(&reading).unwrap());
    // The UI store is debounced; let the writer catch up so the index is current.
    app.update(cx, |app, _| app.store.sync_index().unwrap());
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-row-2").is_some(),
        "the live page is offered"
    );
    assert!(
        cx.debug_bounds("qj-row-3").is_none(),
        "the deleted page is not offered"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // Eight live pages behind a tombstoned one: none of the eight may be displaced, so
    // nine downs from the top land on the oldest of them.
    let live: Vec<PageId> = app.update(cx, |app, _| {
        (0..8)
            .map(|i| app.store.create_page(&format!("Page {i}")).unwrap())
            .collect()
    });
    for page in &live {
        open(&ws, cx, page.clone());
    }
    let doomed = app.update(cx, |app, _| app.store.create_page("Doomed").unwrap());
    open(&ws, cx, doomed.clone());
    cx.run_until_parked();
    app.update(cx, |app, _| app.store.delete_page(&doomed).unwrap());
    app.update(cx, |app, _| app.store.sync_index().unwrap());
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    cx.simulate_keystrokes("down down down down down down down down down enter");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(live[0].clone()),
        "the tombstone did not take a slot"
    );
}

/// QJ-5: the palette lists the keys in force. A binding changed in Settings replaces the
/// default, so the palette must find (and show) the new chord, not the old one (§9.1).
#[gpui::test]
fn quick_jump_finds_commands_by_the_keys_in_force(cx: &mut TestAppContext) {
    let (_app, _ws, cx) = setup(cx);
    cx.update(|_window, cx| {
        let mut o = utopia_ui::actions::Overrides::default();
        o.set("GoToday", "alt-cmd-t");
        utopia_ui::actions::reinstall(cx, &o);
    });
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    cx.simulate_input(">alt-cmd-t");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-command-0-GoToday").is_some(),
        "the rebound chord finds Today"
    );
}

/// The flip side in a fresh window, so no earlier frame can leave a stale debug bound:
/// once rebound, the replaced default is no longer advertised or searched.
#[gpui::test]
fn quick_jump_drops_a_replaced_default_key(cx: &mut TestAppContext) {
    let (_app, _ws, cx) = setup(cx);
    cx.update(|_window, cx| {
        let mut o = utopia_ui::actions::Overrides::default();
        o.set("GoToday", "alt-cmd-t");
        utopia_ui::actions::reinstall(cx, &o);
    });
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    cx.simulate_input(">cmd-shift-t");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("qj-command-0-GoToday").is_none(),
        "no command answers to the replaced default any more"
    );
}

/// P3: a page ⌘-clicked open as a pane (once a split pane) was still opened, so it belongs
/// in the recent list the empty state offers.
#[gpui::test]
fn quick_jump_records_pages_opened_as_a_pane(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let reading = app.update(cx, |app, _| app.store.create_page("Reading log").unwrap());
    ws.update_in(cx, |w, window, cx| {
        w.open_from(
            Origin::Pane(0),
            Click::Secondary,
            reading.clone(),
            None,
            None,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    app.update(cx, |app, _| {
        let stored = app
            .store
            .local_get("recent_pages")
            .unwrap()
            .unwrap_or_default();
        assert_eq!(
            stored.lines().next(),
            Some(reading.as_str()),
            "a pane is an open: {stored:?}"
        );
    });
}

#[gpui::test]
fn query_blocks_list_results(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_input("Read [[Dune]] #book");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Watch [[Dune]]");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("{{query: {and: [[Dune]] #book}}}");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("query-header").is_some(),
        "results header drawn"
    );
    let header = cx.debug_bounds("query-header").unwrap();
    let row3 = cx.debug_bounds("content-3").unwrap();
    assert!(
        header.origin.y > row3.origin.y,
        "results sit under the block"
    );
    // Results follow edits: un-tagging the first block empties the query.
    let page = today_page(&app, cx);
    app.update(cx, |app, cx| {
        let first = app.store.page_state(&page).unwrap().live_roots()[0].clone();
        app.store
            .set_block_text(&page, &first, "Read [[Dune]]", None, None)
            .unwrap();
        app.store.commit_block(&page, &first).unwrap();
        app.changed(vec![page.clone()], false, cx);
    });
    cx.run_until_parked();
    let r = app.update(cx, |app, _| {
        let e = utopia_core::query::parse("{and: [[Dune]] #book}").unwrap();
        app.store.run_query(&e, 10).unwrap()
    });
    assert_eq!(r.total, 0);
}

#[gpui::test]
fn day_navigation(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // From the feed, with the caret still in today's block.
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    let shown = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    // A draft until written (FR-1.2), so read the shard, not the index.
    let date = app.update(cx, |app, _| {
        app.store.page_state(&shown).unwrap().meta.daily_date
    });
    assert_eq!(date, NaiveDate::from_ymd_opt(2026, 11, 1));
    cx.simulate_keystrokes("cmd-shift-d");
    cx.run_until_parked();
    let shown = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    assert_eq!(shown, today_page(&app, cx));
}

/// The three date shortcuts (§9.1): Daily Note is the whole feed, while Today and
/// Yesterday open that one day on its own.
#[gpui::test]
fn date_shortcuts(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let in_feed = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
        !ws.update(cx, |w, cx| w.journal_days(cx)).is_empty()
    };
    let shown_date =
        |app: &Entity<AppState>, ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
            let p = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
            app.update(cx, |app, _| {
                app.store.page_state(&p).unwrap().meta.daily_date
            })
        };
    assert!(in_feed(&ws, cx), "the launch view is the feed (FR-1.1)");

    // Today leaves the feed for the single day.
    cx.simulate_keystrokes("cmd-shift-t");
    cx.run_until_parked();
    assert!(!in_feed(&ws, cx), "Today opens one day, not the feed");
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)).unwrap(),
        today_page(&app, cx)
    );

    cx.simulate_keystrokes("cmd-shift-y");
    cx.run_until_parked();
    assert!(!in_feed(&ws, cx), "Yesterday opens one day");
    assert_eq!(
        shown_date(&app, &ws, cx),
        NaiveDate::from_ymd_opt(2026, 11, 1),
        "yesterday"
    );

    // Daily Note brings the feed back.
    cx.simulate_keystrokes("cmd-shift-d");
    cx.run_until_parked();
    assert!(in_feed(&ws, cx), "Daily Note returns to the feed");
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)).unwrap(),
        today_page(&app, cx)
    );

    // Neither shortcut wrote a page: a day is a draft until it is typed in (FR-1.2).
    assert_eq!(daily_count(&app, cx), 0, "visiting a day creates nothing");
}

#[gpui::test]
fn marks_and_soft_breaks(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("**bold");
    cx.simulate_keystrokes("cmd-b");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("line two");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "**bold**\nline two");
}

#[gpui::test]
fn zoom_collapse_and_delete(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("parent");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab up");
    cx.run_until_parked();
    // Cmd+. collapses locally; nothing changes in the page's content.
    cx.simulate_keystrokes("cmd-.");
    cx.run_until_parked();
    let parent = app.update(cx, |app, _| {
        app.store.page_state(&page).unwrap().live_roots()[0].clone()
    });
    assert!(app.read_with(cx, |app, _| app.store.is_collapsed(&parent)));
    cx.simulate_keystrokes("cmd-.");
    cx.run_until_parked();
    // Cmd+Alt+Down zooms into the block; typing lands in its children.
    cx.simulate_keystrokes("cmd-alt-down");
    cx.run_until_parked();
    let zoom = ws.update(cx, |w, cx| w.main_zoom(cx));
    assert_eq!(zoom, Some(parent.clone()));
    cx.simulate_input("!");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("parent", 0), ("!child", 1)]
    );
    // Cmd+Shift+Backspace tombstones the block (recoverable with undo).
    cx.simulate_keystrokes("cmd-shift-backspace");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("parent", 0)]);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("parent", 0), ("!child", 1)]
    );
}

#[gpui::test]
fn block_reference_autocomplete(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a central thesis");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("see ((thes");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let (target, text) = app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        let roots = st.live_roots();
        (roots[0].clone(), st.block(&roots[1]).unwrap().text.clone())
    });
    assert_eq!(text, format!("see (({target}))"));
    // AC-3.2: the edge appears once the block commits (300 ms idle), not per keystroke.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    app.update(cx, |app, _| {
        assert_eq!(app.store.block_backlinks(&target).unwrap().len(), 1);
    });
}

#[gpui::test]
fn sidebar_stacks_pages(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (a, b) = app.update(cx, |app, _| {
        (
            app.store.create_page("Alpha").unwrap(),
            app.store.create_page("Beta").unwrap(),
        )
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(a.clone(), None, None, true, window, cx);
        w.open(b.clone(), None, None, true, window, cx);
        w.open(a.clone(), None, None, true, window, cx);
    });
    cx.run_until_parked();
    let panes = ws.update(cx, |w, cx| w.sidebar_pages(cx));
    assert_eq!(
        panes,
        vec![a, b],
        "re-opening moves a pane to the top, no duplicates"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(today_page(&app, cx))
    );
}

/// RS-1: clicking a reference in the right sidebar opens the page it came from with that
/// block focused, and clicking a page chip inside it follows the chip instead.
#[gpui::test]
fn a_reference_row_in_the_sidebar_opens_its_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (target, source, block, fold) = app.update(cx, |app, _| {
        use utopia_core::{BlockId, BlockKind, Command};
        let target = app.store.create_page("Target").unwrap();
        let source = app.store.create_page("Source").unwrap();
        // The referencing block sits under a folded parent, so opening it has to unfold that
        // parent first.
        let fold = BlockId::new();
        app.store
            .execute(
                &source,
                vec![Command::InsertBlock {
                    id: fold.clone(),
                    parent: None,
                    after: None,
                    text: "Fold".into(),
                    kind: BlockKind::Bullet,
                }],
                Default::default(),
            )
            .unwrap();
        let b = BlockId::new();
        app.store
            .execute(
                &source,
                vec![Command::InsertBlock {
                    id: b.clone(),
                    parent: Some(fold.clone()),
                    after: None,
                    text: "the note that references [[Target]]".into(),
                    kind: BlockKind::Bullet,
                }],
                Default::default(),
            )
            .unwrap();
        app.store.set_collapsed(&fold, true).unwrap();
        app.store.sync_index().unwrap();
        (target, source, b, fold)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-shift-\\");
    cx.run_until_parked();
    let row = (0..20)
        .find_map(|ix| {
            let sel: &'static str = Box::leak(format!("ref-block-{ix}").into_boxed_str());
            cx.debug_bounds(sel)
        })
        .expect("the sidebar lists the reference");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(source),
        "the reference row opened the page it came from"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .target()
            .map(|t| t.block.clone())),
        Some(block),
        "the caret is in the block that made the reference"
    );
    assert!(
        !app.read_with(cx, |app, _| app.store.is_collapsed(&fold)),
        "its folded parent was unfolded to show the row"
    );
}

/// RS-2: an unresolved reference in the sidebar offers "Create page", which makes the page
/// on the spot, and an empty Unresolved tab says so.
#[gpui::test]
fn unresolved_references_can_be_created_from_the_sidebar(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = app.update(cx, |app, _| {
        let p = app.store.create_page("Drafting").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "ask [[Ana Lovelace]] about it", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        // A link creates its page on commit, so the only way to a dangling intent is to
        // delete the page that the link now points at (AC-2.10).
        let ana = app.store.find_page("Ana Lovelace").unwrap().unwrap();
        app.store.delete_page(&ana).unwrap();
        app.store.sync_index().unwrap();
        p
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-shift-\\");
    cx.run_until_parked();
    let unresolved = cx.debug_bounds("tab-un").expect("the Unresolved tab");
    cx.simulate_click(unresolved.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("unresolved-empty").is_none(),
        "there is one unresolved reference, so the tab lists it"
    );
    let create = cx
        .debug_bounds("unresolved-create-0")
        .expect("the row offers Create page");
    cx.simulate_click(create.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    app.update(cx, |app, _| {
        assert!(
            app.store.unresolved_refs(&page).unwrap().is_empty(),
            "the intent resolves now that its page exists (FR-4.14)"
        );
    });

    // A page with nothing dangling says so instead of showing an empty panel.
    let other = app.update(cx, |app, _| app.store.create_page("Clean").unwrap());
    ws.update_in(cx, |w, window, cx| {
        w.open(other, None, None, false, window, cx)
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("unresolved-empty").is_some(),
        "the empty state is drawn for a page with no unresolved references"
    );
}

/// RS-3: the sidebar's Outline tab lists the page's headings and a click jumps to one.
#[gpui::test]
fn the_outline_lists_headings_and_jumps_to_them(cx: &mut TestAppContext) {
    use utopia_core::{BlockKind, Command};
    let (app, ws, cx) = setup(cx);
    let (page, heading) = app.update(cx, |app, _| {
        let p = app.store.create_page("Plan").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "Risks", None, None)
            .unwrap();
        app.store
            .execute(
                &p,
                vec![Command::SetBlockKind {
                    id: b.clone(),
                    kind: BlockKind::Heading(2),
                    checked: false,
                }],
                Default::default(),
            )
            .unwrap();
        let plan = utopia_core::outline::append_block(app.store.page_state(&p).unwrap(), None);
        let plan = app.store.execute_plan(&p, plan, None).unwrap().unwrap();
        app.store
            .set_block_text(&p, &plan.block, "not a heading", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        (p, b)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-shift-\\");
    cx.run_until_parked();
    let outline = cx.debug_bounds("panel-outline").expect("the Outline tab");
    cx.simulate_click(outline.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let heading_row = cx.debug_bounds("outline-0").expect("the heading is listed");
    assert!(
        cx.debug_bounds("outline-1").is_none(),
        "only headings are listed, not the body block below it"
    );
    assert!(
        heading_row.size.height >= gpui::px(utopia_ui::theme::H2_SIZE),
        "a heading draws at the heading size: {heading_row:?}"
    );
    cx.simulate_click(heading_row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .target()
            .map(|t| t.block.clone())),
        Some(heading),
        "clicking the outline row jumped to the heading"
    );
}

/// RS-3: the Pinned tab keeps a page one click away; pinning is local (settings.json,
/// rule 9) and the row can be unpinned again.
#[gpui::test]
fn pinned_pages_can_be_added_and_removed(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = app.update(cx, |app, _| {
        let p = app.store.create_page("Pinboard").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "a page worth keeping", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        p
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-shift-\\");
    cx.run_until_parked();
    let pinned_tab = cx.debug_bounds("panel-pinned").expect("the Pinned tab");
    cx.simulate_click(pinned_tab.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("pinned-empty").is_some(),
        "nothing is pinned at first"
    );
    let pin = cx.debug_bounds("pin-current").expect("the pin control");
    cx.simulate_click(pin.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("pinned-0").is_some(),
        "the page now sits in the Pinned tab"
    );
    assert!(
        ws.update(cx, |w, cx| w.pinned_pages(cx)).len() == 1,
        "the workspace remembers the pin"
    );
    let unpin = cx.debug_bounds("unpin-0").expect("the unpin control");
    cx.simulate_click(unpin.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.pinned_pages(cx)).is_empty(),
        "unpinning removed it"
    );
}

/// RS-5: pages shift-opened in the sidebar stack as cards with a fold arrow and a close ×.
#[gpui::test]
fn sidebar_cards_collapse_and_close(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (alpha, beta) = app.update(cx, |app, _| {
        (
            app.store.create_page("Alpha").unwrap(),
            app.store.create_page("Beta").unwrap(),
        )
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(alpha.clone(), None, None, true, window, cx);
        w.open(beta.clone(), None, None, true, window, cx);
    });
    // Opening a page in the sidebar is what shows it, so no toggle here.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.sidebar_pages(cx)),
        vec![beta.clone(), alpha.clone()],
        "the newest card is on top"
    );
    // Each card names its page.
    assert!(cx.debug_bounds("pane-title-0").is_some());
    assert!(cx.debug_bounds("pane-title-1").is_some());
    let expanded = cx.debug_bounds("pane-0").expect("the top card");
    let fold = cx.debug_bounds("pane-collapse-0").expect("the fold arrow");
    cx.simulate_click(fold.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let folded = cx.debug_bounds("pane-0").expect("the folded card");
    assert!(
        folded.size.height < expanded.size.height && folded.size.height < gpui::px(40.),
        "a folded card is its header: {folded:?} (was {expanded:?})"
    );
    assert!(
        cx.debug_bounds("pane-title-0").is_some(),
        "the header, and so the page's name, stays"
    );

    // The × closes a card; the rest of the stack stays.
    assert_eq!(ws.update(cx, |w, cx| w.sidebar_pages(cx)).len(), 2);
    let close = cx.debug_bounds("pane-close-1").expect("the close ×");
    cx.simulate_click(close.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.sidebar_pages(cx)),
        vec![beta.clone()],
        "closing a card removes just that page"
    );

    // Opening that page again moves its card to the top without a duplicate, still folded.
    ws.update_in(cx, |w, window, cx| {
        w.open(beta.clone(), None, None, true, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.sidebar_pages(cx)), vec![beta]);
    let again = cx.debug_bounds("pane-0").expect("the card again");
    assert!(
        again.size.height < gpui::px(40.),
        "the card was still folded: {again:?}"
    );
}

/// RS-5 + the focus invariant in AGENTS.md: folding the card that hosts the edit must end
/// the edit and hand the keyboard back to the workspace, because a focused element that is
/// no longer rendered gets no keystrokes, so every shortcut would die.
#[gpui::test]
fn folding_an_edited_card_gives_the_keyboard_back(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (blank, card) = app.update(cx, |app, _| {
        // The main pane shows a page with no rows, so the selectors below can only be the
        // card's.
        let blank = app.store.create_page("Blank").unwrap();
        let card = app.store.create_page("Alpha").unwrap();
        for text in ["one", "two", "three"] {
            let st = app.store.page_state(&card).unwrap();
            let after = st.last_child(None, true);
            let plan = utopia_core::outline::append_block(st, after.as_ref());
            let done = app.store.execute_plan(&card, plan, None).unwrap().unwrap();
            app.store
                .set_block_text(&card, &done.block, text, None, None)
                .unwrap();
            app.store.commit_block(&card, &done.block).unwrap();
        }
        app.store.sync_index().unwrap();
        (blank, card)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(blank, None, None, false, window, cx);
        w.open(card, None, None, true, window, cx);
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    let row = cx
        .debug_bounds("content-3")
        .expect("the card's third block");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).target().is_some()),
        "clicking a block in the card started an edit there"
    );

    let fold = cx.debug_bounds("pane-collapse-0").expect("the fold arrow");
    cx.simulate_click(fold.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).target().is_none()),
        "folding the card ended the edit it hosted"
    );

    // The keyboard went back to the workspace: Cmd+Shift+\ still closes the sidebar, and
    // the page takes the width back.
    let before = cx
        .debug_bounds("main-pane")
        .expect("the main pane")
        .size
        .width;
    cx.simulate_keystrokes("cmd-shift-\\");
    cx.run_until_parked();
    let after = cx
        .debug_bounds("main-pane")
        .expect("the main pane")
        .size
        .width;
    assert!(
        after > before,
        "shortcuts still reach the workspace after folding: {before:?} -> {after:?}"
    );
}

/// RS-3: the Outline lists every heading of the page, so clicking one has to reach it even
/// when it sits under a folded block or outside the zoom the page is showing — otherwise
/// the row is a dead control.
#[gpui::test]
fn outline_reaches_headings_that_are_folded_away(cx: &mut TestAppContext) {
    use utopia_core::{BlockId, BlockKind, Command};
    let (app, ws, cx) = setup(cx);
    let (page, fold, hidden, shown) = app.update(cx, |app, _| {
        let page = app.store.create_page("Plan").unwrap();
        let insert = |app: &mut AppState,
                      id: &BlockId,
                      parent: Option<BlockId>,
                      after: Option<BlockId>,
                      text: &str,
                      kind: BlockKind| {
            app.store
                .execute(
                    &page,
                    vec![Command::InsertBlock {
                        id: id.clone(),
                        parent,
                        after,
                        text: text.to_string(),
                        kind,
                    }],
                    Default::default(),
                )
                .unwrap();
        };
        let fold = BlockId::new();
        insert(app, &fold, None, None, "Fold", BlockKind::Bullet);
        let hidden = BlockId::new();
        insert(
            app,
            &hidden,
            Some(fold.clone()),
            None,
            "Hidden heading",
            BlockKind::Heading(2),
        );
        let shown = BlockId::new();
        insert(
            app,
            &shown,
            None,
            Some(fold.clone()),
            "Shown heading",
            BlockKind::Heading(2),
        );
        app.store.set_collapsed(&fold, true).unwrap();
        app.store.sync_index().unwrap();
        (page, fold, hidden, shown)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-shift-\\");
    cx.run_until_parked();
    let outline = cx.debug_bounds("panel-outline").expect("the Outline tab");
    cx.simulate_click(outline.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    // The folded heading is listed and clickable; before the fix the click did nothing.
    let first = cx.debug_bounds("outline-0").expect("the first heading");
    cx.simulate_click(first.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .target()
            .map(|t| t.block.clone())),
        Some(hidden.clone()),
        "clicking the folded heading put the caret there"
    );
    assert!(
        !app.read_with(cx, |app, _| app.store.is_collapsed(&fold)),
        "its ancestor was unfolded so the row could be shown"
    );

    // A heading outside the zoom is reachable too: the page leaves the zoom to show it.
    ws.update_in(cx, |w, window, cx| {
        w.open(page, Some(shown), None, false, window, cx)
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let first = cx.debug_bounds("outline-0").expect("the first heading");
    cx.simulate_click(first.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .target()
            .map(|t| t.block.clone())),
        Some(hidden),
        "clicking a heading outside the zoom left the zoom and focused it"
    );
    assert!(
        ws.update(cx, |w, cx| w.main_zoom(cx)).is_none(),
        "the zoom was cleared"
    );
}

/// FR-3.9 (LR-5): a reveal that leaves the zoom is a navigation like `open`, so the page's
/// unlinked count is fetched before the next frame. `reveal_block` used to clear the zoom
/// without running the navigation reset, so the header drew too late to know the count and
/// the section stayed blank until an unrelated index update arrived.
#[gpui::test]
fn fr_3_9_leaving_a_zoom_to_reveal_a_block_counts_unlinked_mentions(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (page, in_zoom, outside) = app.update(cx, |app, _| {
        let page = app.store.create_page("Mindfulness").unwrap();
        let a = app.store.ensure_first_block(&page).unwrap();
        app.store
            .set_block_text(&page, &a, "Meeting", None, None)
            .unwrap();
        app.store.commit_block(&page, &a).unwrap();
        let plan = utopia_core::outline::append_block(app.store.page_state(&page).unwrap(), None);
        let b = app
            .store
            .execute_plan(&page, plan, None)
            .unwrap()
            .unwrap()
            .block;
        app.store
            .set_block_text(&page, &b, "Risks", None, None)
            .unwrap();
        app.store.commit_block(&page, &b).unwrap();
        // A plain mention of the page's title on another page (FR-3.9).
        let src = app.store.create_page("Reading").unwrap();
        let s = app.store.ensure_first_block(&src).unwrap();
        app.store
            .set_block_text(&src, &s, "practising mindfulness daily", None, None)
            .unwrap();
        app.store.commit_block(&src, &s).unwrap();
        app.store.sync_index().unwrap();
        (page, a, b)
    });
    // Opened zoomed, so the unlinked section was never on screen and never counted.
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), Some(in_zoom), None, false, window, cx)
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("unlinked-count-1").is_none(),
        "a zoomed block has no Unlinked References section"
    );
    let scans = app.read_with(cx, |app, _| app.store.unlinked_scans());

    // The Outline tab would do this: reveal a heading outside the zoom.
    ws.update_in(cx, |w, window, cx| {
        let view = w.main_view();
        view.update(cx, |view, cx| view.reveal_block(&outside, window, cx));
    });
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.main_zoom(cx)).is_none(),
        "revealing a block outside the zoom leaves it"
    );
    assert_eq!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()),
        scans,
        "DESIGN.md §3.7: a collapsed Unlinked references row is not searched"
    );
    let header = cx
        .debug_bounds("unlinked-header")
        .expect("the whole page is back, with its Unlinked references row");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()) > scans,
        "expanding the row runs the search in the click handler, not in a frame"
    );
    cx.debug_bounds("unlinked-count-1")
        .expect("the expanded row shows its count");
}

/// FR-2.7: zooming changes what the header means — the title is the block's text and
/// `start_rename` offers the calendar there instead of a rename. A page's rename input
/// must therefore close with the zoom, as it does when the page changes; it used to stay
/// open over a header it no longer described.
#[gpui::test]
fn fr_2_7_zooming_within_a_page_closes_the_rename_input(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (page, block) = app.update(cx, |app, _| {
        let p = app.store.create_page("Alpha").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "body", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        (p, b)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    let title = cx.debug_bounds("page-title").expect("the page title");
    cx.simulate_click(title.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.main_view().read(cx).is_renaming()),
        "clicking the title starts the rename"
    );

    // The same transition a bullet click or Cmd+Alt+Down makes.
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), Some(block), None, false, window, cx)
    });
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.main_zoom(cx)).is_some(),
        "the page is zoomed into the block"
    );
    assert!(
        !ws.update(cx, |w, cx| w.main_view().read(cx).is_renaming()),
        "the rename input closed with the zoom"
    );
}

/// FR-1.4: clicking a title that cannot be renamed opens the date calendar. That calendar
/// belongs to the header that opened it, so leaving the zoom (the breadcrumb back to the
/// page) closes it; it used to stay open over the ordinary page header.
#[gpui::test]
fn fr_1_4_leaving_a_zoom_closes_the_date_calendar(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (page, block) = app.update(cx, |app, _| {
        let p = app.store.create_page("Day Planner").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "plan", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        (p, b)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), Some(block), None, false, window, cx)
    });
    cx.run_until_parked();
    let title = cx
        .debug_bounds("page-title")
        .expect("the zoomed header's title");
    cx.simulate_click(title.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.main_view().read(cx).calendar_is_open()),
        "a zoomed title opens the date calendar instead of a rename"
    );

    // The breadcrumb's page link, and any other navigation out of the zoom.
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.main_zoom(cx)).is_none(),
        "the view left the zoom"
    );
    assert!(
        !ws.update(cx, |w, cx| w.main_view().read(cx).calendar_is_open()),
        "the calendar closed with the zoom"
    );
}

/// RS-3: a pinned row's title comes from the shard, so renaming the page reaches the
/// Pinned tab without toggling another pin or restarting.
#[gpui::test]
fn a_renamed_pinned_page_shows_its_new_title(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = app.update(cx, |app, _| {
        let p = app.store.create_page("Old Name").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "pinned", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        p
    });
    cx.update(|_, cx| {
        utopia_ui::appearance::toggle_pinned(cx, &page).expect("settings.json is writable")
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-shift-\\");
    cx.run_until_parked();
    let tab = cx.debug_bounds("panel-pinned").expect("the Pinned tab");
    cx.simulate_click(tab.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, _| w.pinned_titles()),
        vec!["Old Name".to_string()]
    );

    // Rename through the store, as the page menu's Rename does, and let the index catch up.
    app.update(cx, |app, cx| {
        app.store.rename_page(&page, "New Name").unwrap();
        app.changed(Vec::new(), true, cx);
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, _| w.pinned_titles()),
        vec!["New Name".to_string()],
        "the pinned row follows the rename"
    );
}

/// LR-4 with RS-1's shared row: a link inside an inline reference child is followed.
/// The child used to be drawn without its own hit-testing, so a click on its link fell
/// through to the row's handler and opened the referencing block instead.
#[gpui::test]
fn a_link_inside_an_inline_reference_child_is_followed(cx: &mut TestAppContext) {
    use utopia_core::{BlockId, BlockKind, Command};
    let (app, ws, cx) = setup(cx);
    let (target, source, other) = app.update(cx, |app, _| {
        let target = app.store.create_page("Target").unwrap();
        let source = app.store.create_page("Source").unwrap();
        let other = app.store.create_page("Other Page").unwrap();
        let parent = app.store.ensure_first_block(&source).unwrap();
        app.store
            .set_block_text(&source, &parent, "see [[Target]]", None, None)
            .unwrap();
        // A child whose whole body is a page link: its first characters are the link.
        let child = BlockId::new();
        app.store
            .execute(
                &source,
                vec![Command::InsertBlock {
                    id: child,
                    parent: Some(parent.clone()),
                    after: None,
                    text: "[[Other Page]]".into(),
                    kind: BlockKind::Bullet,
                }],
                Default::default(),
            )
            .unwrap();
        app.store.commit_block(&source, &parent).unwrap();
        app.store.sync_index().unwrap();
        (target, source, other)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let child = (0..40)
        .find_map(|ix| {
            let sel: &'static str = Box::leak(format!("hit-child-{ix}-0").into_boxed_str());
            cx.debug_bounds(sel)
        })
        .expect("the reference's first child is drawn inline (LR-4)");
    // The child starts with a bullet, then its text; the link is at the start of the text.
    let link = gpui::point(child.origin.x + gpui::px(28. + 16.), child.center().y);
    cx.simulate_click(link, gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(other),
        "the child's link was followed, not the referencing block opened"
    );
    assert_ne!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(source),
        "the row's own target was not opened"
    );
}

#[gpui::test]
fn keystrokes_are_debounced_then_committed(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("[[Later]] typed fast");
    cx.run_until_parked();
    // In memory immediately; the index catches up after the idle flush (§13.3).
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Later]] typed fast");
    assert!(app.read_with(cx, |app, _| app.store.has_pending()));
    let before = app.read_with(cx, |app, _| app.refs_version);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert!(!app.read_with(cx, |app, _| app.store.has_pending()));
    assert!(
        app.read_with(cx, |app, _| app.refs_version) > before,
        "index update announced"
    );
    app.update(cx, |app, _| {
        let later = app
            .store
            .find_page("Later")
            .unwrap()
            .expect("created on idle commit");
        assert_eq!(app.store.backlink_count(&later).unwrap(), 1);
    });
}

#[gpui::test]
fn caret_survives_remote_edits(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("hello world");
    cx.simulate_keystrokes("left left left left left");
    cx.run_until_parked();
    // Another device inserts text at the start of the same block; sync delivers it.
    let block = app.update(cx, |app, _| {
        app.store.page_state(&page).unwrap().live_roots()[0].clone()
    });
    let update = app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        let bytes = app.store.export_since(&page, &Default::default()).unwrap();
        let mut remote = Store::open_in_memory().unwrap();
        remote.import(&page, &bytes).unwrap();
        remote
            .set_block_text(&page, &block, ">> hello world", None, None)
            .unwrap();
        remote.sync_index().unwrap();
        remote.export_since(&page, &Default::default()).unwrap()
    });
    app.update(cx, |app, cx| {
        app.store.import(&page, &update).unwrap();
        app.changed(vec![page.clone()], true, cx);
    });
    cx.run_until_parked();
    // The caret stayed before "world", not at the old byte offset.
    cx.simulate_input("big ");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, ">> hello big world");
}

/// Settings → Sync must describe a sync that exists: a config whose transport cannot be
/// built (here a folder under a regular file) leaves the pane saying sync is off, while one
/// that starts publishes the transport. Main used to publish the enabled facts first.
#[gpui::test]
fn a_sync_that_cannot_start_does_not_claim_to_be_on(cx: &mut TestAppContext) {
    use utopia_ui::sync_runner::{self, SyncConfig, SyncTarget};
    let (_app, ws, cx) = setup(cx);
    let dir = tempfile::tempdir().unwrap();
    // Its parent is a regular file, so the folder transport cannot be built.
    let file = dir.path().join("not-a-folder");
    std::fs::write(&file, b"x").unwrap();
    let bad = SyncConfig {
        target: SyncTarget::Folder(file.join("pages")),
        graph: "g".into(),
        passphrase: None,
    };
    assert!(!ws.update(cx, |w, cx| w.start_sync(bad, cx)));
    assert!(
        !cx.update(|_, cx| sync_runner::facts(cx).is_on()),
        "a failed start must leave Settings saying sync is off"
    );
    assert_eq!(
        cx.update(|_, cx| sync_runner::facts(cx).graph.clone()),
        Some("g".to_string()),
        "the graph is still named"
    );

    let shared = dir.path().join("shared");
    let good = SyncConfig {
        target: SyncTarget::Folder(shared.clone()),
        graph: "g".into(),
        passphrase: None,
    };
    assert!(ws.update(cx, |w, cx| w.start_sync(good, cx)));
    let facts = cx.update(|_, cx| sync_runner::facts(cx));
    assert!(facts.is_on(), "a started sync is reported");
    assert_eq!(
        facts.target.as_deref(),
        Some(format!("Folder · {}", shared.display()).as_str())
    );
}

#[gpui::test]
fn background_sync_imports_and_pushes(cx: &mut TestAppContext) {
    use utopia_sync::relay::{Relay, RelayTransport};
    use utopia_sync::{SyncClient, SyncStatus};
    let (app, ws, cx) = setup(cx);
    let relay = Relay::new();
    ws.update(cx, |w, cx| {
        w.enable_sync_with(Box::new(RelayTransport::new(relay.clone(), "g")), None, cx)
    });
    // Another device publishes a page.
    let mut other = Store::open_in_memory().unwrap();
    let mut oc = SyncClient::new(RelayTransport::new(relay.clone(), "g"), &other).unwrap();
    let remote_page = other.create_page("From the phone").unwrap();
    oc.sync_once(&mut other).unwrap();
    // Local typing, then one sync tick.
    cx.simulate_input("local words");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(6));
    cx.run_until_parked();
    app.update(cx, |app, _| {
        assert!(
            app.store.page_row(&remote_page).unwrap().is_some(),
            "remote page imported"
        );
    });
    assert!(matches!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(SyncStatus::Synced { .. })
    ));
    // The other device now receives the local edit.
    oc.sync_once(&mut other).unwrap();
    let today = today_page(&app, cx);
    let st = other.page_state(&today).unwrap();
    assert_eq!(st.block(&st.live_roots()[0]).unwrap().text, "local words");
}

/// A transport that counts how many rounds reached the network, so a test can tell one
/// polling loop from two (§12.3).
struct CountingTransport {
    inner: utopia_sync::relay::RelayTransport,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl utopia_sync::Transport for CountingTransport {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }

    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.manifest(if_none_match)
    }

    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        self.inner.push(page, req)
    }

    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        self.inner.pull(page, since_seq)
    }

    fn put_snapshot(
        &mut self,
        page: &PageId,
        covers_seq: u64,
        payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        self.inner.put_snapshot(page, covers_seq, payload)
    }
}

/// A transport that refuses the first `refusals` rounds with 401, then behaves like a real
/// relay: the transient refusal a proxy hiccup or a restarting relay produces.
struct FlakyTransport {
    inner: utopia_sync::relay::RelayTransport,
    refusals: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl utopia_sync::Transport for FlakyTransport {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        self.inner.cursor_space()
    }

    fn manifest(
        &mut self,
        if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.refusals.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            self.refusals
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return Err(utopia_sync::SyncError::Unauthorized);
        }
        self.inner.manifest(if_none_match)
    }

    fn push(
        &mut self,
        page: &PageId,
        req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        self.inner.push(page, req)
    }

    fn pull(
        &mut self,
        page: &PageId,
        since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        self.inner.pull(page, since_seq)
    }

    fn put_snapshot(
        &mut self,
        page: &PageId,
        covers_seq: u64,
        payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        self.inner.put_snapshot(page, covers_seq, payload)
    }
}

/// A transport that refuses every round with a fixed error.
struct Refusing {
    err: fn() -> utopia_sync::SyncError,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl utopia_sync::Transport for Refusing {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        utopia_sync::CursorSpace::new("test:refusing", utopia_sync::CursorKind::Seq)
    }

    fn manifest(
        &mut self,
        _if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err((self.err)())
    }

    fn push(
        &mut self,
        _page: &PageId,
        _req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        Err((self.err)())
    }

    fn pull(
        &mut self,
        _page: &PageId,
        _since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        Err((self.err)())
    }

    fn put_snapshot(
        &mut self,
        _page: &PageId,
        _covers_seq: u64,
        _payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        Err((self.err)())
    }
}

/// §12.3, §13.3: one `SyncRunner` must have one polling loop. A second `enable_sync_with`
/// replaces the runner; it must also retire the loop the previous runner started, or both
/// loops poll the new runner (and the old loop writes its transport into it).
#[gpui::test]
fn enabling_sync_twice_keeps_one_poll_loop(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use utopia_sync::relay::{Relay, RelayTransport};
    let (_app, ws, cx) = setup(cx);
    let relay = Relay::new();
    let first = Arc::new(AtomicUsize::new(0));
    let second = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(CountingTransport {
                inner: RelayTransport::new(relay.clone(), "g"),
                calls: first.clone(),
            }),
            None,
            cx,
        )
    });
    // Replace the runner while the first loop is asleep.
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(CountingTransport {
                inner: RelayTransport::new(relay.clone(), "g"),
                calls: second.clone(),
            }),
            None,
            cx,
        )
    });
    for _ in 0..4 {
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
    }
    assert_eq!(
        first.load(Ordering::SeqCst) + second.load(Ordering::SeqCst),
        2,
        "one loop rounds at 5 s and 15 s in 20 s (5 s then a 10 s idle backoff); a second \
         loop adds rounds"
    );
}

/// §12.6: a round that fails on the device (the store, not the relay) must not be reported
/// as the relay being offline — the network was never tried.
#[gpui::test]
fn a_local_store_error_is_not_reported_as_offline(cx: &mut TestAppContext) {
    use std::time::Duration;
    use utopia_sync::relay::{Relay, RelayTransport};
    use utopia_sync::SyncStatus;
    let (app, ws, cx) = setup(cx);
    ws.update(cx, |w, cx| {
        w.enable_sync_with(Box::new(RelayTransport::new(Relay::new(), "g")), None, cx)
    });
    cx.simulate_input("local words");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    // `prepare` reads `sync_state`; without it the round dies before the network.
    app.update(cx, |app, _| {
        app.store
            .conn()
            .execute_batch("DROP TABLE sync_state")
            .unwrap();
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    let status = ws.read_with(cx, |w, _| w.sync_status());
    assert!(
        !matches!(status, Some(SyncStatus::Offline { .. })),
        "a local store error is not the relay being offline: {status:?}"
    );
    assert_eq!(
        ws.update(cx, |w, _| w.sync_dot()),
        utopia_ui::sync_runner::SyncDot::Error,
        "the dot still says something is wrong"
    );
}

/// §12.6: a failing round backs off. A prepare failure used to keep the last good round's
/// cadence, so a persistent local failure polled every 5 s.
#[gpui::test]
fn a_failed_round_backs_off(cx: &mut TestAppContext) {
    use std::time::Duration;
    use utopia_sync::relay::{Relay, RelayTransport};
    let (app, ws, cx) = setup(cx);
    ws.update(cx, |w, cx| {
        w.enable_sync_with(Box::new(RelayTransport::new(Relay::new(), "g")), None, cx)
    });
    // A round with work keeps the fast 5 s cadence.
    cx.simulate_input("queued words");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    app.update(cx, |app, _| {
        app.store
            .conn()
            .execute_batch("DROP TABLE sync_state")
            .unwrap();
    });
    // This round fails on the device, so the next attempt is 10 s away, not 5 s.
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    // Make the store usable again; a loop without backoff retries at once (5 s).
    app.update(cx, |app, _| {
        app.store
            .conn()
            .execute_batch("CREATE TABLE sync_state (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            .unwrap();
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert!(
        !matches!(
            ws.read_with(cx, |w, _| w.sync_status()),
            Some(utopia_sync::SyncStatus::Synced { .. })
        ),
        "the failed round's 10 s backoff was ignored; it retried after 5 s"
    );
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert!(
        matches!(
            ws.read_with(cx, |w, _| w.sync_status()),
            Some(utopia_sync::SyncStatus::Synced { .. })
        ),
        "the round after the backoff recovers"
    );
}

/// A relay that advertises one page and then hands back an update the store cannot import:
/// the round fails after the network was reached.
struct BadPull {
    page: PageId,
}

impl utopia_sync::Transport for BadPull {
    fn cursor_space(&self) -> utopia_sync::CursorSpace {
        utopia_sync::CursorSpace::new("test:bad-pull", utopia_sync::CursorKind::Seq)
    }

    fn manifest(
        &mut self,
        _if_none_match: Option<u64>,
    ) -> utopia_sync::Result<Option<utopia_sync::Manifest>> {
        Ok(Some(utopia_sync::Manifest {
            rev: 1,
            protocol_version: utopia_sync::PROTOCOL_VERSION,
            pages: vec![utopia_sync::ManifestPage {
                page_id: self.page.to_string(),
                head_seq: 1,
                frontier: String::new(),
                snapshot_seq: 0,
                tail_bytes: 0,
                updated_at: 0,
                deleted: false,
            }],
            instance: "test-relay".into(),
        }))
    }

    fn push(
        &mut self,
        _page: &PageId,
        _req: utopia_sync::PushRequest,
    ) -> utopia_sync::Result<utopia_sync::PushResponse> {
        Ok(utopia_sync::PushResponse {
            accepted: 0,
            head_seq: 0,
            duplicate: 0,
        })
    }

    fn pull(
        &mut self,
        _page: &PageId,
        _since_seq: u64,
    ) -> utopia_sync::Result<utopia_sync::PullResponse> {
        Ok(utopia_sync::PullResponse {
            snapshot: None,
            updates: vec![utopia_sync::UpdateOut {
                seq: 1,
                id: "garbage".into(),
                frontier: String::new(),
                payload: vec![0xff; 8],
            }],
            head_seq: 1,
        })
    }

    fn put_snapshot(
        &mut self,
        _page: &PageId,
        _covers_seq: u64,
        _payload: Vec<u8>,
    ) -> utopia_sync::Result<()> {
        Ok(())
    }
}

/// §13.3: replacing the runner while its round is in the background must not hand that
/// round's result (or its transport) to the new runner. The old loop owns the exchange, so
/// replacing the runner cancels it: it never reaches the relay, and what it would have
/// imported never reaches the store; the new runner starts from `Idle` with no outcome of
/// its own.
#[gpui::test]
fn replacing_the_runner_mid_exchange_discards_the_old_round(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use utopia_sync::relay::{Relay, RelayTransport};
    use utopia_sync::{SyncClient, SyncStatus};
    use utopia_ui::sync_runner::SyncPhase;
    let (app, ws, cx) = setup(cx);
    let relay = Relay::new();
    let old_calls = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(CountingTransport {
                inner: RelayTransport::new(relay.clone(), "g"),
                calls: old_calls.clone(),
            }),
            None,
            cx,
        )
    });
    // One round must finish first: `sync_dot` is amber both while connecting and while an
    // exchange is in flight, so the dot cannot name the moment on its own.
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(old_calls.load(Ordering::SeqCst), 1, "one round ran");
    assert!(matches!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(SyncStatus::Synced { .. })
    ));
    // Something the next round would import if its result were applied.
    let mut other = Store::open_in_memory().unwrap();
    let mut oc = SyncClient::new(RelayTransport::new(relay.clone(), "g"), &other).unwrap();
    other.create_page("From the abandoned round").unwrap();
    oc.sync_once(&mut other).unwrap();
    // Step one task at a time until the runner is really `Exchanging`, i.e. the transport has
    // been handed to the background exchange. (The exchange task itself is only queued.)
    let mut started = false;
    for _ in 0..500 {
        if ws.read_with(cx, |w, _| {
            matches!(w.sync_phase(), Some(SyncPhase::Exchanging))
        }) {
            started = true;
            break;
        }
        cx.dispatcher.advance_clock_to_next_delayed();
        if !cx.background_executor.tick() {
            break;
        }
    }
    assert!(started, "no round ever reached the exchange");
    assert_eq!(
        old_calls.load(Ordering::SeqCst),
        1,
        "the transport is handed over but the exchange has not run: the round is in flight"
    );
    // Replace the runner while that exchange is in flight, pointing it at another relay so
    // anything the abandoned round pulled would be visible as a page only it could import.
    let calls = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(CountingTransport {
                inner: RelayTransport::new(Relay::new(), "g"),
                calls: calls.clone(),
            }),
            None,
            cx,
        )
    });
    assert_eq!(
        ws.read_with(cx, |w, _| w.sync_status()),
        None,
        "the new runner has no round of its own yet"
    );
    for _ in 0..4 {
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
    }
    assert_eq!(
        old_calls.load(Ordering::SeqCst),
        1,
        "the in-flight round was cancelled with its loop and never reached the relay"
    );
    assert!(calls.load(Ordering::SeqCst) >= 1, "the new runner polls");
    assert!(matches!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(SyncStatus::Synced { .. })
    ));
    assert_eq!(
        ws.update(cx, |w, _| w.sync_dot()),
        utopia_ui::sync_runner::SyncDot::Synced
    );
    assert!(
        app.update(cx, |app, _| app
            .store
            .find_page("From the abandoned round")
            .unwrap()
            .is_none()),
        "the abandoned round's result must not be applied"
    );
}

/// §12.6: a store failure *after* the network was reached (an update the store cannot
/// import) is still a local fault, not "the relay is offline". The round applies the rest
/// and reports the page as failed, so it is `Partial` rather than `LocalError`; a failure
/// that stops the whole round (in `prepare`) is `LocalError`, covered by
/// `a_local_store_error_is_not_reported_as_offline`.
#[gpui::test]
fn a_store_failure_after_the_network_is_not_offline(cx: &mut TestAppContext) {
    use std::time::Duration;
    use utopia_sync::SyncStatus;
    let (app, ws, cx) = setup(cx);
    let page = app.update(cx, |app, _| app.store.create_page("Remote").unwrap());
    ws.update(cx, |w, cx| {
        w.enable_sync_with(Box::new(BadPull { page }), None, cx)
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    let status = ws.read_with(cx, |w, _| w.sync_status());
    assert!(
        matches!(status, Some(SyncStatus::Partial { failed: 1, .. })),
        "an import the store rejects is a failed page, not the relay: {status:?}"
    );
    assert!(
        !matches!(status, Some(SyncStatus::Offline { .. })),
        "a local fault is not offline: {status:?}"
    );
    assert_eq!(
        ws.update(cx, |w, _| w.sync_dot()),
        utopia_ui::sync_runner::SyncDot::Error
    );
}

/// §12.6: a refusal stops the fast polls. One retry per slow interval, not every 5 s — the
/// relay refused this device, so hammering it is exactly wrong.
#[gpui::test]
fn a_refusal_retries_only_at_the_slow_cadence(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    let (_app, ws, cx) = setup(cx);
    let calls = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(Refusing {
                err: || utopia_sync::SyncError::Unauthorized,
                calls: calls.clone(),
            }),
            None,
            cx,
        )
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "one refused round");
    assert_eq!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(utopia_sync::SyncStatus::NeedsAuth)
    );
    assert_eq!(
        ws.update(cx, |w, _| w.sync_dot()),
        utopia_ui::sync_runner::SyncDot::Error
    );
    // Well inside the slow interval: the refusal must not be retried at 5 s.
    cx.executor().advance_clock(Duration::from_secs(30));
    cx.run_until_parked();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a refused relay must not be polled every 5 s"
    );
}

/// §12.6: a *transient* refusal (a proxy hiccup, a relay restarting before it has loaded its
/// token file) must not end sync for the session. Polling retries at the slow cadence, so it
/// heals by itself; a 401 that never clears just stays red.
#[gpui::test]
fn a_refusal_recovers_without_a_restart(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use utopia_sync::relay::{Relay, RelayTransport};
    let (_app, ws, cx) = setup(cx);
    let calls = Arc::new(AtomicUsize::new(0));
    let refusals = Arc::new(AtomicUsize::new(1));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(FlakyTransport {
                inner: RelayTransport::new(Relay::new(), "g"),
                refusals: refusals.clone(),
                calls: calls.clone(),
            }),
            None,
            cx,
        )
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(utopia_sync::SyncStatus::NeedsAuth),
        "the first round was refused"
    );
    // The next attempt is a slow interval away, not immediate.
    cx.executor().advance_clock(Duration::from_secs(30));
    cx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "no 5 s hammering");
    // After the slow interval the relay answers again and sync resumes by itself.
    cx.executor().advance_clock(Duration::from_secs(40));
    cx.run_until_parked();
    assert!(calls.load(Ordering::SeqCst) >= 2, "the slow retry happened");
    assert!(
        matches!(
            ws.read_with(cx, |w, _| w.sync_status()),
            Some(utopia_sync::SyncStatus::Synced { .. })
        ),
        "a transient 401 must not kill sync until the app is relaunched"
    );
    assert_eq!(
        ws.update(cx, |w, _| w.sync_dot()),
        utopia_ui::sync_runner::SyncDot::Synced
    );
}

/// §12.6: starting sync again is the explicit way out of a refusal, and it must not need a
/// new Workspace. The replacement runner polls immediately.
#[gpui::test]
fn restarting_sync_after_a_refusal_polls_again(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use utopia_sync::relay::{Relay, RelayTransport};
    let (_app, ws, cx) = setup(cx);
    let refused = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(Refusing {
                err: || utopia_sync::SyncError::Unauthorized,
                calls: refused.clone(),
            }),
            None,
            cx,
        )
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert_eq!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(utopia_sync::SyncStatus::NeedsAuth)
    );
    // The user fixes the token; the app starts sync again on the same workspace.
    let calls = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(CountingTransport {
                inner: RelayTransport::new(Relay::new(), "g"),
                calls: calls.clone(),
            }),
            None,
            cx,
        )
    });
    cx.executor().advance_clock(Duration::from_secs(6));
    cx.run_until_parked();
    assert!(calls.load(Ordering::SeqCst) >= 1, "the new runner polled");
    assert!(matches!(
        ws.read_with(cx, |w, _| w.sync_status()),
        Some(utopia_sync::SyncStatus::Synced { .. })
    ));
}

// ---------------------------------------------------------------- v1.1 (P1)

fn kind_of(
    app: &Entity<AppState>,
    page: &PageId,
    ix: usize,
    cx: &mut VisualTestContext,
) -> (utopia_core::BlockKind, bool) {
    app.update(cx, |app, _| {
        let st = app.store.page_state(page).unwrap();
        let id = st.live_preorder()[ix].0.clone();
        let b = st.block(&id).unwrap();
        (b.kind, b.checked)
    })
}

#[gpui::test]
fn slash_menu_sets_block_types(cx: &mut TestAppContext) {
    use utopia_core::BlockKind;
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("Plan /head");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "Plan ",
        "the /command text is removed"
    );
    assert_eq!(kind_of(&app, &page, 0, cx).0, BlockKind::Heading(1));
    cx.simulate_input("/quote");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(kind_of(&app, &page, 0, cx).0, BlockKind::Quote);
    // Alt+Enter cycles bullet → to-do → done.
    cx.simulate_keystrokes("alt-enter");
    cx.run_until_parked();
    assert_eq!(kind_of(&app, &page, 0, cx), (BlockKind::Todo, false));
    cx.simulate_keystrokes("alt-enter");
    cx.run_until_parked();
    assert_eq!(kind_of(&app, &page, 0, cx), (BlockKind::Todo, true));
}

#[gpui::test]
fn slash_template_and_today(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        let t = app.store.create_page("templates/meeting").unwrap();
        let b = app.store.ensure_first_block(&t).unwrap();
        app.store
            .set_block_text(&t, &b, "Attendees::", None, None)
            .unwrap();
    });
    cx.simulate_input("standup /meet");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("standup ", 0), ("Attendees::", 0)]
    );
    cx.simulate_input(" on /today");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[1].0,
        "Attendees:: on [[November 2nd, 2026]]"
    );
}

#[gpui::test]
fn slash_extract_block_to_page(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a big idea /extr");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input("Big Idea");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Big Idea]]");
    let dst = app.update(cx, |app, _| {
        app.store.find_page("Big Idea").unwrap().unwrap()
    });
    assert_eq!(rows(&app, &dst, cx)[0].0, "a big idea ");
    assert_eq!(
        ws.update(cx, |w, cx| w.sidebar_pages(cx)),
        vec![dst],
        "opened beside"
    );
}

/// SR-2: full-text results sit under a header per page, with the page's blocks indented
/// beneath and the icon on the header's first line.
#[gpui::test]
fn search_groups_results_under_their_page(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        let notes = app.store.create_page("Notes").unwrap();
        let b = app.store.ensure_first_block(&notes).unwrap();
        app.store
            .set_block_text(&notes, &b, "the atlas is late", None, None)
            .unwrap();
        app.store.commit_block(&notes, &b).unwrap();
        let atlas = app.store.create_page("Atlas").unwrap();
        for text in ["atlas launch plan", "atlas risks"] {
            let plan =
                utopia_core::outline::append_block(app.store.page_state(&atlas).unwrap(), None);
            let b = app
                .store
                .execute_plan(&atlas, plan, None)
                .unwrap()
                .unwrap()
                .block;
            app.store
                .set_block_text(&atlas, &b, text, None, None)
                .unwrap();
            app.store.commit_block(&atlas, &b).unwrap();
            app.store.sync_index().unwrap();
        }
    });
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("jump-palette").is_some(),
        "the search palette opened"
    );
    cx.simulate_input("atlas");
    cx.run_until_parked();
    let atlas_header = cx
        .debug_bounds("search-group-0")
        .expect("a header per page");
    let icon = cx.debug_bounds("search-group-icon-0").expect("an icon");
    let title = cx
        .debug_bounds("search-group-title-0")
        .expect("the page name");
    assert!(
        (f32::from(icon.origin.y) - f32::from(title.origin.y)).abs() < 1.5,
        "the icon aligns with the top line (SR-2): {icon:?} {title:?}"
    );
    let hit = cx
        .debug_bounds("search-hit-text-0-0")
        .expect("the first block of the page");
    let hit2 = cx
        .debug_bounds("search-hit-0-1")
        .expect("the page's second block in the same group");
    assert!(
        f32::from(hit.origin.x) > f32::from(title.origin.x) + 8.0,
        "blocks are indented under their page: {hit:?} {title:?}"
    );
    assert!(
        hit.origin.y > atlas_header.origin.y && hit2.origin.y > hit.origin.y,
        "the group's blocks stack below its header: {atlas_header:?} {hit:?} {hit2:?}"
    );
    assert!(
        cx.debug_bounds("search-hit-1-0").is_some(),
        "the next page starts a new group"
    );
}

/// SR-3: the scope chips narrow the grouped results; the age menu drops older hits,
/// and opening it is the only thing that happens (the chips do not move).
#[gpui::test]
fn search_scope_and_age_filter_the_results(cx: &mut TestAppContext) {
    use utopia_core::BlockKind;
    let (app, ws, cx) = setup(cx);
    // Local noon, so the block's local edit date is the intended day in every timezone.
    let ts = |m: u32, d: u32, h| {
        NaiveDate::from_ymd_opt(2026, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_local_timezone(chrono::Local)
            .single()
            .unwrap()
            .timestamp_millis()
    };
    app.update(cx, |app, _| {
        // Now: a page and today's day both mention the zebra.
        app.store.set_clock(Some(ts(11, 2, 12)));
        let atlas = app.store.create_page("Atlas").unwrap();
        let b = app.store.ensure_first_block(&atlas).unwrap();
        app.store
            .set_block_text(&atlas, &b, "zebra plan", None, None)
            .unwrap();
        app.store.commit_block(&atlas, &b).unwrap();
        let (day, db) = app.store.open_daily(today()).unwrap();
        app.store
            .set_block_text(&day, &db, "zebra day", None, None)
            .unwrap();
        app.store.commit_block(&day, &db).unwrap();
        // Last month: an old note outside the Today window.
        app.store.set_clock(Some(ts(10, 1, 12)));
        let old = app.store.create_page("Old").unwrap();
        let b = app.store.ensure_first_block(&old).unwrap();
        app.store
            .set_block_text(&old, &b, "zebra old", None, None)
            .unwrap();
        app.store
            .set_block_kind(&old, &b, BlockKind::Todo, false)
            .unwrap();
        app.store.set_clock(Some(ts(11, 2, 12)));
        app.store.sync_index().unwrap();
    });
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("the palette is open");
    assert_eq!(
        jump.read_with(cx, |j, _| j.groups().len()),
        3,
        "three pages, grouped"
    );

    let journal = cx
        .debug_bounds("search-scope-journal")
        .expect("the Journal chip");
    cx.simulate_click(journal.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        jump.read_with(cx, |j, _| j.scope()),
        utopia_store::SearchScope::Journal
    );
    let groups = jump.read_with(cx, |j, _| j.groups().to_vec());
    assert_eq!(groups.len(), 1, "{groups:?}");
    assert!(groups[0].daily, "only the day is left: {groups:?}");

    // Pages + Today keeps the current page and drops last month's note.
    let pages = cx
        .debug_bounds("search-scope-pages")
        .expect("the Pages chip");
    cx.simulate_click(pages.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let age = cx.debug_bounds("search-age").expect("the age control");
    cx.simulate_click(age.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let today_item = cx
        .debug_bounds("search-age-today")
        .expect("Today in the age menu");
    cx.simulate_click(today_item.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.read_with(cx, |w, _| w.jump_palette().is_some()),
        "the palette is still open (the scrim did not swallow the menu click)"
    );
    assert_eq!(
        jump.read_with(cx, |j, _| j.age()),
        utopia_store::SearchAge::Today
    );
    let titles: Vec<String> = jump.read_with(cx, |j, _| {
        j.groups().iter().map(|g| g.title.clone()).collect()
    });
    assert_eq!(titles, vec!["Atlas".to_string()], "October is out of Today");
}

/// SR-4: the selected block's preview carries its path, state and children, and moving
/// the selection (arrow keys) reads the next block's preview.
#[gpui::test]
fn search_preview_follows_the_selection(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        let atlas = app.store.create_page("Atlas").unwrap();
        let risks = app.store.ensure_first_block(&atlas).unwrap();
        app.store
            .set_block_text(&atlas, &risks, "Risks", None, None)
            .unwrap();
        app.store.commit_block(&atlas, &risks).unwrap();
        for text in ["zebra migration", "zebra budget"] {
            let plan = utopia_core::outline::append_block(
                app.store.page_state(&atlas).unwrap(),
                Some(&risks),
            );
            let b = app
                .store
                .execute_plan(&atlas, plan, None)
                .unwrap()
                .unwrap()
                .block;
            app.store
                .set_block_text(&atlas, &b, text, None, None)
                .unwrap();
            app.store.commit_block(&atlas, &b).unwrap();
        }
        app.store.sync_index().unwrap();
    });
    // A page row selected in quick jump has no block to preview.
    cx.simulate_keystrokes("escape cmd-k");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("quick jump is open");
    assert!(jump.read_with(cx, |j, _| j.preview().is_none()));
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("search is open");
    let first = jump
        .read_with(cx, |j, _| j.preview().cloned())
        .expect("the first hit is a block, so it has a preview");
    assert!(
        cx.debug_bounds("search-preview-state").is_some(),
        "the preview names the block's state"
    );
    assert_eq!(first.page_title, "Atlas");
    assert_eq!(
        first
            .path
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>(),
        vec!["Risks"],
        "the block's path from the root"
    );
    assert_eq!(first.child_count, 0);
    assert_eq!(
        utopia_core::inline::plain_text(&first.text),
        "zebra budget",
        "the newest block is selected first"
    );
    let preview = cx.debug_bounds("search-preview").expect("the pane");
    let results = cx.debug_bounds("jump-results").expect("the list");
    assert!(
        preview.origin.x >= results.origin.x + results.size.width,
        "the preview is beside the list: {preview:?} {results:?}"
    );

    // Down moves to the other hit and the preview follows it.
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    let second = jump
        .read_with(cx, |j, _| j.preview().cloned())
        .expect("the second hit is a block too");
    assert_eq!(
        utopia_core::inline::plain_text(&second.text),
        "zebra migration"
    );
    assert_eq!(second.path.len(), 1, "still under Risks");
    assert_ne!(first.block, second.block);
}

/// SR-5: Cmd+K and Cmd+Shift+F open the same palette. Pages come first, then grouped
/// blocks; Cmd+K selects the page, Cmd+Shift+F the first block.
#[gpui::test]
fn one_palette_opens_on_pages_or_blocks(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        let page = app.store.create_page("Zebra Notes").unwrap();
        let b = app.store.ensure_first_block(&page).unwrap();
        app.store
            .set_block_text(&page, &b, "zebra in the wild", None, None)
            .unwrap();
        app.store.commit_block(&page, &b).unwrap();
        app.store.sync_index().unwrap();
    });

    // Cmd+K: pages first, and the selection starts on the page (no block to preview).
    cx.simulate_keystrokes("escape cmd-k");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("quick jump is open");
    assert!(
        jump.read_with(cx, |j, _| j.preview().is_none()),
        "Cmd+K opens on the page row"
    );
    let page_row = cx.debug_bounds("qj-row-0").expect("the page row");
    let group = cx
        .debug_bounds("search-group-0")
        .expect("the block result is grouped below the pages");
    assert!(
        group.origin.y > page_row.origin.y,
        "pages come first, then blocks: {page_row:?} {group:?}"
    );

    // Down steps over the group header to the block. Cmd+K is a jump list, so it does
    // not open the search preview (SR-4 belongs to the search dialog).
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert!(
        jump.read_with(cx, |j, _| j.preview().is_none()),
        "quick jump opens blocks without a preview pane"
    );

    // Cmd+Shift+F is the same palette, opened on the first block.
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("search is open");
    let preview = jump
        .read_with(cx, |j, _| j.preview().cloned())
        .expect("Cmd+Shift+F opens on the block");
    assert_eq!(preview.page_title, "Zebra Notes");
    assert!(
        !jump.read_with(cx, |j, _| j.offers_create()),
        "block search lists existing blocks, it does not offer to create a page"
    );
}

/// SR-2's decision: when a page name wraps to two lines, the group's icon stays on the
/// top line with the first line of the name, not centred against both.
#[gpui::test]
fn group_icon_stays_on_the_top_line_when_a_name_wraps(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let long = "Atlas Launch Coordination Notes for the Beta Programme, Migration Planning and \
                the Twenty Testers Invited by Hand";
    app.update(cx, |app, _| {
        let page = app.store.create_page(long).unwrap();
        let b = app.store.ensure_first_block(&page).unwrap();
        app.store
            .set_block_text(&page, &b, "zebra crossing", None, None)
            .unwrap();
        app.store.commit_block(&page, &b).unwrap();
        app.store.sync_index().unwrap();
    });
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    let icon = cx
        .debug_bounds("search-group-icon-0")
        .expect("the group icon");
    let title = cx
        .debug_bounds("search-group-title-0")
        .expect("the page name");
    assert!(
        title.size.height >= gpui::px(36.),
        "the name wraps onto two lines: {title:?}"
    );
    assert!(
        (f32::from(icon.origin.y) - f32::from(title.origin.y)).abs() < 1.5,
        "the icon aligns with the first line: {icon:?} {title:?}"
    );
}

/// SR-3: the scope chips filter page-title rows too, not only the grouped blocks. The
/// separately inserted date match is filtered the same way.
#[gpui::test]
fn search_scope_filters_page_titles(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let day = today();
    let day_title = app.update(cx, |app, _| {
        let page = app.store.create_page("Atlas November").unwrap();
        let b = app.store.ensure_first_block(&page).unwrap();
        app.store
            .set_block_text(&page, &b, "zebra plan", None, None)
            .unwrap();
        app.store.commit_block(&page, &b).unwrap();
        let (d, db) = app.store.open_daily(day).unwrap();
        app.store
            .set_block_text(&d, &db, "zebra day", None, None)
            .unwrap();
        app.store.commit_block(&d, &db).unwrap();
        app.store.sync_index().unwrap();
        app.store.page_row(&d).unwrap().unwrap().title
    });

    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("november");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("search is open");
    assert_eq!(
        jump.read_with(cx, |j, _| j.page_titles()),
        vec![day_title.clone(), "Atlas November".to_string()],
        "All shows both the page and the day"
    );

    let journal = cx
        .debug_bounds("search-scope-journal")
        .expect("the Journal chip");
    cx.simulate_click(journal.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        jump.read_with(cx, |j, _| j.page_titles()),
        vec![day_title.clone()],
        "Journal keeps the day and drops the ordinary page"
    );

    let pages = cx
        .debug_bounds("search-scope-pages")
        .expect("the Pages chip");
    cx.simulate_click(pages.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        jump.read_with(cx, |j, _| j.page_titles()),
        vec!["Atlas November".to_string()],
        "Pages keeps the ordinary page and drops the day"
    );
    assert!(
        ws.read_with(cx, |w, _| w.jump_palette().is_some()),
        "the palette is open after the Pages click"
    );

    let todos = cx
        .debug_bounds("search-scope-to-dos")
        .expect("the To-dos chip");
    cx.simulate_click(todos.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        jump.read_with(cx, |j, _| j.page_titles()).is_empty(),
        "a page title is not a to-do"
    );

    // The parser also inserts a date hit that fuzzy titles never matched; it obeys the
    // same scope.
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("2026-11-02");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("search reopened");
    assert!(
        jump.read_with(cx, |j, _| j.page_titles())
            .contains(&day_title),
        "the ISO date resolves to the day in All"
    );
    let pages = cx
        .debug_bounds("search-scope-pages")
        .expect("the Pages chip");
    cx.simulate_click(pages.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        jump.read_with(cx, |j, _| j.page_titles()).is_empty(),
        "Pages must not keep the inserted daily date"
    );

    // With no eligible row, Enter must not create a page outside the chosen scope.
    cx.simulate_keystrokes("escape cmd-k");
    cx.simulate_input("novelpage");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("quick jump reopened");
    let journal = cx
        .debug_bounds("search-scope-journal")
        .expect("the Journal chip");
    cx.simulate_click(journal.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        jump.read_with(cx, |j, _| j.scope()),
        utopia_store::SearchScope::Journal
    );
    assert!(
        jump.read_with(cx, |j, _| j.page_titles()).is_empty(),
        "Journal has no title row for a novel ordinary page"
    );
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(
        app.update(cx, |app, _| app
            .store
            .find_page("novelpage")
            .unwrap()
            .is_none()),
        "Journal scope has no page to create"
    );
}

/// SR-3: the scope applies before the title limit, so ineligible days cannot crowd an
/// ordinary page out of the twelve candidates.
#[gpui::test]
fn search_pages_scope_applies_before_the_title_limit(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        for day in 1..=12 {
            let date = NaiveDate::from_ymd_opt(2026, 11, day).unwrap();
            app.store.ensure_daily_page(date).unwrap();
        }
        app.store.create_page("Plans for November").unwrap();
        app.store.sync_index().unwrap();
    });

    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("november");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("search is open");
    assert_eq!(
        jump.read_with(cx, |j, _| j.page_titles()).len(),
        12,
        "the twelve matching days fill the title limit under All"
    );
    assert!(
        !jump
            .read_with(cx, |j, _| j.page_titles())
            .contains(&"Plans for November".to_string()),
        "the ordinary page is beyond the unfiltered limit"
    );

    let pages = cx
        .debug_bounds("search-scope-pages")
        .expect("the Pages chip");
    cx.simulate_click(pages.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        jump.read_with(cx, |j, _| j.page_titles()),
        vec!["Plans for November".to_string()],
        "Pages limits eligible titles, not the raw candidates"
    );
}

/// SR-5: when page titles fill the results viewport, the block Cmd+Shift+F selects is
/// scrolled into view; otherwise Enter opens a row the user cannot see.
#[gpui::test]
fn search_scrolls_the_selected_block_into_view(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        for i in 1..=12 {
            app.store
                .create_page(&format!("Zebra Page {i:02}"))
                .unwrap();
        }
        let atlas = app.store.create_page("Atlas").unwrap();
        let b = app.store.ensure_first_block(&atlas).unwrap();
        app.store
            .set_block_text(&atlas, &b, "zebra block", None, None)
            .unwrap();
        app.store.commit_block(&atlas, &b).unwrap();
        app.store.sync_index().unwrap();
    });
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("search is open");
    assert_eq!(
        jump.read_with(cx, |j, _| j.page_titles()).len(),
        12,
        "twelve page rows fill the viewport before the group"
    );
    let results = cx.debug_bounds("jump-results").expect("the results list");
    let hit = cx
        .debug_bounds("search-hit-0-0")
        .expect("the selected block row");
    assert!(
        hit.origin.y >= results.origin.y - gpui::px(0.5)
            && hit.origin.y + hit.size.height <= results.bottom() + gpui::px(0.5),
        "the selected block is visible in the list: {hit:?} {results:?}"
    );
}

/// LR-4/rule 10: a keystroke rebuild must not re-run the reference-children queries.
/// Only a new index version (or a display change) needs new children; before the cache,
/// every render of a page with backlinks issued two SQL queries per referencing block.
#[gpui::test]
fn typing_does_not_requery_reference_children(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (target, first) = app.update(cx, |app, _| {
        let target = app.store.create_page("Target").unwrap();
        let first = app.store.ensure_first_block(&target).unwrap();
        let source = app.store.create_page("Source").unwrap();
        let b = app.store.ensure_first_block(&source).unwrap();
        app.store
            .set_block_text(&source, &b, "see [[Target]]", None, None)
            .unwrap();
        app.store.commit_block(&source, &b).unwrap();
        let plan =
            utopia_core::outline::append_block(app.store.page_state(&source).unwrap(), Some(&b));
        let child = app
            .store
            .execute_plan(&source, plan, None)
            .unwrap()
            .unwrap()
            .block;
        app.store
            .set_block_text(&source, &child, "a child of the reference", None, None)
            .unwrap();
        app.store.commit_block(&source, &child).unwrap();
        app.store.sync_index().unwrap();
        (target, first)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, Some(first), false, window, cx)
    });
    cx.run_until_parked();
    let view = ws.read_with(cx, |w, _| w.main_view());
    let before = view.read_with(cx, |v, _| v.ref_children_queries());
    assert!(before > 0, "the first build reads the children it shows");
    let version = app.read_with(cx, |a, _| a.refs_version);

    cx.simulate_input("x");
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |a, _| a.refs_version),
        version,
        "the edit has not reached the index yet"
    );
    let after = view.read_with(cx, |v, _| v.ref_children_queries());
    assert_eq!(
        after, before,
        "a keystroke rebuild reuses the children for this refs_version"
    );
}

/// Rule 10: the IndexUpdated handler loads the reference children before the frame; a
/// rebuild in `render` only reads that cache.
#[gpui::test]
fn index_updates_prefetch_reference_children(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (target, first) = app.update(cx, |app, _| {
        let target = app.store.create_page("Target").unwrap();
        let first = app.store.ensure_first_block(&target).unwrap();
        let source = app.store.create_page("Source").unwrap();
        let b = app.store.ensure_first_block(&source).unwrap();
        app.store
            .set_block_text(&source, &b, "see [[Target]]", None, None)
            .unwrap();
        app.store.commit_block(&source, &b).unwrap();
        let plan =
            utopia_core::outline::append_block(app.store.page_state(&source).unwrap(), Some(&b));
        let child = app
            .store
            .execute_plan(&source, plan, None)
            .unwrap()
            .unwrap()
            .block;
        app.store
            .set_block_text(&source, &child, "a child of the reference", None, None)
            .unwrap();
        app.store.commit_block(&source, &child).unwrap();
        app.store.sync_index().unwrap();
        (target, first)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, Some(first), false, window, cx)
    });
    cx.run_until_parked();
    let view = ws.read_with(cx, |w, _| w.main_view());
    let before = view.read_with(cx, |v, _| v.ref_children_queries());
    assert!(before > 0, "the first build reads the children it shows");

    let frame_before = view.read_with(cx, |v, _| v.ref_children_frame_queries());
    // What a durable index update does: bump the version and notify listeners.
    app.update(cx, |app, cx| {
        app.refs_version += 1;
        cx.emit(IndexUpdated);
    });
    cx.run_until_parked();
    let (total, frame) = view.read_with(cx, |v, _| {
        (v.ref_children_queries(), v.ref_children_frame_queries())
    });
    assert!(
        total > before,
        "the new index version reads the children once"
    );
    assert_eq!(
        frame, frame_before,
        "the frame only reads the children cache"
    );
}

/// §13.3: a handler that writes and rebuilds itself must not be dirtied again by the
/// `Changed` it emitted. gpui queues `emit` and delivers it after the update, so the old
/// `dirty` flag was cleared by the handler's rebuild and set again by its own event; the
/// next frame rebuilt the same rows a second time (rebuild-cycle issue 1).
#[gpui::test]
fn a_handler_rebuild_is_not_repeated_by_its_own_change(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    // End the feed's own edit: otherwise starting the next one commits it first, and that
    // commit's structural change (and its index flush) would rebuild for a real reason.
    ws.update(cx, |w, cx| w.editor().update(cx, |e, cx| e.end(cx)));
    cx.run_until_parked();
    let view = ws.read_with(cx, |w, _| w.main_view());
    let (before, frames_before) = view.read_with(cx, |v, _| (v.rebuilds(), v.frame_rebuilds()));

    // Clicking an empty day starts writing a draft, which reports a change with nothing
    // to flush (FR-1.2) and rebuilds the rows in the click handler. No index update
    // follows, so any second pass is the deferred `Changed` alone.
    let yesterday = today() - chrono::Duration::days(1);
    let empty_sel: &'static str =
        Box::leak(format!("empty-day-{}", utopia_core::date::iso(yesterday)).into_boxed_str());
    let empty = cx
        .debug_bounds(empty_sel)
        .expect("a blank day offers a row to start writing");
    cx.simulate_click(empty.center(), Modifiers::default());
    cx.run_until_parked();

    let (after, frames_after) = view.read_with(cx, |v, _| (v.rebuilds(), v.frame_rebuilds()));
    assert!(
        after > before,
        "the click handler rebuilds the rows it wrote"
    );
    assert_eq!(
        after - before,
        1,
        "one write is one rebuild: the deferred Changed was for the state the handler built"
    );
    assert_eq!(
        frames_after, frames_before,
        "the second pass would have run inside render (rule 10)"
    );
}

/// Rule 10: a query block reads every page, so a change on any page must answer it — in
/// the change handler, not in the next frame. The old `has_embeds` marked the rows dirty
/// and `render` ran the query (rebuild-cycle issue 2).
#[gpui::test]
fn a_query_block_answers_other_pages_changes_outside_the_frame(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("Read [[Dune]] #book");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("{{query: {and: [[Dune]] #book}}}");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let view = ws.read_with(cx, |w, _| w.main_view());
    assert!(
        view.read_with(cx, |v, _| v.interested_in_all_pages()),
        "a query block makes every page's change relevant"
    );
    let (before, frames_before) = view.read_with(cx, |v, _| (v.rebuilds(), v.frame_rebuilds()));

    // A change this view does not show: the query's answer can still change.
    app.update(cx, |app, cx| {
        app.changed(vec![PageId("elsewhere".into())], false, cx)
    });
    cx.run_until_parked();

    let (after, frames_after) = view.read_with(cx, |v, _| (v.rebuilds(), v.frame_rebuilds()));
    assert!(
        after > before,
        "the view rebuilt for a change it is interested in"
    );
    assert_eq!(
        frames_after, frames_before,
        "the query ran in the handler, not inside render (rule 10)"
    );
}

/// A transclusion of another page is an interest in that page only while its row is open.
/// Leaving the page must drop it, or every later change anywhere rebuilt this view
/// (rebuild-cycle issue 2: `open_embeds` was not in the per-page reset list).
#[gpui::test]
fn leaving_a_page_drops_its_open_transclusions(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (target, home, other) = app.update(cx, |app, _| {
        let target = app.store.create_page("Target").unwrap();
        let t = app.store.ensure_first_block(&target).unwrap();
        app.store
            .set_block_text(&target, &t, "a thought", None, None)
            .unwrap();
        app.store.commit_block(&target, &t).unwrap();
        let home = app.store.create_page("Home").unwrap();
        let b = app.store.ensure_first_block(&home).unwrap();
        app.store
            .set_block_text(&home, &b, &format!("(({t}))"), None, None)
            .unwrap();
        app.store.commit_block(&home, &b).unwrap();
        app.store.sync_index().unwrap();
        let other = app.store.create_page("Other").unwrap();
        (target, home, other)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(home, None, None, false, window, cx)
    });
    cx.run_until_parked();
    let content = cx
        .debug_bounds("content-1")
        .expect("the reference row is drawn");
    // The chip starts at the text's left edge; the container is as wide as the row.
    cx.simulate_click(
        point(content.origin.x + px(8.), content.center().y),
        Modifiers::default(),
    );
    cx.run_until_parked();
    let view = ws.read_with(cx, |w, _| w.main_view());
    assert!(
        view.read_with(cx, |v, _| v.interested_in_all_pages()),
        "the opened transclusion reads the target page"
    );

    // SP-1: opening another page stacks a new entry; Home's view stays, out of sight in
    // Single mode, and an entry out of sight does no work when other pages change.
    ws.update_in(cx, |w, window, cx| {
        w.open(other.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    assert!(!view.read_with(cx, |v, _| v.is_active()));
    let hidden = view.read_with(cx, |v, _| v.rebuilds());
    app.update(cx, |app, cx| app.changed(vec![target.clone()], false, cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.rebuilds()),
        hidden,
        "a hidden entry only marks itself stale"
    );

    // The original regression: a view that moves to another page drops the page's open
    // transclusions (rebuild-cycle issue 2).
    view.update(cx, |v, cx| v.open(other, None, cx));
    cx.run_until_parked();
    assert!(
        !view.read_with(cx, |v, _| v.interested_in_all_pages()),
        "the transclusion belonged to the page that was left"
    );
    view.update(cx, |v, cx| v.set_active(true, cx));
    cx.run_until_parked();
    let before = view.read_with(cx, |v, _| v.rebuilds());
    app.update(cx, |app, cx| app.changed(vec![target.clone()], false, cx));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.rebuilds()),
        before,
        "a change to the old transclusion's page no longer rebuilds this view"
    );
}

/// AGENTS.md "Keep that when touching the list": the row being edited carries the
/// editor's focus handle so it keeps receiving keystrokes out of view. That registration
/// must end with the target — `end` used to clear the target and leave the handle on the
/// row until some later rebuild happened to re-splice it (rebuild-cycle issue 4).
#[gpui::test]
fn ending_an_edit_drops_the_rows_focus_registration(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (page, block) = app.update(cx, |app, _| {
        let page = app.store.create_page("Rows").unwrap();
        let block = app.store.ensure_first_block(&page).unwrap();
        app.store
            .set_block_text(&page, &block, "first", None, None)
            .unwrap();
        app.store.commit_block(&page, &block).unwrap();
        (page, block)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, Some(block), false, window, cx)
    });
    cx.run_until_parked();
    let view = ws.read_with(cx, |w, _| w.main_view());
    assert_eq!(
        view.read_with(cx, |v, _| v.focused_rows()),
        1,
        "the row being edited carries the editor's handle"
    );

    // Only the editor ends here: no page-view handler runs, so the `Blurred` event is the
    // one thing that can re-splice the rows before the next frame.
    ws.update(cx, |w, cx| w.editor().update(cx, |e, cx| e.end(cx)));
    assert_eq!(
        view.read_with(cx, |v, _| v.focused_rows()),
        0,
        "the target and the row's focus handle moved together"
    );
}

/// Rule 9: appearance preferences this view mirrors are resynced in one place, so an
/// index-triggered rebuild cannot build rows with the old setting and leave the next frame
/// to correct them (rebuild-cycle issue 3).
#[gpui::test]
fn an_appearance_change_rebuilds_in_the_observer(cx: &mut TestAppContext) {
    use utopia_ui::appearance;
    let (app, ws, cx) = setup(cx);
    let page = app.update(cx, |app, _| {
        let page = app.store.create_page("Project").unwrap();
        let b = app.store.ensure_first_block(&page).unwrap();
        app.store
            .set_block_text(&page, &b, "owner:: Ada", None, None)
            .unwrap();
        app.store.commit_block(&page, &b).unwrap();
        page
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, None, false, window, cx)
    });
    cx.run_until_parked();
    let view = ws.read_with(cx, |w, _| w.main_view());
    let (before, frames_before) = view.read_with(cx, |v, _| (v.rebuilds(), v.frame_rebuilds()));

    // Settings → Appearance → "Children shown in references", applied while open.
    let dir = tempfile::tempdir().unwrap();
    cx.update(|_, cx| {
        appearance::set_ref_children(cx, 3, dir.path()).unwrap();
    });

    let (after_set, frames_after_set) =
        view.read_with(cx, |v, _| (v.rebuilds(), v.frame_rebuilds()));
    assert!(
        after_set > before,
        "the appearance observer rebuilds the rows it mirrors"
    );
    assert_eq!(
        frames_after_set, frames_before,
        "in the observer, not in the frame that redraws the window"
    );
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.frame_rebuilds()),
        frames_before,
        "and a later frame does not have to correct the rows"
    );
}

#[gpui::test]
fn full_search_and_quick_capture(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let target = app.update(cx, |app, _| {
        let p = app.store.create_page("Archive").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "the zebra crossing story", None, None)
            .unwrap();
        app.store.sync_index().unwrap();
        p
    });
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(target.clone()));
    cx.simulate_input("!");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &target, cx)[0].0,
        "the zebra crossing story!",
        "editing the hit"
    );
    // Quick capture appends to today without leaving the page.
    cx.simulate_keystrokes("escape cmd-shift-space");
    cx.simulate_input("remember the milk");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let today = today_page(&app, cx);
    assert!(rows(&app, &today, cx)
        .iter()
        .any(|(t, _)| t == "remember the milk"));
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(target));
}

#[gpui::test]
fn unlinked_references_link_button(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (m, src) = app.update(cx, |app, _| {
        let m = app.store.create_page("Mindfulness").unwrap();
        let src = app.store.create_page("Reading").unwrap();
        let b = app.store.ensure_first_block(&src).unwrap();
        app.store
            .set_block_text(&src, &b, "practising mindfulness daily", None, None)
            .unwrap();
        app.store.sync_index().unwrap();
        (m, src)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(m.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    let header = cx
        .debug_bounds("unlinked-header")
        .expect("section rendered");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let link = (0..32)
        .find_map(|i| {
            let name: &'static str = Box::leak(format!("link-{i}").into_boxed_str());
            cx.debug_bounds(name)
        })
        .expect("a Link button per mention");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &src, cx)[0].0,
        "practising [[Mindfulness]] daily"
    );
    app.update(cx, |app, _| {
        assert_eq!(app.store.backlink_count(&m).unwrap(), 1)
    });
}

/// LR-3: the path above a reference is clickable part by part. The page part opens the
/// referencing page; an ancestor part opens it with that ancestor focused.
#[gpui::test]
fn reference_path_parts_open_their_target(cx: &mut TestAppContext) {
    use utopia_core::{BlockId, BlockKind, Command};
    let (app, ws, cx) = setup(cx);
    let (target, atlas, risks) = app.update(cx, |app, _| {
        let target = app.store.create_page("Target").unwrap();
        let atlas = app.store.create_page("Atlas Launch").unwrap();
        let risks = BlockId::new();
        let risky = BlockId::new();
        app.store
            .execute(
                &atlas,
                vec![
                    Command::InsertBlock {
                        id: risks.clone(),
                        parent: None,
                        after: None,
                        text: "Risks".into(),
                        kind: BlockKind::Heading(2),
                    },
                    Command::InsertBlock {
                        id: risky,
                        parent: Some(risks.clone()),
                        after: None,
                        text: "The beta date depends on [[Target]]".into(),
                        kind: BlockKind::Bullet,
                    },
                ],
                Default::default(),
            )
            .unwrap();
        app.store.sync_index().unwrap();
        (target, atlas, risks)
    });
    let path_part = |cx: &mut VisualTestContext, part: usize| {
        cx.run_until_parked();
        (0..40)
            .find_map(|ix| {
                let sel: &'static str =
                    Box::leak(format!("hit-path-part-{ix}-{part}").into_boxed_str());
                cx.debug_bounds(sel)
            })
            .expect("a path part was rendered")
    };

    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    let page_part = path_part(cx, 0);
    cx.simulate_click(page_part.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(atlas.clone()),
        "the page part opens the page the reference lives on"
    );

    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    let ancestor_part = path_part(cx, 1);
    cx.simulate_click(ancestor_part.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(atlas));
    assert_eq!(
        ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .target()
            .map(|t| t.block.clone())),
        Some(risks),
        "the ancestor part focuses that block"
    );
}

/// LR-5 with DESIGN.md §3.7: the collapsed row has no count (that would need the search),
/// expanding searches, "Link all" links every mention at once, and an empty open section
/// says so.
#[gpui::test]
fn unlinked_references_count_and_link_all(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (m, src) = app.update(cx, |app, _| {
        let m = app.store.create_page("Mindfulness").unwrap();
        let src = app.store.create_page("Reading").unwrap();
        let b = app.store.ensure_first_block(&src).unwrap();
        app.store
            .set_block_text(
                &src,
                &b,
                "practising mindfulness daily, then mindfulness again",
                None,
                None,
            )
            .unwrap();
        app.store.commit_block(&src, &b).unwrap();
        app.store.sync_index().unwrap();
        (m, src)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(m.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    // A closed row cannot know a count without searching; expanding searches.
    assert!(
        cx.debug_bounds("unlinked-count-2").is_none(),
        "a collapsed row is not searched for a count"
    );
    let header = cx.debug_bounds("unlinked-header").expect("header");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.debug_bounds("unlinked-count-2")
        .expect("the expanded row shows how many unlinked mentions there are");
    let link_all = cx
        .debug_bounds("link-all")
        .expect("Link all is offered while the section is open");
    cx.simulate_click(link_all.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &src, cx)[0].0,
        "practising [[Mindfulness]] daily, then [[Mindfulness]] again",
        "every mention, with the right byte offsets"
    );
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        assert_eq!(app.store.backlink_count(&m).unwrap(), 1);
        assert!(app.store.unlinked_references(&m).unwrap().is_empty());
    });
    // The section stays open; with every mention linked it now says it is empty.
    cx.run_until_parked();
    cx.debug_bounds("unlinked-empty")
        .expect("the empty state names itself");
    cx.debug_bounds("unlinked-count-0")
        .expect("and the count is zero");
}

#[gpui::test]
fn embeds_render_and_open_as_a_pane(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (other, target) = app.update(cx, |app, _| {
        let p = app.store.create_page("Source").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "embedded thought", None, None)
            .unwrap();
        (p, b)
    });
    cx.simulate_input(&format!("{{{{embed: (({target}))}}}}"));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    // The embed renders the source block as an extra (editable) row.
    assert!(
        cx.debug_bounds("content-2").is_some(),
        "embedded row rendered"
    );
    // ⌘-click (once a split pane): the page opens as a pane beside the feed (§4.4).
    ws.update_in(cx, |w, window, cx| {
        w.set_scroll_animation(false);
        w.open_from(
            Origin::Pane(0),
            Click::Secondary,
            other.clone(),
            None,
            None,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.stack_pages(cx)).last(),
        Some(&Some(other))
    );
    let pane = cx.debug_bounds("stack-pane-1").expect("second pane drawn");
    let feed = cx.debug_bounds("stack-pane-0").unwrap();
    assert!(pane.origin.x > feed.origin.x && pane.size.width > gpui::px(100.));
}

#[gpui::test]
fn encrypted_folder_sync_from_config(cx: &mut TestAppContext) {
    use utopia_sync::e2e::{Encrypted, GraphKey};
    use utopia_sync::folder::FolderTransport;
    use utopia_sync::SyncClient;
    use utopia_ui::sync_runner::{SyncConfig, SyncTarget};
    let (app, ws, cx) = setup(cx);
    let shared = tempfile::tempdir().unwrap();
    ws.update(cx, |w, cx| {
        w.enable_sync(
            SyncConfig {
                target: SyncTarget::Folder(shared.path().to_path_buf()),
                graph: "notes".into(),
                passphrase: Some("hunter2 hunter2".into()),
            },
            cx,
        )
    });
    cx.simulate_input("synced privately");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(6));
    cx.run_until_parked();
    // Another device with the passphrase reads it; the folder holds only ciphertext.
    let mut other = Store::open_in_memory().unwrap();
    let t = FolderTransport::new(shared.path(), "phone").unwrap();
    let key = GraphKey::derive("hunter2 hunter2", "notes").unwrap();
    let mut c = SyncClient::new(Encrypted::new(t, key, "notes"), &other).unwrap();
    c.sync_once(&mut other).unwrap();
    let today = today_page(&app, cx);
    let st = other.page_state(&today).unwrap();
    assert_eq!(
        st.block(&st.live_roots()[0]).unwrap().text,
        "synced privately"
    );
    for entry in walk(shared.path()) {
        let bytes = std::fs::read(&entry).unwrap();
        assert!(
            !bytes.windows(8).any(|w| w == b"privatel"),
            "plaintext in {entry:?}"
        );
    }
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[gpui::test]
fn graph_view_lays_out_and_opens_pages(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(gpui::px(1280.), gpui::px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    assert!(cx.debug_bounds("graph-view").is_some());
    // Alpha, Beta and today's page, linked through the journal page.
    let (nodes, edges, laid_out) =
        graph.update(cx, |g, _| (g.node_count(), g.edge_count(), g.is_laid_out()));
    assert_eq!((nodes, edges, laid_out), (3, 2, true));
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    let i = (0..nodes)
        .find(|&i| graph.update(cx, |g, _| g.node_page(i)) == Some(alpha.clone()))
        .unwrap();
    let at = graph.update(cx, |g, _| g.node_screen_pos(i)).unwrap();
    let canvas = cx.debug_bounds("graph-canvas").unwrap();
    assert!(
        canvas.contains(&at),
        "node drawn inside the canvas: {at:?} {canvas:?}"
    );
    cx.simulate_click(at, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_none(),
        "graph closes"
    );
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(alpha));
    // Escape closes it too.
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(ws.update(cx, |w, _| w.graph_view()).is_none());
}

/// §9.2 main window: the main column shows one page. Opening a page there — the sidebar's
/// page rows, the recent days, a pinned page, Quick jump, a date shortcut — closes the
/// full-screen graph, or the page "opens" invisibly and the graph keeps focus
/// (workspace-main-pane-content.md, issue 1).
#[gpui::test]
fn opening_a_page_closes_the_graph(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let alpha = app.update(cx, |a, _| {
        let p = a.store.create_page("Alpha").unwrap();
        a.store.ensure_first_block(&p).unwrap();
        p
    });
    cx.simulate_keystrokes("escape cmd-shift-g");
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_some(),
        "the graph is up"
    );
    // Every one of those paths calls `Workspace::open` for the main pane.
    ws.update_in(cx, |w, window, cx| {
        w.open(alpha.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_none(),
        "the graph gives way to the page it was hiding"
    );
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(alpha));
}

/// GR-1: pages grow with their link count, from 6 px to 18 px across.
#[gpui::test]
fn graph_sizes_nodes_by_links(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] [[Alpha]] [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    let today = today_page(&app, cx);
    let nodes = graph.update(cx, |g, _| g.node_count());
    assert_eq!(nodes, 3);
    let mut node = |page: &PageId| {
        (0..nodes)
            .find(|&i| graph.update(cx, |g, _| g.node_page(i)).as_ref() == Some(page))
            .unwrap()
    };
    let (today_i, alpha_i) = (node(&today), node(&alpha));
    let (today_r, alpha_r) = graph.update(cx, |g, _| {
        (
            g.node_radius(today_i).unwrap(),
            g.node_radius(alpha_i).unwrap(),
        )
    });
    // Today links to Alpha twice and Beta once, so it has the larger degree.
    assert!(today_r > alpha_r, "{today_r} > {alpha_r}");
    assert!(
        (3. ..=9.).contains(&alpha_r) && (3. ..=9.).contains(&today_r),
        "diameters stay between 6 and 18 px: {alpha_r} {today_r}"
    );
}

/// GR-2: hovering a node floats its card over the canvas without moving the graph.
#[gpui::test]
fn graph_hover_shows_a_floating_card(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    let nodes = graph.update(cx, |g, _| g.node_count());
    let i = (0..nodes)
        .find(|&i| graph.update(cx, |g, _| g.node_page(i)) == Some(alpha.clone()))
        .unwrap();
    let at = graph.update(cx, |g, _| g.node_screen_pos(i)).unwrap();
    let canvas = cx.debug_bounds("graph-canvas").unwrap();
    cx.simulate_mouse_move(at, None, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(graph.update(cx, |g, _| g.hovered()), Some(i));
    let card = cx.debug_bounds("graph-tooltip").expect("hover card");
    assert!(
        f32::from(card.size.width) > 100.,
        "the card has a readable width"
    );
    assert_eq!(
        cx.debug_bounds("graph-canvas").unwrap(),
        canvas,
        "the card floats; it does not move the canvas"
    );
    // Moving away clears the hover state (the test platform keeps old debug bounds, so
    // assert on the view, per AGENTS.md).
    cx.simulate_mouse_move(
        canvas.origin + point(px(4.), px(4.)),
        None,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert_eq!(graph.update(cx, |g, _| g.hovered()), None);
}

/// GR-2: a plain click opens the page; ⌘-click opens it as a pane beside the feed
/// (DESIGN.md §4.4; it opened a split before the stack replaced splits).
#[gpui::test]
fn graph_cmd_click_opens_a_pane(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    let main = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    let nodes = graph.update(cx, |g, _| g.node_count());
    let i = (0..nodes)
        .find(|&i| graph.update(cx, |g, _| g.node_page(i)) == Some(alpha.clone()))
        .unwrap();
    let at = graph.update(cx, |g, _| g.node_screen_pos(i)).unwrap();
    let cmd = Modifiers {
        platform: true,
        ..Default::default()
    };
    cx.simulate_click(at, cmd);
    cx.run_until_parked();
    assert!(ws.update(cx, |w, _| w.graph_view()).is_none());
    assert_eq!(
        ws.update(cx, |w, cx| w.stack_pages(cx)),
        vec![Some(main), Some(alpha)],
        "the feed stays put and the page opens beside it"
    );
    assert_eq!(
        ws.update(cx, |w, _| w.view_mode()),
        utopia_ui::pane_stack::ViewMode::Panes
    );
}

/// GR-5: "Around this page" is a right-sidebar panel beside the editable page.
#[gpui::test]
fn graph_panel_sits_beside_the_page(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let main = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    let local = cx
        .debug_bounds("graph-local")
        .expect("Around this page filter")
        .center();
    cx.simulate_click(local, Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_none(),
        "the full-screen graph gives way to the panel"
    );
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel open");
    assert!(cx.debug_bounds("graph-panel").is_some());
    assert!(cx.debug_bounds("right-sidebar").is_some());
    assert_eq!(panel.update(cx, |g, _| g.origin()), Some(main.clone()));
    assert!(panel.update(cx, |g, _| g.is_laid_out()));
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(main),
        "the page beside the panel is untouched"
    );

    // Clicking a page in the panel still edits the main pane, and the panel follows it.
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    let nodes = panel.update(cx, |g, _| g.node_count());
    let i = (0..nodes)
        .find(|&i| panel.update(cx, |g, _| g.node_page(i)) == Some(alpha.clone()))
        .unwrap();
    let at = panel.update(cx, |g, _| g.node_screen_pos(i)).unwrap();
    cx.simulate_click(at, Modifiers::default());
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(alpha.clone()));
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel stays");
    assert_eq!(panel.update(cx, |g, _| g.origin()), Some(alpha));

    // "All pages" returns to the full-screen mode.
    let expand = cx.debug_bounds("graph-expand").unwrap().center();
    cx.simulate_click(expand, Modifiers::default());
    cx.run_until_parked();
    assert!(ws.update(cx, |w, _| w.graph_panel()).is_none());
    assert!(ws.update(cx, |w, _| w.graph_view()).is_some());
}

/// GR-5 review: "Around this page" keeps the graph's own origin. Opening the graph from a
/// sidebar page's menu and then moving it into the sidebar must stay around that page, not
/// silently switch to whatever the main pane shows.
#[gpui::test]
fn graph_panel_keeps_the_sidebar_pages_origin(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (a, b) = app.update(cx, |app, _| {
        let a = app.store.create_page("Alpha Page").unwrap();
        app.store.ensure_first_block(&a).unwrap();
        let b = app.store.create_page("Beta Page").unwrap();
        app.store.ensure_first_block(&b).unwrap();
        (a, b)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(a.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    // Beta beside Alpha, so its own page menu is the one we use.
    ws.update_in(cx, |w, window, cx| {
        w.open(b.clone(), None, None, true, window, cx)
    });
    cx.run_until_parked();
    let menu = cx
        .debug_bounds("pane-page-menu")
        .expect("the sidebar card's page menu");
    cx.simulate_click(menu.center(), Modifiers::default());
    cx.run_until_parked();
    let item = cx.debug_bounds("page-menu-graph").expect("Open in graph");
    cx.simulate_click(item.center(), Modifiers::default());
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    assert_eq!(graph.update(cx, |g, _| g.origin()), Some(b.clone()));
    let local = cx
        .debug_bounds("graph-local")
        .expect("Around this page filter")
        .center();
    cx.simulate_click(local, Modifiers::default());
    cx.run_until_parked();
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel open");
    assert_eq!(
        panel.update(cx, |g, _| g.origin()),
        Some(b),
        "the panel stays around the page whose graph was opened"
    );
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(a));
}

/// GR-5 review: hiding the sidebar and bringing it back is not navigation. A panel opened
/// around a sidebar page's graph keeps its own origin and the pan/zoom the user set; only a
/// main-page change made while the sidebar was away may move it (and re-read the index).
#[gpui::test]
fn reopening_the_sidebar_keeps_the_graph_panels_origin(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (a, b) = app.update(cx, |app, _| {
        let a = app.store.create_page("Alpha Page").unwrap();
        app.store.ensure_first_block(&a).unwrap();
        let b = app.store.create_page("Beta Page").unwrap();
        app.store.ensure_first_block(&b).unwrap();
        (a, b)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(a.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    // Beta beside Alpha, then its menu's "Open in graph" and the sidebar panel, so the
    // panel's origin is Beta while the main pane still shows Alpha.
    ws.update_in(cx, |w, window, cx| {
        w.open(b.clone(), None, None, true, window, cx)
    });
    cx.run_until_parked();
    let menu = cx
        .debug_bounds("pane-page-menu")
        .expect("the sidebar card's page menu");
    cx.simulate_click(menu.center(), Modifiers::default());
    cx.run_until_parked();
    let item = cx.debug_bounds("page-menu-graph").expect("Open in graph");
    cx.simulate_click(item.center(), Modifiers::default());
    cx.run_until_parked();
    let local = cx
        .debug_bounds("graph-local")
        .expect("Around this page filter")
        .center();
    cx.simulate_click(local, Modifiers::default());
    cx.run_until_parked();
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel open");
    assert_eq!(panel.update(cx, |g, _| g.origin()), Some(b.clone()));
    // Zoom the panel, so a view reset on reopen is visible as well as the origin move.
    let canvas = cx.debug_bounds("graph-canvas").expect("panel canvas");
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: canvas.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(120.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let zoom = panel.update(cx, |g, _| g.zoom());
    assert!(zoom != 1.0, "the wheel zoomed the panel: {zoom}");

    // Hiding and reopening the sidebar without opening anything is not a navigation.
    cx.simulate_keystrokes("cmd-shift-\\");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-\\");
    cx.run_until_parked();
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel stays");
    assert_eq!(
        panel.update(cx, |g, _| g.origin()),
        Some(b),
        "reopening the sidebar is not navigation"
    );
    assert_eq!(
        panel.update(cx, |g, _| g.zoom()),
        zoom,
        "the viewport is still the user's"
    );
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(a));
}

/// GR-5: the panel is a projection of the index, so a new link while it is open must show
/// up even though the page id did not change.
#[gpui::test]
fn graph_panel_refreshes_after_new_links(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let local = cx
        .debug_bounds("graph-local")
        .expect("Around this page filter")
        .center();
    cx.simulate_click(local, Modifiers::default());
    cx.run_until_parked();
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel open");
    assert_eq!(panel.update(cx, |g, _| g.node_count()), 3);
    assert_eq!(panel.update(cx, |g, _| g.edge_count()), 2);

    // Add a third link to the page open beside the panel. The page id is unchanged, so an
    // origin-only refresh would miss it.
    let today = today_page(&app, cx);
    let block = app.update(cx, |app, _| {
        app.store
            .page_state(&today)
            .unwrap()
            .live_roots()
            .first()
            .cloned()
            .unwrap()
    });
    app.update(cx, |app, cx| {
        let r =
            app.store
                .set_block_text(&today, &block, "[[Alpha]] [[Beta]] [[Gamma]]", None, None);
        app.report(r);
        let r = app.store.commit_block(&today, &block);
        app.report(r);
        app.changed(vec![today.clone()], true, cx);
    });
    cx.run_until_parked();

    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel stays");
    assert_eq!(
        panel.update(cx, |g, _| g.node_count()),
        4,
        "the panel shows the new page"
    );
    assert_eq!(panel.update(cx, |g, _| g.edge_count()), 3);
}

/// GR-5: a graph panel hidden with the right sidebar is not drawn, so reloading its layout
/// on every main-page change and every index update is work for nobody. It catches up when
/// the sidebar comes back (workspace-main-pane-content.md, issue 3).
#[gpui::test]
fn hidden_graph_panel_does_no_work(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let local = cx
        .debug_bounds("graph-local")
        .expect("Around this page filter")
        .center();
    cx.simulate_click(local, Modifiers::default());
    cx.run_until_parked();
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel open");
    let today = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    assert_eq!(panel.update(cx, |g, _| g.origin()), Some(today.clone()));
    assert_eq!(panel.update(cx, |g, _| g.node_count()), 3);

    // Hide the sidebar: the panel stays (reopening brings it back) but is not rendered.
    cx.simulate_keystrokes("cmd-shift-\\");
    cx.run_until_parked();
    let block = app.update(cx, |app, _| {
        app.store
            .page_state(&today)
            .unwrap()
            .live_roots()
            .first()
            .cloned()
            .unwrap()
    });
    app.update(cx, |app, cx| {
        let r =
            app.store
                .set_block_text(&today, &block, "[[Alpha]] [[Beta]] [[Gamma]]", None, None);
        app.report(r);
        let r = app.store.commit_block(&today, &block);
        app.report(r);
        app.changed(vec![today.clone()], true, cx);
    });
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    ws.update_in(cx, |w, window, cx| {
        w.open(alpha.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        panel.update(cx, |g, _| g.node_count()),
        3,
        "a panel nobody sees did not reload"
    );
    assert_eq!(
        panel.update(cx, |g, _| g.origin()),
        Some(today),
        "nor did it follow the page behind it"
    );

    // Reopening the sidebar refreshes it once, from the index and the page now open.
    cx.simulate_keystrokes("cmd-shift-\\");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("right-sidebar").is_some(),
        "sidebar reopened"
    );
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel stays");
    assert_eq!(panel.update(cx, |g, _| g.node_count()), 4);
    assert_eq!(panel.update(cx, |g, _| g.origin()), Some(alpha));
}

/// AGENTS.md focus invariant: removing the focused graph must hand focus back, or every
/// workspace shortcut dies with it.
#[gpui::test]
fn closing_the_graph_panel_restores_focus(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let local = cx
        .debug_bounds("graph-local")
        .expect("Around this page filter")
        .center();
    cx.simulate_click(local, Modifiers::default());
    cx.run_until_parked();
    let canvas = cx.debug_bounds("graph-canvas").expect("panel canvas");
    cx.simulate_click(canvas.origin + point(px(6.), px(6.)), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(ws.update(cx, |w, _| w.graph_panel()).is_none());
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("jump-palette").is_some(),
        "workspace shortcuts work after closing the panel"
    );
}

/// AGENTS.md focus invariant: hiding the right sidebar removes the focused graph panel
/// from rendering. Focus must come back to the workspace, or every shortcut (including the
/// one that would reopen the sidebar) stops reaching a handler.
#[gpui::test]
fn hiding_the_sidebar_restores_focus_from_the_graph_panel(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let local = cx
        .debug_bounds("graph-local")
        .expect("Around this page filter")
        .center();
    cx.simulate_click(local, Modifiers::default());
    cx.run_until_parked();
    let canvas = cx.debug_bounds("graph-canvas").expect("panel canvas");
    cx.simulate_click(canvas.origin + point(px(6.), px(6.)), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-\\");
    cx.run_until_parked();
    // The panel is still logically open; it is just not drawn any more.
    assert!(ws.update(cx, |w, _| w.graph_panel()).is_some());
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("jump-palette").is_some(),
        "workspace shortcuts work after hiding the sidebar"
    );
}

/// AGENTS.md focus invariant: ⌘-click opens a pane, then the full-screen graph that had
/// focus disappears; focus must not go with it.
#[gpui::test]
fn graph_cmd_click_split_restores_focus(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    let alpha = app.update(cx, |a, _| a.store.find_page("Alpha").unwrap().unwrap());
    let nodes = graph.update(cx, |g, _| g.node_count());
    let i = (0..nodes)
        .find(|&i| graph.update(cx, |g, _| g.node_page(i)) == Some(alpha.clone()))
        .unwrap();
    let at = graph.update(cx, |g, _| g.node_screen_pos(i)).unwrap();
    let cmd = Modifiers {
        platform: true,
        ..Default::default()
    };
    cx.simulate_click(at, cmd);
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.stack_pages(cx)).last(),
        Some(&Some(alpha))
    );
    cx.simulate_keystrokes("cmd-k");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("jump-palette").is_some(),
        "workspace shortcuts work after the split opens"
    );
}

/// GR-3: a zoom control floats over the canvas, so its click must not also activate a node
/// that happens to be underneath it.
#[gpui::test]
fn graph_zoom_control_does_not_click_through(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    cx.simulate_input("[[Alpha]] meets [[Beta]]");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    let plus = cx
        .debug_bounds("graph-zoom-in")
        .expect("zoom control")
        .center();
    // At the maximum zoom a click on "+" no longer moves the canvas or the nodes.
    for _ in 0..24 {
        cx.simulate_click(plus, Modifiers::default());
        cx.run_until_parked();
    }
    assert!(
        graph.update(cx, |g, _| g.zoom()) > 7.9,
        "at the zoom ceiling"
    );

    // Drag a node until it sits exactly under the control.
    let canvas = cx.debug_bounds("graph-canvas").expect("canvas");
    let node = graph.update(cx, |g, _| g.node_screen_pos(0)).unwrap();
    let delta = plus - node;
    let start = canvas.origin + point(px(6.), px(6.));
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(start + delta, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(start + delta, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let under = graph.update(cx, |g, _| g.node_screen_pos(0)).unwrap();
    assert!(
        f32::from((under.x - plus.x).abs()) < 3. && f32::from((under.y - plus.y).abs()) < 3.,
        "node under the control: {under:?} {plus:?}"
    );

    let main = ws.update(cx, |w, cx| w.main_page(cx));
    cx.simulate_click(plus, Modifiers::default());
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_some(),
        "the zoom control does not open the node below it"
    );
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), main);
}

#[gpui::test]
fn dropped_files_become_attachments(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("minutes.txt");
    std::fs::write(&file, b"meeting minutes").unwrap();
    // While editing: inserted at the caret.
    cx.simulate_input("see ");
    ws.update_in(cx, |w, window, cx| {
        w.drop_files(std::slice::from_ref(&file), window, cx)
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let page = today_page(&app, cx);
    let hash = utopia_store::sha256_hex(b"meeting minutes");
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![(format!("see [minutes.txt](blob:{hash})").as_str(), 0)]
    );
    assert!(cx.debug_bounds("attachment-chip").is_some(), "chip drawn");
    // Not editing: appended as a new block.
    ws.update_in(cx, |w, window, cx| {
        w.drop_files(std::slice::from_ref(&file), window, cx)
    });
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 2);
    assert!(app.update(cx, |a, _| a.store.has_blob(&hash)));
}

/// A two-page PDF opened in the workspace's PDF viewer, at `window_size`.
#[cfg(target_os = "macos")]
fn pdf_viewer(
    cx: &mut TestAppContext,
    window_size: gpui::Size<gpui::Pixels>,
) -> (
    tempfile::TempDir,
    Entity<AppState>,
    Entity<Workspace>,
    Entity<utopia_ui::pdf_view::PdfView>,
    &mut VisualTestContext,
) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("g.utopia")).unwrap();
    store.set_today(Some(today()));
    let pdf = utopia_ui::pdf::sample_pdf(&[
        &["Leverage points matter", "a second line"],
        &["Page two text"],
    ]);
    let att = store.add_blob("paper.pdf", &pdf).unwrap();
    let page = store.create_page("Reading").unwrap();
    let host = store.ensure_first_block(&page).unwrap();
    store
        .set_block_text(
            &page,
            &host,
            &utopia_core::attach::markup(&att.name, &att.hash, &att.mime),
            None,
            None,
        )
        .unwrap();
    store.commit_block(&page, &host).unwrap();
    let app = cx.new(|_| AppState::new(store));
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    cx.simulate_resize(window_size);
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx);
        w.open_pdf(
            att.hash.clone(),
            att.name.clone(),
            None,
            Some(host.clone()),
            window,
            cx,
        );
    });
    cx.run_until_parked();
    let view = ws.update(cx, |w, _| w.pdf_view()).expect("viewer open");
    (dir, app, ws, view, cx)
}

/// §9.2 main window: the graph replaces what the main column was showing instead of being
/// painted over a PDF that stays alive underneath and silently reappears when the graph
/// closes (workspace-main-pane-content.md, issue 2).
#[cfg(target_os = "macos")]
#[gpui::test]
fn graph_over_a_pdf_does_not_bring_it_back(cx: &mut TestAppContext) {
    let (_dir, _app, ws, _view, cx) = pdf_viewer(cx, size(px(1280.), px(900.)));
    cx.simulate_keystrokes("cmd-shift-g");
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_some(),
        "the graph is up"
    );
    assert!(
        ws.update(cx, |w, _| w.pdf_view()).is_none(),
        "the graph replaced the PDF; it is not hidden behind it"
    );
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(ws.update(cx, |w, _| w.graph_view()).is_none());
    assert!(
        ws.update(cx, |w, _| w.pdf_view()).is_none(),
        "closing the graph returns to the page, not to a PDF from under it"
    );
}

/// P2 "PDF highlights": a release that no page claims — the gutter, the header, or
/// outside the window — ends the drag. Otherwise the rectangle keeps being drawn and the
/// next release can finish a drag nobody is making.
#[cfg(target_os = "macos")]
#[gpui::test]
fn p2_pdf_highlight_release_off_page_cancels_the_drag(cx: &mut TestAppContext) {
    use gpui::{Modifiers, MouseButton, MouseMoveEvent};
    let (_dir, _app, _ws, view, cx) = pdf_viewer(cx, size(px(1280.), px(900.)));
    let b = view
        .update(cx, |v, _| v.page_bounds(0))
        .expect("page drawn");
    let at = |x: f32, y: f32| {
        point(
            b.origin.x + b.size.width * x,
            b.origin.y + b.size.height * y,
        )
    };
    let drag_to = |cx: &mut VisualTestContext, position: gpui::Point<gpui::Pixels>| {
        cx.simulate_mouse_down(at(0.05, 0.07), MouseButton::Left, Modifiers::default());
        cx.simulate_event(MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
    };
    // Release in the gutter beside the page.
    drag_to(cx, at(0.9, 0.105));
    assert!(view.update(cx, |v, _| v.dragging()), "the drag is live");
    cx.simulate_mouse_up(
        point(b.origin.x - px(40.), at(0.5, 0.5).y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert!(
        !view.update(cx, |v, _| v.dragging()),
        "an off-page release ends the drag"
    );
    assert!(
        view.update(cx, |v, _| v.highlights().is_empty()),
        "and writes nothing"
    );
    // Released outside the window there is no hovered element to tell; the drag is over
    // when a move arrives with no button held.
    drag_to(cx, at(0.9, 0.105));
    cx.simulate_mouse_up(
        point(px(-50.), px(-50.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.simulate_event(MouseMoveEvent {
        position: at(0.5, 0.5),
        pressed_button: None,
        modifiers: Modifiers::default(),
    });
    assert!(
        !view.update(cx, |v, _| v.dragging()),
        "a move with no button held ends a drag whose release was missed"
    );
    assert!(view.update(cx, |v, _| v.highlights().is_empty()));
}

/// P2 "PDF highlights": a drag that ends on another page is cancelled, not finished
/// against the page it started from, where clamping turns it into a highlight that runs
/// to the page edge.
#[cfg(target_os = "macos")]
#[gpui::test]
fn p2_pdf_highlight_release_on_another_page_writes_nothing(cx: &mut TestAppContext) {
    use gpui::{Bounds, Modifiers, MouseButton, MouseMoveEvent};
    // Tall enough for both pages to be laid out.
    let (_dir, _app, _ws, view, cx) = pdf_viewer(cx, size(px(1280.), px(2200.)));
    let p0 = view
        .update(cx, |v, _| v.page_bounds(0))
        .expect("page 0 drawn");
    let p1 = view
        .update(cx, |v, _| v.page_bounds(1))
        .expect("page 1 drawn");
    assert!(p1.origin.y > p0.origin.y, "page 1 below page 0");
    let at = |b: Bounds<gpui::Pixels>, x: f32, y: f32| {
        point(
            b.origin.x + b.size.width * x,
            b.origin.y + b.size.height * y,
        )
    };
    cx.simulate_mouse_down(at(p0, 0.05, 0.07), MouseButton::Left, Modifiers::default());
    cx.simulate_event(MouseMoveEvent {
        position: at(p1, 0.5, 0.3),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_mouse_up(at(p1, 0.5, 0.3), MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    assert!(!view.update(cx, |v, _| v.dragging()));
    assert!(
        view.update(cx, |v, _| v.highlights().is_empty()),
        "no highlight clamped onto the page the drag started from"
    );
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn pdf_viewer_drag_creates_highlight_block(cx: &mut TestAppContext) {
    use gpui::{point, px, Modifiers, MouseButton, MouseMoveEvent};
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("g.utopia")).unwrap();
    store.set_today(Some(today()));
    let pdf = utopia_ui::pdf::sample_pdf(&[&["Leverage points matter", "a second line"]]);
    let att = store.add_blob("paper.pdf", &pdf).unwrap();
    let page = store.create_page("Reading").unwrap();
    let host = store.ensure_first_block(&page).unwrap();
    store
        .set_block_text(
            &page,
            &host,
            &utopia_core::attach::markup(&att.name, &att.hash, &att.mime),
            None,
            None,
        )
        .unwrap();
    store.commit_block(&page, &host).unwrap();
    let app = cx.new(|_| AppState::new(store));
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    cx.simulate_resize(gpui::size(px(1280.), px(820.)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx);
        w.open_pdf(
            att.hash.clone(),
            att.name.clone(),
            None,
            Some(host.clone()),
            window,
            cx,
        );
    });
    cx.run_until_parked();
    let view = ws.update(cx, |w, _| w.pdf_view()).expect("viewer open");
    assert_eq!(view.update(cx, |v, _| v.page_count()), 1);
    assert!(cx.debug_bounds("pdf-view").is_some());
    let b = view
        .update(cx, |v, _| v.page_bounds(0))
        .expect("page drawn");
    // Drag across the first line (about 9–11% down the page).
    let at = |x: f32, y: f32| {
        point(
            b.origin.x + b.size.width * x,
            b.origin.y + b.size.height * y,
        )
    };
    cx.simulate_mouse_down(at(0.05, 0.07), MouseButton::Left, Modifiers::default());
    cx.simulate_event(MouseMoveEvent {
        position: at(0.9, 0.105),
        pressed_button: Some(MouseButton::Left),
        modifiers: Modifiers::default(),
    });
    cx.simulate_mouse_up(at(0.9, 0.105), MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let hl = view.update(cx, |v, _| v.highlights().to_vec());
    assert_eq!(hl.len(), 1, "one highlight");
    assert!(
        hl[0].text.contains("Leverage points matter"),
        "{:?}",
        hl[0].text
    );
    assert!(!hl[0].text.contains("second line"));
    // It is a quote block under the PDF's block.
    let parent = app.update(cx, |a, _| {
        a.store.page_state(&page).unwrap().parent_of(&hl[0].block)
    });
    assert_eq!(parent, Some(host));
    assert_eq!(hl[0].rects.len(), 1, "a one-line text selection");
    // Across two lines: one rectangle per line, both lines quoted.
    let drag = |cx: &mut VisualTestContext,
                a: gpui::Point<gpui::Pixels>,
                b: gpui::Point<gpui::Pixels>,
                m: Modifiers| {
        cx.simulate_mouse_down(a, MouseButton::Left, m);
        cx.simulate_event(MouseMoveEvent {
            position: b,
            pressed_button: Some(MouseButton::Left),
            modifiers: m,
        });
        cx.simulate_mouse_up(b, MouseButton::Left, m);
        cx.run_until_parked();
    };
    // (Starting on line 2: pressing on an existing highlight opens its block instead.)
    drag(cx, at(0.3, 0.125), at(0.15, 0.09), Modifiers::default());
    let hl = view.update(cx, |v, _| v.highlights().to_vec());
    let two = hl
        .iter()
        .find(|h| h.rects.len() == 2)
        .expect("two-line highlight");
    assert!(two.text.contains("second"), "{:?}", two.text);
    assert!(cx.debug_bounds("pdf-hl-1-1").is_some() || cx.debug_bounds("pdf-hl-0-1").is_some());
    // Option-drag over blank paper: an area highlight with no quote.
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    drag(cx, at(0.2, 0.5), at(0.6, 0.7), alt);
    let hl = view.update(cx, |v, _| v.highlights().to_vec());
    assert_eq!(hl.len(), 3);
    assert!(hl
        .iter()
        .any(|h| h.text.is_empty() && h.rects.len() == 1 && h.rects[0][1] > 0.45));
    // A plain click is not a highlight.
    cx.simulate_click(at(0.5, 0.5), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.update(cx, |v, _| v.highlights().len()), 3);
}

/// Text of every live block on the daily page for `d`.
fn day_texts(app: &Entity<AppState>, d: NaiveDate, cx: &mut VisualTestContext) -> Vec<String> {
    let page = app.update(cx, |app, _| {
        app.store
            .find_page(&utopia_core::date::iso(d))
            .unwrap()
            .expect("daily page exists")
    });
    rows(app, &page, cx).into_iter().map(|(t, _)| t).collect()
}

#[gpui::test]
fn daily_notes_feed_scrolls_into_older_days(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // Notes on every other day before today: 25 older days.
    app.update(cx, |app, _| {
        for i in 1..=25 {
            let d = today() - chrono::Duration::days(i * 2);
            let (p, b) = app.store.open_daily(d).unwrap();
            app.store
                .set_block_text(&p, &b, &format!("note {i}"), None, None)
                .unwrap();
            app.store.commit_block(&p, &b).unwrap();
        }
        app.store.sync_index().unwrap();
    });
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.run_until_parked();
    let days = ws.update(cx, |w, cx| w.journal_days(cx));
    let today_id = today_page(&app, cx);
    assert_eq!(days[0], today_id, "today comes first");
    // Every calendar day is listed, written or blank; batches load until the end of the
    // feed is out of view, not all at once.
    assert!(
        (2..26).contains(&days.len()),
        "a screenful of days, not all 26 pages: {}",
        days.len()
    );
    let dates: Vec<NaiveDate> = app.update(cx, |app, _| {
        days.iter()
            .map(|p| app.store.page_state(p).unwrap().meta.daily_date.unwrap())
            .collect()
    });
    assert!(
        dates.windows(2).all(|w| w[0] > w[1]),
        "newest first: {dates:?}"
    );
    // The caret starts in today; Down crosses into the next day, editable in place.
    cx.simulate_input("hello");
    cx.simulate_keystrokes("down cmd-right");
    cx.simulate_input(" edited");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(day_texts(&app, today(), cx), ["hello"]);
    assert_eq!(
        day_texts(&app, today() - chrono::Duration::days(2), cx),
        ["note 1 edited"]
    );
    // Scrolling on loads older days, blank ones included; none are created (FR-1.2).
    for _ in 0..40 {
        if ws.update(cx, |w, cx| w.journal_days(cx)).len() == 26 {
            break;
        }
        scroll_main(cx, -4000.);
    }
    let days = ws.update(cx, |w, cx| w.journal_days(cx));
    assert_eq!(days.len(), 26, "every journal page");
    assert_eq!(daily_count(&app, cx), 26);
}

/// FR-1.1/FR-1.2: the feed lists every calendar day back to the oldest daily page and
/// then stops. Without a terminal state the sentinel kept asking for another batch for
/// as long as it was laid out, walking back through empty years.
#[gpui::test]
fn fr_1_1_feed_stops_at_the_oldest_daily_page(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // One page, older than the first batch, so there is something to load.
    let oldest = today() - chrono::Duration::days(23);
    app.update(cx, |app, _| {
        let (p, b) = app.store.open_daily(oldest).unwrap();
        app.store
            .set_block_text(&p, &b, "the first note", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
    });
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.run_until_parked();
    // Scroll far past the last real day.
    for _ in 0..20 {
        scroll_main(cx, -4000.);
    }
    let pages = ws.update(cx, |w, cx| w.journal_days(cx));
    assert_eq!(pages.len(), 2, "today's draft and the only written day");
    assert_eq!(pages[0], today_page(&app, cx), "today comes first");
    // The end of the feed is a terminal state, not an endless sentinel.
    assert!(
        cx.debug_bounds("feed-end").is_some(),
        "the feed says it has no earlier notes once the oldest day is loaded"
    );
    let shown = ws
        .update(cx, |w, cx| w.journal_shown_through(cx))
        .expect("in the feed");
    assert!(
        shown <= oldest,
        "the oldest day is inside the loaded window: {shown} vs {oldest}"
    );
    assert!(
        shown > oldest - chrono::Duration::days(10),
        "stops within one batch of the oldest day, not years past it: {shown}"
    );
    // Scrolling on asks for nothing more: the window is final.
    let open_before = ws.update(cx, |w, cx| w.open_journal_days(cx));
    for _ in 0..5 {
        scroll_main(cx, -4000.);
    }
    assert_eq!(
        ws.update(cx, |w, cx| w.journal_shown_through(cx)),
        Some(shown),
        "an exhausted feed never loads another batch"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w.open_journal_days(cx)),
        open_before,
        "no day opened on the extra scrolls either"
    );
    // Writing on a blank day inside the window writes that day (FR-1.2) but does not
    // move the feed's floor: the day is above `shown_through`, so the feed stays ended.
    // `today()` is 2026-11-02 in this file, so 25 days back is 2026-10-08: older than
    // the only note (2026-10-10) but inside the loaded window (down to 2026-10-04).
    let inside = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    assert_eq!(inside, today() - chrono::Duration::days(25));
    let folded = cx
        .debug_bounds("empty-day-2026-10-08")
        .expect("a blank day inside the window");
    cx.simulate_click(folded.center(), Modifiers::default());
    cx.run_until_parked();
    cx.simulate_input("late note");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.journal_shown_through(cx)),
        Some(shown),
        "writing inside the loaded window leaves the window where it was"
    );
    assert!(
        ws.update(cx, |w, cx| w.journal_days(cx))
            .contains(&PageId::for_date(inside)),
        "the blank day became a page"
    );
    // Nothing was created by scrolling; the one page is the day just written (FR-1.2).
    assert_eq!(daily_count(&app, cx), 2);
}

/// FR-1.2: visiting a day without writing on it only makes a draft, so it must not
/// extend the feed: the feed still ends at the oldest *written* day, not at whatever
/// date was last opened from the calendar.
#[gpui::test]
fn fr_1_2_visited_day_does_not_extend_the_feed(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // Three written days: the oldest is two days back.
    let oldest = today() - chrono::Duration::days(2);
    app.update(cx, |app, _| {
        for i in 0..3 {
            let d = today() - chrono::Duration::days(i);
            let (p, b) = app.store.open_daily(d).unwrap();
            app.store
                .set_block_text(&p, &b, &format!("note {i}"), None, None)
                .unwrap();
            app.store.commit_block(&p, &b).unwrap();
        }
        app.store.sync_index().unwrap();
    });
    // Visit (do not write) a day years back: a draft for the session (FR-1.2).
    let visited = NaiveDate::from_ymd_opt(2019, 1, 1).unwrap();
    ws.update_in(cx, |w, window, cx| {
        w.open_date(visited, false, false, window, cx)
    });
    cx.run_until_parked();
    assert!(
        app.update(cx, |app, _| app.store.is_draft(&PageId::for_date(visited))),
        "opening a date only drafts it"
    );
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.run_until_parked();
    // The visited draft must not make the feed load towards 2019.
    for _ in 0..20 {
        scroll_main(cx, -4000.);
    }
    assert!(
        cx.debug_bounds("feed-end").is_some(),
        "the feed ends at the oldest written day, not at the visited draft"
    );
    let shown = ws
        .update(cx, |w, cx| w.journal_shown_through(cx))
        .expect("in the feed");
    assert!(
        shown <= oldest && shown > oldest - chrono::Duration::days(10),
        "the window stays within one batch of the oldest written day: {shown} vs {oldest}"
    );
    // Visiting wrote nothing.
    assert_eq!(daily_count(&app, cx), 3);
}

/// FR-1.2: an exhausted feed re-opens when an older daily page appears later (from a
/// `[[date]]` link, an import or a sync). The terminal state is recomputed on every
/// rebuild, not latched: the sentinel comes back and the next batch reaches the day.
#[gpui::test]
fn fr_1_2_feed_reopens_when_an_older_day_appears(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // Today's note only: the feed ends at once.
    cx.simulate_input("today's note");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("feed-end").is_some(),
        "there is no older note yet"
    );
    // An older day appears without navigating the feed.
    let older = today() - chrono::Duration::days(14);
    let older_page = app.update(cx, |app, cx| {
        let (p, b) = app.store.open_daily(older).unwrap();
        app.store
            .set_block_text(&p, &b, "found later", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        app.changed(vec![p.clone()], true, cx);
        p
    });
    cx.run_until_parked();
    // The feed asks again and loads down to the day that appeared.
    for _ in 0..10 {
        scroll_main(cx, -4000.);
    }
    let shown = ws
        .update(cx, |w, cx| w.journal_shown_through(cx))
        .expect("in the feed");
    assert!(
        shown <= older && shown > older - chrono::Duration::days(10),
        "the window reaches the new oldest day within one batch: {shown} vs {older}"
    );
    let pages = ws.update(cx, |w, cx| w.journal_days(cx));
    assert_eq!(pages.len(), 2, "today and the day that appeared");
    assert!(pages.contains(&older_page));
}

fn scroll_main(cx: &mut VisualTestContext, dy: f32) {
    let main = cx.debug_bounds("main-pane").expect("main pane");
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: main.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy))),
        ..Default::default()
    });
    cx.run_until_parked();
}

#[gpui::test]
fn feed_blank_day_is_written_by_typing(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape");
    let yesterday = today() - chrono::Duration::days(1);
    let blank_sel: &'static str =
        Box::leak(format!("empty-day-{}", utopia_core::date::iso(yesterday)).into_boxed_str());
    let blank = cx.debug_bounds(blank_sel).expect("a blank day");
    cx.simulate_click(blank.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(daily_count(&app, cx), 0, "clicking is not writing");
    cx.simulate_input("filled in later");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let written: Vec<NaiveDate> = app.update(cx, |app, _| {
        app.store
            .list_pages(true)
            .unwrap()
            .into_iter()
            .filter_map(|p| p.daily_date)
            .collect()
    });
    assert_eq!(written.len(), 1);
    assert!(written[0] < today(), "an older day: {written:?}");
    assert_eq!(day_texts(&app, written[0], cx), ["filled in later"]);
}

/// DESIGN.md §3.6: an empty day is one row in a group; clicking it starts writing there,
/// and the day opens (with its heading) once it has a block. Clicking alone writes
/// nothing (FR-1.2).
#[gpui::test]
fn empty_day_opens_on_a_click(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.open_journal_days(cx)),
        1,
        "only today's draft has a section; yesterday is an empty row"
    );
    let day = today() - chrono::Duration::days(4);
    let selector: &'static str =
        Box::leak(format!("empty-day-{}", utopia_core::date::iso(day)).into_boxed_str());
    let row = cx.debug_bounds(selector).expect("an empty day row");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(daily_count(&app, cx), 0, "clicking is not writing");
    cx.simulate_input("picked up later");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(day_texts(&app, day, cx), ["picked up later"]);
    assert_eq!(
        ws.update(cx, |w, cx| w.open_journal_days(cx)),
        2,
        "the day now shows its date and its block, beside today"
    );
}

/// `docs/state-machines/draft-daily-page.md` issue 5: starting a day writes nothing, so it
/// must not report a structural change, which forces an index flush and an `IndexUpdated`
/// cycle for a commit that does not exist (FR-1.2).
#[gpui::test]
fn fr_1_2_starting_a_draft_does_not_flush_the_index(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let day = today() - chrono::Duration::days(4);
    let before = app.read_with(cx, |app, _| app.refs_version);
    let selector: &'static str =
        Box::leak(format!("empty-day-{}", utopia_core::date::iso(day)).into_boxed_str());
    let row = cx.debug_bounds(selector).expect("an empty day row");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        app.read_with(cx, |app, _| app.store.is_draft(&PageId::for_date(day))),
        "the click started a draft for the day"
    );
    assert_eq!(
        app.read_with(cx, |app, _| app.refs_version),
        before,
        "starting a draft queues no write, so the index is not refreshed"
    );
}

#[gpui::test]
fn shortcuts_work_after_scrolling_the_caret_away(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    // The caret stays in today's block while the feed scrolls far past it.
    for _ in 0..5 {
        scroll_main(cx, -4000.);
    }
    cx.simulate_keystrokes("cmd-[");
    cx.run_until_parked();
    let shown = ws.update(cx, |w, cx| w.main_page(cx)).unwrap();
    assert_eq!(shown, PageId::for_date(today().pred_opt().unwrap()));
}

/// P2 of the draft-day review: the first keystroke on a draft whose day a remote copy
/// already owns merges the draft into that page. The editor and the view still hold the
/// draft's id; they must follow it, or every later keystroke would edit a shard that no
/// longer exists (FR-1.2, rule 5).
#[gpui::test]
fn fr_1_2_typing_survives_a_merged_promotion(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let day = today();
    let draft = app.update(cx, |app, _| app.store.draft_daily(day).unwrap());
    // Show the day as a draft, caret in its first block, before the remote copy lands.
    ws.update_in(cx, |w, window, cx| {
        w.open_date(day, false, true, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(draft.clone()));
    // Another device deleted the day's deterministic id and wrote it under a fresh one;
    // only that page arrives, while the draft is still open here.
    let live = app.update(cx, |app, _| {
        let mut remote = Store::open_in_memory().unwrap();
        remote.set_today(Some(day));
        let gone = remote.ensure_daily_page(day).unwrap();
        remote.delete_page(&gone).unwrap();
        let live = remote.ensure_daily_page(day).unwrap();
        let shard = remote.export_since(&live, &Default::default()).unwrap();
        app.store.import_batch(&[(live.clone(), shard)]).unwrap();
        live
    });
    app.update(cx, |app, cx| app.changed(Vec::new(), false, cx));
    cx.run_until_parked();
    // First keystroke: promotes the draft, merges it into the day's page and redirects.
    cx.simulate_input("t");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(live.clone()),
        "the view follows the day"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w
            .editor()
            .read(cx)
            .target()
            .map(|t| t.page.clone())),
        Some(live.clone()),
        "the editor follows the day"
    );
    // The second keystroke must land on the day, not on the discarded draft.
    cx.simulate_input("y");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(rows(&app, &live, cx), [("ty".to_string(), 0)]);
}

/// A view can still be showing a draft when the same day arrives from sync under another
/// id. Opening the date elsewhere used to discard the draft's shard silently, leaving that
/// view on a missing page (FR-1.2: a remote copy of the day replaces the draft).
#[gpui::test]
fn fr_1_2_opening_a_day_replaces_a_stale_draft_everywhere(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let day = today();
    let draft = app.update(cx, |app, _| app.store.draft_daily(day).unwrap());
    ws.update_in(cx, |w, window, cx| {
        w.open_date(day, false, true, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(draft.clone()));
    // Another device wrote the day under a fresh id while this draft was open.
    let live = app.update(cx, |app, _| {
        let mut remote = Store::open_in_memory().unwrap();
        remote.set_today(Some(day));
        let gone = remote.ensure_daily_page(day).unwrap();
        remote.delete_page(&gone).unwrap();
        let live = remote.ensure_daily_page(day).unwrap();
        let shard = remote.export_since(&live, &Default::default()).unwrap();
        app.store.import_batch(&[(live.clone(), shard)]).unwrap();
        live
    });
    app.update(cx, |app, cx| app.changed(Vec::new(), false, cx));
    cx.run_until_parked();
    // Opening the date in a sidebar pane retires the stale draft.
    ws.update_in(cx, |w, window, cx| {
        w.open_date(day, true, false, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(live.clone()),
        "the open draft follows the day that replaced it"
    );
    assert!(
        app.update(cx, |app, _| app.store.page_state(&live).is_ok()),
        "the main pane shows a page that loads"
    );
}

/// The undo group saved the draft's id in its carets. Undo and redo put the editor back on
/// that id while the feed renders the survivor, so `BlockEditor::is_editing` stops matching
/// and the focused row disappears (FR-1.2, AGENTS.md "the list keeps rendering the row
/// being edited").
#[gpui::test]
fn fr_1_2_undo_after_a_merged_promotion_keeps_the_editor(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let day = today();
    let draft = app.update(cx, |app, _| app.store.draft_daily(day).unwrap());
    ws.update_in(cx, |w, window, cx| {
        w.open_date(day, false, true, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(draft.clone()));
    // Another device wrote the day under a fresh id; only that page arrives.
    let live = app.update(cx, |app, _| {
        let mut remote = Store::open_in_memory().unwrap();
        remote.set_today(Some(day));
        let gone = remote.ensure_daily_page(day).unwrap();
        remote.delete_page(&gone).unwrap();
        let live = remote.ensure_daily_page(day).unwrap();
        let shard = remote.export_since(&live, &Default::default()).unwrap();
        app.store.import_batch(&[(live.clone(), shard)]).unwrap();
        live
    });
    app.update(cx, |app, cx| app.changed(Vec::new(), false, cx));
    cx.run_until_parked();
    // One keystroke promotes the draft and merges it into the day's page.
    cx.simulate_input("t");
    cx.run_until_parked();
    let target_page = |ws: &Entity<Workspace>, cx: &mut VisualTestContext| {
        ws.update(cx, |w, cx| {
            w.editor().read(cx).target().map(|t| t.page.clone())
        })
    };
    // Undo restores the caret: it must point at the page that owns the day.
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(
        target_page(&ws, cx),
        Some(live.clone()),
        "undo puts the caret back on the day"
    );
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(live.clone()));
    // So does redo.
    cx.simulate_keystrokes("cmd-shift-z");
    cx.run_until_parked();
    assert_eq!(
        target_page(&ws, cx),
        Some(live.clone()),
        "redo puts the caret back on the day"
    );
    // The focused row is drawn, so the next keystroke reaches the day.
    cx.simulate_input("y");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(rows(&app, &live, cx), [("ty".to_string(), 0)]);
}

/// P3 of the draft-day review: visiting a date writes nothing, so none of the visit paths
/// may report a structural change (which forces a synchronous index flush and an
/// `IndexUpdated` cycle) (FR-1.2, FR-1.5).
#[gpui::test]
fn fr_1_2_visiting_a_day_does_not_flush_the_index(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let before = app.read_with(cx, |app, _| app.refs_version);

    // Quick jump / previous day (`open_date`): a draft until written.
    let day = today() - chrono::Duration::days(6);
    ws.update_in(cx, |w, window, cx| {
        w.open_date(day, false, false, window, cx)
    });
    cx.run_until_parked();
    assert!(
        app.read_with(cx, |app, _| app.store.is_draft(&PageId::for_date(day))),
        "the day is a draft"
    );
    assert_eq!(
        app.read_with(cx, |app, _| app.refs_version),
        before,
        "opening a draft day queues no write"
    );

    // The day header's calendar (`pick_date` on the page view).
    let title = cx.debug_bounds("page-title").expect("the day's header");
    cx.simulate_click(title.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    // The calendar opens on the day's own month (October, for `day` above).
    let cell = cx
        .debug_bounds("cal-pop-2026-10-15")
        .expect("the header calendar opens");
    cx.simulate_click(cell.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.refs_version),
        before,
        "the calendar queues no write"
    );

    // The Daily Notes feed (`open_journal`).
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.refs_version),
        before,
        "opening the feed queues no write"
    );
}

#[gpui::test]
fn fr_1_2_a_day_becomes_a_page_when_written(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    // Launch shows today with the caret in its block (AC-1.1), but writes nothing.
    assert_eq!(daily_count(&app, cx), 0);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(daily_count(&app, cx), 0, "looking is not writing");
    // Visiting other days (calendar, previous day) writes nothing either.
    let yesterday = today().pred_opt().unwrap();
    ws.update_in(cx, |w, window, cx| {
        w.open_date(yesterday, false, true, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-[");
    cx.run_until_parked();
    assert_eq!(daily_count(&app, cx), 0);
    // The first keystroke writes the day, under its date identity (§10.3).
    ws.update_in(cx, |w, window, cx| {
        w.open_date(yesterday, false, true, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_input("x");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(daily_count(&app, cx), 1);
    assert_eq!(day_texts(&app, yesterday, cx), ["x"]);
    let id = app.update(cx, |app, _| app.store.find_page("2026-11-01").unwrap());
    assert_eq!(id, Some(PageId::for_date(yesterday)));
    app.update(cx, |app, _| {
        let report = app.store.doctor().unwrap();
        assert!(report.is_ok(), "{report:?}");
    });
}

#[gpui::test]
fn daily_notes_feed_shows_linked_references_not_unlinked(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        let d = today() - chrono::Duration::days(1);
        let (p, b) = app.store.open_daily(d).unwrap();
        // Pre-creates Oct 30 (FR-1.10): empty, but it has a linked reference.
        app.store
            .set_block_text(&p, &b, "Follow up on [[October 30th, 2026]]", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        // Visited once before this build created pages on visit: empty, no references.
        app.store
            .open_daily(today() - chrono::Duration::days(5))
            .unwrap();
        app.store.sync_index().unwrap();
    });
    cx.update(|window, cx| {
        let ws = window.root::<Workspace>().flatten().expect("workspace");
        ws.update(cx, |w, cx| w.open_journal(window, cx));
    });
    cx.run_until_parked();
    let days: Vec<NaiveDate> = app.update(cx, |app, _| {
        let s = &mut app.store;
        s.journal_pages(10)
            .unwrap()
            .into_iter()
            .filter_map(|p| p.daily_date)
            .collect()
    });
    assert_eq!(
        days,
        [
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()
        ],
        "days with entries or references; the empty visited day is skipped"
    );
    assert!(
        cx.debug_bounds("refs-header").is_some(),
        "Oct 30's references"
    );
    assert!(cx.debug_bounds("unlinked-header").is_none());
}

#[gpui::test]
fn relative_date_links(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    // Autocomplete offers tomorrow's page first.
    cx.simulate_input("Pay rent [[tomorrow");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // A closed link is replaced once the caret leaves it.
    cx.simulate_keystrokes("cmd-right enter");
    cx.simulate_input("Met Ana [[Yesterday]] and #[[next friday]] too");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        day_texts(&app, today(), cx),
        [
            "Pay rent [[November 3rd, 2026]]",
            "Met Ana [[November 1st, 2026]] and #[[November 6th, 2026]] too",
        ]
    );
    app.update(cx, |app, _| {
        let s = &mut app.store;
        s.sync_index().unwrap();
        for iso in ["2026-11-03", "2026-11-01", "2026-11-06"] {
            let p = s.find_page(iso).unwrap().expect(iso);
            assert_eq!(s.backlink_count(&p).unwrap(), 1, "{iso}");
        }
        assert!(
            s.find_page("Yesterday").unwrap().is_none(),
            "no page named Yesterday"
        );
    });
}

/// The block editor's selected text, for scripts that check what is still selected.
fn selected_text(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
    ws.update(cx, |w, cx| {
        let e = w.editor().read(cx);
        e.field_text()[e.selection()].to_string()
    })
}

/// Typing `[` twice over a selected word makes it a page link, Roam's idiom: the opening
/// bracket surrounds the selection instead of replacing it, and the word stays selected.
#[gpui::test]
fn opening_bracket_wraps_the_selection(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        // A page the menu could offer for `[[launch`, so a menu opening would be visible.
        app.store.create_page("Launch checklist").unwrap();
    });
    cx.simulate_input("Met Ana about launch");
    cx.simulate_keystrokes("shift-left shift-left shift-left shift-left shift-left shift-left");
    assert_eq!(selected_text(&ws, cx), "launch");
    cx.simulate_input("[");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "Met Ana about [launch]");
    assert_eq!(selected_text(&ws, cx), "launch", "still selected, inside");
    cx.simulate_input("[");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "Met Ana about [[launch]]");
    assert_eq!(selected_text(&ws, cx), "launch");
    assert!(
        ws.update(cx, |w, cx| !w.editor().read(cx).autocomplete().is_open()),
        "the user made the link; no menu offers to replace it"
    );
    // Leaving the block commits it like any typed link.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        let launch = app.store.find_page("launch").unwrap().expect("linked page");
        assert_eq!(app.store.backlink_count(&launch).unwrap(), 1);
    });
}

/// Each wrap is one edit, so one undo takes back exactly one wrap and never leaves a
/// half-wrapped `(word` behind. The text is multi-byte and the selection ends on a
/// grapheme cluster (§13.5).
#[gpui::test]
fn bracket_wrap_is_one_undo_step(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("see 日本語👩‍👩‍👧 now");
    // Let the typing commit (300 ms idle) so it is its own undo step.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    cx.simulate_keystrokes("left left left left shift-left shift-left shift-left shift-left");
    assert_eq!(selected_text(&ws, cx), "日本語👩‍👩‍👧");
    cx.simulate_input("(");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "see (日本語👩‍👩‍👧) now");
    // Commit between the two, so they are separate undo steps rather than coalescing.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    cx.simulate_input("{");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "see ({日本語👩‍👩‍👧}) now");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "see (日本語👩‍👩‍👧) now");
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "see 日本語👩‍👩‍👧 now");
    cx.simulate_keystrokes("cmd-shift-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "see (日本語👩‍👩‍👧) now");
}

/// Only brackets wrap. A quote typed over a selection replaces it as in any text field
/// (the editor pairs nothing as you type), and IME composition is never wrapped.
#[gpui::test]
fn only_plain_brackets_wrap(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("say hi");
    cx.simulate_keystrokes("shift-left shift-left");
    cx.simulate_input("\"");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "say \"");
    cx.simulate_input("hi");
    cx.simulate_keystrokes("shift-left shift-left");
    // An IME composes over the selection, then commits a bracket: that is the IME's
    // text, not a keystroke over a selection, so it is inserted as is.
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "[", None, window, cx);
        e.replace_text_in_range(None, "[", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "say \"[");
}

/// The title of the suggestion the open menu has highlighted.
fn highlighted(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> String {
    use utopia_ui::editor::AcItem;
    ws.update(cx, |w, cx| {
        let e = w.editor().read(cx);
        let ac = e.autocomplete().menu().expect("a menu is open");
        match &ac.items[ac.selected] {
            AcItem::Page { title, .. } | AcItem::Create(title) => title.clone(),
            AcItem::Command { label, .. } => label.clone(),
            AcItem::Block { uid, .. } => uid.clone(),
        }
    })
}

/// Tab takes the highlighted suggestion in every menu, as Enter does (FR-2.17, FR-4.1,
/// FR-4.2); with no menu open it still indents (FR-2.4).
#[gpui::test]
fn tab_accepts_the_highlighted_suggestion(cx: &mut TestAppContext) {
    use utopia_core::BlockKind;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("Ana Lovelace").unwrap();
        app.store.create_page("Analytics").unwrap();
        app.store.create_page("Modern Religion").unwrap();
    });
    // `[[`: the first suggestion.
    cx.simulate_input("Met [[ana");
    cx.run_until_parked();
    let first = highlighted(&ws, cx);
    assert!(first.starts_with("Ana"), "{first}");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, format!("Met [[{first}]]"));
    // A moved highlight is what Tab takes.
    cx.simulate_input(" and [[ana");
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    let second = highlighted(&ws, cx);
    assert_ne!(second, first);
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        format!("Met [[{first}]] and [[{second}]]")
    );
    // `#`: the canonical multi-word tag.
    cx.simulate_input(" #mod");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        format!("Met [[{first}]] and [[{second}]] #[[Modern Religion]]")
    );
    // Nothing was indented or split along the way.
    assert_eq!(rows(&app, &page, cx).len(), 1);
    assert_eq!(rows(&app, &page, cx)[0].1, 0);

    // `((`: a block reference.
    cx.simulate_keystrokes("enter");
    cx.simulate_input("see ((Met");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    let (target, text) = app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        let roots = st.live_roots();
        (roots[0].clone(), st.block(&roots[1]).unwrap().text.clone())
    });
    assert_eq!(text, format!("see (({target}))"));

    // `/`: the slash menu runs the command.
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Plan /head");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    let r = rows(&app, &page, cx);
    assert_eq!(rows_str(&r)[2], ("Plan ", 0), "accepted, not indented");
    let kind = app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        st.block(&st.live_roots()[2]).unwrap().kind
    });
    assert_eq!(kind, BlockKind::Heading(1));

    // No menu: Tab indents as before.
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[2].1,
        1,
        "indented under the block above"
    );
}

/// Accepting is a rebindable action: moved off Tab, Tab is free to indent again even with
/// a menu open, and the new keys accept.
#[gpui::test]
fn accept_suggestion_can_be_rebound(cx: &mut TestAppContext) {
    use utopia_ui::actions::{reinstall, Overrides};
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    let mut o = Overrides::default();
    o.set("AcceptCompletion", "ctrl-y");
    cx.update(|_, cx| reinstall(cx, &o));
    app.update(cx, |app, _| {
        app.store.create_page("Ana Lovelace").unwrap();
    });
    cx.simulate_input("first");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("[[ana");
    cx.run_until_parked();
    let first = highlighted(&ws, cx);
    cx.simulate_keystrokes("ctrl-y");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[1].0, format!("[[{first}]]"));
    cx.simulate_input(" [[ana");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("first", 0), (format!("[[{first}]] [[ana]]").as_str(), 1)]
    );
}

/// A link in a rendered block carries its own hitbox (for the pointing-hand cursor);
/// clicking it must still follow the link, and clicking past it must still edit.
#[gpui::test]
fn clicking_a_rendered_link_follows_it(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (ana, src) = app.update(cx, |app, _| {
        let ana = app.store.create_page("Ana Lovelace").unwrap();
        let src = app.store.create_page("Reading").unwrap();
        let b = app.store.ensure_first_block(&src).unwrap();
        app.store
            .set_block_text(&src, &b, "[[Ana Lovelace]] lent me this", None, None)
            .unwrap();
        app.store.sync_index().unwrap();
        (ana, src)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(src.clone(), None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let content = cx.debug_bounds("content-1").expect("the block is rendered");
    let line = gpui::px(utopia_ui::theme::LINE_HEIGHT);
    // Past the link: the row starts editing, the page stays.
    let past = gpui::point(content.right() - gpui::px(4.), content.top() + line / 2.);
    cx.simulate_click(past, gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(src.clone()));
    assert!(ws.update(cx, |w, cx| w.editor().read(cx).target().is_some()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let on_link = gpui::point(content.left() + gpui::px(6.), content.top() + line / 2.);
    cx.simulate_click(on_link, gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, cx| w.main_page(cx)), Some(ana));
}

/// The editor's text and caret (before the block commits).
fn text_and_caret(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> (String, usize) {
    ws.update(cx, |w, cx| {
        let e = w.editor().read(cx);
        (e.field_text().to_string(), e.cursor())
    })
}

/// Brackets pair as you type: the closer appears with the caret between, a typed closer
/// steps over it, and before a word an opener is just an opener (§13.5).
#[gpui::test]
fn brackets_pair_as_you_type(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("call (");
    assert_eq!(text_and_caret(&ws, cx), ("call ()".into(), 6));
    cx.simulate_input("Ana)");
    assert_eq!(
        text_and_caret(&ws, cx),
        ("call (Ana)".into(), 10),
        "not `())`"
    );
    cx.simulate_input(" {x} [");
    assert_eq!(text_and_caret(&ws, cx), ("call (Ana) {x} []".into(), 16));
    cx.simulate_input("]");
    assert_eq!(text_and_caret(&ws, cx), ("call (Ana) {x} []".into(), 17));
    // Typing a whole link by hand still gives one pair of each.
    cx.simulate_input(" [[x]] done");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "call (Ana) {x} [] [[x]] done");
    // Before a word character: no closer.
    cx.simulate_keystrokes("home");
    cx.simulate_input("(");
    assert_eq!(
        text_and_caret(&ws, cx),
        ("(call (Ana) {x} [] [[x]] done".into(), 1)
    );
}

/// Backspace in an empty pair removes both brackets; with text inside, it is an ordinary
/// backspace until the pair is empty again.
#[gpui::test]
fn backspace_removes_an_empty_pair(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a (");
    cx.simulate_keystrokes("backspace");
    assert_eq!(text_and_caret(&ws, cx), ("a ".into(), 2));
    cx.simulate_input("[[");
    assert_eq!(text_and_caret(&ws, cx), ("a [[]]".into(), 4));
    cx.simulate_keystrokes("backspace");
    assert_eq!(text_and_caret(&ws, cx), ("a []".into(), 3));
    cx.simulate_keystrokes("backspace");
    assert_eq!(text_and_caret(&ws, cx), ("a ".into(), 2));
    cx.simulate_input("{x");
    cx.simulate_keystrokes("backspace");
    assert_eq!(text_and_caret(&ws, cx), ("a {}".into(), 3));
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "a ");
}

/// A pair is one edit: one undo takes both brackets back.
#[gpui::test]
fn bracket_pair_is_one_undo_step(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("note ");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    cx.simulate_input("(");
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "note ()");
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "note ");
    cx.simulate_keystrokes("cmd-shift-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "note ()");
    let _ = ws;
}

/// `[[`, `#[[` and `((` auto-close, the menus open as before, and accepting replaces the
/// closers rather than leaving a second set: `[[Page]]` with the caret after `]]`.
#[gpui::test]
fn auto_closed_links_complete_cleanly(cx: &mut TestAppContext) {
    use utopia_ui::editor::AcKind;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("Ana Lovelace").unwrap();
        app.store.create_page("Modern Religion").unwrap();
    });
    cx.simulate_input("Met [[");
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx), ("Met [[]]".into(), 6));
    cx.simulate_input("An");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.editor().read(cx).autocomplete().kind()),
        Some(AcKind::Page),
        "the [[ menu is open inside the auto-closed link"
    );
    // A pause mid-title does not commit `[[An]]` and create a page "An".
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        assert!(app.store.find_page("An").unwrap().is_none());
    });
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    let want = "Met [[Ana Lovelace]]";
    assert_eq!(text_and_caret(&ws, cx), (want.into(), want.len()));
    // Enter, and the `#[[` form.
    cx.simulate_input(" on #[[mod");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let want = "Met [[Ana Lovelace]] on #[[Modern Religion]]";
    assert_eq!(text_and_caret(&ws, cx), (want.into(), want.len()));
    // Once the caret is out of the link, the idle commit runs and the edge appears.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        let ana = app.store.find_page("Ana Lovelace").unwrap().unwrap();
        assert_eq!(app.store.backlink_count(&ana).unwrap(), 1);
        assert!(app.store.find_page("An").unwrap().is_none());
    });
    // `((`: a block reference, closers replaced too.
    cx.simulate_keystrokes("enter");
    cx.simulate_input("see ((Met");
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "see ((Met))");
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    let (target, text) = app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        let roots = st.live_roots();
        (roots[0].clone(), st.block(&roots[1]).unwrap().text.clone())
    });
    let want = format!("see (({target}))");
    assert_eq!(text, want);
    assert_eq!(text_and_caret(&ws, cx).1, want.len());
}

/// IME composition is the input method's text: a bracket it commits is not paired.
#[gpui::test]
fn ime_brackets_are_not_paired(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("x ");
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "(", None, window, cx);
        e.replace_text_in_range(None, "(", window, cx);
    });
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "x (");
}

/// IME text is provisional until the input method commits it (§13.5). A host that
/// finishes the block mid-composition — blur, Escape, navigation, the idle timer — must
/// wait for the input method instead of taking the composition away with the buffer.
#[gpui::test]
fn ac_13_5_host_commit_waits_for_the_ime(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "abにほ",
        "the provisional glyphs are shown"
    );
    // The idle timer (300 ms after the last keystroke) fires mid-composition.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "abにほ",
        "the idle commit waits for the input method"
    );
    assert_eq!(rows(&app, &page, cx)[0].0, "ab");
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).target().is_some()),
        "still editing, and still composing"
    );
    // The input method commits; now the queued commit runs.
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "にほ", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "abにほ",
        "the committed composition is what persists"
    );
    assert_eq!(
        editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx)),
        None,
        "no composition is left running"
    );
}

/// Backspace, Delete, Paste, Cut and soft breaks belong to the input method while it is
/// composing (§13.5): deleting from our side removes provisional glyphs the platform
/// still addresses by offset, and persists them halfway through a composition.
#[gpui::test]
fn ac_13_5_destructive_actions_leave_ime_composition_alone(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "abにほ",
        "the input method's glyphs are not ours to delete"
    );
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "ab",
        "and nothing was persisted"
    );
    let marked = editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx));
    assert_eq!(marked, Some(2..4), "the platform still composes にほ");
}

/// Cmd+B/Cmd+I/Cmd+` reflow the buffer around the selection (§13.5). Doing that under a
/// live composition leaves the marked range pointing at bytes it never covered, so the
/// platform's next update rewrites the wrong run of text.
#[gpui::test]
fn ac_13_5_mark_leaves_ime_composition_alone(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-b");
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "abにほ",
        "bold does not rewrite the text the input method owns"
    );
    let marked = editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx));
    assert_eq!(
        marked,
        Some(2..4),
        "the marked range still covers exactly にほ"
    );
    assert_eq!(rows(&app, &page, cx)[0].0, "ab");
}

/// A store change that lands mid-composition is not dropped (§13.5): the editor re-reads
/// once the input method lets go, so a block deleted or merged away under the composition
/// ends the edit instead of taking the composed text into a tombstone.
#[gpui::test]
fn ac_13_5_ime_composition_ends_when_its_block_is_deleted(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    use utopia_core::Command;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let block = app.update(cx, |app, _| {
        app.store.page_state(&page).unwrap().live_roots()[0].clone()
    });
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    // A remote device (or an undo) tombstones the block while the IME is composing.
    app.update(cx, |app, cx| {
        app.store
            .execute(
                &page,
                vec![Command::SetDeleted {
                    ids: vec![block.clone()],
                    deleted_at: Some(1),
                }],
                Default::default(),
            )
            .unwrap();
        app.changed(vec![page.clone()], true, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "abにほ",
        "the live composition is not thrown away"
    );
    // The input method commits; the block it was editing is gone.
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "にほ", window, cx)
    });
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).target().is_none()),
        "the edit ends: there is nowhere to put the text"
    );
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        assert_eq!(
            st.block(&block).unwrap().text,
            "abにほ",
            "the composed characters stay in the tombstone, where undo and the trash can \
             bring them back (rule 5)"
        );
    });
}

/// `utopia://search?q=` fills the palette's field, so the results must follow the text in
/// it — the host is told about a programmatic change as it is about typing (§13.5).
#[gpui::test]
fn ac_13_5_line_input_set_text_notifies_the_host(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        let page = app.store.create_page("November Retro").unwrap();
        let block = app.store.ensure_first_block(&page).unwrap();
        app.store
            .set_block_text(&page, &block, "november notes", None, None)
            .unwrap();
        app.store.create_page("Atlas").unwrap();
        app.store.sync_index().unwrap();
    });
    ws.update_in(cx, |w, window, cx| w.search_for("november", window, cx));
    cx.run_until_parked();
    let jump = ws
        .read_with(cx, |w, _| w.jump_palette())
        .expect("search is open");
    assert_eq!(
        jump.read_with(cx, |j, _| j.page_titles()),
        vec!["November Retro".to_string()],
        "the results are for the text the field shows"
    );
}

/// Add a second root block after the page's first, for scripts that move between blocks.
fn add_block(
    app: &Entity<AppState>,
    page: &PageId,
    text: &str,
    cx: &mut VisualTestContext,
) -> utopia_core::BlockId {
    use utopia_core::{BlockKind, Command};
    let id = utopia_core::BlockId::new();
    app.update(cx, |app, _| {
        let after = app.store.page_state(page).unwrap().live_roots()[0].clone();
        app.store
            .execute(
                page,
                vec![Command::InsertBlock {
                    id: id.clone(),
                    parent: None,
                    after: Some(after),
                    text: text.to_string(),
                    kind: BlockKind::Bullet,
                }],
                Default::default(),
            )
            .unwrap();
    });
    id
}

/// §13.5: every host that ends an edit does it to move focus (the palette, the graph,
/// another pane). The editor is then no longer the window's input handler and gpui never
/// tells the platform input context, so the IME's commit or `unmarkText` can never arrive:
/// the marked text must be taken as committed there, or the editor composes for the rest
/// of the session with every edit path returning early.
#[gpui::test]
fn ac_13_5_focus_leaving_commits_the_ime_composition(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    use utopia_ui::editor::EditTarget;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    let host = editor.read_with(cx, |e, _| e.target().cloned().expect("editing").host);
    let second = add_block(&app, &page, "second", cx);
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    // Cmd+Shift+F ends the edit and gives the palette the keyboard.
    cx.simulate_keystrokes("cmd-shift-f");
    cx.run_until_parked();
    assert_eq!(
        editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx)),
        None,
        "the composition is over: the input method can no longer reach the field"
    );
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "abにほ",
        "and what it marked was committed, not lost"
    );
    // The editor edits again normally.
    editor.update_in(cx, |e, window, cx| {
        e.begin(
            EditTarget {
                host,
                page: page.clone(),
                block: second.clone(),
                property: false,
            },
            None,
            None,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "second");
    cx.simulate_input("!");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[1].0, "second!");
}

/// §13.5: the latest host request wins, except the idle commit, which never displaces one.
/// A click on another block followed by a request to finish the block must not open that
/// block when the composition finally commits: focusing a row the host has since moved on
/// from is the stranded-focus bug every shortcut dies from.
#[gpui::test]
fn ac_13_5_a_later_request_supersedes_a_queued_begin(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    use utopia_ui::editor::EditTarget;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    let (host, first) = editor.read_with(cx, |e, _| {
        let t = e.target().cloned().expect("editing");
        (t.host, t.block)
    });
    let second = add_block(&app, &page, "second", cx);
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    // A click on the second block, queued behind the composition.
    editor.update_in(cx, |e, window, cx| {
        e.begin(
            EditTarget {
                host,
                page: page.clone(),
                block: second,
                property: false,
            },
            None,
            None,
            window,
            cx,
        )
    });
    // The host asks to finish the block instead (the undo and navigation paths call
    // `commit` the same way): the later request is the one that counts.
    editor.update(cx, |e, cx| e.commit(cx));
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "にほ", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |e, _| e.target().map(|t| t.block.clone())),
        Some(first),
        "the superseded click does not take the editor elsewhere"
    );
}

/// §13.5: the same has to happen when focus moves without any host calling `end` (a popup,
/// another window, the application deactivating). The platform sends no IME event to a
/// field that has stopped being its input handler, so the focus-out listener closes the
/// composition and keeps its text.
#[gpui::test]
fn ac_13_5_focus_out_without_end_keeps_the_composed_text(cx: &mut TestAppContext) {
    use gpui::{EntityInputHandler as _, Focusable as _};
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    // Focus leaves the editor; nobody ends the edit. (gpui only reports focus paths for an
    // active window, which the test platform does not make the window until asked.)
    ws.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| {
        let handle = w.focus_handle(cx);
        window.focus(&handle);
    });
    cx.run_until_parked();
    assert_eq!(
        editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx)),
        None,
        "the composition is closed: the input method can no longer reach the field"
    );
    assert_eq!(rows(&app, &page, cx)[0].0, "abにほ");
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).target().is_some()),
        "the edit itself is still open, just without a composition"
    );
    // And the block edits again normally.
    editor.update_in(cx, |e, window, cx| {
        let target = e.target().cloned().expect("still editing");
        e.begin(target, None, None, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_input("!?");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "abにほ!?");
}

/// §13.5: deleting the block being composed must still hand the caret to the neighbour the
/// host picks. The delete removes the row the editor renders, so focus leaves it; the
/// neighbour is a valid continuation, not a stale request. Taking the composition as
/// committed before the host changes the rows keeps both the text and that navigation
/// (AGENTS.md: a focused element that is not rendered gets no keystrokes).
#[gpui::test]
fn ac_13_5_deleting_a_composing_block_keeps_typing_in_its_neighbour(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("first");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("second");
    cx.run_until_parked();
    // The window has to be active for gpui to report the focus change the row removal
    // causes (the test platform leaves it inactive until asked).
    ws.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    let deleted = editor.read_with(cx, |e, _| e.target().cloned().expect("editing").block);
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    // Cmd+Shift+Backspace deletes it; the block above is where editing continues.
    cx.simulate_keystrokes("cmd-shift-backspace");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), vec![("first", 0)]);
    let first = app.update(cx, |app, _| {
        app.store.page_state(&page).unwrap().live_roots()[0].clone()
    });
    assert_eq!(
        editor.read_with(cx, |e, _| e.target().map(|t| t.block.clone())),
        Some(first.clone()),
        "the caret continues in the block the host chose"
    );
    cx.simulate_input("!");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "first!");
    // And the composition was kept with the block it belonged to, in its tombstone.
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).unwrap();
        assert_eq!(st.block(&deleted).unwrap().text, "secondにほ");
    });
}

/// §13.5: the idle commit a host queued before the composition does not run on top of the
/// IME's own persist. It waits for the 300 ms timer that persist re-arms, as it would
/// without a composition; otherwise every IME phrase runs the commit (NFC, hashtags, page
/// creation, undo seal) synchronously on the UI thread.
#[gpui::test]
fn ac_13_5_composition_leaves_the_idle_commit_to_its_timer(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        assert!(e.insert_text("see [[tomorrow]]", window, cx));
    });
    cx.run_until_parked();
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    // The idle timer fires mid-composition and is queued.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    // The input method commits.
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "にほ", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "see [[tomorrow]]にほ",
        "the queued idle commit waits for its timer, not for the input method"
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "see [[November 3rd, 2026]]にほ",
        "and then it runs as usual"
    );
    assert_eq!(rows(&app, &page, cx)[0].0, "see [[November 3rd, 2026]]にほ");
}

/// §13.5: a click on another block mid-composition is queued until the input method lets
/// go of the buffer, then applied: the marked text is persisted in the block it belonged
/// to and the other block starts editing.
#[gpui::test]
fn ac_13_5_begin_on_another_block_waits_for_the_ime(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    use utopia_ui::editor::EditTarget;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    let (host, first) = editor.read_with(cx, |e, _| {
        let t = e.target().cloned().expect("editing");
        (t.host, t.block)
    });
    let second = add_block(&app, &page, "second", cx);
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    editor.update_in(cx, |e, window, cx| {
        e.begin(
            EditTarget {
                host,
                page: page.clone(),
                block: second.clone(),
                property: false,
            },
            None,
            None,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |e, _| e.target().map(|t| t.block.clone())),
        Some(first.clone()),
        "the buffer still belongs to the input method"
    );
    assert_eq!(text_and_caret(&ws, cx).0, "abにほ");
    // The input method commits, and the click takes effect.
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "にほ", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        editor.read_with(cx, |e, _| e.target().map(|t| t.block.clone())),
        Some(second.clone())
    );
    assert_eq!(text_and_caret(&ws, cx).0, "second");
    assert_eq!(rows(&app, &page, cx)[0].0, "abにほ");
}

/// §13.5 covers every shape the platform uses: explicit UTF-16 ranges over text with a
/// non-BMP character before the caret, a dead key committing over its own mark, and
/// `unmarkText` (what AppKit sends when the IME is told to let go), which must keep the
/// marked glyphs as committed text.
#[gpui::test]
fn ac_13_5_ime_ranges_and_unmark_commit_cleanly(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("😀 x");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    // A dead key (Option+e) marks `´`: two bytes, one UTF-16 unit, after a 2-unit emoji.
    let marked = editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "´", None, window, cx);
        e.marked_text_range(window, cx)
    });
    assert_eq!(marked, Some(4..5), "UTF-16 offsets, not bytes");
    // The letter that completes it commits over the marked range the platform names.
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(Some(4..5), "é", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "😀 xé");
    assert_eq!(rows(&app, &page, cx)[0].0, "😀 xé");
    // `unmarkText`: the input method lets go without a commit; what it marked is text.
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "😀 xé", "still provisional");
    editor.update_in(cx, |e, window, cx| e.unmark_text(window, cx));
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "😀 xéにほ");
    assert_eq!(rows(&app, &page, cx)[0].0, "😀 xéにほ");
    assert_eq!(
        editor.update_in(cx, |e, window, cx| e.marked_text_range(window, cx)),
        None
    );
}

/// §13.5: an IME that empties its marked text (Escape twice in Kotoeri) ends the
/// composition, and a request queued behind it runs then rather than being stranded.
#[gpui::test]
fn ac_13_5_empty_marked_text_ends_the_composition(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        assert!(e.insert_text("see [[tomorrow]]", window, cx));
    });
    cx.run_until_parked();
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
        // Escape went to the IME first: the host asked for the block to be finished.
        e.commit(cx);
        // The IME gives up on its text.
        e.replace_and_mark_text_in_range(None, "", None, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "see [[November 3rd, 2026]]");
    assert_eq!(rows(&app, &page, cx)[0].0, "see [[November 3rd, 2026]]");
}

/// §13.5, kept as it is: a store change to a still-live block under a composition is
/// last-writer-wins on the block, so the text the user just committed replaces it. Only a
/// block that went away ends the edit (`ac_13_5_ime_composition_ends_when_its_block_is_deleted`).
#[gpui::test]
fn ac_13_5_a_live_store_change_under_a_composition_keeps_the_users_text(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ab");
    cx.run_until_parked();
    let block = app.update(cx, |app, _| {
        app.store.page_state(&page).unwrap().live_roots()[0].clone()
    });
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    // Undo (or a remote device) rewrites the block while the IME composes.
    app.update(cx, |app, cx| {
        app.store
            .set_block_text(&page, &block, "a", None, None)
            .unwrap();
        app.changed(vec![page.clone()], true, cx);
    });
    cx.run_until_parked();
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "にほ", window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "abにほ",
        "the composition is the newer edit of this block; merging concurrent edits is the \
         CRDT's job, not the editor's"
    );
}

/// No page titled like a relative date, and the day's page linked instead.
fn assert_tomorrow_linked(app: &Entity<AppState>, cx: &mut VisualTestContext) {
    app.update(cx, |app, _| {
        let s = &mut app.store;
        s.sync_index().unwrap();
        assert!(
            s.find_page("tomorrow").unwrap().is_none(),
            "no page named tomorrow"
        );
        let day = s.find_page("2026-11-03").unwrap().expect("tomorrow's page");
        assert!(s.backlink_count(&day).unwrap() >= 1);
    });
}

/// Review fix: a relative date made into a link by wrapping the selection is still
/// rewritten to the day's title when the block is finished, even though the wrap leaves
/// the caret inside the link (§10.3.1, relative date links).
#[gpui::test]
fn wrapped_relative_date_expands_on_finish(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_input("Pay rent tomorrow");
    cx.simulate_keystrokes(
        "shift-left shift-left shift-left shift-left shift-left shift-left shift-left shift-left",
    );
    cx.simulate_input("[[");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        day_texts(&app, today(), cx),
        ["Pay rent [[November 3rd, 2026]]"]
    );
    assert_tomorrow_linked(&app, cx);
}

/// Review fix: an auto-closed `[[tomorrow]]` whose menu was dismissed is rewritten when
/// the block is finished by Escape, Enter or moving to another block, with the caret
/// still before the auto-inserted `]]`.
#[gpui::test]
fn auto_closed_relative_date_expands_on_finish(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    // Escape twice: close the menu, then leave the block.
    cx.simulate_input("a [[tomorrow");
    cx.simulate_keystrokes("escape escape");
    cx.run_until_parked();
    assert_eq!(day_texts(&app, today(), cx), ["a [[November 3rd, 2026]]"]);
    assert_tomorrow_linked(&app, cx);
}

#[gpui::test]
fn auto_closed_relative_date_expands_on_enter(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_input("b [[tomorrow");
    cx.simulate_keystrokes("escape enter");
    cx.run_until_parked();
    assert_eq!(
        day_texts(&app, today(), cx),
        ["b [[November 3rd, 2026]]", ""],
        "the split lands after the finished link, not inside it"
    );
    assert_tomorrow_linked(&app, cx);
}

#[gpui::test]
fn auto_closed_relative_date_expands_on_moving_away(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    cx.simulate_input("first");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("c [[tomorrow");
    cx.simulate_keystrokes("escape up");
    cx.run_until_parked();
    assert_eq!(
        day_texts(&app, today(), cx),
        ["first", "c [[November 3rd, 2026]]"]
    );
    assert_tomorrow_linked(&app, cx);
}

/// AC-3.2: a block is committed on blur too, not only on Enter, Escape or the 300 ms
/// idle. The idle commit leaves a link the caret is still inside alone (`typing_link`), so
/// a block abandoned by a click elsewhere kept `[[tomorrow]]` unexpanded until some host
/// called `end` (machine issue 1).
#[gpui::test]
fn ac_3_2_blurring_the_editor_commits_the_block(cx: &mut TestAppContext) {
    use gpui::Focusable as _;
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("f [[tomorrow");
    cx.run_until_parked();
    assert_eq!(
        day_texts(&app, today(), cx),
        ["f [[tomorrow]]"],
        "auto-paired, with the caret inside the link"
    );
    let editor = ws.update(cx, |w, _| w.editor());
    assert!(
        editor.read_with(cx, |e, _| e.autocomplete().is_open()),
        "the [[ menu is open"
    );
    // The keyboard goes to the workspace; no host calls `end` (a click into the search box
    // or another pane does the same).
    ws.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| {
        let handle = w.focus_handle(cx);
        window.focus(&handle);
    });
    cx.run_until_parked();
    assert_eq!(day_texts(&app, today(), cx), ["f [[November 3rd, 2026]]"]);
    assert_tomorrow_linked(&app, cx);
    assert!(
        editor.read_with(cx, |e, _| !e.autocomplete().is_open()),
        "the menu was about text the commit rewrote"
    );
}

/// AC-3.2, §6.5: a commit that rewrites the text under the caret (`[[tomorrow]]` → the
/// day's title) moves the caret with it, so the CRDT anchor has to move too. Otherwise the
/// next store update resolves the caret back to where the old link was, inside the new one.
#[gpui::test]
fn ac_3_2_a_commit_reanchors_the_caret_it_moved(cx: &mut TestAppContext) {
    use gpui::Focusable as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("f [[tomorrow");
    cx.run_until_parked();
    // Blur commits with `finished = true`: the link expands and the caret goes past it.
    ws.update_in(cx, |_, window, _| window.activate_window());
    cx.run_until_parked();
    ws.update_in(cx, |w, window, cx| {
        let handle = w.focus_handle(cx);
        window.focus(&handle);
    });
    cx.run_until_parked();
    let want = "f [[November 3rd, 2026]]";
    let (shown, caret) = text_and_caret(&ws, cx);
    assert_eq!((shown.as_str(), caret), (want, want.len()));
    // A store-side edit appends at the end (a sync, another view, an undo): the anchored
    // caret is still where the commit left it, at the end of the block.
    let text = format!("{want}!");
    app.update(cx, |app, cx| {
        let block = app.store.page_state(&page).unwrap().live_roots()[0].clone();
        let r = app.store.set_block_text(&page, &block, &text, None, None);
        app.report(r);
        app.changed(vec![page.clone()], true, cx);
    });
    cx.run_until_parked();
    let (shown, caret) = text_and_caret(&ws, cx);
    assert_eq!(shown, text);
    assert!(
        caret >= want.len(),
        "the caret stays where the commit put it, at the end of the block, not back \
         inside the expanded link (caret {caret} of {})",
        text.len()
    );
}

/// Accepting the suggestion (Tab or Enter) inserts the day's title directly.
#[gpui::test]
fn auto_closed_relative_date_accepts_with_tab(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    cx.simulate_input("d [[tomorrow");
    cx.run_until_parked();
    cx.simulate_keystrokes("tab");
    cx.run_until_parked();
    let want = "d [[November 3rd, 2026]]";
    assert_eq!(text_and_caret(&ws, cx), (want.into(), want.len()));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(day_texts(&app, today(), cx), [want]);
    assert_tomorrow_linked(&app, cx);
}

/// SB-2, rule 10: the tag mark comes from a classification cached off the render path; a
/// store change the app was never told about must not be discovered while drawing a frame.
#[gpui::test]
fn tag_marks_come_from_the_index_feed_not_from_render(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        let (p, b) = app.store.open_daily(today()).unwrap();
        app.store
            .set_block_text(&p, &b, "#atlas", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
    });
    // Make the sidebar re-query its list, without running the index feed: rendering must
    // still use the cached classification.
    app.update(cx, |app, cx| {
        app.refs_version += 1;
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tag-chip-atlas").is_none(),
        "render must not call hashtag_only_pages (rule 10)"
    );
    // Once the app is told the index moved, the cache refreshes and the mark appears.
    app.update(cx, |app, cx| app.flush_now(cx));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tag-chip-atlas").is_some(),
        "the index feed refreshed the tag cache"
    );
}

/// SB-4: when a new exchange starts, the footer's dot must repaint as "syncing" at once.
/// Without a notification the window keeps showing the green dot from the previous round
/// for the whole network exchange.
#[gpui::test]
fn sync_dot_turns_amber_when_an_exchange_starts(cx: &mut TestAppContext) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use utopia_ui::sync_runner::{BoxTransport, SyncDot};

    let (_app, ws, cx) = setup(cx);
    let nudge = Arc::new(AtomicBool::new(false));
    let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
        utopia_sync::relay::Relay::new(),
        "graph",
    ));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(transport, Some(nudge.clone()), cx)
    });
    // The first round finishes quickly: the dot is green and painted.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(10));
    cx.run_until_parked();
    cx.update(|_, _| {});
    assert_eq!(ws.update(cx, |w, _| w.sync_dot()), SyncDot::Synced);
    assert_eq!(
        ws.update(cx, |w, _| w.rendered_sync_dot()),
        Some(SyncDot::Synced)
    );

    // Ask for the next round now and stop at the moment the exchange is in flight.
    nudge.store(true, Ordering::SeqCst);
    let mut started = false;
    for _ in 0..500 {
        if ws.update(cx, |w, _| w.sync_dot()) == SyncDot::Syncing {
            started = true;
            break;
        }
        // Move the test clock to the loop's next 250 ms poll (if it registered one), then
        // run exactly one queued task. The exchange is only queued after the loop takes
        // the transport, so it cannot run before the state is observed.
        cx.dispatcher.advance_clock_to_next_delayed();
        if !cx.background_executor.tick() {
            break;
        }
    }
    assert!(started, "the second exchange never started");
    // Flushing effects paints the window only if the state change requested a frame.
    cx.update(|_, _| {});
    assert_eq!(
        ws.update(cx, |w, _| w.rendered_sync_dot()),
        Some(SyncDot::Syncing),
        "the in-flight dot was never painted"
    );
}

/// SB-2 under the §15.1 budget: the tag-page classification is published by the commit
/// that changes the index, so an edit (and the index update after its flush) never runs the
/// whole-graph query on the UI thread.
#[gpui::test]
fn an_edit_does_not_classify_tag_pages_on_the_ui_thread(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("tags.utopia")).unwrap();
    store.set_today(Some(today()));
    // "atlas" has no blocks of its own and is reached only through a hashtag.
    store.create_page("atlas").unwrap();
    let notes = store.create_page("Notes").unwrap();
    let block = store.ensure_first_block(&notes).unwrap();
    store
        .set_block_text(&notes, &block, "Wrote about #atlas", None, None)
        .unwrap();
    store.commit_block(&notes, &block).unwrap();
    store.sync_index().unwrap();

    let app = cx.new(|_| AppState::new(store));
    assert_eq!(
        app.update(cx, |app, _| app.tag_pages.len()),
        1,
        "atlas is a tag page at startup"
    );
    // AppState classifies once at startup; after that the commit publishes the answer.
    let scans = app.update(cx, |app, _| app.store.tag_page_scans());
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app.clone(), window, cx));
    ws.update_in(cx, |w, window, cx| {
        w.open(notes.clone(), None, Some(block.clone()), false, window, cx)
    });
    cx.run_until_parked();
    cx.simulate_input("!");
    cx.run_until_parked();
    app.update(cx, |app, cx| app.flush_now(cx));
    app.update(cx, |app, _| app.store.sync_index().unwrap());
    cx.run_until_parked();
    assert_eq!(
        app.update(cx, |app, _| app.store.tag_page_scans()),
        scans,
        "typing and the index update did not re-classify on this thread"
    );
    assert_eq!(
        app.update(cx, |app, _| app.tag_pages.len()),
        1,
        "the set published by the commit is current"
    );

    // A change that drops the hashtag is published as well.
    app.update(cx, |app, cx| {
        app.store
            .set_block_text(&notes, &block, "Wrote about [[atlas]]", None, None)
            .unwrap();
        app.store.commit_block(&notes, &block).unwrap();
        app.flush_now(cx);
    });
    app.update(cx, |app, _| app.store.sync_index().unwrap());
    cx.run_until_parked();
    assert!(
        app.update(cx, |app, _| app.tag_pages.is_empty()),
        "a plain link is not a hashtag-only page"
    );
    assert_eq!(app.update(cx, |app, _| app.store.tag_page_scans()), scans);
}

/// DESIGN.md §3.7, rule 10: unlinked references are searched only while their row is
/// expanded, and the search runs in a handler (expansion or an index update), never in a
/// frame. A collapsed row costs nothing when the index moves.
#[gpui::test]
fn unlinked_is_searched_only_while_expanded(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let (page, source) = app.update(cx, |app, _| {
        let page = app.store.create_page("Atlas Launch").unwrap();
        let source = app.store.create_page("Notes").unwrap();
        let b = app.store.ensure_first_block(&source).unwrap();
        app.store
            .set_block_text(&source, &b, "Atlas Launch is the plan", None, None)
            .unwrap();
        app.store.commit_block(&source, &b).unwrap();
        app.store.sync_index().unwrap();
        (page, source)
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()),
        0,
        "opening a page does not search unlinked references"
    );

    let add_mention = |app: &mut AppState, cx: &mut gpui::Context<AppState>| {
        let plan = utopia_core::outline::append_block(app.store.page_state(&source).unwrap(), None);
        let b = app
            .store
            .execute_plan(&source, plan, None)
            .unwrap()
            .unwrap()
            .block;
        app.store
            .set_block_text(&source, &b, "Atlas Launch again", None, None)
            .unwrap();
        app.store.commit_block(&source, &b).unwrap();
        app.flush_now(cx);
    };
    app.update(cx, |app, cx| add_mention(app, cx));
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()),
        0,
        "an index change does not search a collapsed row"
    );

    let header = cx.debug_bounds("unlinked-header").expect("header");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()),
        1,
        "expanding the row searches it once"
    );

    app.update(cx, |app, cx| add_mention(app, cx));
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()),
        2,
        "an expanded row refreshes on the index update"
    );
    ws.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()),
        2,
        "a frame without an index change costs no scan"
    );

    let header = cx.debug_bounds("unlinked-header").expect("header");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    app.update(cx, |app, cx| add_mention(app, cx));
    cx.run_until_parked();
    assert_eq!(
        app.read_with(cx, |app, _| app.store.unlinked_scans()),
        2,
        "collapsing it again stops the searching"
    );
}

/// FR-1.2: a journal template gives a draft day blocks before it is written. Clicking that
/// day must still write nothing; the row the caret goes to is added to the draft in memory
/// and only becomes a page on the first keystroke, together with the template.
#[gpui::test]
fn template_backed_day_is_not_written_by_a_click(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    app.update(cx, |app, _| {
        let t = app.store.create_page("templates/journal").unwrap();
        let b = app.store.ensure_first_block(&t).unwrap();
        app.store
            .set_block_text(&t, &b, "Intention for today", None, None)
            .unwrap();
        app.store.commit_block(&t, &b).unwrap();
        app.store.sync_index().unwrap();
    });
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let day = today() - chrono::Duration::days(4);
    let selector: &'static str =
        Box::leak(format!("empty-day-{}", utopia_core::date::iso(day)).into_boxed_str());
    let row = cx.debug_bounds(selector).expect("an empty day row");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        daily_count(&app, cx),
        0,
        "clicking a template-backed day is not writing it"
    );
    // The template's block and the new note are written together on the first keystroke.
    cx.simulate_input("filled in later");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        day_texts(&app, day, cx),
        ["Intention for today", "filled in later"]
    );
}

/// §4.2 P2 (WASM plugin sandbox), rule 13: a plugin's host writes commit inside the call,
/// so a run that then traps must still tell the views which pages moved. The trap itself is
/// still reported; only the stale views were the bug (plugin run lifecycle issue 1).
#[gpui::test]
fn p2_a_failed_plugin_run_announces_the_pages_it_wrote(cx: &mut TestAppContext) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("a");
    cx.run_until_parked();

    // A plugin that captures through the API and then traps.
    let req = r#"{"op":"capture","text":"captured then trapped"}"#;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plugin.wat"),
        format!(
            r#"(module
  (import "utopia" "call" (func $call (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 0) "{data}")
  (func (export "utopia_alloc") (param i32) (result i32) (i32.const 8192))
  (func (export "utopia_run") (param i32) (param i32) (result i64)
    (drop (call $call (i32.const 0) (i32.const {len})))
    (unreachable)))"#,
            data = req.replace('"', "\\\""),
            len = req.len()
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("plugin.json"),
        serde_json::json!({
            "name": "trapper",
            "commands": [{"id": "go", "title": "Go"}],
            "permissions": ["write"],
            "module": "plugin.wat",
        })
        .to_string(),
    )
    .unwrap();
    let plugin = Rc::new(utopia_plugin::Plugin::load(dir.path()).unwrap());
    app.update(cx, |app, _| app.plugins.push(plugin));

    let seen: Rc<RefCell<Vec<PageId>>> = Rc::default();
    let sink = seen.clone();
    let _sub = cx.cx.update(|cx| {
        cx.subscribe(&app, move |_, e: &utopia_ui::app_state::Changed, _| {
            sink.borrow_mut().extend(e.pages.clone())
        })
    });
    ws.update_in(cx, |w, window, cx| {
        let editor = w.editor();
        editor.update(cx, |editor, cx| editor.run_plugin(0, "go", window, cx));
    });
    cx.run_until_parked();

    assert!(
        app.read_with(cx, |app, _| app.last_error.is_some()),
        "the trap is still reported to the user"
    );
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        assert!(
            !app.store
                .search_blocks("captured then trapped", 5)
                .unwrap()
                .is_empty(),
            "the write itself committed inside the call"
        );
    });
    assert!(
        seen.borrow().contains(&page),
        "the page the plugin wrote is announced, not only the failure: {:?}",
        seen.borrow()
    );
}
/// §4.2 P2: a plugin that rewrites the block being edited through the API replaces the
/// buffer under the editor. The caret's CRDT anchor and the autocomplete menu were derived
/// from the old text, so both have to follow the text that is now there (machine issue 3).
#[gpui::test]
fn p2_a_plugin_rewrite_reanchors_the_editor(cx: &mut TestAppContext) {
    use std::rc::Rc;

    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("see [[Al");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    let block = editor.read_with(cx, |e, _| e.target().cloned().expect("editing").block);
    assert!(
        editor.read_with(cx, |e, _| e.autocomplete().is_open()),
        "the link menu is open"
    );

    // A plugin that replaces this block's text through the API.
    let req = format!(r#"{{"op":"update_block","id":"{block}","text":"rewritten"}}"#);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plugin.wat"),
        format!(
            r#"(module
  (import "utopia" "call" (func $call (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 0) "{data}")
  (func (export "utopia_alloc") (param i32) (result i32) (i32.const 8192))
  (func (export "utopia_run") (param i32) (param i32) (result i64)
    (drop (call $call (i32.const 0) (i32.const {len})))
    (i64.const 0)))"#,
            data = req.replace('"', "\\\""),
            len = req.len()
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("plugin.json"),
        serde_json::json!({
            "name": "rewriter",
            "commands": [{"id": "go", "title": "Go"}],
            "permissions": ["write"],
            "module": "plugin.wat",
        })
        .to_string(),
    )
    .unwrap();
    let plugin = Rc::new(utopia_plugin::Plugin::load(dir.path()).unwrap());
    app.update(cx, |app, _| app.plugins.push(plugin));

    ws.update_in(cx, |w, window, cx| {
        let editor = w.editor();
        editor.update(cx, |e, cx| e.run_plugin(0, "go", window, cx));
    });
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "rewritten");
    assert!(
        editor.read_with(cx, |e, _| !e.autocomplete().is_open()),
        "the menu was about the text the plugin replaced"
    );
}

/// FR-2.17: `/zzz` matches no command, so the menu draws nothing — and must not exist as a
/// state either: Escape leaves the block the first time it is pressed (machine issue 1).
#[gpui::test]
fn fr_2_17_escape_leaves_a_menu_with_nothing_to_show(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("/zzz");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "/zzz");
    assert!(
        editor.read_with(cx, |e, _| e.target().is_none()),
        "one Escape leaves a block whose menu drew nothing"
    );
}

/// FR-2.17: Cmd+Enter starts a block above; whether it does must not depend on a menu with
/// no suggestions counting as "open" (machine issue 1).
#[gpui::test]
fn fr_2_17_cmd_enter_is_not_swallowed_by_an_empty_menu(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("/zzz");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-enter");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), [("", 0), ("/zzz", 0)]);
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        assert!(
            app.store.find_page("zzz").unwrap().is_none(),
            "no page is created from a slash query the menu never offered"
        );
    });
}

/// FR-2.17: the menu replaces the query it was opened for, not whatever range the caret
/// happens to sit in when Enter arrives (machine issue 2). Shift+Left puts the caret at the
/// start of a selection, one byte before the end of the query.
#[gpui::test]
fn fr_2_17_accept_replaces_the_query_the_menu_was_opened_for(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("Alpha").unwrap();
    });
    cx.simulate_input("Met [[Alp");
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).autocomplete().is_open()),
        "the [[ menu is open"
    );
    cx.simulate_keystrokes("shift-left");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "Met [[Alpha]]");
}

/// FR-2.17: a menu describes the buffer it was opened on. When the buffer is replaced
/// wholesale under it (an undo, a remote edit, a plugin through the store), its frozen
/// `start..caret` range names text that is gone; accepting from it would insert a
/// completion into the middle of another string.
#[gpui::test]
fn fr_2_17_a_menu_does_not_outlive_the_text_it_was_opened_for(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("Alpha").unwrap();
    });
    cx.simulate_input("[[Al");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    let block = editor.read_with(cx, |e, _| e.target().cloned().expect("editing").block);
    assert!(
        editor.read_with(cx, |e, _| e.autocomplete().is_open()),
        "the [[ menu is open"
    );
    // The store rewrites the block: the caret offset 4 now falls inside a three-byte
    // character, so the menu's range is not even a character boundary any more.
    app.update(cx, |app, cx| {
        let r = app
            .store
            .set_block_text(&page, &block, "ab日", None, Some(2));
        assert!(app.report(r).is_some());
        app.changed(vec![page.clone()], false, cx);
    });
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "ab日");
    assert!(
        !editor.read_with(cx, |e, _| e.autocomplete().is_open()),
        "the menu was about text that is no longer there"
    );
    // Enter is the block's again: it splits rather than taking a suggestion.
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows_str(&rows(&app, &page, cx)), [("ab日", 0), ("", 0)]);
}

/// §13.5: the buffer belongs to the input method while it composes, so a click must not
/// open a menu over provisional glyphs (machine issue 3).
#[gpui::test]
fn ac_13_5_mouse_down_does_not_open_a_menu_mid_composition(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("Ana Lovelace").unwrap();
    });
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "[[An", None, window, cx)
    });
    cx.run_until_parked();
    let bounds = cx.debug_bounds("block-editor").expect("editor bounds");
    cx.simulate_mouse_down(bounds.center(), MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    assert!(
        !editor.read_with(cx, |e, _| e.autocomplete().is_open()),
        "the menu belongs to committed text, not to the input method's glyphs"
    );
}

/// FR-2.17: a dismissed trigger stays closed for the query it was dismissed at, not for
/// whatever trigger later lands on the same byte offset (machine issue 4). Select-all and
/// paste is one edit, so nothing in between clears the suppression.
#[gpui::test]
fn fr_2_17_a_dismissed_trigger_follows_its_query(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    today_page(&app, cx);
    app.update(cx, |app, _| {
        app.store.create_page("random").unwrap();
    });
    cx.simulate_input("#ran");
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-a");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("#rod".into()));
    cx.simulate_keystrokes("cmd-v");
    cx.run_until_parked();
    assert!(
        ws.update(cx, |w, cx| w.editor().read(cx).autocomplete().is_open()),
        "a different query at the same offset is a fresh menu"
    );
}

/// FR-2.17, §5.5: a commit normalizes the block text to NFC, which shortens the buffer
/// under an open menu (`cafe` + U+0301 → `café`). The menu's frozen range has to follow
/// the rewrite, or accepting a block ref replaces the wrong bytes and leaves `((uid)))`.
#[gpui::test]
fn fr_2_17_a_commit_that_normalizes_the_text_moves_the_menu_with_it(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    // A block the `((` menu can offer for "notes".
    let target = app.update(cx, |app, _| {
        let p = app.store.create_page("Coffee").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "coffee notes", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        b
    });
    cx.simulate_input("cafe");
    let editor = ws.update(cx, |w, _| w.editor());
    // The input method commits a combining acute: the buffer holds the decomposed form
    // until the block is committed.
    editor.update_in(cx, |e, window, cx| {
        e.replace_text_in_range(None, "\u{301}", window, cx)
    });
    cx.simulate_input(" ((notes");
    cx.run_until_parked();
    assert!(
        editor.read_with(cx, |e, _| e.autocomplete().is_open()),
        "the (( menu is open"
    );
    // The idle commit (300 ms) normalizes the text to NFC: one byte shorter, with the
    // caret and the trigger one byte earlier.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, format!("café (({target}))"));
}

/// FR-2.17, §10.3.1: the idle commit also expands a relative date elsewhere in the block
/// (`[[tomorrow]]` → the day's title), shifting the open menu's offsets the same way NFC
/// normalization does. The menu has to be re-read before Enter takes a suggestion.
#[gpui::test]
fn fr_2_17_an_expanded_relative_date_moves_the_open_menu_with_it(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    let target = app.update(cx, |app, _| {
        let p = app.store.create_page("Coffee").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, "coffee notes", None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
        b
    });
    cx.simulate_input("see [[tomorrow]] and ((notes");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    assert!(
        editor.read_with(cx, |e, _| e.autocomplete().is_open()),
        "the (( menu is open"
    );
    // The idle commit expands the link before the trigger, so the whole block grows.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        format!("see [[November 3rd, 2026]] and (({target}))")
    );
}

// ---------------------------------------------------------------------------------------
// macOS text editing shortcuts (§9.1, §13.5). "Line" is the wrapped line, as in TextEdit;
// every deletion is one edit through the normal command path, so one undo step.
// ---------------------------------------------------------------------------------------

/// The block the editor owns and the text in it, before the block commits.
fn editing_block(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<(String, String)> {
    ws.update(cx, |w, cx| {
        let e = w.editor().read(cx);
        Some((e.target()?.block.0.clone(), e.field_text().to_string()))
    })
}

/// The caret within the field (before the block commits).
fn caret(ws: &Entity<Workspace>, cx: &mut VisualTestContext) -> usize {
    ws.update(cx, |w, cx| w.editor().read(cx).cursor())
}

/// Let the 300 ms idle commit and the 500 ms undo coalescing window pass, so what was typed
/// so far is one sealed undo step and the next edit is another.
fn settle_typing(cx: &mut VisualTestContext) {
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(600));
    cx.run_until_parked();
}

/// Alt+Shift+Left/Right extend the selection by a word (macOS' word-wise selection).
#[gpui::test]
fn alt_shift_arrows_select_by_the_word(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("hello big world");
    cx.simulate_keystrokes("alt-shift-left");
    assert_eq!(selected_text(&ws, cx), "world");
    cx.simulate_keystrokes("alt-shift-left");
    assert_eq!(selected_text(&ws, cx), "big world");
    settle_typing(cx);
    assert_eq!(
        rows(&app, &page, cx)[0].0,
        "hello big world",
        "selecting is not an edit"
    );

    // The other direction, from the start of the block.
    cx.simulate_keystrokes("right home alt-shift-right");
    assert_eq!(selected_text(&ws, cx), "hello");
    cx.simulate_keystrokes("alt-shift-right");
    assert_eq!(selected_text(&ws, cx), "hello big");
}

/// A plain word move starts from the edge the move heads towards: with a selection,
/// Option+Left goes to the word before its left edge and Option+Right one past its right
/// edge, rather than travelling from whichever end the caret happens to be at.
#[gpui::test]
fn word_moves_start_from_the_edge_they_move_towards(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let _page = today_page(&app, cx);
    cx.simulate_input("one two three four");
    cx.simulate_keystrokes("alt-shift-left alt-shift-left");
    assert_eq!(selected_text(&ws, cx), "three four");
    // Rightward from the far edge: the caret ends after `four`.
    cx.simulate_keystrokes("alt-right");
    assert_eq!(caret(&ws, cx), "one two three four".len());
    // Leftward from the near edge: the start of `two`, one word before the selection.
    cx.simulate_keystrokes("alt-shift-left alt-shift-left");
    assert_eq!(selected_text(&ws, cx), "three four");
    cx.simulate_keystrokes("alt-left");
    assert_eq!(text_and_caret(&ws, cx).1, "one ".len());
    assert_eq!(selected_text(&ws, cx), "");
}

/// Long enough to wrap in the test window, so the first visual line is not the whole block.
const WRAPPED: &str = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo \
                       lima mike november oscar papa quebec romeo sierra tango uniform victor";

/// Cmd+Shift+Left/Right extend the selection to the ends of the *wrapped* line, which is
/// what TextEdit means by a line; a soft break inside the block is a line end too. Pressed
/// again at a wrap they stay on that line, and one after the other they select the line.
#[gpui::test]
fn cmd_shift_arrows_select_to_the_ends_of_the_wrapped_line(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let _page = today_page(&app, cx);
    cx.simulate_resize(size(px(620.), px(700.)));
    cx.simulate_input(WRAPPED);
    cx.run_until_parked();
    cx.simulate_keystrokes("home cmd-shift-right");
    let first_line = selected_text(&ws, cx);
    assert!(
        !first_line.is_empty() && first_line.len() < WRAPPED.len(),
        "the paragraph wrapped: {first_line:?}"
    );
    assert!(WRAPPED.starts_with(&first_line), "{first_line:?}");
    cx.simulate_keystrokes("cmd-shift-right");
    assert_eq!(
        selected_text(&ws, cx),
        first_line,
        "again: stays on the first line"
    );
    // Cmd+Right stops on the line, before its last character, not on the next line.
    cx.simulate_keystrokes("home cmd-right");
    let mut line = first_line.clone();
    line.pop();
    assert_eq!(&WRAPPED[..caret(&ws, cx)], line);
    // From the middle of the line: Cmd+Shift+Left, then Cmd+Shift+Right, is the line.
    cx.simulate_keystrokes("left left cmd-shift-left");
    assert_eq!(selected_text(&ws, cx), &first_line[..first_line.len() - 3]);
    cx.simulate_keystrokes("cmd-shift-right");
    assert_eq!(selected_text(&ws, cx), first_line);
    // A soft break inside the block is a line end as well.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("one");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("two");
    cx.simulate_keystrokes("home cmd-shift-right");
    assert_eq!(selected_text(&ws, cx), "one");
}

/// Shift+Home and Shift+End extend the selection to where Home and End go in a block:
/// its start and end, across wrapped lines.
#[gpui::test]
fn shift_home_and_shift_end_select_to_the_ends_of_the_block(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let _page = today_page(&app, cx);
    cx.simulate_resize(size(px(620.), px(700.)));
    cx.simulate_input(WRAPPED);
    cx.run_until_parked();
    cx.simulate_keystrokes("home right shift-end");
    assert_eq!(selected_text(&ws, cx), &WRAPPED[1..]);
    cx.simulate_keystrokes("end left shift-home");
    assert_eq!(selected_text(&ws, cx), &WRAPPED[..WRAPPED.len() - 1]);
}

/// Option+Delete deletes back to the previous word start as one edit, in one grapheme-aware
/// step, and one undo brings the word back. At the very start of a block it does nothing:
/// only plain Backspace joins two blocks (rule 5).
#[gpui::test]
fn alt_backspace_deletes_to_the_previous_word_start(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("日本語 テスト done");
    settle_typing(cx);
    cx.simulate_keystrokes("alt-backspace");
    assert_eq!(text_and_caret(&ws, cx), ("日本語 テスト ".into(), 20));
    settle_typing(cx);
    cx.simulate_keystrokes("alt-backspace");
    assert_eq!(text_and_caret(&ws, cx), ("日本語 ".into(), 10));
    settle_typing(cx);
    // One undo per deletion, not one per character.
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "日本語 テスト ");
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "日本語 テスト done");

    // At offset 0 of a block with one above it: nothing is deleted and nothing merges.
    cx.simulate_keystrokes("cmd-down enter");
    cx.simulate_input("second");
    cx.simulate_keystrokes("home alt-backspace");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("日本語 テスト done", 0), ("second", 0)],
        "Option+Delete never joins blocks"
    );
}

/// Option+Fn+Delete deletes forward to the next word end, grapheme by grapheme.
#[gpui::test]
fn alt_delete_deletes_to_the_next_word_end(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("ハロー wörld ok");
    settle_typing(cx);
    cx.simulate_keystrokes("home right alt-delete");
    assert_eq!(text_and_caret(&ws, cx), ("ハ wörld ok".into(), "ハ".len()));
    cx.simulate_keystrokes("alt-delete");
    assert_eq!(text_and_caret(&ws, cx).0, "ハ ok");
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx).0,
        "ハ wörld ok",
        "one undo per word"
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(
        rows(&app, &page, cx).len(),
        1,
        "inside one block throughout"
    );
}

/// Cmd+Backspace deletes to the start of the wrapped line and stops there: at the line's
/// start it deletes nothing, so it cannot merge blocks either.
#[gpui::test]
fn cmd_backspace_deletes_to_the_start_of_the_wrapped_line(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("one two");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("three");
    settle_typing(cx);
    cx.simulate_keystrokes("cmd-backspace");
    assert_eq!(text_and_caret(&ws, cx), ("one two\n".into(), 8));
    // Already at the line start: nothing to delete, and no merge with anything.
    cx.simulate_keystrokes("cmd-backspace");
    assert_eq!(text_and_caret(&ws, cx), ("one two\n".into(), 8));
    settle_typing(cx);
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(text_and_caret(&ws, cx).0, "one two\nthree");
    cx.simulate_keystrokes("escape");
    assert_eq!(rows(&app, &page, cx).len(), 1, "one block, never merged");
}

/// Cmd+Delete pressed twice before the next frame stops at the wrapped line: with the
/// layout out of date it must not fall back to the whole paragraph and delete the lines
/// above.
#[gpui::test]
fn cmd_backspace_repeated_before_a_frame_stays_on_the_wrapped_line(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    let _page = today_page(&_app, cx);
    cx.simulate_resize(size(px(620.), px(700.)));
    cx.simulate_input(WRAPPED);
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-left");
    let last_line = caret(&ws, cx);
    assert!(last_line > 0, "the paragraph wrapped");
    cx.simulate_keystrokes("end");
    cx.run_until_parked();
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |_, window, cx| {
        window.dispatch_action(Box::new(utopia_ui::actions::DeleteToLineStart), cx);
        window.dispatch_action(Box::new(utopia_ui::actions::DeleteToLineStart), cx);
    });
    cx.run_until_parked();
    assert_eq!(
        text_and_caret(&ws, cx),
        (WRAPPED[..last_line].to_string(), last_line)
    );
}

/// Option+Up/Down go to the ends of this block's text first, and only from there carry on
/// into the block above or below (Utopia's paragraph is the block).
#[gpui::test]
fn alt_up_and_down_move_to_the_block_edges_and_cross_blocks(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("alpha");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("beta gamma");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("delta");
    cx.run_until_parked();
    // Caret at the end of `delta`; Option+Up goes to the start of `delta`…
    cx.simulate_keystrokes("alt-up");
    assert_eq!(
        editing_block(&ws, cx).map(|(_, text)| text),
        Some("delta".into()),
        "still in the same block"
    );
    assert_eq!(caret(&ws, cx), 0);
    // …and once there, to the start of the block above.
    cx.simulate_keystrokes("alt-up");
    cx.run_until_parked();
    assert_eq!(
        editing_block(&ws, cx).map(|(_, text)| text),
        Some("beta gamma".into())
    );
    assert_eq!(caret(&ws, cx), 0);
    // Option+Down walks back out: end of this block, then end of the one below.
    cx.simulate_keystrokes("alt-down");
    assert_eq!(caret(&ws, cx), "beta gamma".len());
    cx.simulate_keystrokes("alt-down");
    cx.run_until_parked();
    assert_eq!(
        editing_block(&ws, cx).map(|(_, text)| text),
        Some("delta".into())
    );
    assert_eq!(caret(&ws, cx), "delta".len());

    // A selection collapses to the edge the move heads towards before crossing anything.
    cx.simulate_keystrokes("alt-shift-left");
    assert_eq!(selected_text(&ws, cx), "delta");
    cx.simulate_keystrokes("alt-up");
    assert_eq!(caret(&ws, cx), 0);
    assert_eq!(selected_text(&ws, cx), "");
    assert_eq!(rows(&app, &page, cx).len(), 3);
}

/// Option+Shift+Up/Down extend the selection to the ends of the block's text and, as the
/// macOS "select to the end" keys do, grow it on that side rather than shrink it.
#[gpui::test]
fn alt_shift_up_and_down_select_to_the_block_edges(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    let _page = today_page(&_app, cx);
    cx.simulate_input("one two three");
    cx.simulate_keystrokes("alt-left alt-shift-up");
    assert_eq!(selected_text(&ws, cx), "one two ");
    cx.simulate_keystrokes("alt-shift-up");
    assert_eq!(selected_text(&ws, cx), "one two ", "clamped at the start");
    cx.simulate_keystrokes("alt-shift-down");
    assert_eq!(
        selected_text(&ws, cx),
        "one two three",
        "grows, never collapses"
    );
    cx.simulate_keystrokes("right shift-home");
    assert_eq!(
        selected_text(&ws, cx),
        "one two three",
        "Shift+Home: to where Home goes"
    );
    cx.simulate_keystrokes("left shift-end");
    assert_eq!(
        selected_text(&ws, cx),
        "one two three",
        "Shift+End: to where End goes"
    );
}

/// Cmd+Up/Down are the ends of the block's text, even when it holds several lines.
#[gpui::test]
fn cmd_up_and_down_move_to_the_block_edges(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    let _page = today_page(&_app, cx);
    cx.simulate_input("first");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("second");
    assert_eq!(caret(&ws, cx), "first\nsecond".len());
    cx.simulate_keystrokes("cmd-up");
    assert_eq!(caret(&ws, cx), 0);
    cx.simulate_keystrokes("cmd-down");
    assert_eq!(caret(&ws, cx), "first\nsecond".len());
    // Cmd+Shift+Up/Down still move the block: they are a different binding.
    cx.simulate_keystrokes("cmd-shift-up");
    assert_eq!(
        ws.update(cx, |w, cx| w.editor().read(cx).field_text().to_string()),
        "first\nsecond"
    );
}

/// Ctrl+A/E go to the ends of the paragraph (Cocoa's `moveToBeginningOfParagraph:`),
/// Ctrl+B/F step one grapheme, and a selection collapses to the edge the move heads for.
#[gpui::test]
fn ctrl_a_e_b_f_move_around_the_line(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    let _page = today_page(&_app, cx);
    cx.simulate_input("hello world");
    cx.simulate_keystrokes("home ctrl-f ctrl-f");
    assert_eq!(caret(&ws, cx), 2);
    cx.simulate_keystrokes("ctrl-b");
    assert_eq!(caret(&ws, cx), 1);
    cx.simulate_keystrokes("ctrl-e");
    assert_eq!(caret(&ws, cx), "hello world".len());
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(caret(&ws, cx), 0);
    // A soft break ends the paragraph.
    cx.simulate_keystrokes("ctrl-e shift-enter");
    cx.simulate_input("next");
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(caret(&ws, cx), "hello world\n".len());
    cx.simulate_keystrokes("left ctrl-a");
    assert_eq!(caret(&ws, cx), 0);

    // Grapheme-aware: `日` is one step, and so is a family emoji.
    cx.simulate_keystrokes("cmd-a");
    cx.simulate_input("日👩‍👩‍👧x");
    cx.simulate_keystrokes("ctrl-a ctrl-f");
    assert_eq!(caret(&ws, cx), "日".len());
    cx.simulate_keystrokes("ctrl-f");
    assert_eq!(caret(&ws, cx), "日👩‍👩‍👧".len());
    cx.simulate_keystrokes("ctrl-b");
    assert_eq!(caret(&ws, cx), "日".len());

    // With a selection, Ctrl+F lands on its right edge and Ctrl+B on its left.
    cx.simulate_keystrokes("ctrl-a shift-right shift-right");
    cx.simulate_keystrokes("ctrl-f");
    assert_eq!(caret(&ws, cx), "日👩‍👩‍👧".len());
    assert_eq!(selected_text(&ws, cx), "");
    cx.simulate_keystrokes("shift-left ctrl-b");
    assert_eq!(caret(&ws, cx), "日".len());
    assert_eq!(selected_text(&ws, cx), "");
}

/// On a wrapped paragraph Cmd+Left/Right stop at the wrap and Ctrl+A/E do not: the same
/// split as TextEdit.
#[gpui::test]
fn cmd_arrows_stop_at_the_wrap_and_ctrl_a_e_at_the_paragraph(cx: &mut TestAppContext) {
    let (_app, ws, cx) = setup(cx);
    let _page = today_page(&_app, cx);
    cx.simulate_resize(size(px(620.), px(700.)));
    cx.simulate_input(WRAPPED);
    cx.run_until_parked();
    cx.simulate_keystrokes("home cmd-right");
    let wrap = caret(&ws, cx);
    assert!(
        wrap > 0 && wrap < WRAPPED.len(),
        "Cmd+Right stops at the end of the first wrapped line: {wrap}"
    );
    cx.simulate_keystrokes("cmd-right");
    assert_eq!(caret(&ws, cx), wrap, "and stays there");
    cx.simulate_keystrokes("cmd-left");
    assert_eq!(caret(&ws, cx), 0);
    cx.simulate_keystrokes("ctrl-e");
    assert_eq!(caret(&ws, cx), WRAPPED.len());
    // From the last line Cmd+Left goes to that line's start, not the block's.
    cx.simulate_keystrokes("cmd-left");
    let last_line = caret(&ws, cx);
    assert!(last_line > wrap, "{last_line} > {wrap}");
    cx.simulate_keystrokes("ctrl-a");
    assert_eq!(caret(&ws, cx), 0);
}

/// Ctrl+P/N are Up/Down, including the step into the block above or below.
#[gpui::test]
fn ctrl_p_and_ctrl_n_move_a_line_and_cross_blocks(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("above");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("below");
    cx.run_until_parked();
    cx.simulate_keystrokes("ctrl-p");
    cx.run_until_parked();
    assert_eq!(
        editing_block(&ws, cx).map(|(_, text)| text),
        Some("above".into()),
        "Ctrl+P crosses into the block above"
    );
    cx.simulate_keystrokes("ctrl-n");
    cx.run_until_parked();
    assert_eq!(
        editing_block(&ws, cx).map(|(_, text)| text),
        Some("below".into())
    );
    cx.simulate_keystrokes("escape");
    assert_eq!(rows(&app, &page, cx).len(), 2);
}

/// Ctrl+K kills to the end of the paragraph and Ctrl+Y puts it back at the caret. At a
/// line break it kills the break; at the end of the block nothing, so it never joins
/// blocks, and an empty kill keeps the buffer. Each kill and yank is one undo step.
#[gpui::test]
fn ctrl_k_kills_to_the_end_of_the_line_and_ctrl_y_yanks_it_back(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("first line");
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("second");
    settle_typing(cx);
    cx.simulate_keystrokes("home right right right ctrl-k");
    assert_eq!(
        text_and_caret(&ws, cx),
        ("fir\nsecond".into(), 3),
        "to the end of the paragraph, not the block"
    );
    cx.simulate_keystrokes("ctrl-y");
    assert_eq!(text_and_caret(&ws, cx), ("first line\nsecond".into(), 10));
    // At the line break: the break goes, the two lines join inside the block.
    cx.simulate_keystrokes("ctrl-k");
    assert_eq!(text_and_caret(&ws, cx), ("first linesecond".into(), 10));
    // At the end of the block: nothing to kill, no merge, and the buffer keeps "\n".
    cx.simulate_keystrokes("cmd-down ctrl-k");
    assert_eq!(text_and_caret(&ws, cx).0, "first linesecond");
    cx.simulate_keystrokes("ctrl-y");
    assert_eq!(text_and_caret(&ws, cx).0, "first linesecond\n");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx).len(), 1, "Ctrl+K never merges blocks");

    // One undo per kill and per yank, although they came within the coalescing window.
    let undo = |cx: &mut VisualTestContext| {
        cx.simulate_keystrokes("cmd-z");
        cx.run_until_parked();
        rows(&app, &page, cx)[0].0.clone()
    };
    assert_eq!(undo(cx), "first linesecond", "the yank");
    assert_eq!(undo(cx), "first line\nsecond", "the kill of the break");
    assert_eq!(undo(cx), "fir\nsecond", "the first yank");
    assert_eq!(undo(cx), "first line\nsecond", "the first kill");
}

/// Ctrl+D and Ctrl+H are Delete and Backspace, including Backspace joining two blocks.
#[gpui::test]
fn ctrl_d_and_ctrl_h_delete_characters(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("above");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("abc");
    cx.simulate_keystrokes("home ctrl-d");
    assert_eq!(text_and_caret(&ws, cx), ("bc".into(), 0));
    cx.simulate_keystrokes("cmd-down ctrl-h");
    assert_eq!(text_and_caret(&ws, cx), ("b".into(), 1));
    cx.simulate_keystrokes("home ctrl-h");
    cx.run_until_parked();
    assert_eq!(
        rows_str(&rows(&app, &page, cx)),
        vec![("aboveb", 0)],
        "Ctrl+H at the start of a block is Backspace: it joins with the block above"
    );
}

/// Ctrl+T swaps the graphemes either side of the caret; at the end of the line the two
/// before it swap instead, a cluster is never split, and each swap is one undo step.
#[gpui::test]
fn ctrl_t_transposes_graphemes(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("abcd");
    settle_typing(cx);
    cx.simulate_keystrokes("home right ctrl-t");
    assert_eq!(text_and_caret(&ws, cx), ("bacd".into(), 2));
    cx.simulate_keystrokes("cmd-down ctrl-t");
    assert_eq!(
        text_and_caret(&ws, cx),
        ("badc".into(), 4),
        "at the end: the two before"
    );
    cx.simulate_keystrokes("cmd-z");
    cx.run_until_parked();
    assert_eq!(rows(&app, &page, cx)[0].0, "bacd", "one undo per swap");

    // A family emoji is one grapheme and swaps whole.
    cx.simulate_keystrokes("cmd-down enter");
    cx.run_until_parked();
    cx.simulate_input("日👩‍👩‍👧");
    cx.simulate_keystrokes("ctrl-t");
    assert_eq!(text_and_caret(&ws, cx), ("👩‍👩‍👧日".into(), "日👩‍👩‍👧".len()));
}

/// Ctrl+O breaks the line after the caret and leaves the caret where it was.
#[gpui::test]
fn ctrl_o_opens_a_line_after_the_caret(cx: &mut TestAppContext) {
    let (app, ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    cx.simulate_input("onetwo");
    settle_typing(cx);
    cx.simulate_keystrokes("home right right right ctrl-o");
    assert_eq!(text_and_caret(&ws, cx), ("one\ntwo".into(), 3));
    cx.simulate_input("!");
    assert_eq!(text_and_caret(&ws, cx), ("one!\ntwo".into(), 4));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        rows(&app, &page, cx).len(),
        1,
        "a line break, not a new block"
    );
}

/// While an input method composes, none of the new editing keys touches the buffer: the
/// glyphs belong to the IME until it commits (§13.5).
#[gpui::test]
fn editing_shortcuts_wait_for_the_input_method(cx: &mut TestAppContext) {
    use gpui::EntityInputHandler as _;
    let (_app, ws, cx) = setup(cx);
    let _page = today_page(&_app, cx);
    cx.simulate_input("ab cd");
    settle_typing(cx);
    cx.simulate_keystrokes("ctrl-a ctrl-k ctrl-y");
    let editor = ws.update(cx, |w, _| w.editor());
    editor.update_in(cx, |e, window, cx| {
        e.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.run_until_parked();
    for keys in [
        "alt-backspace",
        "alt-delete",
        "cmd-backspace",
        "ctrl-k",
        "ctrl-y",
        "ctrl-t",
        "ctrl-o",
        "ctrl-d",
        "ctrl-h",
    ] {
        cx.simulate_keystrokes(keys);
        assert_eq!(
            text_and_caret(&ws, cx).0,
            "ab cdにほ",
            "{keys} during composition"
        );
    }
}

/// §12.7: Off cancels the loop, and a replacement gets exactly one loop.
#[gpui::test]
fn ac_12_7_disabling_sync_stops_polling_and_reenable_starts_one_loop(cx: &mut TestAppContext) {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;
    use utopia_sync::relay::{Relay, RelayTransport};
    let (_app, ws, cx) = setup(cx);
    let relay = Relay::new();
    let calls = Arc::new(AtomicUsize::new(0));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(CountingTransport {
                inner: RelayTransport::new(relay.clone(), "g"),
                calls: calls.clone(),
            }),
            None,
            cx,
        );
    });
    ws.update(cx, |w, cx| {
        drop(w.stop_sync(cx));
    });
    for _ in 0..4 {
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    cx.update(|_, cx| assert!(!utopia_ui::sync_runner::facts(cx).is_on()));
    ws.update(cx, |w, cx| {
        w.enable_sync_with(
            Box::new(CountingTransport {
                inner: RelayTransport::new(relay, "g"),
                calls: calls.clone(),
            }),
            None,
            cx,
        );
    });
    for _ in 0..4 {
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

/// Titles of every page that is not a day, after the index has caught up.
fn page_titles(app: &Entity<AppState>, cx: &mut VisualTestContext) -> Vec<String> {
    app.update(cx, |app, _| {
        app.store.sync_index().unwrap();
        let mut t: Vec<String> = app
            .store
            .list_pages(true)
            .unwrap()
            .into_iter()
            .filter(|p| !p.is_daily())
            .map(|p| p.title.clone())
            .collect();
        t.sort();
        t
    })
}

/// One keystroke at a time with a pause after each that is longer than the idle commit
/// (300 ms, AC-3.2), the way a slow typist leaves the block alone mid-word.
fn type_slowly(cx: &mut VisualTestContext, keys: &[&str]) {
    for k in keys {
        cx.simulate_keystrokes(k);
        cx.run_until_parked();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(400));
        cx.run_until_parked();
    }
}

#[gpui::test]
fn ac_3_2_a_slow_hashtag_creates_no_page_per_pause(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    // FR-2.15: `#Apple` is the same link as `[[Apple]]`, so a pause inside it must not
    // commit it any more than a pause inside `[[Appl` does.
    type_slowly(cx, &["#", "shift-a", "p", "p", "l", "e"]);
    assert_eq!(page_titles(&app, cx), Vec::<String>::new());
    // Leaving the tag is what commits it: one page, the whole word.
    type_slowly(cx, &["space", "x"]);
    assert_eq!(page_titles(&app, cx), vec!["Apple".to_string()]);
}

#[gpui::test]
fn ac_3_2_a_slow_link_creates_no_page_per_pause(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    type_slowly(cx, &["[", "[", "shift-a", "p", "p", "l", "e"]);
    assert_eq!(page_titles(&app, cx), Vec::<String>::new());
    type_slowly(cx, &["]", "]", "space", "x"]);
    assert_eq!(page_titles(&app, cx), vec!["Apple".to_string()]);
}

#[gpui::test]
fn ac_3_2_leaving_an_auto_closed_link_creates_no_page(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    // `[[` pairs to `[[|]]`, so the text says `[[Ap]]`, but the user never closed it:
    // AC-3.2's "`[[Alpha` (unclosed) does not create a page" holds on Escape too.
    type_slowly(cx, &["[", "[", "shift-a", "p", "escape", "escape"]);
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Ap]]");
    assert_eq!(page_titles(&app, cx), Vec::<String>::new());
}

#[gpui::test]
fn ac_3_2_leaving_a_closed_link_still_creates_its_page(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    // Typing the closer steps over the paired one: the user closed it, so it commits.
    type_slowly(cx, &["[", "[", "shift-a", "p", "]", "]", "escape"]);
    assert_eq!(page_titles(&app, cx), vec!["Ap".to_string()]);
}

#[gpui::test]
fn ac_3_2_a_hashtag_left_mid_word_commits_as_typed(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    // A bare tag has no closer, so Escape finishes it as typed (FR-2.15).
    type_slowly(cx, &["#", "shift-a", "p", "escape"]);
    assert_eq!(page_titles(&app, cx), vec!["Ap".to_string()]);
}

#[gpui::test]
fn fr_2_17_the_caret_between_closing_brackets_is_not_in_the_title(cx: &mut TestAppContext) {
    let (app, _ws, cx) = setup(cx);
    let page = today_page(&app, cx);
    // One `]` steps over the first paired closer: `[[Guiding Activity]|]`. The menu must
    // not offer `Create "Guiding Activity]"` there, so Enter splits as usual.
    cx.simulate_input("[[Guiding Activity");
    cx.run_until_parked();
    type_slowly(cx, &["]", "enter"]);
    assert_eq!(page_titles(&app, cx), vec!["Guiding Activity".to_string()]);
    assert_eq!(rows(&app, &page, cx)[0].0, "[[Guiding Activity]");
}
