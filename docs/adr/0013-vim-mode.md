# 0013. Vim mode: the grammar, the state and the editor

Status: Accepted. Source: `crates/utopia-core/src/vim/` (`mod.rs`, `parse.rs`, `motion.rs`,
`textobj.rs`), `crates/utopia-ui/src/vim.rs`, and the Vim parts of `editor.rs`, `field.rs`,
`page_view.rs`, `workspace.rs` and `actions.rs` (PRD §9.1.1, VM-1 to VM-14; DESIGN.md §3.1;
`docs/state-machines/vim-mode.md`; `docs/vim-mode-plan.md`). The `F<n>` ids are the defects the
PR 1 review found; the `f<n>_…` tests in `crates/utopia-ui/tests/vim.rs` pin them.

## Context

Vim mode is a modal layer over the one block editor that a window shares between the main
pane and the sidebar's pages. The PRD says what the modes do. This record keeps the choices
that the code does not show and that are easy to undo by mistake.

## Decision

### The grammar (`utopia-core::vim`)

- The grammar is pure: it takes keys and byte offsets into one block's text and answers with
  steps and offsets. It has no CRDT, store or `gpui` dependency, so it cannot write a graph or
  block a render (rules 9 and 10).
- Every offset a motion or a text object returns comes from a grapheme, and every offset it
  takes is snapped down to a grapheme boundary first. A caller may slice with any of them
  (rule 11). A CRLF is one grapheme.
- Three character classes, as Vim has them: word (alphanumeric and `_`), punctuation (any
  other non-blank), blank. `w b e` stop at each run of punctuation; `W B E` fold punctuation
  into words. `text::prev_word_start` and `text::next_word_end` are the Emacs rules that
  Alt+Left and Alt+Right use. They skip punctuation and stay separate: do not merge the two.
- A find (`f t F T`) looks on the cursor's line only. A count counts targets first and applies
  the `till` stop once, to the target it found, so `2tx` passes the first `x`. `;` after a `t`
  passes the target the cursor already stands next to, as Vim does (F6).
- Text objects: on a closing bracket, the cursor is inside the pair that it closes (Vim's
  `findmatch`). Quotes pair up along the cursor's line only. `aw` takes the blanks after the
  word, or the blanks before it when the word ends the line.
- The parser checks a key in this order: Escape; an argument that a key waits for; the `g`
  prefix; digits; keys that take a character; operators; motions; other commands. The
  argument comes before the digits, so `f2` finds a `2` and does not start a count. A leading
  `0` is `LineStart`, never a count. A register comes before an operator (`"_dd`), never after
  it. `r` is a command only without an operator.
- Space is the `Right` motion, but as an argument it is the character itself: `f<Space>`,
  `t<Space>` and `r<Space>` work (F10). The Delete key is `x`.
- A count clamps at `MAX_COUNT` (10,000), and so does the product of two counts (`2d3w`). Each
  unit of a count is real work (`99u` is 99 commits and 99 page rebuilds). When the count
  saturated at `usize::MAX`, a long digit run froze the app (rule 11, F9).
- `REFERENCE` is the help card. A test feeds every row to the parser, and a second test
  checks that every key the grammar accepts has a row, so the card and the grammar cannot
  drift apart.

### The state (`utopia-ui::vim`)

- `VimState` sits on the `BlockEditor`, beside the session and not inside it. `begin` makes a
  fresh session for each block, and `j`, `k`, `o` and `O` cross blocks through it with the mode
  and its pending count. Only a finished edit (`end`, `detach`) resets the state to Normal.
- Because `begin` keeps the mode, a click into another row must reset it explicitly. Both
  rows share one focus handle, so no focus-loss event fires between them (VM-4, F2).
- The resume point holds the caret and also the `PageView` that hosted the edit. The same
  editor serves the sidebar, so a wake-up in the main pane would put the caret on a different
  page (VM-13, F5). When the block at the resume point is gone (sync, an assistant, a merge),
  the wake-up drops the resume point and opens the page's first block. When nothing can open,
  the key is not swallowed.
- A focus loss closes the Insert run, but Normal stays Normal with its pending keys, so a
  half-typed `2d` is not dropped silently (VM-13).
- `EditorEvent::VimChanged` carries nothing and only the mode label listens to it, so a Vim
  keystroke does not redraw the window (rule 10).

### Keys and the workspace

- The Vim keys are in the same keystroke interceptor as the page menu. gpui calls every
  interceptor even after one of them stops propagation, so a second interceptor would act on
  the same Escape.
- `key_from_keystroke` classifies a key in this order. A Cmd chord goes to the keymap. Tab and
  Shift-Tab go to the keymap, and this test comes before the character test because the test
  platform gives them a `key_char`. Named keys come before printable characters, and Shift is
  let through (Shift+arrow is a motion). On macOS every Ctrl+letter is claimed, because an
  unclaimed one types its control character into the block. On other platforms the Cmd
  defaults are spelled Ctrl, so only the Ctrl keys the grammar knows are claimed.
- While a block is in edit, the interceptor does not check what the main pane shows. A
  sidebar page stays editable while the main pane shows the graph, the Tasks view or a PDF
  (F1). The Rest wake-up is different: it acts only when the main pane shows a page and the
  workspace's own handle has focus, so a key for a palette or a sidebar row never reopens the
  editor.
- The Escape ladder is two bindings. The `BlockEditor && VimInsert` binding is listed after
  the plain Escape binding: at the same depth gpui prefers the later binding, so the first
  Escape leaves Insert and a second one ends the edit (VM-13). While an input method composes,
  Escape belongs to it, so the ladder starts after the composition ends (§13.5).
- The toggle action has no default key (VM-1). A test checks that every other catalog default
  resolves to a binding.

### The editor and the cursor

- The Normal cursor rests on a grapheme, never after the last one of a line, and never on a
  line break. Every path that can place the caret calls the clamp: a store change, an insert,
  an attachment, a plugin's rewrite, block entry, a click and a goal resolved at paint. Each of
  these paths was written without a line in mind (F4). A click that selects a range (double
  click, Shift) keeps the insertion point it placed.
- The autocomplete menu and the idle commit that holds it are Insert-only: in Normal the
  caret only moves through `[[Alp`. Entering Insert inside a trigger opens the menu, and
  Escape from Insert closes it, so Tab indents again (VM-3, F3).
- Normal takes no marked text from the input method, and the buffer does not change when it
  refuses (VM-14).
- The block cursor is steady: the blink timer does not start in Normal or Visual. It is
  painted with the selection quads, before the text, so the character under it stays
  readable. It uses the `--selection` fill and a 1 px `--link` outline, so it needs no new
  colour. `VIM_CURSOR_MIN_WIDTH` keeps it visible where there is no cell to fill: an empty
  block, a line break, or a grapheme that wraps.
- A count of `j` or `k` first walks the wrapped lines inside the block, then the host moves
  the rest as whole blocks. An upward count clamps at the first body row when there is no
  properties bar to use the rest of it, as a downward count clamps at the last row (VM-7,
  F8). `gg` and `G` use the page that the cursor's block is on: in the Daily Notes feed that
  is the day.

## Consequences

- The grammar can be tested without a window, and the help card cannot ship a key the
  parser rejects.
- The mode can outlive a block, so each new kind of block entry must decide whether it is a
  command that keeps the mode (`j`, `o`) or an entry that resets it (a click).
- The clamp is not enforced by a type: a new path that places the caret must call it.
