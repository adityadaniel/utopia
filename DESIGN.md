# Utopia Design Spec ("Quiet paper")

Utopia is a desktop outliner built with GPUI. This file is the visual system and the main screens: the Daily Notes feed, a page view, the sidebar, the stacked panes that hold them (§4.3, §4.4) and the Tasks view (§4.5). Build to match what is written here. If something is not covered, pick the option that fits the principles in §1, then add the rule here.

**Last checked against the app:** 2026-11-17. Every token below is a constant in `crates/utopia-ui/src/theme.rs` with the same name (`--line-soft` is `line_soft`). If the two disagree, one of them is a bug: fix the code or this file in the same change.

**How to read it.** §2 is the tokens, §3 the components, §4 the screens, §5 the keyboard and accessibility rules, §6 the open questions and §7 how the web terms below map to GPUI. Section numbers are cited from the code (`DESIGN.md §3.6`), so a new section goes at the end of its group and an old number never moves.

## 1. Principles

1. **Writing first.** Body text is 16px with generous line height. Chrome (sidebar, toolbars, pinned headers) never competes with the notes: it is smaller, quieter and shorter than the content it frames.
2. **One loud color.** Blue means "this goes somewhere" (page links, date links). Nothing else uses it, except focus rings, the active nav icon, the today marker and checked checkboxes.
3. **Quiet structure.** Nesting is shown with thin thread lines and spacing, not boxes. Dividers are 1px warm gray.
4. **One source of truth.** Anything shown on screen is the block data itself, drawn differently. Never show the same data twice (see Properties bar).
5. **One piece, one look.** A row, a button or a hover looks the same on every screen. A real difference is a named variant in §3, never a local tweak.

## 2. Tokens

Use the tokens in `theme.rs`. Don't hardcode a color, a type size, a radius, a duration or a dimension outside them. A new value is a new token here first.

### 2.1 Color

Light and dark mode define the same names. The app follows the system appearance; `UTOPIA_APPEARANCE=light|dark` pins it for screenshots.

| Token | Light | Dark | Use |
|---|---|---|---|
| `--paper` | `#FFFFFF` | `#1B1B19` | Main writing surface |
| `--shelf` | `#F6F6F4` | `#151514` | Sidebar background |
| `--well` | `#FAFAF8` | `#202020` | Reference panels, the pane scroller behind the panes |
| `--hover` | `#F1F1EE` | `#262624` | **Hover on anything that sits on `--paper` or `--well`**: rows, ghost buttons. kbd background |
| `--selected` | `#E9E9E5` | `#2C2C29` | Active sidebar item, selected segment. Hover of a ghost button that sits inside a hoverable row (§3.4) |
| `--nav-hover` | `#EDEDEA` | `#232322` | Hover on anything that sits on `--shelf` (sidebar rows) |
| `--line` | `#E8E7E3` | `#33332F` | Structure: panel borders, thread lines, the sidebar edge, the pinned day header's bottom edge |
| `--line-soft` | `#EFEEEA` | `#2A2A27` | Dividers between days and sections, count badge background |
| `--line-strong` | `#E1E0DB` | `#3D3D38` | Control borders: inputs, outline buttons, chips, ref type labels |
| `--ink` | `#1B1B19` | `#E9E8E3` | Primary text |
| `--ink-2` | `#3A3A35` | `#C6C5BE` | Nav text, reference text |
| `--ink-3` | `#57574F` | `#A3A29A` | Secondary text, icons, counts in the sidebar |
| `--muted` | `#6C6C65` | `#8F8E86` | Labels, placeholders, counts on `--paper` and `--well` |
| `--bullet` | `#8A8A83` | `#6E6D66` | Bullets and carets only. **Never text** (fails contrast) |
| `--disabled` | `#B9B8B1` | `#55554F` | Disabled icon strokes, offline dot. **Never text** |
| `--link` | `#2D5BD2` | `#7FA3F0` | Page links, date links, focus, today |
| `--link-hover` | `#2249B0` | `#9DB9F5` | Link hover and pressed |
| `--link-tint` | `#EAEFFB` | `#262F45` | "Today" pill fill, the ring on a link whose page is open in the next pane |
| `--tag-bg` | `#E9ECF4` | `#2A3040` | Tag chip fill, avatar fill |
| `--tag-ink` | `#34415F` | `#C3CEE6` | Tag chip text, avatar initials |
| `--status-progress` | `#C27A12` | `#E0A445` | Status dot for "in progress" |
| `--danger` | `#B42318` | `#FF9C94` | Destructive actions, the Overdue label, errors |
| `--danger-bg` | `#FBE4E4` | `#4A2222` | Fill behind `--danger` text |
| `--warning` | `#9A5A00` | `#F0B45A` | Sync warnings |
| `--success` | `#1F7A3D` | `#6FD08C` | The in-sync dot |
| `--selection` | `#B7D2FB` | `#264F8F` | Selected text |
| `--highlight-bg` | `#FFF1A8` | `#4E461F` | A search hit, an unlinked mention |
| `--code-bg` | `#F0F0EE` | `#22221F` | Inline code and code block fill |
| `--banner-bg` | `#EAF2FF` | `#222E42` | The sync banner |
| `--scrim` | black 18 % | black 42 % | Behind a modal palette |
| `--pdf-highlight` | yellow 35 % | same | PDF highlight overlay (does not follow the theme) |
| `--pdf-selection` | blue 25 % | same | PDF text selection overlay |

Rules:
- **Contrast.** Every pair of text and background that a component draws must reach 4.5:1 in both themes. The test `every_text_pair_reaches_contrast` in `theme.rs` holds the list; a new component adds its pairs there. One known trap: `--muted` on `--selected` fails (4.34:1 light, 4.26:1 dark), so text on an active row or a selected segment uses `--ink-3` or `--ink`, never `--muted`. For the same reason a count on a sidebar row is always `--ink-3`.
- **Hover.** There are two hover fills: `--hover` on paper and well, `--nav-hover` on shelf. Nothing else is a hover fill. Hover is always quieter than `--selected`.
- **Borders.** The three line tokens are close on purpose: each one has one role (structure, divider, control) and they are not shown side by side.
- Page links and tags must look different in shape, not only color: links are plain blue text, tags are slate chips.

### 2.2 Typography

Fonts: **IBM Plex Sans** (400, 500, 600) for everything people read. **IBM Plex Mono** (400, 500) for small labels, counts, shortcuts and weekday labels. Both are bundled under `crates/utopia-ui/assets/fonts`; code blocks use Menlo. Settings → Appearance can switch the Sans role to the system font.

The ramp. Each role is a `TypeRole` in `theme.rs` (size, line height and weight travel together).

| Role | `theme.rs` | Font | Size / line height | Weight |
|---|---|---|---|---|
| Page title | `PAGE_TITLE` | Sans | 36 / 44 | 600 |
| Day title | `DAY_TITLE` | Sans | 30 / 38 | 600 |
| Page title in a pane | `PANE_PAGE_TITLE` | Sans | 30 / 38 | 600 |
| Day title in a pane | `PANE_DAY_TITLE` | Sans | 26 / 34 | 600 |
| Section heading | `SECTION` | Sans | 20 / 28 | 600 |
| Compact day title (empty day row, pinned day header) | `EMPTY_DAY_TITLE` | Sans | 17 / 26 | 600 |
| Block text | `BODY` | Sans | 16 / 28 | 400 |
| Reference block text | `REFERENCE` | Sans | 15 / 26 | 400 |
| Workspace name | `WORKSPACE_NAME` | Sans | 15 / 22 | 600 |
| UI text (nav, buttons, search) | `UI` | Sans | 14 / 22 | 400 (500 when active) |
| Small (meta, footer, property keys) | `SMALL` | Sans | 13 / 20 | 400 |
| Caption (breadcrumbs, "First mentioned") | `SMALL_TIGHT` | Sans | 12 / 18 | 400 |
| Label | `LABEL` | Mono | 12 / 16 | 500, UPPERCASE |
| Sidebar label, ref type label | `SIDEBAR_LABEL`, `REF_LABEL` | Mono | 11 / 16 | 500 |
| Count, kbd | `COUNT` | Mono | 12 / 16 | 400 |

Rules:
- Use a role, never a bare size. The 15, 17 and 26 sizes exist only for the roles that name them; don't reuse them for anything else.
- GPUI 0.2 has no letter-spacing, so titles and labels have none (§7). macOS draws text antialiased by itself.

### 2.3 Spacing and dimensions

**Spacing** (gaps and padding): 4px base. Allowed values: 4, 8, 12, 16, 20, 24, 32, 40, 48, 56, 64, 72, 96.

Inside a component, a padding may be off the scale when it centres content in a fixed height or on the 28px line grid. These are the only ones: tag chip and Today pill 0 7, count badge 1 8, reference group header 14 top, property bar 6, checkbox gap 10, bullet-to-text 15. A new one is listed here with its reason.

- Content column: max width **760px**, centered, padding 48px top (56 on a page view), 40px sides, 96px bottom.
- Between days: 40px above the 1px `--line-soft` divider and **24px below it**, on every kind of day section (open day, empty-day group). The first day has no divider.
- Sidebar: 12px padding.
- Top bar: padding 8px 20px.

**Dimensions** (fixed sizes of pieces; these are not gaps, so they need not be on the spacing scale). Each one is a constant in `theme.rs`:

| Piece | Size |
|---|---|
| Sidebar | 264 wide (min 240) |
| Top bar | min 52 tall |
| Block line grid | 28 |
| Caret slot / bullet slot | 20 / 24 |
| Child indent (wide / narrow / compact pane) | 45 / 26 / 22 (31 margin + 13 padding + 1 thread line for wide) |
| Empty day row, pinned day header row | 44 tall |
| Empty day weekday column | 92 wide |
| Nav row / page row / search field | 34 / 32 / 36 tall |
| Ghost icon button | 32 × 32 |
| Ghost text button / quiet control | 32 / 28 tall |
| Tag chip / Today pill / ref type label | 22 / 20 / 20 tall |
| Property row | min 36 tall, key column 140 |
| Pane | 560 wide, header 44, spine 40 |

### 2.4 Radius

| Value | `theme.rs` | Use |
|---|---|---|
| 3 | `RADIUS_LINK_RING` | Tint ring on a link open in the next pane |
| 4 | `RADIUS_CHIP` | kbd, ref type label, checkboxes |
| 5 | `RADIUS_TAG` | Tag chips |
| 6 | `RADIUS_CONTROL` | Nav items, ghost buttons, every row hover |
| 8 | `RADIUS_INPUT` | Inputs, search field, button groups, segmented controls |
| 10 | `RADIUS_PANEL` | Panels (Linked references), the Tasks empty state |
| 13 | `RADIUS_PILL` | **Round**: every pill and badge (status pill, count badge, Today pill, due label). Each is at most 26 tall, so 13 makes it fully round |

No shadows anywhere. Use borders instead. The only exception is the inset focus ring on a property row being edited.

### 2.5 Icons

16px line icons, 2px stroke (1.75 for the 18px panel icons), round caps and joins, color `--ink-3`. One set (Lucide). Icons needed: search, calendar, page, person, sidebar-left, sidebar-right, arrow-left, arrow-right, chevron-down, chevron-right, chevron-left, plus, filter, star, settings, list (aliases), dashed-circle (status), square-check (Tasks).

An icon-only button has a tooltip with its name (GPUI has no accessible label, §7).

### 2.6 Motion

| Token | Value | Use |
|---|---|---|
| `SCROLL_ANIM` | 200 ms, ease-out | Smooth scroll of the pane row (§4.3) |
| `BLINK_INTERVAL` | 500 ms | Caret blink, after 500 ms steady from the last keystroke |

Rules:
- Nothing else animates. Collapse, expand, menus, the palette and the pinned day header appear and go at once.
- With the system setting Reduce Motion on, a smooth scroll jumps (`workspace.rs` `reduce_motion`).
- A new animation adds a token here. Opening is slower than closing (close in about two thirds of the time).

## 3. Components

Each component lists its states. A state that is not listed does not change the piece.

### 3.1 Block (outliner row)

- The row height follows a **28px line grid**. Bullets, carets and checkboxes are vertically centered within the first 28px line.
- Layout from left: caret slot (20px, sits in the left gutter outside the text column) → bullet slot (24px) → content. A top-level bullet's left edge sits on the page title's left edge; the text starts 15px further in (the rest of the bullet slot).
- Bullet: 6px circle, `--bullet`.
- Collapsed block: the bullet gets a 12px `--line-strong` ring around a 6px `--ink-3` dot, and the caret points right.
- Caret: 12px chevron, `--bullet`. Shown on a block that has children, on hover and on the row being edited. The keyboard folds with the Collapse action, so the caret is not the only way.
- Children: indent by 45px (31 margin + 13 padding + the 1px `--line` thread line). The thread line lines up under the parent bullet's center. Settings → Appearance → Indent offers a 26px step instead; the thread line follows.
- Todo: a 16px checkbox, radius 4, checked fill `--link`, with a 10px gap before the text. Done todos: text `--muted` with a strikethrough.
- Inline page link: `--link`, no underline, underline on hover.
- Date link: same as page link (dates are pages).
- External link (a web address, typed bare or as `[text](url)`): `--link`, underlined, followed by a thin space and `↗` in `--link` (not underlined), so it reads as leaving the app before it is clicked. The arrow is part of the link. Attachments and `utopia://` links stay in the app and get no arrow.
- Tag: inline chip, height 22, padding 0 7px, radius 5, `--tag-bg` fill, `--tag-ink` text, 14px. The fill reaches up to 7px into the space on each side of the tag. Two chips separated by one space share it: each fill reaches half way, leaving 2px of paper between them (a text run cannot be made wider, §7).
- Ref type label (for example "Scheduled for this day"): height 20, padding 0 7px, radius 4, 1px `--line-strong` border, Mono 11px `--ink-3`, `--paper` fill.

| State | Look |
|---|---|
| Hover | Caret and drag handle show in the gutter |
| Editing | Raw markup, caret; caret and drag handle show |
| Selected (block selection) | `--selection` fill behind the row |

### 3.2 Sidebar

From top to bottom:

1. **Workspace row:** 24px black square with the initial in white, the workspace name (15/600), a chevron-down for switching workspaces, and a collapse-sidebar icon button on the right.
2. **Search field:** a button that opens the command palette. Height 36, radius 8, `--paper` fill, `--line-strong` border, search icon, placeholder text "Search or jump to" in `--muted`, and `⌘K` kbd on the right.
3. **Nav:** Daily Notes, Tasks (square-check icon, open to-do count on the right, §4.5). Each row is 34px tall, radius 6, 14px text.
4. **Mini calendar:** month label (13/600) with prev/next buttons. A 7-column grid, weeks start on Sunday, weekday labels in Mono 11. Always 6 rows, so the sections below never move when the month changes. Day cells are 30px tall. Days of the next or previous month are `--muted`. Today: solid `--link` fill with white text. Days with notes: a 4px `--link` dot under the number. Clicking a day opens that daily note.
5. **Pages:** Mono label "PAGES", then rows 32px tall with an icon (person icon for people pages, page icon for others), the name, and the reference count on the right. Pinned pages come first.
6. **Tags:** Mono label "TAGS", then tag chips in a wrapping row. Shown only when a tag page exists. Tags are listed here, **not** in Pages.
7. **Footer** (pinned to the bottom, 1px top border): an 8px status dot (`--disabled` when local only, `--success` in sync), the vault name in `--ink` 500, " · Local only", and a settings icon button.

There is no "Recent days" list: the feed already shows the days.

**Counts** on every sidebar row are Mono 12 `--ink-3` (not `--muted`, which fails on `--selected` and `--nav-hover`, §2.1).

| State (nav row, page row) | Look |
|---|---|
| Hover | `--nav-hover` fill |
| Active | `--selected` fill, `--ink` text at 500, icon in `--link` |
| Focus (keyboard) | Focus ring (§5) |

### 3.3 Top bar

Back and forward icon buttons (forward is `--disabled` when there's nothing to go forward to), a title or breadcrumb, then actions on the right.

- Daily Notes: title "Daily Notes", then a day navigation group (prev | Today | next) with a 1px `--line-strong` border, radius 8, 32px tall, then the view mode control (§3.9), then a toggle for the right panel.
- Page view: the current page's title in `--ink` 500, then a more (⋯) menu that holds Delete page, the view mode control and the right panel toggle.
- **Title or trail.** In Single mode the top bar shows only the current title. The pages before it are history, not a place the page sits in (pages have no hierarchy), so they are not drawn as a "›" trail. Back's tooltip names the page it goes to ("Back to Atlas Launch"). In Panes mode the top bar shows the trail of open panes (§4.3), because those panes are on screen side by side.

1px `--line-soft` bottom border.

### 3.4 Buttons

| Variant | Size | Text | Rest | Hover | Disabled |
|---|---|---|---|---|---|
| **Ghost icon button** (`ghost_icon`) | 32 × 32, radius 6 | — | transparent, icon `--ink-3` | `--hover` | icon `--disabled` |
| **Ghost text button** (`ghost_text`): top bar and toolbars | 32 tall, padding 0 10, radius 6 | 13px `--ink-3` | transparent | `--hover`, text `--ink` | text `--muted` |
| **Quiet control** (`quiet_control`): inside rows and panels ("Write", "Add property", "Add alias", "Link") | 28 tall, padding 0 8, radius 6 | 13px `--ink-3` | transparent | `--hover`, text `--ink`; **`--selected` when the row around it has its own hover** (`quiet_control_in_row`) | text `--muted` |
| **Outline button** (`outline_button`): "Filter", "Close other panes" | 32 tall, padding 0 10, radius 6 | 13px `--ink-3` | `--paper` fill, 1px `--line-strong` border | `--hover` | text `--muted` |
| **Today pill** | 20 tall, padding 0 7, round | Mono 12 500 `--link` | `--link-tint` | — | — |
| **Count badge** | padding 1 8, round | Mono 12 `--ink-3` | `--line-soft` | — | — |

There is no pressed state: a click acts at once, and the hover stays until the pointer leaves.

A ghost button inside a row that has its own hover (an empty day row) would vanish on the row's `--hover` fill, so its hover is `--selected`.

Every interactive element takes keyboard focus and shows the focus ring (§5).

### 3.5 Day header (Daily Notes)

- Weekday label above the title: Mono 12 UPPERCASE `--muted` (for example "FRIDAY"). On today's date, add the Today pill after it.
- Title: the page name exactly as stored (for example "November 6th, 2026"), Day title style. Don't reformat it, since it's also the page name that links point to. The title is a link to the day's page: `--link` on hover.
- 16px gap below the title before the first block.
- Above every day but the first: the day divider (§2.3: 40 above, 24 below).

**Pinned day header.** When a day's own header has scrolled out of view and the day's blocks still fill the top of the column, a compact copy of the header is pinned at the top of the column, so you always know which day you are in.

- It appears only when the bottom of the day's own title has passed under it. While the real header is still visible, nothing is pinned.
- It is one row, 44px tall, the same as an empty day row (§3.6): weekday label (Mono 12 `--muted`, 92px column), then the title in the Compact day title role (17/600), then the Today pill on today. A 1px `--line` bottom edge. `--paper` fill.
- It covers the column completely: no block text shows through it or between it and its bottom edge. It draws above the list (§7, "Drawing order").
- The Today pill in it scrolls to today. The rest of it does not take clicks; a click goes to the block under it.
- A row the editor moves to is never left under it: the list scrolls so the row starts below its bottom edge.
- An empty-day group does not pin the day above it (its rows carry their own dates).

### 3.6 Empty day row

A day with no blocks is **not** a full section. It's one row 44px tall: weekday label (Mono 12 `--muted`, fixed 92px wide), date title (Compact day title, takes the remaining width), and a quiet control "Write" (§3.4). Clicking "Write" (or the row) expands it into a normal day with one empty block that has focus. Several empty days in a row stack together with 4px gaps, under one day divider (§2.3).

Every row of a group has the same height and the same gap, including the first: the divider and its space sit above the first row, not inside it.

| State | Look |
|---|---|
| Hover | `--hover` fill, radius 6, reaching 8px past the text on each side (the same as every row hover) |
| Hover on "Write" | `--selected` fill behind "Write" (§3.4) |
| Focus | Focus ring around the row (§5) |

### 3.7 Linked references panel

Spacing in this panel says what belongs together: a group title is close to its own references and far from the group above it. The references are the content; the chrome stays small.

- Container: 1px `--line` border, radius 10, `--well` fill.
- Header row (padding 8px 16px, about 45px tall): "Linked references" heading in UI 14/600 `--ink`, plus a count badge. On a page view, an outline "Filter" button sits on the right. Inside Daily Notes, the header is a full-width button with a chevron that collapses the panel.
- **Controls only when asked for.** The filter field, the date menu ("Any time"), the sort menu and the co-reference chips are hidden until Filter is pressed. Filter opens them and puts the caret in the field; pressed again it closes them. While a filter is in force (text, a date range, a co-reference or a sort other than the default), they stay open, so a filter is never hidden. Filter has the `--selected` fill while they are open.
- **Sort names say what the order is:** "Days, then A–Z" (daily pages newest first, then other pages by title; the default) and "Last edited".
- **Co-reference chips are labelled.** The chip row starts with "Also mentions" in Caption `--ink-3`. A chip's number is how many references it would keep, which is not the same as a group's count, so the label is needed.
- **On a daily page** (in the feed or opened as a page), every reference comes from another page: that is what "Scheduled for this day" means (PRD FR-1.12). The header shows the ref type label "Scheduled for this day" once, after the count. The groups below do not repeat it.
- Groups: one per source page, separated by 1px `--line` dividers. The group title is UI 14/500 `--ink` (not `--link`: the one loud colour stays for the links inside the references), followed by the group's count badge only when the group has more than one reference. The title row has 12px above it (4px for the first group, right under the header) and nothing below it, so the title sits with its own references. The row is a button that opens the page; it has the `--hover` fill on hover.
- Referenced blocks render with the Reference block text style. Children keep their thread line. The current page's name inside a reference is shown as **bold `--ink` text, not a link** (linking to the page you're already on is pointless). Inside a day of the feed, the current page is that day: a link to the day is bold `--ink` too, in any date form that resolves to it.
- A path above a nested reference names its parent blocks, in Caption `--muted`; each part is a link. It does not start with the page: the group title right above already names it. A part longer than 40 characters is cut at the last whole word and ends with "…"; it is never cut in the middle of a word without one. The path wraps when it is wider than the panel.
- **More references than the group shows** (more than 3): a row of its own at the group's level, after the last reference: a chevron-down and "Show 2 more from Atlas Launch" in 13px `--ink-3`, 28px tall, radius 6, `--hover` fill and `--ink` text on hover. It names the group, so it does not read as a child of the last reference.
- **Unlinked references** is the other half of "who mentions this page", so it is the same kind of panel, 16px below the linked one: the same well, border and radius, a 44px header with a chevron, the heading "Unlinked references" in UI 14/600 `--ink`, and its count badge once it has been searched. It starts collapsed (rounded on all four corners) and is searched only when it is opened, so a closed panel has no count. Open, its rows (source page link in Caption `--link`, the mention with its `--highlight-bg` fill, a small "Link" button: Caption `--link`, 1px `--line` border, radius 4) sit inside the panel with 16px side padding, and "Link all" sits on the header's right edge.

| State | Look |
|---|---|
| Hover on a reference | `--hover` fill on the block, inside the panel border |
| Hover on a group title row | `--hover` fill on the row |
| Collapsed (feed) | Header only, rounded on all four corners |
| Controls open | Filter has the `--selected` fill; the controls and the chips sit between the header and the first group |

### 3.8 Properties bar (page view)

**Data model, read this first.** Properties are ordinary blocks (for example `status: in progress`). The properties bar is **not** a separate table that copies them. It is the page's leading property blocks, drawn as rows instead of bullets.

Rules:
1. Property blocks are the **first consecutive blocks** at the top of a page. The bar renders exactly those blocks. The first non-property block ends the bar.
2. Property blocks are **never** also rendered as bullets in the outline. They render once, in the bar.
3. A property block found lower in the outline (not at the top) is just a normal block and is drawn as a normal bullet. It does **not** show up in the bar.
4. If a page has no leading property blocks, the bar shows only the "Add property" row.

Layout:
- Placed 20px below the page title. Top and bottom 1px `--line-soft` borders, 6px vertical padding.
- Each row: a 2-column grid, `140px | 1fr`, at least 36px tall, radius 6. Hover: `--hover` (the row extends 8px past the content on each side, so the text stays aligned with the title).
- Key column: a 15px icon slot plus the key in 13px `--ink-3`, lowercase as typed. The icon says what kind of value follows: the dashed circle only for `status`, the person icon for a single page link (drawn with an avatar), the list icon for `aliases`, and no icon for anything else. An empty slot keeps its width, so every key starts on the same edge.
- Value column, drawn by type:
  - `status`: a pill, 26px tall, padding 0 10px, round, 1px `--line-strong` border, `--paper` fill, a 7px status dot, 13px `--ink`. The value is title-cased for display only.
  - Page link values (for example `owner`): a 22px round avatar with initials (`--tag-bg` / `--tag-ink`, 10px 600) plus the link in 14px `--link`.
  - Anything else: plain 14px `--ink`, with links drawn as links.
- `aliases` row: always shown. When empty, it shows a quiet "Add alias" control.
- Last row (no hover): quiet "Add property" on the left, and on the right "First mentioned <date link>" in Caption `--muted`.

Editing:
- Clicking a row (anywhere except a link inside it) turns that row back into its **raw block text**, in place, at the same height. Example: **status:** in progress, with the key in 600 weight. Edit mode shows an inset 1.5px `--link` ring, `--paper` fill, and the caret at the end.
- Enter or Escape commits the edit and draws the row again. Enter at the end of the **last** property row leaves the bar and creates or focuses the first body block below it.
- "Add property" inserts a new empty property block at the end of the bar, in edit mode.
- Arrow up from the first body block moves into the last property row. Arrow down from the last property row moves into the first body block. The bar takes part in normal block keyboard navigation.
- Deleting all the text in a property row removes the block, and its row disappears.

### 3.9 Segmented control

Used for the view mode (Panes | Single, §4.4) and the Tasks filter (Open | Done | All, §4.5).

- Height 32, 1px `--line-strong` border, radius 8, segments separated by the same border.
- Each segment: optional 16px icon, 14px label, optional count in Mono 12 `--muted`.

| State | Look |
|---|---|
| Rest | `--paper` fill, `--ink-3` text |
| Hover (not selected) | `--hover` fill |
| Selected | `--selected` fill, `--ink` text at 500 |
| Focus | Focus ring around the segment (§5) |

### 3.10 Task row and due label

Used by the Tasks view (§4.5).

- Task row: the §3.1 to-do (16px checkbox, 10px gap, Block text on the 28px grid). A done task is `--muted` with a strikethrough.
- Context line (the parent is a plain block): Caption-size 13/20 `--muted` under the task text, aligned with it, 4px tighter to the task above.
- Nested to-do: indented under its parent's checkbox with the §3.1 thread line.
- Due label: after the text on the right, Mono 11 UPPERCASE, round, padding 4px 7px. **Overdue** is `--danger` on `--danger-bg`; **Today** is `--link` on `--link-tint`. The date itself stays in the text as a date link.

| State | Look |
|---|---|
| Hover | `--hover` fill, radius 6, reaching 8px past the text on each side |

## 4. Screens

### 4.1 Daily Notes

Sidebar plus main area. The main area has the top bar and the content column. Days are listed newest first, in the order: day section (§3.5), linked references (if any, inside that day's section, §3.7), then the next day. Consecutive empty days are grouped into empty day rows (§3.6). Older days load as you scroll; when the oldest day is loaded the feed ends with "No earlier notes." in `--muted`. Scroll position resets to today when you click "Today". Days have no Unlinked references in the feed; those stay on the day's own page.

### 4.2 Page view

Sidebar (the page is highlighted in Pages) plus main area: top bar with the page's title (§3.3), page title (Page title style), properties bar, body blocks (placeholder "Click to start writing" in `--muted` with a `--line-strong` bullet when the body is empty), then the Linked references panel 48px below, then the Unlinked references panel 16px below that (§3.7).

### 4.3 Stacked panes (navigation model)

Pages open side by side in panes instead of replacing each other. Daily Notes is always the first pane.

Layout:

- The main area below the top bar is one horizontal scroller with `--well` as its background. Panes sit in a row inside it.
- Each pane: fixed width 560px, full height, `--paper` background, 1px `--line` border on both sides. Each pane scrolls vertically by itself. A scroller narrower than 560 shrinks the pane to fit.
- Each pane sticks at `left: index × 40px`. As you scroll right, older panes stay stuck at the left edge, and newer panes slide over them (GPUI has no sticky positioning: `pane_stack::Geometry` places each pane, §7).
- Spine: when a pane is almost fully covered by the next one (less than 80px of it shows), its visible 40px strip turns into a spine: `--well` fill, 1px `--line` right border, and the page title written top to bottom, 13/500 `--ink-2`, starting 18px from the top (one grapheme per line, upright, §7). Clicking the spine scrolls back so that pane is fully visible (`scrollLeft = index × (paneWidth − 40)`). When the window cannot hold every spine plus a whole pane, the oldest spines fold under one another at the left edge.
- Pane header: 44px tall. It holds a close (×) icon button on the right, on every pane except the first. Pane content: padding 0 40px 72px. Page title is 30/38 inside a pane, and day titles are 26/34.

Behavior:

- Plain click on a page link, date link or tag in pane i keeps panes 0…i, drops every pane to the right of i, adds the target as pane i + 1, and smooth-scrolls to the end (§2.6). ⌘-click replaces pane i in place: keep 0…i−1, put the target at i, and drop the rest. The Daily Notes pane is never replaced, so a ⌘-click inside pane 0 acts like a plain click.
- If the target is already open anywhere in the stack, nothing is added. The scroller just smooth-scrolls to that pane.
- In pane i, any link whose target is open in pane i + 1 gets a `--link-tint` highlight (2px tint ring, radius 3), so you can see which link the next pane came from.
- Closing a pane removes only that pane. The panes after it shift left.
- Sidebar items (pages, tags, calendar days) open their target as a new pane at the end of the stack. Clicking "Daily Notes" in the sidebar resets the stack to only the Daily Notes pane.
- Top bar: the breadcrumb is replaced by a trail of the open panes (titles separated by chevron-right icons; the last one is `--ink` 500, the others `--ink-3`). Clicking a title scrolls to that pane. "Close other panes" (outline button) keeps the Daily Notes pane and the focused pane. Back closes the last pane.
- The stack is saved as device-local state, so a relaunch restores the same panes.
- Day titles in the Daily Notes pane open that date's page as a pane.

### 4.4 View mode toggle: Panes or Single

A segmented control (§3.9) in the top bar, just left of "Close other panes": Panes (three-column icon) | Single (page icon). Remember the choice across app launches (`view_mode` in `settings.json`, not per page).

Both modes share one list: the pane stack. Single mode just shows its last item.

- Single mode: one page fills the main area. The content column is centered with a max width of 760px. There are no spines, close buttons or "Close other panes" button.
- A plain click navigates: it adds the target to the end of the list. If the target is already in the list, cut the list back to that entry instead of adding a duplicate.
- ⌘-click opens the target as a pane: switch to Panes mode and add it at the end. This gives a quick way to "peek" a page next to the current one without changing the setting.
- Back removes the last item. The top bar shows only the current title (§3.3), and Back's tooltip names the page it goes back to.
- Switching modes keeps the list as it is. Single → Panes shows every entry as a pane, scrolled to the end. Panes → Single shows the last pane.

Keyboard (both modes): ⌘W closes the focused pane, ⌥← and ⌥→ move focus between panes, and moving focus into a pane scrolls it fully into view.

### 4.5 Tasks

Every to-do in the graph, grouped by the page it is written on (PRD §9.2.2). It takes the main area the way the graph view does; opening a page brings the pane stack back.

- Top bar: back button, the title "Tasks" (`--ink` 500), then a segmented control **Open N | Done N | All N** (§3.9), then the right panel toggle.
- Content column: the same 760px column as a page, padding 20px top, 28px sides, 32px bottom. Page title "Tasks" (Page title style). No summary line: the counts are in the control.
- Group (one per page): a 1px `--line-soft` divider with 12px above and below it (also 12px above the first group). For a daily page, the weekday label (Mono 12 UPPERCASE `--muted`) sits directly above the title. The title is Section heading style in `--ink`, underlined in `--line-strong` on hover, and opens the page. A count badge (§3.4) sits on the right. A 12px chevron in the left gutter folds the group. No page icon: the title starts on the column edge, like the checkboxes below it.
- Sub-heading (tasks under a heading block): the heading's plain text, 13/20 weight 600 `--ink-3`, 8px above, nothing below. No level tag. Tasks under no heading come first in the group, then one section per heading in the order the headings appear, so a task never looks as if it sits under a heading it is not under.
- Rows: task rows (§3.10), the first one 4px below the group title.
- Empty state: a 1px dashed `--line-strong` box, radius 10, padding 24px, 48px below the title, centered `--muted` text naming `/To-do` and ⌥↵.

## 5. Keyboard and accessibility

**Focus.** Every interactive element (button, row that opens something, segment, link outside the editor) can take keyboard focus.

- **Order:** sidebar (top to bottom) → top bar (left to right) → the content column (top to bottom) → the right panel. Tab moves forward, Shift-Tab back.
- **The editor keeps Tab.** While a block is being edited, Tab and Shift-Tab indent and outdent. Escape ends the edit and puts focus on the block's row; Tab then continues the order above from there.
- **Ring:** 2px `--link`, 2px outside the element, the element's radius plus 2. Shown only for focus that came from the keyboard, never after a click (`FOCUS_RING`, `FOCUS_RING_OFFSET`).
- Enter or Space activates the focused element.

Other rules:
- Text contrast is at least 4.5:1 for every pair a component draws (§2.1). `--bullet` and `--disabled` are never used for text.
- Clickable rows (empty day, unlinked references) are at least 44px tall. Small buttons can be 28 to 32px, since this is desktop only.
- Reduce Motion is respected (§2.6).
- Window min width: 960px. Below about 1100px, the sidebar can be collapsed with the toggle (⌘\). Content stays capped at 760px.

## 6. Open questions

- Status colors beyond "in progress" (done, blocked, and so on) aren't defined yet. Add them as `--status-*` tokens and check each one's contrast before use.
- The right panel (380px, `right_sidebar_width`) and the command palette (⌘K, quick jump) are built but have no section here yet. Write them up before the next change to either.
- Focus (§5) is specified but not built: `FOCUS_RING` is defined and not drawn anywhere yet.

## 7. Platform mapping (GPUI 0.2)

This spec was first written in web terms. GPUI does not have all of them. When a rule needs one of these, build it the way the right column says.

| Web term | In GPUI |
|---|---|
| CSS custom property (`--line-soft`) | A field on `Theme` (`th.line_soft`); sizes are `const`s in `theme.rs` |
| `position: sticky` (panes, pinned day header) | Not available. Compute the position from the scroll offset and draw the piece absolutely, after the list (panes: `pane_stack::Geometry`; day header: `PageView::sticky_heading`) |
| Drawing order / `z-index` | Later siblings paint above earlier ones, but text and quads of an earlier list can still land above a later fill. A piece that must cover the list (the pinned day header, menus) is drawn with `deferred(...)` |
| `writing-mode: vertical-rl` | Not available. A spine sets one grapheme per line, upright |
| `letter-spacing` | Not available. Leave it out |
| `outline` / `:focus-visible` | Not available. Draw the focus ring as a border on a wrapper, only when focus came from the keyboard |
| `accent-color` on a checkbox | Draw the checkbox: 16px, radius 4, `--line-strong` border, `--link` fill and a white check when done |
| `aria-*`, landmarks, accessible labels | No accessibility tree. Use tooltips for icon-only buttons; keep the keyboard rules in §5 |
| `text-overflow: ellipsis` | `.truncate()` on a single-line element; a run of styled text adds its own "…" |
| A transition | Only the motion tokens in §2.6 |
| URL state | Device-local state (`local_state`), never the CRDT (architecture rule 9) |
