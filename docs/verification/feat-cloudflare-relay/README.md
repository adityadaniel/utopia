# Verification: feat-cloudflare-relay

These are Settings → Sync "Live updates" shots (§12.3), taken from the real app with
`UTOPIA_SHOT_SCRIPT` in the 1100 × 760 Settings window. Each shot used a scratch graph,
a scratch `UTOPIA_CONFIG_DIR` and `UTOPIA_SYNC_KEYCHAIN_PREFIX=scratch-`. The 1100 px
window is where the first version of the card overflowed its row.

| Shot | Proves |
|---|---|
| `live-updates-light-normal.png` | Light theme, System font, Normal density (the defaults). The title, the two-line caption and the choices sit inside the row with its padding: the title starts below the header divider, and the choices end above the bottom border. The copy is plain words, with no "WebSocket", "socket", "polling" or em dash. |
| `live-updates-dark-normal.png` | The same card in the dark theme (`UTOPIA_APPEARANCE=dark`). |
| `live-updates-light-relaxed-plex.png` | IBM Plex Sans at Relaxed density (26 px lines). The taller lines still fit in the row with its padding. |
| `live-updates-dark-tight-geist.png` | Geist at Tight density (22 px lines) in the dark theme, with "Only while I'm using Utopia" chosen (`"sync_realtime": "while_active"` in the scratch `settings.json`). The selected state shows on the second choice. |

The geometry is also pinned headlessly for every font, every density and both window
widths (900 and 1100 px) by `live_updates_text_keeps_its_padding` in
`crates/utopia-ui/tests/layout.rs`.

These shots do not show the `while_active` behaviour itself: keystrokes keeping the live
connection open, and the connection closing after two quiet minutes or when the app goes
to the background. The workspace tests cover that (`while_active_*` and
`always_ignores_keystrokes` in `crates/utopia-ui/tests/workspace.rs`). It was not tried
against the real macOS window server, because that would mean activating other
applications on the machine in use.
