# 0007. Outline edits are planners; depth is refused, not clamped

Status: Accepted. Source: `utopia-core/src/outline.rs`, `text.rs` (PRD §6.3, Appendix C,
FR-2.2 to FR-2.10, §13.8 MC-8, §6.5).

## Decision

- A planner reads a `PageState` and returns the commands plus where the caret lands. It
  never mutates state, so a whole plan is one undo step.
- Enter splits at the caret; the tail is the first child when the block has expanded
  children, otherwise the next sibling. At offset 0 of a non-empty block a new empty block
  goes above and the caret stays.
- Backspace at offset 0 merges into the previous visible block (text appended, children
  re-parented, block tombstoned); on the first block it outdents.
- Move up/down at an edge leaves the parent (becomes its previous or next sibling).
- Paste of one line goes in at the caret; several lines become blocks after the current one,
  or fill it when it is empty and childless.
- `MAX_DROP_DEPTH` is 12 (FR-2.9). Drag-and-drop, `move_to` and `append_outline` refuse a
  result past it; they never clamp. The ceiling counts from the parent the outline goes under.
- `append_outline` (MC-8) adds blocks last, under the parent or at the page end, and moves
  nothing that is already there. An empty `anchor` block (a fresh page's only block) takes
  the first line, and its kind, so a day's placeholder becomes the to-do.

## Caret

`carry_caret` places the caret in the new text from the old and new texts alone, through
their single-range `diff`, so no caret anchor has to be kept in step with the store. A caret
before the change stays; after it, it moves by the change in length; inside it, it keeps its
place when the replacement still holds that text, otherwise it goes to the end of the
replacement.
