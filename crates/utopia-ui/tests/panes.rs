//! Stacked panes and the view mode toggle (DESIGN.md §4.3, §4.4; PRD §9.2.1 SP-1…SP-9),
//! driven through the real `Workspace` with simulated clicks and keys. The click rules
//! themselves are unit-tested without a window in `src/pane_stack.rs`; these scripts prove
//! the workspace routes real clicks into them and draws the result.

use chrono::NaiveDate;
use gpui::{
    point, px, size, AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext,
};
use utopia_core::{BlockId, BlockKind, PageId};
use utopia_store::Store;
use utopia_ui::actions::install_keymap;
use utopia_ui::app_state::AppState;
use utopia_ui::pane_stack::{Entry, ViewMode};
use utopia_ui::rich::LinkTarget;
use utopia_ui::theme::{Theme, PANE_SPINE, PANE_WIDTH};
use utopia_ui::workspace::Workspace;

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
}

const NAMES: [&str; 9] = [
    "Home", "Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta", "Theta",
];

/// Nine pages, each linking to the next ("[[Alpha]] comes next"), the last back to Home.
/// Seeded through the store, never typed, so the scripts cannot race the UI.
fn seed_chain(store: &mut Store) -> (Vec<PageId>, Vec<BlockId>) {
    let mut pages = Vec::new();
    let mut blocks = Vec::new();
    for (i, n) in NAMES.iter().enumerate() {
        let p = store.create_page(n).unwrap();
        let b = store.ensure_first_block(&p).unwrap();
        let next = NAMES.get(i + 1).copied().unwrap_or("Home");
        store
            .set_block_text(&p, &b, &format!("[[{next}]] comes next"), None, None)
            .unwrap();
        store.commit_block(&p, &b).unwrap();
        pages.push(p);
        blocks.push(b);
    }
    store.sync_index().unwrap();
    (pages, blocks)
}

struct Chain {
    pages: Vec<PageId>,
    blocks: Vec<BlockId>,
}

fn new_app(cx: &mut TestAppContext) -> (Entity<AppState>, Chain) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let mut store = Store::open_in_memory().unwrap();
    store.set_today(Some(today()));
    let (pages, blocks) = seed_chain(&mut store);
    (cx.new(|_| AppState::new(store)), Chain { pages, blocks })
}

fn open_window<'a>(
    app: &Entity<AppState>,
    cx: &'a mut TestAppContext,
    mode: ViewMode,
) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(|cx| utopia_ui::appearance::set_view_mode(cx, mode).unwrap());
    let app2 = app.clone();
    let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    cx.simulate_resize(size(px(1600.), px(900.)));
    ws.update_in(cx, |w, window, cx| {
        w.set_scroll_animation(false);
        w.open_launch_page(window, cx);
    });
    cx.run_until_parked();
    (ws, cx)
}

fn setup(
    cx: &mut TestAppContext,
    mode: ViewMode,
) -> (
    Entity<AppState>,
    Entity<Workspace>,
    &mut VisualTestContext,
    Chain,
) {
    let (app, chain) = new_app(cx);
    let (ws, cx) = open_window(&app, cx, mode);
    // Out of the feed's first block, so no edit is in progress (⌥←/⌥→, ⌘W act).
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    (app, ws, cx, chain)
}

fn sel(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn cmd() -> Modifiers {
    Modifiers {
        platform: true,
        ..Default::default()
    }
}

/// Click the link at the start of `block`'s text (each seeded block starts with one).
fn click_link(cx: &mut VisualTestContext, block: &BlockId, m: Modifiers) {
    let at = cx
        .debug_bounds(sel(format!("text-{}", block.0)))
        .unwrap_or_else(|| panic!("block {} is drawn", block.0));
    cx.simulate_click(point(at.origin.x + px(8.), at.center().y), m);
    cx.run_until_parked();
}

/// The stack after the feed, as page indexes into `NAMES`.
fn tail(ws: &Entity<Workspace>, chain: &Chain, cx: &mut VisualTestContext) -> Vec<usize> {
    ws.update(cx, |w, cx| w.stack_entries(cx))
        .into_iter()
        .skip(1)
        .map(|e| match e {
            Entry::Page { page, .. } => chain.pages.iter().position(|p| *p == page).unwrap(),
            Entry::Daily => panic!("Daily Notes is only ever first"),
        })
        .collect()
}

fn open(ws: &Entity<Workspace>, page: &PageId, cx: &mut VisualTestContext) {
    let page = page.clone();
    ws.update_in(cx, |w, window, cx| {
        w.open(page, None, None, false, window, cx)
    });
    cx.run_until_parked();
}

// ---------------------------------------------------------------- Panes mode

/// SP-2: a plain click in pane i keeps 0…i, drops the rest and adds the target at i + 1.
#[gpui::test]
fn sp_2_plain_click_drops_the_right_and_adds_the_next_pane(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    open(&ws, &c.pages[0], cx);
    click_link(cx, &c.blocks[0], Modifiers::default()); // Home → Alpha
    click_link(cx, &c.blocks[1], Modifiers::default()); // Alpha → Beta
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 2]);
    // From Home again (pane 1): Alpha and Beta go, Alpha comes back as pane 2.
    open(&ws, &c.pages[3], cx);
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 2, 3]);
    click_link(cx, &c.blocks[2], Modifiers::default()); // in Beta (pane 3) → Gamma: open
    assert_eq!(
        tail(&ws, &c, cx),
        vec![0, 1, 2, 3],
        "Gamma is open: nothing added"
    );
    assert_eq!(
        ws.update(cx, |w, _| w.focused_pane()),
        4,
        "it is scrolled to"
    );
    click_link(cx, &c.blocks[1], Modifiers::default()); // Alpha (pane 2) → Beta: open
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 2, 3]);
    assert_eq!(ws.update(cx, |w, _| w.focused_pane()), 3);
}

/// SP-3: ⌘-click replaces the pane in place; Daily Notes is never replaced.
#[gpui::test]
fn sp_3_cmd_click_replaces_the_pane_in_place(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    open(&ws, &c.pages[0], cx);
    click_link(cx, &c.blocks[0], Modifiers::default()); // Home → Alpha
    click_link(cx, &c.blocks[1], Modifiers::default()); // Alpha → Beta
    click_link(cx, &c.blocks[1], cmd()); // ⌘ in Alpha (pane 2): Beta is open, scroll only
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 2]);
    click_link(cx, &c.blocks[2], cmd()); // ⌘ in Beta (pane 3) → Gamma replaces Beta
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 3]);
    // Pane 0 is the feed: a ⌘-click there acts as a plain click.
    let entries = ws.update(cx, |w, cx| w.stack_entries(cx));
    assert_eq!(entries[0], Entry::Daily, "the feed stays first");
}

/// SP-4: the × closes only its pane; ⌘W closes the focused one; Daily Notes has no ×.
#[gpui::test]
fn sp_4_close_button_and_cmd_w(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    for p in &c.pages[..3] {
        open(&ws, p, cx);
    }
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 2]);
    let close = cx
        .debug_bounds("stack-close-2")
        .expect("pane 2 has a close button");
    cx.simulate_click(close.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        tail(&ws, &c, cx),
        vec![0, 2],
        "only Alpha went; Beta slid left"
    );
    ws.update_in(cx, |w, window, cx| w.focus_pane(1, window, cx));
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    assert_eq!(tail(&ws, &c, cx), vec![2]);
    ws.update_in(cx, |w, window, cx| w.focus_pane(0, window, cx));
    cx.simulate_keystrokes("cmd-w");
    cx.run_until_parked();
    assert_eq!(tail(&ws, &c, cx), vec![2], "⌘W never closes Daily Notes");
}

/// SP-4/SP-5: Back closes the last pane; "Close other panes" keeps the feed and the
/// focused pane; a trail title scrolls to its pane without changing the list.
#[gpui::test]
fn sp_5_back_trail_and_close_others(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    for p in &c.pages[..4] {
        open(&ws, p, cx);
    }
    let back = cx.debug_bounds("back").unwrap();
    cx.simulate_click(back.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 2]);
    let trail = cx.debug_bounds("trail-1").expect("the trail names pane 1");
    cx.simulate_click(trail.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        tail(&ws, &c, cx),
        vec![0, 1, 2],
        "Panes: the trail only scrolls"
    );
    assert_eq!(ws.update(cx, |w, _| w.focused_pane()), 1);
    ws.update_in(cx, |w, window, cx| w.focus_pane(2, window, cx));
    let others = cx.debug_bounds("close-others").unwrap();
    cx.simulate_click(others.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(tail(&ws, &c, cx), vec![1]);
}

/// §4.3: in pane i, the link to the page open in pane i + 1 is ringed; Single mode shows
/// one page, so it rings nothing.
#[gpui::test]
fn sp_2_the_link_to_the_next_pane_is_ringed(cx: &mut TestAppContext) {
    let (app, ws, cx, c) = setup(cx, ViewMode::Panes);
    open(&ws, &c.pages[0], cx);
    click_link(cx, &c.blocks[0], Modifiers::default()); // Home → Alpha
    let home = ws.update(cx, |w, _| w.stack_view(1)).unwrap();
    let next = home
        .read_with(cx, |v, _| v.next_link().cloned())
        .expect("Home knows");
    assert!(next.matches(&LinkTarget::Page("Alpha".into())));
    assert!(
        next.matches(&LinkTarget::Page("  alpha ".into())),
        "titles match as keys do"
    );
    assert!(!next.matches(&LinkTarget::Page("Beta".into())));
    // An alias names the page too (§10.3.1).
    app.update(cx, |app, cx| {
        app.store.add_alias(&c.pages[1], "First Letter").unwrap();
        app.store.sync_index().unwrap();
        app.changed(vec![c.pages[1].clone()], true, cx);
        cx.emit(utopia_ui::app_state::IndexUpdated);
    });
    cx.run_until_parked();
    let next = home.read_with(cx, |v, _| v.next_link().cloned()).unwrap();
    assert!(
        next.matches(&LinkTarget::Page("First Letter".into())),
        "{next:?}"
    );
    let last = ws
        .update(cx, |w, cx| w.stack_view(w.stack_entries(cx).len() - 1))
        .unwrap();
    assert!(last.read_with(cx, |v, _| v.next_link().is_none()));
    ws.update_in(cx, |w, window, cx| w.set_mode(ViewMode::Single, window, cx));
    cx.run_until_parked();
    assert!(home.read_with(cx, |v, _| v.next_link().is_none()));
}

/// SP-9: ⌥→/⌥← move focus between panes and bring the pane into view; while a block is
/// being edited they move the caret by word instead.
#[gpui::test]
fn sp_9_alt_arrows_move_focus_between_panes(cx: &mut TestAppContext) {
    let (app, ws, cx, c) = setup(cx, ViewMode::Panes);
    for p in &c.pages[..5] {
        open(&ws, p, cx);
    }
    assert_eq!(ws.update(cx, |w, _| w.focused_pane()), 5);
    cx.simulate_keystrokes("alt-left alt-left");
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, _| w.focused_pane()), 3);
    cx.debug_bounds("stack-focus-3")
        .expect("the focused pane shows its ring");
    let (scroll, _) = ws.update(cx, |w, _| w.pane_scroll());
    let pane = cx.debug_bounds("stack-pane-3").unwrap();
    let next = cx.debug_bounds("stack-pane-4").unwrap();
    assert!(
        next.origin.x >= pane.origin.x + px(PANE_WIDTH - 1.),
        "pane 3 is fully in view at scroll {scroll}: {pane:?} {next:?}"
    );
    cx.simulate_keystrokes("alt-right");
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, _| w.focused_pane()), 4);
    // Editing: the same keys are the editor's (word left), focus stays.
    click_link(cx, &c.blocks[3], Modifiers::default()); // Gamma → Delta: open, scroll
    let text = cx
        .debug_bounds(sel(format!("text-{}", c.blocks[3].0)))
        .unwrap();
    cx.simulate_click(
        point(text.right() - px(4.), text.center().y),
        Modifiers::default(),
    );
    cx.run_until_parked();
    let editing = ws.update(cx, |w, cx| w.editor().read(cx).target().is_some());
    assert!(editing, "the click put the caret in the block");
    let before = ws.update(cx, |w, _| w.focused_pane());
    cx.simulate_keystrokes("alt-left");
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, _| w.focused_pane()), before);
    let _ = app;
}

/// AGENTS.md: editing, to-dos and collapsing work inside a pane like on a page.
#[gpui::test]
fn editing_works_inside_a_pane(cx: &mut TestAppContext) {
    let (app, ws, cx, c) = setup(cx, ViewMode::Panes);
    open(&ws, &c.pages[0], cx);
    click_link(cx, &c.blocks[0], Modifiers::default()); // Home → Alpha
    let text = cx
        .debug_bounds(sel(format!("text-{}", c.blocks[1].0)))
        .unwrap();
    cx.simulate_click(
        point(text.right() - px(2.), text.center().y),
        Modifiers::default(),
    );
    cx.run_until_parked();
    cx.simulate_keystrokes("end");
    cx.simulate_input("!");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("child");
    cx.simulate_keystrokes("tab alt-enter");
    cx.run_until_parked();
    cx.simulate_keystrokes("up cmd-.");
    cx.run_until_parked();
    let (texts, child_kind, folded) = app.update(cx, |app, _| {
        let st = app.store.page_state(&c.pages[1]).unwrap();
        let rows: Vec<String> = st
            .live_preorder()
            .into_iter()
            .map(|(id, _)| st.block(&id).unwrap().text.clone())
            .collect();
        let child = st.live_preorder()[1].0.clone();
        let kind = st.block(&child).unwrap().kind;
        (rows, kind, app.store.is_collapsed(&c.blocks[1]))
    });
    assert_eq!(texts, vec!["[[Beta]] comes next!", "child"]);
    assert_eq!(
        child_kind,
        BlockKind::Todo,
        "Alt+Enter made a to-do in the pane"
    );
    assert!(folded, "Cmd+. folded the parent in the pane");
    let _ = ws;
}

/// §4.3: when a pane is nearly covered its strip becomes a spine; clicking the spine
/// scrolls back to `index × (560 − 40)`. A deep stack in a small window still shows its
/// newest pane whole.
#[gpui::test]
fn spines_and_a_deep_stack_in_a_narrow_window(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    cx.simulate_resize(size(px(960.), px(800.)));
    for p in &c.pages {
        open(&ws, p, cx);
    }
    assert_eq!(tail(&ws, &c, cx).len(), 9, "Daily Notes plus nine pages");
    let scroller = cx.debug_bounds("pane-scroller").unwrap();
    let last = cx.debug_bounds("stack-pane-9").unwrap();
    assert!(
        last.right() <= scroller.right() + px(0.5),
        "the newest pane is whole: {last:?} {scroller:?}"
    );
    assert_eq!(last.size.width, px(PANE_WIDTH));
    let spine = cx.debug_bounds("stack-spine-8").expect("pane 8 is a spine");
    assert!(spine.size.width >= px(PANE_SPINE) - px(0.5));
    // Nine spines and a pane need 880 px; the scroller has 696, so the oldest spines fold
    // under one another at the left edge and the newest stay a full 40 px.
    assert_eq!(
        cx.debug_bounds("stack-pane-1").unwrap().origin.x,
        scroller.origin.x,
        "the oldest pane is folded at the edge, not pushed off it"
    );
    // Click the spine of pane 7: one press, one target, though five spines lie under it.
    let s7 = cx.debug_bounds("stack-spine-7").expect("pane 7 is a spine");
    cx.simulate_click(s7.center(), Modifiers::default());
    cx.run_until_parked();
    let (scroll, _) = ws.update(cx, |w, _| w.pane_scroll());
    // Pane 7 sticks at 7 × 40 less the 224 px fold (9 × 40 + 560 − 696): 56.
    assert_eq!(scroll, 7. * PANE_WIDTH - 56.);
    assert_eq!(ws.update(cx, |w, _| w.focused_pane()), 7);
    let p7 = cx.debug_bounds("stack-pane-7").unwrap();
    assert_eq!(p7.origin.x - scroller.origin.x, px(56.));
    assert!(
        p7.right() <= scroller.right() + px(0.5),
        "pane 7 is whole: {p7:?}"
    );
    let p8 = cx.debug_bounds("stack-pane-8").unwrap();
    assert_eq!(p8.origin.x, p7.right(), "pane 8 follows, uncovering pane 7");
}

/// A press reaches the top pane only: gpui hands a click to every hitbox under the
/// pointer, so a covered pane must not answer it (found by the spine script above).
#[gpui::test]
fn a_click_reaches_only_the_top_pane(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    for p in &c.pages[..2] {
        open(&ws, p, cx);
    }
    let scroller = cx.debug_bounds("pane-scroller").unwrap();
    // Scroll so pane 1 slides half over pane 0: pane 0's blocks are under pane 1 there.
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroller.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(10_000.), px(0.))),
        ..Default::default()
    });
    cx.run_until_parked();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroller.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(-300.), px(0.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let p1 = cx.debug_bounds("stack-pane-1").unwrap();
    cx.simulate_click(
        point(p1.origin.x + px(20.), p1.center().y),
        Modifiers::default(),
    );
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, _| w.focused_pane()),
        1,
        "pane 1 took the press"
    );
}

/// §4.3: the sideways wheel scrolls the row; panes stick at `index × 40`.
#[gpui::test]
fn the_wheel_scrolls_the_row_and_panes_stick(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    for p in &c.pages[..3] {
        open(&ws, p, cx);
    }
    let (end, _) = ws.update(cx, |w, _| w.pane_scroll());
    assert!(end > 0., "four panes overflow 1600 px less the sidebar");
    let scroller = cx.debug_bounds("pane-scroller").unwrap();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroller.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(10_000.), px(0.))),
        ..Default::default()
    });
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |w, _| w.pane_scroll()).0,
        0.,
        "back at the start"
    );
    let p0 = cx.debug_bounds("stack-pane-0").unwrap();
    let p1 = cx.debug_bounds("stack-pane-1").unwrap();
    assert_eq!(
        p1.origin.x - p0.origin.x,
        px(PANE_WIDTH),
        "side by side at 0"
    );
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: scroller.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(-600.), px(0.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let p1 = cx.debug_bounds("stack-pane-1").unwrap();
    assert_eq!(
        p1.origin.x - scroller.origin.x,
        px(PANE_SPINE),
        "pane 1 sticks at 1 × 40"
    );
}

// ---------------------------------------------------------------- Single mode

/// SP-5/SP-6: Single shows the last entry full width; a plain click navigates forward or
/// cuts back to an open entry; ⌘-click switches to Panes and adds at the end; toggling
/// keeps the list.
#[gpui::test]
fn sp_6_single_mode_rules_and_the_toggle(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Single);
    open(&ws, &c.pages[0], cx);
    cx.debug_bounds("single-page")
        .expect("Single draws one page");
    click_link(cx, &c.blocks[0], Modifiers::default()); // Home → Alpha
    click_link(cx, &c.blocks[1], Modifiers::default()); // Alpha → Beta
    assert_eq!(tail(&ws, &c, cx), vec![0, 1, 2]);
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(c.pages[2].clone())
    );
    // Beta's link goes to Gamma; Gamma's to Delta; Delta's... make a loop back: open Home.
    open(&ws, &c.pages[0], cx);
    assert_eq!(tail(&ws, &c, cx), vec![0], "open already: cut back to it");
    // Back removes the last entry.
    let back = cx.debug_bounds("back").unwrap();
    cx.simulate_click(back.center(), Modifiers::default());
    cx.run_until_parked();
    assert!(tail(&ws, &c, cx).is_empty());
    open(&ws, &c.pages[0], cx);
    click_link(cx, &c.blocks[0], cmd()); // ⌘ in Single: peek Alpha as a pane
    assert_eq!(ws.update(cx, |w, _| w.view_mode()), ViewMode::Panes);
    assert_eq!(tail(&ws, &c, cx), vec![0, 1]);
    assert_eq!(
        ws.update(cx, |_, cx| utopia_ui::appearance::view_mode(cx)),
        ViewMode::Single,
        "a peek does not change the setting"
    );
    cx.debug_bounds("close-others")
        .expect("Panes has Close other panes");
    // The toggle: the list stays, the setting follows.
    let single = cx.debug_bounds("mode-single").expect("the Single side");
    cx.simulate_click(single.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(ws.update(cx, |w, _| w.view_mode()), ViewMode::Single);
    assert_eq!(
        tail(&ws, &c, cx),
        vec![0, 1],
        "switching modes keeps the list"
    );
    assert_eq!(
        ws.update(cx, |w, cx| w.main_page(cx)),
        Some(c.pages[1].clone())
    );
    let panes = cx.debug_bounds("mode-panes").unwrap();
    cx.simulate_click(panes.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        ws.update(cx, |_, cx| utopia_ui::appearance::view_mode(cx)),
        ViewMode::Panes
    );
    // Trail in Single cuts back.
    ws.update_in(cx, |w, window, cx| w.set_mode(ViewMode::Single, window, cx));
    cx.run_until_parked();
    let t1 = cx.debug_bounds("trail-1").unwrap();
    cx.simulate_click(t1.center(), Modifiers::default());
    cx.run_until_parked();
    assert_eq!(tail(&ws, &c, cx), vec![0]);
}

/// SP-7: "Daily Notes" in the sidebar resets the stack; other sidebar items open at the end.
#[gpui::test]
fn sp_7_daily_notes_resets_the_stack(cx: &mut TestAppContext) {
    let (_app, ws, cx, c) = setup(cx, ViewMode::Panes);
    for p in &c.pages[..3] {
        open(&ws, p, cx);
    }
    cx.simulate_keystrokes("cmd-shift-d");
    cx.run_until_parked();
    assert!(tail(&ws, &c, cx).is_empty());
    assert!(ws.update(cx, |w, cx| w.main_view().read(cx).is_journal()));
}

/// SP-8: the stack and the mode come back after a relaunch; a page deleted meanwhile
/// is left out.
#[gpui::test]
fn sp_8_the_stack_is_restored_on_relaunch(cx: &mut TestAppContext) {
    let (app, chain) = new_app(cx);
    {
        let (ws, vcx) = open_window(&app, cx, ViewMode::Panes);
        for p in &chain.pages[..4] {
            open(&ws, p, vcx);
        }
        assert_eq!(tail(&ws, &chain, vcx), vec![0, 1, 2, 3]);
    }
    app.update(cx, |app, cx| {
        app.store.delete_page(&chain.pages[2]).unwrap();
        app.changed(Vec::new(), true, cx);
    });
    // A second window over the same graph reads what the first saved.
    let app2 = app.clone();
    let (ws, vcx) = cx.add_window_view(|window, cx| Workspace::new(app2, window, cx));
    vcx.simulate_resize(size(px(1600.), px(900.)));
    ws.update_in(vcx, |w, window, cx| {
        w.set_scroll_animation(false);
        w.open_launch_page(window, cx);
    });
    vcx.run_until_parked();
    assert_eq!(tail(&ws, &chain, vcx), vec![0, 1, 3]);
    assert_eq!(ws.update(vcx, |w, _| w.view_mode()), ViewMode::Panes);
    let entries = ws.update(vcx, |w, cx| w.stack_entries(cx));
    assert_eq!(entries[0], Entry::Daily);
}
