# 0008. Deterministic force-directed graph layout

Status: Accepted. Source: `utopia-core/src/graph.rs` (PRD §16 "Graph view").

## Decision

- Fruchterman–Reingold with a cooling schedule, seeded with SplitMix64. The same graph and
  seed give the same picture, so the view does not reshuffle on every open. A tiny PRNG is
  enough for layout quality and adds no dependency.
- Nodes start on a jittered circle (stable, no two points on top of each other). The ideal
  edge length is for a unit-area frame. A gentle pull to the centre keeps disconnected parts
  on screen.
- Self-loops and out-of-range edges are ignored. The result is scaled into `-1..=1` on both
  axes with the aspect ratio kept.
