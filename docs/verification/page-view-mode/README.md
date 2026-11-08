# Page view mode — page / zoomed / Daily Notes feed

The state type and its three fixes are covered by tests:

- `a_mode_always_names_the_page_it_draws` (unit, `page_view.rs`): `Mode` pairs the page with
  the zoom and the feed, so a feed without a page (issue 5) cannot be held.
- `fr_3_9_leaving_a_zoom_to_reveal_a_block_counts_unlinked_mentions` (issue 1): after
  `reveal_block` leaves a zoom, the unlinked count is fetched by the navigation handler (the
  store's scan counter goes up) and the header draws it.
- `fr_2_7_zooming_within_a_page_closes_the_rename_input` (issue 2) and
  `fr_1_4_leaving_a_zoom_closes_the_date_calendar` (issue 3): both reset with the zoom.

## Captures

Seeded with `cargo run -p xtask -- seed-page-view target/pvm-shots/demo.utopia`, plus one
mention of *Trellis* captured into today's note with the app's own CLI
(`utopia capture "The Trellis outline is ready for review"`), so the content is deterministic
and no keystrokes typed it. `target/pvm-shots/config` was the scratch config dir.

| File | What it proves |
|---|---|
| `01-daily-notes-feed.png` | The Daily Notes feed (journal mode) after the refactor: today open with its block, yesterday's "Click to start writing", folded blank days, "No earlier notes." The mode type did not change the feed's rendering. |
| `02-page-unlinked.png` | A page in page mode: attributes, headings, references, and the Unlinked References header carrying its count (**1**, the captured mention). That count is the section issue 1's fix keeps current when a reveal leaves a zoom. |
| `03-zoomed-header.png` | A zoomed block: the breadcrumb *Trellis › Open questions*, the block's own text as the title, and the zoomed "Referenced by" section. This is the header the rename input used to linger over and the date calendar used to stay open over. |

## Not captured

- **The before/after pair for the rename input and the date calendar.** Both states are
  entered through a title click, and the zoom/breadcrumb clicks needed to leave them through
  the bullet and the breadcrumb. In this session the AppKit synthetic-click path of
  `UTOPIA_SHOT_SCRIPT` missed more often than it landed — the same flakiness recorded in
  `docs/verification/redesign-09-right-sidebar/README.md`, "the pointer moved, but no handler
  ran". Six runs of the title/bullet sequence produced no state change, so those two fixes are
  proven by the acceptance tests above rather than by a picture. The tests fail when the reset
  is removed from `PageView::set_mode` (verified while writing them).
- **The reveal-out-of-a-zoom end state.** `reveal_block` from the Outline panel needs the same
  clicks (open the right sidebar, switch to the Outline tab, click a heading), so its end state
  is `02-page-unlinked.png` only by construction, not by driving the reveal.
