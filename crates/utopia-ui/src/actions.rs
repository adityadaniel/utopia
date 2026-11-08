//! Actions, the keymap and its catalog (§9.1). Defaults are macOS bindings; `cmd` becomes
//! `ctrl` elsewhere. A `keymap.json` in the config directory adds or overrides bindings;
//! unknown actions are reported and ignored, never fatal.
//!
//! [`CATALOG`] is the single source of truth: it carries the default keys *and* the label,
//! category and one-line help for every bindable action. The defaults installed at startup,
//! the Settings keybinding editor and the shortcut cheat sheet all read it, so a new action
//! cannot show up in one place and be missing from another.

use gpui::{actions, App, KeyBinding};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

actions!(
    utopia,
    [
        Quit,
        QuickJump,
        GoDailyNotes,
        GoToday,
        GoYesterday,
        PrevDay,
        NextDay,
        ToggleRightSidebar,
        ToggleReferences,
        Undo,
        Redo,
        OpenSettings,
        ShowShortcuts,
        UtopiaHelp,
    ]
);

actions!(
    editing,
    [
        Newline,
        SoftBreak,
        InsertAbove,
        Indent,
        Outdent,
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        WordLeft,
        WordRight,
        LineStart,
        LineEnd,
        MoveBlockUp,
        MoveBlockDown,
        ToggleCollapse,
        ToggleCollapseAll,
        ZoomIn,
        ZoomOut,
        Bold,
        Italic,
        Code,
        Copy,
        Cut,
        Paste,
        CopyBlockRef,
        DeleteBlock,
        Escape,
        ShowCharacterPalette,
        ToggleTodo,
        AcceptCompletion,
    ]
);

actions!(
    utopia,
    [FullSearch, QuickCapture, ShowGraph, ToggleLeftSidebar]
);

pub const EDITOR_CONTEXT: &str = "BlockEditor";
/// Added to the editor's context while an autocomplete menu with suggestions is open.
pub const AUTOCOMPLETE_CONTEXT: &str = "autocomplete";
pub const INPUT_CONTEXT: &str = "LineInput";

/// Where an action is grouped in the Settings editor and the cheat sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Navigation,
    Outline,
    Formatting,
    Editing,
    View,
    Application,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Navigation => "Navigation",
            Category::Outline => "Outline",
            Category::Formatting => "Formatting",
            Category::Editing => "Editing",
            Category::View => "View",
            Category::Application => "Application",
        }
    }

    /// Display order of the sections.
    pub fn all() -> &'static [Category] {
        &[
            Category::Navigation,
            Category::Outline,
            Category::Formatting,
            Category::Editing,
            Category::View,
            Category::Application,
        ]
    }
}

/// One bindable action: its default keys plus the text the UI needs to describe it.
#[derive(Clone, Copy, Debug)]
pub struct ActionDoc {
    /// The action name as `keymap.json` spells it, e.g. `"GoToday"`.
    pub action: &'static str,
    pub label: &'static str,
    pub category: Category,
    /// Default keystrokes in gpui form, before [`platform_keys`].
    pub keys: &'static str,
    pub context: Option<&'static str>,
    pub help: &'static str,
}

const EDITOR: Option<&str> = Some("BlockEditor");
const TEXT: Option<&str> = Some("BlockEditor || LineInput");
/// Only while a `[[`, `#`, `((` or `/` menu is open, so the same key can keep its
/// everyday meaning (Tab indents) the rest of the time.
const COMPLETING: Option<&str> = Some("BlockEditor && autocomplete");

/// Every action that can be bound, in the order the Settings editor lists it.
pub const CATALOG: &[ActionDoc] = &[
    // ----- Navigation -----
    ActionDoc {
        action: "GoDailyNotes",
        label: "Daily Note",
        category: Category::Navigation,
        keys: "cmd-shift-d",
        context: None,
        help: "Open the Daily Notes feed: every day from today backwards, editable in place.",
    },
    ActionDoc {
        action: "GoToday",
        label: "Today",
        category: Category::Navigation,
        keys: "cmd-shift-t",
        context: None,
        help: "Open today's daily page on its own, away from the feed.",
    },
    ActionDoc {
        action: "GoYesterday",
        label: "Yesterday",
        category: Category::Navigation,
        keys: "cmd-shift-y",
        context: None,
        help: "Open yesterday's daily page on its own.",
    },
    ActionDoc {
        action: "PrevDay",
        label: "Previous Day",
        category: Category::Navigation,
        keys: "cmd-[",
        context: None,
        help: "Step one day back from the day you are looking at.",
    },
    ActionDoc {
        action: "NextDay",
        label: "Next Day",
        category: Category::Navigation,
        keys: "cmd-]",
        context: None,
        help: "Step one day forward from the day you are looking at.",
    },
    ActionDoc {
        action: "QuickJump",
        label: "Quick Jump",
        category: Category::Navigation,
        keys: "cmd-k",
        context: None,
        help: "Jump to a page by name, or create it by typing a name that does not exist.",
    },
    ActionDoc {
        action: "FullSearch",
        label: "Search",
        category: Category::Navigation,
        keys: "cmd-shift-f",
        context: None,
        help: "Full-text search across every block in the graph.",
    },
    ActionDoc {
        action: "ZoomIn",
        label: "Zoom In To Block",
        category: Category::Navigation,
        keys: "cmd-alt-down",
        context: EDITOR,
        help: "Make the current block the top of the page, hiding everything above it.",
    },
    ActionDoc {
        action: "ZoomOut",
        label: "Zoom Out",
        category: Category::Navigation,
        keys: "cmd-alt-up",
        context: EDITOR,
        help: "Leave a zoomed block and show its parent again.",
    },
    // ----- Outline -----
    ActionDoc {
        action: "Newline",
        label: "New Block",
        category: Category::Outline,
        keys: "enter",
        context: TEXT,
        help: "Finish this block and start a sibling below it.",
    },
    ActionDoc {
        action: "InsertAbove",
        label: "New Block Above",
        category: Category::Outline,
        keys: "cmd-enter",
        context: TEXT,
        help: "Start a sibling block above the current one.",
    },
    ActionDoc {
        action: "SoftBreak",
        label: "Line Break In Block",
        category: Category::Outline,
        keys: "shift-enter",
        context: TEXT,
        help: "Add a line break inside the block instead of starting a new one.",
    },
    ActionDoc {
        action: "Indent",
        label: "Indent",
        category: Category::Outline,
        keys: "tab",
        context: EDITOR,
        help: "Make this block a child of the block above it.",
    },
    ActionDoc {
        action: "Outdent",
        label: "Outdent",
        category: Category::Outline,
        keys: "shift-tab",
        context: EDITOR,
        help: "Move this block out to its parent's level.",
    },
    ActionDoc {
        action: "MoveBlockUp",
        label: "Move Block Up",
        category: Category::Outline,
        keys: "cmd-shift-up",
        context: EDITOR,
        help: "Swap this block with the sibling above it, subtree and all.",
    },
    ActionDoc {
        action: "MoveBlockDown",
        label: "Move Block Down",
        category: Category::Outline,
        keys: "cmd-shift-down",
        context: EDITOR,
        help: "Swap this block with the sibling below it, subtree and all.",
    },
    ActionDoc {
        action: "ToggleCollapse",
        label: "Collapse Or Expand",
        category: Category::Outline,
        keys: "cmd-.",
        context: EDITOR,
        help: "Fold this block's children out of sight, or unfold them.",
    },
    ActionDoc {
        action: "ToggleCollapseAll",
        label: "Collapse Or Expand All",
        category: Category::Outline,
        keys: "cmd-alt-.",
        context: EDITOR,
        help: "Fold or unfold every block on the page at once.",
    },
    ActionDoc {
        action: "ToggleTodo",
        label: "Cycle To-do State",
        category: Category::Outline,
        keys: "alt-enter",
        context: EDITOR,
        help: "Turn the block into a to-do, then tick it, then turn it back to plain text.",
    },
    ActionDoc {
        action: "DeleteBlock",
        label: "Delete Block",
        category: Category::Outline,
        keys: "cmd-shift-backspace",
        context: EDITOR,
        help: "Send this block to the trash. Its id survives, so references still resolve.",
    },
    ActionDoc {
        action: "CopyBlockRef",
        label: "Copy Block Reference",
        category: Category::Outline,
        keys: "cmd-shift-c",
        context: EDITOR,
        help: "Copy this block's ((uid)) so you can reference it from anywhere.",
    },
    // ----- Formatting -----
    ActionDoc {
        action: "Bold",
        label: "Bold",
        category: Category::Formatting,
        keys: "cmd-b",
        context: EDITOR,
        help: "Wrap the selection in **bold**.",
    },
    ActionDoc {
        action: "Italic",
        label: "Italic",
        category: Category::Formatting,
        keys: "cmd-i",
        context: EDITOR,
        help: "Wrap the selection in _italics_.",
    },
    ActionDoc {
        action: "Code",
        label: "Inline Code",
        category: Category::Formatting,
        keys: "cmd-e",
        context: EDITOR,
        help: "Wrap the selection in `code`.",
    },
    // ----- Editing -----
    ActionDoc {
        action: "Undo",
        label: "Undo",
        category: Category::Editing,
        keys: "cmd-z",
        context: None,
        help: "Undo the last change. A rename that merged two pages undoes as one step.",
    },
    ActionDoc {
        action: "Redo",
        label: "Redo",
        category: Category::Editing,
        keys: "cmd-shift-z",
        context: None,
        help: "Redo the change you just undid.",
    },
    ActionDoc {
        action: "Copy",
        label: "Copy",
        category: Category::Editing,
        keys: "cmd-c",
        context: TEXT,
        help: "Copy the selection.",
    },
    ActionDoc {
        action: "Cut",
        label: "Cut",
        category: Category::Editing,
        keys: "cmd-x",
        context: TEXT,
        help: "Cut the selection.",
    },
    ActionDoc {
        action: "Paste",
        label: "Paste",
        category: Category::Editing,
        keys: "cmd-v",
        context: TEXT,
        help: "Paste. Indented Markdown arrives as a block tree.",
    },
    ActionDoc {
        action: "SelectAll",
        label: "Select All",
        category: Category::Editing,
        keys: "cmd-a",
        context: TEXT,
        help: "Select everything in the block.",
    },
    ActionDoc {
        action: "Backspace",
        label: "Backspace",
        category: Category::Editing,
        keys: "backspace",
        context: TEXT,
        help: "Delete backwards; at the start of a block it joins with the one above.",
    },
    ActionDoc {
        action: "Delete",
        label: "Forward Delete",
        category: Category::Editing,
        keys: "delete",
        context: TEXT,
        help: "Delete the character after the caret.",
    },
    ActionDoc {
        action: "Left",
        label: "Move Left",
        category: Category::Editing,
        keys: "left",
        context: TEXT,
        help: "Move the caret one grapheme left.",
    },
    ActionDoc {
        action: "Right",
        label: "Move Right",
        category: Category::Editing,
        keys: "right",
        context: TEXT,
        help: "Move the caret one grapheme right.",
    },
    ActionDoc {
        action: "Up",
        label: "Move Up",
        category: Category::Editing,
        keys: "up",
        context: TEXT,
        help: "Move the caret to the block above.",
    },
    ActionDoc {
        action: "Down",
        label: "Move Down",
        category: Category::Editing,
        keys: "down",
        context: TEXT,
        help: "Move the caret to the block below.",
    },
    ActionDoc {
        action: "SelectLeft",
        label: "Select Left",
        category: Category::Editing,
        keys: "shift-left",
        context: TEXT,
        help: "Extend the selection one grapheme left.",
    },
    ActionDoc {
        action: "SelectRight",
        label: "Select Right",
        category: Category::Editing,
        keys: "shift-right",
        context: TEXT,
        help: "Extend the selection one grapheme right.",
    },
    ActionDoc {
        action: "SelectUp",
        label: "Select Up",
        category: Category::Editing,
        keys: "shift-up",
        context: EDITOR,
        help: "Extend the selection up.",
    },
    ActionDoc {
        action: "SelectDown",
        label: "Select Down",
        category: Category::Editing,
        keys: "shift-down",
        context: EDITOR,
        help: "Extend the selection down.",
    },
    ActionDoc {
        action: "WordLeft",
        label: "Word Left",
        category: Category::Editing,
        keys: "alt-left",
        context: TEXT,
        help: "Move the caret one word left.",
    },
    ActionDoc {
        action: "WordRight",
        label: "Word Right",
        category: Category::Editing,
        keys: "alt-right",
        context: TEXT,
        help: "Move the caret one word right.",
    },
    ActionDoc {
        action: "LineStart",
        label: "Start Of Line",
        category: Category::Editing,
        keys: "cmd-left",
        context: TEXT,
        help: "Move the caret to the start of the line.",
    },
    ActionDoc {
        action: "LineEnd",
        label: "End Of Line",
        category: Category::Editing,
        keys: "cmd-right",
        context: TEXT,
        help: "Move the caret to the end of the line.",
    },
    ActionDoc {
        action: "Home",
        label: "Start Of Block",
        category: Category::Editing,
        keys: "home",
        context: TEXT,
        help: "Move the caret to the start of the block.",
    },
    ActionDoc {
        action: "End",
        label: "End Of Block",
        category: Category::Editing,
        keys: "end",
        context: TEXT,
        help: "Move the caret to the end of the block.",
    },
    ActionDoc {
        action: "Escape",
        label: "Cancel",
        category: Category::Editing,
        keys: "escape",
        context: TEXT,
        help: "Close the open menu or panel, or leave the block.",
    },
    // Listed after Indent: for the same keys at the same depth gpui prefers the later
    // binding, so Tab accepts while a menu is open and indents otherwise.
    ActionDoc {
        action: "AcceptCompletion",
        label: "Accept Suggestion",
        category: Category::Editing,
        keys: "tab",
        context: COMPLETING,
        help: "In a [[, #, (( or / menu, take the highlighted suggestion (Enter does too).",
    },
    ActionDoc {
        action: "ShowCharacterPalette",
        label: "Emoji And Symbols",
        category: Category::Editing,
        keys: "ctrl-cmd-space",
        context: TEXT,
        help: "Open the system character palette.",
    },
    // ----- View -----
    ActionDoc {
        action: "ToggleLeftSidebar",
        label: "Toggle Left Sidebar",
        category: Category::View,
        keys: "cmd-\\",
        context: None,
        help: "Show or hide the left sidebar with recent days and the calendar.",
    },
    ActionDoc {
        action: "ToggleRightSidebar",
        label: "Toggle Right Sidebar",
        category: Category::View,
        keys: "cmd-shift-\\",
        context: None,
        help: "Show or hide the right sidebar, where shift-clicked pages open.",
    },
    ActionDoc {
        action: "ToggleReferences",
        label: "Toggle References",
        category: Category::View,
        keys: "cmd-shift-b",
        context: None,
        help: "Show or hide Linked and Unlinked References under the page.",
    },
    ActionDoc {
        action: "ShowGraph",
        label: "Graph View",
        category: Category::View,
        keys: "cmd-shift-g",
        context: None,
        help: "See the graph of pages and the links between them.",
    },
    // ----- Application -----
    ActionDoc {
        action: "QuickCapture",
        label: "Quick Capture",
        category: Category::Application,
        keys: "cmd-shift-space",
        context: None,
        help: "Type a thought straight into today's note without leaving where you are.",
    },
    ActionDoc {
        action: "OpenSettings",
        label: "Settings",
        category: Category::Application,
        keys: "cmd-,",
        context: None,
        help: "Open Settings, where you can change these keybindings.",
    },
    ActionDoc {
        action: "ShowShortcuts",
        label: "Keyboard Shortcuts",
        category: Category::Application,
        keys: "cmd-/",
        context: None,
        help: "Show the cheat sheet of every shortcut in the app.",
    },
    ActionDoc {
        action: "UtopiaHelp",
        label: "Utopia Help",
        category: Category::Application,
        keys: "cmd-shift-/",
        context: None,
        help: "Open the guided tour of everything the app can do.",
    },
    ActionDoc {
        action: "Quit",
        label: "Quit",
        category: Category::Application,
        keys: "cmd-q",
        context: None,
        help: "Quit Utopia. Edits are already saved.",
    },
];

/// Look an action up in [`CATALOG`].
pub fn doc(action: &str) -> Option<&'static ActionDoc> {
    CATALOG.iter().find(|d| d.action == action)
}

/// The action a catalog name names, so Quick jump can run a command through the same
/// dispatch path as its keystroke (QJ-5). `None` for a name the catalog does not know;
/// [`CATALOG`] and this match are kept in step by a test.
pub fn action_for(name: &str) -> Option<Box<dyn gpui::Action>> {
    let action: Box<dyn gpui::Action> = match name {
        "GoDailyNotes" => Box::new(GoDailyNotes),
        "GoToday" => Box::new(GoToday),
        "GoYesterday" => Box::new(GoYesterday),
        "PrevDay" => Box::new(PrevDay),
        "NextDay" => Box::new(NextDay),
        "QuickJump" => Box::new(QuickJump),
        "FullSearch" => Box::new(FullSearch),
        "ZoomIn" => Box::new(ZoomIn),
        "ZoomOut" => Box::new(ZoomOut),
        "Newline" => Box::new(Newline),
        "InsertAbove" => Box::new(InsertAbove),
        "SoftBreak" => Box::new(SoftBreak),
        "Indent" => Box::new(Indent),
        "Outdent" => Box::new(Outdent),
        "MoveBlockUp" => Box::new(MoveBlockUp),
        "MoveBlockDown" => Box::new(MoveBlockDown),
        "ToggleCollapse" => Box::new(ToggleCollapse),
        "ToggleCollapseAll" => Box::new(ToggleCollapseAll),
        "ToggleTodo" => Box::new(ToggleTodo),
        "DeleteBlock" => Box::new(DeleteBlock),
        "CopyBlockRef" => Box::new(CopyBlockRef),
        "Bold" => Box::new(Bold),
        "Italic" => Box::new(Italic),
        "Code" => Box::new(Code),
        "Undo" => Box::new(Undo),
        "Redo" => Box::new(Redo),
        "Copy" => Box::new(Copy),
        "Cut" => Box::new(Cut),
        "Paste" => Box::new(Paste),
        "SelectAll" => Box::new(SelectAll),
        "Backspace" => Box::new(Backspace),
        "Delete" => Box::new(Delete),
        "Left" => Box::new(Left),
        "Right" => Box::new(Right),
        "Up" => Box::new(Up),
        "Down" => Box::new(Down),
        "SelectLeft" => Box::new(SelectLeft),
        "SelectRight" => Box::new(SelectRight),
        "SelectUp" => Box::new(SelectUp),
        "SelectDown" => Box::new(SelectDown),
        "WordLeft" => Box::new(WordLeft),
        "WordRight" => Box::new(WordRight),
        "LineStart" => Box::new(LineStart),
        "LineEnd" => Box::new(LineEnd),
        "Home" => Box::new(Home),
        "End" => Box::new(End),
        "Escape" => Box::new(Escape),
        "AcceptCompletion" => Box::new(AcceptCompletion),
        "ShowCharacterPalette" => Box::new(ShowCharacterPalette),
        "ToggleLeftSidebar" => Box::new(ToggleLeftSidebar),
        "ToggleRightSidebar" => Box::new(ToggleRightSidebar),
        "ToggleReferences" => Box::new(ToggleReferences),
        "ShowGraph" => Box::new(ShowGraph),
        "QuickCapture" => Box::new(QuickCapture),
        "OpenSettings" => Box::new(OpenSettings),
        "ShowShortcuts" => Box::new(ShowShortcuts),
        "UtopiaHelp" => Box::new(UtopiaHelp),
        "Quit" => Box::new(Quit),
        _ => return None,
    };
    Some(action)
}

/// The actions of one category, in catalog order.
pub fn by_category(c: Category) -> impl Iterator<Item = &'static ActionDoc> {
    CATALOG.iter().filter(move |d| d.category == c)
}

/// Keystrokes in gpui's own spelling, so `cmd-alt-t` and `alt-cmd-t` compare equal.
///
/// A captured keystroke comes back from [`gpui::Keystroke::unparse`] with modifiers in a
/// fixed order, while the catalog spells them however reads best. Comparing the two
/// without normalizing would store a redundant override when someone presses the default,
/// and would miss a clash between two spellings of the same chord.
pub fn canonical_keys(keys: &str) -> String {
    keys.split_whitespace()
        .map(|k| {
            gpui::Keystroke::parse(k)
                .map(|k| k.unparse())
                .unwrap_or_else(|_| k.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn platform_keys(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.to_string()
    } else {
        keys.replace("cmd-", "ctrl-")
    }
}

/// Keystrokes rendered the way a macOS menu would show them (`cmd-shift-d` → `⇧⌘D`).
pub fn pretty_keys(keys: &str) -> String {
    keys.split_whitespace()
        .map(pretty_chord)
        .collect::<Vec<_>>()
        .join(" ")
}

fn pretty_chord(chord: &str) -> String {
    let mut parts: Vec<&str> = chord.split('-').collect();
    // A trailing "-" key (as in `cmd--`) leaves an empty last part.
    let key = if parts.last() == Some(&"") {
        parts.pop();
        "-"
    } else {
        parts.pop().unwrap_or("")
    };
    if !cfg!(target_os = "macos") {
        let mut s: Vec<String> = parts.iter().map(|m| title_case(m)).collect();
        s.push(key_name(key));
        return s.join("+");
    }
    let mut out = String::new();
    // macOS orders modifiers ctrl, alt, shift, cmd.
    for (m, sym) in [
        ("ctrl", "⌃"),
        ("alt", "⌥"),
        ("shift", "⇧"),
        ("cmd", "⌘"),
        ("fn", "fn"),
    ] {
        if parts.contains(&m) {
            out.push_str(sym);
        }
    }
    out.push_str(&key_name(key));
    out
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn key_name(key: &str) -> String {
    match key {
        "enter" => "↩".into(),
        "tab" => "⇥".into(),
        "space" => "Space".into(),
        "backspace" => "⌫".into(),
        "delete" => "⌦".into(),
        "escape" => "esc".into(),
        "left" => "←".into(),
        "right" => "→".into(),
        "up" => "↑".into(),
        "down" => "↓".into(),
        "home" => "↖".into(),
        "end" => "↘".into(),
        k if k.chars().count() == 1 => k.to_uppercase(),
        k => title_case(k),
    }
}

macro_rules! binding_for {
    ($name:expr, $keys:expr, $ctx:expr, [$($action:ident),* $(,)?]) => {
        match $name {
            $(stringify!($action) => Some(KeyBinding::new($keys, $action, $ctx)),)*
            _ => None,
        }
    };
}

/// Build a binding from an action name; `None` for unknown names or invalid keys.
pub fn binding(name: &str, keys: &str, context: Option<&str>) -> Option<KeyBinding> {
    // Validate keystrokes first: KeyBinding::new panics on invalid input.
    if keys.split_whitespace().count() == 0
        || keys
            .split_whitespace()
            .any(|k| gpui::Keystroke::parse(k).is_err())
    {
        return None;
    }
    let keys = keys.to_string();
    let keys = keys.as_str();
    binding_for!(
        name,
        keys,
        context,
        [
            Quit,
            QuickJump,
            GoDailyNotes,
            GoToday,
            GoYesterday,
            PrevDay,
            NextDay,
            ToggleRightSidebar,
            ToggleReferences,
            Undo,
            Redo,
            OpenSettings,
            ShowShortcuts,
            UtopiaHelp,
            FullSearch,
            QuickCapture,
            ShowGraph,
            ToggleLeftSidebar,
            ToggleTodo,
            Newline,
            SoftBreak,
            InsertAbove,
            Indent,
            Outdent,
            Backspace,
            Delete,
            Left,
            Right,
            Up,
            Down,
            SelectLeft,
            SelectRight,
            SelectUp,
            SelectDown,
            SelectAll,
            Home,
            End,
            WordLeft,
            WordRight,
            LineStart,
            LineEnd,
            MoveBlockUp,
            MoveBlockDown,
            ToggleCollapse,
            ToggleCollapseAll,
            ZoomIn,
            ZoomOut,
            Bold,
            Italic,
            Code,
            Copy,
            Cut,
            Paste,
            CopyBlockRef,
            DeleteBlock,
            Escape,
            ShowCharacterPalette,
            AcceptCompletion,
        ]
    )
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KeymapEntry {
    pub key: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Parse `keymap.json` (a list of `{ "key", "action", "context"? }`), returning the valid
/// bindings and a warning for each entry that could not be used.
pub fn parse_keymap(json: &str) -> (Vec<KeyBinding>, Vec<String>) {
    let (entries, mut warnings) = read_entries(json);
    let mut out = Vec::new();
    for e in entries {
        match binding(&e.action, &e.key, e.context.as_deref()) {
            Some(b) => out.push(b),
            None => warnings.push(format!(
                "keymap.json: ignoring unknown action or keys: {} -> {}",
                e.key, e.action
            )),
        }
    }
    (out, warnings)
}

/// The raw entries of a `keymap.json`, plus a warning if it could not be read at all.
pub fn read_entries(json: &str) -> (Vec<KeymapEntry>, Vec<String>) {
    match serde_json::from_str::<Vec<KeymapEntry>>(json) {
        Ok(e) => (e, Vec::new()),
        Err(e) => (Vec::new(), vec![format!("keymap.json: {e}")]),
    }
}

/// What an override says about one action, read once so every reader asks the same value:
/// [`install`] from it, the Settings editor and the cheat sheet show it, [`Overrides::conflicts`]
/// measures chords against it, Quick Jump reads the result of it, and [`Overrides::to_json`]
/// writes it back. The overrides live in three places — the file, the live keymap and the
/// editor — and letting each interpret the file for itself is what made them disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Override {
    /// Keys gpui can bind.
    Valid(String),
}

impl Override {
    /// The keys this override binds.
    pub fn keys(&self) -> &str {
        match self {
            Override::Valid(keys) => keys,
        }
    }
}

/// User keybinding overrides: action name → what the user asked for. Only actions the user
/// actually changed are stored, so a default that moves in a later release still moves for
/// them. The map is private so that nothing can put an entry in it that the readers would
/// have to second-guess.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides(BTreeMap<String, Override>);

impl Overrides {
    /// Read overrides out of a `keymap.json`. Entries for actions that are not in the
    /// catalog are dropped (they cannot be shown or edited), which the caller is warned about.
    pub fn from_json(json: &str) -> (Self, Vec<String>) {
        let (entries, mut warnings) = read_entries(json);
        let mut map = BTreeMap::new();
        for e in entries {
            if doc(&e.action).is_some() {
                map.insert(e.action, Override::Valid(e.key));
            } else {
                warnings.push(format!("keymap.json: unknown action {}", e.action));
            }
        }
        (Self(map), warnings)
    }

    pub fn load(config_dir: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(config_dir.join("keymap.json")) {
            Ok(s) => Self::from_json(&s),
            Err(_) => (Self::default(), Vec::new()),
        }
    }

    /// Serialize to the `keymap.json` format, carrying each action's catalog context so the
    /// override lands in the same context as the default it replaces.
    pub fn to_json(&self) -> String {
        let entries: Vec<KeymapEntry> = self
            .0
            .iter()
            .filter_map(|(action, o)| {
                let d = doc(action)?;
                Some(KeymapEntry {
                    key: o.keys().to_string(),
                    action: action.clone(),
                    context: d.context.map(str::to_string),
                })
            })
            .collect();
        serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".to_string())
    }

    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        std::fs::write(config_dir.join("keymap.json"), self.to_json())
    }

    /// The keys in force for an action: the override if there is one, else the default.
    pub fn keys_for(&self, d: &ActionDoc) -> String {
        self.0
            .get(d.action)
            .map(|o| o.keys().to_string())
            .unwrap_or_else(|| platform_keys(d.keys))
    }

    /// Whether the row differs from the catalog: an entry exists in the map only when it
    /// does.
    pub fn is_changed(&self, action: &str) -> bool {
        self.0.contains_key(action)
    }

    /// The state of one action's override, for readers that need more than the keys (the
    /// editor marks an unusable entry).
    pub fn entry(&self, action: &str) -> Option<&Override> {
        self.0.get(action)
    }

    /// How many actions the user changed.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn set(&mut self, action: &str, keys: &str) {
        if doc(action).is_some() {
            self.0
                .insert(action.to_string(), Override::Valid(keys.to_string()));
        }
    }

    /// Drop an override so the action goes back to its catalog default.
    pub fn reset(&mut self, action: &str) {
        self.0.remove(action);
    }

    /// Actions that would answer to the same keys in the same context. Two actions in
    /// different contexts (the editor vs. the whole window) do not conflict.
    pub fn conflicts(&self) -> Vec<(&'static ActionDoc, &'static ActionDoc)> {
        let mut out = Vec::new();
        for (i, a) in CATALOG.iter().enumerate() {
            for b in CATALOG.iter().skip(i + 1) {
                if a.context == b.context
                    && canonical_keys(&self.keys_for(a)) == canonical_keys(&self.keys_for(b))
                {
                    out.push((a, b));
                }
            }
        }
        out
    }
}

/// The keys the app has bound for an action right now, in `keymap.json` spelling.
///
/// The palette lists commands with their keys, so it has to read the live keymap rather
/// than [`CATALOG`]: [`install`] drops a rebinding's default, and a palette that still
/// advertised it would show a chord that no longer works (§9.1, QJ-5). Falls back to the
/// catalog default when nothing is bound yet (a test app before `install`).
pub fn active_keys(cx: &App, d: &ActionDoc) -> String {
    let Some(action) = action_for(d.action) else {
        return platform_keys(d.keys);
    };
    let bindings = cx.key_bindings();
    let map = bindings.borrow();
    // The last binding added wins: `install` adds the user's overrides after the defaults.
    map.bindings_for_action(action.as_ref())
        .last()
        .map(|b| {
            b.keystrokes()
                .iter()
                .map(|k| k.unparse())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_else(|| platform_keys(d.keys))
}

/// Install default bindings, then the user's overrides on top. A rebound action loses its
/// default keys: gpui would otherwise keep both, and the old keys would go on firing while
/// Settings shows only the new ones. An override that cannot be bound (empty or invalid
/// keys) is reported and ignored, and its action keeps the default (rule 11).
pub fn install(cx: &mut App, overrides: &Overrides) {
    let mut replaced = std::collections::BTreeSet::new();
    let mut user = Vec::new();
    for (action, o) in &overrides.0 {
        let ctx = doc(action).and_then(|d| d.context);
        match binding(action, o.keys(), ctx) {
            Some(b) => {
                replaced.insert(action.as_str());
                user.push(b);
            }
            None => eprintln!("utopia: ignoring keybinding {} -> {action}", o.keys()),
        }
    }
    let mut bindings: Vec<KeyBinding> = CATALOG
        .iter()
        .filter(|d| !replaced.contains(d.action))
        .filter_map(|d| binding(d.action, &platform_keys(d.keys), d.context))
        .collect();
    // Overrides come last: gpui lets a later binding win for the same keys.
    bindings.extend(user);
    cx.bind_keys(bindings);
}

/// Replace every binding with the defaults plus `overrides`. Used when Settings saves.
pub fn reinstall(cx: &mut App, overrides: &Overrides) {
    cx.clear_key_bindings();
    install(cx, overrides);
}

/// Install default bindings, then a raw `keymap.json` on top. Kept for startup, where the
/// file is read before anything is known about the catalog.
pub fn install_keymap(cx: &mut App, user_keymap: Option<&str>) {
    let overrides = match user_keymap {
        Some(json) => {
            let (o, warnings) = Overrides::from_json(json);
            for w in warnings {
                eprintln!("utopia: {w}");
            }
            o
        }
        None => Overrides::default(),
    };
    install(cx, &overrides);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_binding_resolves() {
        for d in CATALOG {
            assert!(
                binding(d.action, &platform_keys(d.keys), d.context).is_some(),
                "{} {}",
                d.action,
                d.keys
            );
        }
    }

    /// The catalog is what the Settings editor and the cheat sheet list, so a default that
    /// shadows another one would be invisible in the UI and dead at runtime.
    #[test]
    fn defaults_do_not_conflict() {
        let c = Overrides::default().conflicts();
        assert!(c.is_empty(), "conflicting defaults: {c:?}");
    }

    /// QJ-5: every command the palette can list has an action to run, spelled the way the
    /// catalog spells it. A name added to the catalog without a match here would show up in
    /// the palette and do nothing when picked.
    #[test]
    fn every_catalog_action_can_be_run() {
        for d in CATALOG {
            assert!(action_for(d.action).is_some(), "{} has no action", d.action);
        }
        assert!(action_for("Nope").is_none());
    }

    #[test]
    fn catalog_has_no_duplicate_actions() {
        let mut seen = std::collections::BTreeSet::new();
        for d in CATALOG {
            assert!(seen.insert(d.action), "{} listed twice", d.action);
        }
    }

    /// The four bindings this feature was asked for (§9.1).
    #[test]
    fn requested_defaults_are_bound() {
        for (action, keys) in [
            ("GoDailyNotes", "cmd-shift-d"),
            ("GoToday", "cmd-shift-t"),
            ("GoYesterday", "cmd-shift-y"),
            ("OpenSettings", "cmd-,"),
        ] {
            let d = doc(action).unwrap_or_else(|| panic!("{action} missing from the catalog"));
            assert_eq!(d.keys, keys, "{action}");
        }
    }

    #[test]
    fn unknown_actions_are_reported_not_fatal() {
        let (b, w) = parse_keymap(
            r#"[{"key":"cmd-j","action":"QuickJump"},{"key":"cmd-y","action":"Nope"}]"#,
        );
        assert_eq!(b.len(), 1);
        assert_eq!(w.len(), 1);
        let (b, w) = parse_keymap("not json");
        assert!(b.is_empty() && w.len() == 1);
    }

    #[test]
    fn overrides_round_trip_through_json() {
        let mut o = Overrides::default();
        o.set("GoToday", "cmd-alt-t");
        o.set("Nope", "cmd-j"); // not in the catalog: ignored
        assert_eq!(o.len(), 1);
        let (back, warnings) = Overrides::from_json(&o.to_json());
        assert_eq!(back, o);
        assert!(warnings.is_empty());
        assert_eq!(back.keys_for(doc("GoToday").unwrap()), "cmd-alt-t");
        // An untouched action still reports its default.
        assert_eq!(
            back.keys_for(doc("GoYesterday").unwrap()),
            platform_keys("cmd-shift-y")
        );
    }

    #[test]
    fn reset_returns_to_the_default() {
        let mut o = Overrides::default();
        o.set("GoToday", "cmd-alt-t");
        assert!(o.is_changed("GoToday"));
        o.reset("GoToday");
        assert!(!o.is_changed("GoToday"));
        assert_eq!(
            o.keys_for(doc("GoToday").unwrap()),
            platform_keys("cmd-shift-t")
        );
    }

    #[test]
    fn overrides_can_introduce_a_conflict() {
        let mut o = Overrides::default();
        o.set("GoToday", platform_keys("cmd-shift-y").as_str());
        let c = o.conflicts();
        assert_eq!(c.len(), 1);
        let names = [c[0].0.action, c[0].1.action];
        assert!(names.contains(&"GoToday") && names.contains(&"GoYesterday"));
    }

    #[test]
    fn modifier_order_does_not_create_a_second_binding() {
        assert_eq!(canonical_keys("cmd-alt-t"), canonical_keys("alt-cmd-t"));
        // A clash is a clash however the chord is spelled.
        let mut o = Overrides::default();
        o.set("GoToday", "alt-cmd-down"); // ZoomIn's default, written the other way round
        assert_eq!(
            doc("ZoomIn").unwrap().keys,
            "cmd-alt-down",
            "this test is about that spelling"
        );
        // Different contexts, so not a conflict; but the canonical forms must match.
        assert_eq!(
            canonical_keys(&o.keys_for(doc("GoToday").unwrap())),
            canonical_keys(&platform_keys(doc("ZoomIn").unwrap().keys))
        );
    }

    #[test]
    fn keys_render_for_people() {
        if cfg!(target_os = "macos") {
            assert_eq!(pretty_keys("cmd-shift-d"), "⇧⌘D");
            assert_eq!(pretty_keys("cmd-,"), "⌘,");
            assert_eq!(pretty_keys("alt-enter"), "⌥↩");
            assert_eq!(pretty_keys("ctrl-cmd-space"), "⌃⌘Space");
            assert_eq!(pretty_keys("shift-tab"), "⇧⇥");
        }
    }

    #[test]
    fn invalid_keys_are_refused() {
        assert!(binding("GoToday", "", None).is_none());
        assert!(binding("GoToday", "nonsense-key", None).is_none());
        assert!(binding("GoToday", "cmd-shift-t", None).is_some());
    }
}
