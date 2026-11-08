# Workspace main pane: page vs graph view vs PDF viewer

## Status (fix/workspace-main-pane-content)

`MainContent { Page, Graph(Entity<GraphView>), Pdf(Entity<PdfView>) }` now owns what the
main column shows, and `render` matches on it, so "one thing at a time" is a state rather
than a precedence between two `Option`s. The tables and line numbers below describe the
machine on `main` before this change.

- **Issue 1 (a page opened while the graph was up was invisible): fixed.** `open(..)` now
  sets the main column back to `Page`, so the graph closes instead of hiding the page.
  Proven by `opening_a_page_closes_the_graph`.
- **Issue 2 (graph over PDF): unrepresentable.** `open_graph` replaces a `Pdf` with `Graph`,
  and `GraphEvent::Close` / `show_graph` return to `Page` — a PDF is never kept alive
  underneath. `GraphEvent` and `PdfEvent` from a view that is no longer on screen no longer
  change the main column. Proven by `graph_over_a_pdf_does_not_bring_it_back`. Visible
  change in that combination only: closing a graph that was opened over a PDF shows the
  page, not the PDF; the PDF is reopened from its block. Restoring it would have meant
  keeping hidden state.
- **Issue 3 (hidden graph panel kept working): handled, not removed.** The panel is still
  kept while the sidebar is hidden (reopening brings it back, like the stacked panes), but
  its main-page and `IndexUpdated` observers return early when `right_open` is false, and
  `show_right` re-reads the origin and the index once when the sidebar returns. Proven by
  `hidden_graph_panel_does_no_work`.

The sidebar `graph_panel` stays orthogonal to `MainContent` (it sits beside the page).
Nothing else listed here was left unhandled.

## What it controls

What the main column shows (`crates/utopia-ui/src/workspace.rs:2720-2724`): a full-screen
graph view wins, then a PDF viewer, then the main `PageView`. Also the sidebar graph panel's
exclusivity with the full-screen graph.

## Where the state lives

| Piece | Location | Set | Cleared |
|---|---|---|---|
| Full-screen graph | `workspace.rs` `graph: Option<Entity<GraphView>>` | `:971` | `:747, :948, :990, :1018, :1031, :1039, :1060, :1254` |
| PDF viewer | `pdf: Option<Entity<PdfView>>` | `:1078` | `:814, :1066, :1255` |
| Sidebar graph panel | `graph_panel: Option<..>` | `:995` | `:966, :1035, :1040` |
| Main page | `main: Entity<PageView>` (always present) | `open :815-816` | – |
| Render order | `:2720-2724` `match (&self.graph, &self.pdf)` | – | – |

Subscriptions pile into `subs` (`:592, :801, :866, :968, :992, :1062`) and are never pruned.

## States

`(graph, pdf)` ∈ {(None, None) → page, (Some, None), (None, Some), (Some, Some)}; `graph_panel`
orthogonal; `main.page` always changes underneath.

## Events and transitions

- `open(page, zoom, sidebar=false)` (`:808-816`): `pdf = None`, `main.open(..)`. **Does not
  clear `graph`.**
- `open_journal` (`:1250-1257`): `end` edit, `graph = None`, `pdf = None`, `main.open_journal`.
- `open_graph` (`:960-972`): `graph_panel = None`, `graph = Some`.
- `GraphEvent::Open/Close` (`:1017-1040`): clears `graph` and opens the page.
- `open_pdf` (`:1066-1078`): `pdf = Some`.

## Impossible or unhandled combinations (evidence)

1. **Pages opened while the graph is up are invisible.** `open` clears `pdf` (`:814`) but not
   `graph`; `open_journal` clears both (`:1254-1255`). Any `open(.., sidebar=false)` that does
   not come from the graph's own click — sidebar page row `:1736`, recent-day row `:1811`,
   pinned row `:2604`, quick jump `Chosen :879`, `open_date`/PrevDay/NextDay `:1298`,
   `open_title`, `open_block` — changes `main` while the render still shows the graph
   (`:2721`). The page "opens" behind the graph and the graph keeps focus.
2. **Graph over PDF.** `open_graph` clears `graph_panel` (`:966`) but not `pdf`, so
   `(Some, Some)` occurs; the graph wins at render and the PDF silently reappears on
   `GraphEvent::Close`. Hidden-but-alive state masked by render order.
3. **Hidden graph panel keeps working.** `graph_panel` can exist while `right_open == false`
   (`hide_right :1370` leaves it); it still reloads its layout on every main-page change
   (`observe(&main) :341-346`) and every `IndexUpdated` (`:351-355`) for a view nobody sees.

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Page
    Page --> Graph : open_graph
    Page --> Pdf : open_pdf
    Graph --> Page : GraphEvent::Close/Open | open_journal
    Graph --> Graph : open(page) from sidebar/quick-jump/date (main changes, graph still shown)
    Pdf --> Page : open(page) | open_journal
    Pdf --> GraphOverPdf : open_graph (pdf kept)
    GraphOverPdf --> Pdf : GraphEvent::Close (pdf reappears)
```

## Would an explicit state type help?

Yes, trivially: one enum, and `open()` sets it to `Page`.

```rust
enum MainContent { Page, Graph(Entity<GraphView>), Pdf(Entity<PdfView>) }
fn open(&mut self, ..) { self.content = MainContent::Page; self.main.update(..); }
```
