# Utopia Design Spec ("Quiet paper")

Utopia is a desktop outliner built with GPUI. This file is the visual system and every screen: the Daily Notes feed, a page view, the sidebar, the stacked panes that hold them (§4.3, §4.4), the Tasks view (§4.5), the right panel, the graph, the PDF viewer, Settings and the start screens (§4.6 to §4.10). Build to match what is written here. If something is not covered, pick the option that fits the principles in §1, then add the rule here.

**Last checked against the app:** 2026-11-18. Where the app does not yet match a rule, §6 lists it. Every token below is a constant in `crates/utopia-ui/src/theme.rs` with the same name (`--line-soft` is `line_soft`). If the two disagree, one of them is a bug: fix the code or this file in the same change.

**How to read it.** §2 is the tokens, §3 the components, §4 the screens, §5 the keyboard and accessibility rules, §6 the open questions and the list of places where the app does not match this file yet, and §7 how the web terms below map to GPUI. Section numbers are cited from the code (`DESIGN.md §3.6`), so a new section goes at the end of its group and an old number never moves.

## 1. Principles

1. **Writing first.** Body text is 16px with generous line height. Chrome (sidebar, toolbars, pinned headers) never competes with the notes: it is smaller, quieter and shorter than the content it frames.
2. **One loud color.** Blue means "this goes somewhere" (page links, date links). Nothing else uses it, except focus rings, the active nav icon, the today marker and a control that is on (a checked checkbox, a chosen radio, a switch).
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
| `--syntax-keyword` | `#8A3A8F` | `#D4A6E0` | Code block keywords, storage words, `true`/`null` (§3.11) |
| `--syntax-string` | `#2F6B2F` | `#9CCC8E` | Code block strings and escapes |
| `--syntax-number` | `#A04A12` | `#E5AC7A` | Code block numbers |
| `--syntax-name` | `#2A5EA8` | `#8DB6EE` | Code block function, type and class names |
| `--banner-bg` | `#EAF2FF` | `#222E42` | The sync banner |
| `--scrim` | black 18 % | black 42 % | Behind a modal palette |
| `--pdf-highlight` | yellow 35 % | same | PDF highlight overlay (does not follow the theme) |
| `--pdf-selection` | blue 25 % | same | PDF text selection overlay |
| `--pdf-paper` | `#FFFFFF` | same | The sheet under a PDF page (§4.8). A document's paper does not follow the theme |

Rules:
- **Contrast.** Every pair of text and background that a component draws must reach 4.5:1 in both themes. The test `every_text_pair_reaches_contrast` in `theme.rs` holds the list; a new component adds its pairs there. `--muted` is safe on `--paper`, `--shelf`, `--well`, `--hover`, `--nav-hover` and `--code-bg` only. It fails on `--selected` (4.34:1 light, 4.26:1 dark), on `--line-soft` (4.37:1 dark) and on `--banner-bg` (4.15:1 dark), so text on an active row, a selected segment, a chip or a banner uses `--ink-3` or `--ink`, never `--muted`. For the same reason a count on a sidebar row is always `--ink-3`.
- **On a fill.** Text or a mark on a `--link` or `--ink` fill is `--paper` (today's cell, the check in a done checkbox, the workspace initial). Never write "white" or "black" in a rule or in the code: they do not follow the theme, and white on the dark `--link` is 2.5:1. The two tokens that are white or black on purpose are `--scrim` and `--pdf-paper`.
- **Selected.** `--selected` is the one fill for "this is the chosen one": the active sidebar row, the selected segment, the row the keyboard is on in a menu or the palette (§3.14). Its text is `--ink`; a second text on it is `--ink-3`.
- **Hover.** There are two hover fills: `--hover` on paper and well, `--nav-hover` on shelf. Nothing else is a hover fill. Hover is always quieter than `--selected`.
- **Borders.** The three line tokens are close on purpose: each one has one role (structure, divider, control) and they are not shown side by side.
- Page links and tags must look different in shape, not only color: links are plain blue text, tags are slate chips.

### 2.2 Typography

Fonts: the **system font** (SF Pro on macOS; 400, 500, 600) for everything people read, by default. Settings → Appearance can switch that role to **IBM Plex Sans** or **Geist**. The mono font pairs with it, for small labels, counts, shortcuts, weekday labels (400, 500) and code blocks: **SF Mono** with the system font (the system's monospaced UI font, `.AppleSystemUIFontMonospaced`, so nothing is bundled), with IBM Plex Sans, **IBM Plex Mono** for the labels and Menlo for code, and with Geist, **Geist Mono** for both (Vercel's pair). The Plex and Geist fonts are bundled under `crates/utopia-ui/assets/fonts`. Geist 1.3.1 has no italic faces, so italic text in Geist is drawn upright.

The ramp. Each role is a `TypeRole` in `theme.rs` (size, line height and weight travel together).

| Role | `theme.rs` | Font | Size / line height | Weight |
|---|---|---|---|---|
| Page title | `PAGE_TITLE` | Sans | 36 / 44 | 600 |
| Day title | `DAY_TITLE` | Sans | 30 / 38 | 600 |
| Page title in a pane | `PANE_PAGE_TITLE` | Sans | 30 / 38 | 600 |
| Day title in a pane | `PANE_DAY_TITLE` | Sans | 26 / 34 | 600 |
| Section heading, block heading 1 | `SECTION` | Sans | 20 / 28 | 600 |
| Block heading 2 | `H2_SIZE` | Sans | 18 / 26 | 600 |
| Block heading 3 | `H3_SIZE` | Sans | 17 / 24 | 600 |
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
| Code (block and copy-ready text) | `CODE_TEXT_SIZE` | Menlo | 13, on the block line | 400 |

Rules:
- Use a role, never a bare size. The 15, 17, 18 and 26 sizes exist only for the roles that name them; don't reuse them for anything else.
- **Density.** Settings → Appearance → Density sets the line box of Block text and the space between blocks; the text stays 16px. **Tight** 22 + 2 (1.375), **Normal** 24 + 2 (1.5, the default), **Relaxed** 26 + 4 (1.625). The gap between blocks is what tells a new block from the next line of a long one, so no option has 0. Every other role in the table keeps its size and line height in all three: a heading keeps its own line box. (Chosen 2026-11-20 over 16 / 28 with no gap, where two blocks sat as far apart as two lines of one block, and 13.5 / 18, which was too small to read.) A settings file that says `compact` reads as Tight and one that says `comfortable` as Normal.
- GPUI 0.2 has no letter-spacing, so titles and labels have none (§7). macOS draws text antialiased by itself.

### 2.3 Spacing and dimensions

**Spacing** (gaps and padding): 4px base. Allowed values: 4, 8, 12, 16, 20, 24, 32, 40, 48, 56, 64, 72, 96.

Inside a component, a padding may be off the scale when it centres content in a fixed height or on the block line grid. These are the only ones: tag chip and Today pill 0 7, count badge 1 8, reference group header 14 top, property bar 6, checkbox gap 10, bullet-to-text 15. A new one is listed here with its reason.

- Content column: max width **760px**, centered, padding 48px top (56 on a page view), 40px sides, 96px bottom.
- Between days: 40px above the 1px `--line-soft` divider and **24px below it**, on every kind of day section (open day, empty-day group). The first day has no divider.
- Sidebar: 12px padding.
- Top bar: padding 8px 20px.

**Dimensions** (fixed sizes of pieces; these are not gaps, so they need not be on the spacing scale). Each one is a constant in `theme.rs`:

| Piece | Size |
|---|---|
| Sidebar | 264 wide (min 240) |
| Top bar, and the window's top band (§3.18) | min 52 tall |
| Traffic lights (close button's top left, room they take) | at 20, 19 / 64 wide |
| Block line grid (Tight / Normal / Relaxed) | 22 + 2 / 24 + 2 / 26 + 4 (line + gap between blocks) |
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
| 2 | `RADIUS_PANEL_JOIN` | The Unlinked references panel's top corners, where it meets the Linked panel's 2px bottom corners (§3.7) |
| 3 | `RADIUS_LINK_RING` | Tint ring on a link open in the next pane |
| 4 | `RADIUS_CHIP` | kbd, ref type label, checkboxes |
| 5 | `RADIUS_TAG` | Tag chips |
| 6 | `RADIUS_CONTROL` | Nav items, ghost buttons, every row hover, code blocks, file chips, image previews, menus, popovers and tooltips (§3.14) |
| 8 | `RADIUS_INPUT` | Inputs, search field, button groups, segmented controls, the palette panel (§3.15) |
| 10 | `RADIUS_PANEL` | Panels (Linked references), the Tasks empty state, cards in Settings (§4.9) |
| 13 | `RADIUS_PILL` | **Round**: every pill and badge (status pill, count badge, Today pill, due label). Each is at most 26 tall, so 13 makes it fully round |

No shadows anywhere. Use borders instead. The only exception is the inset focus ring on a property row being edited. A piece that floats above the page (menu, popover, tooltip, palette) separates with a fill and a 1px border, never a shadow (§3.14).

### 2.5 Icons

16px line icons, 2px stroke (1.75 for the 18px panel icons), round caps and joins, color `--ink-3`. One set (Lucide). Icons needed: search, calendar, page, person, sidebar-left, sidebar-right, arrow-left, arrow-right, chevron-down, chevron-right, chevron-left, plus, filter, star, settings, list (aliases), dashed-circle (status), square-check (Tasks), pin (the top bar's pin toggle).

Also in the set: close (×), more (⋯), check, grip, paperclip, file, image, clock, command, hash, lock, undo, plus the Settings tab icons. A text character is never an icon: "×", "▾", "✓", "⌘", "📎" and the like are drawn from the set, so they take the icon colour and size. (The arrow after an external link, §3.1, is text on purpose: it is part of the link.)

An icon-only button has a tooltip with its name (GPUI has no accessible label, §7).

### 2.6 Motion

| Token | Value | Use |
|---|---|---|
| `SCROLL_ANIM` | 200 ms, ease-out | Smooth scroll of the pane row (§4.3) |
| `BLINK_INTERVAL` | 500 ms | Caret blink, after 500 ms steady from the last keystroke |
| `CARET_GLIDE` | 80 ms, ease-out cubic | Optional movement of the painted caret while editing a block |

Rules:
- Nothing else animates. Collapse, expand, menus, the palette and the pinned day header appear and go at once.
- Smooth caret movement is off by default. When enabled, nearby moves within a block glide from the current painted position. Clicks, selection, IME composition, reflow, and long or cross-block moves place the caret immediately.
- The system setting Reduce Motion makes smooth scrolls and caret movement immediate.
- A new animation adds a token here. Opening is slower than closing (close in about two thirds of the time).

## 3. Components

Each component lists its states. A state that is not listed does not change the piece.

### 3.1 Block (outliner row)

- The row height follows the **density's line grid** (§2.2; Normal: 24px lines and 2px between blocks, 1px above and 1px below the row). Bullets, carets and checkboxes are vertically centered within the first line.
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

- Text caret: 2px wide, one line tall, `--link`; it blinks (§2.6). Selected text: `--selection` fill, the text keeps its color. An input-method composition has a 1px `--ink` underline. An empty block being edited shows the hint "Type, or [[ to link, # to tag, (( to reference" in `--muted`.
- Drag: the bullet is the handle. The piece that follows the pointer is a floating surface (§3.14) with the first 60 characters of the block. The row the block will land after has a 2px `--link` line along its bottom edge.

| State (row) | Look |
|---|---|
| Hover | The caret shows in the gutter; the bullet becomes a grip (2 × 3 dots, 2px each, `--bullet`) |
| Editing | Raw markup, text caret; the gutter caret and the grip show. In the raw text a reference is `--link`, a tag is `--tag-ink`, and a Markdown link's `[text]` is `--link` with its `(url)` in `--muted` |
| Editing, Vim Normal | A block cursor, not a bar: the cell from the cursor's grapheme to the next one on the same line, `--selection` fill with a 1px `--link` outline, painted under the text. Steady, never blinking (§2.6). An empty block, a line break and a wrap use a minimum width. The rest as Editing |
| Selected (block selection) | `--selection` fill behind the row |

| State (checkbox) | Look |
|---|---|
| Rest | 1px `--line-strong` border, no fill |
| Done | `--link` border and fill, `--paper` check (§2.1 "On a fill") |
| Hover | The pointer cursor only |

| State (page link, date link) | Look |
|---|---|
| Rest | `--link`, no underline |
| Hover | Underline, the pointer cursor |
| Open in the next pane | `--link-tint` ring (§4.3) |
| The page it sits on | Bold `--ink`, not a link (§3.7) |
| No page yet | The same as Rest: following it creates the page. The right panel lists these under Unresolved (§4.6) |

### 3.2 Sidebar

From top to bottom:

1. **Workspace row:** the sidebar's part of the window's top band (§3.18): 52 tall, level with the top bar, so the sidebar has no top padding above it. The traffic lights sit at its left end. Then the 24px `--ink` square with the initial in `--paper`, the workspace name (15/600), a chevron-down for switching workspaces, and a collapse-sidebar icon button on the right.
2. **Search field:** a button that opens the command palette. Height 36, radius 8, `--paper` fill, `--line-strong` border, search icon, placeholder text "Search or jump to" in `--muted`, and `⌘K` kbd on the right.
3. **Nav:** Daily Notes, Tasks (square-check icon, open to-do count on the right, §4.5). Each row is 34px tall, radius 6, 14px text.
4. **Mini calendar:** month label (13/600) with prev/next buttons. A 7-column grid, weeks start on Sunday, weekday labels in Mono 11. Always 6 rows, so the sections below never move when the month changes. Day cells are 30px tall. Days of the next or previous month are `--muted`. Today: solid `--link` fill with `--paper` text at 500. Days with notes: a 4px `--link` dot under the number (`--paper` on today). Clicking a day opens that daily note.
5. **Recent:** Mono label "RECENT", then up to 10 page rows, 32px tall, with an icon (person icon for people pages, page icon for others), the name, and the reference count on the right. The pages opened last on this Mac come first (local state, never synced), then the pages edited last, so a new device still shows what was worked on. Every other page is one ⌘K away. Pinned pages have their own "PINNED" group above it.
6. **Tags:** Mono label "TAGS", then tag chips in a wrapping row. Shown only when a tag page exists. Tags are listed here, **not** in Pages.
7. **Footer** (pinned to the bottom, 1px top border): an 8px status dot (`--disabled` when local only, `--success` in sync), the vault name in `--ink` 500, " · Local only", and a settings icon button.

There is no "Recent days" list: the feed already shows the days.

**Counts** on every sidebar row are Mono 12 `--ink-3` (not `--muted`, which fails on `--selected` and `--nav-hover`, §2.1).

| State (nav row, page row) | Look |
|---|---|
| Hover | `--nav-hover` fill |
| Active | `--selected` fill, `--ink` text at 500, icon in `--link` |
| Active and hover | The same as Active: an active row keeps `--selected` under the pointer, so hover never makes it look less chosen |
| Focus (keyboard) | Focus ring (§5) |

| State (calendar day cell) | Look |
|---|---|
| Rest | `--ink` number, Small 13, radius 6; a day of another month is `--muted` |
| Has notes | A 4px `--link` dot under the number |
| Hover | `--nav-hover` fill (the calendar sits on `--shelf`, §2.1) |
| Today | `--link` fill, `--paper` number at 500, `--paper` dot; no hover change |

### 3.3 Top bar

Back and forward icon buttons (forward is `--disabled` when there's nothing to go forward to), a title or breadcrumb, then actions on the right.

- Daily Notes: title "Daily Notes", then a day navigation group (prev | Today | next) with a 1px `--line-strong` border, radius 8, 32px tall, then the view mode control (§3.9), then a toggle for the right panel.
- Page view: the current page's title in `--ink` 500, then the pin button, the more (⋯) button, the view mode control and the right panel toggle. The pin button is a ghost icon button (pin icon) that pins the page to the sidebar's Pinned group in one click; when the page is pinned it has the selected look of a choice chip (§3.16: `--line-soft` fill, `--link` icon) and no hover fill, and its tooltip says "Unpin from sidebar" (otherwise "Pin to sidebar"). The ⋯ menu (§3.14) holds Open in graph, Export, Rename, Rewrite references, then a divider and Delete page as the danger item. A page card's ⋯ menu in the right panel (§4.6) keeps Pin to sidebar, because a card has no room for a second button.
- **Title or trail.** In Single mode the top bar shows only the current title. The pages before it are history, not a place the page sits in (pages have no hierarchy), so they are not drawn as a "›" trail. Back's tooltip names the page it goes to ("Back to Atlas Launch"). In Panes mode the top bar shows the trail of open panes (§4.3), because those panes are on screen side by side.
- **Mode label (Vim mode only).** In the right-hand group, left of the view mode control: `NORMAL`, `INSERT` or `VISUAL` in the Label role (Mono 12, UPPERCASE), `--ink-3`, no fill. A pending count or operator follows it, for example `2d` or `"_`.

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
- **Unlinked references** is the other half of "who mentions this page", so it is the same kind of panel, 8px below the linked one: the same well and border. Its top corners are 2px, like the Linked panel's bottom ones, and its bottom corners are 10px (`RADIUS_PANEL_JOIN`, `RADIUS_PANEL`), so the two panels read as one stack. It has a 44px header with a chevron, the heading "Unlinked references" in UI 14/600 `--ink`, and its count badge once it has been searched. It starts collapsed and is searched only when it is opened, so a closed panel has no count. Open, its rows (source page link in Caption `--link`, the mention with its `--highlight-bg` fill, a small "Link" button: Caption `--link`, 1px `--line` border, radius 4) sit inside the panel with 16px side padding, and "Link all" sits on the header's right edge.

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
- Each segment: optional 16px icon, 13px label, optional count in Mono 12: `--muted` at rest and `--ink-3` in the selected segment (`--muted` on `--selected` fails, §2.1).

| State | Look |
|---|---|
| Rest | `--paper` fill, `--ink-3` text |
| Hover (not selected) | `--hover` fill |
| Selected | `--selected` fill, `--ink` text at 500 |
| Focus | Focus ring around the segment (§5) |

### 3.10 Task row and due label

Used by the Tasks view (§4.5).

- Task row: the §3.1 to-do (16px checkbox, 10px gap, Block text on the density's line grid). A done task is `--muted` with a strikethrough.
- Context line (the parent is a plain block): Caption-size 13/20 `--muted` under the task text, aligned with it, 4px tighter to the task above.
- Nested to-do: indented under its parent's checkbox with the §3.1 thread line.
- Due label: after the text on the right, Mono 11 UPPERCASE, round, padding 4px 7px. **Overdue** is `--danger` on `--danger-bg`; **Today** is `--link` on `--link-tint`. The date itself stays in the text as a date link.

| State | Look |
|---|---|
| Hover | `--hover` fill, radius 6, reaching 8px past the text on each side |

### 3.11 Block kinds

A kind changes what is in the row, not the row's grid: the caret slot and the bullet slot stay as §3.1 has them, and the kind keeps its look while the block is edited.

| Kind | Look |
|---|---|
| Heading 1, 2, 3 | Block heading roles (§2.2): 20 / 28, 18 / 26, 17 / 24, weight 600, `--ink`. The bullet stays. No space is added above or below |
| Quote | A 2px bar on the left, 8px before the text, text `--ink-3` italic. No fill. The bar is `--link` today (§6) |
| Code | A card: `--code-bg` fill, 1px `--line` border, radius 6, padding 4 8, the Code role (Menlo 13). When the first line names a language, the name sits at the top right in Caption `--muted`, and the code is coloured in that language at rest and while edited: keywords `--syntax-keyword`, strings `--syntax-string`, numbers `--syntax-number`, function and type names `--syntax-name`, comments `--muted`, everything else `--ink` (bat's grammars, about 200 languages). The code is drawn as written: markup in it (`**`, `[[ ]]`) is not rendered |
| Numbered | The number and a full stop ("3.") in the 24px bullet slot, centred, Block text size, `--muted`. It takes the place of the bullet and is the drag handle |
| To-do | §3.1 |

### 3.12 Inline marks and chips

These are runs inside the block's text, never separate widgets, so they wrap with the line.

| Mark | Look |
|---|---|
| Bold, italic | Weight 700; italic |
| Strikethrough | A 1px line in the text's own color |
| Highlight (`^^text^^`) | `--highlight-bg` fill, text keeps its color |
| Inline code | `--code-bg` fill; the font and size do not change |
| Attribute key (`key:: value` outside the properties bar) | "key: " in weight 700 `--muted` |
| Attachment in the text (`[name](blob:…)`, `{{pdf: …}}`) | Paperclip icon and the name, `--muted`. The file itself is drawn under the block (§3.13) |
| PDF highlight link (`{{highlight: …}}`) | "p. 12" as an underlined `--link`, no arrow: it opens inside the app |

Chips. A chip is a run with a fill; all are `--line-soft` with `--ink-3` text unless the row says otherwise. `--muted` is never used on a chip (§2.1).

| Chip | Text | Look |
|---|---|---|
| Block reference `((id))` | The first 40 characters of the block, then "›" | `--line-soft` fill, `--ink` text. A click opens the block under the row, and closes it again |
| Block embed `{{embed: ((id))}}` | "⧉" and the first 40 characters | The block is always shown under the row (§3.13) |
| Deleted block | "deleted block" | The id is kept, so the reference still says what happened to it |
| Cyclic reference | "↻ cyclic reference" | |
| Missing block | "Missing block ((id))" | `--danger` on `--danger-bg`, for a reference and for an embed |
| Depth | "depth 13" | After the text of a block deeper than 12 levels, Caption size, radius 4, padding 0 4 |

### 3.13 Content under a block

Some marks draw more under their row. It starts at the row's text edge, one level in, so it lines up with the row's children.

- **Embedded block, opened reference:** the block and its children as normal rows, with a `--well` fill and a 2px `--line-soft` bar on the left. At most 3 levels of embeds inside embeds, and 100 blocks.
- **Notice line:** one line in Block text size, `--muted`, when the content cannot be shown: "↻ cyclic reference", "Missing block ((id))", "deleted block", "Query failed: …", "Query syntax: …".
- **Query (`{{query: …}}`):** a header line in Caption `--muted`, "Query {expression} · 4 results" (or "50 of 212"; a query shows at most 50). Then one row for each hit: the page title in `--link` in a 160px column, then the block text; 2px above and below, 8px between the columns. No hits is the header alone with "0 results".
- **Image:** at most 480 × 320, scaled to fit, radius 6, no border, 4px above and below. A click opens it in the system viewer.
- **File chip** (any other file, and an image that has not arrived): `--line-soft` fill, radius 6, padding 2 8, 8px gaps: the icon (image, file or paperclip), the name in Block text `--ink`, then the size in Caption `--ink-3`. A PDF opens in the viewer (§4.8); other files open in the system.

| State | Look |
|---|---|
| Hover on a query hit | `--hover` fill, radius 6 |
| Hover on a file chip | `--hover` fill; only when the file is on this device |
| File not here yet | The same chip with "waiting for sync" in place of the size; no hover, no pointer |

### 3.14 Floating surfaces (menus, popovers, tooltips)

Everything that floats above the page has one look, so it reads as one layer.

- **Surface:** `--well` fill, 1px `--line` border, radius 6, padding 4. No shadow (§2.4). It is drawn above the page (§7 "Drawing order") and takes the clicks that land on it.
- **Place:** 6px from the edge of the piece that opened it (`MENU_OFFSET`), and kept inside the window.
- **Close:** a click on an item, a click outside, Escape, or a second click on the trigger. All four, on every menu.
- Only the palette has a scrim (§3.15).

**Menu item:** the quiet control of §3.4, full width: 28 tall, padding 0 8, radius 6, 13px `--ink-3`. Items have a 2px gap.

| State (menu item) | Look |
|---|---|
| Hover | `--hover` fill, `--ink` text |
| Keyboard row | `--selected` fill, `--ink` text; a second line on it is `--ink-3` |
| Current choice (a menu that picks one) | A check icon in `--link` at the right edge |
| Danger ("Delete page") | `--danger` text; hover is a `--danger-bg` fill |
| Divider | 1px `--line`, 4px above and below |

The menus:

- **Page menu** (⋯ in the top bar, §3.3; "···" in a compact pane): 190 wide (`PAGE_MENU_WIDTH`), under the trigger's right edge.
- **Reference date and sort menus** (§3.7) and the **search age menu** (§3.15): 160 wide (`REFS_MENU_WIDTH`).
- **Autocomplete** (`[[`, `((`, `#`) **and the slash menu** (`/`): one menu, 340 wide, 4px under the line the caret is on.
  - Row: padding 4 8, radius 4. The label is Block text `--ink`; a second line in Caption `--muted` gives "alias: Trellis" for a page found by its alias, or the page title for a block.
  - Rows: up to 8 pages, then a relative date ("tomorrow") when the text is one, then `Create "x"` with `⌘↵` (not for `#`). Up to 8 blocks for `((`. A slash row starts with "/ ".
  - Nothing to show draws nothing, and the menu then takes no keys.
  - Keys: ↑ and ↓ move and wrap, Enter or Tab accepts, ⌘Enter creates the page. The first Escape closes the menu; the second leaves the block.
- **Sync popover:** §3.17. **Graph hover card:** §4.7.

**Tooltip:** the same surface, padding 4 8, Caption 12 `--ink`, at most 240 wide (`TOOLTIP_MAX_WIDTH`). Every icon-only button has one (§2.5).

**Drag preview** (§3.1): the same surface, padding 4 8, Block text `--ink`.

### 3.15 Palette

One panel does five jobs; the title line says which: "Quick jump" (⌘K, or the sidebar search field), "Search" (⇧⌘F), "Quick capture → today" (⇧⌘Space), "Redirect link to…" and "Extract block to page".

- **Scrim:** `--scrim` over the whole window. A click on it closes the palette.
- **Panel:** centred, 90px from the top of the window. 520 wide, 860 in Search. At most 440 tall, less in a short window. `--well` fill, 1px `--line` border, radius 8, padding 8, 4px between its parts.
- **Title line:** Caption `--muted`.
- **Input:** the text field of §3.16, in Block text size.
- **Scope bar** (Quick jump and Search; not while typing a `>` command): choice chips All, Journal, Pages, To-dos (§3.16), and the age menu "Any time" at the right (§3.14).
- **Result row:** padding 4 8, 8px gaps, radius 4: a 14px icon slot (page, day, tag, command, plus), the label in `--ink` with the typed letters in 600, then a detail at the right in Caption `--muted`: "Page", "Day", "Tag", "Alias", "Today", "Yesterday", or a command's keys.
- **Create row:** plus icon and `Create "x"` in `--link`, always last, under a 1px `--line` divider. Only where a new page makes sense (Quick jump, scope All or Pages).
- **Empty input:** Quick jump lists Today, Yesterday and the 8 most recent pages. The other jobs list nothing.
- **No result:** one line, "No results", Caption `--muted`.
- **Footer:** 1px `--line` above it, Caption `--muted`, the keys for this job ("↑↓ move · ↩ open · ⌘↩ open beside · esc").
- **Quick capture and Extract:** the title, the input and the footer only. Enter saves; an empty input does nothing.

Search adds:

- **Groups:** one for each page that has hits: a header (clock icon for a day, page icon for a page; the title in `--ink`; the count badge of §3.4), then its block rows under the title, two lines at most each.
- **A hit in the text:** `--highlight-bg` fill (§2.1).
- **Preview** of the row the keyboard is on, at the right: 40 % of the panel (240 to 340), a 1px `--line` on its left. The path in Caption `--muted`; a state chip that names the block's kind or "Done" (`--line-soft` fill, radius 6; `--ink-3` for a plain block or a done to-do, `--link` for the other kinds); "Edited Nov 18, 2026" in Caption `--muted`; the block in `--ink`, six lines at most; then its children.

| State (row) | Look |
|---|---|
| Hover | `--hover` fill |
| Keyboard row | `--selected` fill; the detail becomes `--ink-3`. It stays `--selected` under the pointer |
| A command that cannot run now | `--muted` label, no hover, no pointer; Enter does nothing |

Keys: ↑ and ↓ move and wrap, Enter opens, ⌘Enter (or a ⌘-click or Shift-click) opens beside, Escape closes.

### 3.16 Form controls

Used by Settings (§4.9), the palette (§3.15), the reference filter (§3.7) and the start screens (§4.10). There is no filled "primary" button in Utopia: an action is a quiet control or an outline button (§3.4), and the order on the row says which comes first.

**Text field.** One frame everywhere: `--paper` fill, 1px `--line-strong` border, radius 8, 36 tall, padding 0 12, UI text (the palette uses Block text). A label, when there is one, sits above it in Small `--ink-3`, 4px away; a note sits below in Caption `--muted`. A secret (token, passphrase) shows one mark for each character, never the text.

| State (text field) | Look |
|---|---|
| Placeholder | `--muted` |
| Focus | 1px `--link` border; caret and selection as §3.1 |
| Error | 1px `--danger` border, and the reason below in Caption `--danger` |
| Disabled | `--well` fill, `--muted` text, no caret |

**Choosing one of a few.** Three pieces, by what the options need:

| Piece | Use | Rest | Hover | Chosen |
|---|---|---|---|---|
| Segmented control (§3.9) | 2 to 4 short labels that fit on one line (view mode, Tasks filter, sync mode, access level, children shown) | §3.9 | §3.9 | §3.9 |
| Radio row | Options that need a sample or a line of help (font, density, indent). A row of a Settings card | A 14px round mark, 1px `--line-strong`; label; help in Caption `--muted` | `--hover` fill | Mark has a `--link` border and an 8px `--link` dot; the row has the `--selected` fill and its help is `--ink-3` |
| Choice chip | A row of filters that wraps (palette scope, Help topics, graph mode) | Quiet control (§3.4) | `--hover` fill, `--ink` text | `--selected` fill, `--ink` text |

**Switch** (one setting, on or off): a 34 × 20 round track with a 16px `--paper` knob, 2px in from the edge. Off: `--line-strong` track. On: `--link` track. The whole row is the target.

**Key chip** (a shortcut in Settings): the kbd chip (radius 4, `--hover` fill, Mono 12 `--ink`).

| State (key chip, Settings → Keybindings) | Look |
|---|---|
| Hover | 1px `--link` border |
| Recording | 1px `--link` border, `--banner-bg` fill, the text "Press keys · Esc cancels" in `--link` |
| Not a shortcut | The text typed, in `--warning`, and "Not a shortcut: …" below in Caption `--warning` |
| Clash | "Also used by …" below in Caption `--warning`, the names underlined; the other row has a `--banner-bg` fill for 1.8 s |
| Changed from the default | An undo icon button before the chip puts the default back |

**Copy-ready text** (the Assistants setup entry): the Code role in `--ink-2`, no fill, 8px under its label row, which holds a quiet "Copy" control that reads "Copied" after a click.

### 3.17 Sync status, banners and notices

**Sidebar footer** (§3.2). The dot and the vault name are one button that opens the sync popover; an encrypted graph adds a lock icon in `--ink-3` with the tooltip "Encrypted at rest".

| State | Dot | Text after the name |
|---|---|---|
| Local only (sync is off) | `--disabled` | "· Local only" in `--muted` |
| Syncing, connecting | `--warning` | none |
| In sync | `--success` | none |
| A problem (offline, some pages not sent, a local error, sign-in needed, relay not supported, refused) | `--danger` | none |
| Hover | `--nav-hover` fill behind the button, radius 6 | |

**Sync popover:** a floating surface (§3.14), 232 wide (`SYNC_POPOVER_WIDTH`), padding 8, above the footer. Lines, top to bottom: "Sync" in `--ink` 500; the state in words ("Synced 2m ago", "Offline — 3 changes queued"), in `--warning` when it is a problem; the target ("Relay · https://…", "Folder · /…"); "Encrypted at rest." or "Not encrypted at rest."; and, when sync is off, "Set it up in Settings → Sync." in `--muted`. It has no controls: setup is in Settings.

**Banner:** a `--banner-bg` strip across the main area, under the top bar (never above it: the top band stays level with the traffic lights, §3.18): one centred line in `--ink`, 4px above and below, no border and no close button.

- New day: "It's a new day → open today". The whole strip is the button. It goes when the feed opens.
- Merge: "Merged changes from the relay into 3 pages", for 5 seconds. Not a button.

**Footer notice:** one line under the footer row. A result is `--ink` ("Exported 42 pages to …"); a failed action is `--danger`. A new one takes the place of the last.

**Settings strip:** a full-width line at the bottom of the Settings window, 1px `--line` above it, padding 8 24, Caption size: a warning is `--warning` on `--banner-bg`; a status is `--muted` on `--well`.

### 3.18 Window frame (title bar)

The system title bar is transparent: there is no separate strip with the window title. The traffic lights draw over the app's own **top band**, the 52px row that the top bar (§3.3) and the sidebar's workspace row (§3.2) share. The title is still set, so the Window menu and Mission Control name the window. The main window and Settings both work this way (`chrome::titlebar`).

- **Where the lights are:** the close button's top left is at 20, 19 (`TRAFFIC_LIGHT_X`, `TRAFFIC_LIGHT_Y`), so the 14px buttons are centred on the band and start on the sidebar's content edge. They take 64 wide (`TRAFFIC_LIGHTS_WIDTH`: three buttons 23 apart, plus 4).
- **Room for them:** whatever is under them starts its first control 12px after the zoom button, at x 92: the workspace square when the sidebar is open; Back (or the view's title in the graph and PDF views) when it is collapsed. The graph and PDF headers are then as tall as the top bar.
- **Collapsed sidebar:** the rail's top 52px is the top bar carried to the window's edge (`--paper`, 1px `--line-soft` under it), so the lights never sit across the rail's border. "»" sits under it.
- **Full screen:** macOS hides the lights, so the room goes: the workspace square and Back go back to their own padding.
- **Settings:** the rail's top 52px is left for the lights; the "Settings" caption starts under it.
- **Title bar actions:** the empty parts of the band (the lights' room, the workspace name, the space in the top bar after the title, the rail's top) take a double-click as a title bar does (zoom or minimise, as the user set it in System Settings). Buttons in the band keep their own clicks.

| State | Look |
|---|---|
| Window focused | The system's coloured lights |
| Window not focused | The system's grey lights; nothing of ours changes |
| Dark theme | The lights on `--shelf` (sidebar) or `--paper` (top bar); no token changes |
| A banner is showing (§3.17) | The banner is under the top bar, so the band does not move |

## 4. Screens

### 4.1 Daily Notes

Sidebar plus main area. The main area has the top bar and the content column. Days are listed newest first, in the order: day section (§3.5), linked references (if any, inside that day's section, §3.7), then the next day. Consecutive empty days are grouped into empty day rows (§3.6). Older days load as you scroll; when the oldest day is loaded the feed ends with "No earlier notes." in `--muted`. Scroll position resets to today when you click "Today". Days have no Unlinked references in the feed; those stay on the day's own page.

### 4.2 Page view

Sidebar (the page is highlighted in Pages) plus main area: top bar with the page's title (§3.3), page title (Page title style), properties bar, body blocks (placeholder "Click to start writing" in `--muted` with a `--line-strong` bullet when the body is empty), then the Linked references panel 48px below, then the Unlinked references panel 8px below that (§3.7).

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

### 4.6 Right panel

A second column at the right of the main area, for a page, a block or the references of the current page, next to what you are writing.

- **Open and close:** the top bar toggle (tooltip "Toggle right sidebar", ⇧⌘\), a Shift-click on a link, "Toggle References" (⇧⌘B), or "Around {page}" from the graph. It starts closed.
- **Column:** 380 wide by default, 240 to 640 (`right_sidebar_width` in `settings.json`), full height, `--well` fill, 1px `--line` on its left. A 4px strip on that edge resizes it: no fill at rest, `--link` on hover and while dragging (§6); the width is saved when the drag ends.
- **Header:** one row with a close icon button at the right.
- **Order, top to bottom:** the local graph (§4.7, 400 tall), the page cards, then the inspector.
- **Page card** (the newest first): a header row with a fold chevron, the title in Caption 500 `--muted` and a close icon button; under it the page as a compact page view (22px indent step, 90px property keys). An open card takes the free height, at least 140; a folded card is its header. 1px `--line` between cards.
- **Inspector** (for the page in focus): fills the panel when there are no cards, otherwise 260 tall. Choice chips (§3.16) pick References, Outline or Pinned.
  - References: chips Incoming, Outgoing, Unresolved, each with its count. Incoming is the groups of §3.7 in the Reference role. Outgoing is one row for each linked page: Small `--ink`, with the count. Unresolved is one entry for each link whose page does not exist: the title in `--danger`, then quiet controls "Create page", "Redirect…", "Show as text".
  - Outline: one row for each heading block, in its heading role, indented 10px for each level.
  - Pinned: a quiet "Pin this page" or "Unpin this page", then the pinned pages, each with a close icon button.
- **Empty lines:** "Shift-click a link to open it here." (no cards, inspector off), "No references yet.", "No unresolved references", "No headings in this page.", "No pinned pages yet.": Caption `--muted`, padding 4 8.

| State | Look |
|---|---|
| Hover on a row or a card header | `--hover` fill, radius 6 |
| Hover on a close button inside a row | `--selected` fill (§3.4) |
| Toggle while the panel is open | `--selected` fill, the same as Filter while its controls are open (§3.7) |

### 4.7 Graph view

Pages as dots, links as lines. It takes the main area (⇧⌘G, or "Open in graph" in the page menu); Escape or opening a page gives the pane stack back. A smaller copy, "Around this page", sits at the top of the right panel (§4.6).

- **Top bar** (§3.3): back button, the title "Graph" in `--ink` 500, then choice chips (§3.16) "All pages", "Around {page}" and "Journal pages".
- **Canvas:** `--paper`.
- **Node:** a circle, 6 to 18 across by how many links the page has. A page is `--bullet`, a daily page is `--tag-ink`. The page the graph is centred on has a 2px `--link` ring, 2px outside it.
- **Label:** under the node, centred, Caption `--muted`. Shown for the node under the pointer, the centre page and its neighbours, and for as many of the rest as the zoom leaves room for.
- **Link:** a `--line` stroke, 1, 2 or 3px by how many references it stands for, with an arrowhead at the target.
- **Legend,** bottom left, 12px in: Caption `--muted`: a dot for "Pages", one for "Journal pages", a ring for "Current page", then "128 pages · 342 links".
- **Zoom,** bottom right: quiet controls "−", "+" and "Fit". The range is 0.25 to 8.
- **Empty:** "No links yet. Type [[ in a block to link pages." in `--muted`, centred.

| State | Look |
|---|---|
| Hover on a node | The node, its links and their arrowheads turn `--link`; its label turns `--ink`; its neighbours stay; everything else drops to 25 % opacity. The pointer cursor |
| Hover card | A floating surface (§3.14), 240 wide, padding 8, 12px right of and 14px under the pointer: the title in UI 500 `--ink`, then "12 links" and the page's first block in Caption `--muted` |
| Click | Opens the page. Shift-click opens it in the right panel, ⌘-click as a pane |

### 4.8 PDF viewer

A PDF attached to a block opens in the main area (macOS). Escape or "Close" goes back.

- **Ground:** `--well`.
- **Header:** padding 8 16, 1px `--line` under it: the file name in `--ink` 600, then "12 pages · 3 highlights · drag over a page to highlight" in Caption `--muted`, and at the right the quiet controls "Open in Preview" (when the file is on this device) and "Close".
- **Page:** 720 wide, as tall as the page's shape gives, centred, 8px above and below, `--pdf-paper` fill, 1px `--line` border. The crosshair cursor.
- **Highlight:** `--pdf-highlight` rectangles, one for each line of the quoted text. The pointer cursor; a click opens the highlight's block in the right panel (§4.6).
- **While selecting text:** `--pdf-selection` rectangles. **While dragging an area** (Option-drag): a 1px `--link` border with the `--pdf-highlight` fill.
- **Cannot show:** one centred line in `--muted`: "This PDF has not synced to this device yet." or "This PDF cannot be shown here; use Open in Preview."

### 4.9 Settings window

A second window (⌘,), 1100 × 760, at least 900 × 520, titled "Settings". Pressing ⌘, again raises it; it never opens twice.

- **Rail:** 220 wide, `--shelf`, 1px `--line` on its right, padding 16 12. The caption "Settings" in Caption 500 `--muted`, then one nav row (§3.2: icon, label, the same hover and active states) for each tab: Appearance, Keybindings, Shortcuts, Sync, Assistants, Help.
- **Pane:** `--paper`. The tab's name in the Section heading role, 24px from the top and the sides. The content is one column, at most 720 wide, centred, with 16px between its parts. Only the content scrolls; the rail and the heading stay.
- **Card:** a group of rows. A caption row (Caption 500 `--muted`, `--well` fill, padding 12 16 6), then the rows, 1px `--line` between them and around the card, radius 10. 16px between cards.
- **Row:** padding 8 16 (in the Keybindings list, 11 16, and 7 16 in Tight). The label in Block text, a line of help under it in Caption `--muted`, and the control at the right. A row that can be clicked has the `--hover` fill on hover.
- **Strips** at the bottom of the window: §3.17.

The tabs:

- **Appearance:** cards Font, Density and Indent (radio rows, §3.16; a font's sample line is set in that font; an indent's help is "45 px per level"), Editor ("Smooth caret movement" switch, §2.6), References ("Children shown", a segmented control 0 to 3), Pinned pages (how many, and a quiet "Clear"), Right sidebar (a quiet "Reset to 380 px").
- **Keybindings:** a "Vim mode" row above the list: the label, the help "Modal editing. Normal mode for motions and commands, Insert mode to type.", and a segmented control Standard | Vim (§3.16); then a line of help ("Click a key, then press the new one.") with a quiet "Reset All" at the right; a filter text field ("name, description or key"); then one card for each category, one row for each action with its key chip (§3.16). "No match" is one line in Caption `--muted`.
- **Shortcuts:** the same filter and cards, to read, not to change: the keys are Mono 12 `--muted` at the right of each row.
- **Sync:** the mode (a segmented control: Off, Relay, Nearby Mac, Shared folder); the text fields the mode needs (address, token, passphrase, folder; the two secrets are masked, and a blank one keeps the saved value); then "Apply" (it reads "Working…" while it runs), "Test connection" and "Connect another device"; then one status line in `--muted`. A notice in `--warning` names the environment variables that override the saved settings; Apply is disabled while they do.
- **Assistants:** the access level (a segmented control: Off, Read, Read and write), the switch that lets an assistant work while the app is closed (`when_closed`), one row for each client (Claude Code, Claude Desktop, Codex) with its copy-ready entry (§3.16), and whether the bridge runs, in Caption `--muted`.
- **Help:** choice chips for the topics; then the topic's title (Section heading), a summary in `--muted`, its paragraphs in Block text, its shortcuts (key chip and a label in Caption `--muted`) and a screenshot: 640 wide, radius 6, 1px `--line`.

### 4.10 Start screens

What the window shows before a graph is open. The whole window is `--paper`, with no sidebar and no top bar, and one column 420 wide in the centre, 8px between its parts.

- **New graph:** "Welcome to Utopia" (Section heading); a text field (§3.16) with the placeholder "Name your graph"; "Creates ~/Documents/Utopia/<name>.utopia" in Caption `--muted`. Enter creates it; there is no button. An empty name puts the field in its error state: "Please enter a name."
- **Locked graph:** "This graph is locked" (Section heading); "Notes.utopia is encrypted at rest." in `--muted`; a masked text field with the placeholder "Passphrase"; a note in Caption `--muted` that a passphrase saved in the keychain unlocks it without asking. A wrong passphrase puts the field in its error state: "That passphrase does not open this graph."
- **Cannot open:** one centred line in `--danger`: "This graph is already open in Utopia", or "Could not open {path}: {reason}".

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

## 6. Open questions and known gaps

### 6.1 Decisions not made yet

- Status colors beyond "in progress" (done, blocked, and so on) aren't defined yet. Add them as `--status-*` tokens and check each one's contrast before use.
- **Blue on things that are not links.** Principle 2 keeps `--link` for links and a short list of marks. Three built pieces use it outside that list: the bar of a quote block (§3.11), the resize strip of the right panel (§4.6) and the line that shows where a dragged block lands (§3.1). Either add them to the principle's list or give them a neutral token.
- **The sync dot is color only.** Except for "Local only", the footer names no state in words (§3.17); the words are one click away in the popover. Decide if the footer needs a word for a problem.
- **A link inside a highlight in dark mode** is 3.78:1 (`--link` on `--highlight-bg`). It is rare, so the token is unchanged; a darker dark `--highlight-bg` would fix it at the cost of a fainter highlight.
- **The slash menu has no height limit.** It lists every command, template and plugin command with no scroll. Decide a maximum and how it scrolls.
- **Inline code keeps the block's font.** Only the fill marks it (§3.12). Decide if it takes the Code role.

### 6.2 Specified here, built differently

The sections above say what each piece is. The app does not match these yet. Each line is a change to the code, not to this file; take the line out when the code matches.

| Rule | What the code does today | Where |
|---|---|---|
| Focus ring (§5) | `FOCUS_RING` is defined and drawn nowhere; no text field has a focus, error or disabled look | everywhere |
| Chosen = `--selected` fill, `--ink` text (§2.1) | Chosen is a `--line-soft` fill with `--link` text, or `--hover` with `--link` | Settings rail and radio rows, "Children shown", Help topics, access level, palette scope chips and age menu, graph chips, the right panel's inspector tabs |
| Segmented control for 2 to 4 short options (§3.16) | Separate buttons with a `--link` border on the chosen one; number boxes | Settings → Sync mode, Assistants access level, "Children shown" |
| One text field (§3.16) | Four frames: `--line` border and radius 6 (palette, Sync), `--well` fill and radius 8 (Settings filter), a `--link` border at rest (start screens) | `quick_jump.rs`, `settings.rs`, `sync_setup.rs`, `utopia-app/src/main.rs` |
| Buttons are §3.4 | Sync has its own button: `--line-soft` fill, `--link` text, no hover. The right panel closes with a text "×" | `sync_setup.rs`, `workspace.rs` |
| Icons, never text characters (§2.5) | "×", "▾ ▸", "‹ ›", "···", "»", "✓", "↺", "⌘", "#", "+", "◷ ▤", "☑ ☐ •", "📎 🖼 📄", "Aa ☰ ↻ ⇄ ?" | right panel, daily page calendar, compact page menu, collapsed sidebar, menus, Settings, palette, attachments |
| Key chip is the kbd chip; shortcuts are Mono (§3.16) | A second chip (radius 6, `--line-soft`, Sans); keys as Sans text | Settings → Keybindings, Shortcuts, Help; palette command rows |
| Code block fill is `--code-bg` (§3.11) | `--well` | `page_view.rs` |
| A search hit is `--highlight-bg` (§3.15) | `--link` at 16 % | `quick_jump.rs` |
| A missing embed is `--danger` on `--danger-bg` (§3.12) | `--line-soft` with quiet text | `rich.rs` |
| Menu item is the quiet control; every menu closes four ways (§3.14) | The reference date and sort menus use 14px `--ink` items; they, the search age menu, the sync popover and the compact page menu do not close on Escape or on a click outside. The age menu is 170 wide | `page_view.rs`, `quick_jump.rs`, `workspace.rs` |
| States in the tables of §3.1, §3.14, §3.15 | Not drawn: the underline on a hovered page link, the fill of a selected block, hover on an autocomplete row, the "No results" line | `rich.rs`, `page_view.rs`, `editor.rs`, `quick_jump.rs` |
| Create row is `--link` (§3.14, §3.15) | `--ink` in autocomplete | `editor.rs` |
| A notice that is not a link is `--warning` or `--muted` (§4.9) | `--link` text for the override notice, the saved-settings notice and the Assistants notice | `sync_setup.rs`, `assistants.rs` |
| The graph has the top bar of §3.3; the PDF header is weight 600 (§4.7, §4.8) | Their own header rows, weight 700, no back button | `graph_view.rs`, `pdf_view.rs` |
| A row hover is radius 6 (§2.4) | No radius on a query hit; radius 4 in the right panel | `page_view.rs`, `workspace.rs` |
| The right panel toggle is `--selected` while open (§4.6) | The `--hover` fill, the same as its hover | `workspace.rs` |
| Card radius 10 (§2.4, §4.9) | 8 | `settings.rs` |
| One calendar look (§3.2) | The calendar on a daily page has 26 × 22 cells and marks today with a `--line-soft` fill and `--link` text | `calendar.rs` |
| Use a role, never a bare size (§2.2) | These take the block size by naming none: the sync popover, the banners, the footer notice, the graph and PDF headers, the start screens. The Help title is weight 700 | `workspace.rs`, `graph_view.rs`, `pdf_view.rs`, `settings.rs`, `utopia-app/src/main.rs` |
| A dimension is a `theme.rs` constant (§2) | Local numbers: the palette's 520, 860 and 90; autocomplete 340; image 480 × 320; PDF page 720; Settings 1100 × 760, 220 and 720; the start column 420; the switch, the radio mark | the files above |

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
| `accent-color` on a checkbox | Draw the checkbox: 16px, radius 4, `--line-strong` border, `--link` fill and a `--paper` check when done (§3.1) |
| `aria-*`, landmarks, accessible labels | No accessibility tree. Use tooltips for icon-only buttons; keep the keyboard rules in §5 |
| `text-overflow: ellipsis` | `.truncate()` on a single-line element; a run of styled text adds its own "…" |
| A transition | Only the motion tokens in §2.6 |
| URL state | Device-local state (`local_state`), never the CRDT (architecture rule 9) |
