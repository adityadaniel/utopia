//! The Settings window (§9.1, `OpenSettings` / Cmd+,). Five tabs, in a list down the left
//! side as System Settings has it (ST-5):
//!
//! - **Appearance** — the font notes and the interface are set in (System or the bundled
//!   IBM Plex Sans), how densely blocks are set (Comfortable or Compact) and how far
//!   children indent (Wide, 45 px, or Narrow, 26 px). Applies to every window at once;
//!   kept in `settings.json` ([`crate::appearance`]), not in `keymap.json`.
//! - **Keybindings** — every action in [`crate::actions::CATALOG`] with the keys in force,
//!   rebindable by pressing the combination you want. Changes apply at once and are written
//!   to `keymap.json`; only the actions you changed are stored.
//! - **Shortcuts** — the cheat sheet of everything currently bound, which is also what the
//!   Help menu's "Keyboard Shortcuts" opens.
//! - **Sync** — the setup screen for this graph's sync (§12): Off, a relay, a nearby Mac
//!   (mDNS, optionally hosting the relay from this Mac) or a shared folder, applied without
//!   a restart. The relay token and the end-to-end passphrase are typed into masked fields
//!   and go only to the macOS keychain (§15.6); the non-secret choice is kept per graph
//!   under `<config>/sync/`. Environment variables override the saved choice for the run
//!   and the pane says so. "Test connection" makes one manifest request off the UI thread;
//!   "Connect another device" shows what the other instance needs, never a secret. The
//!   pane itself is [`crate::sync_setup::SyncSetup`], installed by the app for the open
//!   graph; without one (component tests) the tab shows the transport facts only.
//! - **Help** — a tour of the app's features, with screenshots taken from the app itself.
//!
//! Capture uses a gpui *keystroke interceptor* rather than a key-down handler on purpose:
//! interceptors run before the keymap is consulted, so stopping propagation there means the
//! keys being recorded cannot also fire their own action. Without that, rebinding onto
//! Cmd+Q would quit the app mid-edit.

use gpui::{
    div, img, prelude::*, px, App, Context, Div, Entity, FocusHandle, Focusable, FontWeight,
    Keystroke, ScrollHandle, SharedString, Subscription, Window,
};
use std::path::PathBuf;

use crate::actions::{
    by_category, doc, pretty_keys, reinstall, ActionDoc, Category, Override, Overrides, CATALOG,
};
use crate::appearance::{self, Density, Indent, UiFont, UiFontExt};
use crate::help::{shot_image, TOPICS};
use crate::line_input::{LineInput, LineInputEvent};
use crate::sync_runner;
use crate::theme::{quiet_control, theme, Theme};

/// Every pane is set in one readable column (ST-1), like the page column does.
const CONTENT_WIDTH: f32 = 720.;
/// The tab list down the left side, as System Settings has it (ST-5).
const RAIL_WIDTH: f32 = 220.;
/// The children a key list draws before its first category card: the toolbar and the
/// filter field. `ScrollHandle::scroll_to_item` counts them, so a clash link can bring the
/// card that holds the other row into view (ST-3).
const KEY_LIST_LEADING: usize = 2;
/// Screenshots are shown at a fixed size. gpui derives an image's aspect ratio from the
/// decoded bytes, but that lands a frame after layout, so a width alone leaves the element
/// at its intrinsic height; giving both keeps the box the right shape from the first frame.
/// Every capture is the same window, so one ratio covers them all (see `xtask help-shots`).
const HELP_SHOT_WIDTH: f32 = 640.;
const HELP_SHOT_HEIGHT: f32 = HELP_SHOT_WIDTH * 798. / 1200.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Appearance,
    Keybindings,
    Shortcuts,
    Sync,
    Help,
}

impl Tab {
    fn label(self) -> &'static str {
        match self {
            Tab::Appearance => "Appearance",
            Tab::Keybindings => "Keybindings",
            Tab::Shortcuts => "Shortcuts",
            Tab::Sync => "Sync",
            Tab::Help => "Help",
        }
    }

    /// The glyph at the left of the tab: a letter or a symbol, drawn in the tab's colour.
    /// Text rather than an image because the app ships no icons and a glyph inherits the
    /// type size, the density and the font the user picked.
    fn icon(self) -> &'static str {
        match self {
            Tab::Appearance => "Aa",
            Tab::Keybindings => "⌘",
            Tab::Shortcuts => "☰",
            Tab::Sync => "↻",
            Tab::Help => "?",
        }
    }

    /// How a debug selector names the tab.
    fn key(self) -> &'static str {
        match self {
            Tab::Appearance => "appearance",
            Tab::Keybindings => "keybindings",
            Tab::Shortcuts => "shortcuts",
            Tab::Sync => "sync",
            Tab::Help => "help",
        }
    }

    fn all() -> &'static [Tab] {
        &[
            Tab::Appearance,
            Tab::Keybindings,
            Tab::Shortcuts,
            Tab::Sync,
            Tab::Help,
        ]
    }
}

/// Whether the window is recording a binding (§9.1, `settings-key-capture.md`).
///
/// The action waiting for a keystroke and the interceptor that records it are one value:
/// they used to be two `Option`s that always moved together, so a capture with no
/// interceptor (or the reverse) was representable. The window a capture belongs to is not
/// kept here — the interceptor's closure carries the handle it was installed for, and both
/// the focus and the activation observers are registered on this view's own window, so a
/// second copy would be state nothing reads.
#[derive(Default)]
enum Capture {
    /// Keystrokes are the window's own.
    #[default]
    Idle,
    /// The next keystroke in this window is recorded for `action`. Replacing the value with
    /// `Idle` drops the subscription and so stops intercepting.
    Listening {
        action: &'static ActionDoc,
        _interceptor: Subscription,
    },
}

impl Capture {
    /// The action waiting for a keystroke, if one is.
    fn action(&self) -> Option<&'static ActionDoc> {
        match self {
            Capture::Idle => None,
            Capture::Listening { action, .. } => Some(action),
        }
    }
}

/// What a recorded keystroke is, as opposed to the raw keys (§9.1).
///
/// Naming the two outcomes keeps the cancel path visible: without it, "this keystroke is
/// not a binding" is a condition buried in the middle of `captured`.
enum Captured {
    /// The user backed out; nothing is written.
    Cancel,
    /// A combination to bind, if the catalog accepts it.
    Chord(Keystroke),
}

impl From<Keystroke> for Captured {
    fn from(ks: Keystroke) -> Self {
        // Escape cancels whatever modifiers are held: a modified Escape reads as "back out"
        // to the user, so binding it silently installed a shortcut nobody meant, on a chord
        // that would also look like the cancel key in the row (`settings-key-capture.md`,
        // issue 3).
        if ks.key == "escape" {
            Captured::Cancel
        } else {
            Captured::Chord(ks)
        }
    }
}

pub struct SettingsView {
    config_dir: PathBuf,
    overrides: Overrides,
    tab: Tab,
    /// What the window is doing with keystrokes.
    capture: Capture,
    /// A file this window read was not what it says: what is in force is not what the user
    /// wrote (rule 11: degrade, never panic). A condition about the disk, so it has a slot
    /// of its own rather than sharing the status line with the result of the last action
    /// (`settings-key-capture.md`, issue 4).
    warning: Option<SharedString>,
    /// The result of the last thing the user did in this window: a rebind, a reset, a
    /// preference change. Transient by nature — one slot is right for these, because only
    /// the newest one is news.
    status: Option<SharedString>,
    /// The last write of `keymap.json` failed. The keys are live for this session but disk
    /// still holds the old ones, so the next launch would silently undo them: a state of its
    /// own, drawn next to the status line, not a status message the next action overwrites
    /// (keymap machine doc, issue 2).
    save_error: Option<SharedString>,
    topic: usize,
    /// The filter at the top of the key list (ST-4), by name, description or key.
    search: Entity<LineInput>,
    query: String,
    /// The row a clash link jumped to (ST-3), kept highlighted for a moment.
    flash: Option<&'static str>,
    /// The scrolling key list, so a clash link can bring the other row into view.
    keys_scroll: ScrollHandle,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsView {
    pub fn new(config_dir: PathBuf, tab: Tab, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The Appearance tab shows a sample of every font it offers.
        for &f in UiFont::ALL {
            appearance::load_font(cx, f);
        }
        let (overrides, mut warnings) = Overrides::load(&config_dir);
        for w in &warnings {
            eprintln!("utopia: {w}");
        }
        // Already logged at startup; repeated here so the user sees it.
        warnings.extend(appearance::Prefs::load(&config_dir).1);
        let search = cx.new(|cx| LineInput::new("name, description or key", cx));
        let mut subs = vec![
            // Escape clears the field and hands the keyboard back to the window, so the
            // key list's own shortcuts keep working.
            cx.subscribe_in(
                &search,
                window,
                |this, _, e: &LineInputEvent, window, cx| match e {
                    LineInputEvent::Changed(query) if *query != this.query => {
                        this.query = query.clone();
                        cx.notify();
                    }
                    LineInputEvent::Changed(_) => {}
                    LineInputEvent::Cancel => {
                        this.clear_filter(cx);
                        let focus = this.focus.clone();
                        window.focus(&focus);
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        ];
        // The theme is a global that each window keeps in step with its own appearance.
        // Without an observer of its own, Settings froze on the last theme when the main
        // window closed first (issue 6).
        subs.push(cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(Theme::for_window(window.appearance()));
            cx.notify();
        }));
        let focus = cx.focus_handle();
        // A recording belongs to this window, so it ends when the keyboard goes elsewhere:
        // another control taking focus here (the filter field — the only other focusable
        // thing in the window), focus leaving the view, or the window losing activation
        // (Cmd+Tab, a click in the main window). Otherwise the chip keeps saying
        // "Press keys…" for a keystroke typed somewhere else, and the interceptor swallows
        // the Escape the field is waiting for (`settings-key-capture.md`, issues 2 and 5).
        //
        // Focus on *this* handle is not enough: the field is a descendant, so moving into it
        // is not a focus-out of the window's own handle. A focusable control added to this
        // window later needs its own listener here.
        //
        // The two halves are not redundant: gpui only reports focus changes while the window
        // is active (it blanks the focus path for an inactive one), so leaving the app is
        // seen by the activation observer and moving between controls by the focus one.
        let search_focus = search.read(cx).focus_handle(cx);
        subs.push(cx.on_focus(&search_focus, window, |this, _, cx| {
            this.capture_left_the_window(cx);
        }));
        subs.push(cx.on_focus_out(&focus, window, |this, _, _, cx| {
            this.capture_left_the_window(cx);
        }));
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.capture_left_the_window(cx);
            }
        }));
        Self {
            config_dir,
            overrides,
            tab,
            capture: Capture::Idle,
            // A malformed keymap.json or settings.json is worth saying out loud: what is in
            // force is not what the user wrote (rule 11: degrade, never panic).
            warning: warnings.first().map(|w| SharedString::from(w.clone())),
            status: None,
            save_error: None,
            topic: 0,
            search,
            query: String::new(),
            flash: None,
            keys_scroll: ScrollHandle::default(),
            focus,
            _subs: subs,
        }
    }

    pub fn show(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.cancel_capture();
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.cancel_capture();
        cx.notify();
    }

    /// Stop recording, if a recording is in progress.
    fn cancel_capture(&mut self) {
        self.capture = Capture::Idle;
    }

    /// End a capture because the keyboard went somewhere else. Said out loud: the chip going
    /// back to the keys in force without a word looks like the recording was applied.
    fn capture_left_the_window(&mut self, cx: &mut Context<Self>) {
        if self.capture.action().is_none() {
            return;
        }
        self.cancel_capture();
        self.status = Some("Capture cancelled.".into());
        cx.notify();
    }

    /// Start listening for the next keystroke and bind it to `d`.
    pub fn start_capture(
        &mut self,
        d: &'static ActionDoc,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.status = None;
        let handle = window.window_handle();
        let me = cx.entity().downgrade();
        let interceptor = cx.intercept_keystrokes(move |ev, window, cx| {
            // Another window's typing is none of our business.
            if window.window_handle() != handle {
                return;
            }
            // Swallow it so the keys cannot also run their own action while we record them.
            cx.stop_propagation();
            let ks = ev.keystroke.clone();
            let _ = me.update(cx, |this, cx| this.captured(ks, cx));
        });
        self.capture = Capture::Listening {
            action: d,
            _interceptor: interceptor,
        };
        cx.notify();
    }

    /// A keystroke arrived while capturing.
    fn captured(&mut self, ks: Keystroke, cx: &mut Context<Self>) {
        let Some(d) = self.capture.action() else {
            return;
        };
        if is_modifier_only(&ks) {
            return; // still waiting for a real key
        }
        // One keystroke ends the recording, whichever way it is read: a capture that stayed
        // open after a cancel would swallow the window's own keys.
        self.cancel_capture();
        let keys = match Captured::from(ks) {
            Captured::Cancel => {
                self.status = Some("Left unchanged.".into());
                cx.notify();
                return;
            }
            Captured::Chord(ks) => ks.unparse(),
        };
        // Round-trip through the catalog's own validation: whatever gpui hands us has to be
        // something `binding` will accept later, or the override would silently do nothing.
        if crate::actions::binding(d.action, &keys, d.context).is_none() {
            self.status =
                Some(format!("{} cannot be used as a shortcut.", pretty_keys(&keys)).into());
            cx.notify();
            return;
        }
        self.rebind(d, keys, cx);
    }

    fn rebind(&mut self, d: &'static ActionDoc, keys: String, cx: &mut Context<Self>) {
        let pretty = pretty_keys(&keys);
        // `set` drops an entry that only repeats the catalog default, so rebinding to the
        // chord an action already has puts it back to default rather than storing a copy
        // (keymap machine doc, issue 4).
        self.overrides.set(d.action, &keys);
        self.apply(cx);
        let taken: Vec<&str> = self
            .overrides
            .conflicts()
            .into_iter()
            .filter(|(a, b)| a.action == d.action || b.action == d.action)
            .map(|(a, b)| {
                if a.action == d.action {
                    b.label
                } else {
                    a.label
                }
            })
            .collect();
        self.status = Some(if taken.is_empty() {
            format!("{} is now {pretty}.", d.label).into()
        } else {
            format!(
                "{} is now {pretty}, which {} also uses.",
                d.label,
                taken.join(" and ")
            )
            .into()
        });
        cx.notify();
    }

    pub fn reset_one(&mut self, d: &'static ActionDoc, cx: &mut Context<Self>) {
        self.overrides.reset(d.action);
        self.apply(cx);
        self.status = Some(
            format!(
                "{} is back to {}.",
                d.label,
                pretty_keys(&crate::actions::default_keys(d))
            )
            .into(),
        );
        cx.notify();
    }

    pub fn reset_all(&mut self, cx: &mut Context<Self>) {
        if self.overrides.is_empty() {
            self.status = Some("Nothing to reset: these are the defaults.".into());
            cx.notify();
            return;
        }
        self.overrides = Overrides::default();
        self.apply(cx);
        self.status = Some("Every shortcut is back to its default.".into());
        cx.notify();
    }

    /// Put the bindings live and write them down. The two outcomes are kept apart: a failed
    /// write leaves the keys working but the file behind, so it is remembered as such rather
    /// than reported as a status the next rebind overwrites.
    fn apply(&mut self, cx: &mut Context<Self>) {
        reinstall(cx, &self.overrides);
        self.save_error = match self.overrides.save(&self.config_dir) {
            Ok(()) => None,
            Err(e) => Some(
                format!("Could not save keymap.json: {e}. These keys work until you quit.").into(),
            ),
        };
    }

    pub fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    /// Whether a keystroke is being recorded right now.
    pub fn capturing(&self) -> Option<&'static ActionDoc> {
        self.capture.action()
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_deref().map(|v| &**v)
    }

    /// What was wrong with a file this window read, while it is still wrong.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref().map(|v| &**v)
    }

    /// Why the last write failed, while disk still disagrees with the keys in force.
    pub fn unsaved(&self) -> Option<&str> {
        self.save_error.as_deref().map(|v| &**v)
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// What is typed in the filter field (ST-4).
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Where an action's row sits among the key list's direct children (ST-3): the toolbar,
    /// the filter, then each category's header and its rows.
    fn key_list_index(&self, action: &str) -> Option<usize> {
        let mut ix = KEY_LIST_LEADING;
        for (_, rows) in self.visible_groups() {
            ix += 1; // the category's header
            for d in rows {
                if d.action == action {
                    return Some(ix);
                }
                ix += 1;
            }
        }
        None
    }

    /// Empty the filter field and the query behind it.
    fn clear_filter(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.search.update(cx, |s, cx| s.set_text("", cx));
    }

    /// The row a clash link just jumped to, while it stays highlighted (ST-3).
    pub fn flashed(&self) -> Option<&'static str> {
        self.flash
    }

    /// ST-3: bring the row that shares this key into view and mark it, so "Also used by"
    /// leads somewhere instead of just naming the problem.
    fn jump_to(&mut self, target: &'static ActionDoc, cx: &mut Context<Self>) {
        // The filter may be hiding the row the link points at; show it before scrolling.
        if !self.matches(target) {
            self.clear_filter(cx);
        }
        self.flash = Some(target.action);
        if let Some(ix) = self.key_list_index(target.action) {
            self.keys_scroll.scroll_to_item(ix);
        }
        let me = cx.entity().downgrade();
        let flash = target.action;
        cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1800))
                .await;
            let _ = me.update(cx, |this, cx| {
                if this.flash == Some(flash) {
                    this.flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// How far the key list is scrolled (px), so a test can see a clash link's jump (ST-3).
    pub fn key_list_offset(&self) -> f32 {
        f32::from(self.keys_scroll.offset().y)
    }

    /// ST-4: the actions the filter lets through, in catalog order.
    pub fn visible_actions(&self) -> Vec<&'static str> {
        CATALOG
            .iter()
            .filter(|d| self.matches(d))
            .map(|d| d.action)
            .collect()
    }

    fn matches(&self, d: &ActionDoc) -> bool {
        row_matches(d, &self.overrides.keys_for(d), &self.query)
    }

    /// The filter's rows, grouped the way the editor lists them. A category with nothing
    /// left to show is dropped rather than drawn empty.
    fn visible_groups(&self) -> Vec<(Category, Vec<&'static ActionDoc>)> {
        let mut out = Vec::new();
        for &c in Category::all() {
            let rows: Vec<_> = by_category(c).filter(|d| self.matches(d)).collect();
            if !rows.is_empty() {
                out.push((c, rows));
            }
        }
        out
    }

    /// Settings → Appearance: put `font` in force in every window and remember it.
    pub fn choose_font(&mut self, font: UiFont, cx: &mut Context<Self>) {
        // The radio shows the result; only a failed save needs words.
        self.status = match appearance::set_font(cx, font, &self.config_dir) {
            Err(e) => Some(
                format!(
                    "{} applies now, but could not save {}: {e}",
                    font.label(),
                    appearance::FILE
                )
                .into(),
            ),
            Ok(()) => None,
        };
        cx.notify();
    }

    /// Settings → Appearance: put the right sidebar back to its default width (RS-4).
    pub fn reset_sidebar_width(&mut self, cx: &mut Context<Self>) {
        self.status = match appearance::reset_right_sidebar_width(cx, &self.config_dir) {
            Ok(()) => None,
            Err(e) => Some(format!("Could not save settings.json: {e}").into()),
        };
        cx.notify();
    }

    /// Settings → Appearance: how many children a reference shows inline (LR-4).
    pub fn choose_ref_children(&mut self, count: usize, cx: &mut Context<Self>) {
        self.status = match appearance::set_ref_children(cx, count, &self.config_dir) {
            Err(e) => Some(
                format!(
                    "{count} applies now, but could not save {}: {e}",
                    appearance::FILE
                )
                .into(),
            ),
            Ok(()) => None,
        };
        cx.notify();
    }

    /// Settings → Appearance: remove every pinned page (SB-3). The pages are untouched;
    /// only the sidebar's local order changes.
    pub fn clear_pins(&mut self, cx: &mut Context<Self>) {
        self.status = match appearance::clear_pinned(cx) {
            Ok(()) => None,
            Err(e) => Some(
                format!(
                    "The pins are gone for this session, but could not save {}: {e}",
                    appearance::FILE
                )
                .into(),
            ),
        };
        cx.notify();
    }

    /// Settings → Appearance: switch density in every window and remember it (ALL-5).
    pub fn choose_density(&mut self, density: Density, cx: &mut Context<Self>) {
        self.status = match appearance::set_density(cx, density, &self.config_dir) {
            Err(e) => Some(
                format!(
                    "{} applies now, but could not save {}: {e}",
                    density.label(),
                    appearance::FILE
                )
                .into(),
            ),
            Ok(()) => None,
        };
        cx.notify();
    }

    /// Settings → Appearance: switch the indent step in every window and remember it.
    pub fn choose_indent(&mut self, indent: Indent, cx: &mut Context<Self>) {
        self.status = match appearance::set_indent(cx, indent, &self.config_dir) {
            Err(e) => Some(
                format!(
                    "{} applies now, but could not save {}: {e}",
                    indent.label(),
                    appearance::FILE
                )
                .into(),
            ),
            Ok(()) => None,
        };
        cx.notify();
    }
}

/// gpui sends modifier presses through as keystrokes with no real key on some layouts.
fn is_modifier_only(ks: &Keystroke) -> bool {
    matches!(
        ks.key.as_str(),
        "" | "cmd" | "ctrl" | "alt" | "shift" | "function" | "fn" | "capslock"
    )
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let density = appearance::density(cx);
        let font = appearance::ui_font(cx);

        let body = match self.tab {
            Tab::Keybindings => self.render_keybindings(cx).into_any_element(),
            Tab::Shortcuts => self.render_shortcuts(cx).into_any_element(),
            Tab::Appearance => self.render_appearance(cx).into_any_element(),
            Tab::Sync => self.render_sync(cx).into_any_element(),
            Tab::Help => self.render_help(cx).into_any_element(),
        };
        // One debug selector per element: the body names the font, the rail the density.
        let body = div()
            .debug_selector(move || format!("settings-ui-font-{}", font.key()))
            .flex()
            .flex_col()
            .flex_1()
            // A flex child's automatic minimum is its content height. Without this the
            // scrolling list would grow the column past the window instead of scrolling
            // inside it, pushing the status line out of reach.
            .min_h_0()
            .child(body);

        // ST-5: the tabs are a list down the left side, as System Settings has it, each
        // with a glyph so the list reads at a glance.
        let tabs: Vec<_> = Tab::all()
            .iter()
            .map(|&t| {
                let on = t == self.tab;
                div()
                    .id(SharedString::from(format!("nav-{}", t.key())))
                    .debug_selector(move || format!("settings-nav-{}", t.key()))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py(px(if density == Density::Compact { 5. } else { 8. }))
                    .rounded(px(crate::theme::RADIUS_CONTROL))
                    .cursor_pointer()
                    .text_size(px(crate::theme::CONTROL_SIZE))
                    .when(on, |d| d.bg(th.line_soft).text_color(th.link))
                    .when(!on, |d| d.text_color(th.ink).hover(|x| x.bg(th.hover)))
                    .child(
                        div()
                            .w(px(18.))
                            .flex_none()
                            .text_color(if on { th.link } else { th.muted })
                            .child(t.icon()),
                    )
                    .child(t.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(t, cx)))
            })
            .collect();
        let rail = div()
            // Names the density in force, for tests (the root names the font; an element
            // carries only one debug selector).
            .debug_selector(move || format!("settings-ui-density-{}", density.key()))
            .w(px(RAIL_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap_0p5()
            .px_3()
            .py_4()
            .border_r_1()
            .border_color(th.line)
            .bg(th.shelf)
            .child(
                div()
                    .px_2()
                    .pb_3()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Settings"),
            )
            .children(tabs);

        // The pane title sits in the same column as its cards, not at the window's edge.
        let header = div().px_6().pt_6().pb_1().child(
            column().flex().flex_col().gap_1().child(
                div()
                    .text_size(px(crate::theme::H1_SIZE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tab.label()),
            ),
        );

        div()
            .debug_selector(|| "settings".into())
            .track_focus(&self.focus)
            .key_context("Settings")
            .size_full()
            .flex()
            .flex_col()
            .bg(th.paper)
            .text_color(th.ink)
            .text_size(px(density.text_size()))
            .line_height(px(density.line_height()))
            .ui_font_style(appearance::ui_font(cx))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(rail)
                    .child(
                        div()
                            .debug_selector(|| "settings-pane".into())
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.))
                            .child(header)
                            .child(body),
                    ),
            )
            .children(self.warning.clone().map(|w| {
                div()
                    .debug_selector(|| "settings-warning".into())
                    .px_6()
                    .py_2()
                    .border_t_1()
                    .border_color(th.line)
                    .bg(th.banner_bg)
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.warning)
                    .child(w)
            }))
            .children(self.status.clone().map(|s| {
                div()
                    .debug_selector(|| "settings-status".into())
                    .px_6()
                    .py_2()
                    .border_t_1()
                    .border_color(th.line)
                    .bg(th.well)
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(s)
            }))
            .children(self.save_error.clone().map(|e| {
                div()
                    .debug_selector(|| "settings-unsaved".into())
                    .px_6()
                    .py_2()
                    .border_t_1()
                    .border_color(th.line)
                    .bg(th.banner_bg)
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.warning)
                    .child(e)
            }))
    }
}

/// ST-4: a row matches when every whitespace-separated term appears in its name, its
/// description, the action's name or the keys in force (so "⇧⌘K" and "cmd-shift-k" both
/// work).
fn row_matches(d: &ActionDoc, keys: &str, query: &str) -> bool {
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    if terms.is_empty() {
        return true;
    }
    let haystack = format!(
        "{} {} {} {} {}",
        d.label,
        d.help,
        d.action,
        keys,
        pretty_keys(keys)
    )
    .to_lowercase();
    terms.iter().all(|t| haystack.contains(t))
}

/// A card (ST-1): one per category, a hairline border on the page's surface.
fn card(th: &Theme) -> Div {
    div()
        .w_full()
        .max_w(px(CONTENT_WIDTH))
        .mx_auto()
        // A card is as tall as its rows; the pane scrolls instead of squeezing it.
        .flex_shrink_0()
        .rounded(px(crate::theme::RADIUS_INPUT))
        .border_1()
        .border_color(th.line)
        .bg(th.well)
        .overflow_hidden()
}

/// The caption at the top of a card.
fn card_title(th: &Theme, label: &str) -> Div {
    div()
        .px_4()
        .pt_3()
        .pb_1p5()
        .text_size(px(crate::theme::CAPTION_SIZE))
        .text_color(th.muted)
        .font_weight(FontWeight::MEDIUM)
        .child(label.to_string())
}

/// One fact on a card: a label and its value, the value free to wrap.
fn fact_row(label: &'static str, value: String, th: &Theme) -> Div {
    div()
        .flex()
        .items_baseline()
        .justify_between()
        .gap_4()
        .child(div().text_size(px(crate::theme::BODY_SIZE)).child(label))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .text_right()
                .child(value),
        )
}

/// One row inside a card: the hairline between rows is the point of ST-1.
fn card_row<E: Styled + gpui::prelude::FluentBuilder>(
    row: E,
    i: usize,
    th: &Theme,
    density: Density,
) -> E {
    row.px_4()
        .py(px(if density == Density::Compact { 7. } else { 11. }))
        .when(i > 0, |d| d.border_t_1().border_color(th.line))
}

/// A row of a key-list card. The card's body is drawn by its rows because the rows are
/// direct children of the scroll region, so a clash link can scroll to the row itself
/// (ST-3); the header and the last row round the card's ends.
fn key_card_row<E: Styled + gpui::prelude::FluentBuilder>(
    row: E,
    last: bool,
    th: &Theme,
    density: Density,
) -> E {
    row.w_full()
        .max_w(px(CONTENT_WIDTH))
        .mx_auto()
        .flex_shrink_0()
        .px_4()
        .py(px(if density == Density::Compact { 7. } else { 11. }))
        .border_l_1()
        .border_r_1()
        .border_t_1()
        .border_color(th.line)
        .when(last, |d| d.border_b_1().rounded_b_lg())
}

/// The top of a key-list card: the category in a rounded, bordered strip.
fn card_top(th: &Theme, label: &str) -> Div {
    column()
        .debug_selector(move || format!("settings-card-{label}"))
        .bg(th.well)
        .border_1()
        .border_b_0()
        .border_color(th.line)
        .rounded_t_lg()
        .px_4()
        .pt_3()
        .pb_1p5()
        .text_size(px(crate::theme::CAPTION_SIZE))
        .text_color(th.muted)
        .font_weight(FontWeight::MEDIUM)
        .child(label.to_string())
}

/// The scrolling key list. Unlike [`pane`], its direct children are the category headers and
/// the rows themselves: `ScrollHandle::scroll_to_item` only targets direct children, and a
/// clash link must reveal the destination *row*, not merely its category (ST-3).
fn key_pane(
    id: &'static str,
    selector: &'static str,
    handle: &ScrollHandle,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || selector.into())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_6()
        .py_5()
        .track_scroll(handle)
}

/// A row of an Appearance card. Like the key list, the cards' rows are direct children of
/// the scroll region (the header and the last row round the card's ends); a row is a touch
/// tighter than a key row so all four preference cards fit one desktop screen.
fn appearance_row<E: Styled + gpui::prelude::FluentBuilder>(row: E, last: bool, th: &Theme) -> E {
    row.w_full()
        .max_w(px(CONTENT_WIDTH))
        .mx_auto()
        .flex_shrink_0()
        .px_4()
        .py(px(8.))
        .border_l_1()
        .border_r_1()
        .border_t_1()
        .border_color(th.line)
        .when(last, |d| d.border_b_1().rounded_b_lg())
}

/// The scroll region every pane shares: one centred readable column.
fn pane(id: &'static str, selector: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || selector.into())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .px_6()
        .py_5()
        .gap_4()
}

/// A child of [`pane`]: fills the pane, but never past the readable width.
fn column() -> Div {
    div()
        .w_full()
        .max_w(px(CONTENT_WIDTH))
        .mx_auto()
        .flex_shrink_0()
}

/// A drawn radio mark: "●"/"○" would come from whichever font is in force, and IBM Plex
/// Sans has neither, so the fallback drew them at another size.
fn radio(on: bool, th: &Theme) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(if on { th.link } else { th.line_strong })
        .flex()
        .items_center()
        .justify_center()
        .when(on, |d| {
            d.child(div().size(px(8.)).rounded_full().bg(th.link))
        })
}

impl SettingsView {
    /// The filter field (ST-4). Click it, type, and the key list below narrows to the
    /// actions whose name, description or keys match.
    fn render_search(&mut self, cx: &mut Context<Self>) -> Div {
        let th = theme(cx).clone();
        column()
            .debug_selector(|| "settings-search".into())
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded(px(crate::theme::RADIUS_INPUT))
            .border_1()
            .border_color(th.line)
            .bg(th.well)
            .child(
                div()
                    .flex_none()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child("Filter"),
            )
            .child(self.search.clone())
    }

    fn render_keybindings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let density = appearance::density(cx);
        let conflicts = self.overrides.conflicts();
        let changed = self.overrides.len();
        let groups = self.visible_groups();
        let shown: usize = groups.iter().map(|(_, rows)| rows.len()).sum();

        let hint = if self.query.trim().is_empty() {
            if changed == 0 {
                "Click a key, then press the new one.".to_string()
            } else {
                format!("Click a key, then press the new one. {changed} changed from the default.")
            }
        } else {
            format!(
                "{shown} of {} shortcuts match “{}”.",
                CATALOG.len(),
                self.query.trim()
            )
        };

        let mut list = key_pane("settings-keys", "settings-keys", &self.keys_scroll)
            .child(
                column()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(hint),
                    )
                    .child(
                        quiet_control("reset-all", "Reset All", &th)
                            .debug_selector(|| "settings-reset-all".into())
                            .on_click(cx.listener(|this, _, _, cx| this.reset_all(cx))),
                    ),
            )
            .child(self.render_search(cx).mt_3());

        if groups.is_empty() {
            list = list.child(
                column()
                    .debug_selector(|| "settings-no-match".into())
                    .mt_4()
                    .py_4()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(format!("No shortcut matches “{}”.", self.query.trim())),
            );
        }

        for (c, rows) in &groups {
            let category = *c;
            list = list.child(card_top(&th, category.label()).mt_4());
            for (i, d) in rows.iter().enumerate() {
                let row = self.render_key_row(d, &conflicts, cx);
                list = list.child(key_card_row(row, i + 1 == rows.len(), &th, density));
            }
        }
        list
    }

    fn render_key_row(
        &self,
        d: &'static ActionDoc,
        conflicts: &[(&'static ActionDoc, &'static ActionDoc)],
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let th = theme(cx).clone();
        let capturing = self.capture.action().map(|c| c.action) == Some(d.action);
        let keys = self.overrides.keys_for(d);
        let changed = self.overrides.is_changed(d.action);
        // An entry gpui cannot bind is shown as the user wrote it, with the reason, instead of
        // pretending it is the binding (keymap machine doc, issue 1).
        let invalid = match self.overrides.entry(d.action) {
            Some(Override::Invalid { raw, why }) => Some((raw.clone(), why.clone())),
            _ => None,
        };
        let flashed = self.flash == Some(d.action);
        // ST-3: whoever else answers to these keys in the same place, said under the row
        // itself rather than in a banner at the top of the window.
        let others: Vec<&'static ActionDoc> = conflicts
            .iter()
            .filter_map(|(a, b)| {
                if a.action == d.action {
                    Some(*b)
                } else if b.action == d.action {
                    Some(*a)
                } else {
                    None
                }
            })
            .collect();

        // ST-2: the key chip *is* the control. Click it and it waits for the next
        // combination; there is no separate Change button to hunt for.
        let chip = div()
            .id(SharedString::from(format!("chip-{}", d.action)))
            .debug_selector(move || format!("settings-chip-{}", d.action))
            .px_2()
            .py_1()
            .rounded(px(crate::theme::RADIUS_CONTROL))
            .border_1()
            .border_color(if capturing || invalid.is_some() {
                th.link
            } else {
                th.line
            })
            .bg(if capturing {
                th.banner_bg
            } else {
                th.line_soft
            })
            .text_color(if invalid.is_some() {
                th.warning
            } else if capturing {
                th.link
            } else {
                th.ink
            })
            .text_size(px(crate::theme::CAPTION_SIZE))
            .cursor_pointer()
            .hover(|x| x.border_color(th.link))
            .child(if capturing {
                "Press keys · Esc cancels".to_string()
            } else if let Some((raw, _)) = &invalid {
                raw.clone()
            } else {
                pretty_keys(&keys)
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.capture.action().map(|c| c.action) == Some(d.action) {
                    this.cancel_capture();
                    this.status = Some("Left unchanged.".into());
                    cx.notify();
                } else {
                    this.start_capture(d, window, cx);
                }
            }));

        // A changed row carries a small undo mark; it puts the catalog default back.
        let reset = changed.then(|| {
            div()
                .id(SharedString::from(format!("reset-{}", d.action)))
                .debug_selector(move || format!("settings-reset-{}", d.action))
                .flex_none()
                .px_1()
                .py_0p5()
                .rounded(px(crate::theme::RADIUS_CONTROL))
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .cursor_pointer()
                .hover(|x| x.bg(th.hover).text_color(th.ink))
                .child("↺")
                .on_click(cx.listener(move |this, _, _, cx| this.reset_one(d, cx)))
        });

        let mut row = div()
            // The row's id scopes everything inside it (chips, reset marks, clash links):
            // two rows may link to the same action, and their elements must not share one
            // GPUI element id.
            .id(SharedString::from(format!("row-{}", d.action)))
            .debug_selector(move || format!("settings-row-{}", d.action))
            .flex()
            .flex_col()
            .bg(th.well)
            .hover(|x| x.bg(th.hover))
            .when(flashed, |x| x.bg(th.banner_bg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(div().child(d.label))
                            .when(!d.help.is_empty(), |c| {
                                c.child(
                                    div()
                                        .text_size(px(crate::theme::CAPTION_SIZE))
                                        .text_color(th.muted)
                                        .child(d.help),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .children(reset)
                            .child(chip),
                    ),
            );

        if let Some((_, why)) = &invalid {
            row = row.child(
                div()
                    .debug_selector(move || format!("settings-invalid-{}", d.action))
                    .flex_wrap()
                    .pt_1()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.warning)
                    .child(format!(
                        "Not a shortcut: {why}. {} is in force.",
                        pretty_keys(&keys)
                    )),
            );
        }
        if !others.is_empty() {
            let mut line = div()
                .debug_selector(move || format!("settings-clash-{}", d.action))
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .pt_1()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.warning)
                .child("Also used by");
            let source = d.action;
            for (i, other) in others.iter().copied().enumerate() {
                if i > 0 {
                    line = line.child(div().child("and"));
                }
                line = line.child(
                    div()
                        .id(SharedString::from(format!(
                            "clash-{}-{}",
                            source, other.action
                        )))
                        .debug_selector(move || {
                            format!("settings-clash-link-{}-{}", source, other.action)
                        })
                        .underline()
                        .cursor_pointer()
                        .hover(|x| x.text_color(th.ink))
                        .child(other.label)
                        .on_click(cx.listener(move |this, _, _, cx| this.jump_to(other, cx))),
                );
            }
            row = row.child(line);
        }
        row
    }

    /// The cheat sheet: everything bound right now, grouped the same way as the editor.
    /// The same filter narrows it, so "what did I put on cmd-shift" has one answer.
    fn render_shortcuts(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let density = appearance::density(cx);
        let groups = self.visible_groups();
        let shown: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
        let mut list = key_pane(
            "settings-shortcuts",
            "settings-shortcuts",
            &self.keys_scroll,
        );
        if !self.query.trim().is_empty() {
            list = list.child(
                column()
                    .debug_selector(|| "settings-search-count".into())
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(format!(
                        "{shown} of {} shortcuts match “{}”.",
                        CATALOG.len(),
                        self.query.trim()
                    )),
            );
        }
        list = list.child(
            self.render_search(cx)
                .when(!self.query.trim().is_empty(), |d| d.mt_3()),
        );

        if groups.is_empty() {
            list = list.child(
                column()
                    .debug_selector(|| "settings-no-match".into())
                    .mt_4()
                    .py_4()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(format!("No shortcut matches “{}”.", self.query.trim())),
            );
        }

        for (c, rows) in &groups {
            let category = *c;
            list = list.child(card_top(&th, category.label()).mt_4());
            for (i, d) in rows.iter().enumerate() {
                let row = div()
                    .debug_selector({
                        let action = d.action;
                        move || format!("settings-shortcut-{action}")
                    })
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap_4()
                    .bg(th.well)
                    .hover(|x| x.bg(th.hover))
                    .child(div().text_size(px(crate::theme::BODY_SIZE)).child(d.label))
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(pretty_keys(&self.overrides.keys_for(d))),
                    );
                list = list.child(key_card_row(row, i + 1 == rows.len(), &th, density));
            }
        }
        list
    }

    fn render_appearance(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let current = appearance::ui_font(cx);

        let mut list = pane("settings-appearance", "settings-appearance")
            .gap_0()
            .child(card_top(&th, "Font"));
        for (i, &f) in UiFont::ALL.iter().enumerate() {
            let on = f == current;
            list = list.child(appearance_row(
                div()
                    .id(SharedString::from(format!("font-{}", f.key())))
                    .debug_selector(move || format!("settings-font-{}", f.key()))
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(on, |d| d.bg(th.line_soft))
                    .cursor_pointer()
                    .hover(|x| x.bg(th.hover))
                    .child(radio(on, &th))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(div().child(f.label()))
                            // Each sample in its own font, whichever one is in force.
                            .child(
                                div()
                                    .ui_font_style(f)
                                    .text_size(px(crate::theme::CAPTION_SIZE))
                                    .text_color(th.muted)
                                    .child("Bob dug a big bog by the pond, quickly."),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_font(f, cx))),
                i + 1 == UiFont::ALL.len(),
                &th,
            ));
        }

        let sidebar_width = appearance::right_sidebar_width(cx);
        let current_density = appearance::density(cx);
        list = list.child(card_top(&th, "Density").mt_4());
        for (i, &d) in Density::ALL.iter().enumerate() {
            let on = d == current_density;
            let th = th.clone();
            list = list.child(appearance_row(
                div()
                    .id(SharedString::from(format!("density-{}", d.key())))
                    .debug_selector(move || format!("settings-density-{}", d.key()))
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(on, |x| x.bg(th.line_soft))
                    .cursor_pointer()
                    .hover(|x| x.bg(th.hover))
                    .child(radio(on, &th))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(div().child(d.label())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_density(d, cx))),
                i + 1 == Density::ALL.len(),
                &th,
            ));
        }

        // How far children step right of their parent (DESIGN.md §3.1), a local preference
        // like the density. Compact panes keep their own step whatever this says.
        let current_indent = appearance::indent(cx);
        list = list.child(card_top(&th, "Indent").mt_4());
        for (i, &d) in Indent::ALL.iter().enumerate() {
            let on = d == current_indent;
            let th = th.clone();
            list = list.child(appearance_row(
                div()
                    .id(SharedString::from(format!("indent-{}", d.key())))
                    .debug_selector(move || format!("settings-indent-{}", d.key()))
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(on, |x| x.bg(th.line_soft))
                    .cursor_pointer()
                    .hover(|x| x.bg(th.hover))
                    .child(radio(on, &th))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .child(div().child(d.label()))
                            .child(
                                div()
                                    .text_size(px(crate::theme::CAPTION_SIZE))
                                    .text_color(th.muted)
                                    .child(format!("{} px per level", d.step())),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_indent(d, cx))),
                i + 1 == Indent::ALL.len(),
                &th,
            ));
        }

        // LR-4: how many children a Linked Reference shows inline, then PG-1's attribute
        // rows, both local preferences.
        let current_children = appearance::ref_children(cx);
        list = list.child(card_top(&th, "References").mt_4());
        list = list.child(appearance_row(
            div()
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .child("0 keeps every child behind the “+N more” chip."),
            false,
            &th,
        ));
        list = list.child(appearance_row(
            {
                let mut row = div().flex().items_center().gap_2().child(
                    div()
                        .flex_none()
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .child("Children shown"),
                );
                for n in 0..=appearance::REF_CHILDREN_MAX {
                    let on = n == current_children;
                    let th = th.clone();
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("ref-children-{n}")))
                            .debug_selector(move || format!("settings-ref-children-{n}"))
                            .w(px(40.))
                            .py_1()
                            .flex()
                            .justify_center()
                            .rounded(px(crate::theme::RADIUS_CONTROL))
                            .border_1()
                            .border_color(if on { th.link } else { th.line })
                            .when(on, |d| d.bg(th.line_soft).text_color(th.link))
                            .when(!on, |d| d.text_color(th.muted))
                            .cursor_pointer()
                            .hover(|x| x.bg(th.hover))
                            .child(n.to_string())
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.choose_ref_children(n, cx)),
                            ),
                    );
                }
                row
            },
            false,
            &th,
        ));
        // SB-3: the pages pinned to the sidebar live in settings.json.
        let pinned = appearance::pinned(cx).len();
        list = list.child(card_top(&th, "Pinned pages").mt_4());
        list = list.child(appearance_row(
            div()
                .debug_selector(|| "settings-pinned".into())
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .text_color(if pinned == 0 { th.muted } else { th.ink })
                        .child(match pinned {
                            0 => "No pinned pages".to_string(),
                            1 => "1 pinned page".to_string(),
                            n => format!("{n} pinned pages"),
                        }),
                )
                .when(pinned > 0, |d| {
                    d.child(
                        quiet_control("settings-clear-pinned", "Clear", &th)
                            .debug_selector(|| "settings-clear-pinned".into())
                            .on_click(cx.listener(|this, _, _, cx| this.clear_pins(cx))),
                    )
                }),
            true,
            &th,
        ));

        // RS-4: the width the sidebar's drag handle set. The handle is the control; this
        // row reports it and offers a way back to the default.
        list = list.child(card_top(&th, "Right sidebar").mt_4());
        list = list.child(appearance_row(
            div()
                .debug_selector(move || {
                    format!("settings-sidebar-width-{}", sidebar_width.round() as i32)
                })
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.muted)
                .child(format!(
                    "Width: {} px, set by dragging the sidebar's left edge.",
                    sidebar_width.round() as i32
                )),
            false,
            &th,
        ));
        list.child(appearance_row(
            quiet_control(
                "sidebar-reset",
                format!(
                    "Reset to {} px",
                    appearance::RIGHT_SIDEBAR_DEFAULT_WIDTH as i32
                ),
                &th,
            )
            .debug_selector(|| "settings-sidebar-reset".into())
            .on_click(cx.listener(|this, _, _, cx| this.reset_sidebar_width(cx))),
            true,
            &th,
        ))
    }

    /// The tour: one readable column of prose, with the topics as chips above it. The old
    /// second column of topics is gone now that the tabs themselves are a list at the left
    /// (ST-5); the screenshots get the full 720 px column instead of a clipped half.
    fn render_help(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let selected = self.topic.min(TOPICS.len().saturating_sub(1));
        let nav: Vec<_> = TOPICS
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let on = i == selected;
                div()
                    .id(SharedString::from(format!("help-nav-{i}")))
                    .debug_selector(move || format!("help-nav-{i}"))
                    .px_2()
                    .py_1p5()
                    .rounded(px(crate::theme::RADIUS_CONTROL))
                    .cursor_pointer()
                    .text_size(px(crate::theme::CONTROL_SIZE))
                    .when(on, |d| d.bg(th.line_soft).text_color(th.link))
                    .when(!on, |d| {
                        d.text_color(th.muted)
                            .hover(|x| x.bg(th.hover).text_color(th.ink))
                    })
                    .child(t.title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.topic = i;
                        cx.notify();
                    }))
            })
            .collect();

        let t = &TOPICS[selected];
        let shortcuts: Vec<_> = t
            .actions
            .iter()
            .filter_map(|a| doc(a))
            .map(|d| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded(px(crate::theme::RADIUS_CONTROL))
                            .bg(th.line_soft)
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .child(pretty_keys(&self.overrides.keys_for(d))),
                    )
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(d.label),
                    )
            })
            .collect();

        // `w_full` first, then `max_w`: a flex child's width is only definite once it is
        // told to fill its parent, and without a definite width the prose does not wrap.
        let mut body = column()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().flex().flex_wrap().gap_1().children(nav))
            .child(
                div()
                    .pt_3()
                    .text_size(px(crate::theme::H1_SIZE))
                    .font_weight(FontWeight::BOLD)
                    .child(t.title),
            )
            .child(div().text_color(th.muted).child(t.summary));
        for p in t.body {
            body = body.child(div().text_size(px(crate::theme::BODY_SIZE)).child(*p));
        }
        if !shortcuts.is_empty() {
            body = body.child(div().flex().flex_wrap().gap_3().pt_1().children(shortcuts));
        }
        if let Some(image) = shot_image(t) {
            body = body.child(
                div()
                    .debug_selector(|| "help-shot".into())
                    .w(px(HELP_SHOT_WIDTH))
                    .flex_none()
                    .pt_2()
                    .child(
                        img(image)
                            .w(px(HELP_SHOT_WIDTH))
                            .h(px(HELP_SHOT_HEIGHT))
                            .rounded(px(crate::theme::RADIUS_CONTROL))
                            .border_1()
                            .border_color(th.line),
                    ),
            );
        }
        pane("help-detail", "help-detail").child(body)
    }

    /// Setup is provided by the app for the open graph. Headless component tests can
    /// still render transport facts without installing a coordinator (§12, ST-5).
    fn render_sync(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if let Some(setup) = cx.try_global::<crate::sync_setup::SetupHandle>() {
            return pane("settings-sync-pane", "settings-sync-pane")
                .child(setup.0.clone())
                .into_any_element();
        }
        let th = theme(cx).clone();
        let density = appearance::density(cx);
        let facts = sync_runner::facts(cx);
        let status = if facts.is_on() {
            "Sync is on for this graph."
        } else {
            "Sync is off for this graph."
        };
        let transport = facts.target.clone().unwrap_or_else(|| "Off".to_string());
        let encryption = if facts.encrypted {
            "End-to-end encrypted"
        } else {
            "Not encrypted: the relay can read your notes"
        };

        let mut this_graph = card(&th)
            .debug_selector(|| "settings-sync".into())
            .child(card_title(&th, "This graph"))
            .child(card_row(
                div().text_size(px(crate::theme::BODY_SIZE)).child(status),
                0,
                &th,
                density,
            ))
            .child(card_row(
                fact_row("Transport", transport, &th)
                    .debug_selector(|| "settings-sync-transport".into()),
                1,
                &th,
                density,
            ))
            .child(card_row(
                fact_row(
                    "Graph id",
                    facts.graph.clone().unwrap_or_else(|| "unknown".to_string()),
                    &th,
                ),
                2,
                &th,
                density,
            ))
            .when(facts.is_on(), |c| {
                c.child(card_row(
                    fact_row("Encryption", encryption.to_string(), &th),
                    3,
                    &th,
                    density,
                ))
            });

        this_graph = this_graph.child(card_row(
            div()
                .text_color(th.muted)
                .child("Open a graph to set up sync."),
            4,
            &th,
            density,
        ));
        pane("settings-sync-pane", "settings-sync-pane")
            .child(this_graph)
            .into_any_element()
    }
}
