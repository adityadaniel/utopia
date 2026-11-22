//! Timing and platform preferences shared by UI transitions (DESIGN.md §2.6).

pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// macOS "Reduce motion" (DESIGN.md §2.6). Read when a transition starts.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2's msg_send! checks a cfg this crate does not declare
pub(crate) fn reduce_motion() -> bool {
    use objc::runtime::{Class, Object, BOOL, NO};
    use objc::{msg_send, sel, sel_impl};
    let Some(cls) = Class::get("NSWorkspace") else {
        return false;
    };
    // SAFETY: `sharedWorkspace` and `accessibilityDisplayShouldReduceMotion` are plain
    // getters on AppKit's singleton (macOS 10.12+); a nil workspace is checked first.
    unsafe {
        let ws: *mut Object = msg_send![cls, sharedWorkspace];
        if ws.is_null() {
            return false;
        }
        let on: BOOL = msg_send![ws, accessibilityDisplayShouldReduceMotion];
        on != NO
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn reduce_motion() -> bool {
    false
}
