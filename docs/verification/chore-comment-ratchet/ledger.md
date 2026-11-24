# Comment sweep ledger: pilot crates

Every comment block in `utopia-core`, `utopia-doc`, `utopia-proto`, `utopia-plugin` and `xtask`, with its category and where its content went (ADR 0001). Line numbers are from the branch just before the strip.

| Category | Blocks |
|---|---|
| Tautology | 257 |
| Decision record | 147 |
| Excuse | 5 |
| Stale | 3 |
| Exempt (SAFETY) | 1 |
| **Total** | **413** |

| Location | Category | Went to | First line |
|---|---|---|---|
| `crates/utopia-core/src/attach.rs:1-9` | Decision record | ADR 0006 | //! Attachment markup (P2). Files live outside the graph file, content-addressed by |
| `crates/utopia-core/src/attach.rs:30` | Tautology | deleted | /// Every attachment a block's text points at, in order. |
| `crates/utopia-core/src/attach.rs:43` | Tautology | deleted | // `{{pdf: blob:H}}` |
| `crates/utopia-core/src/attach.rs:57` | Tautology | deleted | // `[name](blob:H)` / `![name](blob:H)` |
| `crates/utopia-core/src/attach.rs:83` | Tautology | deleted | /// Media type from the file name and the first bytes. |
| `crates/utopia-core/src/attach.rs:112` | Tautology | deleted | /// The markup to insert for a newly attached file. |
| `crates/utopia-core/src/attach.rs:114` | Decision record | ADR 0006 | // Brackets in a file name would end the link text early. |
| `crates/utopia-core/src/attach.rs:126-130` | Decision record | ADR 0006 | /// A PDF highlight (P2): one or more rectangles on one page (one per selected line of |
| `crates/utopia-core/src/attach.rs:134` | Tautology | deleted | /// 0-based page index. |
| `crates/utopia-core/src/attach.rs:140` | Decision record | ADR 0006 | /// Rectangles a highlight may carry (a long selection over many lines). |
| `crates/utopia-core/src/attach.rs:161` | Tautology | deleted | /// Parse the inside of `{{…}}` as a highlight. |
| `crates/utopia-core/src/attach.rs:255` | Tautology | deleted | // A text selection over two lines. |
| `crates/utopia-core/src/attach.rs:318` | Tautology | deleted | // `![[x]]` is not an image. |
| `crates/utopia-core/src/command.rs:1-3` | Decision record | ADR 0004 | //! The mutation vocabulary (PRD §13.2). These primitives are the only way page content |
| `crates/utopia-core/src/command.rs:26` | Tautology | deleted | /// Value of the `block.block_type` column. |
| `crates/utopia-core/src/command.rs:73-74` | Decision record | ADR 0004 | /// Synced page-level metadata, stored in the page's own shard (so aliases and merge |
| `crates/utopia-core/src/command.rs:77-78` | Decision record | ADR 0004 | /// Display title. For daily pages this is ignored: the title is a rendering of |
| `crates/utopia-core/src/command.rs:84-85` | Decision record | ADR 0004 | /// Set when this page was merged away (§8.6); live blocks that later arrive here are |
| `crates/utopia-core/src/command.rs:87` | Tautology | deleted | /// Previous titles and merged titles, as typed. Keys are derived at index time. |
| `crates/utopia-core/src/command.rs:91` | Tautology | deleted | /// A partial update of [`PageMeta`]; `None` leaves the field untouched. |
| `crates/utopia-core/src/command.rs:131` | Tautology | deleted | /// The patch that restores `m`'s current values for every field this patch touches. |
| `crates/utopia-core/src/command.rs:145-146` | Decision record | ADR 0004 | /// Aliases as a set keyed by `title_key`: first spelling wins, sorted by key, and the |
| `crates/utopia-core/src/command.rs:161` | Decision record | ADR 0004 | /// Replace `range` (byte offsets into the current NFC text) with `insert`. |
| `crates/utopia-core/src/command.rs:170-171` | Tautology | deleted | /// Create a block under `parent` (None = page root) directly after sibling `after` |
| `crates/utopia-core/src/command.rs:179-180` | Decision record | ADR 0004 | /// Remove a block (and its subtree) from the document entirely. Only used to invert |
| `crates/utopia-core/src/command.rs:198` | Tautology | deleted | /// Tombstone (`Some(ts)`) or restore (`None`) the listed blocks. |
| `crates/utopia-core/src/command.rs:203-204` | Decision record | ADR 0004 | /// Mark blocks as having moved to another page's shard during a merge (§8.6 step 1). |
| `crates/utopia-core/src/command.rs:213` | Tautology | deleted | /// Commands that, applied in order to the state *after* `self`, restore `before`. |
| `crates/utopia-core/src/command.rs:333` | Decision record | ADR 0004 | /// What a command changed, so views re-render only affected rows (§13.6). |
| `crates/utopia-core/src/command.rs:340-341` | Decision record | ADR 0004 | /// Tree shape or visibility changed (insert/move/delete); depth and liveness of |
| `crates/utopia-core/src/command.rs:354` | Tautology | deleted | /// Every block id whose projection row may need rewriting. |
| `crates/utopia-core/src/date.rs:1-3` | Decision record | ADR 0003 | //! Daily-page dates (PRD §5, §10.3.1). Identity is always ISO `YYYY-MM-DD`; a |
| `crates/utopia-core/src/date.rs:34` | Decision record | ADR 0003 | /// Month-name forms that are always accepted, independent of the graph setting. |
| `crates/utopia-core/src/date.rs:71-72` | Decision record | ADR 0003 | /// Formats this graph used before. Links typed under an earlier setting keep |
| `crates/utopia-core/src/date.rs:91` | Tautology | deleted | /// Switch to `pattern`, remembering the current pattern as a previous one. |
| `crates/utopia-core/src/date.rs:126-127` | Decision record | ADR 0003 | /// A format is unambiguous when it carries year, month and day, and the month is a |
| `crates/utopia-core/src/date.rs:160-161` | Decision record | ADR 0003 | /// `parse_unambiguous_date` from §10.3.1: ISO-8601, the built-in month-name forms, and |
| `crates/utopia-core/src/date.rs:185-195` | Decision record | ADR 0003 | /// A relative date phrase, as people type it into a link: `today`, `yesterday`, |
| `crates/utopia-core/src/date.rs:216` | Tautology | deleted | // `n` units after (sign 1) or before (sign -1) today. |
| `crates/utopia-core/src/date.rs:259-262` | Decision record | ADR 0003 | /// Replace relative date links (`[[tomorrow]]`, `#[[next friday]]`) with links to that |
| `crates/utopia-core/src/date.rs:273-276` | Decision record | ADR 0003 | /// [`expand_relative_date_links`] for a block that is being finished (blur, Enter, |
| `crates/utopia-core/src/date.rs:432-434` | Decision record | ADR 0003 | /// The weekday's full English name ("Thursday") for `date`, for captions above a day |
| `crates/utopia-core/src/date.rs:448` | Decision record | ADR 0003 | // Longest match first so "May" vs "Mar" etc. are unambiguous. |
| `crates/utopia-core/src/date.rs:468-469` | Decision record | ADR 0003 | // Whitespace in literals matches any run of whitespace (including none |
| `crates/utopia-core/src/date.rs:562` | Tautology | deleted | // Tuesday, November 3rd, 2026. |
| `crates/utopia-core/src/date.rs:613` | Tautology | deleted | // Still typing inside the link: untouched. |
| `crates/utopia-core/src/date.rs:616` | Tautology | deleted | // Caret before the link stays put. |
| `crates/utopia-core/src/date.rs:619` | Tautology | deleted | // A real page called "Today" keeps its links. |
| `crates/utopia-core/src/date.rs:624` | Tautology | deleted | // Not links, or not relative: nothing to do. |
| `crates/utopia-core/src/date.rs:639-640` | Tautology | deleted | /// Finishing the block finishes the link the caret is in (a wrapped or auto-closed |
| `crates/utopia-core/src/date.rs:653` | Tautology | deleted | // Elsewhere it behaves like the idle expansion. |
| `crates/utopia-core/src/date.rs:676` | Tautology | deleted | /// DN-2: the weekday caption is a rendering of the date, in English, Monday first. |
| `crates/utopia-core/src/graph.rs:1-5` | Decision record | ADR 0008 | //! Force-directed layout for the graph view (P2, §16 "Graph view — force-directed"). |
| `crates/utopia-core/src/graph.rs:7` | Decision record | ADR 0008 | /// A tiny deterministic generator (SplitMix64); layout quality does not need more. |
| `crates/utopia-core/src/graph.rs:21-22` | Decision record | ADR 0008 | /// Lay out `n` nodes joined by undirected `edges` (pairs of node indices; self-loops and |
| `crates/utopia-core/src/graph.rs:31` | Decision record | ADR 0008 | // Start on a jittered circle: stable and free of coincident points. |
| `crates/utopia-core/src/graph.rs:44` | Decision record | ADR 0008 | // Ideal edge length for a unit-area frame. |
| `crates/utopia-core/src/graph.rs:52` | Tautology | deleted | // Repulsion between every pair. |
| `crates/utopia-core/src/graph.rs:65` | Tautology | deleted | // Attraction along edges. |
| `crates/utopia-core/src/graph.rs:76` | Decision record | ADR 0008 | // Gentle gravity keeps disconnected pieces on screen. |
| `crates/utopia-core/src/graph.rs:93` | Decision record | ADR 0008 | /// Scale and center into `-1..=1`, keeping the aspect ratio. |
| `crates/utopia-core/src/graph.rs:120-121` | Tautology | deleted | // Two triangles joined by nothing: members sit closer to each other than to the |
| `crates/utopia-core/src/graph.rs:139` | Tautology | deleted | // The hub ends up near the middle of its star. |
| `crates/utopia-core/src/ids.rs:1` | Tautology | deleted | //! Identity rules (PRD §10.3). |
| `crates/utopia-core/src/ids.rs:9` | Decision record | ADR 0002 | /// Namespace for pages created from a title: `uuidv5(NS_PAGE, "title:" + title_key)`. |
| `crates/utopia-core/src/ids.rs:11` | Decision record | ADR 0002 | /// Namespace for daily pages: `uuidv5(NS_DAILY, "daily:" + YYYY-MM-DD)`. |
| `crates/utopia-core/src/ids.rs:23-25` | Decision record | ADR 0002 | /// Deterministic id for a page created from a title key. A duplicate-prevention |
| `crates/utopia-core/src/ids.rs:50` | Decision record | ADR 0002 | /// FR-2.1: every block gets a globally unique ULID at creation. |
| `crates/utopia-core/src/ids.rs:59` | Tautology | deleted | /// Milliseconds encoded in the ULID, if this id is a ULID. |
| `crates/utopia-core/src/ids.rs:66` | Decision record | ADR 0002 | /// Shape check for `((uid))` targets: ULIDs and Roam-style short uids. |
| `crates/utopia-core/src/inline.rs:1-5` | Decision record | ADR 0006 | //! Markdown-subset inline grammar (PRD Appendix B) and reference extraction (§7.2). |
| `crates/utopia-core/src/inline.rs:58-59` | Decision record | ADR 0006 | /// `{{embed: ((uid))}}` (or `{{[[embed]]: ((uid))}}`): a block rendered in full, always |
| `crates/utopia-core/src/inline.rs:67-69` | Tautology | deleted | /// The node's byte range in the source. `Text` carries none: it is the whole gap |
| `crates/utopia-core/src/inline.rs:86` | Tautology | deleted | /// `[[Title]]` |
| `crates/utopia-core/src/inline.rs:88-89` | Decision record | ADR 0006 | /// `#tag` or `#[[Title]]` — the same edge as a page ref (FR-2.15), kept distinct |
| `crates/utopia-core/src/inline.rs:91` | Tautology | deleted | /// `((uid))` |
| `crates/utopia-core/src/inline.rs:98` | Tautology | deleted | /// Page title as typed (trimmed) or block uid. |
| `crates/utopia-core/src/inline.rs:132` | Tautology | deleted | /// Parse a block body into inline nodes. |
| `crates/utopia-core/src/inline.rs:178` | Decision record | ADR 0006 | // Unclosed or disallowed nesting: the delimiter is literal text. |
| `crates/utopia-core/src/inline.rs:192-193` | Decision record | ADR 0006 | /// Atomic constructs: code, page refs, tags, block refs, links. These never contain |
| `crates/utopia-core/src/inline.rs:208` | Tautology | deleted | // `![alt](url)`: a Markdown image (attachments use `blob:` urls); one link atom. |
| `crates/utopia-core/src/inline.rs:256-257` | Tautology | deleted | // Attachment widgets read as links: `{{pdf: blob:H}}` opens the PDF, |
| `crates/utopia-core/src/inline.rs:361-362` | Tautology | deleted | /// Title inside `[[...]]`, starting after the opening brackets. Returns the trimmed |
| `crates/utopia-core/src/inline.rs:385` | Decision record | ADR 0006 | // Intraword underscores (snake_case) are not emphasis. |
| `crates/utopia-core/src/inline.rs:397-401` | Decision record | ADR 0006 | /// Where a bare `http://` / `https://` URL starting at `i` ends (Appendix B `bare_url`, |
| `crates/utopia-core/src/inline.rs:430-431` | Decision record | ADR 0006 | /// Whether a link leaves the app: anything but an attachment, a PDF page or a |
| `crates/utopia-core/src/inline.rs:472` | Tautology | deleted | /// Every reference in a block body, in source order (§7.2 `parse`). |
| `crates/utopia-core/src/inline.rs:503-504` | Tautology | deleted | /// Plain-text rendering (markup removed, refs shown by title/uid). Used for previews, |
| `crates/utopia-core/src/inline.rs:531-532` | Decision record | ADR 0006 | /// `Key:: value` at the start of a block (P1 attributes, W7). Returns the key and the |
| `crates/utopia-core/src/inline.rs:547` | Tautology | deleted | /// Byte ranges covered by references, code and links, where mentions never count. |
| `crates/utopia-core/src/inline.rs:563-564` | Decision record | ADR 0006 | /// Unlinked mentions of `title` (FR-3.9): case-insensitive, whole-word, outside any |
| `crates/utopia-core/src/inline.rs:596` | Stale | deleted | /// One word after a bare tag, for the longest-match rule: a run of tag characters. |
| `crates/utopia-core/src/inline.rs:598` | Tautology | deleted | // Returns end offsets of successive words separated by single spaces. |
| `crates/utopia-core/src/inline.rs:621-623` | Decision record | ADR 0006 | /// Commit-time hashtag longest-match (§6.5): a bare `#Word` followed by more words that |
| `crates/utopia-core/src/inline.rs:649-651` | Tautology | deleted | /// The "Tag the whole phrase" affordance (AC-2.8): for the bare tag at `tag_start`, |
| `crates/utopia-core/src/inline.rs:664-665` | Decision record | ADR 0006 | /// Canonical text inserted by autocomplete for a page: `[[Title]]`, or for tags |
| `crates/utopia-core/src/inline.rs:678-680` | Tautology | deleted | /// Rewrite every page-like reference whose target satisfies `matches` to `[[new_title]]` |
| `crates/utopia-core/src/inline.rs:698` | Tautology | deleted | /// "Convert to plain text" for an unresolved intent (FR-4.14). |
| `crates/utopia-core/src/inline.rs:785` | Tautology | deleted | // AC-2.9: unclosed renders literally. |
| `crates/utopia-core/src/inline.rs:810` | Decision record | ADR 0006 | // Refs inside emphasis are still refs; delimiters inside refs don't close. |
| `crates/utopia-core/src/inline.rs:836` | Tautology | deleted | // No such page: the tag ends at the space, nothing rewritten. |
| `crates/utopia-core/src/inline.rs:928` | Tautology | deleted | /// Appendix B `bare_url`: a pasted `https://…` is a link without `[text](url)`. |
| `crates/utopia-core/src/inline.rs:946` | Tautology | deleted | // Not a URL: no host, another scheme, or inside a word or code. |
| `crates/utopia-core/src/inline.rs:950` | Tautology | deleted | // A Markdown link's url is not linked twice. |
| `crates/utopia-core/src/inline.rs:954` | Tautology | deleted | /// Sentence punctuation after a URL, and a bracket that closes around it, stay text. |
| `crates/utopia-core/src/inline.rs:975-976` | Tautology | deleted | /// A URL is opaque (rule 4): its `#fragment` is no tag and its underscores no italics, |
| `crates/utopia-core/src/lib.rs:1-5` | Decision record | ADR 0002 | //! Utopia domain model (PRD §13.1). Pure Rust: no CRDT, no SQL, no UI. |
| `crates/utopia-core/src/lib.rs:28` | Decision record | ADR 0002 | /// Current wall clock as Unix milliseconds (UTC). Display-only; never used for ordering (§10.7). |
| `crates/utopia-core/src/markdown.rs:1-2` | Tautology | deleted | //! Outline ⇄ Markdown (FR-2.10, FR-2.11, G4 export). Block bodies are already markdown |
| `crates/utopia-core/src/markdown.rs:10-11` | Decision record | ADR 0006 | /// Parse pasted text into `(depth, text)` items: one block per non-empty line, list |
| `crates/utopia-core/src/markdown.rs:75` | Tautology | deleted | /// Nested Markdown list for the live subtrees rooted at `roots`. |
| `crates/utopia-core/src/markdown.rs:90` | Tautology | deleted | /// Copy/cut of a subtree (FR-2.11). |
| `crates/utopia-core/src/markdown.rs:95` | Tautology | deleted | /// A whole page as Markdown with its title as a heading. |
| `crates/utopia-core/src/markdown.rs:107` | Tautology | deleted | /// OPML 2.0 outline for pages (§4.1 export, v1.1): `(title, state)` pairs. |
| `crates/utopia-core/src/markdown.rs:165` | Decision record | ADR 0006 | // Width 1 never opened a level; it closes back to the nearest shallower one. |
| `crates/utopia-core/src/outline.rs:1-4` | Decision record | ADR 0007 | //! Outliner operations (PRD §6.3, Appendix C) planned as primitive [`Command`]s. |
| `crates/utopia-core/src/outline.rs:13` | Decision record | ADR 0007 | /// Maximum depth accepted for drag-and-drop (FR-2.9). |
| `crates/utopia-core/src/outline.rs:43-45` | Decision record | ADR 0007 | /// Enter (FR-2.2, §9.1): split at the caret. The tail moves to a new block below; it is |
| `crates/utopia-core/src/outline.rs:94` | Tautology | deleted | /// Cmd+Enter: a new empty block directly above. |
| `crates/utopia-core/src/outline.rs:111` | Tautology | deleted | /// Append a new empty block at the end of the page (or of `parent`). |
| `crates/utopia-core/src/outline.rs:127` | Tautology | deleted | /// Tab (FR-2.4): become the last child of the previous sibling, carrying the subtree. |
| `crates/utopia-core/src/outline.rs:143` | Tautology | deleted | /// Shift+Tab (FR-2.4): become the next sibling of the parent. |
| `crates/utopia-core/src/outline.rs:160-161` | Decision record | ADR 0007 | /// Backspace at offset 0 (FR-2.5): merge into the previous visible block — text appended, |
| `crates/utopia-core/src/outline.rs:223` | Decision record | ADR 0007 | /// Cmd+Shift+↑ (FR-2.6). At the top edge the block becomes its parent's previous sibling. |
| `crates/utopia-core/src/outline.rs:247` | Decision record | ADR 0007 | /// Cmd+Shift+↓ (FR-2.6). At the bottom edge the block becomes its parent's next sibling. |
| `crates/utopia-core/src/outline.rs:271` | Tautology | deleted | /// Cmd+Shift+Backspace: tombstone the block and its live subtree (§6.5). |
| `crates/utopia-core/src/outline.rs:304` | Tautology | deleted | /// Replace a block's text with `new_text` as one minimal edit. |
| `crates/utopia-core/src/outline.rs:317-318` | Decision record | ADR 0007 | /// Drag-and-drop (FR-2.9): rejects drops into the block's own subtree and drops that |
| `crates/utopia-core/src/outline.rs:356-358` | Decision record | ADR 0007 | /// Paste (FR-2.10). A single line is inserted at the caret. Multi-line text becomes one |
| `crates/utopia-core/src/outline.rs:421-428` | Decision record | ADR 0007 | /// Append an indented outline (PRD §13.8 MC-8 `append_blocks`): the blocks go last, under |
| `crates/utopia-core/src/outline.rs:442-445` | Decision record | ADR 0007 | /// [`append_outline`] with one kind for each line, so a caller can append to-dos (the |
| `crates/utopia-core/src/outline.rs:505-507` | Decision record | ADR 0007 | /// Whether an outline of `(depth, text)` items would pass [`MAX_DROP_DEPTH`] when its first |
| `crates/utopia-core/src/outline.rs:514-516` | Tautology | deleted | /// The commands that put `items` after `anchor`, whose parent is `anchor_parent`: a depth-0 |
| `crates/utopia-core/src/outline.rs:524` | Tautology | deleted | // last[d]: the most recent block at depth d; parents[d]: its parent. |
| `crates/utopia-core/src/outline.rs:527-528` | Tautology | deleted | // Children of a freshly inserted block always start empty, so `after` for the first |
| `crates/utopia-core/src/outline.rs:535` | Tautology | deleted | // Sibling of last[d]. |
| `crates/utopia-core/src/outline.rs:538` | Tautology | deleted | // First child of last[d-1]. |
| `crates/utopia-core/src/outline.rs:611` | Tautology | deleted | // Caret mid-block with expanded children: tail becomes first child. |
| `crates/utopia-core/src/outline.rs:622` | Tautology | deleted | // Caret at 0 of non-empty block: empty block above, focus stays. |
| `crates/utopia-core/src/outline.rs:686` | Tautology | deleted | // First child merging into its parent keeps its children in place. |
| `crates/utopia-core/src/outline.rs:696` | Tautology | deleted | // First block at root: nothing to do. |
| `crates/utopia-core/src/outline.rs:740` | Tautology | deleted | // A 12-level chain (depths 0..=11) plus a separate leaf. |
| `crates/utopia-core/src/outline.rs:766` | Tautology | deleted | // Pasting after a non-empty block inserts siblings after it. |
| `crates/utopia-core/src/outline.rs:779` | Tautology | deleted | // Single line goes inline at the caret. |
| `crates/utopia-core/src/outline.rs:785` | Tautology | deleted | /// The depth `n` outline `parse_outline` would give `- d0` … indented one level per line. |
| `crates/utopia-core/src/outline.rs:807-808` | Tautology | deleted | /// MC-8: an appended outline goes last, at the nesting it was written with, after the |
| `crates/utopia-core/src/outline.rs:828-829` | Tautology | deleted | // Under a parent the whole outline goes last under that block, before the page's |
| `crates/utopia-core/src/outline.rs:852-853` | Tautology | deleted | /// MC-8: the empty first block of a fresh page takes the outline's first line instead of |
| `crates/utopia-core/src/outline.rs:872-873` | Tautology | deleted | /// MC-8, FR-2.9: an outline that would go past [`MAX_DROP_DEPTH`] is refused, not |
| `crates/utopia-core/src/query.rs:1-14` | Decision record | ADR 0006 | //! Query blocks (P2, §16 "Query blocks — filtered views, SQL-backed"). |
| `crates/utopia-core/src/query.rs:58-59` | Decision record | ADR 0006 | /// The query expression of a query block, if `text` is one. `Some(Err)` means the block |
| `crates/utopia-core/src/query.rs:69` | Tautology | deleted | /// Parse a query expression. |
| `crates/utopia-core/src/query.rs:197` | Tautology | deleted | /// `[[…]]` with nested brackets balanced. |
| `crates/utopia-core/src/text.rs:1-2` | Tautology | deleted | //! Text helpers shared by the editor and the store (§10.5): grapheme-aware cursor |
| `crates/utopia-core/src/text.rs:29` | Tautology | deleted | /// Start of the word at or before `offset` (Alt+Left / double-click). |
| `crates/utopia-core/src/text.rs:43` | Tautology | deleted | /// End of the word at or after `offset` (Alt+Right / double-click). |
| `crates/utopia-core/src/text.rs:54-61` | Decision record | ADR 0006 | /// Ctrl+T (Emacs `transpose-chars`, Cocoa `transpose:`): swap the two graphemes either |
| `crates/utopia-core/src/text.rs:75` | Decision record | ADR 0006 | // A line break never swaps: that would carry a character onto the other line. |
| `crates/utopia-core/src/text.rs:83` | Tautology | deleted | /// The word range containing `offset`, for double-click selection. |
| `crates/utopia-core/src/text.rs:93` | Tautology | deleted | /// Clamp `offset` down to the nearest char boundary. |
| `crates/utopia-core/src/text.rs:102-103` | Tautology | deleted | /// Minimal single-range edit turning `old` into `new` (common prefix/suffix trim), |
| `crates/utopia-core/src/text.rs:130-138` | Decision record | ADR 0007 | /// Where a caret at byte `caret` of `old` belongs in `new` (§6.5): the text changed under |
| `crates/utopia-core/src/text.rs:157` | Tautology | deleted | /// Apply `edit` to `s`, validating bounds and char boundaries. |
| `crates/utopia-core/src/text.rs:170-172` | Tautology | deleted | /// Wrap/unwrap the selection with `delim` (FR-2.16, §9.1 Cmd+B/I/E). With an empty |
| `crates/utopia-core/src/text.rs:189` | Tautology | deleted | // Selection includes the delimiters. |
| `crates/utopia-core/src/text.rs:198` | Tautology | deleted | // Delimiters sit just outside the selection. |
| `crates/utopia-core/src/text.rs:215-217` | Decision record | ADR 0006 | /// The closing bracket for a typed opening one. Only brackets wrap a selection or pair |
| `crates/utopia-core/src/text.rs:227-229` | Decision record | ADR 0006 | /// Typing an opening bracket over a non-empty selection surrounds it instead of replacing |
| `crates/utopia-core/src/text.rs:248-255` | Decision record | ADR 0006 | /// What typing `typed` at an empty selection does to the brackets around the caret: |
| `crates/utopia-core/src/text.rs:276-277` | Decision record | ADR 0006 | /// Backspace inside an empty pair (`(\|)`, `[\|]`, `{\|}`) deletes both brackets: the range |
| `crates/utopia-core/src/text.rs:321` | Tautology | deleted | /// §6.5: the caret stays with its text when the block changes under the editor. |
| `crates/utopia-core/src/text.rs:324` | Tautology | deleted | // Text put in before the caret (a sync, a healed tag) moves it right. |
| `crates/utopia-core/src/text.rs:329` | Tautology | deleted | // Text taken out before the caret (NFC, an undo) moves it left. |
| `crates/utopia-core/src/text.rs:332` | Tautology | deleted | // A change after the caret leaves it alone. |
| `crates/utopia-core/src/text.rs:334` | Tautology | deleted | // The end of the block stays the end; a caret before `a` stays before `a`. |
| `crates/utopia-core/src/text.rs:338` | Tautology | deleted | // Inside text that the replacement still holds: the same place in it. |
| `crates/utopia-core/src/text.rs:341` | Tautology | deleted | // Inside text that is gone: the end of what replaced it. Before it: no move. |
| `crates/utopia-core/src/text.rs:346` | Tautology | deleted | // Never off a character boundary, and unchanged text is unchanged. |
| `crates/utopia-core/src/text.rs:389` | Tautology | deleted | // Again on the still-selected text: the Roam page-link idiom. |
| `crates/utopia-core/src/text.rs:398` | Tautology | deleted | // Nothing selected, or not a bracket: an ordinary keystroke. |
| `crates/utopia-core/src/text.rs:414` | Tautology | deleted | // `[[` is two pairs, caret in the middle. |
| `crates/utopia-core/src/text.rs:417` | Tautology | deleted | // The closer already there is stepped over, not doubled. |
| `crates/utopia-core/src/text.rs:421` | Tautology | deleted | // Before a word character an opener is just an opener. |
| `crates/utopia-core/src/text.rs:443-444` | Tautology | deleted | /// Ctrl+T swaps the pair around the caret and moves past it; at a line end the two |
| `crates/utopia-core/src/text.rs:456` | Tautology | deleted | // End of the text: the two before it, caret stays. |
| `crates/utopia-core/src/text.rs:458` | Tautology | deleted | // End of a paragraph: same, and the newline stays where it was. |
| `crates/utopia-core/src/text.rs:460-461` | Tautology | deleted | // Start of a paragraph: nothing before it on its line, so nothing moves across the |
| `crates/utopia-core/src/text.rs:463` | Tautology | deleted | // Nothing to do: empty, at the very start, one grapheme only. |
| `crates/utopia-core/src/text.rs:467` | Tautology | deleted | // Multi-byte and a family emoji move as whole graphemes. |
| `crates/utopia-core/src/title.rs:1-3` | Decision record | ADR 0002 | //! Title normalization (PRD §10.3). **Frozen and versioned**: the output feeds the |
| `crates/utopia-core/src/title.rs:9-10` | Decision record | ADR 0002 | /// `title_key`: NFKC, Unicode default case fold, NFKC again (fold can denormalize), |
| `crates/utopia-core/src/title.rs:21` | Decision record | ADR 0002 | /// Display form of a user-typed title: NFC, trimmed, whitespace collapsed. Case is kept. |
| `crates/utopia-core/src/title.rs:43` | Tautology | deleted | // "é" precomposed vs. e + combining acute. |
| `crates/utopia-core/src/title.rs:45` | Tautology | deleted | // Full-width letters fold under NFKC. |
| `crates/utopia-core/src/title.rs:47` | Tautology | deleted | // German sharp s case-folds to "ss". |
| `crates/utopia-core/src/tree.rs:1-5` | Decision record | ADR 0005 | //! In-memory page tree (§6.1, §13.6) and the reference semantics of [`Command`]. |
| `crates/utopia-core/src/tree.rs:20` | Decision record | ADR 0005 | /// All children in order, including tombstoned and moved ones. |
| `crates/utopia-core/src/tree.rs:25` | Decision record | ADR 0005 | /// Fractional sibling position supplied by the document layer; empty in pure models. |
| `crates/utopia-core/src/tree.rs:31` | Decision record | ADR 0005 | /// Derived: belongs to this page (not moved away, nor under a moved ancestor). |
| `crates/utopia-core/src/tree.rs:33-34` | Decision record | ADR 0005 | /// Derived: present and visible. A tombstoned block with a live descendant is |
| `crates/utopia-core/src/tree.rs:42` | Tautology | deleted | /// All root blocks in order, including hidden ones. |
| `crates/utopia-core/src/tree.rs:45` | Decision record | ADR 0005 | /// Bumped on every applied command; views use it to skip redundant work. |
| `crates/utopia-core/src/tree.rs:132` | Tautology | deleted | /// Ancestors of `id`, root first, excluding `id`. |
| `crates/utopia-core/src/tree.rs:140` | Excuse | issue item E1 | break; // corrupted cycle; invariants are checked elsewhere |
| `crates/utopia-core/src/tree.rs:149` | Tautology | deleted | /// Whether `a` is a strict ancestor of `b`. |
| `crates/utopia-core/src/tree.rs:154` | Tautology | deleted | /// `id` and its descendants in pre-order. |
| `crates/utopia-core/src/tree.rs:172` | Tautology | deleted | /// Live blocks in document order with depth (relative to the page root). |
| `crates/utopia-core/src/tree.rs:177-178` | Tautology | deleted | /// Rows to render: live blocks under `zoom` (or the page), skipping children of |
| `crates/utopia-core/src/tree.rs:218-220` | Decision record | ADR 0005 | /// Recompute `present`/`live` for `id`'s subtree, then `live` up its ancestor chain. |
| `crates/utopia-core/src/tree.rs:249` | Tautology | deleted | /// Recompute `present`/`live` for every node. O(n). |
| `crates/utopia-core/src/tree.rs:329` | Tautology | deleted | /// Reference semantics of every command. |
| `crates/utopia-core/src/tree.rs:465-466` | Decision record | ADR 0005 | /// Order-and-content projection used to compare states independent of positions and |
| `crates/utopia-core/src/tree.rs:584` | Tautology | deleted | // A concurrent child arrives under the deleted parent. |
| `crates/utopia-core/src/undo.rs:1-7` | Decision record | ADR 0004 | //! Undo/redo (FR-2.12). Each group stores the forward commands and their inverses per |
| `crates/utopia-core/src/undo.rs:19` | Tautology | deleted | /// Applied in order to undo this step. |
| `crates/utopia-core/src/undo.rs:30-33` | Decision record | ADR 0004 | /// Whether the newest group is still open for coalescing (FR-2.12). |
| `crates/utopia-core/src/undo.rs:36` | Tautology | deleted | /// The next text edit starts its own group. |
| `crates/utopia-core/src/undo.rs:39` | Decision record | ADR 0004 | /// A text edit of `block` at most [`COALESCE_MS`] after `at_ms` joins this group. |
| `crates/utopia-core/src/undo.rs:44` | Tautology | deleted | /// Open for a text edit of `block` made at `at_ms`. |
| `crates/utopia-core/src/undo.rs:49-51` | Decision record | ADR 0004 | /// Whether a group in this state may merge into `top`: only two open states for the same |
| `crates/utopia-core/src/undo.rs:69` | Tautology | deleted | /// Whether the next edit may join this group; only a plain text edit ever opens it. |
| `crates/utopia-core/src/undo.rs:103` | Tautology | deleted | /// The direction of one history transition: a Cmd+Z or a Cmd+Shift+Z. |
| `crates/utopia-core/src/undo.rs:121` | Tautology | deleted | /// Record a freshly performed group. Clears redo. |
| `crates/utopia-core/src/undo.rs:141` | Tautology | deleted | /// Stop the current group from absorbing further edits (e.g. on blur or navigation). |
| `crates/utopia-core/src/undo.rs:148-153` | Decision record | ADR 0004 | /// Take the group `dir` applies out of the history. |
| `crates/utopia-core/src/undo.rs:161-162` | Decision record | ADR 0004 | /// The transition happened: the group is now the other stack's top. It never coalesces, |
| `crates/utopia-core/src/undo.rs:171-172` | Decision record | ADR 0004 | /// The transition did not happen: the group goes back where it came from, unchanged — |
| `crates/utopia-core/src/undo.rs:269` | Tautology | deleted | // Still open for coalescing: a failed undo is not an edit boundary. |
| `crates/utopia-doc/src/lib.rs:1-12` | Decision record | ADR 0009 | //! The CRDT boundary (PRD §13.2): one Loro document per page ("shard"). |
| `crates/utopia-doc/src/lib.rs:51` | Tautology | deleted | /// Opaque version identifier (encoded version vector, hex) — `page_doc.frontier`. |
| `crates/utopia-doc/src/lib.rs:68` | Tautology | deleted | /// Each peer and the number of its changes in this version, sorted, for diagnostics. |
| `crates/utopia-doc/src/lib.rs:79-80` | Decision record | ADR 0009 | /// Whether this version holds a change `other` does not: what a sender asks of a |
| `crates/utopia-doc/src/lib.rs:89-92` | Decision record | ADR 0009 | /// What a copy of a page (a destination's snapshot and updates) holds, for diagnostics |
| `crates/utopia-doc/src/lib.rs:99` | Tautology | deleted | /// Read a copy of a page without building it; see [`CopyState`]. |
| `crates/utopia-doc/src/lib.rs:119-122` | Decision record | ADR 0009 | /// The version that a destination's copy of a page adds up to (its snapshot, then its |
| `crates/utopia-doc/src/lib.rs:132-134` | Tautology | deleted | /// One peer's changes that wait for earlier changes of a page (§12.3 step 3): counters |
| `crates/utopia-doc/src/lib.rs:142-143` | Decision record | ADR 0009 | /// The peers and counters in bytes from [`PageDoc::waiting`], for diagnostics. It reads |
| `crates/utopia-doc/src/lib.rs:178` | Decision record | ADR 0009 | /// The narrow seam between the domain and the CRDT (§13.2). |
| `crates/utopia-doc/src/lib.rs:185-189` | Decision record | ADR 0009 | /// Idempotent. Returns whether anything new was applied. |
| `crates/utopia-doc/src/lib.rs:191-195` | Decision record | ADR 0009 | /// Remote changes this doc received but cannot apply yet, because changes they depend |
| `crates/utopia-doc/src/lib.rs:197` | Tautology | deleted | /// Commands are the only mutation path. |
| `crates/utopia-doc/src/lib.rs:255` | Decision record | ADR 0009 | // Mergeable: two peers creating it concurrently converge on one map. |
| `crates/utopia-doc/src/lib.rs:271` | Tautology | deleted | /// Open a shard from its snapshot plus any later local updates. |
| `crates/utopia-doc/src/lib.rs:333-337` | Decision record | ADR 0009 | /// Rebuild the cached [`PageState`] and id index from the Loro tree. |
| `crates/utopia-doc/src/lib.rs:386-387` | Decision record | ADR 0009 | // '!' sorts below every fractional-index character, so appending the |
| `crates/utopia-doc/src/lib.rs:416` | Tautology | deleted | // Parent pointers follow the children lists exactly. |
| `crates/utopia-doc/src/lib.rs:446-447` | Decision record | ADR 0009 | /// Read back fractional positions after an insert/move. Loro may rearrange sibling |
| `crates/utopia-doc/src/lib.rs:485` | Tautology | deleted | /// The state as a full rebuild from the CRDT would produce it (tests, diagnostics). |
| `crates/utopia-doc/src/lib.rs:763-765` | Decision record | ADR 0009 | // Mirror the command on the cached tree (reference semantics, proven |
| `crates/utopia-doc/src/lib.rs:779` | Excuse | issue item E2 | // A failed command may have partially written; resync the cache. |
| `crates/utopia-doc/src/tests.rs:4` | Tautology | deleted | /// Small deterministic PRNG so the fuzz tests are reproducible without extra deps. |
| `crates/utopia-doc/src/tests.rs:33` | Tautology | deleted | /// A random command that is valid against `st`. |
| `crates/utopia-doc/src/tests.rs:174-175` | Tautology | deleted | // The incrementally maintained cache equals a full rebuild from the CRDT, |
| `crates/utopia-doc/src/tests.rs:191` | Tautology | deleted | // Liveness must agree too. |
| `crates/utopia-doc/src/tests.rs:214-215` | Tautology | deleted | // AC-2.3 in miniature: undo everything and land on the empty page, byte-identical |
| `crates/utopia-doc/src/tests.rs:284` | Tautology | deleted | // Importing the same updates again is a no-op (idempotent import). |
| `crates/utopia-doc/src/tests.rs:408` | Tautology | deleted | // Many inserts at the same spot force Loro to rearrange sibling indexes. |
| `crates/utopia-doc/src/tests.rs:457-458` | Tautology | deleted | /// The relay holds a sender's later changes but not the earlier ones they build on (a |
| `crates/utopia-doc/src/tests.rs:471-472` | Tautology | deleted | /// §12.3 step 3, §2.3.7: an update whose predecessors never arrived applies nothing, and |
| `crates/utopia-doc/src/tests.rs:486-487` | Decision record | ADR 0009 | /// The waiting bytes are the only copy: a snapshot does not hold them, so they must |
| `crates/utopia-doc/src/tests.rs:519-520` | Tautology | deleted | /// §12.3 step 4: a destination that holds only the later change has less than the sender, |
| `crates/utopia-doc/src/tests.rs:536-537` | Tautology | deleted | /// §12.6 diagnostics: a copy that holds only the later change reports it as waiting and |
| `crates/utopia-plugin/src/lib.rs:1-23` | Decision record | ADR 0011 | //! Plugin runtime (P2, §16 "`roam/js`, Depot → WASM plugin sandbox"). |
| `crates/utopia-plugin/src/lib.rs:33` | Decision record | ADR 0011 | /// Instructions a single command may execute. |
| `crates/utopia-plugin/src/lib.rs:35` | Decision record | ADR 0011 | /// Linear memory a plugin may grow to. |
| `crates/utopia-plugin/src/lib.rs:37` | Decision record | ADR 0011 | /// Host calls per run. |
| `crates/utopia-plugin/src/lib.rs:39` | Decision record | ADR 0011 | /// Largest JSON message either way. |
| `crates/utopia-plugin/src/lib.rs:56` | Tautology | deleted | /// `"read"` and/or `"write"`. |
| `crates/utopia-plugin/src/lib.rs:59` | Tautology | deleted | /// Module file, relative to the plugin directory. |
| `crates/utopia-plugin/src/lib.rs:82` | Tautology | deleted | /// The plugin trapped, ran out of fuel or memory, or broke the ABI. |
| `crates/utopia-plugin/src/lib.rs:100` | Tautology | deleted | /// What a command asked the editor to do. |
| `crates/utopia-plugin/src/lib.rs:106` | Tautology | deleted | /// The plugin's whole JSON output. |
| `crates/utopia-plugin/src/lib.rs:110-115` | Decision record | ADR 0011 | /// One run's result, whether it succeeded or not. |
| `crates/utopia-plugin/src/lib.rs:118` | Tautology | deleted | /// The plugin's answer, or why it stopped. |
| `crates/utopia-plugin/src/lib.rs:120` | Tautology | deleted | /// Pages its host writes committed, in the order the writes ran. |
| `crates/utopia-plugin/src/lib.rs:122` | Tautology | deleted | /// Lines the plugin logged, up to the first point that failed. |
| `crates/utopia-plugin/src/lib.rs:124` | Tautology | deleted | /// Instructions consumed. |
| `crates/utopia-plugin/src/lib.rs:129` | Tautology | deleted | /// A run that never reached the module, so nothing changed and nothing was logged. |
| `crates/utopia-plugin/src/lib.rs:139` | Tautology | deleted | /// The plugin's answer, if it finished within its limits. |
| `crates/utopia-plugin/src/lib.rs:152-153` | Decision record | ADR 0011 | /// Per-run host state. `store` points at the caller's `&mut Store`, which outlives the |
| `crates/utopia-plugin/src/lib.rs:191` | Tautology | deleted | /// Copy bytes into plugin memory through its allocator. |
| `crates/utopia-plugin/src/lib.rs:210` | Tautology | deleted | /// Run one API request on behalf of a plugin, enforcing its permissions. |
| `crates/utopia-plugin/src/lib.rs:233-234` | Exempt (SAFETY) | kept | // SAFETY: `store` is the `&mut Store` borrowed by `Plugin::run` for the whole run; |
| `crates/utopia-plugin/src/lib.rs:246` | Tautology | deleted | /// Load the plugin in `dir` (manifest + module). The module is validated now. |
| `crates/utopia-plugin/src/lib.rs:255` | Decision record | ADR 0011 | // The module must stay inside the plugin directory. |
| `crates/utopia-plugin/src/lib.rs:284-287` | Decision record | ADR 0011 | /// Run a command. `input` is merged into `{"command": …}`. |
| `crates/utopia-plugin/src/lib.rs:312-313` | Decision record | ADR 0011 | // From here on the module can run (a start function can call the host too), so |
| `crates/utopia-plugin/src/lib.rs:325-327` | Decision record | ADR 0011 | /// Instantiate the module, hand it the input and unpack its answer. Every failure from |
| `crates/utopia-plugin/src/lib.rs:390-392` | Excuse | issue item E3 | /// Instructions a run consumed. `Store::get_fuel` cannot fail while metering is on |
| `crates/utopia-plugin/src/lib.rs:397` | Decision record | ADR 0011 | /// The two host imports a plugin may use (`call`, `log`). |
| `crates/utopia-plugin/src/lib.rs:428` | Tautology | deleted | /// Every plugin directory under `root` (`<config>/plugins`), loaded or with its error. |
| `crates/utopia-plugin/src/lib.rs:448-449` | Tautology | deleted | /// Issue 3: an unreadable fuel counter reported the whole budget as spent. Nothing is |
| `crates/utopia-plugin/tests/sandbox.rs:1` | Tautology | deleted | //! Plugins run sandboxed WebAssembly against the automation API. |
| `crates/utopia-plugin/tests/sandbox.rs:8-9` | Tautology | deleted | /// A plugin directory from WAT source. `body` is the body of `utopia_run`; data |
| `crates/utopia-plugin/tests/sandbox.rs:87-89` | Decision record | ADR 0011 | /// MC-8: a plugin that declared only `read` cannot append an outline — the permission check |
| `crates/utopia-plugin/tests/sandbox.rs:121` | Tautology | deleted | // Returns the host's response as its output. |
| `crates/utopia-plugin/tests/sandbox.rs:133` | Tautology | deleted | // Reads are allowed with "read". |
| `crates/utopia-plugin/tests/sandbox.rs:156` | Tautology | deleted | // No permissions at all: nothing. |
| `crates/utopia-plugin/tests/sandbox.rs:174` | Tautology | deleted | // An infinite loop runs out of fuel. |
| `crates/utopia-plugin/tests/sandbox.rs:182` | Tautology | deleted | // Memory beyond the cap is refused (memory.grow returns -1), so this traps. |
| `crates/utopia-plugin/tests/sandbox.rs:194` | Tautology | deleted | // Output pointing outside memory is an error, not a crash. |
| `crates/utopia-plugin/tests/sandbox.rs:222` | Decision record | ADR 0011 | // A module importing anything else (say, WASI files) does not instantiate. |
| `crates/utopia-plugin/tests/sandbox.rs:244` | Tautology | deleted | // `load_all` reports each plugin directory, broken ones included. |
| `crates/utopia-plugin/tests/sandbox.rs:286-290` | Decision record | ADR 0011 | /// §4.2 P2 (WASM plugin sandbox), rule 13: a host write commits inside the call, so every |
| `crates/utopia-plugin/tests/sandbox.rs:296` | Tautology | deleted | // Trap. |
| `crates/utopia-plugin/tests/sandbox.rs:298` | Tautology | deleted | // Return something that is not JSON. |
| `crates/utopia-plugin/tests/sandbox.rs:300` | Tautology | deleted | // Claim an output larger than the message cap. |
| `crates/utopia-plugin/tests/sandbox.rs:302` | Tautology | deleted | // Burn the whole fuel budget. |
| `crates/utopia-proto/src/lib.rs:1-8` | Decision record | ADR 0010 | //! v1 wire format (§12.3). Payloads are opaque bytes (CRDT updates, later ciphertext), |
| `crates/utopia-proto/src/lib.rs:12` | Decision record | ADR 0010 | /// Bumped on any incompatible change to the shard or wire format (§12.6, §14.4). |
| `crates/utopia-proto/src/lib.rs:31-34` | Decision record | ADR 0010 | /// Opaque id of the relay store that produced this manifest. A relay whose database |
| `crates/utopia-proto/src/lib.rs:41` | Tautology | deleted | /// Globally unique change id; the relay dedupes on it. |
| `crates/utopia-proto/src/lib.rs:70-71` | Decision record | ADR 0010 | /// Incremental updates, optionally preceded by a snapshot when the requested range was |
| `crates/utopia-proto/src/lib.rs:80` | Decision record | ADR 0010 | /// Client-driven compaction (§11.7): a full snapshot covering `covers_seq`. |
| `xtask/src/main.rs:1-3` | Stale | deleted | //! `cargo run -p xtask --release -- bench`: measure the PRD performance budgets (§15.1 and |
| `xtask/src/main.rs:88` | Tautology | deleted | /// Insert `texts` as a flat list of root blocks after the page's last root. |
| `xtask/src/main.rs:128-129` | Decision record | ADR 0012 | /// 100 pages × 1,000 blocks (100k blocks), 2,000 of them referencing [[Mindfulness]] and |
| `xtask/src/main.rs:179` | Decision record | ADR 0012 | /// Child process: open the graph, load three pages, report resident memory (§15.1). |
| `xtask/src/main.rs:238` | Tautology | deleted | /// Launch the release app with `UTOPIA_PROBE` and parse its measurements. |
| `xtask/src/main.rs:286` | Decision record | ADR 0012 | // The app opens today's journal page; give it a realistic 1,000 blocks. |
| `xtask/src/main.rs:299` | Tautology | deleted | // AC-3.1 / §15.1 backlinks, AC-3.6 co-reference filter. |
| `xtask/src/main.rs:319` | Tautology | deleted | // Full-text search over 100k blocks. |
| `xtask/src/main.rs:331` | Tautology | deleted | // AC-3.5: editing a block in a 100k graph updates its backlinks (commit included). |
| `xtask/src/main.rs:354` | Tautology | deleted | // Keystroke path in debounced mode (the store's share of keystroke→paint). |
| `xtask/src/main.rs:377` | Tautology | deleted | // AC-2.4: indent a block carrying 300 descendants. |
| `xtask/src/main.rs:404` | Tautology | deleted | // AC-2.5: paste 500 lines of nested Markdown as one undo group. |
| `xtask/src/main.rs:419-420` | Tautology | deleted | // AC-2.5, MC-8: the same outline through the assistant path — `Op::AppendOutline`, |
| `xtask/src/main.rs:445` | Tautology | deleted | // AC-4.7: rename onto an existing title merges 1,000 blocks. |
| `xtask/src/main.rs:450-451` | Decision record | ADR 0012 | // §13.3: the merge is applied to the in-memory shards (and visible, undoable) when |
| `xtask/src/main.rs:464` | Decision record | ADR 0012 | // Not a PRD budget: a guard so the background commit stays well under a second. |
| `xtask/src/main.rs:471` | Tautology | deleted | // AC-1.4: backfill 30 days; a day is written by its first character (FR-1.2). |
| `xtask/src/main.rs:484` | Tautology | deleted | // G4: full export. |
| `xtask/src/main.rs:498` | Tautology | deleted | // AC-2.1 / §15.1: open the 5k-block page in a fresh process state (cold shard load). |
| `xtask/src/main.rs:514` | Tautology | deleted | // §15.1 hot start → caret in today's note (data path; window paint not included). |
| `xtask/src/main.rs:524-527` | Decision record | ADR 0012 | // MC-14 (PRD §13.8, design §2.3): one headless assistant call, which is what `utopia |
| `xtask/src/main.rs:559` | Decision record | ADR 0012 | // §15.1 memory: a separate process with three open pages. |
| `xtask/src/main.rs:579-580` | Decision record | ADR 0012 | // A real window: first frame and keystroke → full render/layout/paint (§15.1, |
| `xtask/src/main.rs:611-612` | Tautology | deleted | /// `Info.plist` for the app bundle. `CFBundleURLTypes` claims the `utopia://` scheme so |
| `xtask/src/main.rs:642` | Tautology | deleted | /// Build the app and lay out `target/Utopia.app`. |
| `xtask/src/main.rs:682-685` | Tautology | deleted | /// The mobile pipeline (PRD §14): build the UniFFI core, generate Swift bindings, run the |
| `xtask/src/main.rs:717` | Decision record | ADR 0012 | // Kotlin too: the same surface for an Android shell (compiling it needs kotlinc). |
| `xtask/src/main.rs:747-748` | Decision record | ADR 0012 | // The app is iOS only (ios-prd §5: no iPad, no landscape) and is written for the latest |
| `xtask/src/main.rs:768` | Tautology | deleted | // 1. The bindings against the real core. |
| `xtask/src/main.rs:777` | Tautology | deleted | // 2. The app against the iOS SDK. |
| `xtask/src/main.rs:789` | Tautology | deleted | // 3. The iOS Simulator app. |
| `xtask/src/main.rs:837` | Tautology | deleted | /// `(depth, kind, text)` rows describing a mock page's block tree. |
| `xtask/src/main.rs:840-844` | Tautology | deleted | /// Insert `(depth, kind, text)` rows as a block tree under the page root. |
| `xtask/src/main.rs:853-854` | Tautology | deleted | /// [`insert_tree`] with the sibling a top-level row comes behind, so several calls append in |
| `xtask/src/main.rs:897-899` | Decision record | ADR 0012 | /// The scratch graph for the iPhone app's checks (docs/ios-prd.md §6): the four pages the |
| `xtask/src/main.rs:945-946` | Excuse | issue item E4 | // One entry per call, each behind the one before: the day log's time column comes from the |
| `xtask/src/main.rs:967` | Tautology | deleted | // One to-do is already done, so the day log shows both states of the box (AC-5.1). |
| `xtask/src/main.rs:988-991` | Tautology | deleted | /// The Tasks view's shots (PRD §9.2.2): the help graph, plus to-dos whose date links make |
| `xtask/src/main.rs:1008` | Tautology | deleted | // Commit as the editor does, so each date link writes its day (FR-1.10) and resolves. |
| `xtask/src/main.rs:1027-1029` | Tautology | deleted | /// The mock graph behind the Help screenshots: a small project, a person, and a few days |
| `xtask/src/main.rs:1059` | Tautology | deleted | // A property lower in the outline stays a normal bullet (§3.8 rule 3). |
| `xtask/src/main.rs:1061` | Tautology | deleted | // A nested block that starts collapsed, to show the ring and the caret. |
| `xtask/src/main.rs:1068-1069` | Tautology | deleted | // The alias lives in the properties bar's aliases row (§3.8). It must not collide with |
| `xtask/src/main.rs:1071-1072` | Tautology | deleted | // Blockless pages referenced only by `#atlas` / `#beta`: the sidebar's TAGS section |
| `xtask/src/main.rs:1075` | Tautology | deleted | // A done to-do (muted, struck through, checked box) and a collapsed parent. |
| `xtask/src/main.rs:1101` | Tautology | deleted | // An unlinked mention, so Ana's Unlinked references has something to find. |
| `xtask/src/main.rs:1106` | Tautology | deleted | // A few days of notes, newest last so the feed reads naturally. |
| `xtask/src/main.rs:1144-1145` | Tautology | deleted | // A short page with no linked references but an unlinked mention ("the import plan"), so |
| `xtask/src/main.rs:1153` | Tautology | deleted | // A resolved date link (dates are pages, §3.1) a few days out. |
| `xtask/src/main.rs:1175-1177` | Tautology | deleted | /// Seed a scratch graph for the page-view screenshots (PG-1 … PG-5): a project page with |
| `xtask/src/main.rs:1207` | Tautology | deleted | // A done to-do, an alias for the toolbar and a collapsed parent to show its arrow. |
| `xtask/src/main.rs:1215-1221` | Stale | deleted | /// A scratch graph for the Daily Notes feed screenshots (DN-1 to DN-3): a long today so |
| `xtask/src/main.rs:1251` | Tautology | deleted | // A long page: enough rows to scroll well past one screen. |
| `xtask/src/main.rs:1276` | Tautology | deleted | // Date links use the graph's own title format, so they resolve to the daily pages. |
| `xtask/src/main.rs:1296` | Tautology | deleted | // A page (not a day) linking today: a plain linked reference with children. |
| `xtask/src/main.rs:1320` | Tautology | deleted | // Today, long enough to scroll: the heading has to leave the top of the column. |
| `xtask/src/main.rs:1394` | Tautology | deleted | // Plain-text mention: an unlinked reference on Ana Lovelace's page (FR-3.11). |
| `xtask/src/main.rs:1400` | Tautology | deleted | // Yesterday: written, so it stays open (DN-1). |
| `xtask/src/main.rs:1410` | Tautology | deleted | // Another day linking today: a "Scheduled for this day" reference. |
| `xtask/src/main.rs:1415` | Tautology | deleted | // An older day with notes, below the two empty days the feed folds (DN-1). |
| `xtask/src/main.rs:1433-1435` | Tautology | deleted | /// The demo graph for the Linked References verification shots: the Help graph plus a |
| `xtask/src/main.rs:1470-1471` | Tautology | deleted | // An unlinked mention that follows a link, so the highlight has to be mapped through |
| `xtask/src/main.rs:1485` | Tautology | deleted | // Link yesterday, so the feed shows a day with a Linked References section of its own. |
| `xtask/src/main.rs:1521-1522` | Tautology | deleted | /// A built app, a seeded mock graph and an empty config dir under `target/<name>`, ready |
| `xtask/src/main.rs:1531-1533` | Tautology | deleted | /// Build the app, seed `target/<name>/Demo.utopia` with [`seed_help_graph`] and place the |
| `xtask/src/main.rs:1555-1556` | Decision record | ADR 0012 | // Settings → Assistants names the binary that runs, so it must not show `target/debug`. |
| `xtask/src/main.rs:1560` | Tautology | deleted | // The target dir can be shared through CARGO_TARGET_DIR (this repo's worktrees use one). |
| `xtask/src/main.rs:1580` | Tautology | deleted | /// Run the app once over `script` (steps joined by `; `), light theme. |
| `xtask/src/main.rs:1594` | Tautology | deleted | /// Shrink a capture so its longer side is `max` px, in place, and say by how much. |
| `xtask/src/main.rs:1618-1623` | Tautology | deleted | /// Regenerate the README screenshots and demo GIF in `docs/images`. |
| `xtask/src/main.rs:1630` | Decision record | ADR 0012 | // The first frame after a fresh build waits for the Metal shaders (~1 s). |
| `xtask/src/main.rs:1632-1633` | Decision record | ADR 0012 | // The real pointer may rest over the window and paint a hover; park it on the |
| `xtask/src/main.rs:1643` | Tautology | deleted | // "atlas" ranks the #atlas tag first; the outline shot is the project page. |
| `xtask/src/main.rs:1696` | Tautology | deleted | // The GIF, on a fresh graph so the typing lands in the same note every run. |
| `xtask/src/main.rs:1746` | Tautology | deleted | /// The steps of the README GIF: each `frame` is a `shot` into `dir`, numbered in order. |
| `xtask/src/main.rs:1758-1759` | Tautology | deleted | /// `hold` frames of the same picture: the GIF has one frame rate, so a pause is |
| `xtask/src/main.rs:1768` | Tautology | deleted | /// Type each chunk and capture it at once, while the caret is steady after the key. |
| `xtask/src/main.rs:1771-1772` | Tautology | deleted | // `type` keeps everything after the one space that ends the verb, so a |
| `xtask/src/main.rs:1779` | Tautology | deleted | /// Where the shot scripts park the pointer: the empty middle of the main window's top bar. |
| `xtask/src/main.rs:1782-1783` | Tautology | deleted | /// The keys that put the caret in a new, empty block after the last of today's six |
| `xtask/src/main.rs:1798-1805` | Tautology | deleted | /// Regenerate the screenshots embedded in Settings → Help. |
| `xtask/src/main.rs:1817` | Tautology | deleted | // One run, several captures: each `wait` gives the UI time to settle before `shot`. |
| `xtask/src/main.rs:1820-1821` | Decision record | ADR 0012 | // Leave the block: a focused block shows its raw `[[markup]]`, and Help should |
| `xtask/src/main.rs:1850-1852` | Excuse | issue item E5 | // Settings → Assistants, for the topic that documents it. A click only lands on a |
| `xtask/src/main.rs:1875` | Decision record | ADR 0012 | // Downscale so the binary does not carry full-resolution Retina captures. |
| `xtask/src/icon.rs:1-5` | Decision record | ADR 0012 | //! The app icon. `bundle` builds `Contents/Resources/AppIcon.icns` from one square |
| `xtask/src/icon.rs:13-14` | Decision record | ADR 0012 | /// Apple's macOS icon grid: an 824 px tile centred on a 1024 px canvas. The margin holds |
| `xtask/src/icon.rs:19-20` | Decision record | ADR 0012 | /// The tile is the superellipse `\|x\|^n + \|y\|^n <= 1`, which is close to the system's |
| `xtask/src/icon.rs:23-24` | Decision record | ADR 0012 | /// The tile's shadow: its own shape, moved down and blurred. A paper-white icon has no |
| `xtask/src/icon.rs:29` | Tautology | deleted | /// Every file `iconutil` wants in an `.iconset`, with its side in pixels. |
| `xtask/src/icon.rs:43` | Tautology | deleted | /// How much of the tile's pixel `(x, y)` lies inside the shape, from 4 × 4 samples. |
| `xtask/src/icon.rs:61-62` | Tautology | deleted | /// The 1024 px icon: the picture cut to the tile shape, over the tile's shadow, on a |
| `xtask/src/icon.rs:83` | Tautology | deleted | // The shape is zero outside the tile, so a covered pixel is inside the picture. |
| `xtask/src/icon.rs:89` | Tautology | deleted | // The shadow is black, so only the picture adds colour. |
| `xtask/src/icon.rs:100-101` | Decision record | ADR 0012 | /// The icon at a smaller size. Colours are multiplied by alpha while resizing: averaging |
| `xtask/src/icon.rs:121` | Tautology | deleted | /// Write `out` (an `.icns`) from the square picture at `source`. Needs macOS's `iconutil`. |
| `xtask/src/icon.rs:169` | Tautology | deleted | // The tile reaches its box at the middle of each side. |
| `xtask/src/icon.rs:172` | Tautology | deleted | // The canvas corner, and the corner of the tile's box, which the shape cuts off. |
| `xtask/src/icon.rs:200-201` | Tautology | deleted | // Down the middle column from the top edge to the centre there is no shadow, so |
