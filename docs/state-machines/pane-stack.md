# Pane stack: entries, mode, focus, sideways scroll

## What it controls

The main column's list of open pages (DESIGN.md §4.3, §4.4; PRD §9.2.1 SP-1…SP-9): which
entries exist, whether they show side by side (Panes) or only the last one (Single), which
entry has focus, and how far the pane row is scrolled. `crates/utopia-ui/src/workspace.rs`
holds it; the transitions are pure functions in `crates/utopia-ui/src/pane_stack.rs`.

## Where the state lives

| Piece | Location | Notes |
|---|---|---|
| Entries | `Workspace::stack: Vec<Entity<PageView>>` | `stack[0]` is always the feed. An entry's page and zoom are read from its view (`entries()`), never stored twice. |
| Mode | `Workspace::mode` | Starts from `settings.json` `view_mode`. A ⌘-click in Single switches the window only; the toggle writes the setting. |
| Focus | `Workspace::focused`, `key_focus` | Always the last entry in Single. `key_focus` draws the ring after ⌥←/⌥→. |
| Scroll | `scroll_x`, `scroll_anim` | Clamped every frame to `Geometry::max_scroll`. |
| Saved copy | `local_state` `pane_stack` | Written by `stack_changed` only when the entries differ from `stack_seen`. |
| Per view | `PageView::{in_pane, active, next_link}` | Set by `stack_changed` / `sync_next_links`, never in a frame. |

## Events and transitions

Every change is a `pane_stack::Plan` (keep these old entries, then push one) applied by
`Workspace::apply_plan`:

- Link click in a stack view → `PageViewEvent::Open { click, sidebar, replace }` →
  `open_from(Origin::Pane(i), click)` → `pane_stack::open`.
- Sidebar, Quick jump, `utopia://` → `open()` → `open_from(Origin::Outside, Plain)`.
- × / ⌘W → `close`; Back → `back`; "Close other panes" → `close_others`; trail → `trail`;
  "Daily Notes" → `reset` (`open_journal`); toggle → `set_mode`.
- A rename that merges the shown page (`replace: true`) retargets the same view in place.
- A view's own zoom or a promoted draft day changes an entry without a plan;
  `on_view_changed` notices through `stack_seen` and saves.

## Invariants

1. `stack` is never empty and `stack[0]` shows the feed: a zoom from the feed opens a new
   entry rather than zooming the feed in place.
2. An edit hosted by a view that leaves the stack, or by the pane focus leaves, ends first
   (AGENTS.md: a focused element that is not drawn swallows every key).
3. The pane holding the edit is always drawn, even as a spine or off screen.
4. Each pane `occlude()`s the panes it covers; gpui otherwise hands one click to every
   hitbox under the pointer (found when a spine click scrolled to pane 0).
5. In Single mode only the last entry is active; the others mark themselves stale and
   rebuild on `set_active(true)`.

## Diagram

```mermaid
stateDiagram-v2
    [*] --> Single : view_mode = single (default)
    [*] --> Panes : view_mode = panes
    Single --> Panes : toggle | ⌘-click (window only)
    Panes --> Single : toggle
    state Panes {
        [*] --> Scrolled
        Scrolled --> Animating : open / close / trail / ⌥← ⌥→ / spine
        Animating --> Scrolled : 200 ms, or at once with Reduce motion
        Animating --> Scrolled : wheel (cancels)
    }
```
