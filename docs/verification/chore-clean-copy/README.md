# chore/clean-copy — verification

Captured with `UTOPIA_SHOT_SCRIPT` against a scratch graph (`xtask seed-page-view`) and a
scratch config dir, light appearance, downscaled to 1200 px. The plan is `docs/copy-cleanup.md`.

| Shot | What it proves |
|---|---|
| `settings-keybindings.png` | Pane header is the title alone (no summary line). The hint reads "Click a key, then press the new one." Filtered to `cmd-c`: the Copy row has no help line at all, not an empty one. |
| `settings-keybindings-capture.png` | The Escape fact moved into the capturing state: the chip reads "Press keys · Esc cancels". |
| `settings-appearance.png` | Font samples stay; Density rows are the label and radio only; References keeps the one load-bearing sentence about 0; Pinned pages has no storage caption. |
| `settings-appearance-lower.png` | Right sidebar: "Width: 380 px, set by dragging the sidebar's left edge." above the Reset button. |
| `settings-sync.png` | Sync, Off: no "Sync this graph" heading, no bare "Off" fact above the mode buttons, no "Apply to stop sync" line. The Graph id caption is the two consequences. |
| `settings-sync-nearby.png` | Nearby Mac: the rewritten mDNS/Tailscale line, the host toggle without its caption, placeholders "Blank keeps the saved token/passphrase", and the one-sentence passphrase caption. |
| `onboarding.png` | Title, "Name your graph" placeholder and the "Creates …" path; the "single file you own" line is gone. (Isolated `HOME`, so the path is long and wraps.) |

Not captured: the sync popover in the main window footer and the right sidebar's Pinned tab
(their changed strings are one line each and are covered by `tests/settings.rs` and
`tests/workspace.rs` compiling against the new text); the unlock screen (needs an encrypted graph).
