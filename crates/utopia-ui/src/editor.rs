//! The block editor (§6.2, §13.5): one focused block owns the caret. Text edits persist
//! on every keystroke (coalesced into one undo step per 500 ms), the block is committed on
//! blur, Enter or 300 ms idle (AC-3.2), and outliner gestures are emitted as
//! [`EditorEvent`]s for the hosting view, which knows the visible row order.

use std::time::Duration;

use gpui::{
    anchored, deferred, div, prelude::*, px, ClipboardItem, Context, Entity, EntityId,
    EventEmitter, FocusHandle, Focusable, HighlightStyle, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, SharedString, StyledText, Subscription, Task,
    UnderlineStyle, Window,
};
use utopia_core::inline::{is_tag_char, page_link_text};
use utopia_core::title::normalize_title;
use utopia_core::undo::Caret;
use utopia_core::{BlockId, BlockKind, PageId};

use crate::actions::*;
use crate::app_state::{AppState, PageReplaced};
use crate::field::{FieldElement, FieldHost, Goal, TextField};
use crate::rich::source_highlights;
use crate::theme::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditTarget {
    /// The view whose row hosts the editor (the same block can be visible twice).
    pub host: EntityId,
    pub page: PageId,
    pub block: BlockId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditorEvent {
    Split {
        caret: usize,
    },
    InsertAbove,
    Indent,
    Outdent,
    MergeBackward,
    Navigate {
        up: bool,
        x: Option<Pixels>,
    },
    MoveBlock {
        up: bool,
    },
    ToggleCollapse,
    ToggleCollapseAll,
    ZoomIn,
    ZoomOut,
    DeleteBlock,
    PasteOutline(String),
    /// Slash menu: extract the block to a page (asks for a title).
    ExtractRequested,
    /// Slash menu: insert a page template after the block.
    InsertTemplate(PageId),
    /// After undo/redo: where the caret should go.
    Restore(Caret),
    Blurred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcKind {
    /// `[[query`
    Page,
    /// `#[[query`
    BracketTag,
    /// `#query`
    Tag,
    /// `((query`
    Block,
    /// `/query`: the slash command menu (P1).
    Slash,
}

/// Slash menu commands (§4.2 P1: block types, templates, extract, embeds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCmd {
    Kind(BlockKind),
    Today,
    Embed,
    Query,
    Attach,
    TagPhrase,
    Extract,
    Template(PageId),
    /// `(plugin index, command id)` (P2 plugin runtime).
    Plugin(usize, String),
}

/// The static part of the slash menu.
pub fn slash_commands() -> Vec<(SlashCmd, &'static str)> {
    vec![
        (SlashCmd::Kind(BlockKind::Todo), "To-do"),
        (SlashCmd::Kind(BlockKind::Heading(1)), "Heading 1"),
        (SlashCmd::Kind(BlockKind::Heading(2)), "Heading 2"),
        (SlashCmd::Kind(BlockKind::Heading(3)), "Heading 3"),
        (SlashCmd::Kind(BlockKind::Quote), "Quote"),
        (SlashCmd::Kind(BlockKind::Code), "Code block"),
        (SlashCmd::Kind(BlockKind::Numbered), "Numbered list"),
        (SlashCmd::Kind(BlockKind::Bullet), "Bullet (plain)"),
        (SlashCmd::Today, "Today's date"),
        (SlashCmd::Embed, "Embed a block"),
        (SlashCmd::Query, "Query"),
        (SlashCmd::Attach, "Attach a file"),
        (SlashCmd::TagPhrase, "Tag the whole phrase"),
        (SlashCmd::Extract, "Extract block to page"),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcItem {
    Page {
        title: String,
        alias: Option<String>,
    },
    Create(String),
    Command {
        cmd: SlashCmd,
        label: String,
    },
    Block {
        uid: String,
        text: String,
        page_title: String,
    },
}

/// The completion menu as one explicit state
/// (`docs/state-machines/block-editor-autocomplete.md`, FR-2.17). "A trigger is under the
/// caret" and "there is a suggestion to take" used to be two different questions answered
/// with `is_some()` and `is_some_and(|a| !a.items.is_empty())` at six call sites; every
/// handler now asks [`Autocomplete::captures_keys`] instead.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Autocomplete {
    /// No trigger under the caret: nothing draws, nothing captures a key.
    #[default]
    Closed,
    /// The user dismissed this trigger (Escape, a link they just wrapped, a tag they
    /// accepted); it stays shut until the caret leaves it.
    Suppressed { trigger_start: usize },
    /// A trigger was found and its items computed. `items` may be empty (`/xyz`, `#zzz`,
    /// `((zzz` with no hits): then nothing is drawn and no key is captured, which is why
    /// no handler asks whether the variant is `Open`.
    Open(AutocompleteMenu),
}

/// An open menu's frozen query: the items, the range they replace and the highlight.
#[derive(Debug, Clone, PartialEq)]
pub struct AutocompleteMenu {
    pub kind: AcKind,
    pub start: usize,
    pub query: String,
    pub items: Vec<AcItem>,
    pub selected: usize,
    /// The caret when `items` were computed. `accept` replaces `start..caret`, so moving
    /// the caret after the menu opened (Shift+Left selects the last character) cannot
    /// replace a different range than the one the query covers.
    pub caret: usize,
}

impl Autocomplete {
    /// The menu owns the keyboard: it has suggestions to choose from.
    pub fn captures_keys(&self) -> bool {
        matches!(self, Autocomplete::Open(menu) if !menu.items.is_empty())
    }

    /// A trigger is under the caret (its menu may be empty, and so invisible).
    pub fn is_open(&self) -> bool {
        matches!(self, Autocomplete::Open(_))
    }

    pub fn menu(&self) -> Option<&AutocompleteMenu> {
        match self {
            Autocomplete::Open(menu) => Some(menu),
            _ => None,
        }
    }

    pub fn kind(&self) -> Option<AcKind> {
        self.menu().map(|m| m.kind)
    }

    /// Keep this trigger shut until the caret leaves it.
    fn suppress(&mut self, trigger_start: usize) {
        *self = Autocomplete::Suppressed { trigger_start };
    }
}

/// The reference trigger the caret is inside, if any (FR-2.17).
pub fn detect_trigger(text: &str, cursor: usize) -> Option<(AcKind, usize, String)> {
    let before = text.get(..cursor)?;
    if let Some(i) = before.rfind("[[") {
        let q = &before[i + 2..];
        if !q.contains("]]") && !q.contains('\n') {
            return Some(if before[..i].ends_with('#') {
                (AcKind::BracketTag, i - 1, q.to_string())
            } else {
                (AcKind::Page, i, q.to_string())
            });
        }
    }
    if let Some(i) = before.rfind("((") {
        let q = &before[i + 2..];
        if !q.contains("))") && !q.contains('\n') {
            return Some((AcKind::Block, i, q.to_string()));
        }
    }
    if let Some(i) = before.rfind('#') {
        let q = &before[i + 1..];
        let boundary = before[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '#');
        if boundary && q.chars().all(is_tag_char) {
            return Some((AcKind::Tag, i, q.to_string()));
        }
    }
    if let Some(i) = before.rfind('/') {
        let q = &before[i + 1..];
        let boundary = before[..i]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        if boundary
            && q.len() <= 24
            && q.chars().all(|c| c.is_alphanumeric() || c == ' ')
            && !q.ends_with("  ")
        {
            return Some((AcKind::Slash, i, q.to_string()));
        }
    }
    None
}

/// Text inserted for an autocomplete choice, and how far past the caret an already
/// typed closing delimiter extends the replaced range.
pub fn completion_text(kind: AcKind, item: &AcItem, after_caret: &str) -> (String, usize) {
    let title = match item {
        AcItem::Page { title, .. } | AcItem::Create(title) => title.clone(),
        AcItem::Block { uid, .. } => {
            let skip = if after_caret.starts_with("))") { 2 } else { 0 };
            return (format!("(({uid}))"), skip);
        }
        AcItem::Command { .. } => return (String::new(), 0),
    };
    match kind {
        AcKind::Page => (
            page_link_text(&title, false),
            if after_caret.starts_with("]]") { 2 } else { 0 },
        ),
        AcKind::BracketTag => (
            format!("#[[{title}]]"),
            if after_caret.starts_with("]]") { 2 } else { 0 },
        ),
        AcKind::Tag => (page_link_text(&title, true), 0),
        AcKind::Block | AcKind::Slash => (format!("[[{title}]]"), 0),
    }
}

/// A host request that arrived while the input method owned the field (§13.5). None of
/// these may run then: finishing the block reads and rewrites the buffer through the
/// provisional glyphs, and starting another block replaces the buffer outright
/// (`set_text`). They wait for the input method instead (`after_composition`).
///
/// `end` and a focus move do not wait: they take the marked text as committed
/// (`accept_composition`), because the platform never hands another IME event to a field
/// that has stopped being its input handler.
#[derive(Debug, Clone, PartialEq)]
enum Deferred {
    /// Finish the block and carry on editing it; `finished: false` is the idle commit.
    Commit { finished: bool },
    /// `begin` elsewhere (another block, or another caret in this one).
    Begin {
        target: EditTarget,
        caret: Option<usize>,
        goal: Option<Goal>,
    },
}

impl Deferred {
    /// The idle commit is the only request that may not displace one already queued: it is
    /// not a user action, just housekeeping that the next keystroke re-arms anyway.
    fn is_idle(&self) -> bool {
        matches!(self, Deferred::Commit { finished: false })
    }
}

/// One editing session (§6.2, `docs/state-machines/block-editor-session.md`): the block
/// that owns the caret together with everything that may only exist while it does. One
/// value, so `target`, its CRDT anchor, the idle commit and the input method's queued
/// request are created and dropped together and cannot disagree about whether a block is
/// open.
///
/// The variants are not boxed: the session was already stored inline as five separate
/// fields, so the enum costs an editor entity nothing extra per edit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum EditorSession {
    Idle,
    Editing(Editing),
}

#[derive(Debug)]
pub struct Editing {
    target: EditTarget,
    /// The caret anchored in the text CRDT, so remote edits don't move it (§6.5).
    anchor: Option<utopia_store::DocCursor>,
    /// The idle commit an edit armed (AC-3.2). Replacing it cancels the old one.
    pending: Option<Task<()>>,
    /// What a host asked for while the input method was composing (§13.5).
    deferred: Option<Deferred>,
    /// A store change arrived while the input method owned the buffer (§13.5):
    /// `sync_from_store` could not look, so it looks once the composition ends.
    pending_sync: bool,
}

impl Editing {
    fn new(target: EditTarget) -> Self {
        Self {
            target,
            anchor: None,
            pending: None,
            deferred: None,
            pending_sync: false,
        }
    }
}

impl EditorSession {
    fn target(&self) -> Option<&EditTarget> {
        match self {
            Self::Idle => None,
            Self::Editing(e) => Some(&e.target),
        }
    }

    fn target_mut(&mut self) -> Option<&mut EditTarget> {
        match self {
            Self::Idle => None,
            Self::Editing(e) => Some(&mut e.target),
        }
    }

    fn editing_mut(&mut self) -> Option<&mut Editing> {
        match self {
            Self::Idle => None,
            Self::Editing(e) => Some(e),
        }
    }
}

pub struct BlockEditor {
    app: Entity<AppState>,
    field: TextField,
    focus: FocusHandle,
    session: EditorSession,
    ac: Autocomplete,
    _sub: Subscription,
    _page_sub: Subscription,
}

impl EventEmitter<EditorEvent> for BlockEditor {}

impl Focusable for BlockEditor {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl FieldHost for BlockEditor {
    fn field(&self) -> &TextField {
        &self.field
    }
    fn field_mut(&mut self) -> &mut TextField {
        &mut self.field
    }
    fn focus_handle_ref(&self) -> &FocusHandle {
        &self.focus
    }
}

crate::impl_field_input_handler!(
    BlockEditor,
    BlockEditor::after_edit,
    BlockEditor::typed_bracket
);

impl BlockEditor {
    pub fn new(app: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&app, |this, _, cx| this.sync_from_store(cx));
        // The page this editor is typing into was merged into another one (FR-1.2): keep
        // editing the same block, now on the page that owns the day, so the next keystroke
        // does not go to a shard that no longer exists.
        let page_sub = cx.subscribe(&app, |this, _, e: &PageReplaced, cx| {
            let Some(target) = this.session.target_mut() else {
                return;
            };
            if target.page != e.from {
                return;
            }
            target.page = e.to.clone();
            this.anchor_caret(cx);
            cx.notify();
        });
        Self {
            app,
            field: TextField::new(true),
            focus: cx.focus_handle(),
            session: EditorSession::Idle,
            ac: Autocomplete::Closed,
            _sub: sub,
            _page_sub: page_sub,
        }
    }

    /// The block owning the caret, if any. The host reads this to focus the editor for an
    /// editor-bound command (QJ-5) and the views to draw its row as an editor (PG-1).
    pub fn target(&self) -> Option<&EditTarget> {
        self.session.target()
    }

    /// The completion menu's state (FR-2.17,
    /// `docs/state-machines/block-editor-autocomplete.md`). Read-only: only this editor
    /// moves it.
    pub fn autocomplete(&self) -> &Autocomplete {
        &self.ac
    }

    pub fn is_editing(&self, host: EntityId, page: &PageId, block: &BlockId) -> bool {
        self.target()
            .is_some_and(|t| t.host == host && t.page == *page && t.block == *block)
    }

    pub fn cursor(&self) -> usize {
        self.field.cursor()
    }

    pub fn selection(&self) -> std::ops::Range<usize> {
        self.field.sel.clone()
    }

    /// The text being edited, as typed (before the block commits).
    pub fn field_text(&self) -> &str {
        &self.field.text
    }

    /// Start editing `target`, committing the previous block first.
    pub fn begin(
        &mut self,
        target: EditTarget,
        caret: Option<usize>,
        goal: Option<Goal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.field.composing() {
            // §13.5: starting somewhere else replaces the buffer with stored text, which
            // would wipe the input method's live composition while it still addresses it.
            self.defer(Deferred::Begin {
                target,
                caret,
                goal,
            });
            return;
        }
        if self.target() != Some(&target) {
            self.commit(cx);
            let text = self.app.update(cx, |app, _| {
                app.store
                    .page_state(&target.page)
                    .ok()
                    .and_then(|st| st.block(&target.block).map(|b| b.text.clone()))
            });
            let Some(text) = text else {
                // The block went away between the click and the load: there is nothing to
                // edit, and keeping the previous target would leave the editor on a block
                // the host has already left.
                self.session = EditorSession::Idle;
                self.ac = Autocomplete::Closed;
                cx.emit(EditorEvent::Blurred);
                cx.notify();
                return;
            };
            self.field.set_text(&text, caret.unwrap_or(text.len()));
            self.session = EditorSession::Editing(Editing::new(target));
            self.ac = Autocomplete::Closed;
        } else if let Some(c) = caret {
            self.field.move_to(c);
        }
        self.field.goal = goal;
        self.anchor_caret(cx);
        window.focus(&self.focus);
        cx.notify();
    }

    /// Stop editing (Escape, navigation). Commits the block. Emits `Blurred` so the
    /// workspace can take focus back if nothing else claims it.
    pub fn end(&mut self, cx: &mut Context<Self>) {
        if self.field.composing() {
            // Every caller ends an edit because it is about to move focus (a page, the
            // palette, the graph, another pane), and gpui gives the platform's input
            // context no hint: the IME's commit can never reach a field that stopped being
            // its input handler. Close the composition here instead (§13.5).
            self.accept_composition(cx);
        }
        self.commit(cx);
        let was_editing = !matches!(self.session, EditorSession::Idle);
        // Dropping the session drops the idle commit and the caret anchor with it: neither
        // may outlive the block they belong to (machine issues 2 and 4).
        self.session = EditorSession::Idle;
        self.ac = Autocomplete::Closed;
        if was_editing {
            cx.emit(EditorEvent::Blurred);
        }
        cx.notify();
    }

    fn sync_from_store(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.target().cloned() else {
            return;
        };
        if self.field.composing() {
            // §13.5: the buffer is the input method's. Neither rewriting it under the
            // composition nor dropping the change is acceptable, so look again later.
            if let Some(e) = self.session.editing_mut() {
                e.pending_sync = true;
            }
            return;
        }
        match self.stored_text(&t, cx) {
            Some(text) if text != self.field.text => {
                // Changed underneath us (undo, rewrite, sync): keep the caret in range.
                let anchored = self.session.editing_mut().and_then(|e| e.anchor.clone());
                let anchored = anchored.and_then(|a| {
                    self.app
                        .update(cx, |app, _| app.store.resolve_caret(&t.page, &a))
                });
                let c = anchored.unwrap_or_else(|| self.field.cursor().min(text.len()));
                self.field.set_text(&text, c);
                self.anchor_caret(cx);
                // The buffer was replaced wholesale, so the menu's frozen range and items
                // describe text that is gone (an undo, a remote edit, a plugin through the
                // store). Re-derive it from the text that is here now (FR-2.17).
                self.refresh_autocomplete(cx);
                cx.notify();
            }
            Some(_) => {}
            None => self.detach(cx),
        }
    }

    /// The target's stored text, or `None` when the block went away (its creation undone,
    /// deleted, merged). `Some("")` is a live empty block.
    fn stored_text(&self, t: &EditTarget, cx: &mut Context<Self>) -> Option<String> {
        self.app.update(cx, |app, _| {
            app.store.page_state(&t.page).ok().and_then(|st| {
                st.block(&t.block)
                    .filter(|b| b.live)
                    .map(|b| b.text.clone())
            })
        })
    }

    /// Stop editing: the block went away (undo of its creation, deletion, merge).
    fn detach(&mut self, cx: &mut Context<Self>) {
        self.session = EditorSession::Idle;
        self.ac = Autocomplete::Closed;
        cx.emit(EditorEvent::Blurred);
        cx.notify();
    }

    fn anchor_caret(&mut self, cx: &mut Context<Self>) {
        let Some(t) = self.target().cloned() else {
            return;
        };
        let caret = self.field.cursor();
        let anchor = self.app.update(cx, |app, _| {
            app.store.caret_cursor(&t.page, &t.block, caret)
        });
        if let Some(e) = self.session.editing_mut() {
            e.anchor = anchor;
        }
    }

    /// Run a plugin command on the block being edited and apply what it returns.
    pub fn run_plugin(
        &mut self,
        index: usize,
        command: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.target().cloned() else {
            return;
        };
        let input = serde_json::json!({
            "page": t.page.as_str(),
            "block": t.block.as_str(),
            "text": self.field.text,
        });
        let result = self.app.update(cx, |app, cx| {
            let plugin = app.plugins.get(index).cloned()?;
            let run = plugin.run(&mut app.store, command, input);
            // Host writes commit inside the call, so they are reported whether or not the
            // run then failed (rule 13; plugin run lifecycle issue 1).
            if !run.changed.is_empty() {
                app.changed(run.changed.clone(), true, cx);
            }
            match &run.outcome {
                Ok(out) => app.notice = out.message.clone(),
                Err(e) => app.last_error = Some(format!("{}: {e}", plugin.manifest.name)),
            }
            Some(run)
        });
        let Some(run) = result else {
            cx.notify();
            return;
        };
        let out = run.outcome.as_ref().ok();
        let replace = out.and_then(|o| o.replace.clone());
        let mut rewrote = false;
        if let Some(text) = replace {
            let len = text.len();
            self.field.set_text(&text, len);
            rewrote = true;
        } else if run.changed.contains(&t.page) {
            // The plugin edited this page through the API: show the stored text.
            let stored = self.app.update(cx, |app, _| {
                app.store
                    .page_state(&t.page)
                    .ok()
                    .and_then(|st| st.block(&t.block).map(|b| b.text.clone()))
            });
            if let Some(s) = stored.filter(|s| *s != self.field.text) {
                let len = s.len();
                self.field.set_text(&s, len);
                rewrote = true;
            }
        }
        if rewrote || run.changed.contains(&t.page) {
            // A plugin replaced the buffer under the editor: persist what it asked for,
            // move the CRDT caret with the new text and re-read the trigger. The anchor
            // and the autocomplete menu were derived from the text that is gone (machine
            // issue 3), and `settle` is the one path that makes a rewritten buffer
            // consistent again (§13.5).
            let persisted = self.settle(cx);
            self.after_composition(window, cx, persisted);
        }
        let insert = out.and_then(|o| o.insert.clone());
        if let Some(text) = insert {
            self.insert_text(&text, window, cx);
        }
        cx.notify();
    }

    /// Type `text` at the caret (replacing any selection). `false` when not editing.
    pub fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.target().is_none() || self.field.composing() {
            return false;
        }
        let sel = self.field.sel.clone();
        let start = sel.start.min(sel.end);
        self.field.replace(sel, text);
        self.field.move_to(start + text.len());
        self.after_edit(window, cx);
        cx.notify();
        true
    }

    /// Copy files into the graph's blob store and insert their markup at the caret
    /// (P2 attachments). Returns whether anything was inserted.
    pub fn attach_paths(
        &mut self,
        paths: &[std::path::PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let markup: Vec<String> = self.app.update(cx, |app, _| {
            paths
                .iter()
                .filter_map(|p| {
                    let r = app.store.attach_file(p);
                    app.report(r)
                })
                .map(|a| utopia_core::attach::markup(&a.name, &a.hash, &a.mime))
                .collect()
        });
        !markup.is_empty() && self.insert_text(&markup.join(" "), window, cx)
    }

    /// Brackets typed at the caret (§13.5). Over a selection, `(`, `[` or `{` surrounds it
    /// and keeps it selected, so `[` twice turns `word` into `[[word]]` (Roam's link
    /// idiom). With no selection they pair (`(|)`) and a typed closer steps over the one
    /// already there; see [`utopia_core::text::auto_pair`]. Each is one text edit, so one
    /// undo step like any keystroke. Only for plain typing at the selection: never while
    /// IME text is being composed.
    fn typed_bracket(
        &mut self,
        range: std::ops::Range<usize>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.target().is_none() || self.field.composing() || range != self.field.sel {
            return false;
        }
        if range.is_empty() {
            let Some((paired, caret)) =
                utopia_core::text::auto_pair(&self.field.text, range.start, text)
            else {
                return false;
            };
            if paired == self.field.text {
                self.field.move_to(caret);
                self.moved(cx);
            } else {
                self.field.text = paired;
                self.field.move_to(caret);
                self.after_edit(window, cx);
            }
            return true;
        }
        let Some((wrapped, sel)) = utopia_core::text::wrap_selection(&self.field.text, range, text)
        else {
            return false;
        };
        let reversed = self.field.reversed;
        self.field.text = wrapped;
        self.field.sel = sel;
        self.field.reversed = reversed;
        self.after_edit(window, cx);
        // `[[word` now looks like a link being typed, but the user just made the link;
        // a menu here would let Enter swap their word for some other page.
        self.dismiss_trigger();
        true
    }

    fn after_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.composing() {
            return;
        }
        let persisted = self.settle(cx);
        self.after_composition(window, cx, persisted);
    }

    /// Persist what the buffer holds and look at the store again (§13.5), once the
    /// composition is over whichever way it ended. `true` when the stored text changed.
    fn settle(&mut self, cx: &mut Context<Self>) -> bool {
        let pending_sync = self
            .session
            .editing_mut()
            .is_some_and(|e| std::mem::take(&mut e.pending_sync));
        if pending_sync {
            // A store change landed while the input method owned the buffer. A block that
            // merely changed keeps what the user just committed: the persist below is
            // newer (block text is last-writer-wins). A block that went away first takes
            // the composed text with it, into the tombstone, and then the edit ends.
            let gone = self
                .target()
                .cloned()
                .is_some_and(|t| self.stored_text(&t, cx).is_none());
            if gone {
                // Keep what the input method typed (rule 5): the block is tombstoned but
                // still in the store, so an undo or the trash brings the characters back,
                // and only emptying the trash loses them.
                self.persist(cx);
                self.detach(cx);
                return false;
            }
        }
        let changed = self.persist(cx);
        self.anchor_caret(cx);
        self.refresh_autocomplete(cx);
        changed
    }

    /// Take the input method's marked text as committed (§13.5): `unmarkText` semantics by
    /// hand, for the paths where the platform will not send one. Anything a host queued for
    /// this field is stale — the host is moving focus, so a queued `begin` would put the
    /// caret on a row the host has left.
    fn accept_composition(&mut self, cx: &mut Context<Self>) {
        self.field.end_composition();
        if let Some(e) = self.session.editing_mut() {
            e.deferred = None;
        }
        self.settle(cx);
    }

    /// The editor's handle lost focus (§13.5, AC-3.2): the keyboard is on another element
    /// — a palette, a search box, another pane, another application — nobody called `end`,
    /// so this is the blur the module doc has always promised. The input method can no
    /// longer reach this field, so a live composition is closed here, and the block is
    /// committed at once. Waiting for the idle timer is not enough: it leaves a link the
    /// caret is still inside alone (`typing_link`), so an abandoned `[[tomorrow]]` stayed
    /// unexpanded until some host happened to call `end` (machine issue 1).
    ///
    /// The session stays open. A palette that took the keyboard can dispatch an editor
    /// command back to this block (QJ-5, `quick_jump_runs_editor_commands`), so ending the
    /// edit here would break a documented shortcut; the host's `end` — Escape, navigation,
    /// opening a page — is what ends an edit. Nothing runs meanwhile: the commit dropped
    /// the idle timer, and a session that survives a blur has no caret to maintain.
    pub fn focus_lost(&mut self, cx: &mut Context<Self>) {
        if self.field.composing() {
            self.accept_composition(cx);
        }
        self.commit_block(true, cx);
        // A menu is a keyboard affordance and the keyboard is elsewhere now; the commit
        // above may also have rewritten the text it was offering completions for (an
        // expanded relative date), so accepting from it later would edit the wrong range.
        self.ac = Autocomplete::Closed;
    }

    /// Queue a host request that must wait for the input method (§13.5). The latest request
    /// wins — the user's last click or command is the one they meant — except that the idle
    /// commit never displaces anything.
    fn defer(&mut self, next: Deferred) {
        let Some(e) = self.session.editing_mut() else {
            // Nothing is open, so there is nothing to finish or to begin.
            return;
        };
        if next.is_idle() && e.deferred.is_some() {
            return;
        }
        e.deferred = Some(next);
    }

    /// The input method let go of the buffer (§13.5): do what a host asked for meanwhile.
    fn after_composition(&mut self, window: &mut Window, cx: &mut Context<Self>, persisted: bool) {
        let next = self.session.editing_mut().and_then(|e| e.deferred.take());
        match next {
            None => {}
            // The composition's own edit re-armed the idle commit, so let that run 300 ms
            // from now as it would without a composition, instead of committing (NFC,
            // hashtags, page creation, undo seal) synchronously for every IME phrase.
            Some(Deferred::Commit { finished: false }) if persisted => {}
            Some(Deferred::Commit { finished }) => self.commit_block(finished, cx),
            Some(Deferred::Begin {
                target,
                caret,
                goal,
            }) => self.begin(target, caret, goal, window, cx),
        }
    }

    /// Write the buffer to the store; `true` when the block's text changed (which is also
    /// when the idle commit is re-armed).
    fn persist(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(t) = self.target().cloned() else {
            return false;
        };
        let text = self.field.text.clone();
        let caret = self.field.cursor();
        let changed = self.app.update(cx, |app, cx| {
            let r = app
                .store
                .set_block_text(&t.page, &t.block, &text, None, Some(caret));
            let changed = app.report(r).unwrap_or(false);
            if changed {
                app.changed(vec![t.page.clone()], false, cx);
            }
            changed
        });
        if changed {
            // Replacing `pending` cancels the previous task, so one idle task encodes "a
            // commit is armed" on its own: there is no edit generation to keep in step
            // with it (machine issue 5).
            let task = cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                // `Ok(false)`: still inside a link being typed, so look again later.
                let done = this.update(cx, |this, cx| {
                    if this.typing_link() {
                        return false;
                    }
                    this.commit_block(false, cx);
                    true
                });
                if !matches!(done, Ok(false)) {
                    break;
                }
            });
            if let Some(e) = self.session.editing_mut() {
                e.pending = Some(task);
            }
        }
        changed
    }

    /// The caret is inside a `[[…]]`/`#[[…]]` still being typed. Auto-pairing closes the
    /// link before its title is finished, so committing on idle here would create a page
    /// for every pause (`[[An` → "An"). The idle commit waits until the caret has left
    /// the link; blur, Enter and Escape commit at once, as before.
    fn typing_link(&self) -> bool {
        matches!(
            detect_trigger(&self.field.text, self.field.cursor()),
            Some((AcKind::Page | AcKind::BracketTag, ..))
        )
    }

    /// Commit the current block: NFC, hashtag longest-match, page creation for new links.
    /// This is the block being finished (blur, Enter, Escape, navigation), so relative
    /// dates are expanded even in the link the caret is in.
    pub fn commit(&mut self, cx: &mut Context<Self>) {
        self.commit_block(true, cx);
    }

    /// `finished: false` is the idle commit, which leaves the link being typed alone.
    fn commit_block(&mut self, finished: bool, cx: &mut Context<Self>) {
        if self.field.composing() {
            // §13.5: the buffer holds provisional glyphs, so expanding relative dates or
            // re-reading the block after the store commit would run through text the
            // store has never seen, and writing the result back would wipe the
            // composition. The next commit attempt is the input method letting go.
            self.defer(Deferred::Commit { finished });
            return;
        }
        let Some(t) = self.target().cloned() else {
            return;
        };
        if let Some(e) = self.session.editing_mut() {
            e.pending = None;
        }
        self.expand_relative_dates(&t, finished, cx);
        let before = self.field.text.clone();
        let caret = self.field.cursor();
        let after = self.app.update(cx, |app, cx| {
            let r = app.store.commit_block(&t.page, &t.block);
            app.report(r);
            app.store.seal_undo();
            app.changed(vec![t.page.clone()], true, cx);
            app.store
                .page_state(&t.page)
                .ok()
                .and_then(|st| st.block(&t.block).map(|b| b.text.clone()))
        });
        if let Some(after) = after.filter(|a| *a != before) {
            let shift = after.len() as isize - before.len() as isize;
            let c = if caret >= before.len() {
                after.len()
            } else {
                (caret as isize + shift.min(0)).max(0) as usize
            };
            self.field.set_text(&after, c);
            // The commit may rewrite the text under the caret; the anchor has to move with
            // it, or a remote edit lands the caret where the old text put it.
            self.anchor_caret(cx);
            cx.notify();
        }
    }

    /// `[[yesterday]]`, `[[next sunday]]` → a link to that day's page, once the link is
    /// closed and the caret has left it, or the block is `finished` (a wrapped or
    /// auto-closed link leaves the caret inside it). The phrase is replaced, not resolved
    /// later, because its meaning changes every day (§10.3.1).
    fn expand_relative_dates(&mut self, t: &EditTarget, finished: bool, cx: &mut Context<Self>) {
        let caret = self.field.cursor();
        let expanded = self.app.update(cx, |app, cx| {
            let store = &app.store;
            let exists = |title: &str| store.find_page(title).ok().flatten().is_some();
            let (today, format) = (store.today(), store.date_format());
            let (text, caret) = if finished {
                utopia_core::date::finish_relative_date_links(
                    &self.field.text,
                    caret,
                    today,
                    format,
                    exists,
                )
            } else {
                utopia_core::date::expand_relative_date_links(
                    &self.field.text,
                    caret,
                    today,
                    format,
                    exists,
                )
            }?;
            let r = app
                .store
                .set_block_text(&t.page, &t.block, &text, None, Some(caret));
            app.report(r)?;
            app.changed(vec![t.page.clone()], false, cx);
            Some((text, caret))
        });
        if let Some((text, caret)) = expanded {
            self.field.set_text(&text, caret);
            // The rewrite moved the caret with it (the day's title is longer than the
            // phrase it replaced, and a link the caret was in now ends past it), so the
            // CRDT anchor must move too or the next store change resolves the caret back
            // into the expanded link.
            self.anchor_caret(cx);
            cx.notify();
        }
    }

    fn refresh_autocomplete(&mut self, cx: &mut Context<Self>) {
        let cursor = self.field.cursor();
        let Some((kind, start, query)) = detect_trigger(&self.field.text, cursor) else {
            self.ac = Autocomplete::Closed;
            return;
        };
        if let Autocomplete::Suppressed { trigger_start } = &self.ac {
            if *trigger_start == start {
                return;
            }
        }
        let own = self.target().map(|t| t.block.clone());
        let items = self.app.update(cx, |app, _| -> Vec<AcItem> {
            match kind {
                AcKind::Slash => {
                    let q = query.trim().to_lowercase();
                    let mut all: Vec<(SlashCmd, String)> = slash_commands()
                        .into_iter()
                        .map(|(c, l)| (c, l.to_string()))
                        .collect();
                    for (id, name) in app.store.template_pages().unwrap_or_default() {
                        all.push((SlashCmd::Template(id), format!("Template: {name}")));
                    }
                    for (i, p) in app.plugins.iter().enumerate() {
                        for c in &p.manifest.commands {
                            all.push((
                                SlashCmd::Plugin(i, c.id.clone()),
                                format!("{}: {}", p.manifest.name, c.title),
                            ));
                        }
                    }
                    all.into_iter()
                        .filter(|(_, l)| q.is_empty() || l.to_lowercase().contains(&q))
                        .map(|(cmd, label)| AcItem::Command { cmd, label })
                        .collect()
                }
                AcKind::Block => app
                    .store
                    .search_blocks(&query, 8)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|h| Some(&h.block) != own.as_ref())
                    .map(|h| AcItem::Block {
                        uid: h.block.0,
                        text: utopia_core::inline::plain_text(&h.text),
                        page_title: h.page_title,
                    })
                    .collect(),
                _ => {
                    let hits = app.store.search_titles(&query, 8).unwrap_or_default();
                    let key = normalize_title(&query);
                    let exact = hits.iter().any(|h| {
                        normalize_title(&h.title) == key
                            || h.alias.as_deref().map(normalize_title) == Some(key.clone())
                    });
                    let mut items: Vec<AcItem> = hits
                        .into_iter()
                        .map(|h| AcItem::Page {
                            title: h.title,
                            alias: h.alias,
                        })
                        .collect();
                    // `[[tomorrow` offers tomorrow's page first (after an exact title
                    // match, so a page really called "Today" still wins).
                    if let Some(d) = utopia_core::date::relative_date(&query, app.store.today()) {
                        let title = app.store.date_format().format(d);
                        items
                            .retain(|i| !matches!(i, AcItem::Page { title: t, .. } if *t == title));
                        let at = usize::from(exact);
                        items.insert(at.min(items.len()), AcItem::Page { title, alias: None });
                    }
                    // A bare `#tag` is already a complete link (created on commit), so it
                    // offers no "Create" row; Enter then splits the block as usual.
                    if !key.is_empty() && !exact && kind != AcKind::Tag {
                        items.push(AcItem::Create(query.trim().to_string()));
                    }
                    items
                }
            }
        });
        let selected = self
            .ac
            .menu()
            .filter(|m| m.start == start)
            .map_or(0, |m| m.selected.min(items.len().saturating_sub(1)));
        self.ac = Autocomplete::Open(AutocompleteMenu {
            kind,
            start,
            query,
            items,
            selected,
            caret: cursor,
        });
        cx.notify();
    }

    /// Accept the selected suggestion; `create` forces "create page from query"
    /// (Cmd+Enter, FR-2.17).
    fn accept(
        &mut self,
        create: bool,
        ix: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.field.composing() {
            // §13.5: the text is the input method's until it commits; taking a suggestion
            // now would replace a range of its provisional glyphs.
            return;
        }
        let Autocomplete::Open(menu) = std::mem::replace(&mut self.ac, Autocomplete::Closed) else {
            return;
        };
        let item = if create {
            if menu.kind == AcKind::Block || menu.query.trim().is_empty() {
                self.ac = Autocomplete::Open(menu);
                return;
            }
            AcItem::Create(menu.query.trim().to_string())
        } else {
            match menu.items.get(ix.unwrap_or(menu.selected)) {
                Some(i) => i.clone(),
                None => return,
            }
        };
        if let AcItem::Create(title) = &item {
            self.app.update(cx, |app, cx| {
                let r = app.store.create_page(title);
                app.report(r);
                app.changed(Vec::new(), true, cx);
            });
        }
        // The range the menu was opened for, not the caret now: selecting or clicking
        // after the menu opened leaves the query the items describe, so replacing
        // `start..cursor` would leave the tail of it behind (FR-2.17).
        let end = menu.caret;
        if self.field.text.get(menu.start..end).is_none() {
            // Belt and braces: a buffer replaced under the menu could leave the frozen
            // range off a character boundary. `sync_from_store` closes the menu in that
            // case, so this only guards the offsets themselves (rule 11).
            return;
        }
        if let AcItem::Command { cmd, .. } = item {
            self.field.replace(menu.start..end, "");
            self.ac = Autocomplete::Closed;
            self.after_edit(window, cx);
            return self.run_slash(cmd, window, cx);
        }
        let (insert, skip) = completion_text(menu.kind, &item, &self.field.text[end..]);
        self.field.replace(menu.start..end + skip, &insert);
        // An accepted `#tag` still looks like a trigger; don't reopen on it.
        self.dismiss_trigger();
        self.after_edit(window, cx);
        cx.notify();
    }

    /// Keep the trigger under the caret shut: the editor just produced the text a menu
    /// would offer to replace (a wrapped selection, an accepted completion, FR-2.17).
    fn dismiss_trigger(&mut self) {
        let cursor = self.field.cursor();
        match detect_trigger(&self.field.text, cursor) {
            Some((_, trigger_start, _)) => self.ac.suppress(trigger_start),
            None => self.ac = Autocomplete::Closed,
        }
    }

    fn run_slash(&mut self, cmd: SlashCmd, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.target().cloned() else {
            return;
        };
        match cmd {
            SlashCmd::Kind(kind) => self.set_kind(&t, kind, false, cx),
            SlashCmd::Today => {
                let link = self.app.read_with(cx, |app, _| {
                    let s = &app.store;
                    page_link_text(&s.date_format().format(s.today()), false)
                });
                let sel = self.field.sel.clone();
                self.field.replace(sel, &link);
                self.after_edit(window, cx);
            }
            SlashCmd::Embed => {
                let c = self.field.cursor();
                self.field.replace(c..c, "{{embed: (())}}");
                self.field.move_to(c + "{{embed: ((".len());
                self.after_edit(window, cx);
            }
            SlashCmd::Query => {
                let c = self.field.cursor();
                self.field.replace(c..c, "{{query: {and: }}}");
                self.field.move_to(c + "{{query: {and: ".len());
                self.after_edit(window, cx);
            }
            SlashCmd::Attach => {
                let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Attach".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picked.await {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.attach_paths(&paths, window, cx)
                        });
                    }
                })
                .detach();
            }
            SlashCmd::TagPhrase => {
                let c = self.field.cursor();
                let tag = utopia_core::inline::extract_refs(&self.field.text)
                    .into_iter()
                    .rfind(|r| {
                        r.kind == utopia_core::inline::RefKind::Tag
                            && r.range.end <= c
                            && !self.field.text[r.range.clone()].starts_with("#[[")
                    });
                if let Some(r) = tag {
                    if let Some((range, rep)) =
                        utopia_core::inline::whole_phrase_tag(&self.field.text, r.range.start)
                    {
                        let end = range.start + rep.len();
                        self.field.replace(range, &rep);
                        self.field.move_to(end);
                        self.after_edit(window, cx);
                    }
                }
            }
            SlashCmd::Extract => self.emit_after_commit(EditorEvent::ExtractRequested, cx),
            SlashCmd::Template(id) => self.emit_after_commit(EditorEvent::InsertTemplate(id), cx),
            SlashCmd::Plugin(i, command) => self.run_plugin(i, &command, window, cx),
        }
        cx.notify();
    }

    fn set_kind(&mut self, t: &EditTarget, kind: BlockKind, checked: bool, cx: &mut Context<Self>) {
        self.app.update(cx, |app, cx| {
            let r = app.store.set_block_kind(&t.page, &t.block, kind, checked);
            app.report(r);
            app.changed(vec![t.page.clone()], true, cx);
        });
    }

    /// Alt+Enter: bullet → to-do → done → bullet.
    fn toggle_todo(&mut self, _: &ToggleTodo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.target().cloned() else {
            return;
        };
        let current = self.app.update(cx, |app, _| {
            app.store
                .page_state(&t.page)
                .ok()
                .and_then(|st| st.block(&t.block).map(|b| (b.kind, b.checked)))
        });
        let next = match current {
            Some((BlockKind::Todo, false)) => (BlockKind::Todo, true),
            Some((BlockKind::Todo, true)) => (BlockKind::Bullet, false),
            _ => (BlockKind::Todo, false),
        };
        self.set_kind(&t, next.0, next.1, cx);
    }

    /// A structural host gesture (split, delete, move, zoom, paste outline): the host is
    /// about to change the rows under the editor, so the input method's marked text is
    /// taken as committed first (§13.5). Deferring the commit instead would let the host
    /// remove the row being composed and then queue a `begin` on its neighbour, which the
    /// focus change from that removal discards — the caret would not continue next door.
    fn emit_after_commit(&mut self, e: EditorEvent, cx: &mut Context<Self>) {
        if self.field.composing() {
            self.accept_composition(cx);
        }
        self.commit(cx);
        cx.emit(e);
    }

    // ---------------------------------------------------------------- actions

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.composing() {
            // §13.5: Enter confirms a candidate in the input method; it never splits here.
            return;
        }
        if self.ac.captures_keys() {
            return self.accept(false, None, window, cx);
        }
        // Split where the caret is once the block is finished: committing can rewrite the
        // text before it (a relative date link the caret was in, §10.3.1).
        self.commit(cx);
        let caret = self.field.cursor();
        cx.emit(EditorEvent::Split { caret });
    }

    /// Tab in an open menu (FR-2.17): the highlighted suggestion, which is the first one
    /// unless the user moved the highlight, exactly as Enter takes it. Bound only while
    /// the menu has suggestions; otherwise the key falls through to its usual action.
    fn accept_completion(
        &mut self,
        _: &AcceptCompletion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.ac.captures_keys() {
            self.accept(false, None, window, cx);
        } else {
            cx.propagate();
        }
    }

    fn insert_above(&mut self, _: &InsertAbove, window: &mut Window, cx: &mut Context<Self>) {
        if self.ac.captures_keys() {
            return self.accept(true, None, window, cx);
        }
        self.emit_after_commit(EditorEvent::InsertAbove, cx);
    }

    fn soft_break(&mut self, _: &SoftBreak, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.composing() {
            return;
        }
        let sel = self.field.sel.clone();
        self.field.replace(sel, "\n");
        self.after_edit(window, cx);
        cx.notify();
    }

    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        self.ac = Autocomplete::Closed;
        cx.emit(EditorEvent::Indent);
    }

    fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.ac = Autocomplete::Closed;
        cx.emit(EditorEvent::Outdent);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        // §13.5: Backspace belongs to the input method while it composes (it backs through
        // the candidate); deleting here would remove glyphs it still addresses by offset.
        if self.field.composing() {
            return;
        }
        if self.field.sel.is_empty() {
            if self.field.cursor() == 0 {
                self.ac = Autocomplete::Closed;
                return self.emit_after_commit(EditorEvent::MergeBackward, cx);
            }
            // `(|)` → `|`: the closer was put there by the opener, so it goes with it.
            let pair = utopia_core::text::empty_pair_at(&self.field.text, self.field.cursor());
            match pair {
                Some(r) => {
                    self.field.move_to(r.start);
                    self.field.select_to(r.end);
                }
                None => {
                    let to = self.field.prev_boundary();
                    self.field.select_to(to);
                }
            }
        }
        let sel = self.field.sel.clone();
        self.field.replace(sel, "");
        self.after_edit(window, cx);
        cx.notify();
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        // §13.5: see `backspace`.
        if self.field.composing() {
            return;
        }
        if self.field.sel.is_empty() {
            let to = self.field.next_boundary();
            self.field.select_to(to);
        }
        let sel = self.field.sel.clone();
        self.field.replace(sel, "");
        self.after_edit(window, cx);
        cx.notify();
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.anchor_caret(cx);
        self.refresh_autocomplete(cx);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.prev_boundary()
        } else {
            self.field.sel.start
        };
        self.field.move_to(to);
        self.moved(cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.field.sel.is_empty() {
            self.field.next_boundary()
        } else {
            self.field.sel.end
        };
        self.field.move_to(to);
        self.moved(cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.prev_boundary();
        self.field.select_to(to);
        cx.notify();
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.next_boundary();
        self.field.select_to(to);
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.field.move_to(0);
        let len = self.field.text.len();
        self.field.select_to(len);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.field.move_to(0);
        self.moved(cx);
    }

    fn end_(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        self.field.move_to(len);
        self.moved(cx);
    }

    fn line_start(&mut self, _: &LineStart, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.line_start();
        self.field.move_to(to);
        self.moved(cx);
    }

    fn line_end(&mut self, _: &LineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.line_end();
        self.field.move_to(to);
        self.moved(cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let to = utopia_core::text::prev_word_start(&self.field.text, self.field.cursor());
        self.field.move_to(to);
        self.moved(cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let to = utopia_core::text::next_word_end(&self.field.text, self.field.cursor());
        self.field.move_to(to);
        self.moved(cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, cx);
    }

    fn vertical(&mut self, up: bool, cx: &mut Context<Self>) {
        if self.ac.captures_keys() {
            if let Autocomplete::Open(menu) = &mut self.ac {
                let n = menu.items.len();
                menu.selected = if up {
                    (menu.selected + n - 1) % n
                } else {
                    (menu.selected + 1) % n
                };
                cx.notify();
                return;
            }
        }
        match self.field.vertical(up, None) {
            Some(ix) => {
                self.field.move_to(ix);
                self.moved(cx);
            }
            None => {
                let x = self.field.caret_x();
                self.emit_after_commit(EditorEvent::Navigate { up, x }, cx);
            }
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.field.vertical(true, None).unwrap_or(0);
        self.field.select_to(to);
        cx.notify();
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.field.text.len();
        let to = self.field.vertical(false, None).unwrap_or(len);
        self.field.select_to(to);
        cx.notify();
    }

    fn move_up(&mut self, _: &MoveBlockUp, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::MoveBlock { up: true }, cx);
    }

    fn move_down(&mut self, _: &MoveBlockDown, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::MoveBlock { up: false }, cx);
    }

    fn toggle_collapse(&mut self, _: &ToggleCollapse, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::ToggleCollapse);
    }

    fn toggle_collapse_all(
        &mut self,
        _: &ToggleCollapseAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(EditorEvent::ToggleCollapseAll);
    }

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::ZoomIn, cx);
    }

    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::ZoomOut, cx);
    }

    fn mark(&mut self, delim: &str, window: &mut Window, cx: &mut Context<Self>) {
        // §13.5: reflowing the text around the selection would leave the marked range
        // pointing at bytes it never covered, and the input method's next update would
        // rewrite the wrong run. Styling waits until it commits.
        if self.field.composing() {
            return;
        }
        let (text, sel) =
            utopia_core::text::toggle_mark(&self.field.text, self.field.sel.clone(), delim);
        self.field.text = text;
        self.field.sel = sel;
        self.field.reversed = false;
        self.after_edit(window, cx);
        cx.notify();
    }

    fn bold(&mut self, _: &Bold, window: &mut Window, cx: &mut Context<Self>) {
        self.mark("**", window, cx);
    }

    fn italic(&mut self, _: &Italic, window: &mut Window, cx: &mut Context<Self>) {
        self.mark("_", window, cx);
    }

    fn code(&mut self, _: &Code, window: &mut Window, cx: &mut Context<Self>) {
        self.mark("`", window, cx);
    }

    /// Copy: the selection, or with nothing selected the block's subtree as nested
    /// Markdown (FR-2.11).
    fn copy_text(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.field.sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.field.selected_text().to_string(),
            ));
            return true;
        }
        let Some(t) = self.target().cloned() else {
            return false;
        };
        let md = self.app.update(cx, |app, _| {
            app.store
                .page_state(&t.page)
                .ok()
                .map(|st| utopia_core::markdown::subtree_to_markdown(st, &t.block))
        });
        if let Some(md) = md {
            cx.write_to_clipboard(ClipboardItem::new_string(md));
        }
        false
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_text(cx);
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        // §13.5: cutting the composition would take the input method's own text; copy
        // still works, the delete waits for it to commit.
        if self.field.composing() {
            return;
        }
        if self.copy_text(cx) {
            let sel = self.field.sel.clone();
            self.field.replace(sel, "");
            self.after_edit(window, cx);
            cx.notify();
        } else {
            self.emit_after_commit(EditorEvent::DeleteBlock, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        // §13.5: the caret and selection belong to the input method while it composes.
        if self.field.composing() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        let text = text.replace("\r\n", "\n");
        if text.trim_end_matches('\n').contains('\n') {
            return self.emit_after_commit(EditorEvent::PasteOutline(text), cx);
        }
        let sel = self.field.sel.clone();
        self.field.replace(sel, text.trim_end_matches('\n'));
        self.after_edit(window, cx);
        cx.notify();
    }

    fn copy_block_ref(&mut self, _: &CopyBlockRef, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.target() {
            cx.write_to_clipboard(ClipboardItem::new_string(format!("(({}))", t.block)));
        }
    }

    fn delete_block(&mut self, _: &DeleteBlock, _: &mut Window, cx: &mut Context<Self>) {
        self.emit_after_commit(EditorEvent::DeleteBlock, cx);
    }

    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        // Only a menu with suggestions is dismissed. One that found nothing to offer
        // (`/xyz`, `#zzz`) never drew, so Escape must leave the block the first time
        // instead of silently closing something invisible (FR-2.17).
        if self.ac.captures_keys() {
            self.dismiss_trigger();
            cx.notify();
            return;
        }
        self.end(cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.history(true, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.history(false, cx);
    }

    fn history(&mut self, undo: bool, cx: &mut Context<Self>) {
        self.commit(cx);
        let caret = self.app.update(cx, |app, cx| {
            let r = if undo {
                app.store.undo()
            } else {
                app.store.redo()
            };
            let caret = app.report(r).flatten();
            app.changed(Vec::new(), true, cx);
            caret
        });
        self.sync_from_store(cx);
        if let Some(c) = caret {
            cx.emit(EditorEvent::Restore(c));
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.field.index_at(e.position);
        if e.click_count >= 2 {
            let r = utopia_core::text::word_range_at(&self.field.text, ix);
            self.field.move_to(r.start);
            self.field.select_to(r.end);
        } else if e.modifiers.shift {
            self.field.select_to(ix);
        } else {
            self.field.move_to(ix);
        }
        self.field.selecting = true;
        self.anchor_caret(cx);
        self.refresh_autocomplete(cx);
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.field.selecting {
            let ix = self.field.index_at(e.position);
            self.field.select_to(ix);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.field.selecting = false;
    }

    fn render_autocomplete(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        // Nothing to show means nothing to draw: an open menu with no items is invisible,
        // which is why it must not capture a key either (`captures_keys`).
        let ac = self.ac.menu()?;
        if !self.ac.captures_keys() {
            return None;
        }
        let th = theme(cx).clone();
        let rows = ac.items.iter().enumerate().map(|(i, item)| {
            let (label, detail): (SharedString, Option<SharedString>) = match item {
                AcItem::Page { title, alias } => (
                    title.clone().into(),
                    alias.as_ref().map(|a| format!("alias: {a}").into()),
                ),
                AcItem::Create(t) => (format!("Create \"{t}\"   ⌘↵").into(), None),
                AcItem::Command { label, .. } => (format!("/ {label}").into(), None),
                AcItem::Block {
                    text, page_title, ..
                } => (
                    text.chars().take(60).collect::<String>().into(),
                    Some(page_title.clone().into()),
                ),
            };
            div()
                .id(("ac", i))
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .when(i == ac.selected, |d| d.bg(th.hover))
                .child(div().text_color(th.text).child(label))
                .when_some(detail, |d, s| {
                    d.child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(s),
                    )
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        this.accept(false, Some(i), window, cx);
                    }),
                )
        });
        Some(
            deferred(
                anchored().child(
                    div()
                        .occlude()
                        .debug_selector(|| "autocomplete".into())
                        .mt_1()
                        .w(px(340.))
                        .p_1()
                        .bg(th.surface)
                        .border_1()
                        .border_color(th.line)
                        .rounded_md()
                        .shadow_md()
                        .children(rows),
                ),
            )
            .with_priority(1),
        )
    }
}

impl Render for BlockEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let text = self.field.text.clone();
        let (styled, shown) = if text.is_empty() {
            let hint = "Type, or [[ to link, # to tag, (( to reference";
            (
                StyledText::new(hint).with_highlights([(
                    0..hint.len(),
                    HighlightStyle {
                        color: Some(th.faint),
                        ..Default::default()
                    },
                )]),
                hint.to_string(),
            )
        } else {
            let mut hl = source_highlights(&text, &th);
            if let Some(m) = self.field.marked() {
                hl.retain(|(r, _)| r.end <= m.start || r.start >= m.end);
                hl.push((
                    m.clone(),
                    HighlightStyle {
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(th.text),
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                ));
                hl.sort_by_key(|(r, _)| r.start);
            }
            (StyledText::new(text.clone()).with_highlights(hl), text)
        };
        let mut key_context = gpui::KeyContext::default();
        key_context.add(EDITOR_CONTEXT);
        if self.ac.captures_keys() {
            key_context.add(AUTOCOMPLETE_CONTEXT);
        }
        div()
            .key_context(key_context)
            .debug_selector(|| "block-editor".into())
            .track_focus(&self.focus)
            .w_full()
            .min_h(px(crate::appearance::density(cx).line_height()))
            .cursor_text()
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::insert_above))
            .on_action(cx.listener(Self::accept_completion))
            .on_action(cx.listener(Self::soft_break))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end_))
            .on_action(cx.listener(Self::line_start))
            .on_action(cx.listener(Self::line_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::toggle_collapse))
            .on_action(cx.listener(Self::toggle_collapse_all))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::bold))
            .on_action(cx.listener(Self::italic))
            .on_action(cx.listener(Self::code))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy_block_ref))
            .on_action(cx.listener(Self::delete_block))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::toggle_todo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .child(FieldElement::new(cx.entity(), styled, shown))
            .children(self.render_autocomplete(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers() {
        assert_eq!(
            detect_trigger("see [[Alp", 9),
            Some((AcKind::Page, 4, "Alp".into()))
        );
        assert_eq!(
            detect_trigger("#[[Modern R", 11),
            Some((AcKind::BracketTag, 0, "Modern R".into()))
        );
        assert_eq!(
            detect_trigger("x #min", 6),
            Some((AcKind::Tag, 2, "min".into()))
        );
        assert_eq!(
            detect_trigger("x #", 3),
            Some((AcKind::Tag, 2, String::new()))
        );
        assert_eq!(
            detect_trigger("((thes", 6),
            Some((AcKind::Block, 0, "thes".into()))
        );
        assert_eq!(detect_trigger("[[A]] done", 10), None);
        assert_eq!(detect_trigger("C#", 2), None);
        assert_eq!(detect_trigger("#a b", 4), None);
        assert_eq!(
            detect_trigger("/hea", 4),
            Some((AcKind::Slash, 0, "hea".into()))
        );
        assert_eq!(
            detect_trigger("x /", 3),
            Some((AcKind::Slash, 2, String::new()))
        );
        assert_eq!(detect_trigger("http://a", 8), None, "urls are not commands");
        assert_eq!(detect_trigger("a/b", 3), None);
    }

    #[test]
    fn completions_are_canonical() {
        let page = |t: &str| AcItem::Page {
            title: t.into(),
            alias: None,
        };
        assert_eq!(
            completion_text(AcKind::Page, &page("Alpha"), "]] x"),
            ("[[Alpha]]".into(), 2)
        );
        assert_eq!(
            completion_text(AcKind::Tag, &page("Modern Religion"), ""),
            ("#[[Modern Religion]]".into(), 0)
        );
        assert_eq!(
            completion_text(AcKind::Tag, &page("mind"), ""),
            ("#mind".into(), 0)
        );
        assert_eq!(
            completion_text(AcKind::BracketTag, &page("A B"), ""),
            ("#[[A B]]".into(), 0)
        );
        let blk = AcItem::Block {
            uid: "U1".into(),
            text: String::new(),
            page_title: String::new(),
        };
        assert_eq!(
            completion_text(AcKind::Block, &blk, "))"),
            ("((U1))".into(), 2)
        );
    }
}
