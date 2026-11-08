# Settings key capture: verification shots

Captured from the real window with `UTOPIA_SHOT_SCRIPT`, against a throwaway graph
(`/tmp/skc/g.utopia`) and a scratch config dir whose `keymap.json` is the unparseable string
`{ not json`, so the startup warning is on screen. No real graph, keymap or settings file was
touched.

`warning-on-open.png` — Settings → Keybindings on opening: the unreadable `keymap.json` is
reported in the amber notice line (its own line now, `settings-warning`), below the key list.

`capture-listening.png` — Today's chip clicked: it reads "Press keys…" and the notice line is
still there. Before the fix the click cleared the single status slot, and with it the only
sign that what is in force is not what is on disk (machine doc, issue 4).

`escape-cancelled.png` — `shift-escape` pressed: the recording stops with "Left unchanged." and
Today keeps ⇧⌘T, with the notice still below it. Before the fix a modified Escape was accepted
as a chord, so Today would have been bound to `shift-escape` (issue 3).

Not captured: ending a recording by leaving the window (issue 2). A scripted run floats its
windows (`orderFrontRegardless`, no activation), gpui only reports focus changes while a window
is active, and the script has no step that activates one. The behaviour is covered by
`clicking_into_the_filter_ends_the_capture` and `leaving_the_window_ends_the_capture` in
`crates/utopia-ui/tests/settings.rs`, which drive the same handlers on gpui's test platform
(where the test activates the window first).
