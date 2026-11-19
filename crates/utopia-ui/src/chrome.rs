//! The window frame (DESIGN.md §3.16): the system title bar is transparent, so the
//! traffic lights draw over the app's own top row. Whatever sits under them at the left
//! edge of the window makes room for them.

use gpui::{point, px, App, Global, TitlebarOptions, Window};

use crate::theme::{TRAFFIC_LIGHTS_WIDTH, TRAFFIC_LIGHT_X, TRAFFIC_LIGHT_Y};

/// The title bar options for every Utopia window. `title` is not drawn, but the Window
/// menu and Mission Control still show it.
pub fn titlebar(title: &str) -> TitlebarOptions {
    TitlebarOptions {
        title: Some(title.to_string().into()),
        appears_transparent: true,
        traffic_light_position: Some(point(px(TRAFFIC_LIGHT_X), px(TRAFFIC_LIGHT_Y))),
    }
}

/// True while the traffic lights are drawn over the window's content. Full screen hides
/// them (they come back only with the menu bar), and only macOS draws them.
pub fn lights_shown(window: &Window) -> bool {
    cfg!(target_os = "macos") && !window.is_fullscreen()
}

/// The room, from the window's left edge, that the traffic lights take: their left offset
/// plus their width, or 0 when they are not drawn.
pub fn lights_room(window: &Window) -> f32 {
    if lights_shown(window) {
        TRAFFIC_LIGHT_X + TRAFFIC_LIGHTS_WIDTH
    } else {
        0.
    }
}

/// The least left padding a top bar in the main column needs so that its first control
/// starts 12 pt after the traffic lights, where the workspace square starts when the
/// sidebar is open. The workspace sets it each frame (0 while the sidebar is open, because
/// the sidebar holds the lights); views that draw their own top bar (graph, PDF, tasks)
/// read it, so they need not know about the sidebar.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct LeadingInset(pub f32);

impl Global for LeadingInset {}

/// The gap after [`lights_room`]: with the 4 pt that `TRAFFIC_LIGHTS_WIDTH` holds past the
/// zoom button, the next control starts 12 pt after it, as the workspace row's gap does.
pub const LIGHTS_GAP: f32 = 8.;

/// A top bar's left padding: its own, or more when the traffic lights are over it.
pub fn leading_padding(own: f32, cx: &App) -> f32 {
    own.max(cx.try_global::<LeadingInset>().map_or(0., |i| i.0))
}

/// Double-clicking an empty part of a top row does what the user set for a title bar
/// (zoom or minimise, System Settings → Desktop & Dock).
pub fn on_titlebar_double_click(event: &gpui::ClickEvent, window: &mut Window) {
    if matches!(event, gpui::ClickEvent::Mouse(m) if m.up.click_count == 2) {
        window.titlebar_double_click();
    }
}
