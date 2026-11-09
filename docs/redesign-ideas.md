# Redesign ideas, per screen

Status: proposal, reviewed on 2026-11-06. No code has changed.

Source: the screenshots in `docs/images/` and the right-sidebar capture in
`docs/unslop-audit/d-right.jpg`, taken on the Demo graph.

Mockups (HTML, private artifacts):

- Daily Notes feed and left sidebar: https://claude.ai/artifact/KsFdZCRPugbnokJEpXo1mV
- All other screens: https://claude.ai/artifact/DpYay978djndhb9tDFa87j
- The idea catalogue with the current screenshots: https://claude.ai/artifact/2wKUEtgJgQsTDRXnjRMDVp

Every idea is a view change only. None touches the CRDT shards, page identity or
the resolver. Where an idea meets a rule from `AGENTS.md`, the rule is named.
Effort: **S** a day or less, **M** a few days, **L** a new component or a layout change.

Review decisions so far are marked **Decided** or **Dropped**.

## 00 · Across all screens

What the captures show: one font size for almost everything; links are blue and
tags are purple and no other colour exists; hover, focus and selected states are
mostly absent; controls are plain text ("Delete page", "Any time", "Close").

| Id | Idea | Effort |
|---|---|---|
| ALL-1 | **One type scale.** Title 28, H1 20, H2 17, body 15, caption 12.5. Day headings and page titles share the title size. Outline headings never pass H1. | S |
| ALL-2 | **A named colour token set.** Text, muted text, line, surface, accent, tag, danger, in light and dark. Every component reads a token. A dark theme becomes a one-file change. | M |
| ALL-3 | **Make controls look like controls.** Quiet button style: 6 px radius, 1 px line on hover, 13 px label. Menus get a chevron. "Delete page" becomes a danger item in a menu. | S |
| ALL-4 | **Hover and focus states.** Soft hover fill on sidebar rows, reference groups, palette results and settings rows. The focused block gets a 2 px accent bar at the left. | S |
| ALL-5 | **A density setting.** "Comfortable" and "Compact" in Settings → Appearance. Compact uses 13.5 px body and 4 px block spacing. Stored in `settings.json` (rule 9). | M |

**ALL-4 update (2026-11-09):** the 2 px focus bar at the left of the block being edited was removed. It had the caret's width, colour and height, so on short or empty blocks it read as a second caret; the caret alone marks the edited block. Hover fills stay.

## 01 · Left sidebar

What the capture shows: Recent Days shows two preview lines per day, so three
days fill half the sidebar; tags such as *atlas* sit between real pages; the
footer is two plain lines; "Calendar" is a disclosure row.

| Id | Idea | Effort |
|---|---|---|
| SB-1 | **Compact Recent Days.** One line per day with a muted block count. The first block shows as a tooltip on hover. Ten days fit where three did. | S |
| SB-2 | **Mark tag pages.** Pages that only exist because of a `#tag` get a small "#" glyph in the tag colour. One list, display only, no tags subsystem (FR-2.15). | S |
| SB-3 | **Pinned pages.** A "Pinned" group above Pages. Pin from a page's menu or by drag. Local UI state in `settings.json` (rule 9). | M |
| SB-4 | **Status footer.** One row: a graph chip with the file name, then a sync dot. Grey off, green in sync, amber syncing, red error. Click opens the sync view. The encryption lock sits next to it. | S |
| SB-5 | **Inline month strip.** A quiet 7-column month grid under Journal. Days with an entry get a dot; today is the accent colour. Collapse with one click on the month name. | M |

## 02 · Daily Notes feed

What the capture shows: every day has a 28 px heading, so empty days take as
much space as full days; "Click to start writing" is the only hint; a day's
Linked References sit between two days; no weekday; nothing says which day is
today.

| Id | Idea | Effort | Review |
|---|---|---|---|
| DN-1 | **Fold empty days.** An empty day is one 36 px row: weekday and date in the muted colour. The whole row is the click target; a faint "Click to start writing" hint shows on hover. Today and yesterday always stay open. A day becomes a page on the first write (FR-1.2), so nothing changes in the store. | M | **Decided.** No "+" icon at the right; clicking the row is enough. |
| DN-2 | **Weekday and relative label.** A 12 px caption above each date: "Thursday · Today", "Wednesday · Yesterday", "Monday". Today's caption is in the accent colour. Titles stay display only; identity is still `daily_date` (rule 6). | S | |
| DN-3 | **Sticky day header.** While a long day scrolls, its date stays at the top of the column with a hairline under it. A cheap element, no extra work in `render` (rule 10). | M | |
| DN-4 | Quiet references in the feed: fold a day's Linked References into one row. | S | **Dropped** for now. |
| DN-5 | Scroll rail with one dot per loaded day. | L | **Dropped** for now. |

## 03 · Page view

What the capture shows: attributes appear twice (metadata header and attribute
blocks); "+ alias" is a lone button; "Delete page" is loose text at the top
right; outline headings are almost as big as the title; a code block shows its
language as a plain line above the code.

| Id | Idea | Effort |
|---|---|---|
| PG-1 | **Show each attribute once.** Keep the metadata header. In the outline, render an attribute block as a compact "key · value" row in the muted colour. A "Show as blocks" toggle brings the full blocks back. The header is a projection; the blocks stay the source (rule 2). | M |
| PG-2 | **A page header toolbar.** Under the title: alias chips, "+ alias", then a "···" menu with Open in graph, Export, Rename and Delete page (danger, last). | S |
| PG-3 | **Tame the heading sizes.** Outline H1 20 px, H2 17 px, H3 15.5 px semibold. The title stays 28 px. | S |
| PG-4 | **Block affordances on hover.** The bullet becomes a drag handle, the collapse arrow fades in. To-do box gets 3 px corners; a done to-do is struck through in the muted colour. | M |
| PG-5 | **Quote and code styles.** A quote gets a 2 px accent bar and a warm grey italic. A code block gets a surface card in Menlo with 6 px corners and a language label at the top right. | S |

## 04 · Linked References

What the capture shows: a full-width filter with "Any time" and "Sort: title" as
plain text; co-reference chips alone on a line; the count at the far right edge;
a small lone "› Risks" breadcrumb; Unlinked References closed with no count.

| Id | Idea | Effort | Review |
|---|---|---|---|
| LR-1 | **One control row.** A 240 px filter field, then "Any time ▾" and "Title ▾" as menu buttons. Co-reference chips wrap on the next line with counts ("Ana Lovelace 3"). Selected chips fill with the soft accent. | S | |
| LR-2 | **Count next to the name.** The count sits right after the page or day name as a small chip, not at the far edge. | S | **Decided.** No icons in group headers; they look busy. |
| LR-3 | **A full path above the block.** "Atlas Launch › Risks" as one 12.5 px line, each part clickable. | S | |
| LR-4 | **Children inline, up to a limit.** A referencing block shows its first children inline. Past the limit, a "+N more" chip opens the rest. Counts come from the index, not paint-time work (rule 3). | M | **Decided.** The limit is a Settings option: Appearance → "Children shown in references", default 2; 0 folds all children behind the chip. |
| LR-5 | **Unlinked References with a count and "Link all".** Header "Unlinked References · 3". Open, each mention has a Link button and the matched text is marked. Empty state: "No unlinked mentions". | S | |

## 05 · Quick jump (⌘K)

What the capture shows: a flat list with no row types or icons; the create row
looks like a result; nothing when the field is empty; no key hints.

| Id | Idea | Effort |
|---|---|---|
| QJ-1 | **A useful empty state.** Today, Yesterday and the last eight pages opened. Local UI state (rule 9). | S |
| QJ-2 | **Row types with icons.** Page, tag page and day each get an icon and a type label. "Create page" is last, in the accent colour, above a hairline. | S |
| QJ-3 | **Key hints in a footer.** "↑↓ move · ↩ open · ⌘↩ open in split · esc". | S |
| QJ-4 | **Highlight the match.** The typed letters in semibold inside each result. | S |
| QJ-5 | **Commands in the same palette.** Type ">" to list actions from `actions::CATALOG` with their keys. | M |

## 06 · Search (⌘⇧F)

What the capture shows: the same shell as Quick jump; no term highlighting; no
grouping; no scope; no preview.

| Id | Idea | Effort | Review |
|---|---|---|---|
| SR-1 | **Mark the matched terms.** A soft accent background on each matched word. FTS5 already returns the offsets. | S | |
| SR-2 | **Group results by page.** A header per page or day with a block count, then its blocks indented. | M | **Decided.** On two-line group rows the icon aligns to the top line. |
| SR-3 | **Scope chips.** "All · Journal · Pages · To-dos" and "Any time ▾" under the field. The query evaluator already supports these filters. | M | |
| SR-4 | **A preview pane.** Widen the dialog to about 860 px. The right half shows the selected block with its path, children and state. | L | |
| SR-5 | **One palette, two modes.** Merge Search into Quick jump: pages first, then blocks. ⌘K opens on pages, ⌘⇧F on blocks. | L | |

## 07 · Graph view (⌘⇧G)

What the capture shows: equal 10 px dots; 1 px edges with no direction or
weight; no legend, no zoom control, no hover state; the graph replaces the page.

| Id | Idea | Effort |
|---|---|---|
| GR-1 | **Size nodes by links.** From 6 px to 18 px. The page you came from gets a 2 px accent ring. | S |
| GR-2 | **Hover focus.** Non-neighbours fade to 25 %. A tooltip shows the name, the link count and the first block. Click opens the page, ⌘-click opens it in a split. | M |
| GR-3 | **Legend and zoom controls.** Colour key and count at the bottom left; "−", "+" and "Fit" at the bottom right. The top bar keeps only the filters and Close. | S |
| GR-4 | **Edge weight and direction.** Width grows with the reference count, 1 px to 3 px. An arrowhead points at the target. Use stored counts (rule 3). | M |
| GR-5 | **Local graph in the right sidebar.** "Around this page" opens as a panel next to the page, which stays editable. "All pages" keeps the full-screen mode. | L |

## 08 · Settings (⌘,)

What the capture shows: top tabs; each row spans the full window with the key
chip and a "Change" button at the far right; no search; a clash is only shown
after recording.

| Id | Idea | Effort |
|---|---|---|
| ST-1 | **A readable width.** Content at 720 px, one card per category, hairlines between rows. | S |
| ST-2 | **The chip is the control.** Click the key chip to record; it shows "Press keys…" until you press. The "Change" button goes away. A changed row gets a small "↺". | M |
| ST-3 | **Clashes inline.** An amber line under the row: "Also used by Zoom Out", linked to the other row. | S |
| ST-4 | **Search actions.** A field at the top filters rows by name, description or key. | S |
| ST-5 | **Side tabs like System Settings.** A list at the left with an icon per tab: Appearance, Keybindings, Shortcuts, Sync, Help. | M |

## 09 · Right sidebar

What the capture shows: "REFERENCES" with Incoming, Outgoing, Unresolved tabs;
day names with counts and no block text; mostly empty; no resize or close.

| Id | Idea | Effort |
|---|---|---|
| RS-1 | **Show the blocks inline.** Each reference row shows its block, with the same renderer as Linked References. | S |
| RS-2 | **Unresolved rows you can act on.** Each gets a "Create page" button. Empty state: "No unresolved references". | S |
| RS-3 | **More than references.** Tabs: References, Outline (headings, click to jump), Pinned. The graph panel from GR-5 can join them. | M |
| RS-4 | **Resize, collapse, remember.** A 4 px drag handle on the left edge, a close "×", a shortcut to toggle. Width in `settings.json` (rule 9). | S |
| RS-5 | **Stacked panes.** Pages opened with ⌘⇧-click stack in the panel as cards with a collapse arrow and a close "×", like Roam's right sidebar. | L |

## Order of work (proposal)

1. ALL-1, ALL-3, ALL-4: the type scale and control styles, since every other screen builds on them.
2. Daily Notes feed: DN-1, DN-2, DN-3, with SB-1, SB-4, SB-5.
3. Page view and Linked References: PG-1 to PG-5, LR-1 to LR-5.
4. Palettes: QJ-1 to QJ-5, then SR-1 to SR-3.
5. Settings: ST-1 to ST-5.
6. Larger pieces last: SR-4, SR-5, GR-5, RS-5.

Each step is verified the way `AGENTS.md` describes: seed a graph, drive the
app with `UTOPIA_SHOT_SCRIPT`, read the PNGs, and keep them under
`docs/verification/<branch>/`.
