# 0013. Vim mode: the grammar, the state and the editor

Status: Accepted. Source: `crates/utopia-core/src/vim/` (`mod.rs`, `parse.rs`, `motion.rs`,
`textobj.rs`, `operator.rs`), `crates/utopia-core/src/undo.rs` (runs), the Vim planners in
`crates/utopia-core/src/outline.rs`, `Store::restore_and_move_to_page`,
`crates/utopia-ui/src/vim.rs`, and the Vim parts of `editor.rs`, `field.rs`, `page_view.rs`,
`workspace.rs` and `actions.rs` (PRD §9.1.1, VM-1 to VM-14; DESIGN.md §3.1;
`docs/state-machines/vim-mode.md`; `docs/vim-mode-plan.md`). The `F<n>` ids are the defects the
reviews found: in the PR 1 sections they are PR 1's (pinned by the `f<n>_…` tests in
`crates/utopia-ui/tests/vim.rs`), and in the PR 2 sections they are PR 2's.

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

### Undo runs (PR 2, VM-12)

- An Insert run (`UndoStack::begin_run` to `end_run`) folds every group pushed during it into
  one undo step, so the 500 ms coalesce window does not apply inside it. Its first group never
  coalesces with the edit before it. The fold concatenates the groups in push order: each
  group's inverses are applied in reverse, so each inverse stays next to the step it undoes.
- The run opens before the command acts: before the delete of `c`, `s`, `S` and `C`, and before
  the split of `o`. So the command and the text typed after it are one step. Opening a run that
  is already open changes nothing.
- The stack cannot tell the editor's writes from others, so every other writer closes the run
  first: `utopia_api::handle` (an assistant's `/v1/op` call, MC-8, MC-9) and `Store::capture`.
  Without that, the owner's next `u` would also take the assistant's write (F4).
- An undo or a redo closes the run that it finds open, and the editor's `insert_run` flag
  follows the store. When something closed the run behind the editor (a Cmd+Z, an API write),
  the next keystroke opens a new one. A stale flag would never reopen the run, and the rest of
  the Insert session would be one step per keystroke (F7).
- Every path that ends Insert (Escape, a blur, a click, the end of the session) closes the run
  through `BlockEditor::close_insert_run`, after its last commit, so the committed text is
  inside the run. `VimState` does not close it itself, so the flag and the stack cannot drift.
- The run limit is the stack's limit: a run longer than the history is still bounded, and the
  fold never leaves more groups than the limit.

### Charwise operators (PR 2)

- A motion is a jump and an operator is a span. `utopia-core::vim::operator` owns Vim's
  inclusive and exclusive rules (`dw` stops before the next word, `e` and `f` include the
  grapheme they land on, `$` stops before the line break), so `d`, `c` and `y` cannot disagree.
- An exclusive end that lands on the first grapheme of a line belongs to the end of the line
  before it (`:help exclusive`), so `dw` over a line's last word keeps the line break. For this,
  the operator uses `word_forward_raw`, the end that the motion wanted before the Normal clamp.
- `cw` on a word takes the word under the cursor whole. Vim treats it as `ce`, but `e` takes its
  first step to the right, so from the last character of a word it ran on into the next word
  (F5). On a blank, `cw` is `dw`.
- A find to the left (`dF`) starts on its target and leaves the cursor's own grapheme.
- `x` never joins two lines. `3D` reaches the end of three lines. `r` and `~` replace, so they
  write no register. A text object that is not there writes nothing. An empty range
  (`ci(` in `()`) still enters Insert.
- A register before a command that deletes first is kept for that delete: `"_s` discards and
  `"+C` puts the text on the clipboard (F9).

### Block commands and the register (PR 2, VM-8, VM-10, VM-11)

- A count on `dd`, `yy` and `J` takes siblings, each with its subtree, because a block's
  visible children are its own next rows. Each command with its count is one plan, so `3dd`
  and `3J` are one undo step.
- After `dd` the cursor goes to the next visible row, as in Vim (Cmd+Shift+Backspace goes to the
  previous one). Visible means the zoom and the collapsed set decide, or `dd` on the last row of
  a zoom would give the caret to a block the view cannot show (F12). The properties bar is not
  the outline: `dd` does not delete it and `p` does not paste into it.
- The register's `cut` and the delete both come from `outline::block_ids`, so a `p` restores
  exactly what the delete took. A revived tombstone (§12.4) goes and comes back with the rest.
- The first `p` after a `dd` is a move, not a copy: it restores the tombstones and moves them,
  so every id survives and each `((reference))` still resolves. On the same page that is one
  plan (`outline::restore_after`). On another page it is one undo group in
  `Store::restore_and_move_to_page`, because it touches two shards. Later pastes, a cut that was
  undone, and a cut whose blocks have gone copy with new ids.
- A cut is still a cut only while every block it took is a tombstone of the source page and has
  no `moved_to` (a drag, a merge or a sync moved it away). Otherwise a restore would leave a
  live block marked `moved_to` (F11).
- The cross-page move decides every refusal before its first command. A group that fails half
  way goes through `Store::drop_shard`, and a dropped shard takes the owner's unflushed typing
  with it, so a refused `p` must write nothing (F3). A refusal for depth (FR-2.9) keeps the cut
  for a shallower `p`. A refusal because the blocks have gone spends the cut, and the `p` copies.
- When the destination already has a block's row as `moved_to` history, the move restores that
  row instead of inserting a copy, because `InsertBlock` refuses an id the shard knows. That
  keeps the id (F3).
- A moved block hangs from its own parent when that parent comes too, and otherwise from its
  nearest ancestor that does. Its anchor is the nearest previous sibling that the cut also
  brings: a tombstone that stays behind is not an anchor in the destination, and `InsertBlock`
  puts a block with an unknown anchor first, which reversed the children (F1).
- The unnamed register is memory only, one per thread (rule 9). A yank writes the register and
  the system clipboard (as Markdown, which is all the clipboard can carry). `d`, `c` and `x`
  write only the register, as in Vim; `"+` names the clipboard and `"_` writes nothing.
- `p` takes the system clipboard only when it changed after the register was written. The
  register records what Utopia last wrote to the clipboard and, for a delete, what the clipboard
  held at that moment. So `x` then `p` pastes what `x` took, and a later copy in another app
  wins (F2, F6). A live cut wins over an unchanged clipboard, because the clipboard has no
  linewise flag and would turn the move into a copy.

### Known limits of PR 2

- A count on the paste of a cut is not honoured: the first `p` moves the blocks and spends the
  cut, so `3p` pastes once. A yank's paste does make `count` copies (F9).
- A count on `>>`, `<<` and `cc` acts on the cursor's block only; multi-block indent and change
  are PR 4 (F9). `>` with a motion is PR 4.

## Consequences

- The grammar can be tested without a window, and the help card cannot ship a key the
  parser rejects.
- The mode can outlive a block, so each new kind of block entry must decide whether it is a
  command that keeps the mode (`j`, `o`) or an entry that resets it (a click).
- The clamp is not enforced by a type: a new path that places the caret must call it.
- Any new writer outside the editor (a new API route, a new capture path) must close the
  Insert run before it writes, or the owner's next `u` takes its write too.
