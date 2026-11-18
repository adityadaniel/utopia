# PRD: Utopia for iPhone — adding blocks by touch

| Field | Value |
|---|---|
| Document version | 0.1, draft for review |
| Date | 2026-11-18 |
| Owner | Aditya Daniel |
| Source of behaviour | `~/Developer/utopia/PRD.md` (§2.3, §4, §5, §6, §10, §12, §14), `DESIGN.md`, `docs/sync-modes.md`, `mobile/ios/UtopiaMobile`, `crates/utopia-ffi` at `main` `bc7f6fd` |

The existing PRD stays the spec for the data model, the resolver, sync and every `FR-`/`AC-`
id. This document covers the iPhone app only. New ids start with `IO-`.

**Answers from the owner (2026-11-18).**

| # | Answer |
|---|---|
| K1 | The phone's main job is capture. Reading and light edits come second. Heavy restructuring stays on the Mac. |
| K2 | The desktop outliner is wrong on a phone in four ways: nesting by Tab and Shift+Tab (the worst), typing `[[` and its autocomplete, small targets, too much on screen. |
| K3 | Besides typing in the app, the first release must start a capture from a widget or the lock screen. |
| K4 | It is not a copy of the desktop outliner. The data model is the same as desktop. |

## 0. What exists today

Read from the repo. I did not run the shell.

- `mobile/ios/UtopiaMobile` is about 450 lines of SwiftUI: today's page as a flat read-only
  list, a one-line capture field at the bottom, `[[` title suggestions, a page screen with
  Linked References, and a sync sheet (relay URL, graph id, token, passphrase).
- It does not copy the desktop outliner. It is closer to a capture bar. But it stops at one
  level: every capture is a top-level block on today's page.
- It has not run on an iPhone or a simulator (`AGENTS.md`, Known limits).
- The mobile core (`utopia-ffi`) has nine calls. A block can only be appended at the top
  level of a page. There is no call to add under a parent, indent, outdent, move, delete,
  make a to-do, tick a to-do, undo, search block text, or read past days.
- Two things in the shell break the existing rules and must not carry over:
  - It finds links and tags by parsing block text in Swift (`linkedTitles`). The existing
    PRD §14.4 says the core alone decides what is a link.
  - `[[` suggestions need the user to type `[[` first, which is the hassle named in K2.
- To check: the core's `today()` comment says the day's page is "created on first open".
  FR-1.2 says a day becomes a page on its first write, never on a visit.

## 1. Problem

A thought arrives when the owner is away from the Mac. Today they cannot put it into Utopia
at all, so it goes into another app and is copied later, or is lost.

The desktop way of adding blocks does not move to a phone:

- **Nesting is done with Tab and Shift+Tab.** A phone keyboard has neither.
- **Links are typed as `[[Title]]`.** On a phone keyboard the brackets are two layers down,
  and the autocomplete covers the text.
- **Structure is fixed by moving text.** Small bullets and drag handles are hard to hit with
  a thumb.
- **The screen shows everything.** Sidebars, references and panes are too much for one line.

## 2. Users

- **Primary: the owner, away from the Mac,** one hand on the phone, 5 to 30 seconds to spare.
  They want the thought in today's note, linked, and to see it on the Mac later.
- **Secondary: the same person, reading.** In a meeting or on a train they open a person or
  project page and read what links to it.
- **Not served in v1:** a person who uses only the phone and wants full outline editing;
  iPad users; Android users.

## 3. Goal and success metrics

**Goal.** A thought goes from the lock screen into today's note, with its links, its place
in the outline and its to-do state, without one bracket, Tab or drag.

| # | Metric | Target |
|---|---|---|
| G1 | Lock screen or widget tap to first character typed | ≤ 2 s on a warm app, ≤ 4 s cold [ASSUMPTION] |
| G2 | Characters `[`, `]`, `#`, `(` the user must type to make a block with two page links and a tag | 0 |
| G3 | Taps to put the next block one level deeper, or one level shallower | 1 |
| G4 | Captures while offline that reach the Mac after the next sync | 100%, no duplicates |
| G5 | Smallest touch target for any block action | 44 × 44 points |
| G6 | The owner's own use | 14 days with no capture made in another notes app [ASSUMPTION] |
| G7 | Data safety | After a scripted session on the phone, `utopia doctor` on the synced Mac graph reports zero problems |

## 4. Design directions

Three ways to add a block. Each keeps the same data: blocks in a tree, pages, links.
These describe behaviour. Screens and visual style belong to a designer.

### A. Composer with a place marker (recommended)

One composer sits at the bottom of the screen, above the keyboard. The outline above it is
for reading. You never edit structure by moving text.

- **Adding.** Type, press Send. The text becomes one block. The composer stays open and
  empty for the next one.
- **Nesting.** The composer shows where the next block will go, for example
  "Today › Meeting with Dana". Two large buttons move that place: **Deeper** puts the next
  block under the one you just sent; **Shallower** moves the place up one level. Tapping
  any block and choosing "Add under this" moves the place there. You change where you are
  writing, not the indent of something already written.
- **Linking.** As you type, words that match an existing page title appear in a row above
  the keyboard. One tap turns the word into a link. A **Link** button opens a page picker
  (recent pages, search, "Create") for anything else. Typing `[[` or `#` still opens the
  same picker, for people who want it.
- **To-do, tag, date.** One button each in the same row.

*Good:* answers every pain in K2; one thumb; the existing shell is already half of it; the
core needs few new calls. *Weak:* reshaping an outline already written is indirect; it adds
a new idea (the place marker) that the desktop does not have.

### B. Touch outliner

The outline is the editing surface, as on desktop. Tap a block to put the caret in it.
A row above the keyboard has Indent, Outdent, Move up, Move down, Link, To-do. Swiping a
row right or left indents or outdents it. Long-press drags it.

*Good:* full editing power; no new idea for a desktop user. *Weak:* it is the desktop
outliner with buttons for the missing keys, which K4 rules out; dense; many small targets;
the hardest text-editing work; capture is no faster than today.

### C. Capture now, arrange later

Every capture is a flat card in today's note. Nothing is nested or linked while capturing.
A separate "Arrange" step later lets you drop one card onto another to nest it, and tap
suggested links to accept them.

*Good:* the fastest capture; structure is done when there is time. *Weak:* it makes a
sorting chore, and the existing PRD's principle 2 and 3 say no filing and no cleanup debt;
links are added late or never, so backlinks are poor; most arranging would wait for the
Mac anyway.

### Comparison

| | A. Composer | B. Touch outliner | C. Arrange later |
|---|---|---|---|
| No Tab or drag to nest | ✓ | — | ✓ |
| No brackets to link | ✓ | — | ✓ |
| Links made at capture time | ✓ | ✓ | — |
| One-thumb use | ✓ | — | ✓ |
| Reshape an existing outline | — | ✓ | ✓ |
| No cleanup step later | ✓ | ✓ | — |
| Smallest core and build work | ✓ | — | — |

**Recommendation: A.** Take two things from B for light edits on blocks already written:
swipe right or left on a row to indent or outdent, and long-press to reorder among
siblings. The owner decides.

The rest of this document specifies direction A.

### Desktop rules that rely on a keyboard

| Desktop rule | On the phone |
|---|---|
| Enter makes a sibling block | Send makes the block. Return on the keyboard inserts a line break inside the block [ASSUMPTION; Q4]. |
| Tab and Shift+Tab indent and outdent | Deeper and Shallower move the place marker. Swipe right or left on an existing row. |
| Backspace at the start merges into the previous block | Not in v1. Edit or delete the block instead. |
| Cmd+Shift+Up and Down move a block | Long-press and drag among siblings. |
| `[[`, `#` open autocomplete | Tap-to-link suggestions and the Link button. Typing them still works. |
| `((` inserts a block reference | Out of v1. Existing ones are shown. |
| Alt+Enter cycles to-do state | A To-do button in the composer; a checkbox on the row. |
| `/` slash menu | Out of v1. To-do is the only block type you can make. |
| Cmd+K quick jump, Cmd+Shift+F search | One search field: pages first, then blocks. |
| Shift-click opens beside; panes | One page at a time, with Back. |
| Cmd+Z | Shake or an Undo button after each action [ASSUMPTION]. |
| Click a bullet to zoom | Out of v1. |
| Cmd+. collapse | Tap the collapse control on a row with children. Local to the device, never synced. |

## 5. Scope

**In (v1):**

- iPhone, portrait. One graph.
- Today screen: today's note and the composer. Scroll up for earlier days, read-only until
  tapped.
- Composer with place marker, Deeper, Shallower, Link, To-do, Date.
- Tap-to-link suggestions from existing page titles.
- Page picker: recent pages, search by title or alias, create a page.
- Edit the text of an existing block. Delete a block, with Undo. Tick a to-do.
- Swipe to indent or outdent a block. Long-press to reorder among siblings.
- Page screen: outline, properties shown as rows, Linked References. Tap a link to open its
  page.
- Search: page titles and block text.
- Capture from outside the app: a lock screen widget, a home screen widget, and a control
  for Control Center and the Action button. Each opens the composer with the keyboard up.
- Sync through a relay, with end-to-end encryption, token and passphrase in the Keychain.
- First run: start a new graph, or connect to an existing one by relay URL, graph id, token
  and passphrase.
- Blocks the phone cannot make are still shown: headings, quotes, code, numbered blocks,
  block references, embeds, attachments, queries (as a labelled placeholder).

**Out (v1):**

- Share sheet, voice capture, photo or scan capture. The owner did not choose them for v1.
  The system keyboard's own dictation works in the composer with no work from us.
- Rename, merge, aliases, delete page, trash, templates, extract to page, slash menu.
- Inserting block references and embeds. Making headings, quotes, code or numbered blocks.
- Moving a block to another page. Drag to a different parent (use swipe).
- Unlinked References, reference filters, the reference inspector.
- Tasks view, graph view, PDF viewer, plugins, query editing, automation API, MCP.
- Nearby Mac and shared-folder sync. `docs/sync-modes.md` says the mobile core syncs over
  HTTP only.
- iPad layout, landscape, hardware keyboard shortcuts, Android.
- Encryption at rest setup on the phone. A graph already encrypted can be unlocked.
- Any AI feature (existing PRD §2.2).

## 6. User stories and acceptance criteria

All checks run on a scratch graph.

**IO-1 Capture one line.** As the owner, I type a thought and it is in today's note.
- AC-1.1: Open the app. The composer has focus and the keyboard is up. No tap is needed.
- AC-1.2: Type `Call the bank` and press Send. The block appears as the last top-level
  block of today's note. The composer is empty and still has focus.
- AC-1.3: On a day with no page, opening the app creates no page. The first Send creates
  it with exactly one block (FR-1.2, AC-1.4).
- AC-1.4: Send with only spaces does nothing.
- AC-1.5: Type half a line, leave the app, kill it, reopen. The half line is in the
  composer.

**IO-2 Capture from the lock screen or a widget.**
- AC-2.1: Tap the lock screen widget and unlock. The composer is open on today with the
  keyboard up. Time from tap to first character meets G1.
- AC-2.2: The home screen widget, the Control Center control and the Action button do the
  same.
- AC-2.3: If the app was last left on another page, the capture still goes to today.
- AC-2.4: With no graph set up, the tap opens first-run setup, and nothing is written.

**IO-3 Nest without Tab.**
- AC-3.1: Send `Meeting with Dana`. Tap Deeper. The place marker reads
  "Today › Meeting with Dana". Send `Budget is approved`. It is a child of the first block.
- AC-3.2: Send two more lines. Both are siblings of `Budget is approved`, in the order
  sent.
- AC-3.3: Tap Shallower. The marker reads "Today". The next line is a top-level block
  placed after `Meeting with Dana`.
- AC-3.4: Deeper is disabled until a block has been sent at the current place. Shallower
  is disabled at the top level.
- AC-3.5: Tap any block, choose "Add under this". The marker shows that block. The next
  line is its last child.
- AC-3.6: The marker shows at most the last two levels of the path and never pushes the
  text field off screen.
- AC-3.7: At depth 12 Deeper is disabled (existing PRD §6.1).
- AC-3.8: After leaving the app for more than 5 minutes, the marker is back at "Today"
  [ASSUMPTION; Q5].

**IO-4 Link without brackets.**
- AC-4.1: With a page "Dana" in the graph, type `Lunch with dana`. A suggestion "Dana"
  appears above the keyboard. Tap it. The word becomes a link, shown in the link colour.
  The stored text is `Lunch with [[Dana]]`.
- AC-4.2: Send it. On the Mac after sync, the block is under Linked References on "Dana".
- AC-4.3: A two-word title ("Atlas Launch") is suggested when both words are typed, and
  one tap links the whole phrase.
- AC-4.4: Suggestions never change the text unless tapped.
- AC-4.5: Tap Link with no text selected. The picker shows recent pages. Type `atl`,
  tap "Atlas Launch". The link is inserted at the caret.
- AC-4.6: In the picker, type a title that does not exist and tap Create. The link is
  inserted. The page exists after Send, and not before.
- AC-4.7: Typing `[[` or `#` opens the same picker.
- AC-4.8: Which words are links, and which page each one names, is the same on the phone
  and the Mac for the same text. Check with the shared golden file (existing PRD §10.4
  item 11).
- AC-4.9: The Date button offers Today, Tomorrow and a date picker, and inserts a link to
  that day. On that day's page on the Mac, the block is under "Scheduled for this day".

**IO-5 To-dos.**
- AC-5.1: Tap To-do, type `Pay rent`, Send. The block is a to-do with an empty checkbox.
  The To-do button turns off again after Send [ASSUMPTION].
- AC-5.2: Tap the checkbox. The to-do is done, shown struck through. On the Mac after
  sync it is ticked.
- AC-5.3: The checkbox target is at least 44 × 44 points.

**IO-6 Fix a block already written.**
- AC-6.1: Tap a block's text. Its text loads into an editor with the keyboard up. Change
  it and tap Done. The block shows the new text. Its children are unchanged.
- AC-6.2: Swipe a block right. It becomes the last child of the block above it, with its
  own children. Swipe left: it becomes the next sibling of its parent.
- AC-6.3: Swipe right on a first child does nothing and the row gives a short shake.
- AC-6.4: Long-press a block and drag. It moves among its siblings with its children.
  Dropping outside its siblings puts it back.
- AC-6.5: Delete a block from its menu. It and its children vanish. An Undo button shows
  for 5 seconds and restores all of them.
- AC-6.6: Each action above is one undo step and syncs as the same change a Mac edit
  would make.

**IO-7 Read a page and its references.**
- AC-7.1: Tap a link in any block. The page opens with its outline, then Linked
  References grouped by source page with a count.
- AC-7.2: Tap a reference. The source page opens with that block in view.
- AC-7.3: Leading `key:: value` blocks show once, as rows above the outline (DESIGN §3.8).
- AC-7.4: The composer on a page screen adds to that page, and the marker starts with the
  page's title.
- AC-7.5: A block type the phone cannot make is shown readable, never as raw markup with
  missing text. A query block shows "Query — open on Mac".

**IO-8 Find.**
- AC-8.1: One search field. Typing `atl` lists matching pages first, then blocks that
  contain the word, each with its page title.
- AC-8.2: With an empty field it lists Today, Yesterday and the last 8 pages opened.
- AC-8.3: A title with no match offers "Create page".

**IO-9 Sync.**
- AC-9.1: In airplane mode, send three blocks. Each appears at once. The status shows
  "Offline — 3 changes queued".
- AC-9.2: Turn the network on and open the app. Within 10 s the status shows synced, and
  the Mac shows the three blocks, once each [ASSUMPTION: 10 s].
- AC-9.3: A sync round starts when the app opens, after each Send, and when the app
  leaves the screen.
- AC-9.4: A wrong token shows "sign-in required", not "Offline".
- AC-9.5: The phone and the Mac both add a line to today's note offline. After both sync,
  both show both lines.
- AC-9.6: Typing in the composer never waits for a sync round.

**IO-10 First run.**
- AC-10.1: "Start a new graph" opens today's note with the composer in under 3 taps.
- AC-10.2: "Connect to my graph" asks for relay URL, graph id, token and passphrase. After
  the first sync, the Mac's pages are on the phone.
- AC-10.3: A wrong passphrase is refused with a message and writes nothing to the relay.
- AC-10.4: The token and passphrase fields are secure fields. Neither value is shown again.

## 7. Edge cases

- **Midnight while composing.** A line started at 23:59 and sent at 00:01 goes to the day
  shown in the marker. The marker then offers the new day.
- **The marker's block is deleted or moved by sync.** The marker falls back to that
  block's page, and says so. The typed text is kept.
- **Today's note changes by sync while the composer is open.** New blocks appear. The
  typed text, the caret and the marker do not move.
- **Paste of many lines.** The paste stays one block with line breaks, and a button offers
  "Split into blocks", one per line, keeping indentation as nesting [ASSUMPTION].
- **A suggestion matches a common word** (a page titled "On"). Titles of one or two
  letters are not suggested [ASSUMPTION]. The Link button still finds them.
- **A typed date that is ambiguous** (`11/6/26`). Never linked as a date (existing PRD
  §5.5).
- **A new title that differs from an existing one only by case or spacing.** It links to
  the existing page. No second page is made.
- **Widget tapped while the graph is locked** (encrypted at rest). The unlock screen
  shows first, then the composer. No text is typed into the unlock field by mistake.
- **App killed mid-Send.** After reopen, the block is either fully there or the text is
  back in the composer. Never both, never neither.
- **A page with 5,000 blocks.** It opens and scrolls. Only visible rows are drawn.
- **A very long block** (2,000 characters). The composer grows to 6 lines, then scrolls.
- **The system keyboard's dictation or autocorrect** changes a word inside a link. The
  link is kept if the title still matches; otherwise it becomes plain text, and the user
  sees that.
- **A swipe on a row inside a scrolling list.** A vertical scroll never indents. An
  indent needs a clear sideways move.
- **Low Power Mode or no background time.** Sync waits for the next open. The queued
  count is shown.
- **Relay speaks a newer protocol.** The app says to update. Captures stay queued.
- **Text size set to the largest accessibility size.** The composer buttons stay
  reachable and at least 44 points.
- **VoiceOver.** Each composer button has a spoken label. The marker is read as
  "Adding under …".

## 8. Constraints for the builder

These limit the build. They do not choose its design.

- C1. One implementation of the frozen contract. The core decides what is a link, tag or
  date, which page a title names, and which titles to suggest. The app does no parsing of
  its own (existing PRD §14.4). The present shell's `linkedTitles` breaks this.
- C2. Every write is a core command, through the same path as the desktop editor, so
  undo, references and sync behave the same (rule 13).
- C3. The mobile core needs new calls before this app can be built: add a block under a
  parent or after a sibling, indent, outdent, move, delete, set to-do and tick, undo,
  days before today, block search, recent pages, title suggestions for a text, and the
  parsed parts of a block for display. That work is in the Rust repo.
- C4. A capture started from a widget or control writes to the same graph, with one
  writer at a time.
- C5. No blocking work on the main thread. Sync is off the main thread.
- C6. Secrets in the Keychain. No telemetry by default. No model and no call to a model.
- C7. Place marker, collapse state, recents and drafts are local to the phone and never
  sync (rule 9).
- C8. `DESIGN.md` is a desktop spec. Its colours and type roles can carry over; its
  layout cannot. A designer owns the phone screens.

## 9. Open questions

1. **Direction.** A, B or C? *Recommendation:* A, with swipe and reorder from B.
2. **Does the phone hold the whole graph, or only recent days and pages?** The whole graph
   makes search and backlinks complete, and costs storage and first-sync time.
   *Recommendation:* the whole graph.
3. **Where does a capture from the widget go?** Always today, or the last place used?
   *Recommendation:* always today.
4. **Return key.** Does Return send the block, or add a line break with a separate Send
   button? *Recommendation:* line break, with a Send button, so a thumb slip never sends.
5. **How long does the place marker last?** Until you change it, or back to "Today" after
   a pause? *Recommendation:* back to "Today" after 5 minutes away.
6. **Tasks on the phone.** A list of open to-dos with tick boxes is small and useful away
   from the Mac. In v1, or after?
7. **A home screen widget that shows content** (today's open to-dos or last lines), or
   only a button that starts a capture?
8. **Oldest iOS version.** Lock screen widgets, the Control Center control and the Action
   button each need a minimum version and a recent iPhone. Which is the floor?
9. **Distribution.** TestFlight for the owner only, or the App Store? The App Store needs
   a privacy policy and review.
10. **Does the Mac need a matching change?** For example, a mark on blocks that came from
    the phone. *Recommendation:* no.
11. **Order against the Swift and AppKit Mac build** (`~/Developer/utopia-swift/PRD.md`).
    Both need a larger binding layer. Which comes first?

## 10. Assumptions

- A1. The owner already runs, or will run, a relay. Without one the phone and the Mac
  cannot share a graph in v1. [ASSUMPTION]
- A2. iPhone only, portrait only. [ASSUMPTION]
- A3. One graph on the phone. [ASSUMPTION]
- A4. The targets in G1, G6, AC-3.8, AC-6.5 and AC-9.2 are placeholders. [ASSUMPTION]
- A5. Share sheet, voice and photo capture come after v1 because the owner chose only the
  widget and lock screen. [ASSUMPTION]
- A6. The To-do button applies to one block, then turns off. [ASSUMPTION]
- A7. Undo is a button shown after an action, plus shake. [ASSUMPTION]
- A8. The existing shell is a starting point to replace, not to extend as is. [ASSUMPTION]
- A9. Attachments made on the Mac show as a name and type on the phone; images show a
  preview once downloaded. [ASSUMPTION]

## 11. What I checked, and what I did not

- Read: `mobile/ios/UtopiaMobile` (all three Swift files), the public surface of
  `crates/utopia-ffi/src/lib.rs`, and earlier in this work the full `PRD.md`, `AGENTS.md`,
  `DESIGN.md`, `README.md`, `docs/redesign-ideas.md` and the summary of
  `docs/sync-modes.md`.
- Not done: I did not build or run the shell or the core. I did not check which iOS
  version each widget and control type needs. I did not check `today()` against FR-1.2
  beyond its comment. No file in the utopia repo was changed for this document.
