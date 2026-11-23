# Verification shots — Vim mode (PR 0 + PR 1)

Captured with `UTOPIA_SHOT_SCRIPT` against a scratch graph seeded by
`cargo run -p xtask -- seed-page-view`, with `"vim": true` in a scratch
`UTOPIA_CONFIG_DIR`. The user's own graph was never opened.

| Shot | What it proves |
|---|---|
| `block-cursor-light.png` | VM-3, VM-6: after `ww` the block cursor rests on the last character of "Goals" and does not leave the block. Other blocks stay rendered; only the block in edit is live. The `NORMAL` label sits in the top bar, left of the view-mode control (DESIGN.md §3.3). |
| `block-cursor-dark.png` | The same frame in the dark theme: the glyph stays readable on the `--selection` fill, so no new colour token was needed. |
| `mode-label-pending.png` | A pending count renders after the mode name: `NORMAL 3`. |
| `mode-label-insert.png` | `i` enters Insert and the label follows. This is the regression that stage 3c left and stage 3d found: `i`/`a`/`I`/`A` moved the caret without changing the mode. |
