# Verifying the main pane content machine (`docs/state-machines/workspace-main-pane-content.md`)

The visible change: opening a page in the main column now closes the full-screen graph
instead of opening it invisibly behind the graph (issue 1). The graph is also never painted
over a live PDF that reappears when the graph closes (issue 2).

Seeded with `cargo run -p xtask -- seed-refs target/main-pane-shots/Demo.utopia`, driven with
`UTOPIA_SHOT_SCRIPT` and a scratch `UTOPIA_CONFIG_DIR`:

```bash
UTOPIA_CONFIG_DIR=/tmp/main-pane-shots/config \
UTOPIA_SHOT_SCRIPT="wait 1200; keys escape; wait 400; keys cmd-shift-g; wait 1200; \
  keys cmd-k; wait 400; type atlas; wait 500; keys enter; wait 900; shot /tmp/main-pane-shots/4-after.png" \
  cargo run -p utopia-app -- --graph target/main-pane-shots/Demo.utopia
```

1. `1-graph-open.png` — Cmd+Shift+G shows the graph.
2. `2-quick-jump-over-graph.png` — Cmd+K and "atlas" with the graph still up.
3. `3-page-wins.png` — after Enter the graph is gone, the window title is "Atlas Launch",
   and the page with its Linked References is what is on screen.

The PDF combination needs a macOS window with a PDF attachment and is covered by
`graph_over_a_pdf_does_not_bring_it_back` (`crates/utopia-ui/tests/workspace.rs`), not by a
shot here.
