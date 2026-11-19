//! `UTOPIA_SHOT_SCRIPT`: drive the real window and capture it to PNG, so the UI can be
//! checked by eye (and by agents) without Screen Recording permission — an app may always
//! capture its own windows.
//!
//! The script is a list of steps separated by newlines or `;`:
//! - `keys <k1> <k2> …` dispatches keystrokes in GPUI syntax (`cmd-k`, `escape`, `tab`)
//! - `type <text>` types text into whatever has focus
//! - `wait <ms>` lets the app run (sync, debounce, animations)
//! - `scroll <dy>` scrolls the wheel by `dy` pixels at the centre of the window (negative
//!   moves the content up, i.e. scrolls down, as a trackpad does)
//! - `click <x> <y>` clicks the left button at a point in the window's content, in points
//!   from its top left (a Retina capture is twice that; the title bar is transparent, so the
//!   content starts at the window's top edge, DESIGN.md §3.18),
//!   for controls with no shortcut
//! - `hover <x> <y>` (or `move <x> <y>`) moves the pointer there without pressing, for
//!   states that only show on hover (a block's drag handle, PG-4, or a graph node, GR-2)
//! - `shot <path.png>` captures the window
//! - `drop <file>` drops a file on the window, as from Finder
//! - `ws <command> [args…]` runs a workspace command the app exposes for scripts
//!   (e.g. `ws open-pdf`, `ws highlight <page> <x> <y> <w> <h>` for an area,
//!   `ws select <page> <x0> <y0> <x1> <y1>` for a text selection)
//! - `target main|last` points the steps after it at the main window or at the most
//!   recently opened one, so a second window (Settings, Cmd+,) can be driven and captured
//!
//! The app quits after the last step.

use std::time::Duration;

use gpui::{AnyWindowHandle, AsyncApp, Keystroke, Window};

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Keys(Vec<String>),
    Type(String),
    Wait(u64),
    Scroll(f32),
    Click(f32, f32),
    Move(f32, f32),
    Hover(f32, f32),
    Shot(String),
    Drop(String),
    Ws(Vec<String>),
    /// Switch which window later steps drive: `main` or `last`.
    Target(String),
}

/// How `drop` and `ws` steps reach the workspace.
pub trait Hooks {
    fn drop_files(&self, paths: &[std::path::PathBuf], window: &mut Window, cx: &mut gpui::App);
    fn ws(&self, args: &[String], window: &mut Window, cx: &mut gpui::App);
}

pub fn parse(script: &str) -> Result<Vec<Step>, String> {
    script
        .split(['\n', ';'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
            match verb {
                "keys" => Ok(Step::Keys(
                    rest.split_whitespace().map(str::to_string).collect(),
                )),
                "type" => Ok(Step::Type(rest.to_string())),
                "wait" => rest
                    .trim()
                    .parse()
                    .map(Step::Wait)
                    .map_err(|_| format!("bad wait: {line}")),
                "scroll" => rest
                    .trim()
                    .parse()
                    .map(Step::Scroll)
                    .map_err(|_| format!("bad scroll: {line}")),
                "click" | "hover" => match rest
                    .split_whitespace()
                    .map(str::parse::<f32>)
                    .collect::<Vec<_>>()[..]
                {
                    [Ok(x), Ok(y)] if verb == "click" => Ok(Step::Click(x, y)),
                    [Ok(x), Ok(y)] => Ok(Step::Hover(x, y)),
                    _ => Err(format!("bad {verb}: {line}")),
                },
                "move" => match rest
                    .split_whitespace()
                    .map(str::parse::<f32>)
                    .collect::<Vec<_>>()[..]
                {
                    [Ok(x), Ok(y)] => Ok(Step::Move(x, y)),
                    _ => Err(format!("bad move: {line}")),
                },
                "shot" if !rest.trim().is_empty() => Ok(Step::Shot(rest.trim().to_string())),
                "drop" if !rest.trim().is_empty() => Ok(Step::Drop(rest.trim().to_string())),
                "ws" if !rest.trim().is_empty() => Ok(Step::Ws(
                    rest.split_whitespace().map(str::to_string).collect(),
                )),
                "target" if matches!(rest.trim(), "main" | "last") => {
                    Ok(Step::Target(rest.trim().to_string()))
                }
                _ => Err(format!("unknown step: {line}")),
            }
        })
        .collect()
}

/// Run the steps against `handle`, then quit.
pub async fn run(
    steps: Vec<Step>,
    main: AnyWindowHandle,
    hooks: Box<dyn Hooks>,
    cx: &mut AsyncApp,
) {
    let mut handle = main;
    // Bring the window forward so it is presented, then let the first frames land.
    // GPUI stops presenting while a window is occluded, so float it above other apps.
    let _ = cx.update(|cx| cx.activate(true));
    let _ = handle.update(cx, |_, window, _| raise(window));
    cx.background_executor()
        .timer(Duration::from_millis(400))
        .await;
    for step in steps {
        let r = match step {
            Step::Wait(ms) => {
                cx.background_executor()
                    .timer(Duration::from_millis(ms))
                    .await;
                Ok(())
            }
            Step::Drop(path) => handle.update(cx, |_, window, cx| {
                hooks.drop_files(&[std::path::PathBuf::from(&path)], window, cx)
            }),
            Step::Ws(args) => handle.update(cx, |_, window, cx| hooks.ws(&args, window, cx)),
            Step::Target(which) => {
                // `last` is the most recently opened window, which is the one a step like
                // `keys cmd-,` just put on screen.
                let next = if which == "main" {
                    Some(main)
                } else {
                    cx.update(|cx| cx.windows().last().copied()).ok().flatten()
                };
                match next {
                    Some(h) => {
                        handle = h;
                        let _ = handle.update(cx, |_, window, _| raise(window));
                        cx.background_executor()
                            .timer(Duration::from_millis(250))
                            .await;
                        Ok(())
                    }
                    None => {
                        eprintln!("target {which}: no such window");
                        Ok(())
                    }
                }
            }
            Step::Scroll(dy) => {
                // A real wheel event through AppKit, sent outside any window update because
                // GPUI handles it synchronously.
                let target = handle.update(cx, |_, window, _| scroll_target(window));
                if let Ok(Some((w, at))) = target {
                    scroll_wheel(w, at, dy);
                }
                Ok(())
            }
            Step::Click(x, y) => {
                // Real AppKit mouse events, sent outside any window update (as for scroll).
                let target = handle.update(cx, |_, window, _| click_target(window));
                if let Ok(Some((w, height))) = target {
                    // AppKit window coordinates have their origin at the bottom left.
                    click(w, (f64::from(x), height - f64::from(y)));
                }
                Ok(())
            }
            Step::Move(x, y) | Step::Hover(x, y) => {
                // A pointer move with no button, so a hover state shows in the capture.
                let target = handle.update(cx, |_, window, _| click_target(window));
                if let Ok(Some((w, height))) = target {
                    hover(w, (f64::from(x), height - f64::from(y)));
                }
                Ok(())
            }
            Step::Keys(keys) => handle.update(cx, |_, window, cx| {
                for k in keys {
                    match Keystroke::parse(&k) {
                        Ok(ks) => {
                            window.dispatch_keystroke(ks, cx);
                        }
                        Err(e) => eprintln!("shot: bad keystroke {k}: {e}"),
                    }
                }
            }),
            Step::Type(text) => handle.update(cx, |_, window, cx| {
                for ch in text.chars() {
                    let key = if ch == ' ' {
                        "space".to_string()
                    } else {
                        ch.to_string()
                    };
                    if let Ok(mut ks) = Keystroke::parse(&key) {
                        ks.key_char = Some(ch.to_string());
                        window.dispatch_keystroke(ks, cx);
                    }
                }
            }),
            Step::Shot(path) => {
                // Capture reads what the window server shows, so let the display link
                // present a fresh frame first rather than drawing synchronously.
                let view = handle
                    .update(cx, |_, window, _| {
                        window.refresh();
                        ns_view(window)
                    })
                    .ok()
                    .flatten();
                // The display link may be paused (display asleep, window hidden), so also
                // render and present synchronously, outside any window update.
                if let Some(view) = view {
                    present(view);
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                handle.update(cx, |_, window, _| match capture(window, &path) {
                    Ok((w, h)) => println!("shot {path} {w}x{h}"),
                    Err(e) => eprintln!("shot: {path}: {e}"),
                })
            }
        };
        if r.is_err() {
            break;
        }
        // One frame between steps so each step sees the previous one rendered.
        cx.background_executor()
            .timer(Duration::from_millis(60))
            .await;
    }
    let _ = cx.update(|cx| cx.quit());
}

/// The window's NSView, as an address so it can leave the window update.
#[cfg(target_os = "macos")]
fn ns_view(window: &Window) -> Option<usize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => Some(h.ns_view.as_ptr() as usize),
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn ns_view(_: &Window) -> Option<usize> {
    None
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn ns_window(window: &Window) -> Option<*mut objc::runtime::Object> {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    let view = ns_view(window)? as *mut Object;
    // SAFETY: the view is live for the window's lifetime; `window` is a getter.
    Some(unsafe { msg_send![view, window] })
}

/// GPUI's view renders a frame in `displayLayer:`; call it directly.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn present(view: usize) {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    let view = view as *mut Object;
    // SAFETY: called on the main thread with no window borrowed; the view outlives the
    // script because the app quits only after it.
    unsafe {
        let layer: *mut Object = msg_send![view, layer];
        let _: () = msg_send![view, displayLayer: layer];
    }
}

#[cfg(not(target_os = "macos"))]
fn present(_: usize) {}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn raise(window: &Window) {
    use objc::{msg_send, sel, sel_impl};
    if let Some(w) = ns_window(window) {
        // SAFETY: plain setters on a live NSWindow; 3 = NSFloatingWindowLevel.
        unsafe {
            let _: () = msg_send![w, setLevel: 3isize];
            let _: () = msg_send![w, orderFrontRegardless];
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn raise(_: &Window) {}

/// The NSWindow and the screen point (top-left origin) a scripted scroll goes to: the
/// middle of the main pane.
#[cfg(target_os = "macos")]
fn scroll_target(window: &Window) -> Option<(usize, (f64, f64))> {
    // An NSEvent made from a CGEvent has no window, so AppKit hands GPUI the event's
    // screen point (bottom-left origin) as if it were the window location. Place it so
    // that reading lands in the middle of the main pane.
    let size = window.viewport_size();
    let screen_h = core_graphics::display::CGDisplay::main()
        .bounds()
        .size
        .height;
    let x = f64::from(f32::from(size.width * 0.6));
    let y_in_window = f64::from(f32::from(size.height * 0.5));
    let at = (
        x,
        screen_h - (f64::from(f32::from(size.height)) - y_in_window),
    );
    ns_window(window).map(|w| (w as usize, at))
}

#[cfg(not(target_os = "macos"))]
fn scroll_target(_: &Window) -> Option<(usize, (f64, f64))> {
    None
}

/// Post a pixel scroll of `dy` at screen point `at` (top-left origin) to the window.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn scroll_wheel(window: usize, at: (f64, f64), dy: f32) {
    use core_graphics::geometry::CGPoint;
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreateScrollWheelEvent(
            source: *const c_void,
            units: u32,
            wheel_count: u32,
            wheel1: i32,
            ...
        ) -> *mut c_void;
        fn CGEventSetLocation(event: *mut c_void, location: CGPoint);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    // SAFETY: the event is created, used and released here; `window` is the live
    // NSWindow of this app, and no GPUI window is borrowed while it handles the event.
    unsafe {
        // 0 = kCGScrollEventUnitPixel.
        let event = CGEventCreateScrollWheelEvent(std::ptr::null(), 0, 1, dy as i32);
        if event.is_null() {
            return;
        }
        CGEventSetLocation(event, CGPoint::new(at.0, at.1));
        let ns: *mut Object = msg_send![class!(NSEvent), eventWithCGEvent: event];
        if !ns.is_null() {
            let _: () = msg_send![window as *mut Object, sendEvent: ns];
        }
        CFRelease(event);
    }
}

#[cfg(not(target_os = "macos"))]
fn scroll_wheel(_: usize, _: (f64, f64), _: f32) {}

/// The NSWindow and its content height, which flips GPUI's top-left points to AppKit's.
#[cfg(target_os = "macos")]
fn click_target(window: &Window) -> Option<(usize, f64)> {
    let h = f64::from(f32::from(window.viewport_size().height));
    ns_window(window).map(|w| (w as usize, h))
}

#[cfg(not(target_os = "macos"))]
fn click_target(_: &Window) -> Option<(usize, f64)> {
    None
}

/// Move the pointer to `at` without pressing, in the window's own coordinates.
#[cfg(target_os = "macos")]
fn hover(window: usize, at: (f64, f64)) {
    // NSEventTypeMouseMoved = 5.
    mouse(window, 5, at);
}

#[cfg(not(target_os = "macos"))]
fn hover(_: usize, _: (f64, f64)) {}

/// Move to, press and release the left button at `at`, in the window's own coordinates.
#[cfg(target_os = "macos")]
fn click(window: usize, at: (f64, f64)) {
    // NSEventType: MouseMoved = 5, LeftMouseDown = 1, LeftMouseUp = 2. The move first, so
    // a hover state fires before the press, as it would for a person.
    for kind in [5u64, 1, 2] {
        mouse(window, kind, at);
    }
}

#[cfg(not(target_os = "macos"))]
fn click(_: usize, _: (f64, f64)) {}

/// Send one mouse event of `kind` at `at`, in the window's own coordinates.
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn mouse(window: usize, kind: u64, at: (f64, f64)) {
    use core_graphics::geometry::CGPoint;
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    // SAFETY: `window` is this app's live NSWindow; the event is autoreleased by AppKit
    // and no GPUI window is borrowed while it is handled.
    unsafe {
        let number: isize = msg_send![window as *mut Object, windowNumber];
        let ns: *mut Object = msg_send![class!(NSEvent),
            mouseEventWithType: kind
            location: CGPoint::new(at.0, at.1)
            modifierFlags: 0u64
            timestamp: 0f64
            windowNumber: number
            context: std::ptr::null::<Object>()
            eventNumber: 0isize
            clickCount: if kind == 5 { 0isize } else { 1isize }
            pressure: if kind == 1 { 1f32 } else { 0f32 }];
        if !ns.is_null() {
            let _: () = msg_send![window as *mut Object, sendEvent: ns];
        }
    }
}

#[cfg(target_os = "macos")]
// objc 0.2's `msg_send!` checks a cfg this crate does not declare.
#[allow(unexpected_cfgs)]
fn capture(window: &Window, path: &str) -> Result<(usize, usize), String> {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::window::{
        create_image, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
        kCGWindowListOptionIncludingWindow,
    };
    use objc::{msg_send, sel, sel_impl};

    let w = ns_window(window).ok_or("not an AppKit window")?;
    // SAFETY: `windowNumber` is a plain getter on a live NSWindow.
    let number: isize = unsafe { msg_send![w, windowNumber] };
    let null = CGRect::new(
        &CGPoint::new(f64::INFINITY, f64::INFINITY),
        &CGSize::new(0., 0.),
    );
    let image = create_image(
        null,
        kCGWindowListOptionIncludingWindow,
        number as u32,
        kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
    )
    .ok_or("CGWindowListCreateImage returned nothing")?;
    let (w, h) = (image.width(), image.height());
    if image.bits_per_pixel() != 32 {
        return Err(format!("unexpected {} bpp", image.bits_per_pixel()));
    }
    let stride = image.bytes_per_row();
    let data = image.data();
    let bytes = data.bytes();
    // BGRA (premultiplied, little-endian) → RGBA.
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = &bytes[y * stride..y * stride + w * 4];
        for px in row.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&rgba))
        .map_err(|e| e.to_string())?;
    Ok((w, h))
}

#[cfg(not(target_os = "macos"))]
fn capture(_: &Window, _: &str) -> Result<(usize, usize), String> {
    Err("window capture is only implemented on macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steps() {
        let s = parse("keys escape cmd-k; type hello world\nwait 200;shot /tmp/a.png;drop /x.png")
            .unwrap();
        assert_eq!(
            s,
            vec![
                Step::Keys(vec!["escape".into(), "cmd-k".into()]),
                Step::Type("hello world".into()),
                Step::Wait(200),
                Step::Shot("/tmp/a.png".into()),
                Step::Drop("/x.png".into()),
            ]
        );
        assert!(parse("jump").is_err());
        assert!(parse("wait soon").is_err());
        assert!(parse("shot").is_err());
        assert_eq!(
            parse("click 12 40.5").unwrap(),
            vec![Step::Click(12., 40.5)]
        );
        assert!(parse("click 12").is_err());
        assert!(parse("click a b").is_err());
        assert_eq!(parse("move 12 40.5").unwrap(), vec![Step::Move(12., 40.5)]);
        assert!(parse("move 12").is_err());
        assert_eq!(
            parse("hover 12 40.5").unwrap(),
            vec![Step::Hover(12., 40.5)]
        );
        assert!(parse("hover 12").is_err());
        assert_eq!(
            parse("ws open-pdf").unwrap(),
            vec![Step::Ws(vec!["open-pdf".into()])]
        );
    }
}
