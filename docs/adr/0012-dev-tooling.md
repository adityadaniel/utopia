# 0012. Benchmarks, screenshots, the icon and the mobile check

Status: Accepted. Source: `xtask/src/main.rs`, `xtask/src/icon.rs` (PRD §15.1, §13.3, §14,
MC-14; DESIGN.md §3.18).

## Benchmarks (`xtask bench`)

- Release only: debug timings mean nothing. A missed budget exits non-zero.
- The fixture is PRD scale: 100 pages × 1,000 blocks, 2,000 referencing `[[Mindfulness]]`
  and 37 of those also `[[Books]]` (AC-3.6), a 5,000-block page (AC-2.1), and a 1,000-block
  journal day for the app probe.
- A rename that merges is timed when it returns (applied to the in-memory shards, §13.3);
  the durable SQLite commit on the writer thread has its own guard row, which is not a PRD
  budget.
- The MCP headless call (MC-14) opens the store for each sample, as each real call does.
- `--ui` keystroke→frame covers input handling and full render/layout/paint, not GPU
  present or vsync. Memory is measured in a separate process with three pages open.

## Screenshots (`help-shots`, `readme-shots`)

- Mock graphs are seeded through the `Store`, never typed, so each run reads the same.
- The app is run from a bundle-shaped copy (`Utopia.app/Contents/MacOS`), because Settings →
  Assistants shows the path of the running binary and must not show `target/debug`.
- Scripts wait ~1.5 s first (Metal shaders compile on the first frame of a fresh build),
  park the pointer on the empty top bar (a real pointer would paint a hover), and press
  Escape before a shot (a focused block shows raw markup).
- Captures are downscaled, because Settings → Help embeds them with `include_bytes!`.

## Icon

- One square full-bleed picture, `crates/utopia-app/assets/icon.png`. macOS does not round
  an `.icns`, so xtask draws the tile: Apple's grid (824 px tile on a 1024 px canvas, the
  margin holds the shadow and matches neighbours in the Dock), a superellipse with exponent
  5 (closer to the system's continuous corners than a rounded rectangle), and a dropped,
  blurred shadow (a paper-white icon has no edge on a light Dock without it).
- Smaller sizes are resized premultiplied, so the rim does not darken.

## Mobile check

The iPhone app is iOS only (ios-prd §5) and is written for the latest iOS, so it is type
checked and built against that SDK in Swift 6 mode. Kotlin bindings are generated too; a
Kotlin compiler is not needed to generate them.
