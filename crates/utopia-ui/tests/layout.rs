//! Layout geometry (§9.2) checked on GPUI's test platform: the window is laid out and
//! painted headlessly and element bounds are read back by `debug_selector`.

use chrono::NaiveDate;
use gpui::{px, size, AppContext as _, Bounds, Entity, Pixels, TestAppContext, VisualTestContext};
use utopia_core::outline;
use utopia_core::{BlockId, BlockKind, Command, PageId};
use utopia_store::{EditOpts, Store};
use utopia_ui::actions::install_keymap;
use utopia_ui::app_state::AppState;
use utopia_ui::theme::{Theme, CHILD_INDENT, SIDEBAR_WIDTH};
use utopia_ui::workspace::Workspace;

const W: f32 = 1280.;
const H: f32 = 820.;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
}

fn setup(cx: &mut TestAppContext) -> &mut VisualTestContext {
    setup_seeded(cx, |_| {})
}

/// The workspace over a store seeded directly (never through keystrokes): deterministic
/// content for the sidebar, and no race with the UI's debounce.
fn setup_seeded(cx: &mut TestAppContext, seed: impl FnOnce(&mut Store)) -> &mut VisualTestContext {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today()));
    seed(&mut store);
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx
}

/// Like [`setup`], but the graph already holds `titles` (Quick jump result rows).
fn setup_titles<'a>(cx: &'a mut TestAppContext, titles: &[&str]) -> &'a mut VisualTestContext {
    setup_seeded(cx, |store| {
        for t in titles {
            store.create_page(t).unwrap();
        }
    })
}

/// `n` days of history, two blocks each, so the old two-line preview would have shown
/// both (SB-1).
fn seed_days(store: &mut Store, n: usize) {
    for i in 0..n {
        let d = today() - chrono::Duration::days(i as i64);
        let (p, b) = store.open_daily(d).unwrap();
        store
            .set_block_text(&p, &b, &format!("Day {i} first block"), None, None)
            .unwrap();
        store.commit_block(&p, &b).unwrap();
        let plan = outline::append_block(store.page_state(&p).unwrap(), None);
        let second = store.execute_plan(&p, plan, None).unwrap().unwrap().block;
        store
            .set_block_text(&p, &second, &format!("Day {i} second block"), None, None)
            .unwrap();
        store.commit_block(&p, &second).unwrap();
    }
}

/// Today's page on its own (the calendar's "Today"), not the Daily Notes feed.
fn setup_page(cx: &mut TestAppContext) -> &mut VisualTestContext {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape");
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open_date(
            NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(),
            false,
            true,
            window,
            cx,
        )
    });
    cx.run_until_parked();
    cx
}

fn bounds(cx: &mut VisualTestContext, sel: &'static str) -> Bounds<Pixels> {
    bounds_of(cx, sel.to_string())
}

/// `debug_bounds` only takes a `&'static str`; a test-built selector is leaked for the
/// duration of the process (a handful of short strings).
fn bounds_of(cx: &mut VisualTestContext, sel: String) -> Bounds<Pixels> {
    let sel: &'static str = Box::leak(sel.into_boxed_str());
    cx.run_until_parked();
    cx.debug_bounds(sel)
        .unwrap_or_else(|| panic!("{sel} was not rendered"))
}

fn approx(a: Pixels, b: f32) -> bool {
    (f32::from(a) - b).abs() < 1.5
}

#[gpui::test]
fn three_column_window(cx: &mut TestAppContext) {
    let cx = setup(cx);
    let left = bounds(cx, "left-sidebar");
    let main = bounds(cx, "main-pane");
    assert!(
        approx(left.origin.x, 0.) && approx(left.size.width, SIDEBAR_WIDTH),
        "{left:?}"
    );
    assert!(
        approx(left.size.height, H),
        "sidebar fills the height: {left:?}"
    );
    assert!(
        approx(main.origin.x, SIDEBAR_WIDTH),
        "main pane starts after the sidebar: {main:?}"
    );
    assert!(
        approx(main.size.width, W - SIDEBAR_WIDTH),
        "main pane fills the rest: {main:?}"
    );
    assert!(
        cx.debug_bounds("right-sidebar").is_none(),
        "right sidebar starts closed"
    );
    // Cmd+Shift+\ opens the right sidebar at the right edge (§9.2).
    cx.simulate_keystrokes("escape cmd-shift-\\");
    let right = bounds(cx, "right-sidebar");
    let main = bounds(cx, "main-pane");
    assert!(
        approx(right.size.width, 380.) && approx(right.origin.x + right.size.width, W),
        "{right:?}"
    );
    assert!(
        approx(main.size.width, W - SIDEBAR_WIDTH - 380.),
        "{main:?}"
    );
    assert!(
        main.origin.x + main.size.width <= right.origin.x + px(0.5),
        "no overlap"
    );
}

#[gpui::test]
fn outline_rows_indent_and_stack(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("parent");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab enter");
    cx.simulate_input("grandchild");
    cx.simulate_keystrokes("tab escape");
    let title = bounds(cx, "page-title");
    // Items: 0 header, 1..=3 rows.
    let r1 = bounds(cx, "content-1");
    let r2 = bounds(cx, "content-2");
    let r3 = bounds(cx, "content-3");
    assert!(
        title.origin.y + title.size.height <= r1.origin.y + px(0.5),
        "title above rows"
    );
    assert!(
        r1.origin.y < r2.origin.y && r2.origin.y < r3.origin.y,
        "rows stack downward"
    );
    assert!(
        approx(r2.origin.x - r1.origin.x, CHILD_INDENT),
        "one indent per level: {r1:?} {r2:?}"
    );
    assert!(approx(r3.origin.x - r2.origin.x, CHILD_INDENT));
    let main = bounds(cx, "main-pane");
    for r in [r1, r2, r3] {
        assert!(
            r.origin.x > main.origin.x
                && r.origin.x + r.size.width <= main.origin.x + main.size.width + px(0.5)
        );
        assert!(
            r.size.height >= px(20.),
            "a row is at least one line tall: {r:?}"
        );
    }
    let refs = bounds(cx, "refs-header");
    assert!(
        refs.origin.y > r3.origin.y,
        "Linked References sit below the outline"
    );
    // List items shrink to their content unless wrapped; the header's rule and count
    // must span the column inside its 40 px margins (DESIGN.md §2.3). The column is a
    // readable width, centred in the pane.
    let column = f32::from(main.size.width).min(760.);
    let left = (f32::from(main.size.width) - column) / 2.;
    assert!(
        approx(refs.origin.x - main.origin.x, left + 40.) && approx(refs.size.width, column - 80.),
        "refs header spans the column: {refs:?} {main:?}"
    );
}

#[gpui::test]
fn editor_and_autocomplete_placement(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("first");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("[[fir");
    let editor = bounds(cx, "block-editor");
    let row2 = bounds(cx, "content-2");
    assert!(
        approx(editor.origin.y, f32::from(row2.origin.y)),
        "editor renders inside the focused row"
    );
    // "Create" is offered for a novel title, so the popup is open below the text.
    let popup = bounds(cx, "autocomplete");
    assert!(
        popup.origin.y >= editor.origin.y + px(10.),
        "popup below the caret line: {popup:?} {editor:?}"
    );
    assert!(
        popup.origin.x + popup.size.width <= px(W) + px(0.5),
        "popup stays inside the window"
    );
}

#[gpui::test]
fn quick_jump_is_centered(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    let p = bounds(cx, "jump-palette");
    // Page mode keeps the compact quick-jump width; search widens for its preview.
    assert!(approx(p.size.width, 520.));
    let center = f32::from(p.origin.x) + 260.;
    assert!((center - W / 2.).abs() < 2., "centered horizontally: {p:?}");
    assert!(
        p.origin.y > px(60.) && p.origin.y < px(H / 2.),
        "in the upper half: {p:?}"
    );
}

/// SR-4: search widens to 860 px and the preview fills the right half, beside the list
/// rather than over it.
#[gpui::test]
fn search_preview_sits_in_the_right_half(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("zebra crossing");
    cx.simulate_keystrokes("escape cmd-shift-f");
    cx.simulate_input("zebra");
    let palette = bounds(cx, "jump-palette");
    assert!(
        approx(palette.size.width, 860.),
        "the search dialog is about 860 px wide: {palette:?}"
    );
    let results = bounds(cx, "jump-results");
    let preview = bounds(cx, "search-preview");
    assert!(
        f32::from(preview.origin.x) >= f32::from(palette.origin.x) + 430.,
        "the preview starts in the right half: {preview:?} {palette:?}"
    );
    assert!(
        preview.origin.x >= results.origin.x + results.size.width - px(0.5),
        "the list ends before the preview: {results:?} {preview:?}"
    );
    assert!(
        preview.right() <= palette.right() + px(0.5),
        "the preview stays inside the dialog: {preview:?} {palette:?}"
    );
    assert!(
        results.size.width >= px(300.),
        "the list keeps usable width: {results:?}"
    );
}

/// SR-4/SR-5: the 720 px minimum window must contain the widened palette, input, chips,
/// list and preview. A fixed 860 px dialog hung off both sides.
#[gpui::test]
fn search_palette_fits_the_minimum_window(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("zebra crossing");
    cx.simulate_keystrokes("escape");
    cx.simulate_resize(size(px(720.), px(480.)));
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-shift-f");
    cx.simulate_input("zebra");
    cx.run_until_parked();
    let palette = bounds(cx, "jump-palette");
    assert!(
        palette.origin.x >= px(0.) && palette.right() <= px(720.) + px(0.5),
        "the palette stays in the window horizontally: {palette:?}"
    );
    assert!(
        palette.origin.y >= px(0.) && palette.bottom() <= px(480.) + px(0.5),
        "and vertically: {palette:?}"
    );
    let input = bounds(cx, "search-input");
    let scopes = bounds(cx, "search-scopes");
    for b in [input, scopes] {
        assert!(
            b.origin.x >= palette.origin.x && b.right() <= palette.right() + px(0.5),
            "controls stay inside the palette: {b:?} {palette:?}"
        );
    }
    let results = bounds(cx, "jump-results");
    let preview = bounds(cx, "search-preview");
    assert!(
        preview.right() <= palette.right() + px(0.5),
        "the preview fits: {preview:?} {palette:?}"
    );
    assert!(
        results.right() <= preview.origin.x + px(0.5),
        "the list ends before the narrow preview: {results:?} {preview:?}"
    );
    assert!(
        preview.size.width >= px(240.),
        "the preview keeps a readable width: {preview:?}"
    );
}

/// SR-3: the scope chips sit under the field inside the palette, and the "Any time" menu
/// floats over the results without moving them (deferred + anchored, like the page menu).
#[gpui::test]
fn search_scope_row_and_age_menu(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-shift-f");
    let palette = bounds(cx, "jump-palette");
    let scopes = bounds(cx, "search-scopes");
    assert!(
        scopes.origin.y > palette.origin.y + px(60.),
        "under the field: {scopes:?} {palette:?}"
    );
    assert!(
        scopes.origin.x >= palette.origin.x && scopes.right() <= palette.right() + px(0.5),
        "inside the palette: {scopes:?} {palette:?}"
    );
    let age = bounds(cx, "search-age");
    assert!(
        age.right() <= palette.right() + px(0.5),
        "the age control stays inside: {age:?}"
    );
    cx.simulate_click(age.center(), gpui::Modifiers::default());
    let popup = bounds(cx, "search-age-popup");
    assert!(
        popup.origin.y >= age.origin.y + age.size.height - px(1.),
        "the menu opens below its control: {popup:?} {age:?}"
    );
    assert!(
        popup.origin.x >= palette.origin.x
            && popup.right() <= px(W)
            && popup.origin.y + popup.size.height <= px(H),
        "the menu stays in the window: {popup:?}"
    );
    let after = bounds(cx, "search-scopes");
    assert_eq!(scopes, after, "opening the menu does not move the chips");
}

/// QJ-1: with nothing typed the palette offers today and yesterday, in that order, and no
/// "Create" row — there is no query to create from.
#[gpui::test]
fn quick_jump_empty_state_lists_today_and_yesterday(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    let palette = bounds(cx, "jump-palette");
    let today = bounds(cx, "qj-row-0");
    let yesterday = bounds(cx, "qj-row-1");
    assert!(
        cx.debug_bounds("qj-icon-0-day").is_some(),
        "today is a day row"
    );
    assert!(
        cx.debug_bounds("qj-icon-1-day").is_some(),
        "yesterday is a day row"
    );
    assert!(yesterday.origin.y > today.origin.y, "today comes first");
    assert!(
        today.origin.y >= palette.origin.y
            && yesterday.origin.y + yesterday.size.height
                <= palette.origin.y + palette.size.height + px(0.5),
        "both rows sit inside the palette: {today:?} {yesterday:?} {palette:?}"
    );
    assert!(
        cx.debug_bounds("jump-create").is_none(),
        "no create row before anything is typed"
    );
}

/// QJ-3: the key hints are a footer under the results, with a hairline above them.
#[gpui::test]
fn quick_jump_footer_sits_under_the_results(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    let palette = bounds(cx, "jump-palette");
    let footer = bounds(cx, "jump-footer");
    let last_row = bounds(cx, "qj-row-1");
    let results = bounds(cx, "jump-results");
    assert!(
        footer.origin.y >= results.origin.y + results.size.height - px(0.5),
        "the footer follows the list: {footer:?} {results:?}"
    );
    assert!(
        footer.origin.y >= last_row.origin.y + last_row.size.height - px(0.5),
        "and the last row: {footer:?} {last_row:?}"
    );
    assert!(
        footer.origin.y + footer.size.height <= palette.origin.y + palette.size.height + px(0.5),
        "the footer is inside the palette: {footer:?} {palette:?}"
    );
    assert!(approx(footer.size.width, 520. - 18.), "{footer:?}");
}

/// QJ-2: "Create page" is the last row, below a hairline of its own.
#[gpui::test]
fn quick_jump_create_row_is_last(cx: &mut TestAppContext) {
    let cx = setup_titles(cx, &["Brand new page"]);
    cx.simulate_keystrokes("escape cmd-k");
    cx.simulate_input("Brand");
    let palette = bounds(cx, "jump-palette");
    let result = bounds(cx, "qj-row-0");
    let create = bounds(cx, "qj-row-1");
    assert!(cx.debug_bounds("qj-icon-0-page").is_some(), "a page row");
    assert!(
        cx.debug_bounds("qj-icon-1-create").is_some(),
        "the create row is last"
    );
    let hairline = bounds(cx, "jump-create");
    assert!(
        hairline.origin.y >= result.origin.y + result.size.height - px(0.5),
        "the create row sits under its hairline, under the results: {hairline:?} {result:?}"
    );
    assert!(create.origin.y > result.origin.y, "{create:?} {result:?}");
    assert!(
        create.origin.y + create.size.height <= palette.origin.y + palette.size.height + px(0.5),
        "inside the palette: {create:?} {palette:?}"
    );
}

/// Opening the palette is an overlay: the page underneath does not move.
#[gpui::test]
fn quick_jump_does_not_move_the_page(cx: &mut TestAppContext) {
    let cx = setup(cx);
    let before = bounds(cx, "main-pane");
    cx.simulate_keystrokes("escape cmd-k");
    cx.simulate_input("Brand new page");
    let after = bounds(cx, "main-pane");
    assert_eq!(before, after, "the palette floats over the page");
}

/// QJ-2: every row is icon | label | type label, so the icons line up in one gutter and
/// the type labels share a right edge.
#[gpui::test]
fn quick_jump_rows_line_up(cx: &mut TestAppContext) {
    let cx = setup_titles(cx, &["Atlas Launch", "Ana Lovelace"]);
    cx.simulate_keystrokes("escape cmd-k");
    cx.simulate_input("a");
    let first = bounds(cx, "qj-row-0");
    let second = bounds(cx, "qj-row-1");
    assert!(
        approx(first.size.height, f32::from(second.size.height)),
        "{first:?}"
    );
    let icon0 = bounds(cx, "qj-icon-0-page");
    let icon1 = bounds(cx, "qj-icon-1-page");
    assert!(
        approx(icon0.origin.x, f32::from(icon1.origin.x)),
        "one gutter"
    );
    assert!(
        icon0.origin.x > first.origin.x
            && icon0.origin.x + icon0.size.width < first.origin.x + first.size.width,
        "the icon leads the row: {icon0:?} {first:?}"
    );
    let label0 = bounds(cx, "qj-label-0");
    let label1 = bounds(cx, "qj-label-1");
    assert!(
        approx(label0.origin.x, f32::from(label1.origin.x)),
        "labels line up"
    );
    assert!(
        label0.origin.x >= icon0.origin.x + icon0.size.width,
        "the label follows the icon: {label0:?} {icon0:?}"
    );
    let detail0 = bounds(cx, "qj-detail-0");
    let detail1 = bounds(cx, "qj-detail-1");
    assert!(
        detail0.origin.x + detail0.size.width <= first.origin.x + first.size.width,
        "the type label stays inside the row: {detail0:?} {first:?}"
    );
    assert!(
        approx(
            detail0.origin.x + detail0.size.width,
            f32::from(detail1.origin.x + detail1.size.width)
        ),
        "type labels are right-aligned: {detail0:?} {detail1:?}"
    );
}

/// QJ-1/QJ-3: ten rows (today, yesterday and eight recent pages) keep the palette under
/// its height cap, so the footer stays visible while the list scrolls.
#[gpui::test]
fn quick_jump_caps_its_height(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let mut ids = Vec::new();
    for t in [
        "Atlas Launch",
        "Ana Lovelace",
        "Library",
        "Operations",
        "Reading log",
        "Trips",
        "Recipes",
        "Garden",
        "Budget",
        "Inbox",
    ] {
        ids.push(store.create_page(t).unwrap());
    }
    // Most recently opened first, as the workspace writes it (rule 9).
    let keyed = ids
        .iter()
        .rev()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    store.local_set("recent_pages", &keyed).unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape cmd-k");
    let palette = bounds(cx, "jump-palette");
    assert!(
        palette.size.height <= px(440.) + px(0.5),
        "the palette caps its height: {palette:?}"
    );
    let footer = bounds(cx, "jump-footer");
    assert!(
        footer.origin.y + footer.size.height <= palette.origin.y + palette.size.height + px(0.5),
        "the footer stays inside: {footer:?} {palette:?}"
    );
    assert!(
        cx.debug_bounds("qj-row-9").is_some(),
        "all ten rows are offered, and the list scrolls"
    );
}

/// ALL-5: Compact density does not break the palette — the rows still fit, in order.
#[gpui::test]
fn quick_jump_works_at_compact_density(cx: &mut TestAppContext) {
    let cx = setup(cx);
    let dir = tempfile::tempdir().unwrap();
    cx.update(|_window, cx| {
        utopia_ui::appearance::set_density(cx, utopia_ui::appearance::Density::Compact, dir.path())
            .unwrap();
    });
    cx.simulate_keystrokes("escape cmd-k");
    let palette = bounds(cx, "jump-palette");
    let today = bounds(cx, "qj-row-0");
    let yesterday = bounds(cx, "qj-row-1");
    assert!(
        yesterday.origin.y > today.origin.y,
        "{today:?} {yesterday:?}"
    );
    let footer = bounds(cx, "jump-footer");
    assert!(
        footer.origin.y + footer.size.height <= palette.origin.y + palette.size.height + px(0.5),
        "Compact keeps the footer inside: {footer:?} {palette:?}"
    );
}

/// QJ-3/QJ-5: ↑/↓ move through the whole command catalog, so the list has to follow the
/// selection — otherwise Enter runs a command nobody can see.
#[gpui::test]
fn quick_jump_scrolls_the_selection_into_view(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-k");
    cx.simulate_input(">");
    cx.run_until_parked();
    let last = utopia_ui::actions::CATALOG.len() - 1;
    let visible = |cx: &mut VisualTestContext, i: usize| {
        let row = bounds_of(cx, format!("qj-row-{i}"));
        let results = bounds(cx, "jump-results");
        row.origin.y >= results.origin.y - px(1.)
            && row.origin.y + row.size.height <= results.origin.y + results.size.height + px(1.)
    };
    // Up from the first row wraps to the last: the list scrolls down to show it.
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    assert!(visible(cx, last), "the selected last command is in view");
    // Down from there wraps back to the top, which scrolls the list up again.
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert!(visible(cx, 0), "the first command is in view again");
    // And walking down past the viewport keeps the selection in view.
    cx.simulate_keystrokes("down down down down down down down down down down down down");
    cx.run_until_parked();
    assert!(visible(cx, 12), "the selection follows the list");
}

#[gpui::test]
fn minimum_window_size_still_lays_out(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_resize(size(px(720.), px(480.)));
    cx.simulate_input("still editable at the minimum size");
    let main = bounds(cx, "main-pane");
    let row = bounds(cx, "content-1");
    assert!(approx(main.size.width, 720. - SIDEBAR_WIDTH), "{main:?}");
    assert!(
        row.origin.x + row.size.width <= px(720.) + px(0.5),
        "rows wrap inside the window"
    );
}

#[gpui::test]
fn daily_notes_feed_stacks_days(cx: &mut TestAppContext) {
    let cx = setup(cx);
    // Launch shows the feed: today's title, then its first block.
    cx.simulate_input("today's note");
    cx.simulate_keystrokes("escape");
    let title = bounds(cx, "today-title");
    let row = bounds(cx, "content-1");
    // An older day with nothing on it is an empty row (§3.6), not a section.
    let older = bounds(cx, "empty-day-2026-11-01");
    assert!(older.origin.y > row.origin.y, "older days follow today");
    assert!(title.origin.y + title.size.height <= row.origin.y + px(0.5));
    // §3.5: a mono weekday label sits above today's date, and is not a second title.
    let today_caption = bounds(cx, "day-caption-2026-11-02");
    let today_date = bounds(cx, "day-date-2026-11-02");
    assert!(
        today_caption.origin.y + today_caption.size.height <= today_date.origin.y + px(0.5),
        "the weekday is above its date: {today_caption:?} {today_date:?}"
    );
    assert!(
        today_caption.size.height < today_date.size.height,
        "a label, not a second title: {today_caption:?} {today_date:?}"
    );
    assert!(
        cx.debug_bounds("empty-weekday-2026-11-01").is_some(),
        "the empty row names its weekday"
    );
    assert!(
        cx.debug_bounds("refs-header").is_none(),
        "no references inside the feed"
    );
}

/// DESIGN.md §3.6: consecutive days with nothing on them read as one block — a single
/// divider above, 44 px rows 4 px apart, a fixed 92 px weekday column, the date title, and
/// a ghost Write. They are separate list items, so the list virtualizes each. Clicking
/// Write expands the day with one empty block focused, and nothing is written until the
/// first keystroke (FR-1.2).
#[gpui::test]
fn empty_day_rows_stack_and_write_expands_one(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("today's note");
    cx.simulate_keystrokes("escape");
    let first = bounds(cx, "empty-day-2026-11-01");
    let second = bounds(cx, "empty-day-2026-10-31");
    assert!(
        approx(first.size.height, 44.),
        "an empty day is one 44 px row: {first:?}"
    );
    assert!(
        approx(second.origin.y - (first.origin.y + first.size.height), 4.),
        "4 px between empty rows: {first:?} {second:?}"
    );
    // The weekday column is fixed, so the date titles line up.
    let weekday = bounds(cx, "empty-weekday-2026-11-01");
    let title = bounds(cx, "empty-title-2026-11-01");
    assert!(approx(weekday.size.width, 92.), "{weekday:?}");
    assert!(
        approx(title.origin.x - weekday.origin.x, 92.),
        "the date starts after the fixed weekday column: {weekday:?} {title:?}"
    );
    assert!(
        approx(title.size.height, 26.),
        "the date is the 17/26 empty-day title: {title:?}"
    );
    let write = bounds(cx, "empty-write-2026-10-31");
    assert!(
        write.size.height >= px(28.) && write.size.height <= px(32.) + px(0.5),
        "a ghost Write button: {write:?}"
    );

    // Write opens that day with one empty block focused; the click alone writes nothing.
    cx.simulate_click(write.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let opened = bounds(cx, "day-date-2026-10-31");
    let below = bounds(cx, "empty-day-2026-10-30");
    assert!(
        opened.origin.y < below.origin.y,
        "the opened day has its heading above the rows that are still empty: \
         {opened:?} {below:?}"
    );
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    assert_eq!(
        ws.update(cx, |w, cx| w.editor().read(cx).field_text().to_string()),
        "",
        "the caret is in a fresh, empty block"
    );
    let day = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
    assert!(
        app.read_with(cx, |app, _| app.store.is_draft(&PageId::for_date(day))),
        "a click starts a draft, not a write"
    );
    cx.simulate_input("picked up later");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let texts: Vec<String> = app.update(cx, |app, _| {
        let st = app.store.page_state(&PageId::for_date(day)).unwrap();
        st.live_roots()
            .into_iter()
            .filter_map(|b| st.block(&b).map(|b| b.text.clone()))
            .collect()
    });
    assert_eq!(
        texts,
        ["picked up later"],
        "the first keystroke writes the day"
    );
    // The readable column still holds the rows.
    let (left, right) = column(cx);
    let (x0, x1) = (
        f32::from(first.origin.x),
        f32::from(first.origin.x + first.size.width),
    );
    assert!(x0 >= left && x1 <= right + 0.5, "{first:?}");
}

/// DESIGN.md §3.6 without losing virtualization: each empty day is its own list item, so a
/// blank stretch costs the list per-row work instead of one item covering all of it.
#[gpui::test]
fn empty_days_are_individual_list_items(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    // The first window is JOURNAL_BATCH (10) calendar days: today is open (its draft has
    // the block the caret goes in) and the other nine are empty rows.
    let rows = ws.update(cx, |w, cx| w.main_empty_day_rows(cx));
    assert!(
        rows >= 8,
        "one list item per empty day, not one item for the whole run: {rows}"
    );
    let first = bounds(cx, "empty-day-2026-11-01");
    let second = bounds(cx, "empty-day-2026-10-31");
    assert!(approx(first.size.height, 44.));
    assert!(approx(
        second.origin.y - (first.origin.y + first.size.height),
        4.
    ));
}

/// DESIGN.md §3.1: rows follow the 28 px line grid. The caret slot (20 px) sits in the
/// gutter, the bullet slot is 24 px, a 6 px bullet is centred in the first line, block
/// text lines up with the page title, and children step 45 px with a thread line under
/// each ancestor's bullet.
#[gpui::test]
fn block_rows_follow_the_line_grid_and_the_thread(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("parent");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab enter");
    cx.simulate_input("grandchild");
    cx.simulate_keystrokes("tab escape");
    cx.run_until_parked();

    let row1 = bounds(cx, "row-1");
    let content1 = bounds(cx, "content-1");
    let content2 = bounds(cx, "content-2");
    let content3 = bounds(cx, "content-3");
    assert!(
        approx(row1.size.height, 28.),
        "a one-line row is one 28 px line: {row1:?}"
    );
    assert!(approx(content1.size.height, 28.), "{content1:?}");
    // The caret slot is 20 px and the bullet slot 24; the bullet sits 20 + 12 px in.
    let caret = bounds(cx, "caret-slot-1");
    let dot = bounds(cx, "dot-1");
    assert!(approx(caret.size.width, 20.), "{caret:?}");
    assert!(
        approx(dot.size.width, 6.) && approx(dot.size.height, 6.),
        "a 6 px bullet: {dot:?}"
    );
    assert!(
        approx(dot.origin.x + dot.size.width / 2. - row1.origin.x, 32.),
        "bullet centre after the 20 px caret slot and half the 24 px bullet slot: \
         {dot:?} {row1:?}"
    );
    // The gutter hangs in the column's padding, so the text starts where the title does.
    let title = bounds(cx, "page-title");
    assert!(
        approx(content1.origin.x, f32::from(title.origin.x)),
        "block text aligns with the title: {content1:?} {title:?}"
    );
    // Children step 45 px (31 margin + 13 padding + the 1 px line).
    assert!(approx(content2.origin.x - content1.origin.x, CHILD_INDENT));
    assert!(approx(content3.origin.x - content2.origin.x, CHILD_INDENT));
    // The grandchild draws a thread line for each ancestor, under its bullet centre.
    for (k, parent) in [(0, "dot-1"), (1, "dot-2")] {
        let thread = bounds_of(cx, format!("thread-3-{k}"));
        let centre = bounds(cx, parent).origin.x + bounds(cx, parent).size.width / 2.;
        assert!(approx(thread.size.width, 1.), "{thread:?}");
        assert!(
            (f32::from(thread.origin.x + thread.size.width) - f32::from(centre)).abs() <= 1.,
            "thread {k} lines up under the parent bullet: {thread:?} {centre:?}"
        );
    }
}

/// DESIGN.md §3.5: a mono weekday label above the day's title, a Today pill on today, and
/// 16 px between the title and the first block.
#[gpui::test]
fn day_header_has_a_weekday_label_pill_and_sixteen_px_gap(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("today's note");
    cx.simulate_keystrokes("escape");
    let caption = bounds(cx, "day-caption-2026-11-02");
    let date = bounds(cx, "day-date-2026-11-02");
    let row = bounds(cx, "content-1");
    assert!(
        approx(caption.size.height, 20.),
        "the weekday line is the pill's 20 px: {caption:?}"
    );
    assert!(
        caption.origin.y + caption.size.height <= date.origin.y + px(0.5),
        "the label is above the title: {caption:?} {date:?}"
    );
    let pill = bounds(cx, "today-pill");
    let weekday = bounds(cx, "day-weekday-2026-11-02");
    assert!(
        approx(pill.size.height, 20.),
        "the Today pill is 20 px: {pill:?}"
    );
    assert!(
        pill.origin.x >= weekday.origin.x + weekday.size.width - px(0.5),
        "the pill follows the weekday label: {pill:?} {weekday:?}"
    );
    assert!(
        approx(row.origin.y - (date.origin.y + date.size.height), 16.),
        "16 px below the title before the first block: {date:?} {row:?}"
    );
}

/// DESIGN.md §2.3/§4.1: day sections are 40 px apart, with the divider between them.
#[gpui::test]
fn day_sections_are_forty_px_apart(cx: &mut TestAppContext) {
    let (_ws, cx) = setup_feed(cx, &[(0, 1), (1, 1)], H);
    let today_row = bounds(cx, "content-2");
    let yesterday = bounds(cx, "day-title");
    assert!(
        approx(
            yesterday.origin.y - (today_row.origin.y + today_row.size.height),
            40.
        ),
        "40 px between the last block of a day and the next day's divider: \
         {today_row:?} {yesterday:?}"
    );
}

/// DESIGN.md §4.1: "Today" resets the feed's scroll position to today. Both the top bar's
/// Today button and the Today pill in the pinned day header do it.
#[gpui::test]
fn today_resets_the_feed_scroll(cx: &mut TestAppContext) {
    let (ws, cx) = setup_feed(cx, &[(0, 40)], H);
    scroll_main(cx, -600.);
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()),
        "scrolled deep into today, today is the pinned day"
    );
    let pill = bounds(cx, "sticky-today-pill");
    cx.simulate_click(pill.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        None,
        "the feed is back at the top, so nothing is pinned"
    );
    let title = bounds(cx, "today-title");
    assert!(
        approx(title.origin.y, content_top(cx)),
        "today's section starts at the top of the feed: {title:?}"
    );
}

/// A feed whose days hold `blocks` blocks each (`(days ago, blocks)`), in a window `h`
/// tall; every other day is left unwritten so the feed groups it into an empty-day row.
fn setup_feed<'a>(
    cx: &'a mut TestAppContext,
    blocks: &[(i64, usize)],
    h: f32,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let today = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today));
    for &(back, n) in blocks {
        let date = today - chrono::Duration::days(back);
        let page = store.draft_daily(date).unwrap();
        let mut after = store.page_state(&page).unwrap().last_child(None, false);
        let mut cmds = Vec::new();
        for i in 0..n {
            let id = BlockId::new();
            cmds.push(Command::InsertBlock {
                id: id.clone(),
                parent: None,
                after: after.clone(),
                text: format!("note {i}"),
                kind: BlockKind::Bullet,
            });
            after = Some(id);
        }
        store
            .execute(
                &page,
                cmds,
                EditOpts {
                    no_undo: true,
                    ..Default::default()
                },
            )
            .unwrap();
    }
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(h)));
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    (ws, cx)
}

/// The top of the page content: the bottom of the top bar (DESIGN.md §3.3).
fn content_top(cx: &mut VisualTestContext) -> f32 {
    let bar = bounds(cx, "top-bar");
    f32::from(bar.origin.y + bar.size.height)
}

/// Scroll the main pane by `dy` pixels (negative scrolls down).
fn scroll_main(cx: &mut VisualTestContext, dy: f32) {
    let main = bounds(cx, "main-pane");
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: main.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(dy))),
        ..Default::default()
    });
    cx.run_until_parked();
}

/// DN-3: while a long day scrolls, its date stays at the top of the column, with a
/// hairline under it; at the top of the feed nothing is pinned.
#[gpui::test]
fn long_day_keeps_its_date_at_the_top(cx: &mut TestAppContext) {
    let (ws, cx) = setup_feed(cx, &[(0, 40)], H);
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        None,
        "the day is at the top of the feed: nothing is pinned"
    );
    let main = bounds(cx, "main-pane");
    scroll_main(cx, -600.);
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()),
        "the day being scrolled keeps its heading"
    );
    let pin = bounds(cx, "sticky-day");
    assert!(
        approx(pin.origin.y, content_top(cx)),
        "pinned to the top of the column below the top bar: {pin:?} {main:?}"
    );
    assert!(pin.size.height >= px(30.), "a readable heading: {pin:?}");
    let (left, right) = column(cx);
    let (x0, x1) = (
        f32::from(pin.origin.x),
        f32::from(pin.origin.x + pin.size.width),
    );
    assert!(
        x0 >= left && x1 <= right + 0.5,
        "inside the column: {pin:?}"
    );
}

/// DN-3: a row that the list reveals at its top edge (arrowing past the viewport) must
/// sit below the pinned date, so the caret is never hidden under it while typing.
#[gpui::test]
fn revealing_a_row_keeps_the_caret_below_the_pinned_date(cx: &mut TestAppContext) {
    let (ws, cx) = setup_feed(cx, &[(0, 80)], H);
    // `setup_feed` leaves editing; open the feed again with the caret in the first block.
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.run_until_parked();
    for _ in 0..30 {
        cx.simulate_keystrokes("down");
    }
    cx.run_until_parked();
    // Scroll the day far past the block being edited: the next Down moves to a block above
    // the viewport, which the list reveals at its top edge, where the pinned date is drawn.
    // Each wheel step only reaches what the list has measured so far, so repeat.
    for _ in 0..10 {
        scroll_main(cx, -600.);
    }
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    // The 31st block (item 32, after the day's heading) is the one now being edited.
    assert_eq!(
        ws.update(cx, |w, cx| w.editor().read(cx).field_text().to_string()),
        "note 30"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()),
        "the day's date is still pinned"
    );
    let pin = bounds(cx, "sticky-day");
    let row = bounds(cx, "content-32");
    assert!(
        row.origin.y >= pin.origin.y + pin.size.height - px(0.5),
        "the row being edited stays below the pinned date: row {row:?}, pin {pin:?}"
    );
}

/// DN-3: a row that the list considers visible can still be drawn *under* the pinned
/// date (its heading is `STICKY_HEIGHT` tall). Navigating up to such a row must move it below the
/// heading, or the caret and the text being edited disappear behind it.
#[gpui::test]
fn a_row_under_the_pinned_date_is_moved_below_it(cx: &mut TestAppContext) {
    let (ws, cx) = setup_feed(cx, &[(0, 80)], H);
    // Open the feed with the caret in the first block, then move it down a page's worth.
    ws.update_in(cx, |w, window, cx| w.open_journal(window, cx));
    cx.run_until_parked();
    for _ in 0..23 {
        cx.simulate_keystrokes("down");
    }
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.editor().read(cx).field_text().to_string()),
        "note 22",
        "the caret is on note 22 (item 23)"
    );
    // Put note 20 (item 21) at the top of the list, so note 21 (item 22) — the row an Up
    // lands on — is the second visible row, inside the height the pinned date covers.
    // A wheel event only reaches what the list has measured so far, so repeat.
    for _ in 0..8 {
        let delta = f32::from(bounds(cx, "content-21").origin.y) - content_top(cx);
        if delta.abs() <= 1. {
            break;
        }
        scroll_main(cx, -delta);
    }
    assert!(
        approx(bounds(cx, "content-21").origin.y, content_top(cx)),
        "note 20 sits at the top of the list"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()),
        "the day's date is pinned over the top of the column"
    );
    let pin = bounds(cx, "sticky-day");
    let hidden = bounds(cx, "content-22");
    assert!(
        hidden.origin.y < pin.origin.y + pin.size.height - px(0.5),
        "the row above the caret really is under the heading: {hidden:?} {pin:?}"
    );

    // Up from note 22 lands on note 21, which the list thinks is visible.
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.editor().read(cx).field_text().to_string()),
        "note 21"
    );
    let row = bounds(cx, "content-22");
    assert!(
        row.origin.y >= pin.origin.y + pin.size.height - px(0.5),
        "the row being edited stays below the pinned date: row {row:?}, pin {pin:?}"
    );
}

/// DN-3 with DESIGN.md §3.6: an empty day group carries its own dates in its rows, so the
/// previous day's heading is not pinned over it.
#[gpui::test]
fn an_empty_day_group_does_not_keep_the_previous_day_pinned(cx: &mut TestAppContext) {
    // A short window, so the feed can scroll down to the empty days.
    let (ws, cx) = setup_feed(cx, &[(0, 1), (1, 1), (4, 12)], 600.);
    assert_eq!(ws.update(cx, |w, cx| w.main_sticky_day(cx)), None);
    let main = bounds(cx, "main-pane");
    let empty = bounds(cx, "empty-day-2026-10-31");
    // Slide the first empty row to the top of the column, a few pixels past its edge.
    let delta = f32::from(empty.origin.y - main.origin.y) + 4.;
    assert!(delta > 0.);
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        None,
        "still at the top of the feed"
    );
    scroll_main(cx, -delta);
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        None,
        "the empty rows carry their own dates: nothing is pinned above them"
    );
}

/// Several hundred characters with no newline: a referencing block that must wrap.
const LONG: &str = "A deliberately long note that keeps going without any newline so that \
    it has to wrap at the readable column: the beta needs twenty testers invited by hand, \
    the import path has to stay unchanged so that old files still open, and the migration \
    notes must reach the reviewer before the end of the month so that nothing slips.";

/// Like `setup`, but the store first gets a long block on "Source" that links to the
/// "Target" page or, with `daily`, to today.
fn setup_refs(
    cx: &mut TestAppContext,
    daily: bool,
) -> (&mut VisualTestContext, Entity<Workspace>, PageId) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    let src = store.create_page("Source").unwrap();
    let b = store.ensure_first_block(&src).unwrap();
    let link = if daily {
        "[[November 2nd, 2026]]"
    } else {
        "[[Target]]"
    };
    store
        .set_block_text(&src, &b, &format!("{LONG} {link}"), None, None)
        .unwrap();
    store.commit_block(&src, &b).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    (cx, ws, target)
}

/// The list index of the first Linked References hit drawn.
fn first_hit_ix(cx: &mut VisualTestContext) -> usize {
    cx.run_until_parked();
    (0..40)
        .find(|ix| {
            let sel: &'static str = Box::leak(format!("hit-{ix}").into_boxed_str());
            cx.debug_bounds(sel).is_some()
        })
        .expect("a linked reference was rendered")
}

/// The first Linked References hit drawn (its list index depends on what sits above it).
/// Each test opens a fresh window, so no stale bounds from an earlier frame can match.
fn first_hit(cx: &mut VisualTestContext) -> Bounds<Pixels> {
    cx.run_until_parked();
    (0..40)
        .find_map(|ix| {
            // `debug_bounds` takes a `&'static str`; leaking a few test strings is fine.
            let sel: &'static str = Box::leak(format!("hit-{ix}").into_boxed_str());
            cx.debug_bounds(sel)
        })
        .expect("a linked reference was rendered")
}

/// The readable column (760 px, centred) inside the main pane.
fn column(cx: &mut VisualTestContext) -> (f32, f32) {
    let main = bounds(cx, "main-pane");
    let width = f32::from(main.size.width).min(760.);
    let left = f32::from(main.origin.x) + (f32::from(main.size.width) - width) / 2.;
    (left, left + width)
}

fn assert_wraps_within(hit: Bounds<Pixels>, left: f32, right: f32) {
    let (x0, x1) = (
        f32::from(hit.origin.x),
        f32::from(hit.origin.x + hit.size.width),
    );
    assert!(
        x0 >= left && x1 <= right + 0.5,
        "the reference stays inside [{left}, {right}]: {hit:?}"
    );
    assert!(
        hit.size.height >= px(40.),
        "long text wraps onto several lines instead of running wide: {hit:?}"
    );
}

/// FR-3.1: a long referencing block wraps at the page's readable width, like the outline
/// above it, instead of running past the column (found by screenshot).
#[gpui::test]
fn linked_reference_wraps_at_readable_width(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    let hit = first_hit(cx);
    let (left, right) = column(cx);
    assert_wraps_within(hit, left, right);
}

/// DESIGN.md §3.7: the Linked references are one bordered well: a header with the count
/// (and, on a page, the Filter button), the groups, and a rounded bottom edge, all inside
/// the readable column.
#[gpui::test]
fn references_panel_is_a_bordered_well(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    cx.run_until_parked();
    let header = bounds(cx, "refs-header");
    let end = bounds(cx, "refs-end");
    let (left, right) = column(cx);
    assert_eq!(
        f32::from(header.origin.x),
        left + 40.,
        "the panel is inset 40 px in the column: {header:?}"
    );
    assert_eq!(
        f32::from(header.size.width),
        right - left - 80.,
        "and spans the column minus its insets: {header:?}"
    );
    assert_eq!(
        end.origin.x, header.origin.x,
        "the rounded bottom edge lines up with the header"
    );
    assert_eq!(end.size.width, header.size.width);
    assert!(
        end.origin.y >= header.origin.y + header.size.height,
        "the bottom edge comes after the header: {header:?} {end:?}"
    );
    let title = bounds(cx, "refs-title");
    assert!(
        title.origin.x >= header.origin.x && title.right() <= header.right(),
        "the heading sits inside the header: {title:?} {header:?}"
    );
    let filter = bounds(cx, "refs-filter");
    assert!(
        filter.right() <= header.right() + px(0.5),
        "the Filter button is at the header's right edge: {filter:?} {header:?}"
    );
    let group = (0..40)
        .find_map(|ix| {
            let sel: &'static str = Box::leak(format!("group-title-{ix}").into_boxed_str());
            cx.debug_bounds(sel)
        })
        .expect("a source page header was rendered");
    assert!(
        group.origin.x >= header.origin.x + px(16.) && group.right() <= header.right(),
        "the source page sits inside the well: {group:?} {header:?}"
    );
    assert!(
        group.origin.y >= header.bottom() && group.bottom() <= end.origin.y + px(1.),
        "between the header and the bottom edge: {group:?} {header:?} {end:?}"
    );
    let count = (0..10)
        .find(|n| {
            let sel: &'static str = Box::leak(format!("refs-count-{n}").into_boxed_str());
            cx.debug_bounds(sel).is_some()
        })
        .expect("the header carries a count badge");
    assert_eq!(count, 1, "one referencing block");
}

/// DESIGN.md §3.7: in the feed each day's header is a button that folds that day's
/// references, and only that day's: the panel shrinks to the header and its bottom edge.
#[gpui::test]
fn a_feed_day_collapses_its_references(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_refs(cx, true);
    let before = bounds(cx, "refs-end");
    let header = bounds(cx, "refs-header");
    // The feed has no Filter button; its header is the collapse control.
    assert!(
        cx.debug_bounds("refs-filter").is_none(),
        "Filter is a page-view control"
    );
    assert!(
        header.size.height >= px(40.),
        "a full hit target: {header:?}"
    );

    let toggle = bounds(cx, "refs-state-2026-11-02-open");
    cx.simulate_click(toggle.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("refs-state-2026-11-02-closed").is_some(),
        "the day's references folded"
    );
    let after = bounds(cx, "refs-end");
    assert!(
        after.origin.y < before.origin.y,
        "the panel is now just its header: {before:?} {after:?}"
    );
    assert!(
        after.origin.y >= header.origin.y + header.size.height - px(1.),
        "and ends right under it: {header:?} {after:?}"
    );
}

/// FR-1.2: each day in the Daily Notes feed shows its Linked References, wrapped the same way.
#[gpui::test]
fn feed_linked_reference_wraps_at_readable_width(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_refs(cx, true);
    let hit = first_hit(cx);
    let (left, right) = column(cx);
    assert_wraps_within(hit, left, right);
}

/// Seed a page whose blocks nest `depth` levels deep (0..=depth), returning its id.
fn seed_nested_page(store: &mut Store, depth: usize) -> PageId {
    let page = store.create_page("Deep").unwrap();
    let mut parent = store.ensure_first_block(&page).unwrap();
    store
        .set_block_text(&page, &parent, "level 0", None, None)
        .unwrap();
    store.commit_block(&page, &parent).unwrap();
    for level in 1..=depth {
        let st = store.page_state(&page).unwrap().clone();
        let plan = outline::append_block(&st, Some(&parent));
        let block = store
            .execute_plan(&page, plan, None)
            .unwrap()
            .unwrap()
            .block;
        store
            .set_block_text(&page, &block, &format!("level {level}"), None, None)
            .unwrap();
        store.commit_block(&page, &block).unwrap();
        parent = block;
    }
    store.sync_index().unwrap();
    page
}

/// DESIGN.md §3.1 indents children 45 px, which a 380 px right sidebar cannot afford: the
/// pane's rows use its own narrower step, so a deep block still starts inside the pane.
#[gpui::test]
fn deep_blocks_stay_inside_a_narrow_sidebar_pane(cx: &mut TestAppContext) {
    let page = std::cell::RefCell::new(None);
    let cx = setup_seeded(cx, |store| {
        *page.borrow_mut() = Some(seed_nested_page(store, 8))
    });
    let page = page.borrow_mut().take().expect("seeded page");
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, None, true, window, cx)
    });
    cx.run_until_parked();
    let side = bounds(cx, "right-sidebar");
    // Items: 0 header, 1..=9 the nine blocks; content-9 is depth 8.
    let deep = bounds(cx, "content-9");
    assert!(
        deep.origin.x >= side.origin.x,
        "a deeply nested row starts inside the pane: {deep:?} {side:?}"
    );
    assert!(
        deep.origin.x + deep.size.width <= side.origin.x + side.size.width + px(0.5),
        "a deeply nested row stays inside the pane: {deep:?} {side:?}"
    );
    assert!(
        deep.size.width >= px(80.),
        "the deepest row still has readable width: {deep:?}"
    );
}

/// §9.2: a page shift-opened in the right sidebar wraps its references at the sidebar's edge.
#[gpui::test]
fn sidebar_linked_reference_wraps(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, true, window, cx)
    });
    let hit = first_hit(cx);
    let side = bounds(cx, "right-sidebar");
    let left = f32::from(side.origin.x);
    assert_wraps_within(hit, left, left + f32::from(side.size.width));
}

/// RS-1: the right sidebar's References tab shows the referencing block itself — the same
/// row Linked References draws, wrapped at the panel's edge — not just a page name and a
/// count.
#[gpui::test]
fn inspector_shows_the_referencing_block(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    cx.simulate_keystrokes("escape cmd-shift-\\");
    cx.run_until_parked();
    let side = bounds(cx, "right-sidebar");
    let group = bounds(cx, "ref-group-0");
    let row = bounds(cx, "ref-block-0");
    assert!(
        group.origin.y > side.origin.y && group.origin.y < row.origin.y,
        "the source page heads its blocks: {group:?} {row:?}"
    );
    assert!(
        row.size.height >= px(40.),
        "the long referencing block wraps onto several lines: {row:?}"
    );
    let (x0, x1) = (
        f32::from(row.origin.x),
        f32::from(row.origin.x + row.size.width),
    );
    assert!(
        x0 >= f32::from(side.origin.x) && x1 <= f32::from(side.right()) + 0.5,
        "the row stays inside the panel: {row:?} {side:?}"
    );
}

/// FR-3.9: a long unlinked mention wraps too, keeping its "Link" button in the column.
#[gpui::test]
fn unlinked_mention_wraps_and_keeps_link_in_view(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    let app = ws.update(cx, |w, _| w.app().clone());
    app.update(cx, |app, _| {
        let p = app.store.create_page("Elsewhere").unwrap();
        let b = app.store.ensure_first_block(&p).unwrap();
        app.store
            .set_block_text(&p, &b, &format!("{LONG} target"), None, None)
            .unwrap();
        app.store.commit_block(&p, &b).unwrap();
        app.store.sync_index().unwrap();
    });
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, false, window, cx)
    });
    let header = bounds(cx, "unlinked-header");
    cx.simulate_click(header.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let (link, row) = (0..40)
        .find_map(|ix| {
            let link: &'static str = Box::leak(format!("link-{ix}").into_boxed_str());
            let row: &'static str = Box::leak(format!("mention-{ix}").into_boxed_str());
            Some((cx.debug_bounds(link)?, cx.debug_bounds(row)?))
        })
        .expect("a mention with a Link button was rendered");
    let (left, right) = column(cx);
    assert!(
        f32::from(link.origin.x + link.size.width) <= right + 0.5,
        "Link stays inside the column ({left}, {right}): {link:?}"
    );
    assert_wraps_within(row, left, right);
}

#[gpui::test]
fn left_sidebar_collapses_to_a_rail(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-\\");
    let rail = bounds(cx, "left-rail");
    let main = bounds(cx, "main-pane");
    assert!(approx(rail.size.width, 32.), "{rail:?}");
    assert!(
        approx(main.origin.x, 32.) && approx(main.size.width, W - 32.),
        "{main:?}"
    );
    cx.simulate_keystrokes("cmd-\\");
    let main = bounds(cx, "main-pane");
    assert!(approx(main.origin.x, SIDEBAR_WIDTH), "{main:?}");
    // The open sidebar is the spec's: a search field, the nav row and the mini calendar.
    let search = bounds(cx, "sidebar-search");
    let nav = bounds(cx, "daily-notes");
    let calendar = bounds(cx, "left-calendar");
    assert!(approx(search.size.height, 36.), "{search:?}");
    assert!(approx(nav.size.height, 34.), "{nav:?}");
    assert!(calendar.origin.y > nav.origin.y, "{calendar:?} {nav:?}");
}

/// ALL-3: "Delete page" is a danger item inside the page header's menu, not loose text.
#[gpui::test]
fn page_menu_holds_the_danger_delete(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    // An empty day is a draft until its first write (FR-1.2); write one so it has a page
    // with actions to offer.
    cx.simulate_input("note");
    cx.simulate_keystrokes("escape");
    assert!(
        cx.debug_bounds("page-menu-delete").is_none(),
        "the menu starts closed"
    );
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    let popup = bounds(cx, "page-menu-popup");
    let delete = bounds(cx, "page-menu-delete");
    assert!(
        popup.origin.y >= menu.origin.y + menu.size.height - px(1.),
        "the menu opens below its control: {popup:?} {menu:?}"
    );
    assert!(
        delete.origin.x >= popup.origin.x && delete.origin.x + delete.size.width <= popup.right(),
        "the danger item stays inside the menu: {delete:?} {popup:?}"
    );
}

/// Rule 6: a daily page's title is display-only, so its menu must not offer Rename (which
/// `start_rename` turns into the calendar on daily pages).
#[gpui::test]
fn daily_page_menu_does_not_offer_rename(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("note");
    cx.simulate_keystrokes("escape");
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("page-menu-delete").is_some(),
        "the daily page still offers Delete page"
    );
    assert!(
        cx.debug_bounds("page-menu-rename").is_none(),
        "daily titles are display-only: no Rename action"
    );
}

/// RS-4: the sidebar's left edge is a 4 px drag handle, the header has a close ×, and the
/// width the drag set comes back when the sidebar is reopened.
#[gpui::test]
fn the_right_sidebar_resizes_and_closes(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-shift-\\");
    let side = bounds(cx, "right-sidebar");
    assert!(approx(side.size.width, 380.), "the default width: {side:?}");
    let handle = bounds(cx, "right-resize");
    assert!(approx(handle.size.width, 4.), "a 4 px handle: {handle:?}");
    assert!(
        approx(handle.origin.x, f32::from(side.origin.x)),
        "the handle is the sidebar's left edge: {handle:?} {side:?}"
    );

    // Drag the edge 60 px to the left: the sidebar widens by 60 and the page gives way.
    let start = handle.center();
    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::default());
    let to = gpui::point(start.x - px(60.), start.y);
    cx.simulate_mouse_move(to, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.simulate_mouse_up(to, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    let side = bounds(cx, "right-sidebar");
    assert!(
        approx(side.size.width, 440.),
        "dragged 60 px wider: {side:?}"
    );
    let main = bounds(cx, "main-pane");
    assert!(
        approx(main.size.width, W - SIDEBAR_WIDTH - 440.),
        "the page gives the space up: {main:?}"
    );

    // The × collapses the sidebar and the page takes the space back.
    let close = bounds(cx, "right-close");
    cx.simulate_click(close.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let main = bounds(cx, "main-pane");
    assert!(approx(main.size.width, W - SIDEBAR_WIDTH), "{main:?}");

    // Reopening brings the dragged width back (RS-4 "remember").
    cx.simulate_keystrokes("cmd-shift-\\");
    let side = bounds(cx, "right-sidebar");
    assert!(approx(side.size.width, 440.), "{side:?}");
}

/// ALL-1: page title 28, H1 20, body 15 — measured as line boxes, since the heading used to
/// draw at 24 px (larger than H1 and close to the title).
#[gpui::test]
fn type_scale_caps_outline_headings_at_h1(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("body");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("Heading /head");
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input(" one");
    cx.simulate_keystrokes("escape");
    let title = bounds(cx, "page-title");
    let body = bounds(cx, "content-1");
    let h1 = bounds(cx, "content-2");
    assert!(
        title.size.height > h1.size.height,
        "the title outranks H1: {title:?} {h1:?}"
    );
    // DESIGN.md §2.2: a section heading is 20 / 28 and body is 16 / 28, so the heading
    // outranks the body by type size, not by line box: both rows are 28 px tall.
    assert!(
        h1.size.height >= body.size.height && h1.size.height <= body.size.height + px(0.5),
        "H1 and body share the 28 px line grid: {h1:?} {body:?}"
    );
    assert!(
        h1.size.height <= px(utopia_ui::theme::H1_SIZE + 8.),
        "H1 is 20 px on a 28 px line: {h1:?}"
    );
}

/// ALL-5: Compact shrinks the body and adds 4 px between blocks; both are measurable, and
/// the setting applies to the open window without a restart.
#[gpui::test]
fn compact_density_is_tighter(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_input("one");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("two");
    cx.simulate_keystrokes("escape");
    let body_before = bounds(cx, "content-1").size.height;
    let row_before = bounds(cx, "row-1").size.height;

    let dir = tempfile::tempdir().unwrap();
    cx.update(|_window, cx| {
        utopia_ui::appearance::set_density(cx, utopia_ui::appearance::Density::Compact, dir.path())
            .unwrap();
    });
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("ui-density-compact").is_some(),
        "the open window re-rendered at the new density"
    );
    let body_after = bounds(cx, "content-1").size.height;
    let row_after = bounds(cx, "row-1").size.height;
    assert!(
        approx(body_after, 18.),
        "Compact body is 13.5 px on an 18 px line: {body_after:?}"
    );
    assert!(
        body_after < body_before && row_after < row_before,
        "Compact is tighter: body {body_before:?} -> {body_after:?}, \
         row {row_before:?} -> {row_after:?}"
    );
    assert!(
        approx(row_after - body_after, 4.),
        "4 px of block spacing: row {row_after:?}, body {body_after:?}"
    );
}

/// A fresh page window for one menu state, so no debug bounds from the other state linger.
fn page_title_with_menu(cx: &mut TestAppContext, menu_open: bool) -> Bounds<Pixels> {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Menu Target").unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    if menu_open {
        let menu = cx
            .debug_bounds("page-menu")
            .expect("the page menu button is rendered");
        cx.simulate_click(menu.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("page-menu-popup").is_some(),
            "the menu opened"
        );
    }
    cx.debug_bounds("page-title")
        .expect("the page title is rendered")
}

/// ALL-3: the page menu floats over the page. Opening it must not push the title down.
#[gpui::test]
fn opening_the_page_menu_does_not_move_the_title(cx: &mut TestAppContext) {
    let closed = page_title_with_menu(cx, false);
    let open = page_title_with_menu(cx, true);
    assert_eq!(
        closed, open,
        "the title keeps its bounds when the menu opens"
    );
}

/// DESIGN.md §3.2: the old "Recent days" list is gone on purpose — the feed already shows
/// those days, and the sidebar now starts with the nav and the mini calendar.
#[gpui::test]
fn recent_days_are_gone(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| seed_days(s, 10));
    cx.simulate_keystrokes("escape");
    assert!(
        cx.debug_bounds("recent-days-header").is_none(),
        "no Recent days heading"
    );
    assert!(
        cx.debug_bounds("recent-day-0").is_none(),
        "no recent-day rows"
    );
    // The room it used is the calendar's: the grid is drawn without a toggle.
    let calendar = bounds(cx, "left-calendar");
    let pages = bounds(cx, "pages-header");
    assert!(
        calendar.origin.y < pages.origin.y,
        "the calendar sits above Pages: {calendar:?} {pages:?}"
    );
}

/// DESIGN.md §3.2: tag pages are chips under TAGS, not rows in Pages.
#[gpui::test]
fn tag_pages_are_chips_not_pages(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| {
        let (p, b) = s.open_daily(today()).unwrap();
        s.set_block_text(&p, &b, "#atlas and [[Atlas Launch]]", None, None)
            .unwrap();
        s.commit_block(&p, &b).unwrap();
    });
    cx.simulate_keystrokes("escape");
    let chip = bounds(cx, "tag-chip-atlas");
    let header = bounds(cx, "tags-header");
    assert!(
        approx(chip.size.height, 22.),
        "a tag is a 22 px chip: {chip:?}"
    );
    assert!(
        chip.origin.y >= header.origin.y,
        "the chip is under the TAGS label: {chip:?} {header:?}"
    );
    // Only "Atlas Launch" is a page row; the tag page is not one.
    let row = bounds(cx, "page-row-0");
    assert!(
        cx.debug_bounds("page-row-1").is_none(),
        "the tag page is not in Pages: {row:?}"
    );
}

/// DESIGN.md §3.2's sidebar metrics: 24 px workspace square, 36 px search field, 34 px nav
/// row, 32 px page rows, 30 px calendar cells, 8 px footer dot.
#[gpui::test]
fn sidebar_matches_the_spec_metrics(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| {
        s.create_page("Alpha").unwrap();
    });
    cx.simulate_keystrokes("escape");
    let sidebar = bounds(cx, "left-sidebar");
    let square = bounds(cx, "workspace-initial");
    let search = bounds(cx, "sidebar-search");
    let nav = bounds(cx, "daily-notes");
    let row = bounds(cx, "page-row-0");
    let cell = bounds(cx, "cal-2026-11-02");
    let dot = bounds(cx, "sync-dot-off");
    assert!(approx(square.size.width, 24.) && approx(square.size.height, 24.));
    assert!(approx(search.size.height, 36.), "{search:?}");
    assert!(approx(nav.size.height, 34.), "{nav:?}");
    assert!(approx(row.size.height, 32.), "{row:?}");
    assert!(approx(cell.size.height, 30.), "{cell:?}");
    assert!(
        approx(dot.size.width, 8.) && approx(dot.size.height, 8.),
        "{dot:?}"
    );
    // The footer is pinned to the bottom and the settings button is part of it.
    let footer = bounds(cx, "sidebar-footer");
    let settings = bounds(cx, "open-settings");
    assert!(
        footer.origin.y > sidebar.origin.y + sidebar.size.height / 2.,
        "the footer sits at the bottom: {footer:?} {sidebar:?}"
    );
    assert!(
        settings.origin.x > footer.origin.x + footer.size.width / 2.
            && settings.origin.x + settings.size.width <= sidebar.right(),
        "the settings button is at the footer's right edge: {settings:?}"
    );
}

/// DESIGN.md §3.2: the search field is a button that opens the command palette.
#[gpui::test]
fn sidebar_search_opens_the_command_palette(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape");
    let search = bounds(cx, "sidebar-search");
    assert!(approx(search.size.height, 36.));
    cx.simulate_click(search.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let palette = bounds(cx, "jump-palette");
    assert!(
        palette.origin.x > search.origin.x,
        "the palette opens beside the sidebar: {palette:?} {search:?}"
    );
}

/// DESIGN.md §3.3: the Daily Notes top bar carries the title and a prev | Today | next
/// group; prev moves the feed to the older day and Today brings it back.
#[gpui::test]
fn top_bar_day_navigation_moves_the_feed(cx: &mut TestAppContext) {
    let (ws, cx) = setup_feed(cx, &[(0, 5), (1, 5)], H);
    let bar = bounds(cx, "top-bar");
    assert!(
        bar.size.height >= px(52.),
        "the top bar is at least 52 px: {bar:?}"
    );
    let title = bounds(cx, "topbar-title");
    let nav = bounds(cx, "day-nav");
    assert!(
        title.origin.y >= bar.origin.y
            && title.origin.y + title.size.height <= bar.origin.y + bar.size.height + px(0.5),
        "the title sits in the top bar: {title:?} {bar:?}"
    );
    assert!(
        approx(nav.size.height, 32.),
        "the day group is 32 px tall: {nav:?}"
    );
    assert!(
        nav.origin.x + nav.size.width <= bar.origin.x + bar.size.width + px(0.5),
        "the day group is on the bar's right: {nav:?} {bar:?}"
    );

    // Prev = the older day, as PrevDay means everywhere else.
    let prev = bounds(cx, "day-prev").center();
    cx.simulate_click(prev, gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_sticky_day(cx)),
        None,
        "yesterday's section is at the top, so nothing is pinned"
    );
    let yesterday = bounds(cx, "day-date-2026-11-01");
    let top = content_top(cx);
    assert!(
        yesterday.origin.y > px(top) && yesterday.origin.y < px(top + 90.),
        "the older day's heading is at the top of the feed: {yesterday:?}"
    );

    // Today resets the feed to the first day (§4.1).
    let today = bounds(cx, "day-today").center();
    cx.simulate_click(today, gpui::Modifiers::default());
    cx.run_until_parked();
    let today = bounds(cx, "today-title");
    assert!(
        approx(today.origin.y, content_top(cx)),
        "today is back at the top: {today:?}"
    );
}

/// DESIGN.md §3.3: the page menu is transient. Navigating (a sidebar page, the feed),
/// Escape or a click outside it closes it, and the next page never inherits it.
#[gpui::test]
fn page_menu_is_dismissed_by_navigation_escape_and_outside_click(cx: &mut TestAppContext) {
    let pages = std::cell::RefCell::new(Vec::new());
    let cx = setup_seeded(cx, |store| {
        *pages.borrow_mut() = vec![
            store.create_page("Alpha Page").unwrap(),
            store.create_page("Beta Page").unwrap(),
        ];
    });
    let (alpha, beta) = {
        let pages = pages.borrow();
        (pages[0].clone(), pages[1].clone())
    };
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open(alpha.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    let open = |cx: &mut VisualTestContext| ws.update(cx, |w, _| w.page_menu_open());

    // Navigation: click Beta in the sidebar while the menu is open.
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(open(cx), "the menu opened");
    let row = bounds(cx, "page-row-1");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(beta),
        "the click navigated"
    );
    assert!(!open(cx), "navigating closes the menu");

    // Escape closes it (and does not leave it open for the next page).
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(open(cx));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(!open(cx), "Escape closes the menu");

    // A click outside the menu closes it.
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(open(cx));
    let content = bounds(cx, "main-pane");
    let outside = gpui::point(content.center().x, content.origin.y + px(200.));
    cx.simulate_click(outside, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(!open(cx), "an outside click closes the menu");

    // Leaving for the feed with the menu open must not leave it armed either.
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(open(cx));
    cx.simulate_keystrokes("cmd-shift-d");
    cx.run_until_parked();
    assert!(!open(cx), "opening the feed closes the menu");
}

/// DESIGN.md §3.3: prev/next move between days, including the empty days between open
/// ones — a run of empty-day rows is one stop, not skipped and not stepped row by row.
#[gpui::test]
fn day_nav_stops_at_empty_days(cx: &mut TestAppContext) {
    // Today, three empty days, then an older written day; both ends hold enough that the
    // feed can scroll the empty run to the top.
    let (_ws, cx) = setup_feed(cx, &[(0, 30), (4, 30)], H);
    let prev = bounds(cx, "day-prev").center();
    cx.simulate_click(prev, gpui::Modifiers::default());
    cx.run_until_parked();
    let empty = bounds_of(cx, "empty-day-2026-11-01".into());
    let top = content_top(cx);
    assert!(
        empty.origin.y > px(top) && empty.origin.y < px(top + 90.),
        "prev stops at the first empty day of the run, not the older open day: {empty:?}"
    );
}

/// DESIGN.md §3.3 with FR-1.2: prev at the oldest loaded day asks the feed for the next
/// window instead of dead-ending; the window grows.
#[gpui::test]
fn prev_at_the_last_loaded_day_loads_the_next_window(cx: &mut TestAppContext) {
    // Today is long enough that the empty tail and the sentinel start below the fold, and
    // a note two weeks back keeps the feed from being exhausted: there is a next window.
    let (ws, cx) = setup_feed(cx, &[(0, 30), (14, 30)], H);
    let before = ws
        .update(cx, |w, cx| w.journal_shown_through(cx))
        .expect("in the feed");
    for _ in 0..2 {
        let prev = bounds(cx, "day-prev").center();
        cx.simulate_click(prev, gpui::Modifiers::default());
        cx.run_until_parked();
    }
    let after = ws
        .update(cx, |w, cx| w.journal_shown_through(cx))
        .expect("in the feed");
    assert!(
        after < before,
        "prev at the oldest loaded day loaded older days: {before} -> {after}"
    );
}

/// DESIGN.md §2.5: icon-only buttons name themselves through a tooltip (gpui 0.2 has no
/// accessibility tree), just like the collapse, settings, calendar and menu buttons.
#[gpui::test]
fn day_nav_arrows_carry_a_label(cx: &mut TestAppContext) {
    let (_ws, cx) = setup_feed(cx, &[(0, 3), (1, 3)], H);
    for (id, label) in [("day-prev", "Previous day"), ("day-next", "Next day")] {
        let arrow = bounds(cx, id);
        cx.simulate_mouse_move(arrow.center(), None, gpui::Modifiers::default());
        // gpui waits 500 ms before showing a tooltip.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(700));
        cx.run_until_parked();
        let tip = cx.debug_bounds("tip").unwrap_or_else(|| {
            panic!("{id} has no label tooltip");
        });
        assert!(
            tip.size.width > px(20.) && tip.size.height >= px(12.),
            "{id}'s label ({label}) is drawn: {tip:?}"
        );
    }
}

/// DESIGN.md §3.3: a page view's top bar has the "Pages / title" breadcrumb, the page's ⋯
/// menu and the right-panel toggle. Back/forward and the star are absent: the app has no
/// history and no favorite action to bind them to.
#[gpui::test]
fn page_top_bar_breadcrumb_and_actions(cx: &mut TestAppContext) {
    let page = std::cell::RefCell::new(None);
    let cx = setup_seeded(cx, |store| {
        *page.borrow_mut() = Some(store.create_page("Atlas Launch").unwrap());
    });
    let page = page.borrow_mut().take().expect("seeded page");
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, None, false, window, cx)
    });
    cx.run_until_parked();
    let bar = bounds(cx, "top-bar");
    let crumb = bounds(cx, "breadcrumb");
    let menu = bounds(cx, "page-menu");
    let title = bounds(cx, "page-title");
    assert!(bar.size.height >= px(52.), "{bar:?}");
    assert!(
        crumb.origin.y >= bar.origin.y
            && crumb.origin.x >= bar.origin.x
            && crumb.origin.y + crumb.size.height <= bar.origin.y + bar.size.height + px(0.5),
        "the breadcrumb sits in the bar: {crumb:?} {bar:?}"
    );
    assert!(
        menu.origin.x > crumb.origin.x + crumb.size.width,
        "the page menu is on the right of the crumb: {menu:?} {crumb:?}"
    );
    assert!(
        title.origin.y >= bar.origin.y + bar.size.height - px(0.5),
        "the content title stays below the bar: {title:?} {bar:?}"
    );
    // Deliberate for this phase, not a contract: the app has no history or favorite
    // action, so the controls stay out rather than being invented. Remove these two
    // assertions when back/forward and the star get real actions.
    assert!(
        cx.debug_bounds("go-back").is_none() && cx.debug_bounds("go-forward").is_none(),
        "no history actions exist, so no back/forward controls are drawn"
    );
    assert!(
        cx.debug_bounds("favorite").is_none(),
        "no favorite action exists, so no star is drawn"
    );

    // The right panel toggle opens the right sidebar.
    assert!(cx.debug_bounds("right-sidebar").is_none());
    let toggle = bounds(cx, "toggle-right").center();
    cx.simulate_click(toggle, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("right-sidebar").is_some(),
        "the right-panel toggle opens the sidebar"
    );
}

/// SB-3: a page pinned from its "···" menu sits in a Pinned group above Pages, the pin is
/// written to settings.json as local state (rule 9), and the row's ✕ unpins it without
/// opening the page behind the click.
#[gpui::test]
fn pinned_pages_sit_above_pages_and_are_local(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| {
        s.create_page("Alpha").unwrap();
        s.create_page("Beta").unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    cx.update(|_, cx| utopia_ui::appearance::install(cx, dir.path()));
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let alpha = PageId::for_title_key("alpha");
    let beta = PageId::for_title_key("beta");
    cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open(alpha.clone(), None, None, false, window, cx)
        })
    });
    cx.run_until_parked();
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    let pin = bounds(cx, "page-menu-pin");
    cx.simulate_click(pin.center(), gpui::Modifiers::default());
    cx.run_until_parked();

    let pinned_header = bounds(cx, "pinned-header");
    let pages_header = bounds(cx, "pages-header");
    let row = bounds_of(cx, "pinned-row-0".into());
    assert!(
        pinned_header.origin.y < pages_header.origin.y,
        "the Pinned group is above Pages: {pinned_header:?} {pages_header:?}"
    );
    assert!(
        row.origin.y >= pinned_header.origin.y
            && row.origin.y + row.size.height <= pages_header.origin.y + px(0.5),
        "the pinned row sits between the two headers: {row:?}"
    );
    let saved = || -> serde_json::Value {
        let s = std::fs::read_to_string(dir.path().join("settings.json")).expect("settings.json");
        serde_json::from_str(&s).expect("settings.json is JSON")
    };
    assert_eq!(saved()["pinned"], serde_json::json!([alpha.as_str()]));
    assert!(
        !dir.path().join("keymap.json").exists(),
        "a pin is not a keybinding"
    );
    // A new launch reads the pin back from the file.
    cx.update(|_, cx| {
        utopia_ui::appearance::install(cx, dir.path());
        assert_eq!(
            utopia_ui::appearance::pinned(cx),
            std::slice::from_ref(&alpha)
        );
    });

    // Show another page, then unpin Alpha with the row's ✕. The click must not open Alpha.
    cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open(beta.clone(), None, None, false, window, cx)
        })
    });
    cx.run_until_parked();
    let before = pages_header.origin.y;
    let unpin = bounds_of(cx, "unpin-0".into());
    cx.simulate_click(unpin.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(beta),
        "unpinning does not follow the row's link"
    );
    assert_eq!(saved()["pinned"], serde_json::json!([]));
    let pages_header = bounds(cx, "pages-header");
    assert!(
        pages_header.origin.y < before,
        "Pages moves up once the Pinned group is gone: {pages_header:?}"
    );
}

/// SB-4: the footer is one row — the graph chip, the sync dot and (for an encrypted graph)
/// a lock — at the bottom of the sidebar. Clicking it opens the sync view as an overlay
/// above the footer; nothing in the page moves.
#[gpui::test]
fn footer_opens_the_sync_view_without_moving_the_page(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape");
    let sidebar = bounds(cx, "left-sidebar");
    let footer = bounds(cx, "sidebar-footer");
    let chip = bounds(cx, "vault-name");
    let dot = bounds(cx, "sync-dot-off");
    assert!(
        cx.debug_bounds("local-only").is_some(),
        "a graph with no sync target says it is local only"
    );
    assert!(
        footer.origin.y + footer.size.height <= sidebar.origin.y + sidebar.size.height + px(0.5),
        "the footer is inside the sidebar: {footer:?} {sidebar:?}"
    );
    assert!(
        footer.origin.y > sidebar.origin.y + sidebar.size.height / 2.,
        "the footer sits at the bottom: {footer:?} {sidebar:?}"
    );
    // One row: the dot first, then the vault name; the settings button is separate.
    let chip_mid = f32::from(chip.origin.y) + f32::from(chip.size.height) / 2.;
    let dot_mid = f32::from(dot.origin.y) + f32::from(dot.size.height) / 2.;
    assert!(
        (chip_mid - dot_mid).abs() <= 4.,
        "the vault name and the dot are on one row: {chip:?} {dot:?}"
    );
    assert!(
        approx(dot.origin.x, f32::from(footer.origin.x))
            && chip.origin.x >= dot.origin.x + dot.size.width
            && f32::from(chip.right()) <= f32::from(footer.right()),
        "dot, then the vault name, both inside the row: {chip:?} {dot:?} {footer:?}"
    );
    assert!(
        cx.debug_bounds("encryption-lock").is_none(),
        "an in-memory graph is not encrypted"
    );

    let main = bounds(cx, "main-pane");
    cx.simulate_click(footer.center(), gpui::Modifiers::default());
    let popover = bounds(cx, "sync-popover");
    assert!(
        f32::from(popover.origin.x) >= f32::from(sidebar.origin.x)
            && f32::from(popover.right()) <= f32::from(sidebar.right()) + 0.5,
        "the sync view stays inside the sidebar: {popover:?} {sidebar:?}"
    );
    assert!(
        f32::from(popover.bottom()) <= f32::from(footer.origin.y) + 0.5,
        "the sync view floats above the footer: {popover:?} {footer:?}"
    );
    let after = bounds(cx, "main-pane");
    assert!(
        approx(after.origin.x, f32::from(main.origin.x))
            && approx(after.size.width, f32::from(main.size.width)),
        "opening the sync view does not move the page: {main:?} {after:?}"
    );

    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    assert!(ws.update(cx, |w, _| w.sync_view_open()));
    cx.simulate_click(footer.center(), gpui::Modifiers::default());
    assert!(
        !ws.update(cx, |w, _| w.sync_view_open()),
        "clicking the footer again closes the sync view"
    );
}

/// DESIGN.md §3.2: the sidebar's mini calendar is always shown — month label with
/// prev/next, Sunday-first weeks, mono weekday labels, 30 px cells, today a solid link
/// cell, a link dot under days with notes, and a day click that opens that daily note.
#[gpui::test]
fn mini_calendar_opens_days_and_marks_notes(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| seed_days(s, 3));
    cx.simulate_keystrokes("escape");
    let month = bounds(cx, "cal-month");
    let grid = bounds(cx, "left-calendar");
    assert!(
        grid.origin.y >= month.origin.y + month.size.height - px(0.5),
        "the grid is under the month label: {grid:?} {month:?}"
    );

    // November 2026 starts on a Sunday, so the first row is Nov 1–7 (not Oct 26–Nov 1).
    let row = [
        "2026-11-01",
        "2026-11-02",
        "2026-11-03",
        "2026-11-04",
        "2026-11-05",
        "2026-11-06",
        "2026-11-07",
    ];
    let cells: Vec<Bounds<Pixels>> = row
        .iter()
        .map(|iso| bounds_of(cx, format!("cal-{iso}")))
        .collect();
    for (i, cell) in cells.iter().enumerate() {
        assert!(approx(cell.size.height, 30.), "a 30 px cell: {cell:?}");
        assert!(
            approx(cell.origin.y, f32::from(cells[0].origin.y)),
            "seven columns share a line: {cell:?}"
        );
        if i > 0 {
            assert!(
                cell.origin.x > cells[i - 1].origin.x,
                "columns go left to right: {cell:?}"
            );
        }
        assert!(
            cell.origin.x >= grid.origin.x
                && cell.origin.x + cell.size.width <= grid.origin.x + grid.size.width + px(0.5),
            "cell {i} stays inside the grid: {cell:?} {grid:?}"
        );
    }

    // Seeded days (Nov 2, Nov 1, Oct 31) have dots; an untouched day does not.
    for iso in ["2026-11-02", "2026-11-01"] {
        bounds_of(cx, format!("cal-dot-{iso}"));
    }
    assert!(
        cx.debug_bounds("cal-dot-2026-11-15").is_none(),
        "a day with no entry has no dot"
    );

    // A day cell opens that day (a draft until written, FR-1.2).
    let cell = bounds_of(cx, "cal-2026-11-15".into());
    cx.simulate_click(cell.center(), gpui::Modifiers::default());
    let day = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(PageId::for_date(day))
    );
}

/// DESIGN.md §3.2 fixes the sidebar's page rows at 32 px; the density setting tightens
/// blocks, not this chrome.
#[gpui::test]
fn sidebar_rows_keep_the_spec_height_in_compact(cx: &mut TestAppContext) {
    let cx = setup_seeded(cx, |s| {
        s.create_page("Alpha").unwrap();
    });
    cx.simulate_keystrokes("escape");
    let before = bounds_of(cx, "page-row-0".into());
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"version": 1, "density": "compact"}"#,
    )
    .unwrap();
    cx.update(|_, cx| {
        utopia_ui::appearance::install(cx, dir.path());
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("ui-density-compact").is_some(),
        "the workspace was drawn again in Compact"
    );
    let after = bounds_of(cx, "page-row-0".into());
    assert!(
        approx(before.size.height, 32.) && approx(after.size.height, 32.),
        "a page row keeps its spec height: {before:?} {after:?}"
    );
}

/// SB-3: a written daily note is a page too (§5.5), so it can be pinned. Pins resolve
/// against every live page, not only the ordinary Pages list (which excludes daily pages).
#[gpui::test]
fn pinned_daily_pages_show_above_pages(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    // The page menu is only offered on a live page (a draft day has none).
    cx.simulate_input("note");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    let pin = bounds(cx, "page-menu-pin");
    cx.simulate_click(pin.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let header = bounds(cx, "pinned-header");
    let row = bounds_of(cx, "pinned-row-0".into());
    let pages = bounds(cx, "pages-header");
    assert!(
        header.origin.y < row.origin.y && row.origin.y + row.size.height <= pages.origin.y,
        "the pinned day sits in the Pinned group above Pages: {header:?} {row:?} {pages:?}"
    );
}

/// A page whose outline holds a quote, then a code block that names its language (PG-5):
/// items are 0 header, 1 quote, 2 code.
fn setup_quote_and_code(
    cx: &mut TestAppContext,
) -> (&mut VisualTestContext, Vec<utopia_core::BlockId>) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Types").unwrap();
    // A quote, a code block that names its language, and a plain bullet to measure against.
    let rows = [
        (BlockKind::Quote, "Ship the small thing that works."),
        (BlockKind::Code, "rust\nfn main() {}"),
        (BlockKind::Bullet, "plain"),
    ];
    let mut ids = Vec::new();
    let mut cmds = Vec::new();
    let mut prev: Option<BlockId> = None;
    for (kind, text) in rows {
        let id = BlockId::new();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent: None,
            after: prev.clone(),
            text: text.to_string(),
            kind,
        });
        prev = Some(id.clone());
        ids.push(id);
    }
    store
        .execute(
            &page,
            cmds,
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    (cx, ids)
}

/// PG-5: a quote is set off from the outline by its bar and padding, and a code block's
/// language sits at the top right of its card, above the code, not as a line of code.
#[gpui::test]
fn quote_is_indented_and_code_names_its_language(cx: &mut TestAppContext) {
    let (cx, ids) = setup_quote_and_code(cx);
    let text = |cx: &mut VisualTestContext, block: &utopia_core::BlockId| {
        // `debug_bounds` takes a `&'static str`; leaking a few test selectors is fine.
        let sel: &'static str = Box::leak(format!("text-{}", block.0).into_boxed_str());
        bounds(cx, sel)
    };
    let quote = text(cx, &ids[0]);
    let code = text(cx, &ids[1]);
    let bullet = text(cx, &ids[2]);
    let card = bounds(cx, "content-2");
    let lang = bounds(cx, "code-language-2");
    let row = bounds(cx, "row-2");
    // 2 px bar plus 8 px padding for the quote, the card's 8 px padding for the code.
    assert!(
        approx(quote.origin.x - bullet.origin.x, 10.),
        "the quote's bar and padding inset its text: {quote:?} {bullet:?}"
    );
    assert!(
        approx(code.origin.x - bullet.origin.x, 8.),
        "the code sits inside its card: {code:?} {bullet:?}"
    );
    assert!(
        lang.origin.y < code.origin.y,
        "the label is above the first line of code: {lang:?} {code:?}"
    );
    assert!(
        lang.origin.x + lang.size.width <= card.right() + px(1.),
        "the label stays inside the card: {lang:?} {card:?}"
    );
    assert!(
        card.right() - (lang.origin.x + lang.size.width) < px(12.),
        "the label is right-aligned in the card: {lang:?} {card:?}"
    );
    assert!(
        row.size.height >= card.size.height,
        "the card takes its own room in the outline: {row:?} {card:?}"
    );
}

/// PG-4: the bullet reads as a drag handle only under the pointer, and a parent's collapse
/// arrow appears with it — but only for the row being hovered.
#[gpui::test]
fn block_affordances_appear_on_hover(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("parent");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab escape");
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("grip-1").is_none(),
        "a row shows no drag handle until the pointer is over it"
    );
    assert!(
        cx.debug_bounds("arrow-1").is_none(),
        "an expanded parent's arrow is quiet at rest"
    );

    let child = bounds(cx, "row-2");
    cx.simulate_mouse_move(child.center(), None, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("grip-2").is_some(),
        "the hovered row shows its drag handle"
    );
    assert!(
        cx.debug_bounds("arrow-1").is_none(),
        "hovering one row does not reveal another row's arrow"
    );

    let parent = bounds(cx, "row-1");
    cx.simulate_mouse_move(parent.center(), None, gpui::Modifiers::default());
    cx.run_until_parked();
    let grip = bounds(cx, "grip-1");
    let arrow = bounds(cx, "arrow-1");
    let content = bounds(cx, "content-1");
    assert!(
        arrow.origin.x < grip.origin.x && grip.origin.x < content.origin.x,
        "arrow, then the handle, then the text: {arrow:?} {grip:?} {content:?}"
    );
    assert!(
        grip.size.height > px(4.) && grip.size.height < px(14.),
        "the grip is a small drawn handle: {grip:?}"
    );
}

/// DESIGN.md §3.1: the to-do box is a drawn 16 px control with a 10 px gap before the
/// text, and clicking it checks the to-do (the box, not the font's ☐ glyph, is the target).
#[gpui::test]
fn todo_box_is_a_drawn_control(cx: &mut TestAppContext) {
    let cx = setup_page(cx);
    cx.simulate_input("milk");
    cx.simulate_keystrokes("alt-enter escape");
    cx.run_until_parked();
    let boxed = bounds(cx, "todo-1");
    let content = bounds(cx, "content-1");
    assert!(
        approx(boxed.size.width, 16.) && approx(boxed.size.height, 16.),
        "a 16 px box: {boxed:?}"
    );
    assert!(
        boxed.origin.x < content.origin.x,
        "the box sits left of the text: {boxed:?} {content:?}"
    );
    assert!(
        approx(content.origin.x - (boxed.origin.x + boxed.size.width), 10.),
        "10 px between the box and the text: {boxed:?} {content:?}"
    );
    cx.simulate_click(boxed.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    let page = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        let first = st.live_roots().first().cloned().expect("one block");
        let b = st.block(&first).expect("the block");
        assert_eq!(b.kind, utopia_core::BlockKind::Todo, "still a to-do");
        assert!(b.checked, "clicking the box checked it");
    });
}

/// A page with one alias, opened on its own, for the header toolbar (PG-2).
fn setup_alias_page(cx: &mut TestAppContext) -> &mut VisualTestContext {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Trellis").unwrap();
    store.add_alias(&page, "the board").unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    cx
}

/// PG-2: under the title sits one toolbar — alias chips, "+ alias", then the "···" menu —
/// and the menu's items run Open in graph, Export, Rename, Delete page last.
#[gpui::test]
fn page_toolbar_sits_under_the_title(cx: &mut TestAppContext) {
    let cx = setup_alias_page(cx);
    let title = bounds(cx, "page-title");
    let bar = bounds(cx, "properties-bar");
    let aliases = bounds(cx, "property-aliases");
    let alias = bounds(cx, "alias-0");
    let add = bounds(cx, "add-alias");
    let menu = bounds(cx, "page-menu");
    assert!(
        bar.origin.y >= title.origin.y + title.size.height - px(1.),
        "the properties bar follows the title: {title:?} {bar:?}"
    );
    assert!(
        alias.origin.x < add.origin.x && add.origin.x < aliases.right() + px(0.5),
        "chips, then + alias, in the aliases row: {alias:?} {add:?} {aliases:?}"
    );
    for (what, b) in [("alias", alias), ("+ alias", add)] {
        assert!(
            b.origin.y >= aliases.origin.y - px(1.)
                && b.origin.y + b.size.height <= aliases.bottom() + px(1.),
            "{what} sits in the aliases row: {b:?} {aliases:?}"
        );
    }
    // DESIGN.md §3.3: the page actions moved to the top bar's ⋯ button.
    let top = bounds(cx, "top-bar");
    assert!(
        menu.origin.y >= top.origin.y
            && menu.origin.y + menu.size.height <= top.origin.y + top.size.height + px(0.5),
        "the page menu sits in the top bar: {menu:?} {top:?}"
    );

    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let opened = bounds(cx, "properties-bar");
    assert_eq!(
        bar, opened,
        "opening the menu floats over the page; the bar stays put"
    );
    let graph = bounds(cx, "page-menu-graph");
    let export = bounds(cx, "page-menu-export");
    let rename = bounds(cx, "page-menu-rename");
    let delete = bounds(cx, "page-menu-delete");
    assert!(
        graph.origin.y < export.origin.y
            && export.origin.y < rename.origin.y
            && rename.origin.y < delete.origin.y,
        "menu order: {graph:?} {export:?} {rename:?} {delete:?}"
    );
    assert!(
        delete.origin.y > rename.origin.y + rename.size.height,
        "a hairline sets the danger item apart, last: {rename:?} {delete:?}"
    );
}

/// PG-2: "Open in graph" in the page menu shows the graph view with the page highlighted,
/// the same view Cmd+Shift+G opens.
#[gpui::test]
fn page_menu_opens_the_graph(cx: &mut TestAppContext) {
    let cx = setup_alias_page(cx);
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let item = bounds(cx, "page-menu-graph");
    cx.simulate_click(item.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    assert!(
        ws.update(cx, |w, _| w.graph_view()).is_some(),
        "the graph view is showing"
    );
    assert!(
        cx.debug_bounds("graph-view").is_some(),
        "and it is drawn over the page"
    );
}

/// A page with two `key:: value` root blocks and one ordinary block (PG-1), with
/// `settings.json` living in `dir` so a toggle can be checked on disk.
/// A page with two leading properties, a body note, and the page the `owner` links to.
fn setup_properties_page<'a>(
    cx: &'a mut TestAppContext,
    dir: &std::path::Path,
) -> (&'a mut VisualTestContext, Vec<utopia_core::BlockId>) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    let path = dir.to_path_buf();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::install(cx, &path);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    store.create_page("Ana Lovelace").unwrap();
    let page = store.create_page("Trellis").unwrap();
    let ids: Vec<BlockId> = (0..3).map(|_| BlockId::new()).collect();
    store
        .execute(
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
                    text: "owner:: [[Ana Lovelace]]".into(),
                    kind: BlockKind::Bullet,
                },
                Command::InsertBlock {
                    id: ids[2].clone(),
                    parent: None,
                    after: Some(ids[1].clone()),
                    text: "An ordinary note".into(),
                    kind: BlockKind::Bullet,
                },
            ],
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    (cx, ids)
}

/// The texts of a page's live root blocks, in order.
fn root_texts(cx: &mut VisualTestContext) -> Vec<String> {
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    let page = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        st.live_roots()
            .iter()
            .map(|b| st.block(b).expect("block").text.clone())
            .collect()
    })
}

/// A page's block text by id (any depth).
fn block_text(cx: &mut VisualTestContext, block: &utopia_core::BlockId) -> String {
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let app = ws.update(cx, |w, _| w.app().clone());
    let page = ws.update(cx, |w, cx| w.main_page(cx)).expect("a page");
    app.update(cx, |app, _| {
        let st = app.store.page_state(&page).expect("page state");
        st.block(block).map(|b| b.text.clone()).unwrap_or_default()
    })
}

fn editor_target(cx: &mut VisualTestContext) -> Option<utopia_core::BlockId> {
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update(cx, |w, cx| {
        w.editor().read(cx).target().map(|t| t.block.clone())
    })
}

/// DESIGN.md §3.8 rules 1 and 2: the bar renders exactly the leading consecutive property
/// blocks and they are not drawn again as bullets. It sits 20 px below the title, rows are
/// at least 36 px, the status value is a 26 px pill and a page link gets a 22 px avatar.
#[gpui::test]
fn properties_bar_draws_the_leading_blocks_once(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _ids) = setup_properties_page(cx, dir.path());
    let title = bounds(cx, "page-title");
    let bar = bounds(cx, "properties-bar");
    let status = bounds(cx, "property-0");
    let owner = bounds(cx, "property-1");
    let note = bounds(cx, "content-1");
    assert!(
        bar.origin.y >= title.origin.y + title.size.height,
        "the bar is below the title: {bar:?} {title:?}"
    );
    assert!(
        status.origin.y < owner.origin.y && owner.origin.y < note.origin.y,
        "properties, then the body: {status:?} {owner:?} {note:?}"
    );
    assert!(
        status.size.height >= px(36.),
        "a property row is at least 36 px: {status:?}"
    );
    assert!(
        approx(bounds(cx, "property-status").size.height, 26.),
        "the status value is a 26 px pill"
    );
    assert!(
        approx(bounds(cx, "property-avatar").size.width, 22.),
        "a page-link value carries a 22 px avatar"
    );
    // The old projection and compact rows are gone: the data is drawn once.
    assert!(cx.debug_bounds("meta-status").is_none());
    assert!(cx.debug_bounds("attr-0").is_none());
    assert!(
        cx.debug_bounds("content-2").is_none(),
        "the two properties are not also bullets; one body row remains"
    );
}

/// DESIGN.md §3.8 rule 3: a property block lower in the outline is a normal bullet.
#[gpui::test]
fn a_lower_property_block_is_a_normal_bullet(cx: &mut TestAppContext) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::install(cx, &path);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Trellis").unwrap();
    let mut prev: Option<BlockId> = None;
    let mut cmds = Vec::new();
    for text in ["An ordinary note", "status:: lower"] {
        let id = BlockId::new();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent: None,
            after: prev.clone(),
            text: text.into(),
            kind: BlockKind::Bullet,
        });
        prev = Some(id);
    }
    store
        .execute(
            &page,
            cmds,
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("property-0").is_none(), "the bar is empty");
    bounds(cx, "add-property");
    // Both roots are bullets, the lower property included.
    bounds(cx, "content-1");
    let lower = bounds(cx, "content-2");
    assert!(lower.size.height >= px(28.), "{lower:?}");
}

/// DESIGN.md §3.8 rule 4: no leading properties means only the "Add property" row (and the
/// always-present aliases row).
#[gpui::test]
fn the_bar_without_properties_shows_only_add_property(cx: &mut TestAppContext) {
    let page = std::cell::RefCell::new(None);
    let cx = setup_seeded(cx, |store| {
        let p = store.create_page("Trellis").unwrap();
        let b = store.ensure_first_block(&p).unwrap();
        store.set_block_text(&p, &b, "A note", None, None).unwrap();
        store.commit_block(&p, &b).unwrap();
        *page.borrow_mut() = Some(p);
    });
    let page = page.borrow_mut().take().expect("page");
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, None, false, window, cx)
    });
    cx.run_until_parked();
    bounds(cx, "properties-bar");
    bounds(cx, "add-property");
    bounds(cx, "property-aliases");
    assert!(cx.debug_bounds("property-0").is_none(), "no property rows");
    bounds(cx, "content-1");
}

/// DESIGN.md §3.8 editing: clicking a row turns it into its raw block text in place; Escape
/// commits and draws the row again.
#[gpui::test]
fn clicking_a_property_row_edits_its_raw_block(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_properties_page(cx, dir.path());
    let row = bounds(cx, "property-0");
    cx.simulate_mouse_down(
        row.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    cx.simulate_mouse_up(
        row.center(),
        gpui::MouseButton::Left,
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    bounds(cx, "property-edit");
    bounds(cx, "block-editor");
    assert_eq!(editor_target(cx), Some(ids[0].clone()));
    cx.simulate_input("!");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(block_text(cx, &ids[0]), "status:: dogfooding!");
    bounds(cx, "property-status");
}

/// DESIGN.md §3.8: Enter at the end of the last property row leaves the bar and focuses the
/// first body block.
#[gpui::test]
fn enter_at_the_end_of_the_last_property_focuses_the_body(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_properties_page(cx, dir.path());
    // The last property is the owner row; clicking it puts the caret at the end.
    let row = bounds(cx, "property-1");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(editor_target(cx), Some(ids[1].clone()));
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(
        editor_target(cx),
        Some(ids[2].clone()),
        "Enter left the bar for the first body block"
    );
    assert!(
        cx.debug_bounds("property-edit").is_none(),
        "the bar is drawn as rows again"
    );
}

/// DESIGN.md §3.8: "Add property" inserts an empty property block at the end of the bar, in
/// edit mode; typing a `key:: value` makes it a property.
#[gpui::test]
fn add_property_inserts_an_empty_row_in_edit_mode(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_properties_page(cx, dir.path());
    let add = bounds(cx, "add-property");
    cx.simulate_click(add.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "property-edit");
    assert!(
        !ids.contains(&editor_target(cx).expect("a new block is being edited")),
        "the editor is in a fresh block, not an existing property"
    );
    cx.simulate_input("priority:: high");
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    bounds(cx, "property-2");
    assert!(root_texts(cx).iter().any(|t| t == "priority:: high"));
}

/// DESIGN.md §3.8: deleting all the text in a property row removes the block and its row.
#[gpui::test]
fn emptying_a_property_row_removes_the_block(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, _ids) = setup_properties_page(cx, dir.path());
    let row = bounds(cx, "property-0");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes("cmd-a backspace escape");
    cx.run_until_parked();
    let texts = root_texts(cx);
    assert!(
        !texts.iter().any(|t| t == "status:: dogfooding"),
        "the emptied property block is gone: {texts:?}"
    );
    // The next property moved up into the first row.
    let owner = bounds(cx, "property-0");
    assert!(owner.size.height >= px(36.));
}

/// DESIGN.md §3.8: the bar takes part in normal block arrow navigation.
#[gpui::test]
fn arrow_keys_move_between_the_bar_and_the_body(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_properties_page(cx, dir.path());
    let row = bounds(cx, "property-0");
    cx.simulate_click(row.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(
        editor_target(cx),
        Some(ids[1].clone()),
        "down to the next property"
    );
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(
        editor_target(cx),
        Some(ids[2].clone()),
        "down into the body"
    );
    cx.simulate_keystrokes("up");
    cx.run_until_parked();
    assert_eq!(
        editor_target(cx),
        Some(ids[1].clone()),
        "up into the last property"
    );
}

/// FR-2.5 with DESIGN.md §3.8: Backspace at the start of the first body block does not
/// merge it into a property that is not part of the outline.
#[gpui::test]
fn backspace_at_the_first_body_block_does_not_merge_the_bar(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (cx, ids) = setup_properties_page(cx, dir.path());
    let note = bounds(cx, "content-1");
    cx.simulate_click(
        gpui::point(note.origin.x + px(1.), note.origin.y + px(8.)),
        gpui::Modifiers::default(),
    );
    cx.run_until_parked();
    cx.simulate_keystrokes("backspace");
    cx.run_until_parked();
    assert_eq!(block_text(cx, &ids[2]), "An ordinary note");
    assert_eq!(block_text(cx, &ids[1]), "owner:: [[Ana Lovelace]]");
    assert_eq!(block_text(cx, &ids[0]), "status:: dogfooding");
}

/// DESIGN.md §4.2: an empty body is the muted placeholder with a `line_strong` bullet.
#[gpui::test]
fn an_empty_body_gets_a_line_strong_bullet(cx: &mut TestAppContext) {
    use utopia_core::{BlockId, BlockKind, Command};
    use utopia_store::EditOpts;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        utopia_ui::appearance::install(cx, &path);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let page = store.create_page("Trellis").unwrap();
    store
        .execute(
            &page,
            vec![Command::InsertBlock {
                id: BlockId::new(),
                parent: None,
                after: None,
                text: "status:: x".into(),
                kind: BlockKind::Bullet,
            }],
            EditOpts {
                no_undo: true,
                ..Default::default()
            },
        )
        .unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| {
        w.open(page.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    bounds(cx, "empty");
    assert!(
        approx(bounds(cx, "empty-bullet").size.width, 6.),
        "the placeholder bullet is 6 px"
    );
}

/// A graph whose "Target" page has two referencing blocks, one of them also mentioning "Ana
/// Lovelace", so the reference controls offer a co-reference chip with a count (LR-1).
fn setup_ref_controls(
    cx: &mut TestAppContext,
) -> (&mut VisualTestContext, Entity<Workspace>, PageId) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    store.create_page("Ana Lovelace").unwrap();
    let src = store.create_page("Source").unwrap();
    let b = store.ensure_first_block(&src).unwrap();
    store
        .set_block_text(
            &src,
            &b,
            "A note about [[Target]] with [[Ana Lovelace]]",
            None,
            None,
        )
        .unwrap();
    store.commit_block(&src, &b).unwrap();
    let plan = utopia_core::outline::append_block(store.page_state(&src).unwrap(), None);
    let b2 = store.execute_plan(&src, plan, None).unwrap().unwrap().block;
    store
        .set_block_text(&src, &b2, "Another note about [[Target]] alone", None, None)
        .unwrap();
    store.commit_block(&src, &b2).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    (cx, ws, target)
}

/// LR-1: one control row — a 240 px filter field, then the date and sort menus as buttons —
/// and the co-reference chips (with their counts) wrapping on the line below it.
#[gpui::test]
fn reference_controls_are_one_row_with_chips_below(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    let filter = bounds(cx, "refs-filter");
    let row = bounds(cx, "refs-control-row");
    let date = bounds(cx, "refs-date-menu-any-time");
    let sort = bounds(cx, "refs-sort-menu-title");
    assert!(
        approx(filter.size.width, 240.),
        "a 240 px field: {filter:?}"
    );
    assert!(
        approx(
            date.origin.y + date.size.height / 2.,
            f32::from(row.origin.y) + f32::from(row.size.height) / 2.
        ),
        "the date menu sits on the same row as the field: {date:?} {row:?}"
    );
    assert!(
        date.origin.x >= filter.origin.x + filter.size.width,
        "after the field: {date:?} {filter:?}"
    );
    assert!(sort.origin.x >= date.origin.x + date.size.width, "{sort:?}");
    // Both are quiet control buttons (ALL-3): the same compact height, not a form field.
    assert!(
        approx(sort.size.height, f32::from(date.size.height)) && sort.size.height <= px(34.),
        "compact menu buttons: {sort:?} {date:?}"
    );
    let chips = bounds(cx, "refs-chips");
    let chip = bounds(cx, "suggest-chip-0");
    assert!(
        chips.origin.y >= row.origin.y + row.size.height - px(0.5),
        "co-reference chips wrap onto the next line: {chips:?} {row:?}"
    );
    assert!(
        chip.origin.y >= row.origin.y + row.size.height - px(0.5),
        "a chip is below the control row: {chip:?}"
    );
}

/// ALL-3/LR-1: the filter menus float over the page. Opening one must not move the controls
/// or the references under them.
#[gpui::test]
fn a_reference_menu_floats_without_moving_the_layout(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    let row_before = bounds(cx, "refs-control-row");
    let hit_before = first_hit(cx);
    assert!(
        cx.debug_bounds("refs-menu-popup").is_none(),
        "no menu until the control is clicked"
    );
    let date = bounds(cx, "refs-date-menu-any-time");
    cx.simulate_click(date.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let popup = bounds(cx, "refs-menu-popup");
    assert!(
        popup.origin.y >= date.origin.y + date.size.height - px(1.),
        "the menu opens below its control: {popup:?} {date:?}"
    );
    assert!(
        approx(
            row_before.origin.y,
            f32::from(bounds(cx, "refs-control-row").origin.y)
        ) && approx(hit_before.origin.y, f32::from(first_hit(cx).origin.y)),
        "opening the menu takes no space in the column"
    );
    // Picking a choice applies it (the label and its state change) and closes the menu.
    let option = bounds(cx, "refs-date-option-last-7-days");
    cx.simulate_click(option.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-date-menu-last-7-days");
    let sort = bounds(cx, "refs-sort-menu-title");
    cx.simulate_click(sort.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let recent = bounds(cx, "refs-sort-option-recent");
    cx.simulate_click(recent.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-sort-menu-recent");
}
/// ALL-3 follow-up: the page menu is drawn over the page, so it must also take the clicks
/// meant for it. Without `occlude` the "Delete page" item is visible but never fires (found
/// while building the reference menus).
#[gpui::test]
fn page_menu_items_take_their_clicks(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_refs(cx, false);
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    cx.run_until_parked();
    let menu = bounds(cx, "page-menu");
    cx.simulate_click(menu.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let delete = bounds(cx, "page-menu-delete");
    cx.simulate_click(delete.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let app = ws.update(cx, |w, _| w.app().clone());
    let deleted = app.update(cx, |app, _| {
        app.store
            .page_row(&target)
            .unwrap()
            .is_some_and(|r| r.deleted_at.is_some())
    });
    assert!(deleted, "the danger item deleted the page it belongs to");
}

/// The first `group-title-{ix}` and its count chip, by trying list indices.
fn first_group(cx: &mut VisualTestContext) -> (Bounds<Pixels>, Bounds<Pixels>) {
    cx.run_until_parked();
    (0..40)
        .find_map(|ix| {
            let title: &'static str = Box::leak(format!("group-title-{ix}").into_boxed_str());
            let title = cx.debug_bounds(title)?;
            // The count is in the selector, so try the counts a small graph can have.
            (1..=9).find_map(|n| {
                let count: &'static str =
                    Box::leak(format!("group-count-{ix}-{n}").into_boxed_str());
                Some((title, cx.debug_bounds(count)?))
            })
        })
        .expect("a referencing group was rendered")
}

/// LR-2: the count is a small chip right after the page or day name, not a number pushed to
/// the far edge of the row.
#[gpui::test]
fn reference_counts_sit_next_to_the_name(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    let title = bounds(cx, "refs-title");
    let count = bounds(cx, "refs-count-2");
    let header = bounds(cx, "refs-header");
    assert!(
        count.origin.x >= title.origin.x + title.size.width - px(1.),
        "the count follows the title: {count:?} {title:?}"
    );
    assert!(
        count.origin.x - (title.origin.x + title.size.width) < px(16.),
        "and sits right next to it: {count:?} {title:?}"
    );
    assert!(
        approx(
            count.origin.y + count.size.height / 2.,
            f32::from(title.origin.y) + f32::from(title.size.height) / 2.
        ),
        "on the same line: {count:?} {title:?}"
    );
    assert!(
        count.origin.x + count.size.width < header.origin.x + header.size.width - px(200.),
        "not at the far edge: {count:?} {header:?}"
    );

    let (group, group_count) = first_group(cx);
    assert!(
        group_count.origin.x >= group.origin.x + group.size.width - px(1.),
        "the group's count follows its name: {group_count:?} {group:?}"
    );
    assert!(
        group_count.origin.x - (group.origin.x + group.size.width) < px(16.),
        "right next to it: {group_count:?} {group:?}"
    );
}

/// "Atlas Launch" → heading "Risks" → a block that links to "Target", so the reference has a
/// path above it.
fn setup_nested_refs(
    cx: &mut TestAppContext,
) -> (
    &mut VisualTestContext,
    Entity<Workspace>,
    PageId,
    Vec<utopia_core::BlockId>,
) {
    use utopia_core::{BlockKind, Command};
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    let atlas = store.create_page("Atlas Launch").unwrap();
    let risks = utopia_core::BlockId::new();
    let risky = utopia_core::BlockId::new();
    let mut cmds = vec![
        Command::InsertBlock {
            id: risks.clone(),
            parent: None,
            after: None,
            text: "Risks".into(),
            kind: BlockKind::Heading(2),
        },
        Command::InsertBlock {
            id: risky.clone(),
            parent: Some(risks.clone()),
            after: None,
            text: "The beta date depends on [[Target]]".into(),
            kind: BlockKind::Bullet,
        },
    ];
    // Four children under the referencing block, for the inline-children limit (LR-4).
    let mut after = None;
    for n in 1..=4 {
        let id = utopia_core::BlockId::new();
        cmds.push(Command::InsertBlock {
            id: id.clone(),
            parent: Some(risky.clone()),
            after: after.clone(),
            text: format!("Reason {n}"),
            kind: BlockKind::Bullet,
        });
        after = Some(id);
    }
    store.execute(&atlas, cmds, Default::default()).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    (cx, ws, target, vec![risks, risky])
}

/// LR-3: a referencing block nested under "Risks" shows "Atlas Launch › Risks" as one caption
/// line above it, and each part is its own click target.
#[gpui::test]
fn reference_shows_the_full_path_above_the_block(cx: &mut TestAppContext) {
    let (cx, _, _, _) = setup_nested_refs(cx);
    let hit = first_hit(cx);
    cx.run_until_parked();
    let (page, ancestor) = (0..40)
        .find_map(|ix| {
            let page: &'static str = Box::leak(format!("hit-path-part-{ix}-0").into_boxed_str());
            let ancestor: &'static str =
                Box::leak(format!("hit-path-part-{ix}-1").into_boxed_str());
            Some((cx.debug_bounds(page)?, cx.debug_bounds(ancestor)?))
        })
        .expect("a path above the reference");
    let path = cx
        .debug_bounds("hit-path-7")
        .or_else(|| {
            (0..40).find_map(|ix| {
                let sel: &'static str = Box::leak(format!("hit-path-{ix}").into_boxed_str());
                cx.debug_bounds(sel)
            })
        })
        .expect("a path line was rendered");
    assert!(
        path.origin.y + path.size.height <= hit.origin.y + px(0.5),
        "the path sits above the block: {path:?} {hit:?}"
    );
    assert!(
        path.size.height <= px(20.),
        "one caption line, not a stacked crumb list: {path:?}"
    );
    assert!(
        ancestor.origin.x >= page.origin.x + page.size.width - px(1.),
        "the ancestor follows the page name: {ancestor:?} {page:?}"
    );
    assert!(
        approx(page.origin.y, f32::from(ancestor.origin.y)),
        "both parts on the same line: {page:?} {ancestor:?}"
    );
    let (left, right) = column(cx);
    assert!(
        f32::from(page.origin.x) >= left
            && f32::from(ancestor.origin.x + ancestor.size.width) <= right + 0.5,
        "the path stays inside the column: {path:?} ({left}, {right})"
    );
}

/// LR-4: a reference shows the first two children inline and folds the rest behind a
/// "+N more" chip, which opens them. The numbers come from the index, not the paint.
#[gpui::test]
fn reference_children_are_inline_up_to_the_limit(cx: &mut TestAppContext) {
    let (cx, _, _, _) = setup_nested_refs(cx);
    let ix = first_hit_ix(cx);
    let first: &'static str = Box::leak(format!("hit-child-{ix}-0").into_boxed_str());
    let second: &'static str = Box::leak(format!("hit-child-{ix}-1").into_boxed_str());
    let third: &'static str = Box::leak(format!("hit-child-{ix}-2").into_boxed_str());
    let hit = first_hit(cx);
    let first = bounds(cx, first);
    let second = bounds(cx, second);
    assert!(
        first.origin.y >= hit.origin.y + hit.size.height - px(0.5),
        "children sit under their block: {first:?} {hit:?}"
    );
    assert!(
        second.origin.y > first.origin.y,
        "and stack in outline order: {first:?} {second:?}"
    );
    assert!(
        first.origin.x >= hit.origin.x - px(0.5) && first.origin.x <= hit.origin.x + px(20.),
        "children line up under the block's text, not its bullet: {first:?} {hit:?}"
    );
    assert!(
        cx.debug_bounds(third).is_none(),
        "only two are shown by default"
    );
    let more: &'static str = Box::leak(format!("hit-more-{ix}-2").into_boxed_str());
    let chip = bounds(cx, more);
    assert!(
        chip.origin.y >= second.origin.y + second.size.height - px(0.5),
        "the chip follows the shown children: {chip:?} {second:?}"
    );
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let third = bounds(cx, third);
    assert!(
        third.origin.y >= second.origin.y + second.size.height - px(0.5),
        "the chip opened the rest, in order: {third:?} {second:?}"
    );
}

/// Rule 10: opening "+N more" and changing "Children shown in references" load the
/// children before the frame; the frame itself only reads the cache.
#[gpui::test]
fn reference_children_are_prefetched_outside_the_frame(cx: &mut TestAppContext) {
    let (cx, ws, _, _) = setup_nested_refs(cx);
    let ix = first_hit_ix(cx);
    let view = ws.read_with(cx, |w, _| w.main_view());
    let frame_before = view.read_with(cx, |v, _| v.ref_children_frame_queries());
    let total_before = view.read_with(cx, |v, _| v.ref_children_queries());

    let more: &'static str = Box::leak(format!("hit-more-{ix}-2").into_boxed_str());
    let chip = bounds(cx, more);
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let (total, frame) = view.read_with(cx, |v, _| {
        (v.ref_children_queries(), v.ref_children_frame_queries())
    });
    assert!(
        total > total_before,
        "opening the chip reads the folded children once"
    );
    assert_eq!(
        frame, frame_before,
        "the read happens in the handler, not in the frame"
    );

    // Changing the preference invalidates the cache; that read is prefetched too.
    let dir = tempfile::tempdir().unwrap();
    cx.update(|_, app| {
        utopia_ui::appearance::set_ref_children(app, 3, dir.path()).unwrap();
    });
    cx.run_until_parked();
    let (total_after, frame_after) = view.read_with(cx, |v, _| {
        (v.ref_children_queries(), v.ref_children_frame_queries())
    });
    assert!(total_after > total, "the new limit reads the children once");
    assert_eq!(
        frame_after, frame_before,
        "the preference change does not read in a frame"
    );
}

/// LR-1: clicking a co-reference chip narrows the references and the choice becomes a filled
/// chip on the same line; clicking it again widens them back.
#[gpui::test]
fn selecting_a_co_reference_chip_filters_and_fills(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_controls(cx);
    assert!(
        cx.debug_bounds("filter-chip-0").is_none(),
        "nothing is selected yet"
    );
    let chip = bounds(cx, "suggest-chip-0");
    let chips = bounds(cx, "refs-chips");
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let selected = bounds(cx, "filter-chip-0");
    assert!(
        selected.origin.y >= chips.origin.y - px(0.5),
        "the selected chip joins the chips line: {selected:?} {chips:?}"
    );
    // One of the two references also mentions Ana, so the list narrowed to one.
    bounds(cx, "refs-count-1");
    let selected = bounds(cx, "filter-chip-0");
    cx.simulate_click(selected.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-count-2");
}

/// LR-1 review fix: the reference controls also render in the 380 px right sidebar. The
/// filter field keeps 240 px as its preferred width but must give way instead of pushing
/// the menu buttons past the pane.
#[gpui::test]
fn reference_controls_fit_the_sidebar_pane(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_ref_controls(cx);
    ws.update_in(cx, |w, window, cx| {
        w.open(target, None, None, true, window, cx)
    });
    let side = bounds(cx, "right-sidebar");
    let row = bounds(cx, "refs-control-row");
    assert!(
        row.origin.x >= side.origin.x && row.origin.x + row.size.width <= side.right() + px(0.5),
        "the control row stays inside the sidebar: {row:?} {side:?}"
    );
    let sort = bounds(cx, "refs-sort-menu-title");
    assert!(
        sort.origin.x + sort.size.width <= side.right() + px(0.5),
        "the sort menu is still reachable: {sort:?} {side:?}"
    );
    assert!(
        approx(
            row.size.height,
            f32::from(bounds(cx, "refs-date-menu-any-time").size.height)
        ),
        "the controls stay on one line instead of wrapping: {row:?}"
    );
}

/// LR-1 review fix: a split pane can be narrower than the sidebar. The row must fit there
/// too, with all three controls still on it.
#[gpui::test]
fn reference_controls_fit_a_narrow_split_pane(cx: &mut TestAppContext) {
    let (cx, ws, target) = setup_ref_controls(cx);
    cx.simulate_resize(size(px(900.), px(700.)));
    ws.update_in(cx, |w, window, cx| w.open_split(target, window, cx));
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
    let split = bounds(cx, "split-0");
    let row = bounds(cx, "refs-control-row");
    assert!(
        row.origin.x >= split.origin.x && row.origin.x + row.size.width <= split.right() + px(0.5),
        "the control row stays inside the split: {row:?} {split:?}"
    );
    let filter = bounds(cx, "refs-filter");
    assert!(
        filter.size.width < px(240.),
        "the field shrank to make room: {filter:?} {split:?}"
    );
    let sort = bounds(cx, "refs-sort-menu-title");
    assert!(
        sort.origin.x + sort.size.width <= split.right() + px(0.5),
        "{sort:?} {split:?}"
    );
}

/// A graph where one hit mentions "Ana Lovelace" twice and another once: the co-reference
/// chip must count the two hits it selects, not the three occurrences of the link (LR-1).
fn setup_ref_token_counts(
    cx: &mut TestAppContext,
) -> (&mut VisualTestContext, Entity<Workspace>, PageId) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
    let target = store.create_page("Target").unwrap();
    store.create_page("Ana Lovelace").unwrap();
    let src = store.create_page("Source").unwrap();
    let b = store.ensure_first_block(&src).unwrap();
    store
        .set_block_text(
            &src,
            &b,
            "[[Target]] with [[Ana Lovelace]] and [[Ana Lovelace]] again",
            None,
            None,
        )
        .unwrap();
    store.commit_block(&src, &b).unwrap();
    let plan = utopia_core::outline::append_block(store.page_state(&src).unwrap(), None);
    let b2 = store.execute_plan(&src, plan, None).unwrap().unwrap().block;
    store
        .set_block_text(
            &src,
            &b2,
            "another [[Target]] with [[Ana Lovelace]]",
            None,
            None,
        )
        .unwrap();
    store.commit_block(&src, &b2).unwrap();
    store.sync_index().unwrap();
    let app = cx.new(|_| AppState::new(store));
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
    cx.simulate_resize(size(px(W), px(H)));
    ws.update_in(cx, |w, window, cx| w.open_launch_page(window, cx));
    cx.run_until_parked();
    cx.simulate_keystrokes("escape");
    ws.update_in(cx, |w, window, cx| {
        w.open(target.clone(), None, None, false, window, cx)
    });
    (cx, ws, target)
}

/// LR-1 review fix: a co-reference chip counts the distinct hits it would leave, exactly
/// like the filter it applies, instead of every occurrence of the link in the text.
#[gpui::test]
fn co_reference_counts_match_the_hits_they_select(cx: &mut TestAppContext) {
    let (cx, _, _) = setup_ref_token_counts(cx);
    // Two hits mention Ana; one of them mentions her twice. The chip says 2, not 3.
    bounds(cx, "suggest-count-0-2");
    let chip = bounds(cx, "suggest-chip-0");
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    bounds(cx, "refs-count-2");
}

/// PG-1 review fix: a root code block that happens to contain `::` (C++'s `std::cout`) is a
/// code block, not page metadata. The projector never indexes code as an attribute, so the
/// outline and the metadata header must not either.
/// GR-3: the legend and the count sit at the bottom left, the zoom controls at the bottom
/// right, and the top bar no longer spends a row on the count.
#[gpui::test]
fn graph_legend_and_zoom_controls_sit_at_the_edges(cx: &mut TestAppContext) {
    let cx = setup(cx);
    cx.simulate_keystrokes("escape cmd-shift-g");
    cx.run_until_parked();
    let canvas = bounds(cx, "graph-canvas");
    let legend = bounds(cx, "graph-legend");
    let zoom_in = bounds(cx, "graph-zoom-in");
    let zoom_out = bounds(cx, "graph-zoom-out");
    let fit = bounds(cx, "graph-zoom-fit");
    let right = canvas.origin.x + canvas.size.width;
    let bottom = canvas.origin.y + canvas.size.height;
    assert!(
        legend.origin.x > canvas.origin.x && legend.origin.x < canvas.center().x,
        "legend on the left: {legend:?}"
    );
    assert!(
        legend.origin.y + legend.size.height <= bottom + px(1.)
            && legend.origin.y > canvas.center().y,
        "legend at the bottom: {legend:?}"
    );
    assert!(
        zoom_out.origin.x > canvas.center().x && fit.origin.x + fit.size.width <= right + px(1.),
        "zoom controls on the right: {zoom_out:?} {fit:?}"
    );
    assert!(
        zoom_in.origin.y + zoom_in.size.height <= bottom + px(1.),
        "zoom controls at the bottom: {zoom_in:?}"
    );

    // The controls change the scale, and Fit returns to the layout's frame.
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let graph = ws.update(cx, |w, _| w.graph_view()).expect("graph shown");
    let at = zoom_in.center();
    cx.simulate_click(at, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(graph.update(cx, |g, _| g.zoom()) > 1.);
    let at = fit.center();
    cx.simulate_click(at, gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(graph.update(cx, |g, _| g.zoom()), 1.);
}

/// GR-3 review: an index refresh keeps the panel's pan and zoom; only a new origin (or the
/// Fit control) resets the viewport. Editing prose next to the panel must not throw the
/// user's graph view away.
#[gpui::test]
fn graph_panel_keeps_its_viewport_across_an_index_refresh(cx: &mut TestAppContext) {
    use utopia_ui::app_state::IndexUpdated;
    let cx = setup_alias_page(cx);
    cx.simulate_keystrokes("escape cmd-shift-g");
    cx.run_until_parked();
    let local = bounds(cx, "graph-local").center();
    cx.simulate_click(local, gpui::Modifiers::default());
    cx.run_until_parked();
    let ws = cx
        .update(|window, _| window.root::<Workspace>())
        .flatten()
        .expect("workspace root");
    let panel = ws.update(cx, |w, _| w.graph_panel()).expect("panel open");
    let at = bounds(cx, "graph-zoom-in").center();
    cx.simulate_click(at, gpui::Modifiers::default());
    cx.run_until_parked();
    let zoomed = panel.update(cx, |g, _| g.zoom());
    assert!(zoomed > 1., "the control zoomed the panel");

    // What a debounced commit does once the index catches up.
    let app = ws.read_with(cx, |w, _| w.app().clone());
    app.update(cx, |app, cx| {
        app.refs_version += 1;
        cx.emit(IndexUpdated);
    });
    cx.run_until_parked();
    assert_eq!(
        panel.update(cx, |g, _| g.zoom()),
        zoomed,
        "a refresh keeps the viewport"
    );
}
