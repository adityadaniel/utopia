# PRD — Utopia

**A local-first, Roam-style networked-notes desktop app in Rust + GPUI**

| Field | Value |
|---|---|
| Working name | **Utopia** (placeholder; crates `utopia-*`) |
| Document version | 1.0 (draft for review) |
| Date | 2026-11-02 |
| Owner | Aditya Daniel |
| Status | Proposal — decisions requested in §19 |
| Target platforms | macOS 13+ (primary), Windows 10+, Linux (X11/Wayland) |
| Tech constraints | Rust, GPUI for all desktop UI, SQLite local store, CRDT-based sync |

---

## 0. TL;DR

We are building a desktop outliner whose mental model matches Roam Research: **everything is a block, every block is addressable, pages are just named blocks, and connections are first-class**. Version 1 ships four load-bearing features:

1. **Daily notes** — the front door of the app.
2. **Bullet list outliner** — infinite-depth blocks with the keyboard ergonomics people expect from Roam/Workflowy/Logseq.
3. **Backlinks** — "Linked References" and "Unlinked References" computed from an indexed reference graph, not from full-text scanning at render time.
4. **Reference nodes** — every block and page is a stable, addressable node that can be transcluded anywhere, edited in place, and inspected for who points at it.

**The unifying model — validated against how Roam is actually used.** Nat Eliason's widely-cited guide ("Roam: Why I Love It and How I Use It", 2020) is the best available description of the *behavioral* product, and it surfaces rules that are easy to miss from a feature list:

- **Notes live nowhere and everywhere.** There is no folder tree. A page does not belong to another page; it is reachable from all of them.
- **Every page is a tag and every tag is a page.** `[[Page Link]]` and `#Hashtag` are stylistic variants of the same edge. Tags are not a separate feature — they are pages with a lighter syntax.
- **Page creation is free and consequence-free.** Typing `[[Modern Religion]]` creates a legitimate placeholder node with no content and no obligation to fill it. There is no "orphan" debt.
- **Links are bidirectional by construction**, and *un*linked mentions are surfaced too — with a one-click "Link" action to promote a mention into an edge. Eliason calls this "the best feature I've found for discovering new relationships between information."
- **Renaming a page merges accidental duplicates and rewrites every link.** Rename `apple watch` → `Apple Watch` and the two pages combine; no broken references, no manual cleanup.
- **Dates are pages.** Tagging `[[January 6th]]` pre-creates that day's page; the tagged to-do resurfaces as a linked reference on that date.
- **The right sidebar is load-bearing.** Shift-clicking a link opens it beside the current note, several notes deep, so you can work across sources without losing your place.

These seven rules — not the four features — are what make the tool feel like a second brain. They are all shipped in v1; §4.3 turns them into jobs. (Eliason's post is from 2020 and its "no mobile app / no API" complaints have since been partly addressed upstream, but the workflows he documents are unchanged.)

**Storage decision (see §11):** one **SQLite** file per graph as the local system of record, with **Loro** (Rust CRDT) as the merge engine and the document model per page-shard. Sync runs over a deliberately dumb, self-hostable relay that stores opaque CRDT updates. Mobile gets the *same* Rust core through UniFFI. SQLite is the only storage engine that exists natively on every phone and desktop, survives as a single copyable file, and gives us FTS5 and SQL joins for backlinks for free.

**What we are explicitly not building in v1:** multiplayer/team graphs, queries, extensions/plugins, PDF annotation, graph visualization, and a web app.

---

## 1. Context

### 1.1 What Roam Research actually is

Roam's novelty is not markdown or bullets — it is a **graph of addressable nodes with an outliner as the input device**. Publicly observable properties of the product and its data model:

- **Every bullet is an entity** with a stable id (`:block/uid`), a body (`:block/string`), an owning page, a parent, and an order within its parent.
- **Pages are nodes too** (`:node/title`), created implicitly the moment you type `[[Something New]]`.
- **References are typed**: `[[Page]]` points at a page by title; `((block-uid))` points at a specific block and renders as a live, editable transclusion.
- **Backlinks are computed, not authored**: LinkedIn/Unlinked References are derived from the same graph.
- **Metadata is text**: attributes written as `key:: value` are queryable because the block is an entity with properties.
- **Time is a first-class surface**: today's date is a page, so journaling, tasks, and meeting notes all land in the same graph.

The data model is exposed through a Datalog-style query API, which is why the community treats the graph as a database rather than a document collection.

### 1.2 Why a desktop rebuild

| Pain with the incumbent | Our answer |
|---|---|
| Cloud-first: offline is a degraded mode, and your graph lives on someone else's server | Local-first: the graph is a SQLite file on your disk; sync is optional and self-hostable |
| Web app performance degrades with graph size and long sessions | Native GPU-rendered UI (GPUI), virtualized lists, sub-frame keystroke latency |
| Mobile apps are thin clients of the same web bundle | Native mobile shell over the same Rust core, with real offline writes |
| Export is lossy and one-way | Markdown/JSON/OPML export at any time; Roam JSON import as an adoption path |
| Pricing and continuity risk | Open data format, no lock-in, self-hostable sync |

### 1.3 Problem statement

> Knowledge workers who think in outlines need a fast, offline-capable desktop tool where **capture is one keystroke**, **connections are automatic**, and **the data outlives the app**.

---

## 2. Goals, non-goals, and principles

### 2.1 Goals (v1)

| ID | Goal | Measure |
|---|---|---|
| G1 | Capture is frictionless | From app launch to first character typed in today's daily note: **≤ 2 s**, and **≤ 1 keystroke** from an already-open window |
| G2 | The outliner feels native-fast | p95 keystroke-to-paint **≤ 16 ms**; page open **≤ 50 ms** at 5,000 blocks |
| G3 | Connections are automatic and trustworthy | 100% of `[[...]]` and `((...))` occurrences resolve or are explicitly marked unresolved; backlink panel p95 **≤ 100 ms** at 2,000 backlinks |
| G4 | The graph is genuinely portable | Full export to Markdown + JSON in < 10 s for 100k blocks; import of a Roam JSON export |
| G5 | Multi-device without merge anxiety | Concurrent edits from 2 devices never silently lose text; every merge is explainable |

### 2.2 Non-goals (v1)

- Real-time multiplayer or shared team graphs (single-user graph, multiple personal devices).
- Plugin/extension runtime, `roam/js`, custom CSS injection.
- Query blocks, `{{[[query]]}}`, Datalog surface.
- PDF annotation/highlighting.
- Graph visualization view.
- Web client.
- E2E encrypted sync (designed for in §11.7, shipped in v1.1).
- AI features of any kind.

### 2.3 Product principles

1. **The block is the atom.** No document-level operations that bypass blocks. Everything addressable, everything referenceable.
2. **No filing.** There is no folder tree and no "move to notebook". A page is reachable from any page that links to it; hierarchy is an emergent property of links, not a container. The app must never ask the user where something belongs.
3. **No cleanup debt.** Empty pages, one-line pages, and pages created by a typo are all legitimate. We never nag about orphans, never auto-delete, and never surface "you have 400 unused pages" — we offer *merge* when a rename reveals a duplicate, and nothing else.
4. **Text is the interface.** Structure is typed, not clicked. Every operation has a keyboard path, and link creation must not interrupt the typing flow.
5. **Local is the truth.** The network is an optimization. Every feature works in airplane mode, including first launch.
6. **Derived data is disposable.** Backlinks, search index, and outline cache must be rebuildable from the block table. If they disagree, the block table wins.
7. **Never lose a character.** Merge conflicts may produce odd ordering; they may not produce deletion.
8. **Escape hatches are features.** Markdown export, SQL access to the file, and plain-text block bodies are part of the product, not an afterthought.

---

## 3. Users and jobs to be done

| Persona | Context | JTBD |
|---|---|---|
| **The researcher** (primary) | Reading, writing, synthesizing across months | "When I learn something, I want it in today's note in one line, so future-me can find it through the concept page." |
| **The operator** | Meetings, tasks, decisions | "When a meeting ends, I want the decisions to be findable from the project page without me filing anything." |
| **The writer** | Long-form drafting in outlines | "When I draft, I want to move whole subtrees and reference prior notes without copy-paste rot." |
| **The connector** (CRM / people notes) | A page per person, a block per interaction | "When I meet someone, I want it tagged to them and to today, so their page is a relationship history without me maintaining a CRM." |
| **The returning user** | Opens the app after a week | "When I open it, I want to see today, and yesterday's unfinished threads, without deciding where to put things." |

**Anti-persona:** teams wanting shared wikis with permissions. We will not serve them in v1 and should say so on the landing page.

---

## 4. Feature baseline and scope

### 4.1 Roam capability map vs. our scope

| Capability | Roam | v1 (P0) | v1.1 (P1) | Later (P2) |
|---|---|---|---|---|
| Daily notes / journal | ✅ | ✅ | templates | — |
| Infinite bullets + zoom | ✅ | ✅ | drag across pages | — |
| Page refs `[[ ]]` | ✅ | ✅ | aliases UI | — |
| Tags `#tag` = pages | ✅ | ✅ | multi-word UI polish | — |
| **Date links → daily page** | ✅ | ✅ | date filters | — |
| Block refs `(( ))` transclusion | ✅ | ✅ | inline embed variants | — |
| Linked References | ✅ | ✅ | filters by co-reference | — |
| Unlinked References | ✅ | — | ✅ + one-click "Link" | — |
| Reference inspector panel | partial | ✅ | outgoing graph | — |
| **Right sidebar (stacked, shift-click)** | ✅ | ✅ | split panes / tabs | — |
| **Rename → merge duplicates + rewrite links** | ✅ | ✅ | — | — |
| Attributes `key:: value` | ✅ | — | ✅ + page metadata header | queryable |
| Block embeds `{{embed}}` | ✅ | — | ✅ | — |
| Extract block → page | manual | — | ✅ | — |
| Queries / Datalog | ✅ | — | — | ✅ |
| Templates | ✅ | — | ✅ (journal + page) | — |
| Slash command menu | ✅ | — | ✅ | — |
| Full-text search | ✅ | titles only | ✅ blocks (FTS5) | semantic |
| Global quick capture | quick capture | — | ✅ (hotkey) | mobile share sheet |
| Local automation API | ✅ (in-app JS API) | — | — | ✅ (local HTTP) |
| Multiplayer | ✅ | — | — | maybe never |
| Extensions / `roam/js` | ✅ | — | — | ✅ (WASM sandbox?) |
| PDF highlights | ✅ | — | — | ✅ |
| Graph view | ✅ (of limited use) | — | — | ✅ |
| Mobile app | ✅ (webview) | — | read/write | native |
| Markdown/JSON export | ✅ | ✅ | OPML | — |
| Roam JSON import | — | — | ✅ | — |

### 4.2 Priority ladder

- **P0 (must ship, v1.0):** Daily notes · Bullet outliner · Backlinks · Reference nodes · **tags-as-pages** · **date links** · **rename/merge** · **right sidebar with stacked pages** · local persistence · undo/redo · quick jump by page title · export.
- **P1 (v1.1):** search (FTS5), todos/headings/quotes/code block types, slash menu, unlinked references + promote-to-link, reference filters, attributes + page metadata header, templates (journal + page), extract-block-to-page, Roam import, block embeds, quick capture hotkey, split panes (since replaced by stacked panes, §9.2.1).
- **P2 (v1.2+):** mobile client, E2E sync, queries, graph view, attachments, PDF, plugin runtime, local HTTP API.

### 4.3 Validated workflows (from practitioner usage)

These are the jobs the P0 set must actually perform. They are used as end-to-end acceptance scripts in M1/M2, not just prose.

| # | Workflow | What must work | Depends on |
|---|---|---|---|
| W1 | **Capture into the day** | Open app → today's note → type one line with two `[[links]]` and a `#tag` → done in < 15 s, zero filing decisions | Daily notes, outliner, refs |
| W2 | **Lateral retrieval** | Open "Mindfulness" → see every page that referenced it, plus plain-text mentions → click "Link" on three mentions → those pages now update | Backlinks, unlinked refs (P1) |
| W3 | **Placeholder pages** | Create a page with zero content (autocomplete on `#`, or `#[[Modern Religion]]`) → it exists immediately, appears in the page list, and the app never complains about it being empty or unreferenced | Tags-as-pages, no-cleanup-debt |
| W4 | **Duplicate rescue** | Create `apple watch` by typo, then rename it to `Apple Watch` → data merges, all links resolve, no dangling refs | Rename/merge |
| W5 | **Future task surfacing** | On Nov 2, write a to-do tagged `[[January 6th, 2027]]` → on Jan 6, it appears as a linked reference on that day's page | Date links |
| W6 | **Multi-source writing** | Shift-click three links into the right sidebar, scroll between them, drag a quote into the main note | Right sidebar |
| W7 | **People CRM** | A page per person with `Email::`, `Company::`, `How We Met::` at top; every interaction tagged on the daily note shows up on the person page | Attributes + metadata header (P1) |
| W8 | **Progressive summarization** | Paste a long quote → bold/italic/highlight spans → the note is readable at a glance without a second tool | Marks, outliner |

---

## 5. P0-1 — Daily Notes

### 5.1 Why it is first

Daily notes solve the "where does this go?" problem. In Roam, they are the default landing surface and the main entry point for capture. If this is slow or confusing, nothing else matters.

### 5.2 Model

A daily note is a `page` with `kind = 'daily'` and a canonical `daily_date` (`YYYY-MM-DD`, local calendar). The **title is a rendering** of the date (default `MMMM do, yyyy` → "November 2nd, 2026"), while the date is the identity. Daily titles are therefore display-only and may legitimately repeat (a `yyyy` format produces "2026" for every day); identity is guaranteed by `daily_date` uniqueness plus date-first resolution (§10.3.1), so a given day can never have two pages.

### 5.3 Requirements

| ID | Requirement |
|---|---|
| FR-1.1 | On launch, the app opens today's daily note (configurable: last-open page / today / a pinned page). |
| FR-1.2 | The daily note is created **lazily, on first write** — never eagerly at startup, and never by looking at it. Visiting a date shows a draft held only in memory (one empty block, or the journal template) with the caret in it; its first edit writes the page to the graph and the outbox. An untouched graph stays empty. The Daily Notes feed lists every calendar day from today backwards, loading older days as it scrolls; a day without a page shows "Click to start writing" and becomes a draft when clicked. Each day shows its Linked References, and no Unlinked References. |
| FR-1.3 | Sidebar "Journal" section shows: Today, Yesterday, Tomorrow, a date picker, and the last 14 journal pages with a 2-line preview. |
| FR-1.4 | A daily note's title is **display-only**: it renders the date format and is never free-text edited. Clicking it opens the date picker (or, with `Cmd`, the graph's date-format setting). This keeps a daily page permanently bound to its date identity. |
| FR-1.5 | Opening any date (including past/future) via the calendar, `Cmd+Shift+D` ("go to date"), or `[` / `]` navigation shows it, as a draft if absent (FR-1.2). |
| FR-1.6 | Daily notes behave exactly like normal pages: they can be referenced by any accepted date form (FR-1.9), receive backlinks, host block refs, and be transcluded. A *normal* page may also be merged into a daily page by renaming it to that date's title (§8.6). |
| FR-1.7 | "Today" recomputes at local midnight and on system timezone change; an open page never auto-navigates on rollover. A non-blocking banner offers "It's a new day → open today". |
| FR-1.8 | A graph setting controls whether journal pages appear in the page list (default: yes, grouped separately). |
| FR-1.9 | **Date links resolve to daily pages.** `[[2026-11-06]]`, `[[November 6th, 2026]]`, and the graph's configured format all resolve to the *same* daily page. Resolution happens in the resolver and normalizes to `daily_date`, never in the renderer. |
| FR-1.10 | **Future dates are pre-creatable by reference.** Typing a date link for a date with no page creates the daily page (empty, unvisited) and the link resolves to it. When that date arrives, every block that referenced it appears in its Linked References. This is the "assign a to-do to future me" workflow (W5). |
| FR-1.11 | Formatting a date is a *rendering* decision only. The canonical identity is always ISO `YYYY-MM-DD`; changing the date format setting re-renders titles without touching identity, links, or backlinks. |
| FR-1.12 | The daily page surfaces a **"Scheduled for this day"** group at the top of Linked References: blocks from *other* pages that reference this date, with a link back to their source. |

### 5.4 Acceptance criteria

- **AC-1.1** Fresh install → launch → cursor is in the first block of today's note → typing `hello` → quit → relaunch → same page, text intact. Total elapsed from hot launch to first keystroke ≤ 2 s on a 2020-era laptop.
- **AC-1.2** Set system clock to 23:59, write a block, wait for rollover: the block stays on the old date, the banner appears, "open today" creates the new date without duplicating anything.
- **AC-1.3** Two devices with different timezones both open "2026-11-02": they see the **same** page (identity is the calendar date string, not the title).
- **AC-1.4** Visiting 30 days from the calendar creates no pages; writing one character on each creates 30 pages, each with exactly one block, in under 300 ms total.
- **AC-1.5** Deleting a daily page and revisiting the date re-creates it as a new page; the old page remains recoverable from trash for 30 days.
- **AC-1.6** On a page dated 2026-11-02, type `Pay rent [[2026-12-01]]`. Create/open 2026-12-01: the "Scheduled for this day" group shows that block, and clicking it navigates to the source block's page, not the daily page.
- **AC-1.7** Type the same date three ways (`2026-12-01`, `December 1st, 2026`, and the configured format) in three different pages: all three render as the same page chip, and that daily page lists three backlinks — not three duplicate pages.
- **AC-1.8** Change the date format setting from `MMMM do, yyyy` to `yyyy-MM-dd`: every daily page title re-renders, no ids change, no link breaks, backlink counts are identical.

### 5.5 Edge cases to specify in implementation

- **Ambiguous numeric dates never auto-resolve.** `11/6/26` is US-vs-rest-of-world ambiguous; it resolves only if the graph's configured format is unambiguous (`yyyy-MM-dd` or month-name based). Otherwise it is created as an ordinary text page named "11/6/26". Silently guessing a date is worse than a plain page.
- Timezone change mid-session (travel): today's identity follows the machine's current local date; historical pages do not shift.
- DST transitions: date arithmetic uses calendar days, never `+86400s`.
- User renames the date format to include a literal that collides with another page title: identity still wins; daily titles are display-only and may repeat.
- **Renaming a normal page to a date-shaped title never creates a second page.** If that date's daily page exists, the rename becomes a merge into it (with a preview of what will move); if it does not exist, the daily page is created and the page merges into it. Aliases may not be date-shaped, because date resolution takes precedence (FR-1.9).
- Graphs with journal pages whose dates were created under a different calendar system (e.g., user switches locale): out of scope; store ISO date + locale tag at creation.

---

## 6. P0-2 — Bullet Outliner

### 6.1 The core object

A **block** is `{ id, page_id, parent_id, position, depth, text, type, checked, created_at, updated_at, deleted_at }`. `position` is a fractional index string; ordering is lexicographic. Blocks form a forest rooted at the page. Indentation depth is unbounded in the model; the UI caps visual indent at 12 levels and shows a depth badge beyond that.

### 6.2 Editing model

- **One focused block at a time** owns the caret in v1. Cross-block selection is P1; this is a deliberate de-risking choice because cross-block selection + IME + rich runs is where GPUI text editing is weakest.
- The block body is **markdown-subset source text** (`**bold**`, `_italic_`, `~~strike~~`, `` `code` ``, `[[Page]]`, `((uid))`, `#tag`). Rationale in §10.6.
- Inline reference tokens are rendered as styled runs inside the same shaped line, not as child widgets. Hit-testing maps x-offset → byte range → token. This keeps IME viable and layout cheap.

### 6.3 Requirements

| ID | Requirement |
|---|---|
| FR-2.1 | Every block is created with a globally unique ULID and is immediately referenceable. |
| FR-2.2 | Enter creates a sibling directly below, inheriting nothing but block type; inside a collapsed parent, Enter creates a sibling of the parent. |
| FR-2.3 | Shift+Enter inserts a soft line break within the block. |
| FR-2.4 | Tab indents the block under its previous sibling (carrying its subtree); Shift+Tab outdents to become the next sibling of its parent. Tab on a first child is a no-op with a subtle shake. |
| FR-2.5 | Backspace at offset 0 merges the block into the previous visible block: text appended, children re-parented, block tombstoned. Whole operation is one undo step. At offset 0 of the first block, it outdents instead. |
| FR-2.6 | Cmd+Shift+↑/↓ moves the block (and subtree) among siblings; at edges it becomes the previous sibling of its parent. |
| FR-2.7 | Clicking a block's bullet zooms into it; breadcrumbs show the path and a "zoom out" affordance. |
| FR-2.8 | Collapse/expand is a per-block toggle persisted **locally per device** (not synced), preserving caret position when collapsed around the cursor. |
| FR-2.9 | Drag-and-drop reorders blocks within a page with a valid-drop indicator; invalid drops (own descendant, depth > 12) are rejected with a shake. Cross-page drag is P1. |
| FR-2.10 | Paste: single line → inline text; multi-line plain text → one block per line preserving relative indentation; pasted markdown → parsed into blocks + inline marks; pasted URL on selection → markdown link. |
| FR-2.11 | Copy/cut of a collapsed or multi-selected subtree yields nested Markdown list syntax. |
| FR-2.12 | Undo/redo is a per-page command stack, coalescing same-kind consecutive edits within 500 ms; survives window close (persisted per session). |
| FR-2.13 | Block type switching (P1): bullet, numbered, todo, quote, code, heading 1–3. `todo` checkboxes are the first P1 item and may be pulled into M1 if the daily-note workflow demands it. |
| FR-2.14 | A page renders only visible blocks; scroll performance must not degrade linearly with page size. |
| FR-2.15 | **A hashtag is a page link.** `#mindfulness` and `[[mindfulness]]` produce the identical edge, the identical page, and the identical backlink. Typing `#` opens the same autocomplete as `[[`, and the canonical stored form is `#[[Title]]` for any multi-word title. The bare form is single-token (`#` + letters/digits/`-`/`_`); a bare multi-word phrase commits as a tag only under the longest-match rule in §6.5. Grammar: §20.B. |
| FR-2.16 | Inline marks are part of the block source: `**bold**`, `_italic_`, `~~strike~~`, `^^highlight^^`, `` `code` ``. Marks never nest within themselves; unclosed delimiters render literally until healed on commit. |
| FR-2.17 | Link creation must not break flow: `[[`, `#`, and `((` all open an inline autocomplete positioned at the caret; `Cmd+Enter` on a novel title creates the page and inserts the link; `Esc` dismisses and leaves the typed text intact. |
| FR-2.18 | (P1) **Extract block to page**: converts a block (with its subtree) into a new page whose first block is a reference back, replacing the original block with a `[[Title]]` link. One undo step. |
| FR-2.19 | (P1) **Global quick capture**: a system-wide hotkey opens a floating single-block capture window that appends to today's daily note and closes on `Esc`. |

### 6.4 Acceptance criteria

- **AC-2.1** A 5,000-block page opens in ≤ 50 ms after warm start; scrolling it never drops below 60 fps on an M1 Air.
- **AC-2.2** Typing a 40-character sentence in a block shows ≤ 16 ms p95 keystroke-to-paint and ≤ 8 ms median.
- **AC-2.3** 200 consecutive undos return the page to a byte-identical projection (asserted by a test that snapshots the block table).
- **AC-2.4** Indenting a block with 300 descendants completes in ≤ 20 ms and does not re-render off-screen rows.
- **AC-2.5** Pasting a 500-line markdown document produces 500 blocks with correct nesting in ≤ 400 ms, in one undo group.
- **AC-2.6** Collapsing a block with 1,000 visible descendants drops the rendered row count to the viewport size within one frame.
- **AC-2.7** Write `#mindfulness` on one page and `[[mindfulness]]` on another: one page exists, both blocks appear in its Linked References, and the page list shows a single entry.
- **AC-2.8** Type `#Modern Religion` where that page exists: the committed block contains one tag chip resolving to "Modern Religion". Type `#modern religion` in a graph where no such page exists: the tag terminates at the space, the word "religion" remains plain text, and an inline affordance offers "Tag the whole phrase" which rewrites it to `#[[modern religion]]`.
- **AC-2.9** Type `**bold` and commit: the text renders literally with the asterisks visible, no broken run styling, and pressing `Cmd+B` at the end closes and applies the mark.
- **AC-2.10** Typing `#` and choosing "Modern Religion" from autocomplete inserts the canonical `#[[Modern Religion]]`, not a bare phrase; deleting that page afterwards turns the chip into an unresolved intent with a create action — it never silently degrades to plain text.

### 6.5 Non-obvious implementation notes

- **Fractional indexing**: use base-62 fractional keys (`fractional_index` crate). Rebalancing is never required in normal use; a background job rewrites positions when key length exceeds 64 bytes.
- **Depth is denormalized** for fast rendering and indent guides; it is recomputed on move/merge in the same transaction.
- **Tombstones**: deletion sets `deleted_at` and cascades to the subtree, retaining ids so `((refs))` render as "deleted block" rather than dangling garbage. Hard delete only via empty-trash.
- **Caret preservation across remote merges** is explicit: the editor holds a CRDT cursor (Loro cursor), not a byte offset, so a sync update that inserts text above the caret does not move the caret.
- **Hashtag parsing (normative).** `#` starts a tag candidate; token characters are letters, digits, `_` and `-`. On commit, in order: (1) if the candidate plus following words matches a **live page title** under the longest-match rule (case-insensitive, punctuation-terminated), the whole phrase becomes a tag and is canonically stored as `#[[Title]]`; (2) otherwise the tag ends at the first space or punctuation; (3) `#[[Title]]` is always available as the unambiguous escape form and is what autocomplete inserts. This is why `#Modern Religion` "just works" in Roam while `#2 pencils` does not become a page.
- **Consequence of page-state-dependent parsing:** the derived `ref` for a block can differ between two devices that disagree about whether a page exists. Since refs are projections, this is safe — on page creation we re-derive only blocks holding unresolved intents that match the new title (§7.3 FR-3.12), and the *block text is never rewritten by sync*.
- **Unlinked mentions are an index query, not rows.** A mention is computed from FTS5 at panel-open time and filtered by exact title-key match, so prose never inflates the reference table.

---

## 7. P0-3 — Backlinks

### 7.1 Definition

A **backlink** is a reference edge whose target is the page or block currently being viewed. Roam surfaces two kinds:

- **Linked References** — explicit `[[Title]]` / `((uid))` edges anywhere in the graph.
- **Unlinked References** — plain-text mentions of the page title (P1).

Backlinks are **derived**: they are recomputed from block text by the indexer, never authored by the user.

### 7.2 Data flow

```
block text ──parse──► RefSpan { kind, target } ──resolve──► ref row
                                                   │
                          refs(target) ──group by source page──► LinkedReferences
```

Resolution order for `[[Title]]`: exact `title_key` → alias table → unresolved (rendered as a "create page" chip). Resolution for `((uid))`: block id lookup, including tombstones.

### 7.3 Requirements

| ID | Requirement |
|---|---|
| FR-3.1 | Every page renders a "Linked References" section at the bottom, grouped by source page, showing the source page title, a breadcrumb path to the referencing block, and the block with the matched token highlighted. |
| FR-3.2 | Groups are collapsed to 3 visible references with "+N more"; expansion state persists per page per device. |
| FR-3.3 | Sort: by source page title (default) or by recency; filter by text query within results. |
| FR-3.4 | Backlinks live-update while editing the current page, including self-references (a page referring to itself). |
| FR-3.5 | A block's reference panel shows "Referenced by (N)" — the same data scoped to a block id. |
| FR-3.6 | Renaming a page keeps all existing references working by auto-registering the old title as an alias; a one-click "rewrite references to new title" action exists. |
| FR-3.7 | Deleting a referencing block removes its edges immediately; deleting a *target* block leaves edges resolving to a tombstone ("deleted block") rather than silently disappearing. |
| FR-3.8 | The index is incrementally maintained: editing one block updates only that block's edges. |
| FR-3.9 | P1 — Unlinked References: case-insensitive whole-word mentions of the page title outside brackets, computed from the FTS index with a title-key filter. |
| FR-3.10 | **Reference filters (co-reference).** A filter chip lets you narrow Linked References to sources that *also* reference another page — e.g. "Mindfulness ∩ Books". Filters are stackable with the text filter, persist per page per device, and are pure index queries (no query language in v1). |
| FR-3.11 | **Promote mention → link.** Every unlinked mention has a "Link" action that rewrites the mention in place to `[[Title]]` (preserving surrounding text and marks), as one undo step. This is the discovery loop Eliason calls the best feature in the tool. |
| FR-3.12 | Creating a page re-derives only blocks holding unresolved intents that match the new title (bounded by an index on `dst_title_key`), so pending intent links resolve without a full reindex. |
| FR-3.13 | The Linked References panel can be pinned into the right sidebar as a live view while editing the page. |

### 7.4 Acceptance criteria

- **AC-3.1** A page with 2,000 backlinks renders its panel in ≤ 100 ms; adding a new reference from another page appears within 200 ms of commit.
- **AC-3.2** Typing `[[Alpha` (unclosed) does not create a page; typing `[[Alpha]]` creates/resolves it **on commit (blur, Enter, or 300 ms idle)** and the target page's backlinks update without a refresh.
- **AC-3.3** Rename "Alpha" → "Alpha Prime": the old page's 500 backlinks still resolve; `[[Alpha]]` in old text renders as Alpha Prime with a tooltip showing the alias.
- **AC-3.4** Deleting a page and its backlinks are consistent after a forced index rebuild (`PRAGMA integrity` script): rebuilt table equals incremental table exactly.
- **AC-3.5** With 100k blocks, editing a block updates backlinks in ≤ 5 ms (only touched rows).
- **AC-3.6** On "Mindfulness" (400 backlinks), add filter chip "Books": panel shows only the 37 sources that mention both, in ≤ 120 ms; removing the chip restores the full list without a re-query round trip.
- **AC-3.7** In Unlinked References, click "Link" on a mention: the source block's text changes `mindfulness` → `[[Mindfulness]]` with no other character altered, the mention disappears from Unlinked, and it appears in Linked References. `Cmd+Z` restores both views exactly.
- **AC-3.8** Create a page titled "Zettelkasten" while 6 blocks contain an unresolved `[[Zettelkasten]]`: within one idle cycle all 6 resolve, the page shows 6 backlinks, and no other page is reindexed (assert via query plan / row counts).

### 7.5 UI sketch

```
┌──────────────────────────────────────────────────────────────┐
│  Alpha Prime                                     [⋯] [⋮]     │
│                                                              │
│  • A thesis I keep returning to.                             │
│    • Sub-point                                                │
│                                                              │
│ ── Linked References ──────────────────────── 214 ────────── │
│  [ + filter: also refs ▾ ] [ Books × ] [ text… ]            │
│  ▾ November 2nd, 2026                              12        │
│      • Met with **[[Alpha Prime]]** today to discuss…        │
│        › Nov 2 › meeting notes                               │
│      • …                                                     │
│  ▸ Research log                                    38        │
│  ▸ Book: The Timeless Way                          7         │
│                                                              │
│ ── Unlinked References ─────────────────────── 6 ──  (P1)   │
│      • …the mindfulness of the thing…            [ Link ]    │
│      • …a mindfulness practice…                  [ Link ]    │
└──────────────────────────────────────────────────────────────┘
```

---

## 8. P0-4 — Reference Nodes

### 8.1 The concept (and an explicit interpretation)

The phrase "references node" maps to two Roam behaviors that share one underlying idea. **We implement the idea, not either half:**

> **Any node (page or block) is a first-class addressable entity. A reference to it is a first-class typed edge. The same edge powers transclusion, backlinks, and inspection.**

This gives us:

- **Page reference** — `[[Title]]` in text → chip → navigate.
- **Block reference** — `((ulid))` → inline transclusion of the referenced block, **editable in place through the reference** (edits hit the same underlying block).
- **Reference inspector** — a panel that shows, for the focused node: what it points at (outgoing), what points at it (incoming), and unresolved intents.

If the intent was narrower (only the transclusion, or only the inspector), the other parts are already implied by the same data structures and cost little extra.

### 8.2 Address types

```rust
pub enum NodeRef {
    Page  { page_id: PageId, title: String },
    Block { block_id: BlockId, page_id: PageId, preview: String },
}

pub enum RefResolution {
    Resolved(NodeRef),
    Unresolved { kind: UnresolvedKind, query: String }, // kind: Page | Block
    Ambiguous(Vec<NodeRef>),                            // duplicate titles (should not occur; see §10.3)
}
```

### 8.3 Requirements

| ID | Requirement |
|---|---|
| FR-4.1 | Typing `[[` opens a fuzzy page autocomplete (titles + aliases); Enter inserts `[[Title]]`; `Cmd+Enter` on a new title creates the page and inserts a reference. |
| FR-4.2 | Typing `((` opens a block search over block bodies with page context; Enter inserts `((ulid))`. A "copy reference" action on any block copies its `((ulid))` to the clipboard. |
| FR-4.3 | A `((uid))` renders inline as a compact chip showing the first 40 chars of the target block plus a chevron. Clicking the chevron expands to a full inline embed rendering the block and its children, indented and visually bracketed. |
| FR-4.4 | Editing inside an expanded embed edits the **original** block. The same edit appears at the source location and in every other embed immediately. |
| FR-4.5 | Self-reference and reference cycles are detected; the embed stops expanding at depth 3 and shows "↻ cyclic reference" instead of recursing. |
| FR-4.6 | Reference inspector panel (right sidebar) with three tabs: **Outgoing** (edges from this node), **Incoming** (backlinks), **Unresolved** (dangling intents, e.g. `[[Typo Page]]` with zero content, offering create/redirect). |
| FR-4.7 | Unresolved block refs render as a red-tinted chip: "Missing block ((uid))" with copy-id and dismiss actions. They are never silently dropped. |
| FR-4.8 | Hovering any reference shows a peek card (target title, first 3 blocks, backlink count) without navigation; `Cmd+Click` opens in the other pane. |
| FR-4.9 | Reference edges record an anchor span so highlights survive edits elsewhere in the block; the anchor is recomputed on block edit. |
| FR-4.10 | All node types are addressable by copy-paste of a `utopia://` URL: `utopia://page/<id>`, `utopia://block/<id>` — enabling cross-app linking from other tools. |
| FR-4.11 | **Reference kinds are uniform.** A `[[page]]`, a `#tag`, a `((block))`, and a `[[date]]` produce the same edge shape with different resolution rules. The inspector, backlink panel, and export all treat them uniformly — there is no separate "tags" subsystem. |
| FR-4.12 | **Rename is safe.** Renaming a page keeps its id, rewrites its rendered title, and registers every previous title as an alias so existing `[[old title]]` references keep resolving (with a tooltip showing the alias). |
| FR-4.13 | **Rename onto an existing title merges.** If the new title collides with another live page, the app merges the two (see §8.6) rather than erroring or creating a duplicate. |
| FR-4.14 | **Unresolved intents are actionable.** Every dangling `[[Title]]` offers: Create page · Redirect to an existing page · Convert to plain text. Choosing "create" resolves every block with that intent graph-wide. |

### 8.4 Acceptance criteria

- **AC-4.1** Type `((`, search "thesis", select a block → the chip renders the target's text within one frame; expanding shows its 3 children.
- **AC-4.2** Edit inside the expanded embed → switch to the source page → the edit is there; `Cmd+Z` in the embed reverses it at both sites.
- **AC-4.3** A block referencing itself renders `↻ cyclic reference`, no infinite layout, no crash.
- **AC-4.4** Rendering a page with 200 refs to the same block costs ≤ 10 ms and reuses a single resolved node.
- **AC-4.5** Deleting the target of 40 block refs leaves 40 chips marked "Missing block" — never blank space, never a panic.

### 8.5 Visual language

```
• The central claim is that [[Alpha Prime]] is load-bearing.
                                  └── page chip (click → navigate)

• Supporting evidence: ((01J9F3K2M8Q4X7B6C5D4E3F2G1))
                        └── block chip  ▸ expand to transclude
```

### 8.6 Rename and merge (normative)

Roam's most quietly important behavior: **renaming a page onto another page's title merges them and rewrites every link.** Users rely on this to fix typos and duplicate tags; it is the reason the graph does not rot. It is also the subtlest thing in this document, so it gets a full specification.

**Trigger:** user renames page `B` to a title whose `title_key` equals live page `A`'s (`A ≠ B`). Equality uses the frozen normalization (§10.3): case-fold, NFKC, whitespace-collapsed. `apple watch` → `Apple Watch` collides and therefore merges.

**Steps (single transaction, single undo step):**

1. Re-parent every root block of `B` under `A` at the end of `A`'s roots, preserving block ids and subtree structure. Block ids are globally unique, so this is a move — never a copy.
2. Re-point every `ref` row with `dst_page_id = B` to `A`; recompute `src_page_id` for blocks that moved.
3. Move `B`'s aliases to `A`; register `B.title_key` as an alias of `A`.
4. **Daily status follows the survivor's title.** If `A`'s title parses as a date, `A.daily_date` is that date and `A.kind` becomes `daily`; otherwise `A` stays a normal page. Because daily pages cannot be renamed (FR-1.4), `B` is a normal page in practice; the tombstone retains its prior state so the merge stays undoable.
5. Tombstone `B` (`deleted_at`), retain its `page_doc` shard for history and undo.
6. Emit CRDT ops on `A` (moves + title) and a tombstone marker on `B`. Remote peers receive these as ordinary updates; **no special "merge" message exists in the sync protocol.**

**Concurrency rule (critical):** merging is not a one-shot event but a convergent *rule*. If a peer that has not seen the merge adds a block under `B`, then on import the shard for a tombstoned page that receives live blocks is re-merged into `A`. This makes merge idempotent and keeps the "no duplicate live pages" invariant under partition.

**Import-time deduplication:** if an incoming page's `title_key` collides with a local live page and the ids differ (both devices created the same title while offline, or one had a tombstone), the importer runs the same merge routine automatically and writes a `History` entry for the affected page. It never prompts, because the outcome is unambiguous.

**Acceptance criteria**

- **AC-4.6** Two pages with 3 and 5 root blocks respectively (one block has a two-level-deep subtree) merge on rename: the result has 8 root blocks, every original block id is preserved with its subtree intact, zero text is duplicated, and all 40 existing `[[B]]` references resolve to `A`.
- **AC-4.7** Rename `apple watch` → `Apple Watch` where `Apple Watch` already exists: merge completes in ≤ 30 ms for 1,000 blocks and is reversible with one `Cmd+Z`.
- **AC-4.8** Device 1 merges `B` into `A` while device 2 (offline) adds three blocks to `B`. On sync, device 2's blocks appear under `A`, `B` remains tombstoned on both devices, and both converge to identical state.
- **AC-4.9** Renaming a page to a title that differs only by Unicode normalization form or consecutive spaces also merges (title-key equality, not string equality).

---

## 9. Interaction spec

### 9.1 Keyboard map (macOS; Ctrl substituted on Windows/Linux)

| Action | Binding | Notes |
|---|---|---|
| New block below | `Enter` | Splits at caret if mid-block |
| Soft line break | `Shift+Enter` | |
| New block above | `Cmd+Enter` | |
| Indent / outdent | `Tab` / `Shift+Tab` | |
| Move block up/down | `Cmd+Shift+↑/↓` | Carries subtree |
| Toggle collapse | `Cmd+.` | |
| Expand/collapse all | `Cmd+Alt+.` | |
| Zoom into block | `Cmd+Alt+↓` | Or click bullet |
| Zoom out | `Cmd+Alt+↑` | Breadcrumb also clickable |
| Go to today | `Cmd+Shift+D` | |
| Previous / next day | `Cmd+[` / `Cmd+]` | In a daily page context |
| Quick jump (pages) | `Cmd+K` | v1: titles + aliases only |
| Full search | `Cmd+Shift+F` | P1 |
| Tasks | `Cmd+Shift+X` | Every to-do in the graph (§9.2.2) |
| Backlink panel | `Cmd+Shift+B` | |
| Toggle right sidebar | `Cmd+Shift+\` | |
| Open in right sidebar | `Shift+Click` on any link | Stacks the page beside the current one; `Cmd+Shift+Click` opens a new pane |
| Quick capture (P1) | `Cmd+Shift+Space` | System-wide; appends to today's note |
| Bold / italic / code | `Cmd+B` / `Cmd+I` / `Cmd+E` | Wraps or unwraps |
| Undo / redo | `Cmd+Z` / `Cmd+Shift+Z` | |
| Copy block ref | `Cmd+Shift+C` | Copies `((ulid))` |
| Delete block | `Cmd+Shift+Backspace` | Deletes subtree to trash |

Bindings load from `keymap.json` in the app config dir; unknown actions log a warning and are ignored (never panic).

### 9.2 Main window

```
┌───────────────┬──────────────────────────────────────┬────────────────────────┐
│ Utopia   ⌘K  │ ◂ Nov 2 ›              Today    ⋯    │ ⧉ Alpha Prime      ✕   │
│               ├──────────────────────────────────────┤   • A thesis I keep…   │
│ ▾ Journal     │  • Morning pages                     ├────────────────────────┤
│   Today    ●  │    • Woke up thinking about          │ ⧉ Dana             ✕   │
│   Yesterday   │      [[Alpha Prime]]                 │   • Met at the conf…   │
│   Nov 1       │  • ((01J9F…))                        ├────────────────────────┤
│   …           │    └ transcluded block               │ References         ⌄   │
│               │  • Meeting with [[Dana]]             │ Incoming 14            │
│ ▾ Pages       │    └ shift-click → opens in sidebar  │  ▾ Nov 2   6           │
│   Alpha Prime │                                      │  ▸ Oct 30  4           │
│   Reading log │  ── Linked References ── 214 ──      │ Outgoing 3             │
│   Modern Rel. │  [ + filter ] [ Books × ]            │ Unresolved 0           │
│               │  ▾ Nov 2, 2026              12       │                        │
│ ── Storage ── │  ▸ Research log             38       │                        │
│ Synced 2m ago │                                      │                        │
└───────────────┴──────────────────────────────────────┴────────────────────────┘
```

#### 9.2.1 Stacked panes and the view mode (SP)

The main column holds one list of open pages, the **pane stack**, like Andy Matuschak's
notes. The visual spec is DESIGN.md §4.3 and §4.4; the click rules live in one pure
function (`utopia_ui::pane_stack::open`).

| Id | Requirement |
|---|---|
| SP-1 | There is one stack per window. Entry 0 is always the Daily Notes feed; an entry is a page, optionally zoomed into a block. The stack is local UI state (§11, rule 9): never in a shard, never synced. |
| SP-2 | Panes mode: a plain click on a page link, date link or tag in pane *i* keeps panes 0…*i*, drops the rest and adds the target as pane *i* + 1. A target already open anywhere is not added again; the row scrolls to it. |
| SP-3 | Panes mode: ⌘-click (Ctrl-click off macOS, ⌘⇧ included) in pane *i* > 0 replaces pane *i*. In pane 0 it acts as a plain click. ⌘⇧-click no longer opens a split pane: the stack replaces split panes (P1). |
| SP-4 | Each pane but the first has a close button; ⌘W closes the focused pane; "Close other panes" keeps Daily Notes and the focused pane. Daily Notes never closes. |
| SP-5 | Back removes the last entry. The top bar's trail lists every entry: in Panes mode a title scrolls to its pane, in Single mode it cuts the stack back to it. Single mode: a plain click adds the target at the end, or cuts back to it if it is already open. |
| SP-6 | Panes \| Single is an app setting (`settings.json` `view_mode`, default `single`). Switching keeps the stack. ⌘-click in Single switches the window to Panes and adds the target at the end, without changing the setting. |
| SP-7 | Sidebar pages, tags, calendar days, Quick jump and `utopia://` links open their target at the end of the stack (or scroll to it when open). "Daily Notes" resets the stack to the feed alone. |
| SP-8 | The stack is saved in `local_state` (`pane_stack`) on every change and restored at launch, without pages deleted since; the launch rule (FR-1.1) applies only when nothing beyond the feed was saved. |
| SP-9 | ⌥←/⌥→ move focus between panes outside text editing (inside a block they move by word); a focused pane scrolls fully into view. |

#### 9.2.2 Tasks view (TK)

One list of every to-do in the graph, so open work written in daily notes and pages can be
found without a query block. It is a view over the `block` projection (§11.1): it has no
data of its own, and it writes only through the same command a page uses. The visual spec is
DESIGN.md §4.5.

| Id | Requirement |
|---|---|
| TK-1 | The sidebar shows **Tasks** under Daily Notes, with the number of open to-dos on the right. Clicking it, or `GoTasks` (`Cmd+Shift+X`), shows the Tasks view in the main area, the way the graph view does: the pane stack is kept and comes back when a page is opened. A to-do block counts once, whatever its children are. |
| TK-2 | The view lists every live `todo` block on a live page, one group per page. A group is headed by the page title (and, for a daily page, its weekday) and opens that page. Groups are ordered by the most recent change to a task in them; tasks keep outline order. A task under a heading block sits under a sub-heading with that heading's text; a task whose parent is a plain block shows the parent's text under it; a to-do under a listed to-do keeps its thread line. |
| TK-3 | An Open / Done / All filter with a count on each. Open is the default; Done lists every done task, with no time limit. The filter and folds are local UI state (rule 9) and are not saved. |
| TK-4 | Ticking a box runs the same command as ticking it on the page (rule 13): one undo step, synced like any edit. A row ticked under Open stays in place, struck through, until the filter changes or the view is left. |
| TK-5 | A task's due date is its earliest date link (FR-1.10, W5). It is drawn once, inline, as a link; the row adds only **Overdue** (before today, not done) or **Today**. Ambiguous numeric dates never resolve (§5.5), so they never count. |
| TK-6 | Clicking a task's text opens its page with the block revealed, by the reference-row rules (§9.2.1; Shift-click opens it in the right sidebar). |
| TK-7 | The view and the sidebar count follow the index (`IndexUpdated`), so edits made elsewhere, including by sync, show without a refresh. |
| TK-8 | With nothing to show, the view says so and how to make a to-do: `/To-do` or `Alt+Enter` in any block. |

### 9.3 Empty and error states

| State | Behavior |
|---|---|
| No graph yet | One-field onboarding: "Name your graph" → creates `~/Documents/Utopia/<name>.utopia`. |
| No daily note today | Created on first visit; a single empty bullet with the caret, and a ghost hint that fades on the first keystroke. |
| Unresolved ref | Chip with `Create "Foo"` / `Change to…` / `Show as text`. |
| Sync offline | Status dot turns amber, "Offline — N changes queued"; everything remains editable. |
| Sync conflict | "Merged N changes from <device>" toast; a per-page "History" view shows the merge. |
| Read-only graph (opened from a backup) | Banner + "Duplicate to edit". |

---

## 10. Data model

### 10.1 Entity relationships

```
graph (one per .utopia file)
 └── page (kind: page | daily)
      └── block (forest; parent_id null = root level)
           └── ref edges (src_block → dst_page | dst_block)

page ⇄ page_doc  (1:1 CRDT shard, synced)
block ──► ref     (derived, rebuildable)
```

### 10.2 Schema (SQLite, authoritative)

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE page (
  id          TEXT PRIMARY KEY,          -- uuidv5 (deterministic) or ULID
  kind        TEXT NOT NULL CHECK (kind IN ('page','daily')),
  title       TEXT NOT NULL,
  title_key   TEXT NOT NULL,             -- casefold(trim(title)) — NFKC, whitespace-collapsed
  daily_date  TEXT,                      -- 'YYYY-MM-DD' iff kind='daily'
  created_at  INTEGER NOT NULL,          -- ms since epoch, UTC
  updated_at  INTEGER NOT NULL,
  deleted_at  INTEGER
);
CREATE UNIQUE INDEX page_title_uq ON page(title_key)
  WHERE deleted_at IS NULL AND kind = 'page';
CREATE UNIQUE INDEX page_daily_uq ON page(daily_date)
  WHERE kind = 'daily' AND deleted_at IS NULL;
CREATE INDEX page_recent ON page(updated_at DESC) WHERE deleted_at IS NULL;

CREATE TABLE block (
  id            TEXT PRIMARY KEY,        -- ULID
  page_id       TEXT NOT NULL REFERENCES page(id) ON DELETE CASCADE,
  parent_id     TEXT REFERENCES block(id) ON DELETE CASCADE,
  position      TEXT NOT NULL,           -- fractional index
  depth         INTEGER NOT NULL,
  text          TEXT NOT NULL DEFAULT '',
  block_type    TEXT NOT NULL DEFAULT 'bullet'
                  CHECK (block_type IN ('bullet','numbered','todo','quote','code','heading')),
  heading_level INTEGER CHECK (heading_level BETWEEN 1 AND 3),
  checked       INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  deleted_at    INTEGER
);
CREATE UNIQUE INDEX block_sibling_uq ON block(page_id, ifnull(parent_id,''), position)
  WHERE deleted_at IS NULL;
CREATE INDEX block_page   ON block(page_id, position)   WHERE deleted_at IS NULL;
CREATE INDEX block_parent ON block(parent_id, position) WHERE deleted_at IS NULL;
CREATE INDEX block_updated ON block(updated_at DESC);

CREATE TABLE ref (
  id            INTEGER PRIMARY KEY,
  src_block_id  TEXT NOT NULL REFERENCES block(id) ON DELETE CASCADE,
  src_page_id   TEXT NOT NULL,
  dst_kind      TEXT NOT NULL CHECK (dst_kind IN ('page','block')),
  dst_page_id   TEXT,
  dst_block_id  TEXT,
  dst_title_key TEXT,                    -- unresolved page intents
  anchor_start  INTEGER NOT NULL,
  anchor_end    INTEGER NOT NULL
);
CREATE UNIQUE INDEX ref_uq ON ref(
  src_block_id, dst_kind,
  ifnull(dst_page_id,''), ifnull(dst_block_id,''), ifnull(dst_title_key,'')
);
CREATE INDEX ref_in_page  ON ref(dst_page_id, src_page_id);
CREATE INDEX ref_in_block ON ref(dst_block_id, src_page_id);
CREATE INDEX ref_dangling ON ref(dst_title_key) WHERE dst_page_id IS NULL AND dst_kind = 'page';
CREATE INDEX ref_src      ON ref(src_block_id);

CREATE TABLE alias (
  page_id   TEXT NOT NULL REFERENCES page(id) ON DELETE CASCADE,
  alias_key TEXT NOT NULL,
  PRIMARY KEY (page_id, alias_key)
);
CREATE INDEX alias_key ON alias(alias_key);

-- CRDT shard per page: the synced document
CREATE TABLE page_doc (
  page_id   TEXT PRIMARY KEY REFERENCES page(id) ON DELETE CASCADE,
  snapshot  BLOB NOT NULL,
  frontier  TEXT NOT NULL,               -- encoded version vector
  bytes     INTEGER NOT NULL
);

-- Local outbound queue for sync
CREATE TABLE outbox (
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  page_id    TEXT NOT NULL,
  payload    BLOB NOT NULL,
  client_seq INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  sent_at    INTEGER
);

CREATE TABLE sync_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE VIRTUAL TABLE block_fts USING fts5(
  text, block_id UNINDEXED, page_id UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE trash (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, payload BLOB NOT NULL, deleted_at INTEGER NOT NULL
);
```

### 10.3 Identity rules (important)

| Entity | Id | Rationale |
|---|---|---|
| Page from `[[Title]]` | `uuidv5(NS_PAGE, "title:" + title_key)` | Two devices independently creating a brand-new title offline converge on **one** id. Existing-page and tombstone cases are handled by the resolver and merge rule, not by the id. |
| Daily page | `uuidv5(NS_DAILY, "daily:" + "YYYY-MM-DD")` | Same reason; also makes "did this date exist?" a pure function. |
| Block | ULID (26 chars, time-sortable) | Blocks are authored locally; no coordination needed. |
| Attachment (P2) | SHA-256 of content | Content addressing. |

Consequences (documented as invariants):
- Page ids are **immutable**; renaming never changes identity, and merging never rewrites block ids.
- **Deterministic ids are a duplicate-prevention heuristic, not an invariant.** Creation resolves first (§10.3.1); only if nothing matches is a new page created. If the computed uuidv5 happens to be tombstoned (a page was deleted, or merged away), a fresh ULID is issued — a tombstone is never silently resurrected by a title collision.
- Two **live** pages can never share a `title_key` or an `alias_key`. Merge (§8.6) is the only operation that can create a collision, and it resolves it in the same transaction.
- The title normalization algorithm (case-fold, NFKC, whitespace collapse) is **frozen and versioned**; changing it is a migration, not a code change, because it feeds the uuidv5 namespace.
- None of this is visible to users. They type names; the app maintains the invariant.

### 10.3.1 Resolution pipeline (normative)

This function must be implemented identically on every platform — it is part of the frozen contract (§14.4). It is pure over `(graph state, link target string)`.

```
resolve(target_string, kind) -> Resolution

if kind == BlockRef:
    block = lookup_block(ulid)             # includes tombstones
    if block.is_none:                      return Unresolved(Block)
    return Resolved(Block, deleted = block.deleted_at.is_some())   # "deleted block" chip

# kind == PageRef (from [[...]], #token, or #[[...]])
if d = parse_unambiguous_date(target_string, graph.date_format):
    return Resolved(DailyPage(d))          # step 1: dates win
key = normalize(target_string)
if p = page_by_title_key(key):   return Resolved(p)   # step 2 (non-daily pages only)
if p = page_by_alias_key(key):   return Resolved(p)   # step 3
return Unresolved(Page, key)                          # step 4: intent
```

Design notes:
- **Daily pages are reachable only through step 1.** Title and alias lookups skip `kind='daily'` rows, so a bare display format like `MMM d` ("Nov 2") can never make a daily page reachable under a non-date title.
- **Dates resolve before titles.** A normal page literally titled `2026-11-06` therefore cannot exist alongside the daily page for that date — the daily page *is* that node. The editor refuses to *create* a date-titled normal page; renaming onto one merges instead (§5.5, §8.6).
- `parse_unambiguous_date` accepts ISO-8601, the graph's configured format when that format is unambiguous, and month-name forms. It must reject ambiguous numeric forms rather than guess (§5.5).
- Unresolved intents keep the normalized key, so creating the page later resolves every intent at once (FR-3.12).
- The resolver is called at **index time**, never at paint time; renderers consume stored resolutions.

### 10.4 Invariants (enforced by tests, checked by a `utopia doctor` command)

1. `block.page_id` equals the page of its parent (single-page subtrees).
2. `block.depth` = parent depth + 1, roots = 0.
3. No cycles in `parent_id`.
4. `(page_id, parent_id, position)` unique among live rows.
5. Every live block has exactly one page_doc shard covering it.
6. `ref` rows are exactly the parse of their `src_block.text` (rebuild equality).
7. `page.title_key` is unique among live **non-daily** pages; daily identity is `daily_date`, and daily titles are allowed to repeat.
8. `alias.alias_key` maps to exactly one live page, never shadows a live `title_key`, and is never date-shaped (date resolution precedes alias lookup).
9. No live block belongs to a tombstoned page — merge moves blocks *before* tombstoning (FR-4.13, §8.6).
10. Every `((uid))` whose uid exists resolves; every non-existent uid is still represented as a dangling ref.
11. Resolution is deterministic: identical graph state ⇒ identical result under §10.3.1. This is asserted by a golden test file shared across desktop and mobile implementations.

### 10.5 Text encoding

- UTF-8, NFC-normalized on commit.
- Cursor movement is **grapheme-cluster aware** (use `unicode-segmentation`), not byte- or char-indexed.
- Reference spans are stored as **byte offsets into the NFC text** and recomputed on any edit of that block (they are never synced as authoritative data — they are derived).
- Emoji, CJK, and combining marks are in the acceptance test corpus.

### 10.6 Why markdown-subset text rather than structured rich text

| Option | Pros | Cons |
|---|---|---|
| **Markdown subtext (chosen)** | Export is free; grep-able; plain-text recovery; matches Roam; import/export trivial | Concurrent edits can split a token (`[[Al` … `pha]]`); escaping rules needed |
| Structured inline nodes (atoms) | Atomic tokens, no split risk | Every editor op becomes tree surgery; export is a second implementation; IME complexity |

**Mitigation for the known weakness:** a token split by a concurrent edit is rendered as literal text until the block is next edited, then healed by the "normalize on commit" pass (unclosed `[[` becomes plain text, with an undoable suggestion to fix). Documented, tested, and accepted for v1.

### 10.7 Time and ordering

- All timestamps are Unix ms UTC. Display uses local time.
- Ordering within siblings uses fractional index (lexicographic), **not** timestamps — this makes ordering stable under clock skew, which matters for multi-device merge.
- Hybrid logical clocks (HLC) are carried in the CRDT metadata for LWW fields (page title, block type). Wall clock is display-only and never used to resolve conflicts.

---

## 11. Storage decision (ADR-001)

### 11.1 Decision summary

> **Local system of record: a single SQLite database per graph.**
> **Merge model: Loro CRDT, one document shard per page, persisted as snapshot + update log in the same SQLite file.**
> **Projections: `block`, `page`, `ref`, FTS5 tables are derived indexes over the CRDT state, written in the same transaction as the shard.**
> **Sync: a small self-hostable Rust relay that stores opaque CRDT updates per page; no conflict logic on the server.**
> **Mobile: the same `utopia-core` crate compiled for iOS/Android behind a UniFFI surface, with a native UI shell.**

One sentence: **SQLite is the durable, portable, queryable substrate; the CRDT is the merge algorithm; the relay is a dumb, replaceable pipe.**

### 11.2 Why SQLite is the right substrate here

- **Ubiquitous**: the same file format and engine are available on macOS, Windows, Linux, iOS, and Android with no server. There is no other storage engine with that property and a mature Rust binding (`rusqlite`).
- **One file = one backup**. Users can copy `graph.db` to a USB stick and open it in any SQLite browser. This directly serves the "your data outlives the app" principle.
- **FTS5** gives full-text search (P1) without a second system.
- **Transactions** let us atomically commit {CRDT snapshot, block projection, ref edges, FTS rows, outbox entry}. This is the single most valuable property for correctness: there is no window where the index disagrees with the document.
- **Derived indexes are cheap to rebuild** with SQL, and can be inspected/debugged by any tool.

### 11.3 Why a CRDT at all (vs. last-write-wins op log)

Because the product's core interaction is **two devices editing the same page while offline**, and the most common overlap is *the same block*. LWW at block granularity would silently discard one side's sentence. The user's stated requirement is mobile sync; that guarantees concurrent editing on the same note.

A CRDT also gives us:
- **Movable-list semantics** for sibling order (concurrent reorders converge deterministically).
- **Text merge** for a block body (concurrent inserts both survive).
- **Undo that survives remote updates** (command-level undo, CRDT-aware).
- **Causal consistency** for reference resolution (a ref never resolves before its target arrives).

### 11.4 Why Loro specifically

| Criterion | Loro | Automerge | Yjs/yrs |
|---|---|---|---|
| Native Rust core | ✅ | ✅ | ✅ (yrs) |
| Text CRDT with marks | ✅ | ✅ | ✅ |
| **Movable list** (outliner reorder) | ✅ | partial (list moves are awkward) | ❌ (needs workaround) |
| Tree container | ✅ | ❌ | ❌ |
| History GC / shallow snapshots | ✅ (shallow snapshot encoding) | limited | partial |
| Oplog/state separation, cursors, version vectors | ✅ first-class | ✅ | partial |
| Mobile bindings | ✅ `loro-ffi` (UniFFI), `loro-react-native` | ✅ Swift/Kotlin (mature) | ✅ via community |
| Doc size / speed | strong published benchmarks | improved in v3, historically larger | strong |

**Choice: Loro.** The deciding factor is the **movable list + tree + shallow snapshot** combination, which maps directly onto "blocks in an ordered tree that get reordered" and "a graph that lives for ten years". Automerge is the documented fallback if Loro's mobile FFI proves insufficient; the swap is contained in `utopia-doc` behind a narrow trait (§13.1) so it is a 1-module change, not a rewrite.

> Note: this is a **contained** abstraction, not a generic CRDT framework. We do not attempt to support two CRDTs simultaneously.

### 11.5 Alternatives considered and rejected

| Option | Why rejected |
|---|---|
| **Turso/libSQL offline sync** | Bi-directional offline sync is beta-grade with **conflict detection but no resolution**, and the vendor's own docs at launch said it was not production-ready; it syncs whole databases and owns the sync path. Also introduced corporate uncertainty (2026 Supabase announcement). Wrong layer: it solves transport, not document merge. |
| **ElectricSQL** | Excellent Postgres→client read sync, but writes go through a server API; poor fit for offline-first editing with no backend in the critical path. |
| **PowerSync** | Strong product, but server-authoritative Postgres sync, no first-class Rust client, and heavy for a single-user notes app. |
| **cr-sqlite** | Attractive "CRDT in SQLite" story, but **rich-text CRDT is still unimplemented**, and block-text merge is exactly our hard problem. Also no mobile packaging story comparable to Loro's. |
| **Custom op-log + LWW** | Simplest to build, but loses concurrent same-block edits — a guaranteed user-visible data-loss bug in the target workflow. |
| **Plain files + Syncthing/iCloud** | Whole-file last-writer-wins destroys data on concurrent edits; file-level sync has no merge semantics. Kept only as a *transport* variant (§12.7), never as the merge model. |
| **Git as storage** | Great audit trail, terrible interactive latency, no partial merges, merge conflicts surface to end users. |
| **Datomic (what Roam uses)** | Server-centric, JVM, licensing/ops burden; incompatible with "single file on your phone". |

### 11.6 What lives where

```
~/Library/Application Support/Utopia/graphs/<graph-id>/
├── graph.db          # SQLite: pages, blocks, refs, page_doc blobs, FTS, outbox
├── graph.db-wal      # transient
├── blobs/<sha256>    # attachments (P2), content-addressed
└── backup/           # rolling snapshots (see §15.3)
```

Everything except attachments is in one file. `page_doc.snapshot` is a Loro snapshot BLOB; `outbox.payload` is a Loro update BLOB.

### 11.7 Encryption (designed now, shipped v1.1)

Because the relay is a dumb blob store, E2E encryption is a natural extension:

- Each graph has a 256-bit key derived from a user passphrase via **Argon2id** (or a device-managed key in the OS keychain).
- Every `outbox.payload` and `page_doc.snapshot` is encrypted with **XChaCha20-Poly1305**, per-page nonce, AAD = `graph_id || page_id`.
- The server stores ciphertext only and cannot compact. Compaction is client-driven: a client uploads a fresh encrypted snapshot and the server truncates covered update rows.
- Trade-off: key loss = data loss on the server copy; the local DB stays readable (or is SQLCipher-encrypted at rest, separately).

---

## 12. Sync design

### 12.1 Goals

- Works offline indefinitely; syncs when connectivity returns.
- Single-user graph across N personal devices (2–5 typical).
- No server-side CRDT knowledge required (enables self-hosting, encryption, and cheap operations).
- Bounded work per sync: proportional to *changes*, not graph size.
- Conflict outcomes are deterministic and explainable.

### 12.2 Model

- **Sync unit = page shard** (`page_doc`). A page's history is a Loro oplog; devices exchange updates.
- **Server = per-page append-only log + optional snapshot.** It never resolves conflicts.
- **Client = version vector + outbox.** Import is idempotent, so duplicates and reordering are harmless.

### 12.3 Protocol (v1)

```
GET  /v1/graphs/{gid}/manifest
  → 200 { "rev": 912340,
          "pages": [ { "page_id": "...", "head_seq": 812, "frontier": "b64…",
                       "snapshot_seq": 700, "tail_bytes": 12034,
                       "updated_at": 1762000000000, "deleted": false } ] }
     ETag: "912340"        (304 on If-None-Match)

POST /v1/graphs/{gid}/pages/{pid}/updates
  { "updates": [ { "id": "uuid", "client_seq": 41, "frontier": "b64…", "payload": "b64…" } ] }
  → 200 { "accepted": 1, "head_seq": 831, "duplicate": 0 }

GET  /v1/graphs/{gid}/pages/{pid}/updates?since={frontier}
  → 200 { "updates": [ … ] }                       // incremental
  → 200 { "snapshot": "b64…", "frontier": "b64…" }  // when the server judges a snapshot cheaper
```

**Sync loop (client):**
1. `GET manifest` (conditional, every 5 s when foreground, exponential backoff to 60 s when idle).
2. For each page where local frontier ≠ remote head:
   - If local has pending outbox entries → `POST updates` (batched per page, max 1 MB, zstd-compressed body).
   - If remote is ahead → `GET updates?since=local_frontier` and `import`.
3. `import()` is idempotent; applying an update re-derives block rows + refs + FTS in one SQLite tx.
4. On import, broadcast `DocChanged(page_id)` to the UI so open pages re-render incrementally.
5. Snapshot policy: if a page's server tail > 256 KB or > 200 updates, the client uploads a fresh snapshot (`POST /pages/{pid}/snapshot`) and the server truncates covered rows.

**Realtime (optional, v1.1):** a WebSocket that only pushes `{"page_id","head_seq"}` invalidation notices, triggering the same HTTP pull. Keeps the server stateless-ish.

### 12.4 Conflict semantics (the contract we promise users)

| Scenario | Result | Test |
|---|---|---|
| Different blocks, same page | Both survive; page converges | property test with 2 peers, 1k random ops |
| Same block, disjoint text inserts | Both survive, interleaved at their offsets | Loro text semantics |
| Same block, overlapping insert at same offset | Deterministic order by peer id; both survive | two-peer fuzz |
| Delete block on A, add child on B | Block is tombstoned but **revived** as "deleted (has content)" if a live child exists; user can restore | scripted scenario |
| Concurrent reorder of same siblings | Deterministic order (movable list) | permutation fuzz |
| Both rename page | LWW by HLC; loser title becomes alias (no broken links) | scripted |
| Both create "[[Alpha]]" offline | **One page** (deterministic uuidv5) | scripted |
| Both edit the same daily note offline | One page, merged content | scripted |
| Clock skew ±24 h | No impact on ordering (fractional index + HLC) | fuzz with skewed clocks |

**Invariant to test:** for any two peers that exchange all updates, `to_json(page)` is identical. This is the convergence property test run in CI on every PR touching `utopia-doc`.

### 12.5 What never syncs

- Collapse state, sidebar width, scroll position, caret, selection → **local-only** (avoids sync churn and surprise jumps).
- Search index → rebuilt locally.
- Window/UI preferences → local-only, except a small `graph_settings` shard (date format, export defaults).
- The **sync graph id** (the relay path and part of the E2E AAD) is chosen when the graph first syncs and frozen from then on. A copy of a graph file can pick its own id before that, so duplicating `Notes.utopia` starts a second notebook instead of publishing into the original's relay graph.

### 12.6 Failure handling

| Failure | Behavior |
|---|---|
| Network error | Outbox retains payloads; exponential backoff; UI shows queued count |
| 401/403 | Halt sync, prompt re-auth; edits continue locally |
| 409/duplicate | Treated as success (idempotent) |
| Server 5xx | Retry with jitter; never blocks editing |
| Corrupt page snapshot locally | Fall back to replaying retained updates; if that fails, quarantine the shard, mark page read-only from remote, and surface a repair dialog |
| Schema/format upgrade | `protocol_version` in manifest; client refuses to push unsupported versions rather than corrupting the log |

### 12.7 Transport variants (all reuse the same update format)

1. **Hosted relay** (default): `utopia-sync` binary deployed by us or by the user.
2. **Self-hosted**: single binary + SQLite/Postgres + optional S3; `docker run`.
3. **Folder transport** (P2, zero-infra): a sync folder (iCloud Drive, Dropbox, Syncthing, WebDAV) containing `pages/<page_id>/<peer_id>/<seq>.upd`. Append-only, never overwrite, so file-sync tools are safe. The app watches the folder and merges. This is the "no account, no server" path and directly addresses "sync with my phone" for users who already pay for iCloud/Dropbox.
4. **LAN direct** (P3): mDNS + QUIC between desktop and phone at home.

### 12.8 Server data model (reference implementation)

```sql
CREATE TABLE graph (id TEXT PRIMARY KEY, created_at INTEGER, owner_id TEXT);
CREATE TABLE page_log (
  graph_id TEXT NOT NULL, page_id TEXT NOT NULL,
  seq INTEGER NOT NULL, change_id TEXT NOT NULL UNIQUE,
  client_id TEXT, client_seq INTEGER,
  frontier TEXT, payload BLOB NOT NULL, received_at INTEGER NOT NULL,
  PRIMARY KEY (graph_id, page_id, seq)
);
CREATE TABLE page_snapshot (
  graph_id TEXT NOT NULL, page_id TEXT NOT NULL,
  seq INTEGER NOT NULL, frontier TEXT, payload BLOB NOT NULL,
  PRIMARY KEY (graph_id, page_id)
);
CREATE TABLE graph_rev (graph_id TEXT PRIMARY KEY, rev INTEGER NOT NULL); -- bumps on every write
```

Server responsibilities: auth, quota, dedupe by `change_id`, monotonic `seq`, snapshot truncation, manifest generation. **No Loro dependency required** unless we add server-side compaction (optional, with ciphertext off).

---

## 13. Desktop architecture (Rust + GPUI)

### 13.1 Crate layout

```
utopia/
├── crates/
│   ├── utopia-core/    # domain: ids, commands, block tree, markdown parser, undo stack
│   ├── utopia-doc/     # CRDT boundary: Loro schema, apply/project, cursors, frontiers
│   ├── utopia-store/   # SQLite: migrations, projections, refs index, FTS, queries
│   ├── utopia-sync/    # client: outbox, pull loop, backoff, Transport trait
│   ├── utopia-ui/      # GPUI components: bullet row, virtual list, chips, panels
│   ├── utopia-app/     # bin: window, keymap, actions, app assembly
│   ├── utopia-ffi/     # UniFFI surface for mobile/CLI (P2)
│   └── utopia-syncd/   # server bin (axum) — separate deployment
└── xtask/               # packaging, notarization, fixtures, benches
```

Dependency direction: `app → ui → core`; `core` never depends on `store` (store depends on core types); `doc` depends on `core` + `loro`; `store` depends on `core` + `doc` (to project). Keep `loro` out of `core` so the domain stays testable without CRDT semantics.

### 13.2 The CRDT boundary (deliberately narrow)

```rust
pub trait PageDoc {
    fn new(page_id: PageId) -> Self;
    fn from_snapshot(bytes: &[u8]) -> Result<Self>;
    fn snapshot(&self) -> Vec<u8>;
    fn frontier(&self) -> Frontier;
    fn export_since(&self, f: &Frontier) -> Option<Vec<u8>>;
    fn import(&mut self, bytes: &[u8]) -> ImportStatus;

    // Commands are the only mutation path
    fn apply(&mut self, cmd: &Command, author: PeerId) -> Result<DocDelta>;
    fn state(&self) -> PageState;          // blocks, order, metadata
    fn cursor_for(&self, block: BlockId, offset: usize) -> DocCursor;
}

pub enum Command {
    InsertBlock { id: BlockId, parent: Option<BlockId>, after: Option<BlockId>, text: String, kind: BlockKind },
    DeleteBlock { id: BlockId },
    MoveBlock { id: BlockId, parent: Option<BlockId>, after: Option<BlockId> },
    SetBlockText { id: BlockId, replace: TextEdit },
    SetBlockType { id: BlockId, kind: BlockKind, checked: Option<bool> },
    RenamePage { page_id: PageId, title: String },   // may trigger MergePages at the store layer
    SetCollapsed { .. },   // local-only, not a CRDT command
}
```

This is the seam that makes the Loro→Automerge swap containable and lets us unit-test the domain without a CRDT.

### 13.3 Threading model

```
┌──────────────────────── GPUI main thread ────────────────────────┐
│ input → Action → Command → Entity<PageView> (optimistic apply)   │
│ render (virtualized)          ↑ DocDelta / PageVersion           │
└───────────────┬──────────────────────────────────▲───────────────┘
                │ mpsc (bounded, coalescing)       │
┌───────────────▼──────────────────────────────────┴───────────────┐
│ background runtime (tokio, N=2..4)                               │
│  • doc actor per open page (owns PageDoc, serializes apply)       │
│  • SQLite writer thread (single connection, WAL)                  │
│  • sync task (network)                                            │
│  • indexer (refs, FTS, batched)                                   │
└───────────────────────────────────────────────────────────────────┘
```

Rules:
- **No blocking I/O on the main thread.** Ever. A debug build panics on a > 4 ms main-thread stall in a `flush_guard`.
- **Writes are optimistic**: UI applies the command to an in-memory tree immediately; the doc actor confirms or rolls back. Keystroke latency never waits on disk.
- **Disk commit is debounced**: text edits are applied to the CRDT immediately (in memory), snapshot+projection commit at 250 ms idle or 1 s max, and outbox enqueue at 300 ms. A crash loses at most ~1 s of typing, and the WAL + outbox replay recovers.
- **Serialization point**: one writer thread for SQLite, one doc actor per open page. No cross-thread `&mut`.

### 13.4 Rendering strategy

- **Virtualized block list**: measured heights cached per `(block_id, viewport_width, content_version)`. Overscan 8 rows. Rows are GPUI elements built from a lazy iterator.
- **No per-token child views.** Inline marks and reference chips are shaped runs within one line via the platform text system. This is the single largest performance decision in the UI.
- **Element identity stability**: stable element ids per block so GPUI reuses layout; avoid rebuilding the whole list on any keystroke.
- **Incremental repaint**: only the edited row invalidates. The rest of the page does not re-shape.
- **Reference embeds** render a nested, depth-limited block list with its own virtual window (max 100 blocks).

### 13.5 Text editing inside GPUI (the highest-risk area)

GPUI is pre-1.0 (`gpui 0.2.2`, Apache-2.0, published Oct 2025) and has no rich inline text editor out of the box. Plan:

- One `BlockEditor` element implementing `EntityInputHandler` (IME, marked text) modeled on Zed's `editor` internals, but scoped to a single-line-ish block.
- Custom shaping via `WindowTextSystem`; runs carry mark flags (bold/italic/code) and reference spans.
- Grapheme-aware cursor/selection; `Cmd+A` selects block text; `Escape` blurs; `Up/Down` moves between blocks (keeping x-offset).
- Clipboard: our own serializer for subtree copy (Markdown), plus plain text.
- **Spike required before M1** (see M0): IME (Japanese/Korean/Chinese), emoji ZWJ sequences, macOS dead keys, double-click word select, and `Cmd+←/→` line navigation.

### 13.6 State and reactivity

- `RootView` holds `Entity<AppState>`: open pages, sidebar, panels, sync status.
- Each page has `PageState { blocks: im::OrdMap<BlockId, BlockNode>, roots: Vec<BlockId>, version: u64 }` — persistent data structures so snapshots are cheap and diffing is O(changed).
- Commands produce `DocDelta { changed: Vec<BlockId>, inserted, removed }`; the view re-renders only affected rows.
- Undo stack is a `Vec<UndoEntry>` in `utopia-core`, persisted per page in `meta` on close for crash recovery.

### 13.7 Platform integration

| Concern | Approach |
|---|---|
| Menu bar / shortcuts | GPUI actions + native menu on macOS |
| File dialogs | `rfd` |
| Deep links | `utopia://` handler registration (macOS `CFBundleURLTypes`) |
| Autosave | Continuous; no save button |
| Crash safety | WAL + outbox; rolling snapshots every 5 min |
| Updates (P2) | Signed appcast / self-hosted feed |
| Packaging | `cargo-dist` or `cargo-bundle` → `.dmg`, `.msi`, `.AppImage`; notarization via `notarytool` |

---

## 14. Mobile strategy

### 14.1 Constraint

**GPUI does not target iOS or Android.** Mobile is therefore a separate shell that must share the *data model and merge engine*, not the UI code.

### 14.2 Options

| Option | Description | Effort | Risk | Recommendation |
|---|---|---|---|---|
| **A. Rust core + UniFFI + native UI** | `utopia-core`/`doc`/`store` compiled to a static lib; SwiftUI (iOS) and Compose (Android) shells | High | Medium — FFI surface must stay small | **Recommended for v1.2 if mobile is a real product surface** |
| **B. Tauri 2 mobile + web UI** | Reuse `utopia-core` via the same FFI; web front end (Svelte/React) | Medium | Medium — second UI, but one codebase for iOS+Android | **Recommended if speed matters more than GPUI parity** |
| **C. React Native + `loro-react-native` + expo-sqlite** | Reuses only the sync/data format | Low | High — third tech stack, diverging logic | Only for a throwaway read-only client |
| **D. Responsive web client served by the relay** | Same Rust core compiled to WASM (Loro has WASM) | Medium | Medium — requires server to be online | Good companion for "quick capture from phone browser" |

**Recommendation:** Build the desktop first with a **UniFFI-clean core** (no GPUI types below `utopia-ui`). Then ship **Option A (iOS first)** for v1.2. If the team is 1–2 people, take **Option B** instead and accept UI divergence. Either way the `.utopia` file and the sync protocol are the contract, so a third-party client is always possible.

### 14.3 Minimal mobile v1 scope (when it happens)

Daily note (today only) · bullet capture · page/block refs (insert) · backlinks view · full offline · sync. **No** drag-and-drop, no graph view, no plugins.

### 14.4 The mobile-critical contract

| Contract | Why it must be frozen before mobile work starts |
|---|---|
| `graph.db` schema + migration policy | Mobile must open a DB written by an older desktop build |
| Loro shard format + `protocol_version` | A mobile client that can't import is useless |
| Title normalization algorithm (uuidv5 input) | Changing it duplicates pages across devices |
| Ref resolution rules (alias, dangling) | Backlinks must agree across platforms |
| Sync wire format | Two implementations must interoperate |

---

## 15. Non-functional requirements

### 15.1 Performance budgets

| Metric | Budget | Measured by |
|---|---|---|
| Cold start → window visible | ≤ 400 ms | instrumented trace |
| Hot start → caret in daily note | ≤ 2 s (goal ≤ 800 ms) | e2e test |
| Open page (5k blocks, warm) | ≤ 50 ms | bench |
| Keystroke → paint | p95 ≤ 16 ms, p50 ≤ 8 ms | `KeyToPaint` trace counter |
| Backlink panel (2k refs) | ≤ 100 ms | bench |
| Full-text search (100k blocks) | ≤ 50 ms | bench |
| Sync incremental (10 changed blocks) | ≤ 500 ms round trip | e2e |
| Memory (100k blocks, 3 open pages) | ≤ 300 MB RSS | soak test |
| DB size (100k blocks) | ≤ 150 MB | fixture |

### 15.2 Reliability

- Zero-data-loss target for local edits: WAL + debounced commits + rolling snapshots (every 5 min, keep 6).
- `utopia doctor` CLI: invariant checks, index rebuild, orphan detection, export-on-corruption.
- Fuzz targets: markdown parser, ref parser, sync import, block move.
- Property tests: convergence (2 peers), undo/redo round-trip, projection == full rebuild.

### 15.3 Backup and recovery

- Rolling snapshots in `backup/` (SQLite `VACUUM INTO`), 6 generations, 5-minute interval.
- "Restore from snapshot" UI action.
- Export always available even when the DB is corrupted (raw block table read).

### 15.4 Accessibility

- Full keyboard operability (no mouse-only action).
- GPUI's accessibility support is nascent (AccessKit integration is in progress upstream); **do not promise VoiceOver/NVDA parity in v1**. Provide: semantic role hints where the API allows, focus order, high-contrast theme, reduced-motion setting, and dynamic type scaling.
- Explicit risk item (§18) with a v1.1 consultation.

### 15.5 Internationalization

- UI strings externalized from day one (`fluent` or simple `rust-i18n`).
- IME correctness is a P0 quality bar, not a P1 feature.
- RTL layout is P2 but text rendering must not break on RTL content.

### 15.6 Privacy and security

- No network calls except sync and update checks. No telemetry by default; opt-in anonymous crash reports.
- Local graph unencrypted at rest in v1 (rely on OS full-disk encryption); SQLCipher option in v1.1.
- Sync tokens stored in the OS keychain, never in `graph.db`.
- Server sees ciphertext only once E2E lands.
- The E2E key-check marker (`setup_e2e_check`, a fixed plaintext sealed under the graph key) is an **offline verifier**: anyone holding the graph file or a rolling backup can test passphrase guesses against it at the Argon2 cost, without the relay. That is the same oracle a relay's ciphertext gives, and the file already holds the notes, so the exposure is passphrase reuse elsewhere; the marker is what makes a copied graph or a lost Keychain item unable to re-key silently (§12.6). A distinct, higher-cost derivation for the marker alone would not remove the oracle (the relay's ciphertext still is one), so the cost stays shared with payload encryption and the trade-off is documented here instead.

### 15.7 Platform support

| OS | Status | Notes |
|---|---|---|
| macOS 13+ (arm64, x64) | Primary | GPUI Metal backend |
| Windows 10+ | Supported | DirectX backend; test IME (CJK) carefully |
| Linux X11/Wayland | Supported | Vulkan; test fractional scaling |

---

## 16. Metrics

**North-star:** weekly notes created in daily pages (capture volume), plus 4-week retention.

| Category | Metric | Target |
|---|---|---|
| Activation | Install → first block typed | ≥ 80% within 5 min |
| Habit | Sessions with a daily note per active week | ≥ 5 |
| Feature adoption | Graphs with ≥ 1 backlink viewed | ≥ 60% |
| Feature adoption | `((refs))` used at least once | ≥ 35% |
| Reliability | Crash-free sessions | ≥ 99.5% |
| Reliability | Sync failures per 1k syncs | ≤ 5 |
| Reliability | Reported merge data-loss incidents | **0** |
| Performance | p95 keystroke latency (opt-in telemetry) | ≤ 16 ms |
| Portability | Exports per active user per month | ≥ 0.3 |

---

## 17. Milestones

Indicative for a 1–2 engineer team; the point is ordering and exit criteria, not dates.

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0 — Spike (1–2 wks)** | GPUI window + virtualized 1,000-row bullet list + per-block text input with IME + SQLite read/write + one daily page. **No sync, no refs.** | Typing feels native in a 1k-block page; CJK IME works; go/no-go on the editor approach recorded in an ADR |
| **M1 — Capture (4–5 wks)** | Daily notes, full outliner ops, hashtags-as-pages, date links, right sidebar with stacked pages, undo/redo, persistence, export Markdown, quick jump | Dogfooded 2 weeks; AC-1.x and AC-2.x pass; workflows W1, W3, W6 pass end-to-end; crash-free ≥ 99% |
| **M2 — Connections (3–4 wks)** | Ref parser + index, backlinks panel, block refs + transclusion, reference inspector, rename/merge, resolve-on-create; co-reference filters as stretch | AC-3.x and AC-4.x pass; W4, W5 pass; convergence fuzz green |
| **M3 — Sync (4–5 wks)** | Relay server, outbox, manifest loop, snapshot compaction, sync UI, 2-device tests | Two machines editing the same page offline converge; AC tests for §12.4 pass |
| **M4 — Mobile (4–8 wks)** | iOS shell (Option A or B) with daily capture + refs + backlinks | Real daily use on phone for 1 week with no data loss |
| **M5 — Polish (2–3 wks)** | Search, todos/headings, slash menu, templates, Roam import, packaging, notarization, docs | Public beta |

**Deliberate sequencing:** the editor spike (M0) gates everything. If GPUI text editing can't hit the quality bar, we learn it in week 2, not month 4 — and the fallback is to embed a `gpui`-hosted custom editor with a simpler v1 interaction model (no cross-block selection, which we already excluded).

---

## 18. Risks

| # | Risk | L | I | Mitigation | Trigger to act |
|---|---|---|---|---|---|
| R1 | **GPUI is pre-1.0**; API churn breaks the app | High | Medium | Pin exact version; wrap all GPUI touches in `utopia-ui`; budget 1 upgrade day per quarter; track upstream breaking changes | A needed feature requires a fork |
| R2 | **Text editing quality** (IME, RTL, emoji, selection) below bar | High | High | M0 spike; grapheme-aware core; corpus tests; single-block editing scope | M0 spike fails → fall back to simpler editor + native text field composition |
| R3 | **Accessibility** insufficient (screen readers) | High | Medium | Don't promise it in v1; keyboard completeness; AccessKit when upstream lands | Enterprise/user demand |
| R4 | **CRDT + index divergence** → wrong backlinks | Medium | High | Rebuildable projections + invariant tests + `doctor` | Any divergence in CI |
| R5 | **Sync complexity creep** (multiplayer, teams) | Medium | High | Hard non-goal; single-user graph; no presence in v1 | A user asks for shared graphs → separate product |
| R6 | **Loro immaturity / mobile FFI gaps** | Medium | High | Contained trait; convergence property tests; Automerge fallback documented | FFI can't support iOS in M4 |
| R7 | **Editor performance at 10k+ blocks** | Medium | Medium | Virtualization + measured heights; cap autosizing work; bench in CI | p95 > 16 ms in bench |
| R8 | **Scope explosion** from "same as Roam" | High | High | Explicit P0/P1/P2 ladder; every new request goes to P1+ | Any P0 addition |
| R9 | **Data loss** from a bug in projection/commit | Low | Critical | Transactional projection, snapshots, outbox replay, export | Any report → freeze features |
| R10 | **License/ecosystem** surprises (gpui Apache-2.0; Loro MIT; SQLite public domain) | Low | Low | Legal review before beta; keep dependency list auditable | New dep with copyleft |
| R11 | **Sync server ops burden** for a solo developer | Medium | Medium | Folder transport + self-host docker image; hosted tier optional | > 100 beta users |

---

## 19. Decisions requested / open questions

1. **Reference node semantics** — this PRD interprets "references node" as *addressable node + typed edge + transclusion + inspector* (§8). If you meant only block transclusion, the inspector's Outgoing/Unresolved tabs (FR-4.6) can slip to P1 — the data model and sync format are unchanged either way. **Confirm.**
2. **Mobile timing** — is a native mobile app in v1.2 acceptable, or must phone access exist at desktop launch? If the latter, we should do the **folder transport** (§12.7.3) in M3 so iCloud/Dropbox carries the graph to a phone immediately.
3. **Sync hosting** — self-host only, or do we operate a hosted relay? Hosting implies accounts, billing, and a privacy policy.
4. **Open source** — if yes, the sync protocol must be public and the server Apache-2.0/MIT from day one (it is designed to be).
5. **Roam import** — is migration from an existing Roam graph a launch requirement? It changes M2 scope by ~1 week (JSON + EDN export shapes).
6. **Graph scale target** — 100k blocks is the design point. Do you expect 1M+? That changes the sharding and search design.
7. **Encryption at rest** — required in v1 for a specific threat model, or acceptable in v1.1?
8. **Naming/branding** — "Utopia" is a placeholder.
9. **Unlinked References: P0 or P1?** Eliason calls the unlinked→linked promotion loop "the best feature I've found for discovering new relationships between information", which argues for P0. Cost is roughly one week (FTS query, in-place rewrite command, panel section). **Recommendation: pull into M2 if M1 lands on schedule.**
10. **Multi-word hashtags.** Our normative rule stores multi-word tags as `#[[Modern Religion]]` and uses a commit-time longest-match heuristic for the un-delimited form (§6.5). Roam users type `#Modern Religion` naturally, so this is a visible papercut. Confirm the rule is acceptable, or budget a two-day disambiguation UX (typed-while-`#` phrase mode) before M1.

---

## 20. Appendices

### A. Roam parity matrix (detailed)

| Roam feature | Our equivalent | P |
|---|---|---|
| `[[Page]]` | Page ref chip | P0 |
| `#tag` | Same edge as `[[tag]]`; same page | P0 |
| `((block-uid))` | Block ref / transclusion | P0 |
| Daily Notes | Journal | P0 |
| Dates as pages (future tagging) | Date links → daily pages, "Scheduled for this day" | P0 |
| Rename merges duplicates + rewrites links | `MergePages` (§8.6) | P0 |
| Linked References | Backlinks panel | P0 |
| Right sidebar, shift-click to open | Stacked sidebar panes | P0 |
| Unlinked References | FTS-backed mentions + "Link" | P1 |
| Reference filtering | Co-reference filter chips | P1 |
| Attributes `k:: v` | Typed properties + page metadata header | P1 |
| `{{embed}}` | Block embed block type | P1 |
| Templates | Journal + page templates | P1 |
| `/` command menu | Slash palette | P1 |
| Convert block → page | Extract-to-page | P1 |
| Quick capture | Global hotkey capture | P1 |
| Aliases | `alias` table + UI | P1 |
| Query blocks | Filtered views (SQL-backed) | P2 |
| Graph view | Force-directed graph | P2 |
| PDF highlights | Embedded PDF + annotations | P2 |
| `roam/js`, Depot | WASM plugin sandbox | P2 (maybe never) |
| In-app automation API | Local HTTP API + `utopia://` | P2 |
| Mobile app | Native shell | P2 |

### B. Markdown subset (normative)

```ebnf
inline      = text | bold | italic | strike | highlight | code
            | page_ref | block_ref | tag | link ;
bold        = "**", text, "**" ;
italic      = "_",  text, "_"  ;
strike      = "~~", text, "~~" ;
highlight   = "^^", text, "^^" ;
code        = "`",  text, "`"  ;
page_ref    = "[[", title, "]]" ;
block_ref   = "((", ulid, "))" ;
tag         = "#", tag_name | "#[[", title, "]]" ;
tag_name    = ( letter | digit | "_" | "-" )+ ;
link        = "[", text, "](", url, ")" ;
```

Rules:
1. No nesting of the same emphasis type; mixed emphasis is allowed one level deep.
2. `\` escapes the next metacharacter, including `[`, `(`, `#`, `*`, `_`, `~`, `^`, `` ` ``.
3. Unclosed delimiters render literally (no partial styling), and are healed on commit.
4. `code` is opaque: no refs, tags, or emphasis are parsed inside it.
5. `#` followed by whitespace or punctuation is literal text, not a tag.
6. `tag_name` may contain spaces **only** through the commit-time longest-match rule (§6.5) or the explicit `#[[Title]]` form; the stored canonical form for a multi-word tag is always `#[[Title]]`.
7. A `title` that parses as an unambiguous date resolves to that day's page (§10.3.1); the literal text the user typed is preserved in the source.
8. `[[` and `((` inside `code` are never references.

### C. Command surface (`utopia-core`)

```rust
pub enum Command {
    InsertBlock { parent: Option<BlockId>, after: Option<BlockId>, text: String, kind: BlockKind },
    DeleteBlock { id: BlockId },
    MoveBlock   { id: BlockId, parent: Option<BlockId>, after: Option<BlockId> },
    IndentBlock { id: BlockId },
    OutdentBlock{ id: BlockId },
    MergeIntoPrevious { id: BlockId },
    SetBlockText { id: BlockId, edit: TextEdit },
    SetBlockKind { id: BlockId, kind: BlockKind, checked: Option<bool> },
    RenamePage    { page_id: PageId, title: String },   // triggers merge on collision (§8.6)
    MergePages    { from: PageId, into: PageId },
    CreatePage    { title: String, kind: PageKind },     // resolves first; never duplicates
    EnsureDailyPage { date: NaiveDate },
    AddPageRef    { block: BlockId, span: Range<usize>, title: String },
    AddBlockRef   { block: BlockId, span: Range<usize>, target: BlockId },
    PromoteMention{ block: BlockId, span: Range<usize>, to_page: PageId },
    ExtractBlockToPage { block: BlockId, title: String },
    SoftDelete    { id: BlockId, cascade: bool },
}
```

Every command returns `Result<DocDelta, DomainError>`; every command is undoable; every command is exercised by the convergence fuzz.

### D. Glossary

| Term | Meaning |
|---|---|
| **Block** | The atomic unit: a bullet with stable id, body, parent, position. |
| **Page / Daily page** | A named root node; daily pages are keyed by calendar date. |
| **Page ref** | `[[Title]]` — an edge to a page, resolved by normalized title or alias. |
| **Block ref** | `((ulid))` — an edge to a block; renders as a chip and optionally a live transclusion. |
| **Reference node** | Any addressable node (page or block) that references point to. |
| **Backlink** | An incoming reference edge, grouped by source page. |
| **Shard** | One page's CRDT document — the unit of sync. |
| **Frontier** | A version vector identifying a set of applied operations. |
| **Projection** | SQLite rows derived from CRDT state; rebuildable, never authoritative. |
| **Outbox** | Durable queue of local updates awaiting upload. |

### E. References

- Roam Research — <https://roamresearch.com>
- Nat Eliason, *Roam: Why I Love It and How I Use It* (2020) — <https://www.nateliason.com/blog/roam> — source for the behavioral model in §0, the practitioner workflows in §4.3, and the rename/merge and date-link rules in §8.6 and §10.3.1.
- Roam data model / Datalog surface — <https://github.com/2b3pro/roam-research-mcp/blob/main/Roam_Research_Datalog_Cheatsheet.md>
- GPUI crate (0.2.2, Apache-2.0) — <https://crates.io/crates/gpui>
- GPUI component library — <https://crates.io/crates/gpui-component>
- Loro CRDT docs (sync, persistence, containers) — <https://loro.dev/docs/tutorial/sync>
- Loro FFI bindings (mobile) — <https://lib.rs/crates/loro-ffi>
- Automerge Swift (fallback evaluation) — <https://automerge.org/automerge-swift/documentation/automerge/automergetext/>
- cr-sqlite (rejected: rich-text CRDT unimplemented) — <https://github.com/vlcn-io/cr-sqlite>
- Turso offline sync beta (rejected: beta, no conflict resolution) — <https://turso.tech/blog/turso-offline-sync-public-beta>
- UniFFI — <https://mozilla.github.io/uniffi-rs/>

---

*End of document. Reviewers: decisions needed in §19 before M1 planning.*
