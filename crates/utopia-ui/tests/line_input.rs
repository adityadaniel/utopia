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

    // A masked field (a passphrase) neither feeds the kill buffer nor takes from it: a
    // yank would put text there that the user cannot see.
    input.update(cx, |i, cx| i.set_masked(true, cx));
    cx.simulate_input("secret");
    cx.simulate_keystrokes("ctrl-a ctrl-k");
    assert_eq!(text(&input, cx), "");
    cx.simulate_keystrokes("ctrl-y");
    assert_eq!(text(&input, cx), "", "nothing yanked into a masked field");
    input.update(cx, |i, cx| i.set_masked(false, cx));
    cx.simulate_keystrokes("ctrl-y");
    assert_eq!(
        text(&input, cx),
        "Z",
        "the earlier kill, not the passphrase"
    );
}

/// Ctrl+H and Ctrl+D leave a composition alone, like every other editing key (§13.5).
#[gpui::test]
fn ctrl_h_and_ctrl_d_wait_for_the_input_method(cx: &mut TestAppContext) {
    let (input, cx) = focused_input(cx);
    cx.simulate_input("ab");
    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    cx.simulate_keystrokes("ctrl-h ctrl-d");
    assert_eq!(text(&input, cx), "abにほ");
}

/// §13.5: a stored value arriving from a late load waits for the input method. The
/// composition is not ended behind the IME's back, and the text the user commits is newer
/// than the load, so it wins.
#[gpui::test]
fn fill_waits_for_an_ime_composition(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::light()));
    let (input, cx) = cx.add_window_view(|_, cx| LineInput::new("relay", cx));
    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    let marked = input.update_in(cx, |i, window, cx| i.marked_text_range(window, cx));
    assert!(marked.is_some(), "the input method is composing");

    input.update(cx, |i, cx| i.fill("https://saved.example", cx));
    assert_eq!(
        text(&input, cx),
        "にほ",
        "a fill does not end the composition"
    );
    assert_eq!(
        input.update_in(cx, |i, window, cx| i.marked_text_range(window, cx)),
        marked,
        "the marked range is untouched"
    );

    input.update_in(cx, |i, window, cx| {
        i.replace_text_in_range(None, "日本", window, cx)
    });
    assert_eq!(
        text(&input, cx),
        "日本",
        "the user's commit wins over the stale load"
    );
    assert_eq!(
        input.update_in(cx, |i, window, cx| i.marked_text_range(window, cx)),
        None
    );
}

/// §13.5: a saved value from an async load does not overwrite what the user has typed.
#[gpui::test]
fn fill_after_typing_does_not_overwrite(cx: &mut TestAppContext) {
    let (input, cx) = focused_input(cx);
    input.update(cx, |i, cx| i.fill("initial", cx));
    assert_eq!(text(&input, cx), "initial");
    cx.simulate_input(" typed");
    assert_eq!(text(&input, cx), "initial typed");

    input.update(cx, |i, cx| i.fill("https://saved.example", cx));
    assert_eq!(
        text(&input, cx),
        "initial typed",
        "the user's text wins over the late load"
    );
}

/// §13.5: a fill deferred during a composition is shown once the input method lets go,
/// when the user cancelled that composition rather than committing text.
#[gpui::test]
fn a_cancelled_composition_lets_a_deferred_fill_through(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::light()));
    let (input, cx) = cx.add_window_view(|_, cx| LineInput::new("relay", cx));
    input.update(cx, |i, cx| i.fill("before", cx));
    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    input.update(cx, |i, cx| i.fill("https://saved.example", cx));
    assert_eq!(text(&input, cx), "beforeにほ", "the composition survives");

    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "", None, window, cx)
    });
    assert_eq!(
        text(&input, cx),
        "https://saved.example",
        "nothing the user committed replaced the composition, so the load shows"
    );
    assert_eq!(
        input.update_in(cx, |i, window, cx| i.marked_text_range(window, cx)),
        None
    );
}

/// §13.5: cancelling a composition leaves the committed text exactly as it was, so a
/// secret the host already submitted must still be clearable when its request finishes.
#[gpui::test]
fn a_cancelled_composition_keeps_a_submitted_field_clean(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Theme::light()));
    let (input, cx) = cx.add_window_view(|_, cx| LineInput::new("secret", cx));
    input.update(cx, |i, cx| {
        i.fill("saved-token", cx);
        i.submitted();
    });
    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "にほ", None, window, cx)
    });
    input.update_in(cx, |i, window, cx| {
        i.replace_and_mark_text_in_range(None, "", None, window, cx)
    });
    assert_eq!(text(&input, cx), "saved-token");

    // The request settles: the submitted secret clears even though a composition was
    // started and cancelled after it was submitted.
    input.update(cx, |i, cx| i.fill("", cx));
    assert_eq!(text(&input, cx), "");
}
