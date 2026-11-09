# Undo group: open for coalescing vs sealed

## Status

The machine is now explicit in two places, and issues 1 and 3 are fixed.

- **Coalescing is a sum type.** `utopia_core::undo::Coalesce` is `Sealed` or
  `Open { block, at_ms }` (`undo.rs:35-64`), replacing `Option<BlockId>` plus a free
  `at_ms`: "sealed, but at a time" cannot be constructed, and only `Coalesce::joins`
  decides whether an edit merges (same block, `0..=COALESCE_MS` after the group opened).
- **A transition owns its group.** `undo()`/`redo()` are one `Store::history(dir)`
  (`lib.rs:1157-1230`) driving `UndoStack::begin` → apply → `finish` or `abort`
  (`undo.rs:154-179`). `begin` takes the group out of the history; the caller hands it back
  on *every* path, so "popped and then dropped" no longer exists as a reachable state.
- **Issue 1 (fixed).** All shard loads happen before the first inverse
  (`lib.rs:1179-1191`); a page with no shard left (Empty Trash, §6.5) has its steps skipped
  and any other load error `abort`s the group untouched. The history is moved to the other
  stack *before* the fallible `queue` (`lib.rs:1227-1228`), because a failed writer is
  repaired from the shards and the stack must agree with them. Tests:
  `fr_2_12_undo_keeps_the_group_when_a_page_is_gone` and
  `fr_2_12_undo_keeps_the_group_when_its_commit_fails`
  (`crates/utopia-store/tests/undo.rs`).
- **Issue 3 (fixed).** `execute_multi` records the group even when `queue`/`flush` fails:
  the edit is already in the shard and in `pending` (rule 5), so it must be undoable
  (`lib.rs:1037-1068`). Test: `fr_2_12_an_edit_whose_commit_failed_is_still_undoable`,
  which asserts `Store::undo_depth` grew and that the edit comes off after the repair.
- **Issue 2 (already fixed, no change here).** It depended on `sync_index` clearing `docs`;
  the store-commit-pipeline fix keeps every loaded shard and repairs by queueing full
  snapshots (`lib.rs:613-632`, `:546-600`), so stored inverses still name states that
  exist. The `if let Ok(d)` skips in undo/redo remain for remote edits that made an inverse
  inapplicable, which is a different, deliberate case (§2.3).
- **Issue 4 (not fixed).** A frozen clock (`Store::set_clock`) still coalesces every
  same-block edit. It is only reachable from tests, where "the same instant" merging is the
  intended reading; a *backwards* clock (NTP) now seals instead of merging, because the
  window is checked as `0..=COALESCE_MS` (`undo.rs:52-64`,
  `undo::tests::a_clock_that_goes_backwards_seals_the_group` (`crates/utopia-core/src/undo.rs`)).

The sections below describe the machine as it was when this doc was written; line numbers
there are historical.

## What it controls

Whether consecutive keystrokes in one block merge into one undo step (FR-2.12: same block,
within 500 ms) and what Cmd+Z applies. `crates/utopia-core/src/undo.rs` and
`crates/utopia-store/src/lib.rs`.

## Where the state lives

| Piece | Location | Set | Cleared |
|---|---|---|---|
| Coalescing state | `undo.rs:35` `Coalesce::{Sealed, Open { block, at_ms }}` on the newest `UndoGroup` | `execute_multi` maps `opts.coalesce` to `Coalesce::open` (`lib.rs:1000-1003`); only `set_block_text` passes `Some` (`lib.rs:1128`) | `seal` (`undo.rs:142`); `finish` (`undo.rs:163`) |
| Stacks | `undo.rs:111-114` `undo`, `redo` | `push` (`undo.rs:122-140`, clears redo), `finish`, `abort` | – |
| Transition | `history` owns the group between `begin` and `finish`/`abort` (`lib.rs:1170-1230`) | `finish` after the commands applied | `abort` on a failed pre-flight |
| Seal triggers | UI blur `editor.rs:943`; API `utopia-api/src/lib.rs:332, 367`; every structural store op (`pages.rs:550, 560, 663, 665, 681, 690`; `extras.rs:135, 145, 246, 256, …`) | | |

## States

**Open** (top group is `Coalesce::Open { block, at_ms }`; the next text edit of the same
block within `COALESCE_MS` merges) / **Sealed** (`Coalesce::Sealed`) / **In transition**
(the group is owned by `history`: it is on neither stack and cannot coalesce).

## Events and transitions

- text edit → `push(group)`; merges if the new group is open for the same block at most
  500 ms after the top (`undo.rs:52-64`), else pushes a new group.
- blur / Enter / structural op → `seal`.
- undo/redo (`lib.rs:1170-1230`): `begin` → load every page → skip steps whose page is
  gone → apply inverses (reverse order) or forwards → `finish` → `queue`.

## Fixed combinations (were "impossible or unhandled")

1. **A group is popped before anything fallible runs.** Fixed: the group is only taken out
   to be applied, every fallible load happens first, and `finish`/`abort` always runs.
2. **Stack survives shard rollback.** Fixed by the store-commit-pipeline change: shards are
   never discarded on a failed commit, and a repair rewrites them whole.
3. **Edit applied but not undoable.** Fixed: `execute_multi` pushes the group before
   returning the queue/flush error.
4. **Frozen clock coalesces forever.** Unchanged; tests only (see Status).

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Sealed
    Sealed --> Open : set_block_text (Coalesce::open(block, now))
    Open --> Open : same block within 500 ms (merge)
    Open --> Open : other block / > 500 ms / backwards clock (new group, old sealed)
    Open --> Sealed : seal (blur, Enter, structural op, API)
    Open --> InTransition : undo/redo (begin takes the group)
    Sealed --> InTransition : undo/redo (begin takes the group)
    InTransition --> Sealed : finish (applied; the other stack's top)
    InTransition --> Open : abort (failed pre-flight; state restored unchanged)
```
