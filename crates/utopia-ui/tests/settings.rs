//! The Settings keybinding editor (§9.1), driven through GPUI's headless test platform.

use gpui::{px, size, TestAppContext};
use utopia_ui::actions::{active_keys, doc, install_keymap, platform_keys, Overrides, Quit};
use utopia_ui::settings::{SettingsView, Tab};
use utopia_ui::theme::Theme;

fn keymap_json(dir: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(dir.join("keymap.json")).ok()
}

/// Rebinding writes `keymap.json`, and only for the action that changed.
#[gpui::test]
fn rebinding_is_applied_and_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    assert!(keymap_json(dir.path()).is_none(), "nothing saved yet");
    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    assert_eq!(
        view.update(cx, |v, _| v.capturing().map(|d| d.action)),
        Some("GoToday")
    );

    cx.simulate_keystrokes("cmd-alt-t");
    cx.run_until_parked();

    view.update(cx, |v, _| {
        assert!(v.capturing().is_none(), "capture ends on the first key");
        assert_eq!(
            v.overrides().keys_for(today),
            "alt-cmd-t",
            "gpui canonical order"
        );
        // Untouched actions are not written down, so later default changes still reach you.
        assert_eq!(v.overrides().len(), 1);
    });
    let saved = keymap_json(dir.path()).expect("keymap.json written");
    assert!(
        saved.contains("GoToday") && saved.contains("alt-cmd-t"),
        "{saved}"
    );
    assert!(
        !saved.contains("GoYesterday"),
        "only the changed action: {saved}"
    );

    // And it survives a reload.
    let (back, warnings) = Overrides::from_json(&saved);
    assert!(warnings.is_empty());
    assert_eq!(back.keys_for(today), "alt-cmd-t");
}

/// Escape leaves the binding alone rather than binding Escape to it.
#[gpui::test]
fn escape_cancels_capture(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    view.update(cx, |v, _| {
        assert!(v.capturing().is_none());
        assert!(v.overrides().is_empty(), "nothing was rebound");
        assert_eq!(v.overrides().keys_for(today), platform_keys("cmd-shift-t"));
    });
    assert!(keymap_json(dir.path()).is_none(), "nothing to save");
}

/// The keys being recorded must not also fire their own action. Rebinding onto Cmd+Q would
/// otherwise quit the app in the middle of the edit.
#[gpui::test]
fn capturing_swallows_the_keystroke(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let quit = std::rc::Rc::new(std::cell::Cell::new(false));
    let q = quit.clone();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        cx.on_action(move |_: &Quit, _| q.set(true));
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    // Cmd+Q really is bound to Quit here: it fires when we are not capturing.
    cx.simulate_keystrokes("cmd-q");
    cx.run_until_parked();
    assert!(quit.get(), "Cmd+Q quits when nothing is being captured");
    quit.set(false);

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes("cmd-q");
    cx.run_until_parked();

    assert!(
        !quit.get(),
        "the recorded keystroke must not also run its action"
    );
    view.update(cx, |v, _| {
        assert_eq!(v.overrides().keys_for(today), "cmd-q", "Cmd+Q was recorded");
    });
}

/// Rebinding onto keys another action already uses is allowed, but said out loud.
#[gpui::test]
fn a_clash_is_reported(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes(&platform_keys("cmd-shift-y"));
    cx.run_until_parked();

    view.update(cx, |v, _| {
        let status = v.status().unwrap_or_default();
        assert!(status.contains("Yesterday"), "clash not reported: {status}");
        assert_eq!(v.overrides().conflicts().len(), 1);
    });
}

/// Reset puts an action back and removes it from the file.
#[gpui::test]
fn reset_restores_the_default(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes("cmd-alt-t");
    cx.run_until_parked();
    assert!(keymap_json(dir.path()).unwrap().contains("alt-cmd-t"));

    view.update(cx, |v, cx| v.reset_one(today, cx));
    view.update(cx, |v, _| {
        assert!(v.overrides().is_empty());
        assert_eq!(v.overrides().keys_for(today), platform_keys("cmd-shift-t"));
    });
    assert_eq!(keymap_json(dir.path()).unwrap().trim(), "[]");
}

/// The Settings body is a scroll region, not a growing column: in a short window the key
/// rows scroll inside it and the status line stays on screen.
#[gpui::test]
fn keybindings_scroll_inside_a_short_window(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("keymap.json"), "{ not json").unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(720.), px(480.)));
    cx.run_until_parked();
    let status = cx
        .debug_bounds("settings-status")
        .expect("the unreadable keymap status is shown");
    assert!(
        status.origin.y + status.size.height <= px(480.) + px(0.5),
        "the status line stays inside the window: {status:?}"
    );
}

/// ST-1: a pane is one centred 720 px column, one card per category, with the rows of a
/// card touching so the hairline between them is the only separation.
#[gpui::test]
fn keybindings_are_a_readable_column_of_cards(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    let nav = cx
        .debug_bounds("settings-card-Navigation")
        .expect("the Navigation card is drawn");
    let width = f32::from(nav.size.width);
    assert!((width - 720.).abs() < 1.5, "the column is 720 px: {nav:?}");
    // Centred in the pane beside the tab rail, not in the whole window (ST-5).
    let pane = cx.debug_bounds("settings-pane").unwrap();
    let left = f32::from(nav.origin.x) - f32::from(pane.origin.x);
    let right = f32::from(pane.size.width) - left - width;
    assert!(
        (left - right).abs() < 2.,
        "the column is centred in the pane: {left} left, {right} right"
    );
    let app = cx
        .debug_bounds("settings-card-Application")
        .expect("the Application card is drawn");
    assert!(
        (f32::from(app.origin.x) - f32::from(nav.origin.x)).abs() < 1.5
            && (f32::from(app.size.width) - width).abs() < 1.5,
        "every category card shares the column: {app:?}"
    );

    // Rows are stacked flush; the card's hairline separates them (no gap).
    let today = cx.debug_bounds("settings-row-GoToday").unwrap();
    let yesterday = cx.debug_bounds("settings-row-GoYesterday").unwrap();
    assert!(
        (f32::from(yesterday.origin.y) - f32::from(today.origin.y + today.size.height)).abs() < 1.,
        "rows touch: {today:?} then {yesterday:?}"
    );
}

/// ST-5: the tabs are a list down the left, each with an icon, and picking one swaps the
/// pane without moving the rail.
#[gpui::test]
fn side_tabs_switch_panes(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    for key in ["appearance", "keybindings", "shortcuts", "sync", "help"] {
        assert!(
            cx.debug_bounds(match key {
                "appearance" => "settings-nav-appearance",
                "keybindings" => "settings-nav-keybindings",
                "shortcuts" => "settings-nav-shortcuts",
                "sync" => "settings-nav-sync",
                _ => "settings-nav-help",
            })
            .is_some(),
            "the {key} tab is in the list"
        );
    }
    // The rail fills the left edge and the pane starts where it ends.
    let first = cx.debug_bounds("settings-nav-appearance").unwrap();
    assert!(f32::from(first.origin.x) < 30., "{first:?}");
    let pane = cx.debug_bounds("settings-pane").unwrap();
    assert!(f32::from(pane.origin.x) < 221., "{pane:?}");
    assert!(
        f32::from(pane.size.width) > 800.,
        "the pane takes the rest of the window: {pane:?}"
    );
    // The top tab row is gone (never drawn, so a bounds check is sound).
    assert!(cx.debug_bounds("settings-tab-Keybindings").is_none());

    let appearance = cx.debug_bounds("settings-nav-appearance").unwrap();
    cx.simulate_click(appearance.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.tab()), Tab::Appearance);
    assert!(
        cx.debug_bounds("settings-card-Font").is_some(),
        "the Appearance pane is drawn after the click"
    );

    let sync = cx.debug_bounds("settings-nav-sync").unwrap();
    cx.simulate_click(sync.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.tab()), Tab::Sync);
    assert!(
        cx.debug_bounds("settings-sync").is_some(),
        "the Sync pane is drawn after the click"
    );
}

/// ST-5: the Sync tab reports the transport in force and never a secret. The facts are
/// built from the config the app resolved at launch, which holds the token and passphrase.
#[gpui::test]
fn the_sync_tab_shows_the_transport(cx: &mut TestAppContext) {
    use utopia_ui::sync_runner::{SyncConfig, SyncFacts, SyncTarget};
    let cfg = SyncConfig {
        target: SyncTarget::Relay {
            url: "https://relay.example".into(),
            token: Some("s3cret-token".into()),
        },
        graph: "graph-123".into(),
        passphrase: Some("hunter2".into()),
    };
    let facts = SyncFacts::from_config(&cfg);
    // The token and the passphrase are never carried into the view.
    let shown = format!("{facts:?}");
    assert!(facts.encrypted);
    assert!(shown.contains("https://relay.example"), "{shown}");
    assert!(!shown.contains("s3cret-token") && !shown.contains("hunter2"));

    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
        cx.set_global(facts.clone());
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Sync, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-sync").is_some());
    assert!(
        cx.debug_bounds("settings-sync-transport").is_some(),
        "the transport has a row of its own"
    );
}

/// ST-5: Help is one readable column now that the topics sit above it instead of in a
/// second sidebar; the 640 px screenshots fit in the 720 px column.
#[gpui::test]
fn help_screenshots_fit_the_column(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (_view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Help, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    let pane = cx.debug_bounds("settings-pane").unwrap();
    let shot = cx
        .debug_bounds("help-shot")
        .expect("the topic has a screenshot");
    assert!((f32::from(shot.size.width) - 640.).abs() < 1.5, "{shot:?}");
    assert!(
        f32::from(shot.origin.x) >= f32::from(pane.origin.x)
            && f32::from(shot.origin.x + shot.size.width)
                <= f32::from(pane.origin.x + pane.size.width),
        "the screenshot is inside the pane: {shot:?} in {pane:?}"
    );
}

/// ST-4: the field at the top narrows the rows by name, description or key, and no match
/// says so instead of showing an empty pane.
#[gpui::test]
fn search_filters_by_name_description_and_key(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    let all = view.read_with(cx, |v, _| v.visible_actions().len());
    assert_eq!(all, utopia_ui::actions::CATALOG.len());

    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.simulate_input("zoom");
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.query().to_string()), "zoom");
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()),
        ["ZoomIn", "ZoomOut"],
        "by name"
    );

    // By keys, in the spelling the catalog stores them in.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()).len(),
        all,
        "escape clears the filter"
    );
    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.simulate_input("cmd-shift-y");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()),
        ["GoYesterday"],
        "by key"
    );

    // By description ("cheat sheet" is in ShowShortcuts' help text).
    cx.simulate_keystrokes("escape");
    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.simulate_input("cheat");
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.visible_actions()),
        ["ShowShortcuts"],
        "by description"
    );

    // Nothing matches: the pane says so.
    cx.simulate_keystrokes("escape");
    let field = cx
        .debug_bounds("settings-search")
        .expect("the filter field");
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.simulate_input("zzzz");
    cx.run_until_parked();
    assert!(view.read_with(cx, |v, _| v.visible_actions().is_empty()));
    assert!(
        cx.debug_bounds("settings-no-match").is_some(),
        "the empty state is drawn"
    );
}

/// ST-2: the key chip is the control. Clicking it records keys, a changed row shows the
/// small ↺, and the Change button is gone.
#[gpui::test]
fn the_key_chip_records_and_resets(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("settings-change-GoToday").is_none(),
        "no separate Change button (never drawn, so a bounds check is sound)"
    );
    assert!(
        cx.debug_bounds("settings-reset-GoToday").is_none(),
        "the ↺ only appears once the row changed"
    );

    let chip = cx
        .debug_bounds("settings-chip-GoToday")
        .expect("the key chip is the control");
    cx.simulate_click(chip.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    let today = doc("GoToday").unwrap();
    assert_eq!(
        view.read_with(cx, |v, _| v.capturing().map(|d| d.action)),
        Some("GoToday"),
        "clicking the chip starts recording"
    );

    cx.simulate_keystrokes("cmd-alt-t");
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.capturing().is_none());
        assert_eq!(v.overrides().keys_for(today), "alt-cmd-t");
    });
    assert!(
        cx.debug_bounds("settings-reset-GoToday").is_some(),
        "the changed row carries the ↺"
    );

    // Clicking the ↺ puts the catalog default back.
    let reset = cx.debug_bounds("settings-reset-GoToday").unwrap();
    cx.simulate_click(reset.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    view.read_with(cx, |v, _| {
        assert!(v.overrides().is_empty());
        assert_eq!(v.overrides().keys_for(today), platform_keys("cmd-shift-t"));
    });
}

/// ST-3: a clash is an amber line under each of the two rows, and its link jumps to the
/// other row instead of only naming it.
#[gpui::test]
fn a_clash_is_shown_inline_and_links_to_the_other_row(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes(&platform_keys("cmd-shift-y"));
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.overrides().conflicts().len()),
        1
    );

    let chip = cx
        .debug_bounds("settings-chip-GoToday")
        .expect("the rebound row's key chip");
    let line = cx
        .debug_bounds("settings-clash-GoToday")
        .expect("the clash line under the row");
    assert!(
        f32::from(line.origin.y) >= f32::from(chip.origin.y + chip.size.height) - 1.,
        "the line sits under the row's own line: {chip:?} then {line:?}"
    );
    // Both rows say who else uses the keys, so whichever one you are looking at, it shows.
    assert!(
        cx.debug_bounds("settings-clash-GoYesterday").is_some(),
        "the other row carries the line too"
    );

    let link = cx
        .debug_bounds("settings-clash-link-GoToday-GoYesterday")
        .expect("the other action is a link");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.flashed()),
        Some("GoYesterday"),
        "the link jumps to the other row"
    );
}

/// ST-3: a clash across categories jumps the list to the other card, so "Also used by
/// Quit" actually takes you to Quit rather than naming it out of reach.
#[gpui::test]
fn a_clash_link_scrolls_to_the_other_card(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut o = Overrides::default();
    o.set("GoToday", "cmd-q");
    std::fs::write(dir.path().join("keymap.json"), o.to_json()).unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.key_list_offset()), 0.);

    let link = cx
        .debug_bounds("settings-clash-link-GoToday-Quit")
        .expect("Quit is linked under Today");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.flashed()), Some("Quit"));
    assert!(
        view.read_with(cx, |v, _| v.key_list_offset()) < -100.,
        "the list scrolled down to the Application card"
    );
}

/// A clash link must reveal the *row* it names, not just its category: "New Block" shares
/// Cmd+A with "Select All" in the 25-row Editing card, and scrolling only to that card leaves
/// Select All above the viewport. The window is tall enough for the Outline row to be on
/// screen so the link can be clicked.
#[gpui::test]
fn a_clash_link_reveals_a_row_in_a_tall_category(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut o = Overrides::default();
    o.set("Newline", "cmd-a");
    std::fs::write(dir.path().join("keymap.json"), o.to_json()).unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(1150.)));
    cx.run_until_parked();
    let pane = cx.debug_bounds("settings-keys").unwrap();
    let link = cx
        .debug_bounds("settings-clash-link-Newline-SelectAll")
        .expect("the link under New Block");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.flashed()), Some("SelectAll"));
    let row = cx
        .debug_bounds("settings-row-SelectAll")
        .expect("the Select All row");
    assert!(
        f32::from(row.origin.y) >= f32::from(pane.origin.y) - 1.
            && f32::from(row.origin.y + row.size.height)
                <= f32::from(pane.origin.y + pane.size.height) + 1.,
        "the target row is in the viewport: {row:?} in {pane:?}"
    );
}

/// The filter may hide the row a clash link points at; following the link must reveal it
/// rather than only highlighting a row that is not on screen.
#[gpui::test]
fn a_clash_link_reveals_a_row_the_filter_hides(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut o = Overrides::default();
    o.set("GoToday", "cmd-q");
    std::fs::write(dir.path().join("keymap.json"), o.to_json()).unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    let field = cx.debug_bounds("settings-search").unwrap();
    cx.simulate_click(field.center(), gpui::Modifiers::default());
    cx.simulate_input("today");
    cx.run_until_parked();
    let filtered = view.read_with(cx, |v, _| v.visible_actions());
    assert!(filtered.contains(&"GoToday"), "{filtered:?}");
    assert!(
        !filtered.contains(&"Quit"),
        "the target is hidden: {filtered:?}"
    );
    let link = cx
        .debug_bounds("settings-clash-link-GoToday-Quit")
        .expect("the link under Today");
    let pane = cx.debug_bounds("settings-keys").unwrap();
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |v, _| v.flashed()), Some("Quit"));
    assert_eq!(
        view.read_with(cx, |v, _| v.query().to_string()),
        "",
        "the filter is cleared so the target is really shown"
    );
    let row = cx.debug_bounds("settings-row-Quit").expect("the Quit row");
    assert!(
        f32::from(row.origin.y) >= f32::from(pane.origin.y) - 1.
            && f32::from(row.origin.y + row.size.height)
                <= f32::from(pane.origin.y + pane.size.height) + 1.,
        "the target row is in the viewport: {row:?} in {pane:?}"
    );
}

/// Two rows can link to the same action. Each link is scoped to its own row, so the second
/// row's link (which shares the target with the first) is still live: GPUI element ids are
/// nested under the row, not shared across rows.
#[gpui::test]
fn the_second_rows_clash_link_is_live(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut o = Overrides::default();
    o.set("GoToday", "cmd-q");
    o.set("GoYesterday", "cmd-q");
    std::fs::write(dir.path().join("keymap.json"), o.to_json()).unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();
    // Yesterday's link to Quit is the later of the two rows' "Also used by Quit" links.
    let link = cx
        .debug_bounds("settings-clash-link-GoYesterday-Quit")
        .expect("Yesterday's link to Quit");
    cx.simulate_click(link.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    assert_eq!(
        view.read_with(cx, |v, _| v.flashed()),
        Some("Quit"),
        "the second row's link to the same action is live"
    );
    // And the first row's link to the same action still exists under its own id.
    let first = cx
        .debug_bounds("settings-clash-link-GoToday-Quit")
        .expect("Today's link to Quit");
    assert!(first.size.width > px(0.));
}

/// A `keymap.json` that cannot be read must not take the window down with it (rule 11).
#[gpui::test]
fn a_broken_keymap_is_reported_not_fatal(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("keymap.json"), "{ not json").unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.run_until_parked();
    view.update(cx, |v, _| {
        assert!(v.overrides().is_empty());
        assert!(
            v.status().is_some(),
            "the user is told the file was unreadable"
        );
    });
}

/// QJ-5: what the palette shows for a command is the keys in force. Settings installs a
/// rebinding at once, so a row must describe the new chord, not the catalog default.
#[gpui::test]
fn a_rebinding_changes_the_keys_a_command_shows(cx: &mut TestAppContext) {
    use utopia_ui::actions::reinstall;
    let mut o = Overrides::default();
    o.set("GoToday", "alt-cmd-t");
    cx.update(|cx| {
        install_keymap(cx, None);
        reinstall(cx, &o);
    });
    let keys = cx.update(|cx| active_keys(cx, doc("GoToday").unwrap()));
    assert_eq!(keys, "alt-cmd-t");
    assert_eq!(
        utopia_ui::actions::pretty_keys(&keys),
        "⌥⌘T",
        "rendered the way the palette draws it"
    );
    // An action nobody rebound still reports its catalog default.
    let other = cx.update(|cx| active_keys(cx, doc("GoYesterday").unwrap()));
    assert_eq!(other, platform_keys("cmd-shift-y"));
}

/// A rebound action answers to its new keys only: the default it replaced is not left
/// installed alongside, invisible in Settings but still firing.
#[gpui::test]
fn rebinding_replaces_the_default_keys(cx: &mut TestAppContext) {
    use utopia_ui::actions::{install, GoToday};
    let mut o = Overrides::default();
    o.set("GoToday", "alt-cmd-t");
    cx.update(|cx| install(cx, &o));
    let keys: Vec<String> = cx.update(|cx| {
        cx.key_bindings()
            .borrow()
            .bindings_for_action(&GoToday)
            .map(|b| {
                b.keystrokes()
                    .iter()
                    .map(|k| k.unparse())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    });
    assert_eq!(keys, ["alt-cmd-t"]);
}

/// Review fix: an override whose keys cannot be used (empty, or not a keystroke) is
/// ignored with a warning, and the action keeps its default rather than ending up with
/// no binding at all (rule 11: degrade, don't break). Since the keymap overrides fix, the
/// warning names the action: an entry that cannot be bound is a state the editor shows.
#[gpui::test]
fn an_unusable_override_keeps_the_default(cx: &mut TestAppContext) {
    use utopia_ui::actions::{install, GoToday};
    for bad in ["", "   ", "nonsense-key"] {
        let (o, warnings) =
            Overrides::from_json(&format!(r#"[{{"key":"{bad}","action":"GoToday"}}]"#));
        assert_eq!(warnings.len(), 1, "the unusable entry is reported: {bad:?}");
        assert!(warnings[0].contains("GoToday"), "{}", warnings[0]);
        cx.update(|cx| {
            cx.clear_key_bindings();
            install(cx, &o);
        });
        let keys: Vec<String> = cx.update(|cx| {
            cx.key_bindings()
                .borrow()
                .bindings_for_action(&GoToday)
                .map(|b| {
                    b.keystrokes()
                        .iter()
                        .map(|k| k.unparse())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        });
        assert_eq!(keys, [platform_keys("cmd-shift-t")], "override {bad:?}");
    }
}

/// Keymap machine doc, issue 1: a `keymap.json` entry whose keys gpui cannot parse used to
/// be shown as the row's binding while the app kept the default. The row must show what the
/// user wrote *and* say that the default is what works (§9.1, ST-3, rule 11).
#[gpui::test]
fn an_unusable_entry_is_marked_and_the_default_is_in_force(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("keymap.json"),
        r#"[{"key":"nonsense-key","action":"GoToday"}]"#,
    )
    .unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    let today = doc("GoToday").unwrap();
    view.read_with(cx, |v, _| {
        assert_eq!(
            v.overrides().keys_for(today),
            platform_keys("cmd-shift-t"),
            "the default is what gpui uses, so it is what the row shows as in force"
        );
    });
    assert!(
        cx.debug_bounds("settings-invalid-GoToday").is_some(),
        "the row says the written keys cannot be used"
    );
}

/// Keymap machine doc, issue 2: a write that fails was overwritten by the rebind's own status
/// line, so the next launch silently went back to the old file. The failure is now a state of
/// its own and stays on screen next to the message (§9.1).
#[gpui::test]
fn a_failed_save_is_reported_after_the_rebind(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should go: `Overrides::save` fails every time.
    std::fs::create_dir(dir.path().join("keymap.json")).unwrap();
    cx.update(|cx| {
        cx.set_global(Theme::light());
        install_keymap(cx, None);
    });
    let path = dir.path().to_path_buf();
    let (view, cx) =
        cx.add_window_view(|window, cx| SettingsView::new(path, Tab::Keybindings, window, cx));
    cx.simulate_resize(size(px(1100.), px(760.)));
    cx.run_until_parked();

    let today = doc("GoToday").unwrap();
    view.update_in(cx, |v, window, cx| v.start_capture(today, window, cx));
    cx.simulate_keystrokes("cmd-alt-t");
    cx.run_until_parked();

    view.read_with(cx, |v, _| {
        assert_eq!(
            v.overrides().keys_for(today),
            "alt-cmd-t",
            "the keys are live for this session"
        );
        assert!(
            v.status().unwrap_or_default().contains("Today is now"),
            "the rebind is still reported"
        );
        assert!(
            v.unsaved().is_some_and(|e| e.contains("Could not save")),
            "the failed write is a state of its own"
        );
    });
    assert!(
        cx.debug_bounds("settings-unsaved").is_some(),
        "and so is the fact that it is not on disk"
    );
}

mod appearance {
    use chrono::NaiveDate;
    use gpui::{AppContext as _, Modifiers, TestAppContext, VisualTestContext};
    use utopia_core::PageId;
    use utopia_store::Store;
    use utopia_ui::actions::install_keymap;
    use utopia_ui::app_state::AppState;
    use utopia_ui::appearance::{self, AttributeBlocks, Density, UiFont};
    use utopia_ui::settings::{SettingsView, Tab};
    use utopia_ui::theme::Theme;
    use utopia_ui::workspace::Workspace;

    fn settings_json(dir: &std::path::Path) -> Option<serde_json::Value> {
        let s = std::fs::read_to_string(dir.join("settings.json")).ok()?;
        Some(serde_json::from_str(&s).expect("settings.json is JSON"))
    }

    /// Open the main window's launch page and its right sidebar, as a user would.
    fn open_sidebar(w: &mut VisualTestContext) {
        let ws = w
            .update(|window, _| window.root::<Workspace>())
            .flatten()
            .expect("the workspace root");
        ws.update_in(w, |this, window, cx| this.open_launch_page(window, cx));
        w.run_until_parked();
        w.simulate_keystrokes("escape cmd-shift-\\");
        w.run_until_parked();
    }

    /// A main window and the Settings window, both open, as in the app.
    fn open_both(
        cx: &mut TestAppContext,
        dir: &std::path::Path,
    ) -> (gpui::AnyWindowHandle, gpui::AnyWindowHandle) {
        let path = dir.to_path_buf();
        cx.update(|cx| {
            cx.set_global(Theme::light());
            install_keymap(cx, None);
            appearance::install(cx, &path);
        });
        let mut store = Store::open_in_memory().unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let app = cx.new(|_| AppState::new(store));
        let main = cx.add_window(|window, cx| Workspace::new(app, window, cx));
        let settings =
            cx.add_window(move |window, cx| SettingsView::new(path, Tab::Appearance, window, cx));
        cx.run_until_parked();
        (main.into(), settings.into())
    }

    /// A main window showing "Target" (referenced by a block with four children) and the
    /// Settings window, both open, as in the app (LR-4).
    fn open_both_with_reference(
        cx: &mut TestAppContext,
        dir: &std::path::Path,
    ) -> (
        gpui::AnyWindowHandle,
        gpui::AnyWindowHandle,
        utopia_core::PageId,
        VisualTestContext,
    ) {
        use utopia_core::{BlockId, BlockKind, Command};
        let path = dir.to_path_buf();
        cx.update(|cx| {
            cx.set_global(Theme::light());
            install_keymap(cx, None);
            appearance::install(cx, &path);
        });
        let mut store = Store::open_in_memory().unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let target = store.create_page("Target").unwrap();
        let src = store.create_page("Source").unwrap();
        let link = BlockId::new();
        let mut cmds = vec![Command::InsertBlock {
            id: link.clone(),
            parent: None,
            after: None,
            text: "The release depends on [[Target]]".into(),
            kind: BlockKind::Bullet,
        }];
        let mut after = None;
        for n in 1..=4 {
            let id = BlockId::new();
            cmds.push(Command::InsertBlock {
                id: id.clone(),
                parent: Some(link.clone()),
                after: after.clone(),
                text: format!("Reason {n}"),
                kind: BlockKind::Bullet,
            });
            after = Some(id);
        }
        store.execute(&src, cmds, Default::default()).unwrap();
        store.sync_index().unwrap();
        let app = cx.new(|_| AppState::new(store));
        let main: gpui::AnyWindowHandle = cx
            .add_window(|window, cx| Workspace::new(app, window, cx))
            .into();
        let settings =
            cx.add_window(move |window, cx| SettingsView::new(path, Tab::Appearance, window, cx));
        cx.run_until_parked();
        let mut w = VisualTestContext::from_window(main, cx);
        let ws = w
            .update(|window, _| window.root::<Workspace>())
            .flatten()
            .expect("workspace root");
        ws.update_in(&mut w, |w, window, cx| {
            w.open(target.clone(), None, None, false, window, cx)
        });
        w.run_until_parked();
        (main, settings.into(), target, w)
    }

    /// Settings → Appearance: picking IBM Plex Sans applies it to every open window at once
    /// and writes it to `settings.json` (not `keymap.json`), versioned.
    #[gpui::test]
    fn choosing_a_font_applies_everywhere_and_is_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        assert_eq!(cx.update(|cx| appearance::ui_font(cx)), UiFont::System);
        assert!(settings_json(dir.path()).is_none(), "nothing saved yet");
        {
            let mut w = VisualTestContext::from_window(main, cx);
            assert!(w.debug_bounds("ui-font-system").is_some());
        }

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(s.debug_bounds("settings-ui-font-system").is_some());
        let option = s
            .debug_bounds("settings-font-ibm-plex-sans")
            .expect("the IBM Plex Sans choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();

        assert_eq!(
            s.update(|_, cx| appearance::ui_font(cx)),
            UiFont::IbmPlexSans
        );
        assert_eq!(
            s.update(|_, cx| appearance::font_family(cx)),
            "IBM Plex Sans",
            "the family the bundled faces register under"
        );
        // Both windows were drawn again with the new family at their root, unprompted.
        assert!(s.debug_bounds("settings-ui-font-ibm-plex-sans").is_some());
        let mut w = VisualTestContext::from_window(main, cx);
        w.run_until_parked();
        assert!(w.debug_bounds("ui-font-ibm-plex-sans").is_some());

        assert_eq!(
            settings_json(dir.path()),
            Some(serde_json::json!({
                "version": 1,
                "font": "ibm-plex-sans",
                "density": "comfortable",
                "ref_children": 2,
                "attribute-blocks": "compact",
                "pinned": [],
                "right_sidebar_width": 380.0,
            }))
        );
        assert!(
            !dir.path().join("keymap.json").exists(),
            "the font is not a keybinding"
        );

        // And back: System is written down too, so the choice is explicit.
        let mut s = VisualTestContext::from_window(settings, cx);
        let option = s.debug_bounds("settings-font-system").unwrap();
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(s.update(|_, cx| appearance::ui_font(cx)), UiFont::System);
        assert_eq!(
            settings_json(dir.path()).unwrap()["font"],
            "system",
            "switching back is saved"
        );
    }

    /// ALL-5: density is a local preference like the font. Picking Compact applies to every
    /// open window and is written to settings.json; Comfortable can be picked back.
    #[gpui::test]
    fn choosing_density_applies_everywhere_and_is_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        assert_eq!(
            cx.update(|cx| appearance::density(cx)),
            Density::Comfortable
        );
        assert!(settings_json(dir.path()).is_none(), "nothing saved yet");
        {
            let mut w = VisualTestContext::from_window(main, cx);
            assert!(w.debug_bounds("ui-density-comfortable").is_some());
        }

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(s.debug_bounds("settings-ui-density-comfortable").is_some());
        let option = s
            .debug_bounds("settings-density-compact")
            .expect("the Compact choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();

        assert_eq!(s.update(|_, cx| appearance::density(cx)), Density::Compact);
        assert!(s.debug_bounds("settings-ui-density-compact").is_some());
        let mut w = VisualTestContext::from_window(main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds("ui-density-compact").is_some(),
            "the main window was redrawn compact without being asked"
        );
        let saved = settings_json(dir.path()).expect("settings.json written");
        assert_eq!(saved["density"], "compact");
        assert_eq!(saved["font"], "system", "the font is kept");

        // And back: Comfortable is explicit in the file, like System.
        let option = s.debug_bounds("settings-density-comfortable").unwrap();
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            s.update(|_, cx| appearance::density(cx)),
            Density::Comfortable
        );
        assert_eq!(settings_json(dir.path()).unwrap()["density"], "comfortable");
    }

    /// SB-3: pins live in settings.json (not keymap.json, never the graph). Settings →
    /// Appearance says how many there are and can clear them.
    #[gpui::test]
    fn pinned_pages_are_local_and_cleared_from_settings(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (_main, settings) = open_both(cx, dir.path());
        let page = PageId("page-a".into());
        cx.update(|cx| {
            let on = appearance::toggle_pinned(cx, &page).expect("settings.json is writable");
            assert!(on);
        });
        assert_eq!(
            settings_json(dir.path()).unwrap()["pinned"],
            serde_json::json!(["page-a"]),
            "the pin is local state in settings.json"
        );
        assert!(
            !dir.path().join("keymap.json").exists(),
            "a pin is not a keybinding"
        );

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(
            s.debug_bounds("settings-pinned").is_some(),
            "Settings → Appearance shows the pinned pages"
        );
        let clear = s
            .debug_bounds("settings-clear-pinned")
            .expect("a Clear control when something is pinned");
        s.simulate_click(clear.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["pinned"],
            serde_json::json!([]),
            "Clear unpins every page"
        );
        assert!(s.update(|_, cx| appearance::pinned(cx).is_empty()));
        // Nothing to clear any more. (The old control's debug bounds linger, so assert on
        // what replaced it: the status line says the pins are gone.)
        let view = s
            .update(|window, _| window.root::<SettingsView>())
            .flatten()
            .expect("settings root");
        assert_eq!(
            view.update(cx, |v, _| v.status().map(str::to_string)),
            Some("Unpinned every page.".to_string())
        );
    }

    /// RS-3: the Pinned tab writes through the same local list as the left sidebar's Pinned
    /// group (SB-3), so a pin made there is in `settings.json` and the next launch sees it.
    #[gpui::test]
    fn pinning_a_page_is_written_to_settings_json(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        cx.update(|cx| {
            cx.set_global(Theme::light());
            install_keymap(cx, None);
            appearance::install(cx, &path);
        });
        let mut store = Store::open_in_memory().unwrap();
        store.set_today(Some(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()));
        let page = store.create_page("Keep me").unwrap();
        let app = cx.new(|_| AppState::new(store));
        let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app, window, cx));
        ws.update_in(cx, |w, window, cx| {
            w.open(page.clone(), None, None, false, window, cx)
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("escape cmd-shift-\\");
        cx.run_until_parked();
        let pinned_tab = cx.debug_bounds("panel-pinned").expect("the Pinned tab");
        cx.simulate_click(pinned_tab.center(), Modifiers::default());
        cx.run_until_parked();
        let pin = cx.debug_bounds("pin-current").expect("the pin control");
        cx.simulate_click(pin.center(), Modifiers::default());
        cx.run_until_parked();

        assert_eq!(
            settings_json(dir.path()).expect("settings.json written")["pinned"],
            serde_json::json!([page.as_str()])
        );
        let (prefs, warnings) = appearance::Prefs::load(dir.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(prefs.pinned, vec![page.clone()]);
        assert_eq!(
            prefs.density,
            Density::Comfortable,
            "pinning does not disturb the other preferences"
        );

        let unpin = cx.debug_bounds("unpin-0").expect("the unpin control");
        cx.simulate_click(unpin.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["pinned"],
            serde_json::json!([])
        );
    }

    /// RS-4: dragging the sidebar's edge changes its width at once and writes the width to
    /// `settings.json` on release; Settings → Appearance reports it and can reset it.
    #[gpui::test]
    fn the_sidebar_width_is_dragged_and_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        let mut w = VisualTestContext::from_window(main, cx);
        open_sidebar(&mut w);
        let handle = w.debug_bounds("right-resize").expect("the 4 px handle");
        let start = handle.center();
        w.simulate_mouse_down(start, gpui::MouseButton::Left, Modifiers::default());
        let to = gpui::point(start.x - gpui::px(80.), start.y);
        w.simulate_mouse_move(to, gpui::MouseButton::Left, Modifiers::default());
        w.simulate_mouse_up(to, gpui::MouseButton::Left, Modifiers::default());
        w.run_until_parked();

        let side = w.debug_bounds("right-sidebar").expect("the sidebar");
        assert!(
            (f32::from(side.size.width) - 460.).abs() < 1.5,
            "dragged 80 px wider: {side:?}"
        );
        assert_eq!(
            settings_json(dir.path()).expect("settings.json written")["right_sidebar_width"],
            serde_json::json!(460.0),
            "one write on release, with the width in it"
        );

        let mut s = VisualTestContext::from_window(settings, cx);
        // The Appearance tab is a scrolling column and this card is its last one, so give
        // the window room to show it rather than clicking bounds that are scrolled away.
        s.simulate_resize(gpui::size(gpui::px(1100.), gpui::px(1200.)));
        s.run_until_parked();
        assert!(
            s.debug_bounds("settings-sidebar-width-460").is_some(),
            "Settings reports the width in force"
        );
        let reset = s
            .debug_bounds("settings-sidebar-reset")
            .expect("the reset control");
        s.simulate_click(reset.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["right_sidebar_width"],
            serde_json::json!(380.0),
            "the default is written down too"
        );
        w.run_until_parked();
        let side = w.debug_bounds("right-sidebar").expect("the sidebar");
        assert!(
            (f32::from(side.size.width) - 380.).abs() < 1.5,
            "the sidebar went back to the default width: {side:?}"
        );
    }

    /// RS-4: the width a previous session left is the width the next one opens with.
    #[gpui::test]
    fn a_saved_sidebar_width_is_used_at_startup(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"version": 1, "right_sidebar_width": 480}"#,
        )
        .unwrap();
        let (main, _) = open_both(cx, dir.path());
        let mut w = VisualTestContext::from_window(main, cx);
        open_sidebar(&mut w);
        let side = w.debug_bounds("right-sidebar").expect("the sidebar");
        assert!(
            (f32::from(side.size.width) - 480.).abs() < 1.5,
            "the saved width is in force: {side:?}"
        );
    }

    /// The saved choice is what a new launch comes up with.
    #[gpui::test]
    fn a_saved_font_is_used_at_startup(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"version": 1, "font": "ibm-plex-sans"}"#,
        )
        .unwrap();
        let (main, _) = open_both(cx, dir.path());
        assert_eq!(cx.update(|cx| appearance::ui_font(cx)), UiFont::IbmPlexSans);
        let mut w = VisualTestContext::from_window(main, cx);
        assert!(w.debug_bounds("ui-font-ibm-plex-sans").is_some());
    }

    /// Rule 11: an unknown font in the file comes up as System, says so, and the window opens.
    #[gpui::test]
    fn an_unknown_font_falls_back_to_system(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"version": 1, "font": "comic-sans"}"#,
        )
        .unwrap();
        let warnings = cx.update(|cx| appearance::install(cx, dir.path()));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(cx.update(|cx| appearance::ui_font(cx)), UiFont::System);

        let (main, settings) = open_both(cx, dir.path());
        let mut w = VisualTestContext::from_window(main, cx);
        assert!(w.debug_bounds("ui-font-system").is_some());
        let view = settings.downcast::<SettingsView>().unwrap();
        let status = cx
            .read(|cx| view.read(cx).map(|v| v.status().map(str::to_string)))
            .unwrap()
            .unwrap_or_default();
        assert!(
            status.contains("comic-sans"),
            "the user is told: {status:?}"
        );
        // The file is left as it was until the user picks something.
        assert!(std::fs::read_to_string(dir.path().join("settings.json"))
            .unwrap()
            .contains("comic-sans"));
    }

    /// PG-1: how attribute blocks are drawn is a third local preference, picked in
    /// Settings → Appearance, applied to every open window and written to settings.json.
    #[gpui::test]
    fn choosing_attribute_blocks_applies_everywhere_and_is_saved(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (main, settings) = open_both(cx, dir.path());
        assert_eq!(
            cx.update(|cx| appearance::attribute_blocks(cx)),
            AttributeBlocks::Compact,
            "compact rows are the default"
        );
        assert!(settings_json(dir.path()).is_none(), "nothing saved yet");

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        let option = s
            .debug_bounds("settings-attributes-blocks")
            .expect("the As blocks choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            s.update(|_, cx| appearance::attribute_blocks(cx)),
            AttributeBlocks::Blocks
        );
        let saved = settings_json(dir.path()).expect("settings.json written");
        assert_eq!(saved["attribute-blocks"], "blocks");
        assert_eq!(saved["font"], "system", "the other preferences are kept");

        // The main window follows, without being asked, and can be switched back here.
        let mut w = VisualTestContext::from_window(main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds("ui-density-comfortable").is_some(),
            "the page window still draws"
        );
        let option = s
            .debug_bounds("settings-attributes-compact")
            .expect("the Compact choice is shown");
        s.simulate_click(option.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).unwrap()["attribute-blocks"],
            "compact",
            "switching back is saved too"
        );
    }

    /// LR-4: "Children shown in references" is a local preference (rule 9). Picking 0 folds
    /// every child behind the chip in a page that is already open; 3 shows three of four.
    #[gpui::test]
    fn choosing_reference_children_is_saved_and_applies(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let (_main, settings, _target, mut w) = open_both_with_reference(cx, dir.path());
        let ix = (0..40)
            .find(|ix| {
                let sel: &'static str = Box::leak(format!("hit-child-{ix}-0").into_boxed_str());
                w.debug_bounds(sel).is_some()
            })
            .expect("the default of two children is shown");
        let (more_four, more_one): (&'static str, &'static str) = (
            Box::leak(format!("hit-more-{ix}-4").into_boxed_str()),
            Box::leak(format!("hit-more-{ix}-1").into_boxed_str()),
        );

        let mut s = VisualTestContext::from_window(settings, cx);
        s.run_until_parked();
        assert!(s.debug_bounds("settings-ref-children-2").is_some());
        let zero = s
            .debug_bounds("settings-ref-children-0")
            .expect("the 0 choice is offered");
        s.simulate_click(zero.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(
            settings_json(dir.path()).expect("settings.json written")["ref_children"],
            0
        );
        assert!(s.debug_bounds("settings-ref-children-0").is_some());

        // The page that was already open folds all four behind the chip.
        let mut w = VisualTestContext::from_window(_main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds(more_four).is_some(),
            "0 hides all four children"
        );

        let three = s
            .debug_bounds("settings-ref-children-3")
            .expect("the 3 choice is offered");
        s.simulate_click(three.center(), Modifiers::default());
        s.run_until_parked();
        assert_eq!(settings_json(dir.path()).unwrap()["ref_children"], 3);
        let mut w = VisualTestContext::from_window(_main, cx);
        w.run_until_parked();
        assert!(
            w.debug_bounds(more_one).is_some(),
            "three shown, one folded"
        );
    }
}
