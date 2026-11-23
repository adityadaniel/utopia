# Vim mode — implementation plan

**Source of truth:** `docs/vim-mode-proposal.md`. This file turns that proposal into
numbered tasks. Each task names the files, the symbols and the check that proves it.

**Branch:** `feat/vim-mode`. **Base:** `main` at `c2edb59`.

**Scope of this plan:** PR 0 (docs), PR 1 (modes, motions, insert) and PR 2 (operators,
registers, block commands). PR 3 to PR 5 stay in the proposal.

**Owner decisions (proposal §8) are taken as the recommendation of each row.** The two
that change existing behaviour are written down here:
- Decision 6: one Insert run is one undo step. This changes FR-2.12 for Vim mode only.
- Decision 5: Escape in Normal ends the edit. Any Normal key then wakes the cursor.

---

## Rules you must not break

These come from `AGENTS.md`, "Architecture rules". Each task repeats the ones it touches.

1. Every content change is a `utopia_core::Command` through `Store::execute*`. Add no
   second write path.
2. SQLite rows are projections. Mutate through the store, never with raw SQL.
5. Never hard-delete a block. `dd` sets `deleted_at` (a tombstone) and keeps the id.
9. Mode, registers, pending keys and the resume point are local state. Put none of them
   in a CRDT shard, in the outbox or in a sync message.
10. Do no blocking I/O in `render`. The parser must not read the store.
11. Do not panic on user data. Clamp every offset. Use no `unwrap()` on a range.

**Build environment (AGENTS.md):**
```bash
source ~/.cargo/env
export DEVELOPER_DIR="$(xcode-select -p)"
```
`.cargo/config.toml` in this worktree already points `target-dir` at the main checkout,
because the disk is nearly full. Do not change or delete that file. Do not commit it.

**Check after every task:**
```bash
cargo check --workspace            # fast loop
cargo test -p <the crate you changed>
```
**Check before you say a PR is done:**
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

---

# PR 0 — Specification (documents only)

No crate includes these files, so `make check` is not needed for PR 0 alone.

### Task 0.1 — PRD §9.1.1 "Vim mode (VM)"

File: `PRD.md`. Put the new section after §9.1 (Settings and keybindings).

Write requirement ids `VM-1` to `VM-14` and acceptance ids `AC-VM-*`. Each id must be
one testable sentence. Use this list:

| Id | Requirement |
|---|---|
| VM-1 | Vim mode is off by default. `settings.json` holds `"vim": false`. An unknown value means off, and the app logs it. |
| VM-2 | With Vim off, behaviour and keystroke cost do not change. |
| VM-3 | Vim mode has four states: Rest, Normal, Insert and Visual. Pending is a sub-state of Normal and Visual. |
| VM-4 | A block in edit is in Normal mode at entry: at launch, after a click and after navigation. |
| VM-5 | In Normal mode no key puts a character in the block. An unknown key does nothing. |
| VM-6 | Charwise motions and operators act only inside the block that owns the caret. |
| VM-7 | `j` and `k` move by wrapped line, then cross to the next or previous block. |
| VM-8 | Linewise commands (`dd`, `yy`, `cc`, `>>`, `<<`, `J`, `o`, `O`) act on blocks. |
| VM-9 | `dd` tombstones the block and its subtree, and keeps every id. |
| VM-10 | `dd` then `p` moves the same blocks back with the same ids, in one undo step, so each `((reference))` to them still resolves. |
| VM-11 | `y` writes the register and the system clipboard. `d`, `c` and `x` write the register only. |
| VM-12 | One command is one undo step. One Insert run is one undo step. |
| VM-13 | Escape is a ladder: it cancels a composition, then leaves Insert, then ends the edit. A Normal key then wakes the cursor where it was. |
| VM-14 | In Insert mode, input-method behaviour is the same as with Vim off (§13.5). In Normal mode the app takes no marked text. |

Add an FR-2.12 note: "In Vim mode, one Insert run (from entering Insert to Escape, a
blur, or the end of the session) is one undo group. The 500 ms coalesce window does not
apply inside a run."

### Task 0.2 — DESIGN.md

File: `DESIGN.md`.
- §3.1 state table: add a row "Editing, Vim Normal". The cursor is a block, not a bar.
  Fill `--selection`, 1 px `--link` outline, painted under the text. It does not blink.
- §3.3 top bar: add the mode label. Text `NORMAL`, `INSERT` or `VISUAL` in the Mono label
  role, colour `--ink-3`, no fill. Pending keys follow it. It shows only with Vim on.
- §4.9 (or the Settings section): add the switch in Settings → Keybindings, above the
  list. It is a segmented control, "Standard | Vim".

### Task 0.3 — State machine document

New file: `docs/state-machines/vim-mode.md`. Copy the mermaid diagram from proposal §2
and write one paragraph for each state and each edge.

### Task 0.4 — AGENTS.md

File: `AGENTS.md`. Add `"vim": false` to the `settings.json` line in "Commands".

**PR 0 acceptance:** the four files are written. No code changed.

---

# PR 1 — Modes, motions and insert

## 1A — The pure grammar (`utopia-core`)

New directory `crates/utopia-core/src/vim/` with `mod.rs`, `parse.rs`, `motion.rs`.
Add `pub mod vim;` to `crates/utopia-core/src/lib.rs`.

**This code must not depend on `gpui`, `loro` or `rusqlite`.** It takes `&str` and
offsets and returns offsets and commands. It is the only place the grammar lives.

### Task 1A.1 — Types (`vim/mod.rs`)

```rust
/// One keystroke, as the UI layer classifies it before the keymap sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable character the platform reports, e.g. 'd', '$', 'G'.
    Char(char),
    Escape,
    Enter,
    Space,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    /// Control plus a letter, lower-cased, e.g. Ctrl('r').
    Ctrl(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left, Right,
    /// Down or up one wrapped line, then into the next or previous block.
    Down, Up,
    WordForward { big: bool },      // w W
    WordBack { big: bool },         // b B
    WordEnd { big: bool },          // e E
    LineStart,                      // 0
    LineFirstNonBlank,              // ^
    LineEnd,                        // $
    Find { ch: char, till: bool, back: bool },   // f t F T
    RepeatFind { reverse: bool },   // ; ,
    PageFirst,                      // gg
    PageLast,                       // G
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator { Delete, Change, Yank, Indent, Outdent }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextObject {
    Word { big: bool, around: bool },     // iw aw iW aW
    Bracket { open: char, around: bool }, // i( a( i[ a[ i{ a{
    Quote { ch: char, around: bool },     // i" a" i' a' i` a`
    Ref { around: bool },                 // il al  (PR 4)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target { Motion(Motion), Object(TextObject), Line }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Move { motion: Motion, count: usize },
    Operate { op: Operator, target: Target, count: usize, register: Option<char> },
    /// i a I A o O s S C
    Insert(InsertAt),
    DeleteChar { back: bool, count: usize },   // x X
    DeleteToLineEnd,                           // D
    ReplaceChar(char),                         // r{c}
    ToggleCase { count: usize },               // ~
    Paste { before: bool, count: usize, register: Option<char> },
    JoinNext { count: usize },                 // J
    Undo { count: usize },
    Redo { count: usize },
    EnterVisual { line: bool },                // v V   (PR 3)
    RepeatChange,                              // .     (PR 3)
    LeaveNormal,                               // Escape with nothing pending
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    Cursor,          // i
    After,           // a
    LineStart,       // I
    LineEnd,         // A
    BlockBelow,      // o
    BlockAbove,      // O
    ReplaceChar,     // s
    ReplaceLine,     // S  and cc
    ToLineEnd,       // C
}

/// What the parser says after one key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// More keys are needed. The string is what the mode label shows, e.g. "2d".
    Pending(String),
    Run(Command),
    /// A key the grammar knows nothing about. The UI swallows it and shows nothing.
    Cancel,
    /// The UI must give this keystroke to the keymap (Tab, a Cmd chord).
    Pass,
}
```

### Task 1A.2 — The parser (`vim/parse.rs`)

```rust
#[derive(Default, Debug)]
pub struct Parser { /* count, register, operator, prefix, awaiting-char */ }

impl Parser {
    pub fn feed(&mut self, key: Key) -> Step;
    /// Drop every pending key. Escape and a completed command call this.
    pub fn reset(&mut self);
    /// What the mode label shows after the mode name; "" when nothing is pending.
    pub fn pending(&self) -> String;
    /// The last `f`/`t`/`F`/`T` target, for `;` and `,`.
    pub fn last_find(&self) -> Option<(char, bool, bool)>;
}
```

Grammar, in the order the parser tries it:

1. `Key::Escape` → `reset()`, then `Step::Run(Command::LeaveNormal)` when nothing was
   pending, else `Step::Pending(String::new())` (the cancel is silent).
2. `"` then one character → the register. Only `_` and `+` are accepted in PR 2.
   Any other character is `Step::Cancel`.
3. Digits `1`–`9`, then any digit, build the count. A leading `0` is the `LineStart`
   motion, never a count.
4. A character the parser is waiting for (`f`, `t`, `F`, `T`, `r`) takes the next key as
   a `Key::Char`. Any other key kind is `Step::Cancel`.
5. `g` is a prefix. `gg` is `Motion::PageFirst`. `gd` is PR 4. Any other second key is
   `Step::Cancel`.
6. An operator (`d`, `c`, `y`, `>`, `<`) waits for a target. A second press of the same
   operator letter is `Target::Line` (`dd`, `yy`, `cc`, `>>`, `<<`). `i` or `a` after an
   operator starts a text object. Otherwise the target is a motion.
7. With no operator pending, a motion key is `Command::Move`.
8. Everything else in the table of proposal §3 maps to its `Command`.
9. Any key that is not in the grammar is `Step::Cancel`.

The count rule: `2d3w` means the operator runs over 6 words. Multiply the two counts.

### Task 1A.3 — Motions on text (`vim/motion.rs`)

Every function takes `(text: &str, offset: usize)` and returns a byte offset inside
`text`. Every offset you return must be a grapheme boundary. Use
`unicode_segmentation::UnicodeSegmentation`, which the crate already depends on.

```rust
/// Vim's three character classes: word characters, punctuation, and blank.
/// `big` folds punctuation into word, which is `W`, `B`, `E`.
pub fn word_forward(text: &str, offset: usize, big: bool) -> usize;
pub fn word_back(text: &str, offset: usize, big: bool) -> usize;
pub fn word_end(text: &str, offset: usize, big: bool) -> usize;
pub fn line_start(text: &str, offset: usize) -> usize;
pub fn line_first_non_blank(text: &str, offset: usize) -> usize;
/// The offset of the last grapheme of the line, not one past it (Normal clamp).
pub fn line_end_inclusive(text: &str, offset: usize) -> usize;
/// One past the last grapheme, for `$` used as an operator target.
pub fn line_end_exclusive(text: &str, offset: usize) -> usize;
pub fn find_char(text: &str, offset: usize, ch: char, till: bool, back: bool) -> Option<usize>;
/// Keeps the cursor on a grapheme and never past the last one of its line.
pub fn clamp_normal(text: &str, offset: usize) -> usize;
```

**Do not write `text.chars()` arithmetic.** `prev_grapheme` and `next_grapheme` in
`crates/utopia-core/src/text.rs` are the existing helpers. `text.rs` already has
`prev_word_start` and `next_word_end`, but they are the Emacs rules: they skip
punctuation. Vim treats punctuation as its own word. Write new functions; do not change
the old ones, because the Emacs key bindings use them.

Every motion stops at the block's edges. `word_forward` at the end of the text returns
the clamped last offset, not a value past the end.

### Task 1A.4 — Text objects (`vim/textobj.rs`)

```rust
/// The range an operator acts on, or `None` when the cursor is not in such an object.
pub fn range_of(text: &str, offset: usize, obj: TextObject) -> Option<Range<usize>>;
```
- `Word { around: false }` is the word under the cursor. `around: true` adds the blanks
  after it, or the blanks before it when there are none after.
- `Bracket` searches left for the open character and right for its pair, and counts
  nesting. `around: true` includes both brackets.
- `Quote` finds the pair on the cursor's line. `around: true` includes both quotes.
- `Ref` is PR 4. Return `None` for now.

### Task 1A.5 — The reference table

```rust
/// Every key the grammar knows, with the text Settings → Shortcuts shows.
pub const REFERENCE: &[(&str, &str)] = &[ ("h", "One character left"), /* … */ ];
```
A unit test must feed each row's keys to a fresh `Parser` and assert the last step is
`Step::Run(..)` or `Step::Pending(..)`, never `Step::Cancel`. That test keeps the help
card and the parser in step.

### Task 1A.6 — Unit tests (`#[cfg(test)] mod tests` next to the code)

Table-driven. Cover at least:
- Key strings to commands: `dw`, `d2w`, `2dw`, `2d3w`, `dd`, `3dd`, `ci[`, `caw`, `yy`,
  `f,`, `3j`, `gg`, `G`, `r x`, `"_dd`.
- Every unfinished prefix is `Pending`: `d`, `2`, `2d`, `g`, `f`, `"`, `ci`.
- Every unknown continuation is `Cancel`: `dq`, `gq`, `"q`, `zz`.
- `0` after a digit is a count; `0` alone is `LineStart`.
- Motions on hard text: an emoji ZWJ sequence (`"👨‍👩‍👦 cat"`), a combining mark
  (`"e\u{301}cole"`), CJK (`"日本語 text"`), an empty string, a string that is only
  blanks, and text with `\n` in it.
- `clamp_normal` on an empty string returns 0.
- `clamp_normal` never returns an offset that is not a grapheme boundary.

**Task 1A acceptance:** `cargo test -p utopia-core` passes. `cargo clippy -p utopia-core
--all-targets -- -D warnings` passes. `utopia-core/Cargo.toml` gained no dependency.

## 1B — The setting

### Task 1B.1 — `Prefs.vim`

File: `crates/utopia-ui/src/appearance.rs`. Copy the `smooth_caret` field exactly. It is
a boolean pref added in the commit before this branch, so every line you need is there:

- the module doc block at the top (`//!   "smooth_caret": false,`) — add `"vim": false,`;
- `pub smooth_caret: bool,` in `Prefs` → add `pub vim: bool,` with a doc comment that
  cites VM-1;
- `Default` → `vim: false,`;
- `from_json` → copy the `rest.remove("smooth_caret")` arm. The log line must read
  `"{FILE}: vim should be a boolean, not {v}; using false"`;
- the struct literal near the end of `from_json` → `vim,`;
- `to_json` → `m.insert("vim".into(), self.vim.into());`;
- `pub fn vim(cx: &App) -> bool` next to `pub fn smooth_caret`;
- `pub fn set_vim(cx: &mut App, on: bool, config_dir: &Path) -> std::io::Result<()>`
  next to `set_smooth_caret`;
- the two existing round-trip tests in that file → add `vim: true` and `"vim": true`.

### Task 1B.2 — The switch in Settings

File: `crates/utopia-ui/src/settings.rs`. Copy `toggle_smooth_caret` and its row. Put
the Vim row in the **Keybindings** tab, above the action list, not in Appearance. Use
`settings_switch`, the same helper the smooth-caret row uses. Label: "Vim mode". Help:
"Modal editing. Normal mode for motions and commands, Insert mode to type."

### Task 1B.3 — `ToggleVimMode`

File: `crates/utopia-ui/src/actions.rs`. The action goes in all four places:
`actions!`, `CATALOG`, `action_for` and the `binding_for!` list.

```rust
ActionDoc {
    action: "ToggleVimMode",
    label: "Vim Mode",
    category: Category::Application,
    keys: "",                 // no default key
    context: None,
    help: "Turn modal editing on or off (VM-1).",
},
```
Handle it in `crates/utopia-ui/src/workspace.rs` next to the other global actions. It
calls `appearance::set_vim` and shows the result in the status line.

### Task 1B.4 — Tests

File: `crates/utopia-ui/tests/settings.rs`. Copy
`smooth_caret_is_opt_in_and_the_switch_is_saved` into
`vm_1_vim_mode_is_opt_in_and_the_switch_is_saved`.

**Task 1B acceptance:** `cargo test -p utopia-ui` passes. With no `settings.json`,
`appearance::vim(cx)` is false. A `"vim": 3` in the file logs and reads as false.

## 1C — `VimState` and the key context

### Task 1C.1 — `crates/utopia-ui/src/vim.rs`

Add `pub mod vim;` to `crates/utopia-ui/src/lib.rs`.

```rust
pub enum Mode { Normal, Insert, Visual { anchor: usize, line: bool } }

pub struct VimState {
    pub mode: Mode,
    pub parser: utopia_core::vim::Parser,
    pub resume: Option<utopia_core::undo::Caret>,
    pub last_change: Option<Change>,      // PR 3; keep the field, leave it None
    pub insert_run: bool,                 // PR 2
}
```
`VimState::default()` is `Mode::Normal` and an empty parser.

Registers are a thread local in this file, on the model of
`crates/utopia-ui/src/kill.rs`:
```rust
pub enum Register {
    Chars(String),
    Blocks { markdown: String, tree: Vec<TreeRow>, cut: Option<(PageId, Vec<BlockId>)> },
}
pub struct TreeRow { pub depth: usize, pub text: String, pub kind: BlockKind, pub checked: bool }
```
PR 1 only needs the type to exist. PR 2 fills it.

### Task 1C.2 — The field on `BlockEditor`

File: `crates/utopia-ui/src/editor.rs`.
- Add `vim: crate::vim::VimState` to `BlockEditor`, next to `session` and `ac`.
- `end()` and `detach()` set `self.vim.mode = Mode::Normal` and call
  `self.vim.parser.reset()`. **The mode must survive `begin()`**, because `j` and `k`
  cross blocks through `begin`.
- `focus_lost()` keeps the mode but closes the Insert run (PR 2) and, when the mode was
  Insert, sets it to Normal.
- `mouse_down` sets `Mode::Normal` when Vim is on and the mode was Insert.

### Task 1C.3 — The key context

File: `crates/utopia-ui/src/editor.rs`, where `key_context.add(EDITOR_CONTEXT)` is.
Add one more identifier when Vim is on: `VimNormal`, `VimInsert` or `VimVisual`.
Declare the names next to `EDITOR_CONTEXT` in `actions.rs`.

### Task 1C.4 — `VimNormalMode`

File: `crates/utopia-ui/src/actions.rs`, all four places.
```rust
ActionDoc {
    action: "VimNormalMode",
    label: "Vim: Normal Mode",
    category: Category::Editing,
    keys: "escape",
    context: Some("BlockEditor && VimInsert"),
    help: "Leave Insert mode. The block commits and the cursor steps one character left.",
},
```
It must be listed **after** the plain `Escape` row in `CATALOG`, so it wins at equal
depth. `AcceptCompletion` already relies on that rule; read its comment.

Handler in `editor.rs`, on the `EmacsKillLine` model:
1. Return at once when `self.field.composing()`.
2. Call `self.commit(cx)`.
3. Close the Insert undo run (PR 2).
4. Set `Mode::Normal`.
5. Move the caret one grapheme left, then `clamp_normal`.
6. `cx.notify()` and emit `EditorEvent::VimChanged`.

**Task 1C acceptance:** the two actions appear in Settings → Keybindings. The existing
test `defaults_do_not_conflict` in `actions.rs` still passes.

## 1D — The interceptor

### Task 1D.1 — Merge into the page-menu interceptor

File: `crates/utopia-ui/src/workspace.rs`, the `cx.intercept_keystrokes` block that
closes the page menu. gpui calls every interceptor even after one stops propagation, so
two separate ones would both act on the same Escape. Put the Vim branch in that one
closure, **after** the page-menu branch.

The closure claims a keystroke only when every one of these is true:
- `window.window_handle() == handle` (this window);
- `appearance::vim(cx)`;
- the page menu is closed;
- the block editor's focus handle is focused;
- a block is in edit (`EditorSession::Editing`);
- the mode is not `Insert`;
- nothing is composing.

Then it classifies the keystroke:

| Keystroke | What to do |
|---|---|
| `escape`, `enter`, `space`, `backspace`, `delete`, `left`, `right`, `up`, `down`, with no `cmd`, `alt` or `fn` | Map by `keystroke.key` to the matching `Key`. Classify by `key` **first**: the test platform gives Enter, Tab and Space a `key_char`. |
| Any keystroke with a `key_char` and no `cmd` | `Key::Char(c)`. This makes `$`, `^` and `G` work on layouts that need Shift or Option. |
| `ctrl-<letter>` | On macOS, claim it all. Run the keys the parser knows; do nothing for the rest. Elsewhere claim only the parser's own keys, because there `cmd-` defaults become `ctrl-`. |
| `tab`, `shift-tab`, every `cmd-` chord, an `alt-` chord with no character | **Return without claiming.** The keymap must still see them. |

A claimed keystroke calls `cx.stop_propagation()` and hands the `Key` to the editor.

### Task 1D.2 — `BlockEditor::vim_key`

File: `crates/utopia-ui/src/vim.rs`, as `impl BlockEditor`.
```rust
pub fn vim_key(&mut self, key: Key, window: &mut Window, cx: &mut Context<BlockEditor>);
```
It feeds the parser and acts on the `Step`:
- `Pending(label)` → store it for the mode label, emit `VimChanged`, notify.
- `Cancel` → `parser.reset()`, emit `VimChanged`. Change no text.
- `Pass` → do nothing (the interceptor already decided not to claim).
- `Run(cmd)` → `self.vim_run(cmd, window, cx)`, then `parser.reset()`.

### Task 1D.3 — `BlockEditor::vim_run`, PR 1 subset

Handle only these commands in PR 1. Every other command is a no-op that logs at trace
level.

- `Move { motion, count }`:
  - `Left`, `Right`, the word motions, `LineStart`, `LineFirstNonBlank`, `LineEnd`,
    `Find`, `RepeatFind`: call the `utopia_core::vim::motion` function `count` times on
    `self.field.text`, then `clamp_normal`, then `self.field.move_to(offset)`.
  - `Down` and `Up`: call `self.field.vertical(up)` once for each count. When it returns
    `None`, emit `EditorEvent::Navigate { up, x: self.field.caret_window_x() }` with the
    count that is left. **Add `count: usize` to `Navigate`** and handle it in
    `page_view.rs`.
  - `PageFirst` and `PageLast`: emit the new `EditorEvent::GoEdge { first: bool }`.
- `Insert(at)`: move the caret as the variant says, set `Mode::Insert`, open the undo run
  (PR 2). `BlockBelow` emits `EditorEvent::Split { caret: text.len() }`. `BlockAbove`
  emits `EditorEvent::InsertAbove`. `ReplaceChar`, `ReplaceLine` and `ToLineEnd` are
  PR 2 (they delete first).
- `Undo` and `Redo`: call the paths the `Undo` and `Redo` actions already use.
- `LeaveNormal`: call `self.end(cx)`. Keep `self.vim.resume` first, so the wake-up knows
  the block and the offset.

### Task 1D.4 — The two input gates

File: `crates/utopia-ui/src/editor.rs` and `field.rs`.

**Committed text outside Insert.** `BlockEditor`'s existing intercept hook is
`typed_bracket`. Rename it `intercept_typed`. When Vim is on and the mode is not Insert,
it feeds each character of the text to `vim_key` as `Key::Char` and returns `true`, so
the buffer is never touched. Otherwise it does what `typed_bracket` does today.

**Marked text outside Insert.** Add to `FieldHost` in `field.rs`:
```rust
/// Whether the host takes marked text from the input method this frame. A modal
/// host answers `false` outside its insert mode (VM-14).
fn accepts_marked_text(&self) -> bool { true }
```
Check it at the top of `replace_and_mark_text_in_range`. On `false`, return without
touching `self.field.text` or `self.field.composition`. `LineInput` keeps the default.

### Task 1D.5 — Menus and the idle commit are Insert-only

File: `crates/utopia-ui/src/editor.rs`.
- `refresh_autocomplete` returns at once when Vim is on and the mode is not Insert. It
  already returns early while composing; copy that guard.
- `typing_link` returns `false` when Vim is on and the mode is not Insert. Today it holds
  the idle commit while the caret sits in a link, which would stall Normal-mode motion
  through `[[Alp`.

### Task 1D.6 — The Rest wake-up

File: `crates/utopia-ui/src/workspace.rs`. A second claim rule in the same interceptor:
Vim is on, the workspace's own focus handle is focused, the main content is a page, and
no block is in edit.

Then the key reopens the block: `PageView::restore_caret(&self.vim.resume)` when a
resume point is known, else `PageView::focus_first`. After the block is in edit, give the
same key to `vim_key`.

### Task 1D.7 — New host events

File: `crates/utopia-ui/src/editor.rs` (`EditorEvent`) and
`crates/utopia-ui/src/page_view.rs` (`on_editor_event`).
- `Navigate` gains `count: usize`. Every existing construction site passes `1`.
- `GoEdge { first: bool }`: focus the first or last visible block of the page the cursor's
  block is on. In the Daily Notes feed that is the day, not the whole feed. In a zoom it
  is the zoomed subtree. `focus_first` is the model.
- `VimChanged`: carries nothing. `Workspace` redraws the mode label on it and on nothing
  else, so a keystroke does not re-render the whole window (rule 10).

## 1E — The block cursor

### Task 1E.1 — `CaretShape`

File: `crates/utopia-ui/src/field.rs`.
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretShape { Bar, Block }
```
On `FieldHost`: `fn caret_shape(&self, _cx: &App) -> CaretShape { CaretShape::Bar }`.
`BlockEditor` returns `Block` when Vim is on and the mode is `Normal` or `Visual`.
`LineInput` keeps the default and does not change.

### Task 1E.2 — Paint

File: `crates/utopia-ui/src/field.rs`, in `prepaint` and `paint`.
- The block cursor is one quad from the cursor's grapheme to the next one **on the same
  visual line**. Paint it **under** the text, where the selection quads are painted, not
  after it.
- Fill `--selection`; 1 px `--link` outline. `--selection` is already the fill that text
  must stay readable on, so no new colour token is needed.
- It is steady. Do not blink it; the blink belongs to the Insert caret.
- A minimum width applies in three cases: an empty block, a line break, and where the
  next grapheme wraps to the next line. Add the constant to
  `crates/utopia-ui/src/theme.rs`, e.g. `pub const VIM_CURSOR_MIN_WIDTH: f32 = 8.;`.
  Do not write a raw number in `field.rs` (DESIGN.md §2.4).

### Task 1E.3 — The Normal clamp

The cursor rests on a grapheme and never after the last one of its line. An empty block
has it at 0. Call `clamp_normal` after every motion, after `sync_from_store`, after
`insert_text`, after `attach_paths`, after a dropped file and after `run_plugin`.

## 1F — The mode indicator

File: `crates/utopia-ui/src/workspace.rs`. Put the label in the top bar's right-hand
group, left of the view-mode control. Show it only when Vim is on. Text is the mode name
in upper case, then a space, then `parser.pending()`. Use `theme::label_mono()` and
`--ink-3`. Redraw it on `EditorEvent::VimChanged` only.

Add one case to `crates/utopia-ui/tests/layout.rs` that asserts the label's
`debug_selector` bounds are inside the top bar. Read the note in `AGENTS.md`: GPUI's test
frames never clear debug bounds, so assert on what replaced an element, never on
`debug_bounds(..).is_none()`.

## 1G — Tests

### Task 1G.1 — `crates/utopia-ui/tests/vim.rs`

New file. Use the harness in `crates/utopia-ui/tests/workspace.rs`:
`simulate_keystrokes` and `simulate_input`. Turn Vim on in the test's config dir.
Assert on store state, undo depth (`Store::undo_depth`), and the editor's target and
cursor. Never assert on absent `debug_bounds`.

Name each test after its requirement id:
- `vm_2_with_vim_off_a_letter_still_types`
- `vm_4_a_block_in_edit_starts_in_normal_mode`
- `vm_5_normal_mode_types_nothing`
- `vm_5_an_unknown_normal_key_does_nothing`
- `vm_6_w_at_the_end_of_a_block_stays_in_the_block`
- `vm_7_j_crosses_into_the_next_block`
- `vm_13_escape_ladder_ends_at_rest_and_a_key_wakes_it`
- `vm_1_tab_and_cmd_chords_still_reach_the_keymap`

### Task 1G.2 — IME tests

Drive the input-handler methods, as the `ac_13_5_*` tests do:
- Marked text in Insert, then Escape: the composition and the mode do not change.
- Marked text offered in Normal: neither `field.text` nor `Composition` changes.
- Committed text in Normal reaches the parser and not the block.
- A click into Normal during a composition keeps the composed characters.

### Task 1G.3 — Robustness

A seeded random key string of a few thousand keys against a small graph: no panic
(rule 11), `doctor` stays clean (rule 2), and a full undo restores the start (rule 5).

**PR 1 acceptance**
- With Vim off, every existing test passes unchanged.
- With Vim on, a seeded outline can be walked and edited with the PR 1 keys.
- No key types a character in Normal mode.
- `cargo run -p xtask --release -- bench --ui` passes.
- Screenshots show the block cursor and the mode label in light and dark.

---

# PR 2 — Operators, registers and block commands

## 2A — Undo runs

### Task 2A.1 — `undo.rs`

File: `crates/utopia-core/src/undo.rs`.
```rust
impl UndoStack {
    /// Open a run. Every group pushed until `end_run` folds into one.
    pub fn begin_run(&mut self);
    /// Close the run and fold its groups into one group, in order.
    pub fn end_run(&mut self);
    pub fn in_run(&self) -> bool;
}
```
A group is an ordered list of steps whose inverses are applied in reverse, so folding is
safe: concatenate the step lists in push order. An undo that starts while a run is open
closes the run first. The 1000-group limit must still hold after a fold.

### Task 2A.2 — Store wrappers

File: `crates/utopia-store/src/lib.rs`: `pub fn begin_undo_run(&mut self)` and
`pub fn end_undo_run(&mut self)`.

### Task 2A.3 — Where the editor opens and closes a run

File: `crates/utopia-ui/src/editor.rs`. Open the run when Vim enters Insert, **before**
the delete of a `c` command and before the split of `o`. Close it on Escape, on blur and
when the session ends.

### Task 2A.4 — Tests

File: `crates/utopia-core/src/undo.rs` unit tests and `crates/utopia-ui/tests/vim.rs`:
- An Insert run with pauses longer than 300 ms is one group. The clock is settable
  (`Store::set_clock`).
- `cw` plus text is one group.
- A run closed by a blur is one group.
- An undo during an open run closes it first.
- 200 undos after a scripted Vim session give a byte-identical projection.

## 2B — Charwise operators

File: `crates/utopia-ui/src/vim.rs`.

Each of `x X D r{c} ~ d{motion} y{motion} c{motion} C s S` computes a range from
`self.field.text` with the `utopia_core::vim` functions, writes the register, then calls
`discrete_edit`, which seals the undo group on both sides. One command is one undo step.
Guard on `self.field.composing()`, as `EmacsKillLine` does.

`c` and `s` and `S` and `C` enter Insert after the delete, inside one undo run.

## 2C — Linewise block commands

### Task 2C.1 — New planners in `crates/utopia-core/src/outline.rs`

```rust
/// This block and its next `count - 1` siblings, each with its subtree. One plan, so
/// one undo step. `Command::SetDeleted` already takes a list of ids.
pub fn delete_blocks(/* … */) -> Plan;
/// Join the next visible block into this one with a space. Its children re-parent to
/// this block. It is tombstoned, never hard-deleted (rule 5).
pub fn join_next(/* … */) -> Plan;
/// Paste a yanked tree after (or before) a block. New ids. `InsertBlock` carries
/// `kind`; a checked to-do also needs `SetBlockKind`.
pub fn insert_tree_after(/* … */) -> Plan;
/// Restore tombstoned blocks and move them in one group, so `dd` then `p` keeps ids.
pub fn restore_after(/* … */) -> Plan;
```
`merge_into_previous` in the same file is the model for `join_next`. `delete_subtree` is
the model for `delete_blocks`; note that it focuses the **previous** block today, while
the Vim path must pass its own focus, because Vim puts the cursor on the **next** block
after `dd`.

### Task 2C.2 — New editor events

`DeleteBlocks { count: usize }`, `JoinNext { count: usize }`,
`PasteBlocks { before: bool }`, and `YankBlocks { count: usize }`. Handle each in
`page_view.rs::on_editor_event`. Each one runs **one** `Store::execute_plan`, so `3dd`
is one plan and one undo step, not three events.

`>>` and `<<` with a count reuse `EditorEvent::Indent` and `Outdent` for count 1.
Multi-block indent is PR 4.

### Task 2C.3 — The register and the clipboard

- `y` writes the register **and** the system clipboard. Blocks go to the clipboard as
  nested Markdown, which `subtree_to_markdown` already makes.
- `d`, `c` and `x` write the register only.
- `p` uses the system clipboard when its text differs from what Utopia last wrote there;
  otherwise it uses the register.
- `"_` discards. `"+` is the clipboard. No other named register in PR 2.
- The Ctrl+K kill buffer in `kill.rs` does not change. It is the Insert-mode pair.

### Task 2C.4 — `dd` then `p`

| Case | Behaviour |
|---|---|
| `yy` then `p` | Copy. New ids. Keep `kind` and the checked state. |
| `dd` then `p`, first paste | **Move.** `SetDeleted { ids, None }` restores the blocks and the same plan moves them. One undo step. Same ids, so every `((reference))` still resolves. |
| `dd` then `p`, later pastes | Copy. |
| `dd` then `p` after the delete was undone, or the trash was emptied | Copy. |
| `dd` then `p` on another page | `cross_page_move` in `crates/utopia-store/src/extras.rs` keeps ids but refuses a block that is not live. Write a store method that restores and moves in one group. **If that proves hard, fall back to a copy and write the limit in the PR description.** |
| `p` of clipboard text with line breaks | Today's `EditorEvent::PasteOutline`. |

After `dd` the cursor goes to the next block.

### Task 2C.5 — Tests

- A store test: `dd` then `p` keeps every block id, and a `((reference))` to a moved
  block still resolves.
- A to-do yanked and pasted is still a to-do.
- Each block command is one undo step.
- `ciw`, typing, Escape, `u` restores the word in one step.
- `doctor` stays clean after a scripted run.

## 2D — Text objects in the editor

Wire `d`, `c` and `y` to `vim::textobj::range_of` for `iw aw iW aW i( a( i[ a[ i{ a{
i" a" i' a'` and the backtick pair. `ci[` inside `[[Alpha]]` must leave the caret inside
the link, so entering Insert opens the `[[` menu.

**PR 2 acceptance**
- Each command is one `u`.
- `ciw`, typing, Escape, `u` restores the word in one step.
- A to-do yanked and pasted is still a to-do.
- `dd` then `p` keeps ids; a `((reference))` still resolves.
- `bench --ui` passes.

---

# Verification (AGENTS.md, "Verifying a change")

Four steps, in order, for PR 1 and PR 2.

1. `make check` — fmt, build, test, clippy.
2. Seed a scratch graph by driving the `Store` directly. `seed_help_graph` in
   `xtask/src/main.rs` is the worked example. Seed under `target/vim-shots/`, never into
   a real graph.
3. Drive the real app with `UTOPIA_SHOT_SCRIPT`, with `UTOPIA_CONFIG_DIR` and `--graph`
   both on scratch paths. Set `"vim": true` in the scratch `settings.json` first.
   Capture: the block cursor on ASCII; on an emoji; in an empty block; at a wrap; the
   mode label in Normal and in Insert. Capture each in light and dark with
   `UTOPIA_APPEARANCE`.
4. Read every PNG. Check the text is the seeded data, nothing overflows, and the change
   is visible. Save the keepers under `docs/verification/feat-vim-mode/`, downscaled with
   `sips -Z 1200`.

---

# Known limits to write in the PR description

- The real input-method paths (dead keys, CJK) are read from gpui's source, not measured.
  A manual layout matrix (US, one dead-key layout, Japanese, one non-Latin) is an
  acceptance item of PR 1. Record what you ran.
- gpui is pinned at `=0.2.2`. Interceptor order, key-context depth and `key_char`
  behaviour are facts of that version.
- `field.vertical` needs a layout shaped from the current text and returns `None`
  otherwise. Between an edit and the next paint, `x` then a fast `j` can therefore leave
  a multi-line block early. The fix is to queue the vertical move and resolve it in
  `prepaint`, where `Goal` is already resolved.
- Dot-repeat (`.`), Visual mode, folds, `{`, `}`, `%`, `il`/`al`, `gd` and `:` are PR 3
  and PR 4.
- Vim is not available in `LineInput` fields: the palette, filters, rename and Settings.
