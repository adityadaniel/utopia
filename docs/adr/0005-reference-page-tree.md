# 0005. `PageState` is the CRDT-free reference semantics

Status: Accepted. Source: `utopia-core/src/tree.rs` (PRD §6.1, §12.4, §13.6).

## Decision

- `PageState::apply` is the definition of every command without a CRDT. The Loro
  implementation in `utopia-doc` is tested against it (fuzz tests compare the incremental
  cache with a full rebuild), and the editor mirrors structural commands on it instead of
  rebuilding the page.
- `children` and `roots` hold every block, tombstoned and moved ones too. `present` means
  "belongs to this page" (not moved away, no moved ancestor); `live` means present and
  visible.
- A tombstoned block with a live descendant (a concurrent child arrived under a deleted
  parent) is revived, so content is never hidden (§12.4).
- After an insert or a move only the subtree and the ancestor chain are refreshed
  (O(subtree + depth × siblings)), so a keystroke-sized edit never walks the whole page.
- `position` is the fractional index from the document layer; it is empty in pure models.
  `version` goes up on every applied command so views can skip work.
- `canonical()` is an order-and-content projection without positions or timestamps, used
  to compare states in convergence and undo tests.
