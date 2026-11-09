//! The kill buffer behind Ctrl+K and Ctrl+Y (§9.1): the last run of text killed this
//! session, in memory only.
//!
//! macOS keeps its own kill ring; this is ours because the app owns both ends of the
//! gesture (Ctrl+K kills to the end of the wrapped line, Ctrl+Y puts it back at the caret)
//! and the text is never clipped, written to a file or synced (rule 9). One slot is enough
//! for the pair: Ctrl+K twice in a row keeps the second run, as Emacs does for a
//! non-consecutive kill.
//!
//! It lives in a thread local rather than an `App` global so that a single-line input far
//! from the store (the palette, a filter box) can use it without being handed the app, and
//! so that nothing has to be installed at startup — an unset buffer is the empty string.

use std::cell::RefCell;

thread_local! {
    /// The last killed text. Per thread, which for this app (and for a test) is the
    /// session.
    static KILLED: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Remember `text` as the last killed run. Killing nothing must not wipe the buffer, so
/// callers only call this when they actually removed something.
pub fn remember(text: &str) {
    KILLED.with(|k| *k.borrow_mut() = text.to_string());
}

/// The last killed run, empty when nothing has been killed yet. A copy: the buffer is the
/// app's, the caller's insertion is its own.
pub fn last() -> String {
    KILLED.with(|k| k.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One slot, empty until something is killed, and never consumed by yanking twice.
    #[test]
    fn the_last_kill_is_kept_until_the_next_one() {
        assert_eq!(last(), "");
        remember("half a sentence");
        assert_eq!(last(), "half a sentence");
        assert_eq!(last(), "half a sentence");
        remember("the next one");
        assert_eq!(last(), "the next one");
    }
}
