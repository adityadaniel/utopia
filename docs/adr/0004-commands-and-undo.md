# 0004. Invertible commands, one global undo stack

Status: Accepted. Source: `utopia-core` `command.rs`, `undo.rs` (PRD §13.2, FR-2.12, §8.6, AC-4.2).

## Decision

- `Command` is the only way page content changes. Every command is invertible against the
  state it applies to (`Command::invert`), and that is how undo works, with no dependency on
  CRDT internals.
- User deletes are `SetDeleted` tombstones (§6.5). `PurgeBlock` exists only to invert
  `InsertBlock`. `SetMoved` marks blocks that went to another page's shard in a merge
  (§8.6 step 1); moved blocks are invisible in the old page.
- Page metadata (`PageMeta`) is stored in the page's own shard, so aliases and merge
  tombstones replicate as ordinary updates (§8.6 step 6). A daily page's `title` is ignored:
  its title is a rendering of `daily_date`. `merged_into` is set when a page was merged
  away, and live blocks that arrive there later are merged again into the target.
  `canonical_aliases` keys aliases by `title_key`, first spelling wins, sorted, and the
  current title is never its own alias (§10.4 invariant 8).
- `TextEdit` ranges are byte offsets into the NFC text.
- `DocDelta.structural` means the tree shape or visibility changed, so projections diff the
  whole page; otherwise only the touched rows are rewritten (§13.6).
- Undo groups store the forward commands and their inverses per page, so undo and redo are
  ordinary command applications and sync like any other edit.
- The stack is **global, not per page**: a merge or an edit through a transclusion touches
  several pages, and one linear history guarantees that every inverse is applied to exactly
  the state it was computed against.
- Text edits to the same block within `COALESCE_MS` (500 ms) join one group. The state is an
  enum, `Coalesce::{Sealed, Open { block, at_ms }}`, so "sealed, but at a time" cannot be
  built. A clock that went backwards starts a new group.
- `UndoStack::begin` takes a group out; the caller must give it back with `finish` (the
  group is now the other stack's top, sealed) or `abort` (back where it was, coalescing state
  included, so a failed Cmd+Z does not split a run of keystrokes). History cannot vanish
  because something after the pop failed.
