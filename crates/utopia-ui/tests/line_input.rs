//! The single-line input's own side of §13.5: a host that writes the whole text is told
//! about it, a write of the text already shown is not a change (the reference filter is
//! cleared on every page change, §13.4), and a composition in it is never mistaken for a
//! committed query.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{EntityInputHandler as _, TestAppContext};
use utopia_ui::line_input::{LineInput, LineInputEvent};
use utopia_ui::theme::Theme;

#[gpui::test]
fn set_text_notifies_only_a_change(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::light()));
    let (input, cx) = cx.add_window_view(|_, cx| LineInput::new("search", cx));
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = seen.clone();
    let _sub = cx.cx.update(|cx| {
        cx.subscribe(&input, move |_, e: &LineInputEvent, _| {
            if let LineInputEvent::Changed(t) = e {
                sink.borrow_mut().push(t.clone());
            }
        })
    });

    input.update(cx, |i, cx| i.set_text("november", cx));
    assert_eq!(seen.borrow().as_slice(), ["november"]);
    // Clearing a filter that is already empty must not make its listener rebuild.
    input.update(cx, |i, cx| i.set_text("november", cx));
    assert_eq!(
        seen.borrow().as_slice(),
        ["november"],
        "writing the text it already shows changes nothing"
    );

    // A composition is provisional: the host hears about the commit, not the glyphs.
    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    assert_eq!(seen.borrow().len(), 1, "a composition is not a query");
    // A host write still wins over a live composition and is announced once.
    input.update(cx, |i, cx| i.set_text("zebra", cx));
    assert_eq!(seen.borrow().as_slice(), ["november", "zebra"]);
    assert_eq!(input.read_with(cx, |i, _| i.text().to_string()), "zebra");
    assert_eq!(
        input.update_in(cx, |i, window, cx| i.marked_text_range(window, cx)),
        None,
        "the field is the host's again"
    );
}

/// A focused single-line input with the default keymap, as in the palette or a filter.
fn focused_input(
    cx: &mut TestAppContext,
) -> (gpui::Entity<LineInput>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        cx.set_global(Theme::light());
        utopia_ui::actions::install_keymap(cx, None);
    });
    let (input, cx) = cx.add_window_view(|_, cx| LineInput::new("search", cx));
    input.update_in(cx, |i, window, _| i.focus(window));
    cx.run_until_parked();
    (input, cx)
}

fn text(input: &gpui::Entity<LineInput>, cx: &mut gpui::VisualTestContext) -> String {
    input.read_with(cx, |i, _| i.text().to_string())
}

/// The macOS text keys work in single-line fields too (§9.1): word and line selection
/// extend from the caret, word deletion, and the Emacs keys with their kill buffer.
#[gpui::test]
fn macos_text_keys_work_in_a_single_line_input(cx: &mut TestAppContext) {
    let (input, cx) = focused_input(cx);
    cx.simulate_input("one two three");
    cx.simulate_keystrokes("alt-shift-left");
    cx.simulate_input("X");
    assert_eq!(
        text(&input, cx),
        "one two X",
        "Option+Shift+Left selects a word"
    );
    cx.simulate_keystrokes("alt-backspace");
    assert_eq!(text(&input, cx), "one two ");
    // Shift+Home and Shift+End extend from the caret; they do not select the whole field.
    cx.simulate_keystrokes("left left shift-home");
    cx.simulate_input("Q");
    assert_eq!(text(&input, cx), "Qo ");
    cx.simulate_keystrokes("ctrl-a shift-end");
    cx.simulate_input("abcd");
    assert_eq!(text(&input, cx), "abcd");
    cx.simulate_keystrokes("ctrl-b cmd-shift-left");
    cx.simulate_input("Z");
    assert_eq!(text(&input, cx), "Zd");
    // Ctrl+T, Ctrl+K and Ctrl+Y.
    cx.simulate_keystrokes("ctrl-t");
    assert_eq!(text(&input, cx), "dZ");
    cx.simulate_keystrokes("ctrl-a ctrl-f ctrl-k");
    assert_eq!(text(&input, cx), "d");
    cx.simulate_keystrokes("ctrl-a ctrl-y");
    assert_eq!(text(&input, cx), "Zd");
    cx.simulate_keystrokes("ctrl-a alt-delete");
    assert_eq!(text(&input, cx), "");

    // What a masked field (a passphrase) kills is not kept for Ctrl+Y.
    input.update(cx, |i, cx| i.set_masked(true, cx));
    cx.simulate_input("secret");
    cx.simulate_keystrokes("ctrl-a ctrl-k ctrl-y");
    assert_eq!(
        text(&input, cx),
        "Z",
        "the earlier kill, not the passphrase"
    );
}
