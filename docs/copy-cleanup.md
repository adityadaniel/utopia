# Copy cleanup

An audit of the interface copy with the `clean-copy` ladder. Each string is read next to
what is on screen around it: the label, the value, the button. A string is cut when one of
these rungs fires:

1. **Surface** — the user would never have asked the question the string answers.
2. **Label** — the label, the field or the value beside it already says it.
3. **Pre-empt** — the string rules out a failure, defends a design decision or hedges about
   something that works. The fix for a fixed bug is the fix, not a caption about it.
4. **Rewrite** — remove the offending part. If nothing is left, delete the element.

Copy survives when the user cannot act without it: a real consequence at the moment of
choosing, a scope or compatibility fact they cannot see, what a blank state means and what to
do next, or the unit on a bare value.

Lines refer to `main` at `dec731f`. Branch: `chore/clean-copy`.

## Settings window

| Where | Current | Change | Rung |
|---|---|---|---|
| `settings.rs:93-113` tab summaries, one per tab under the pane title | Appearance: "The font your notes and the interface are set in (code stays in Menlo), and how tightly blocks are set." Keybindings: "Every action in the app and the keys it answers to. Click a key to change it." Shortcuts: "Everything bound right now, as a cheat sheet." Sync: "Where this graph syncs to: choose a transport, enter its credentials and connect another device." Help: "A short tour of what the app can do, with pictures of the real thing." | **cut** the summary line and the `summary()` method. The "Font" and "Density" cards, the key rows with their own hint, the shortcut list, the Sync form and the tour chips are directly beneath and say it. | 2 |
| `settings.rs:1089` Keybindings hint | "Click a key, then press the combination you want. Esc keeps the one you had." | "Click a key, then press the new one." The Escape fact belongs in the capturing state: the chip that reads "Press keys…" now reads "Press keys · Esc cancels". | 4 (rare state) |
| `settings.rs:1475-1477` Density captions | "15 px body on 24 px lines, as before." / "13.5 px body and 4 px between blocks." | **cut** both captions and the caption element. "as before" is scar tissue from before the setting existed; the window itself reflows on click. | 3, 2 |
| `settings.rs:1496` References caption | "How many children a Linked Reference shows inline. 0 keeps them all behind its "+N more" chip." | "0 keeps every child behind the “+N more” chip." The first sentence restates the card title and the "Children shown" row label. The 0 fact survives: the value is ambiguous without it. | 2 |
| `settings.rs:1548` Pinned pages caption | "Pages pinned to the top of the sidebar (SB-3). Local to this machine, kept in settings.json, never synced." | **cut** the row. A PRD id leaked into the UI, and the rest pre-empts a worry about sync. The "N pinned pages" row and Clear button remain. | 3 |
| `settings.rs:1592` Right sidebar caption | "Width: 380 px. Drag the sidebar's left edge to resize it; the width is kept in settings.json and never synced." | "Width: 380 px, set by dragging the sidebar's left edge." The control is elsewhere, so the pointer stays; the storage clause goes. | 3 |
| `settings.rs:606,669,628` no-op selections | "The font is already X." / "The density is already X." / "References already show N children." | **cut**: clicking the selected radio says nothing. | 2 |
| `settings.rs:607,671,629,617,646` success statuses | "The font is now X." / "The density is now X." / "References now show N children." / "The right sidebar is back to 380 px." / "Unpinned every page." | **cut**: the radio, the count row and the width row show the result. The failure statuses ("X applies now, but could not save settings.json") stay: the user cannot see that. | 2 |
| `settings.rs:1730-1737` fallback Sync pane help | "Page changes are exchanged in the background; the sidebar of the main window shows the latest round." / "Utopia is local-only: nothing leaves this machine unless you start it with a sync target." | **cut** the caption. | 3 |
| `settings.rs:1743` fallback transport | "Local only (no relay, no shared folder)" | "Off" | 2 |
| `settings.rs:1745-1749` fallback encryption | "Nothing to encrypt: sync is off" / "End-to-end encrypted: the relay only sees ciphertext" / "Not encrypted: a relay can read page payloads" | Row shown only when sync is on. "End-to-end encrypted" / "Not encrypted: the relay can read your notes". | 2, 4 |
| `settings.rs:1798` fallback footer | "Open a graph to configure its sync transport and credentials." | "Open a graph to set up sync." | 4 |

## Settings → Sync form (`crates/utopia-ui/src/sync_setup.rs`)

| Where | Current | Change | Rung |
|---|---|---|---|
| `:56,61` placeholders | "Enter token · blank keeps saved token" / "Enter passphrase · blank keeps saved key" | "Blank keeps the saved token" / "Blank keeps the saved passphrase". "Enter token" restates the field label. | 2 |
| `:277` heading | "Sync this graph" | **cut**: the pane title already reads "Sync". | 2 |
| `:288` Graph id caption | "Names this graph on the relay or folder. Choose a different id for a copy of a graph; it cannot be changed once the graph has synced." | "Fixed once the graph has synced. A copy of a graph needs its own id." Two consequences at the moment of choosing; the definition goes. | 2 |
| `:302` transport fact when off | "Off · your notes stay on this Mac" | **cut**: the line renders only when a target is running; Off is the highlighted mode button beneath. | 2, 3 |
| `:311-313` encryption fact | "End-to-end encrypted: the relay or folder only holds ciphertext" / "Not encrypted: the relay or folder holds page payloads in the clear" | "End-to-end encrypted" / "Not encrypted: the relay or folder can read your notes". The consequence of the unencrypted case stays, in plain words. | 4 |
| `:342` Off mode line | "Apply to stop sync. Local edits and queued changes are kept." | **cut**. Apply is the button beneath; the second sentence hedges. | 2, 3 |
| `:350` Nearby line | "Discover a Mac hosting this graph over mDNS. For Tailscale, choose Relay on the other Mac and enter the host's Tailscale address." | "Finds a Mac on this network that hosts this graph. Over Tailscale, choose Relay and enter the host's address instead." The compatibility fact stays. | 4 |
| `:355` hosting caption | "Hosting starts when you Apply and needs a relay token; enter the same token on the other device." | **cut**. Apply is the button, the token field is beneath, Apply refuses without one, and the pairing panel says to enter the same token. | 2 |
| `:371` Folder caption | "Use a dedicated folder for this graph, shared by your file sync service." | "Give the graph a folder of its own." The scope constraint stays; "shared by your file sync service" restates the mode. | 2 |
| `:380` secrets caption | "Secrets are kept only in macOS Keychain. Blank fields keep saved credentials. A saved passphrase cannot be changed; leave it blank for unencrypted sync only if none has been saved." | "A passphrase cannot be changed once it is saved." Keychain is a pre-empt, blank-keeps is in the placeholders, the last clause is a permission slip. | 3, 2 |
| `:432` pairing panel | "Enter the relay token and the same E2E passphrase yourself on the other device. They are never displayed here." | "Enter the same relay token and passphrase on the other device." | 3 |

## Sync results (`crates/utopia-app/src/sync_setup.rs`)

| Where | Current | Change | Rung |
|---|---|---|---|
| `:160` | "Could not initialize sync bookkeeping. Local editing is still available." | "Could not set up sync for this graph." | 3 |
| `:214` | "Sync is Off. Your notes and queued edits are kept." | "Sync is Off." | 3 |
| `:224` | "Hosting requires a relay token. Enter one, then Apply." | "Hosting needs a relay token." | 2 |
| `:275` | "Sync is on. The sidebar shows connection status." | "Sync is on." | 1 |
| `:314` | "Sync is Off: could not queue this graph for the new destination. Local data is unchanged." | "Sync is Off: could not queue this graph for the new destination." | 3 |
| `:332` | "Sync is Off: the runner could not start. Local editing is still available." | "Sync is Off: it could not start." | 3 |
| `:424` | "The E2E passphrase differs from this graph's saved key. Re-keying is not supported; use the original passphrase." | "This passphrase is not the one the graph was first synced with. Use the original passphrase." The unit test that matched "Re-keying is not supported" now matches "original passphrase". | 3 |

## Main window

| Where | Current | Change | Rung |
|---|---|---|---|
| `workspace.rs:2350` sync popover, off | "Off — this graph stays on this machine." | "Off." | 3 |
| `workspace.rs:2356` sync popover, no target | "No relay or folder configured." | **cut**: the target line only renders when there is one. | 2 |
| `workspace.rs:2384` sync popover, off | "Set UTOPIA_SYNC_URL or UTOPIA_SYNC_FOLDER, then reopen the graph, to sync." | "Set it up in Settings → Sync." Scar tissue from before Settings → Sync existed. | 3 |
| `workspace.rs:2846` right sidebar Pinned tab | "No pinned pages yet. Pin the page you are reading to keep it one click away." | "No pinned pages yet." The "Pin this page" button is directly above. | 2 |
| `graph_view.rs:846` | "No links yet. Type [[ to link pages and they appear here." | "No links yet. Type [[ in a block to link pages." | 2 |
| `main.rs:549` onboarding | "Name your graph. It is a single file you own." | **cut**: the input placeholder reads "Name your graph"; the "Creates …/<name>.utopia" line stays. | 2, 3 |
| `main.rs:587` unlock | "{file} is encrypted at rest. Enter its passphrase." | "{file} is encrypted at rest." The input placeholder reads "Passphrase". | 2 |
| `main.rs:602` unlock | "Tip: store it in the keychain (service utopia-db) to unlock automatically." | "A passphrase saved in the keychain (service utopia-db) unlocks it without asking." A scope fact the screen cannot show otherwise; the "Tip:" prefix goes. | 4 |

## Action help (`crates/utopia-ui/src/actions.rs`, shown under each Keybindings row)

| Action | Current | Change | Rung |
|---|---|---|---|
| Copy | "Copy the selection." | **cut** (the row hides an empty help line) | 2 |
| Cut | "Cut the selection." | **cut** | 2 |
| Redo | "Redo the change you just undid." | **cut** | 2 |
| Open Settings | "Open Settings, where you can change these keybindings." | **cut**: read from inside Settings. | 2 |
| Undo | "Undo the last change. A rename that merged two pages undoes as one step." | "Undo the last change." | 3 |
| Delete Block | "Send this block to the trash. Its id survives, so references still resolve." | "Send this block to the trash." | 3 |
| Kill line | "Delete to the end of the paragraph and keep the text for Ctrl+Y. Never merges blocks." | "Delete to the end of the paragraph and keep the text for Ctrl+Y." | 3 |
| Quit | "Quit Utopia. Edits are already saved." | "Quit Utopia." | 3 |

## Kept on purpose

- `main.rs:561` "Creates …/<name>.utopia": where the file goes, which the user cannot see.
- `sync_setup.rs:321` environment override notice: Apply is inert and the user needs the why.
- `workspace.rs:2151` "Type [[ or # to make a page." and `:3355` "Shift-click a link to open it here.": blank states with a next step.
- `page_view.rs` "Click to start writing", "No earlier notes.", "This page no longer exists.": blank and terminal states.
- `pdf_view.rs:543-545` the two reasons a PDF has no pages: the user cannot act without them.
- The Help tab bodies (`help.rs`): a tour is content, not a caption.
- Rebind statuses ("X is now ⌘⇧Y", "X is back to …"): a key capture is a modal gesture with no other confirmation.

## Also in this branch

- `settings.rs` search subscription: a `collapsible_match` lint from stable clippy 1.95 that
  predates this branch; the arm now carries the guard. Without it `make check` fails.
- `tests/settings.rs` `pinned_pages_are_local_and_cleared_from_settings` asserted the
  "Unpinned every page." status as its proof of the clear. A clear that saved is now silent,
  so the test asserts that silence; the `pinned` list and `settings.json` assertions above it
  still prove the clear.
- Keybinding rows skip the help line when an action's help is empty, so a cut help string
  leaves no blank caption.
- Screenshots: `docs/verification/chore-clean-copy/`.
