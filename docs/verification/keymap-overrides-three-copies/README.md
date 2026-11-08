# Keymap overrides: verification shots

Captured from the real window with `UTOPIA_SHOT_SCRIPT` against a scratch graph
(`xtask seed-refs`) and a scratch config dir, so no real notes or keymap were touched.

`keybindings-invalid-and-clash.png` — `keymap.json` holds `nonsense-key` for Today, Quick
Jump on `cmd-v` (Paste's chord in the editor) and a plain rebinding. Today's chip shows the
text the user wrote in amber, the row says why it cannot be used and that ⇧⌘T is in force;
Quick Jump's row carries the "Also used by Paste" line although the two actions declare
different context strings (whole window vs. `BlockEditor || LineInput`).

`shortcuts-keys-in-force.png` — the same config, Shortcuts tab: the cheat sheet shows the
keys actually bound, so Today is ⇧⌘T (not `nonsense-key`) while Quick Jump is ⌘V.

`keybindings-unsaved.png` — `keymap.json` is read-only. Rebinding Daily Note works for the
session ("Daily Note is now ⌥⌘L.") and the failed write is a state of its own in the footer:
"Could not save keymap.json: Permission denied (os error 13). These keys work until you quit."
