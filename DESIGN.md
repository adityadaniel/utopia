# Utopia Design Spec ("Quiet paper")

Utopia is a desktop outliner. This file covers the visual system and the main screens: the Daily Notes feed, a page view, the sidebar, and the stacked panes that hold them (§4.3, §4.4). Build it to match what's written here. If something isn't covered, pick the option that fits the principles below.

## 1. Principles

1. **Writing first.** Body text is 16px with generous line height. Chrome (sidebar, toolbars) never competes with the notes.
2. **One loud color.** Blue means "this goes somewhere" (page links, date links). Nothing else uses it, except focus rings, the active nav icon, the today marker and checked checkboxes.
3. **Quiet structure.** Nesting is shown with thin thread lines and spacing, not boxes. Dividers are 1px warm gray.
4. **One source of truth.** Anything shown on screen is the block data itself, drawn differently. Never show the same data twice (see Properties bar).

## 2. Tokens

Use these as CSS custom properties, or the equivalent in your stack. Don't hardcode values outside this list.

### 2.1 Color

| Token | Hex | Use |
|---|---|---|
| `--paper` | `#FFFFFF` | Main writing surface |
| `--shelf` | `#F6F6F4` | Sidebar background, property row hover |
| `--well` | `#FAFAF8` | Reference panels |
| `--hover` | `#F1F1EE` | Ghost button hover, kbd background |
| `--selected` | `#E9E9E5` | Active sidebar item |
| `--nav-hover` | `#EDEDEA` | Sidebar item hover |
| `--line` | `#E8E7E3` | Borders, thread lines, sidebar edge |
| `--line-soft` | `#EFEEEA` | Dividers between days and sections, count badge background |
| `--line-strong` | `#E1E0DB` | Input and button borders, chip borders |
| `--ink` | `#1B1B19` | Primary text |
| `--ink-2` | `#3A3A35` | Nav text, reference text |
| `--ink-3` | `#57574F` | Secondary text, icons |
| `--muted` | `#6F6F68` | Labels, placeholders, counts |
| `--bullet` | `#8A8A83` | Bullets and carets only. **Never use for text** (fails contrast) |
| `--disabled` | `#B9B8B1` | Disabled icon strokes, offline dot |
| `--link` | `#2D5BD2` | Page links, date links, focus, today |
| `--link-hover` | `#2249B0` | Link hover and pressed |
| `--link-tint` | `#2D5BD2` at 10% alpha (`#EAEFFB` solid) | "Today" pill background |
| `--tag-bg` | `#E9ECF4` | Tag chip fill, avatar fill |
| `--tag-ink` | `#34415F` | Tag chip text, avatar initials |
| `--status-progress` | `#C27A12` | Status dot for "in progress" (only status color so far) |

Rules:
- Every text and background pair must reach at least 4.5:1 contrast. The pairs above already pass. Check any new pair before using it.
- Page links and tags must look different in shape, not only color: links are plain blue text, tags are slate chips.
- No dark mode yet. When you add one, define the same token names again; don't add new ones.

### 2.2 Typography

Fonts: **IBM Plex Sans** (400, 500, 600) for everything people read. **IBM Plex Mono** (400, 500) for small labels, counts, shortcuts and weekday labels. Bundle both with the app. Don't load them from the web at runtime.

| Role | Font | Size / line height | Weight | Other |
|---|---|---|---|---|
| Page title | Sans | 36 / 44 | 600 | letter-spacing -1.5% |
| Day title | Sans | 30 / 38 | 600 | letter-spacing -1% |
| Section heading | Sans | 20 / 28 | 600 | |
| Empty day title | Sans | 17 / 26 | 600 | |
| Block text | Sans | 16 / 28 | 400 | |
| Reference block text | Sans | 15 / 26 | 400 | |
| UI text (nav, buttons, search) | Sans | 14 / 22 | 400 or 500 | |
| Small (meta, footer, property keys) | Sans | 13 / 20 or 12 / 18 | 400 | |
| Label | Mono | 12 / 16 (11 in sidebar) | 400 or 500 | UPPERCASE, letter-spacing +6% |
| Count, kbd | Mono | 12 | 400 | |

Turn on antialiased font smoothing.

### 2.3 Spacing

4px base. Allowed values: 4, 8, 12, 16, 20, 24, 32, 40, 48, 56, 64, 96.

- Content column: max width **760px**, centered, padding 48px top (56 on a page view), 40px sides, 96px bottom.
- Space between days: 40px above and below, with a 1px `--line-soft` divider.
- Sidebar: 264px wide (min 240), 12px padding.
- Top bar: min height 52px, padding 8px 20px.

### 2.4 Radius

| Value | Use |
|---|---|
| 4 | kbd, ref-type label, small chips, checkboxes |
| 5 | Tag chips |
| 6 | Nav items, ghost buttons, property rows |
| 8 | Inputs, search field, button groups |
| 10 | Panels (Linked references), count badges are fully round |
| 13 | Status pill (fully round) |

No shadows anywhere. Use borders instead. The only exception is the inset focus ring on a property row being edited.

### 2.5 Icons

16px line icons, 2px stroke (1.75 for the 18px panel icons), round caps and joins, color `--ink-3`. Use one consistent set (Lucide matches well). Icons needed: search, calendar, page, person, sidebar-left, sidebar-right, arrow-left, arrow-right, chevron-down, chevron-right, chevron-left, plus, filter, star, settings, list (aliases), dashed-circle (status).

Icon-only buttons must have an accessible label.

## 3. Components

### 3.1 Block (outliner row)

- The row height follows a **28px line grid**. Bullets, carets and checkboxes are vertically centered within the first 28px line.
- Layout from left: caret slot (20px, sits in the left gutter outside the text column) → bullet slot (24px) → content. A top-level bullet's left edge sits on the page title's left edge; the text starts 15px further in (the rest of the bullet slot).
- Bullet: 6px circle, `--bullet`.
- Collapsed block: the bullet gets a 12px `--line-strong` ring around a 6px `--ink-3` dot, and the caret points right.
- Caret: 12px chevron, `--bullet`. Shown on blocks that have children. Can be shown only on hover if you prefer, but must always be there for keyboard and screen reader users.
- Children: indent by `margin-left: 31px; padding-left: 13px;` with a 1px `--line` left border (the thread line). The thread line lines up under the parent bullet's center. Settings → Appearance → Indent offers a 26px step instead of the 45px; the thread line follows.
- Todo: a real checkbox, 16px, `accent-color: --link`, with 10px gap before the text. Done todos: text `--muted` with a strikethrough.
- Inline page link: `--link`, no underline, underline on hover.
- Date link: same as page link (dates are pages).
- Tag: inline chip, height 22, padding 0 7px, radius 5, `--tag-bg` fill, `--tag-ink` text, 14px.
- Ref type label (for example "Scheduled for this day"): height 20, padding 0 7px, radius 4, 1px `--line-strong` border, Mono 11px `--ink-3`, white fill.

### 3.2 Sidebar

From top to bottom:

1. **Workspace row:** 24px black square with the initial in white, the workspace name (15/600), a chevron-down for switching workspaces, and a collapse-sidebar icon button on the right.
2. **Search field:** a button that opens the command palette. Height 36, radius 8, white fill, `--line-strong` border, search icon, placeholder text "Search or jump to" in `--muted`, and `⌘K` kbd on the right.
3. **Nav:** Daily Notes, All pages. Each row is 34px tall, radius 6, 14px text. Active row: `--selected` fill, `--ink`, weight 500, icon in `--link`. Hover: `--nav-hover`.
4. **Mini calendar:** month label (13/600) with prev/next buttons. A 7-column grid, weeks start on Sunday, weekday labels in Mono 11. Day cells are 30px tall. Today: solid `--link` fill with white text. Days with notes: a 4px `--link` dot under the number. Clicking a day opens that daily note.
5. **Pages:** Mono label "PAGES", then rows 32px tall with an icon (person icon for people pages, page icon for others), the name, and the reference count in Mono 12 `--muted` on the right.
6. **Tags:** Mono label "TAGS", then tag chips in a wrapping row. Tags are listed here, **not** in Pages.
7. **Footer** (pinned to the bottom, 1px top border): an 8px status dot (`--disabled` when local only), the vault name in `--ink` 500, " · Local only", and a settings icon button.

The old "Recent days" list is removed on purpose, since it repeated what the feed already shows.

### 3.3 Top bar

Back and forward icon buttons (forward is `--disabled` when there's nothing to go forward to), a title or breadcrumb, then actions on the right.

- Daily Notes: title "Daily Notes", then a day navigation group (prev | Today | next) with a 1px `--line-strong` border, radius 8, 32px tall, and a toggle for the right panel.
- Page view: breadcrumb "Pages / Atlas Launch" (parent in `--ink-3`, current page in `--ink` 500), then a favorite (star) button, a more (⋯) menu that holds Delete page, and the right panel toggle.

1px `--line-soft` bottom border.

### 3.4 Buttons

- **Ghost icon button:** 32×32, radius 6, transparent background, `--hover` on hover.
- **Ghost text button:** 28 to 32px tall, padding 0 8 to 10px, 13px `--ink-3`, optional leading plus icon. Examples: "Write", "Add property", "Add alias".
- **Outline button:** same, plus a 1px `--line-strong` border and white fill. Example: "Filter".
- **Today pill:** height 20, padding 0 7px, fully round, `--link-tint` fill, `--link` text, Mono 12 weight 500.
- **Count badge:** Mono 12 `--ink-3`, `--line-soft` fill, padding 1px 8px, fully round.

Every interactive element must be a real button, link or input so keyboard Tab works. Focus ring: 2px `--link` outline with 2px offset.

### 3.5 Day header (Daily Notes)

- Weekday label above the title: Mono 12 UPPERCASE `--muted` (for example "FRIDAY"). On today's date, add the Today pill after it.
- Title: the page name exactly as stored (for example "November 6th, 2026"), Day title style. Don't reformat it, since it's also the page name that links point to.
- 16px gap below the title before the first block.

### 3.6 Empty day row

A day with no blocks is **not** a full section. It's one row at least 44px tall: weekday label (Mono, fixed 92px wide), date title (17/600, takes the remaining width), and a ghost "Write" button. Clicking "Write" (or the title) expands it into a normal day with one empty block that has focus. Several empty days in a row stack together with 4px gaps, inside one block that has a single divider above it.

### 3.7 Linked references panel

- Container: 1px `--line` border, radius 10, `--well` fill.
- Header row (padding 12px 16px): "Linked references" heading plus a count badge. On a page view, an outline "Filter" button sits on the right. Inside Daily Notes, the header is a full-width button with a chevron that collapses the panel, and the heading is 13/600 `--ink-2`.
- Groups: one per source page, separated by 1px `--line` dividers, padding 14px 0. The source page link is 14/500 `--link`, optionally followed by a ref type label.
- Referenced blocks render with the Reference block text style. Children keep their thread line. The current page's name inside a reference is shown as **bold `--ink` text, not a link** (linking to the page you're already on is pointless).
- "Unlinked references" sits below as a collapsed row at least 44px tall with a 1px top divider and a chevron-right. Content is searched only when the row is expanded.

### 3.8 Properties bar (page view)

**Data model, read this first.** Properties are ordinary blocks (for example `status: in progress`). The properties bar is **not** a separate table that copies them. It is the page's leading property blocks, drawn as rows instead of bullets.

Rules:
1. Property blocks are the **first consecutive blocks** at the top of a page. The bar renders exactly those blocks. The first non-property block ends the bar.
2. Property blocks are **never** also rendered as bullets in the outline. They render once, in the bar.
3. A property block found lower in the outline (not at the top) is just a normal block and is drawn as a normal bullet. It does **not** show up in the bar.
4. If a page has no leading property blocks, the bar shows only the "Add property" row.

Layout:
- Placed 20px below the page title. Top and bottom 1px `--line-soft` borders, 6px vertical padding.
- Each row: a 2-column grid, `140px | 1fr`, at least 36px tall, radius 6. Hover: `--shelf` background (the row extends 8px past the content on each side through negative margin, so the text stays aligned with the title).
- Key column: a 15px icon plus the key in 13px `--ink-3`, lowercase as typed.
- Value column, drawn by type:
  - `status`: a pill, 26px tall, padding 0 10px, fully round, 1px `--line-strong` border, white fill, a 7px status dot, 13px `--ink`. The value is title-cased for display only.
  - Page link values (for example `owner`): a 22px round avatar with initials (`--tag-bg` / `--tag-ink`, 10px 600) plus the link in 14px `--link`.
  - Anything else: plain 14px `--ink`, with links drawn as links.
- `aliases` row: always shown. When empty, it shows a ghost "Add alias" button.
- Last row (no hover): ghost "Add property" on the left, and on the right "First mentioned <date link>" in 12px `--muted`.

Editing:
- Clicking a row (anywhere except a link inside it) turns that row back into its **raw block text**, in place, at the same height. Example: **status:** in progress, with the key in 600 weight. Edit mode shows an inset 1.5px `--link` ring, white fill, and the caret at the end.
- Enter or Escape commits the edit and draws the row again. Enter at the end of the **last** property row leaves the bar and creates or focuses the first body block below it.
- "Add property" inserts a new empty property block at the end of the bar, in edit mode.
- Arrow up from the first body block moves into the last property row. Arrow down from the last property row moves into the first body block. The bar takes part in normal block keyboard navigation.
- Deleting all the text in a property row removes the block, and its row disappears.

## 4. Screens

### 4.1 Daily Notes

Sidebar plus main area. The main area has the top bar and the content column. Days are listed newest first, in the order: day section, linked references (if any, inside that day's section), then the next day. Consecutive empty days are grouped into empty day rows. Scroll position resets to today when you click "Today".

### 4.2 Page view

Sidebar (the page is highlighted in Pages) plus main area: top bar with breadcrumb, page title (Page title style), properties bar, body blocks (placeholder "Click to start writing" in `--muted` with a `--line-strong` bullet when the body is empty), then the Linked references panel 48px below, then Unlinked references.

### 4.3 Stacked panes (navigation model)

Pages open side by side in panes instead of replacing each other. Daily Notes is always the first pane.

Layout:

- The main area below the top bar is one horizontal scroller (overflow-x: auto) with `--well` as its background. Panes sit in a row inside it.
- Each pane: fixed width 560px (allowed range 480 to 720), full height, `--paper` background, 1px `--line` border on both sides. Each pane scrolls vertically by itself.
- Each pane is `position: sticky` with `left: index × 40px`. As you scroll right, older panes stay stuck at the left edge, and newer panes slide over them.
- Spine: when a pane is almost fully covered by the next one (less than about 80px of it shows), its visible 40px strip turns into a spine: `--well` fill, 1px `--line` right border, and the page title written vertically (`writing-mode: vertical-rl`, 13/500 `--ink-2`), starting 18px from the top. Clicking the spine scrolls back so that pane is fully visible (`scrollLeft = index × (paneWidth − 40)`).
- Pane header: 44px tall. It holds a close (×) icon button on the right, on every pane except the first. Pane content: padding 0 40px 72px. Page title is 30/38 inside a pane, and day titles are 26/34.

Behavior:

- Plain click on a page link, date link or tag in pane i keeps panes 0…i, drops every pane to the right of i, adds the target as pane i + 1, and smooth-scrolls to the end. ⌘-click (Ctrl-click off macOS) replaces pane i in place: keep 0…i−1, put the target at i, and drop the rest. The Daily Notes pane is never replaced, so a ⌘-click inside pane 0 acts like a plain click.
- If the target is already open anywhere in the stack, nothing is added. The scroller just smooth-scrolls to that pane.
- In pane i, any link whose target is open in pane i + 1 gets a `--link-tint` highlight (2px tint ring, radius 3), so you can see which link the next pane came from.
- Closing a pane removes only that pane. The panes after it shift left.
- Sidebar items (pages, tags, calendar days) open their target as a new pane at the end of the stack. Clicking "Daily Notes" in the sidebar resets the stack to only the Daily Notes pane.
- Top bar: the breadcrumb is replaced by a trail of the open panes (titles separated by chevron-right icons; the last one is `--ink` 500, the others `--ink-3`). Clicking a title scrolls to that pane. "Close other panes" (outline button) keeps the Daily Notes pane and the focused pane. Back closes the last pane.
- Save the stack to the window's state (and the URL, if you have one) so a reload restores the same panes.
- Day titles in the Daily Notes pane are buttons that open that date's page as a pane.

### 4.4 View mode toggle: Panes or Single

A segmented control in the top bar, just left of "Close other panes": Panes (three-column icon) | Single (page icon). Height 32, 1px `--line-strong` border, radius 8. The selected side has a `--selected` fill and `--ink` 500 text; the other side is white with `--ink-3` text. Use `aria-pressed`. Remember the choice across app launches (a setting, not per page).

Both modes share one list: the pane stack. Single mode just shows its last item.

- Single mode: one page fills the main area. The content column is centered with a max width of 760px. There are no spines, close buttons or "Close other panes" button.
- A plain click navigates: it adds the target to the end of the list. If the target is already in the list, cut the list back to that entry instead of adding a duplicate.
- ⌘-click opens the target as a pane: switch to Panes mode and add it at the end. This gives a quick way to "peek" a page next to the current one without changing the setting.
- Back removes the last item. Clicking a title in the top bar trail goes back to that entry.
- Switching modes keeps the list as it is. Single → Panes shows every entry as a pane, scrolled to the end. Panes → Single shows the last pane.

Keyboard (both modes): each pane is a landmark (section with the page title as its label). Suggested shortcuts: ⌘W closes the focused pane, ⌥← and ⌥→ move focus between panes, and moving focus into a pane scrolls it fully into view.

## 5. Behavior and accessibility checklist

- All clickable things are buttons or links. No click handlers on plain divs.
- Icon-only buttons have accessible labels.
- Text contrast is at least 4.5:1. `--bullet` and `--disabled` are never used for text.
- Clickable rows (empty day, unlinked references) are at least 44px tall. Small buttons can be 28 to 32px, since this is desktop only.
- Respect "reduce motion". Collapse and expand can animate height over 120ms ease-out, or not at all.
- Window min width: 960px. Below about 1100px, the sidebar can be collapsed with the toggle. Content stays capped at 760px.

## 6. Open questions

- Status colors beyond "in progress" (done, blocked, and so on) aren't defined yet. Add them as `--status-*` tokens and check each one's contrast before use.
- The right panel (open a page next to the current one) has a toggle but no design yet.
- The command palette (⌘K) has a trigger but no design yet.
