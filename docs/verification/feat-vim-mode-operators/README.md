# Verification shots — Vim mode operators (PR 2)

`dd` then `p` then `u` `u`, driven with `UTOPIA_SHOT_SCRIPT` against a scratch graph seeded
by `cargo run -p xtask -- seed-page-view`, with `"vim": true` in a scratch
`UTOPIA_CONFIG_DIR`. The user's own graph was never opened.

| Shot | What it proves |
|---|---|
| `1-before-dd.png` | The block cursor on the to-do "Send [[Ana Lovelace]] the migration notes". The block in edit shows its raw markup, as DESIGN.md §3.1 requires; every other block stays rendered. |
| `2-after-dd.png` | `dd` tombstoned the block and the cursor moved to the **next** block, which is Vim's rule. `delete_subtree` focuses the previous block, so the Vim path passes its own focus. The sidebar's reference count for Ana Lovelace drops from 2 to 1: the `owner::` attribute still links her, the deleted block no longer does. |
| `3-after-p.png` | `p` moved the same block below `make check`. It is still a to-do, still ticked, and `[[Ana Lovelace]]` is intact, so `kind`, the checked state and the reference all survived (VM-10). The count returns to 2. |
| `4-after-undo.png` | Two `u` presses put the block back where it started, ticked and linked. `dd` and `p` are one undo step each (VM-12). |
