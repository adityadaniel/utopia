# AGENTS.md — working on Utopia

Utopia is a local-first, Roam-style outliner for the desktop, written in Rust with
a GPUI front end, a SQLite store, and Loro CRDT documents. **`PRD.md` is the spec.**
Read the section you are touching before changing behaviour, and cite requirement
ids (`FR-2.5`, `AC-4.6`, `§10.3.1`) in test names and ADRs (`docs/adr/`).

## Agents Protocol
- “Make a note” => edit AGENTS.md (shortcut; not a blocker).
- Editor: zed <path>.
- Commits: Conventional Commits (feat|fix|refactor|build|ci|chore|docs|style|perf|test).
- when working on a feature / fix, use new worktree
- When its branch is merged, delete the worktree (`git worktree remove <path>`, its
  `target/` folder too) and the local branch.

## Papercuts

When you hit friction during work — a dead-end tool call, a broken link, a
misleading doc, a footgun config, a missing helper — file it before moving on:

```shell
papercuts add "<what you hit and what would have prevented it>" --tag <area>
```

Don't stop working; file it and push through. Severity: minor (default) for
annoyances, major for time sinks, blocker for hard walls. Run `papercuts schema`
once if you need the full contract. Attach `--cmd`, `--exit`, or `--stderr-file`
when filing tool failures; never feed raw environment dumps.

## Visual system

**`DESIGN.md` is the visual spec ("Quiet paper").** Read it before changing anything on
screen. Every colour, type role, spacing and radius is a named token in
`crates/utopia-ui/src/theme.rs`; components use the tokens and never raw colours, sizes or
shadows (§2.4). The dark theme defines the same token names with contrast-checked values and
follows the system appearance. **The system font is the default UI and body font, and Normal the
default density; the mono font (labels, counts, code) pairs with it: SF Mono with System, IBM Plex
Mono and Menlo with Plex, Geist Mono with Geist** (`theme::label_mono()` / `code_mono()`; the Appearance setting offers System,
IBM Plex Sans and Geist, and Tight, Normal and Relaxed: 16 px text on 22, 24 or 26 px lines). Geometry
is pinned by `crates/utopia-ui/tests/layout.rs`; the visual result is checked by the
`UTOPIA_SHOT_SCRIPT` screenshots described under "Verifying a change". `UTOPIA_APPEARANCE=light|dark`
pins the theme when both schemes are needed from a machine set to one.

## Environment setup (macOS)

```bash
source ~/.cargo/env                                             # rustup is not on PATH by default
export DEVELOPER_DIR="$(xcode-select -p)"                      # a version bump renames the bundle
```

- GPUI is built with the `runtime_shaders` feature, so the Metal toolchain
  (`xcodebuild -downloadComponent MetalToolchain`) is **not** needed. Don't remove that feature.
- `rusqlite` uses the `bundled` feature (SQLite + FTS5 compiled in). Don't link the system SQLite.
  On macOS `utopia-store` adds `bundled-sqlcipher` (encryption at rest; its crypto is bundled,
  no OpenSSL). Elsewhere encryption reports unavailable (`encryption_available()`).
- The first build compiles GPUI and takes several minutes; run long builds in the background.
- Disk is tight on this machine: build with `CARGO_INCREMENTAL=0`, and if `target/debug` balloons
  (stale copies of the GPUI and Loro rlibs pile up when features change) delete it and rebuild.

## Commands (definition of done)

`make` lists the everyday tasks (`make check`, `make dev` for a throwaway graph, `make app` /
`make zip` / `make install` for a signed release bundle). A change is finished only when all of
these pass from the repo root (`make check` runs them):

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

A change to the sync engine or the store's import/outbox path (`crates/utopia-sync`,
`crates/utopia-doc`, `utopia-store`'s `import_batch`, `waiting.rs`, `sync_destination.rs`, the
outbox) also runs `make sync-fuzz`: 2,000 seeds of `crates/utopia-sync/tests/convergence.rs`
(three devices on disk, two relays × two graph ids, switches with and without a reset,
restarts, compaction, edit bursts past the relay's snapshot threshold, and outbox rows marked
sent without a push). It asserts that no page differs between devices without some device's
`utopia sync inspect` naming it, that a device holding changes it cannot apply says so in its
own status, and that every line converges once the devices `inspect` names have repaired.
`make check` runs 20 of those seeds. A failure prints its seed and steps; rerun one with
`UTOPIA_SYNC_FUZZ_SEED=<seed>`. Block ids and Loro peers are random, so a seed can fail on one
run only: rerun it a few times before calling it fixed.

A change to either sync relay (`crates/utopia-syncd`, `relay-cloudflare/`), the wire format or the
sync client also runs `make relay-check`: the Worker's `tsc` and vitest suite, `cargo test -p
utopia-syncd`, then `relay-cloudflare/scripts/test-against-rust-client.sh`, which starts `wrangler dev
--local` (never an account) and runs the relay contract (`crates/utopia-syncd/tests/contract`, the
scenarios `tests/http.rs` also runs against `utopia-syncd`) and the raw differential test
(`tests/differential.rs`, both relays' answers compared, every difference recorded in its
`EXCEPTIONS`) with `UTOPIA_RELAY_REQUIRED=1`, so a missing Worker fails rather than skips. It needs
Node (≥ 22.12) and `npm ci` in `relay-cloudflare/` once; `.github/workflows/relay.yml` runs it in
CI when those paths change. Without `UTOPIA_RELAY_URL` the Worker tests print a skip, so `make
check` needs no Node. A new relay scenario goes into `tests/contract/mod.rs`, which runs it against
both relays.

Run the app with `cargo run -p utopia-app` (binary name `utopia`). The graph is chosen in
this order: `--graph <file>`, `UTOPIA_GRAPH`, the last graph opened (recorded in the config
dir), the only `*.utopia` in `~/Documents/Utopia`; otherwise onboarding asks for a name (§9.3).
`UTOPIA_CONFIG_DIR` overrides the config dir (`keymap.json`, `settings.json`, `last_graph`); point
both env vars at a scratch folder when trying the app so you never touch the user's real graph.
`settings.json` holds local UI preferences (never synced, rule 9), a versioned object:
`{"version": 1, "font": "system" | "ibm-plex-sans" | "geist", "density": "tight" | "normal" | "relaxed",
"indent": "wide" | "narrow", "ref_children": 2, "attribute-blocks": "compact" | "blocks", "pinned": [],
"right_sidebar_width": 380.0, "view_mode": "single" | "panes", "vim": false,
"sync_realtime": "always" | "while_active"}`
(`utopia_ui::appearance`); a missing file or
an unknown value means the default, logged; it is written atomically and keeps keys it does
not know.

Headless commands (`cargo run -p utopia-app -- <cmd>`, or the standalone `ut` from
`cargo run -p utopia-cli -- <cmd>`; both go through `utopia_cli::dispatch`, so add a command
there, never in one binary only): `doctor <file> [--rebuild]` checks
the §10.4 invariants (exit 1 on problems); `export <file> <dir>` writes Markdown (attachments in
`blobs/`), `graph.json` and `graph.opml`; `import-roam <file> <roam.json>` imports a Roam export;
`capture <text…>` appends to today's note (when another process owns the graph and runs the
API, it sends the text to that owner instead, see below); `encrypt` / `decrypt <file>` switch encryption at rest
(`UTOPIA_DB_PASSPHRASE`, `UTOPIA_DB_NEW_PASSPHRASE`, keychain service `utopia-db`, or a
prompt); `serve-api <file> [addr] [--sync]` runs the automation API without a window;
`sync --continuous --graph <file>` (alias: `serve-api <file> --sync`) is the headless
daemon: it owns the graph, runs the app's sync rounds on notices and after local writes, and
serves the API until SIGTERM/SIGINT; `sync launchd-plist --graph <file>` prints a
`RunAtLoad`/`KeepAlive` LaunchAgent for it and installs nothing;
`mcp --graph <file> [--read-only]` is the MCP server on stdio for an assistant client
(PRD §13.8): `--graph` is necessary (MC-13) and each tool call runs in the open app through
`/v1/op`, or headless when the app is closed and the access file's `when_closed` is on;
`mcp config <claude-code|claude-desktop|codex> [--graph <file>]` prints the setup entry for
one of them; `sync setup --graph <file> --mode relay|nearby|folder [--url …] [--folder …]
[--graph-id …]` writes the same per-graph settings Settings → Sync uses, and `sync --graph
<file> [--json]` runs one sync to completion (then exit 0; 2 = Sync is Off, 3 = another
process owns the graph), `sync status` reports the saved settings (and pages that wait for another
device's changes), `sync repair` makes the next sync compare every page with the
destination and send what it lacks, `sync inspect [--deep] [--page …]` compares this device
with the destination read-only and says which device has to act (`docs/sync-modes.md`,
"Debugging sync"; every round that did something also writes a line to
`<config>/logs/sync.jsonl`), and `sync config` changes
them (the continuous daemon exits 2/3 the same way, and on a clean stop flushes the store,
releases the lock and removes its discovery file); the token and E2E passphrase come from
stdin without echo or from
`--token-stdin`/`--passphrase-stdin`, and only ever reach the Keychain (`docs/sync-modes.md`,
"Headless sync"); a `utopia://…` argument opens that link.

**One owner for each graph (MC-4).** A process that writes a graph first takes an exclusive
`flock` on the `<graph>.owner` sidecar (`utopia_store::OwnerLock`, `.owner` appended to the
canonical path; the file stays on disk on purpose). The app (before `Store::open_with_key`,
held as long as its store), `serve-api` and `sync --continuous` (their whole life), `capture`,
`import-roam` and `doctor --rebuild` take it, trying for 2 s; if it stays held they write nothing and stop with
"This graph is already open in Utopia" (the app shows it in the window). `capture` first
sends the text to the owner (`POST /v1/capture` through the graph's discovery file, decision
D10 A) and exits 0 when the owner wrote it; only when there is no discovery file, nothing
listens at its address, or the token is refused (nothing was sent) does it wait for the lock
as above. Any other failure stops it with the owner's message, so a capture is never written
twice. `doctor`, `export`
and `Store::open` do not take it. The kernel releases it when a process stops, so a crash
leaves no lock. A scratch graph next to an open real one still starts: the lock is per graph.

**Looking at the UI.** `UTOPIA_SHOT_SCRIPT` drives the real window and captures it to PNG (an
app may capture its own window without Screen Recording permission), e.g.

```bash
UTOPIA_CONFIG_DIR=/tmp/t UTOPIA_SHOT_SCRIPT="type Met [[Ana]]; keys escape; wait 300; shot /tmp/t/a.png; keys cmd-k; type ana; shot /tmp/t/b.png" \
  cargo run -p utopia-app -- --graph /tmp/t/g.utopia
```

Steps (`;` or newline separated): `keys <k…>`, `type <text>` (lines are trimmed; a second space
after `type` keeps a leading one), `wait <ms>`, `scroll <dy>` (a real AppKit wheel event in the
main pane; negative scrolls down), `click <x> <y>` (a real AppKit left click in points
from the top left of the content; the title bar is transparent (DESIGN.md §3.18), so the
content starts at the window's top edge: halve a Retina PNG's pixels and that is the point), `hover <x> <y>` (move the pointer there without pressing, for hover states), `shot <png>`,
`drop <file>` (as
from Finder),
`ws open-pdf` / `ws highlight <page> <x> <y> <w> <h>` (area) / `ws select <page> <x0> <y0> <x1> <y1>`
(text selection), `target main|last` (which window the steps after it drive and capture —
`last` is the most recently opened, which is how the Settings window is reached:
`keys cmd-,; wait 800; target last; shot …`). Then read the PNG. Do this for any visible
change: it has caught bugs the geometry tests could not.

UI behaviour is tested without a screen: `crates/utopia-ui/tests/workspace.rs` drives the real
`Workspace` through GPUI's test platform (`gpui` `test-support` dev-feature) with simulated
keystrokes and asserts on store state; `crates/utopia-ui/tests/layout.rs` asserts layout
geometry via `debug_selector` bounds; `crates/utopia-ui/tests/settings.rs` drives the keybinding
editor (capture, save, reset, clash). Add a script there for any new interaction or region.
The list keeps rendering the row being edited when it scrolls out of view (`splice_items`
passes the editor's focus handle to `ListState::splice_focusable`): a focused element that is
not rendered gets no keystrokes, so every shortcut would die. Keep that when touching the list.
GPUI's test frames never clear debug bounds, so an element that stops rendering still reports
its old bounds: assert on what replaced it (or on view state), never on `debug_bounds(..).is_none()`
after it was once drawn.

Performance budgets (§15.1 and the timed ACs) — run in release, fails on any miss:

```bash
cargo build --release -p utopia-app            # needed for --ui
cargo run -p xtask --release -- bench --ui      # --ui opens a real window for < 1 s
```

`UTOPIA_PROBE=1` makes the app print its first-frame time and keystroke→frame cost, then quit.

Automation (P2): `UTOPIA_API=1` (or `=127.0.0.1:<port>`) starts the loopback HTTP API with the
app, and so does an assistant access level `read` or `write` for the graph (address
`127.0.0.1:0`). The owner (app or `serve-api`) writes `{url, token}` to
`<config>/api/<graph_key>.json` (mode 0600), one file for each graph; `graph_key` is
`utopia_store::graph_key`, the SHA-256 hex of the canonical graph path (the same key names
the sync settings file). Routes are documented in `crates/utopia-api/src/lib.rs`; `POST /v1/op`
takes one `utopia_api::Op` as JSON (the MCP bridge, PRD §13.8) and is the only route the
access level controls (403 when `!level.permits(op.kind())`, never queued; 400 for a body that
is not an `Op`). The access setting is `<config>/mcp/<graph_key>.json`,
`{"version": 1, "access": "off" | "read" | "write", "when_closed": false}`
(`utopia_api::access`): missing, malformed, an unknown version or value means Off (MC-1); it is
written atomically, read at start-up and by Settings → Assistants, which applies a change to the
running server with no restart (the headless path re-reads it for each call), and no
environment variable or flag raises it. With Off and no `UTOPIA_API` there is no server and no
socket. `utopia_api::Client` is the blocking client for an owner's API. `cargo run -p xtask -- bundle` builds `target/Utopia.app`, whose
Info.plist claims the `utopia://` scheme (registering it with LaunchServices happens when the
bundle is opened, so don't open it on a user's machine without asking). Its icon is built
from one square picture, `crates/utopia-app/assets/icon.png` (full-bleed, at least 1024 px):
`xtask/src/icon.rs` cuts it to the macOS tile shape, adds the shadow and writes
`Contents/Resources/AppIcon.icns` with `iconutil`. To change the icon, replace that file. The
iPhone app's icon is a link to the same file (`mobile/ios/AGENTS.md`), and iOS needs it at
1024 × 1024 px with no alpha channel. An
app run with `cargo run` has no bundle, so it shows the generic icon in the Dock.

Plugins (P2): directories under `<config>/plugins/<name>/` with `plugin.json` + a `.wasm`/`.wat`
module; commands appear in the slash menu. See `crates/utopia-plugin/src/lib.rs` for the ABI and
`examples/plugins/word-count` for a complete one.

Mobile (PRD §14, `docs/ios-prd.md`): `cargo run -p xtask -- mobile` builds `utopia-ffi`,
generates Swift bindings (UniFFI) into `target/mobile/gen`, runs `mobile/ios/Check` against the
real core, type-checks the iPhone app against the iOS SDK in Swift 6 language mode, and
generates Kotlin bindings (`target/mobile/kotlin`) for an Android shell. `--ios` also links
`target/mobile/UtopiaMobile.app` for the simulator. To work in Xcode, `cd mobile/ios && make open`
(XcodeGen writes the project from `mobile/ios/project.yml`). **`mobile/ios/AGENTS.md` has the rules for
work on the iPhone app**: platform (iOS 27), build and run, the simulator, native navigation
bars and toolbars, and its known limits.

Sync (§12; secrets never go into the graph file, §15.6):
- **Settings → Sync is the primary setup path.** Choose Off, Relay, Nearby Mac (mDNS), or
  Shared folder, then Apply. Changes take effect without restarting. Nearby Mac can host a
  token-protected relay; the other Mac discovers it or uses its LAN/Tailscale URL in Relay mode.
  Test connection performs one manifest request in the background. Connect another device
  shows the graph id and connection instructions, never the token or passphrase.
- Non-secret settings live in `<config>/sync/<sha256-of-canonical-graph-path>.json`:
  `{"version":1,"mode":"off|relay|nearby|folder","url":"","folder":null,"host":null}`.
  Writes use a same-directory temporary file and atomic rename. Missing means Off; malformed
  or unsupported settings mean Off with a visible notice. They never enter the CRDT.
- Secret fields are masked; blank retains the saved value. Tokens and E2E passphrases use
  macOS Keychain services `utopia-sync` / `utopia-sync-passphrase`, account = graph id; on Linux
  (the standalone `ut`) the same service/account pairs in `<config>/credentials.json`, mode 0600
  (`utopia_sync::setup::file_secrets`, ADR 0014).
  Tests use the `Secrets` trait's in-memory double. For scripted verification set
  `UTOPIA_SYNC_KEYCHAIN_PREFIX` to a scratch namespace; it prefixes both service names.
- `sync_state` records the stable graph id (initially the graph filename), destination identity,
  and an encrypted fixed key-check marker. The marker contains no password or derived key;
  it refuses changing/removing the E2E passphrase even after the graph is copied. Apply with a
  first passphrase insists on a target it can reach and read with it, then freezes the key
  (Keychain + marker); a launch with saved or environment credentials records the marker only
  once the target has been read, and `VerifiedTarget` refuses to push until then, so an
  unreachable target at launch still starts sync without locking in an unproven key.
  In-place re-keying is not supported.
- Switching destination (relay ↔ folder, another relay) calls `Store::prepare_sync_destination`,
  which queues a full update of every shard and marks uploaded attachments pending again, so
  the new target receives everything, not only unsent edits (`sync-engine-outbox-and-cursor.md`).
- Nearby Mac hosting runs the in-process relay with its database at
  `<config>/sync/<sha256>.relay.db` and needs a token (except when `UTOPIA_LAN_SERVE` hosts).
- Environment variables still override saved settings **for that run**. Transport precedence:
  `UTOPIA_LAN_SERVE`, `UTOPIA_SYNC_URL` (`lan` discovers), `UTOPIA_SYNC_FOLDER`, saved transport.
  `UTOPIA_SYNC_GRAPH`, `UTOPIA_SYNC_TOKEN`, `UTOPIA_SYNC_PASSPHRASE` override their corresponding
  values. Settings names active overrides and disables Apply until relaunched without them.
  Environment credentials are not copied into Keychain or settings files.
- Standalone relay: `cargo run -p utopia-syncd` (env `UTOPIA_SYNCD_ADDR`, `UTOPIA_SYNCD_DB`,
  `UTOPIA_SYNCD_TOKENS`, `UTOPIA_SYNCD_QUOTA_MB`); realtime notices on `/v1/graphs/{gid}/events`.

## Verifying a change (do this before opening a PR)

`make check` proves the code compiles and the tests pass. It does **not** prove the feature
works: every UI bug found in this repo so far — prose that would not wrap, a screenshot drawn
at the wrong size, a second Settings window opening instead of the first being raised — passed
every test and was caught by looking at a picture. So verification has four steps, in order.

**1. `make check`.** fmt, build, test, clippy. Never skip it and never open a PR without it.
The one exception is a change that touches only documentation (`docs/**`, `*.md`,
`.agents/**`, `.claude/**`): no crate includes those files, so `make check` is not needed and
CI does not run for it (`paths-ignore` in `.github/workflows/ci.yml`). Say so in the PR.

**2. Seed a graph with mock data.** Drive the `Store` directly rather than typing content
through keystrokes: it is deterministic, it reads well in a screenshot, and it cannot race the
UI. `seed_help_graph` in `xtask/src/main.rs` is the worked example — a project page, a person,
and a few days of notes that link them, which between them exercise links, backlinks,
attributes, to-dos, nesting and the graph view. Seed into a scratch directory under `target/`,
never into a real graph.

**3. Drive the real app and capture it.** `UTOPIA_SHOT_SCRIPT` with `UTOPIA_CONFIG_DIR` and
`--graph` both pointing at scratch paths, so the user's own notes are never touched:

```bash
UTOPIA_CONFIG_DIR=/tmp/scratch/config \
UTOPIA_SHOT_SCRIPT="wait 900; keys escape; wait 300; keys cmd-shift-t; wait 700; shot /tmp/scratch/today.png" \
  cargo run -p utopia-app -- --graph target/help-shots/Demo.utopia
```

- `keys escape` before a shot: a focused block shows its raw `[[markup]]`, which is not what
  the feature looks like in use.
- The caret blinks (`field::Blink`, Zed's blink manager): steady for 500 ms after the last
  keystroke or caret move, then 500 ms on, 500 ms off. A `shot` itself takes ~400 ms to
  present and capture, so a shot that must show the caret goes right after the key, with no
  `wait`; a shot taken later may land in the off half and is not a bug.
- `wait` generously (500–1200 ms) after anything that opens a panel, loads days or lays out the
  graph. A shot taken too early captures a half-drawn frame and is worse than no shot.
- For a second window (Settings), `keys cmd-,; wait 1200; target last; wait 500; shot …`.

**4. Read every PNG you captured.** Not "it ran without error" — actually look. Check the text
is the mock data you seeded and not stray characters, that nothing overflows its container, and
that the thing you changed is visible and correct. A capture run against a scratch graph that an
earlier run already typed into will show that earlier mess; reseed (step 2) rather than
explaining the mess away. If you cannot see your change in the picture, you have not verified it.

Then save the shots you would want a reviewer to see under
`docs/verification/<branch-name>/` and reference them from the PR (see below). Keep them
downscaled (`sips -Z 1200`); a handful of 150–300 KB PNGs is the budget, not full Retina
captures.

## Opening a PR

CI (`.github/workflows/ci.yml`) runs fmt, build, test and clippy on macOS (and builds, tests and runs `ut` on Linux) for every PR that
changes more than documentation; it is the floor, not the evidence. A PR is reviewable when its description carries:

- **What changed and why**, citing the PRD ids the change implements.
- **Proof of work**: the screenshots from step 4 above, inlined from
  `docs/verification/<branch>/` so they render in the PR, each captioned with what it proves.
- **The checks**, quoted as output (`make check`, test count), not asserted in prose.
- **What you did not do**: anything you left out, worked around, or could not verify, and why.
  A known limit written down is worth more than a clean-looking PR that hides it.

Bugs found and fixed along the way belong in the description too — they are the most useful
part of a review. `.github/pull_request_template.md` holds this as a checklist.

Performance-sensitive work also needs `cargo run -p xtask --release -- bench --ui` locally. CI
deliberately does not run it: a shared runner's timings say nothing about the §15.1 budgets.

## Workspace layout (§13.1)

| Crate | Responsibility | May depend on |
|---|---|---|
| `crates/utopia-core` | Domain: ids, title normalization, dates, inline markdown parser, ref extraction, page tree model, `Command`, `DocDelta`, undo stack, markdown import/export | std + small pure crates. **Never** `loro`, `rusqlite`, `gpui` |
| `crates/utopia-doc` | The CRDT boundary: the `PageDoc` trait and its Loro implementation (one shard per page) | `utopia-core`, `loro` |
| `crates/utopia-store` | SQLite: schema/migrations, projections (`block`, `ref`, `block_fts`, `attr`), resolver, backlinks, rename/merge, trash, outbox, export, `doctor`, query evaluation, page graph, attachments (`<graph>.blobs/<sha256>`), PDF highlights, encryption at rest | `utopia-core`, `utopia-doc`, `rusqlite` |
| `crates/utopia-proto` | Sync wire format v1 (§12.3), shared by client and relay | `serde` only |
| `crates/utopia-sync` | Sync client: `Transport` trait, `SyncEngine` (prepare → `exchange` → apply), the UI-free `runner` round (one round at a time, backoff/reset, refusal policy, nudge) shared by the app and the headless daemon, `HttpTransport`, `FolderTransport`, `e2e::Encrypted`, `realtime::Listener`, in-memory reference relay | `utopia-core`, `utopia-doc`, `utopia-store`, `utopia-proto` |
| `crates/utopia-syncd` | Relay server (axum, §12.8): per-page logs, dedupe, auth, quota, snapshots, WebSocket notices; `lan` = in-process relay + mDNS. **No CRDT dependency** | `utopia-proto`, `rusqlite` |
| `crates/utopia-api` | Local automation API (loopback HTTP, bearer token) and `utopia://` link parsing; `handle(store, Op)` is the one implementation shared with plugins | `utopia-core`, `utopia-store`, `axum` |
| `crates/utopia-mcp` | MCP server on stdio for assistant clients (PRD §13.8): the JSON-RPC wire for the two protocol eras, the tool catalog, the outline format with its id marker, the `Direct` headless backend, the `Remote` backend for the open app's API, the rule that picks one for each call, and the setup text for each client. No `gpui`, no async runtime | `utopia-api`, `utopia-store`, `utopia-core` |
| `crates/utopia-plugin` | WASM plugin sandbox (wasmi: fuel, memory cap, permissioned API calls) | `utopia-api`, `utopia-store`, `wasmi` |
| `crates/utopia-ui` | GPUI components: block editor (IME via `EntityInputHandler`), outline view, chips, panels, sidebar, background sync runner, graph view, PDF viewer (PDFKit on macOS), Settings window (keybinding editor, shortcut sheet, Assistants tab, Help tour) | `utopia-core`, `utopia-store`, `utopia-sync`, `utopia-plugin`, `utopia-api`, `utopia-mcp`, `gpui` |
| `crates/utopia-app` | Binary: window, keymap, actions, app assembly, startup probe, screenshot scripts, API/link inbox, unlock screen; runs the CLI subcommands through `utopia-cli` | everything above |
| `crates/utopia-cli` | The headless commands (`doctor`, `export`, `capture`, `serve-api`, `mcp`, `sync`, …) and `dispatch`, shared by the app and the standalone `ut` binary. **No GPUI**, so `ut` builds on a server | `utopia-api`, `utopia-mcp`, `utopia-store`, `utopia-sync`, `utopia-syncd` |
| `crates/utopia-ffi` | Mobile core: UniFFI surface over the store and sync (§14) | `utopia-store`, `utopia-sync`, `uniffi` |
| `mobile/ios` | SwiftUI shell (`UtopiaMobile`) and the Swift binding check | the generated bindings |
| `xtask` | Dev tasks: `bench` (budgets), `profile-merge`, `bundle`, `mobile`, `help-shots`, `readme-shots` | anything |

Dependency direction is one-way: `app → ui → (plugin → api) → store → doc → core`;
`app → cli → (mcp → api, sync → store)`; `ffi → sync → store`. If you find yourself importing upward, move the type
down instead.

## Architecture rules (do not break these)

1. **The block is the atom (§2.3).** Every mutation of page content is a `utopia_core::Command`
   applied through `PageDoc::apply`. There is no other write path to block content.
2. **CRDT doc is the source of truth; SQLite rows are projections (§11.1).** `block`, `ref`
   and `block_fts` must always be rebuildable from `page_doc` (+ `page_update`) shards. Mutate
   through `Store::execute` / `execute_multi` / `execute_plan`; commands apply to the in-memory
   shards at once and each commit (shard delta, outbox row, every projection) is **one SQLite
   transaction** on the writer thread (`writer.rs`, §13.3). `doctor` asserts "incremental
   projection == full rebuild"; keep that test green.
   - *Strict* mode (default; tests, CLI): commits ship immediately and index reads wait for them.
   - *Debounced* mode (the UI, `AppState`): plain text edits flush after 250 ms idle / 1 s max,
     structural and identity edits ship at once, index reads never wait; the UI re-queries on
     `IndexUpdated`. Anything that reads the index to decide what to write (create, rename,
     merge, commit_block, import) calls `sync_index()` first.
   - All write transactions are `BEGIN IMMEDIATE` (a deferred upgrade fails under WAL).
3. **Resolution happens at index time, never at paint time (§10.3.1).** The renderer reads stored
   resolutions. The resolver order is frozen: unambiguous date → title_key → alias → unresolved.
4. **Identity is frozen (§10.3).** Pages created from a title use `uuidv5(NS_PAGE, "title:" + key)`;
   daily pages use `uuidv5(NS_DAILY, "daily:" + YYYY-MM-DD)`; blocks use ULIDs. If the computed
   uuidv5 is tombstoned, issue a fresh ULID — never resurrect. `normalize_title` (casefold +
   NFKC + whitespace collapse) is versioned: changing it is a migration, not a refactor.
5. **Never lose a character (§2.3.7).** Deleting a block tombstones it (`deleted_at`) and keeps its
   id so `((refs))` render "deleted block". Hard delete exists only for empty-trash.
6. **Dates are pages.** Daily titles are display-only; identity is `daily_date`. Ambiguous numeric
   dates (`11/6/26`) never resolve to a date (§5.5).
7. **Hashtags are page links (FR-2.15).** `#tag`, `#[[Multi Word]]` and `[[tag]]` produce identical
   edges; there is no tags subsystem.
8. **Rename onto an existing title merges (§8.6)** in one transaction and one undo step.
9. **Local-only UI state** (collapse, sidebar width, scroll, caret) never goes into a CRDT shard.
10. **No blocking I/O in GPUI render paths.** Keep `render` cheap; do store work in actions/handlers.
    Network I/O never runs on the UI thread (`sync_runner.rs` exchanges on a background thread).
11. **No panics on user data.** Unknown keymap actions, malformed text, missing blocks: log and
    degrade (e.g. "Missing block" chip). `unwrap()` is only acceptable on invariants that are
    proven locally; say which in an `expect("…")` message, not in a comment.
12. **Blobs are content-addressed and verified (§10.3).** Attachments are written once under
    their SHA-256 and rejected on sync if the bytes don't hash to the name. Blocks reference them
    only through markup (`[name](blob:<sha256>)`, `![…]`, `{{pdf: …}}`, `{{highlight: …}}`).
13. **Every API, plugin and FFI write goes through the same command path** as the editor
    (`utopia_api::handle` / `Store` methods), so undo, references and sync behave identically.
14. **Schema changes are migrations** (`schema.rs`, `PRAGMA user_version`), never edits to an
    existing version: graphs on disk must keep opening (§14.4). Check FK-heavy changes against
    `xtask bench`: SQLite cannot use partial indexes for foreign-key checks. Current: v11
   (v6 `attr`, v7 `attachment`, v8 `tombstone`, v9 attachment `state`, v10 `block_todo` index,
   v11 `page_pending`: remote changes that wait for another device's earlier ones).

## Conventions

- Rust 2021 edition, workspace-level dependency versions in the root `Cargo.toml`
  (`[workspace.dependencies]`). Pin GPUI exactly (`=0.2.2`, risk R1).
- `zed-font-kit` is patched (`[patch.crates-io]` in the root `Cargo.toml`) to the fork
  github.com/adityadaniel/font-kit, branch `utopia/glyph-bounds`, pinned by `rev`: crates.io
  `zed-font-kit 0.14.1-zed` plus one commit in `src/loaders/core_text.rs` `raster_bounds`, so
  glyph bitmaps are measured at the size drawn plus a pixel of room and headings in the system
  font are not clipped (`crates/utopia-ui/tests/glyph_bounds.rs`). CI fails when the patch
  stops applying (a GPUI upgrade moved off 0.14.1-zed): then drop it, or rebase the fork's
  commit onto the new version if the bug is still there. Upstream: zed-industries/zed#45957
  and #63469.
- Errors: `thiserror` enums in library crates (`DomainError`, `StoreError`), `anyhow` only in
  `utopia-app`.
- Timestamps: Unix ms UTC (`i64`). Ordering uses fractional positions, never timestamps (§10.7).
- Text: UTF-8, NFC-normalized on commit; reference spans are byte offsets into the NFC text;
  cursor movement is grapheme-aware (`unicode-segmentation`).
- Tests live next to the code (`#[cfg(test)] mod tests`) for unit behaviour, and in
  `crates/*/tests/` for acceptance scripts. Name acceptance tests after the PRD id, e.g.
  `ac_4_6_merge_preserves_subtrees`. Prefer real SQLite (in-memory or tempfile) over mocks.
- **No code comments** (`docs/adr/0001-no-code-comments.md`). The PRD says *what*; a non-obvious
  *why* goes into a name, a type, a test or an ADR in `docs/adr/`, and a shortcut goes into the
  `comment-excuse` tracking issue. Only `// SAFETY:` on an `unsafe` block is allowed. A ratchet
  enforces it: `xtask/comment-baseline.txt` holds the comment lines each file may still have, and
  `cargo test --workspace` fails when a file has more, or fewer without
  `cargo run -p xtask -- comments --update`. `/delete-comment <path>`
  (`.claude/skills/delete-comment/SKILL.md`) sweeps a crate and tightens the ratchet.

## Scope

Implemented:
- **v1 / P0 (§4.2):** daily notes (the launch view is Roam's Daily Notes feed: every calendar
  day from today backwards, editable in place, each with its Linked References and no Unlinked
  References; older days load as you scroll; there is no recent days list, the mini calendar
  marks the days that have notes, DESIGN.md §3.2). A day becomes a page
  on its first write, not on a visit (FR-1.2): `Store::draft_daily` keeps it in memory only
  (never projected, synced or seen by `doctor`) and the first `queue` of it writes the whole
  shard; a link to the date writes the draft, a remote copy of the day replaces it. Pages sit in a centred
  column of readable width (760 px). Outliner, backlinks (linked references, "Scheduled for this
  day"), reference nodes (page/block refs, transclusion, inspector, unresolved intents),
  tags-as-pages, date links, rename/merge, right sidebar, local persistence, undo/redo, quick
  jump, Markdown/JSON export, rolling backups. Relative date links: `[[tomorrow]]`,
  `[[next sunday]]`, `[[2 weeks ago]]` (`utopia_core::date::relative_date`) are offered in `[[`
  autocomplete and rewritten to the day's title when the block commits; the resolver never sees
  them (their meaning changes daily, §10.3.1) and a real page with that title wins. The left
  sidebar collapses (Cmd+\) and its month grid is behind a "Calendar" toggle (local state).
- **v1.1 / P1 (§4.2, §4.1 v1.1 column):** full-text search UI (Cmd+Shift+F), block types
  (to-do, H1–H3, quote, code (syntax-coloured from its first line's language, bat's grammars through syntect, `utopia_ui::syntax`), numbered; Alt+Enter cycles to-dos), slash menu, unlinked references
  + one-click Link, co-reference and date filters, attributes + page metadata header, aliases UI,
  journal and page templates (`templates/<name>` pages), extract block to page, drag across
  pages, block embeds (`{{embed: ((uid))}}`), quick capture (in-app and system-wide hotkey,
  plus `utopia capture`), stacked panes (DESIGN.md §4.3/§4.4, PRD §9.2.1; replaced split panes), Roam JSON import, OPML export.
  Tasks view (PRD §9.2.2, DESIGN.md §4.5, `GoTasks` ⇧⌘X): every to-do grouped by page, Open /
  Done / All, ticks through the page's own command; it covers the stack like the graph view
  and gives it back when a page opens. `cargo run -p xtask -- seed-tasks <graph>` seeds its shots.
- **Sync (§11.7, §12):** HTTP relay and client, background sync with status and merge notices,
  snapshot compaction, caret preservation across merges (§6.5: carried through the text
  diff, `utopia_core::text::carry_caret`; the editor stores no caret anchor), end-to-end
  encryption (Argon2id +
  XChaCha20-Poly1305), realtime WebSocket notices, folder transport, LAN relay with mDNS.
- **Settings and keybindings (§9.1):** a Settings window (Cmd+,) with six tabs —
  **Keybindings**, where every action can be rebound by pressing the keys and the change
  applies at once; **Shortcuts**, the cheat sheet of everything bound; **Appearance**, the font
  (System with SF Mono, or IBM Plex Sans or Geist with its mono, both bundled under the OFL in
  `crates/utopia-ui/assets/fonts`), applied to every window at once and kept in `settings.json`; **Sync**, per-graph
  transport setup, Keychain credentials, connection testing and device pairing; **Assistants**,
  the graph's MCP access level (Off, Read, Read and write), the `when_closed` switch, the setup
  entry to copy into Claude Code, Claude Desktop or Codex, and whether the bridge runs — a
  change saves `<config>/mcp/<graph key>.json` and starts, stops or re-levels the loopback API
  with no restart (PRD §13.8 MC-12); and **Help**, a tour of
  the features with screenshots captured from the app (`cargo run -p xtask -- help-shots`).
  `utopia_ui::actions::CATALOG` is the one source of truth: default keys plus the label,
  category and help text for each action, so the editor, the cheat sheet and the installed
  bindings cannot drift apart. Only actions the user changed are written to `keymap.json`,
  so defaults that move in a later release still reach them. Day navigation is
  `GoDailyNotes` (Cmd+Shift+D, the feed), `GoToday` (Cmd+Shift+T) and `GoYesterday`
  (Cmd+Shift+Y). Rebinding captures through a gpui *keystroke interceptor*, which runs before
  the keymap is consulted, so the keys being recorded cannot also fire their own action —
  without that, rebinding onto Cmd+Q would quit the app. Keys are compared with
  `canonical_keys` (gpui's spelling), so `cmd-alt-t` and `alt-cmd-t` are one binding.
- **Assistant access over MCP (§13.8, MC-1 to MC-14; `docs/mcp-design.md`):** `utopia mcp
  --graph <file>` is a stdio server for Claude Code, Claude Desktop and Codex, in both
  protocol eras (the handshake revisions and 2026-07-28). Each tool call is one
  `utopia_api::Op`: with the app open it runs there through `POST /v1/op`, and with the app
  closed it runs headless only when the graph's access file says `when_closed` (MC-5). It
  adds no write path, and page text goes only into tool results (MC-11). The six read tools
  are built (`search`, `list_pages`, `read_page`, `read_block`, `get_backlinks`, `run_query`);
  an outline line ends with the block id marker `{id:01J8…}`, which is not Utopia markup, so
  an agent can send a line back; a read never makes a page (FR-1.2) and each result is cut at
  24 KB with the call that continues it. Access is per graph and off by default (MC-1);
  `Op::kind()` is the one read/write classification. `utopia mcp config <client>` prints the
  entry that Settings → Assistants also copies, each pinning an absolute `--graph` (MC-13).
  All four write tools are built: `append_blocks` (`Op::AppendOutline`: one outline, one
  transaction and one undo step, the id markers stripped), `update_block` (`Op::UpdateBlock`
  with `kind`, `checked` and `if_rev`: one call, one undo step, and a stale `if_rev` writes
  nothing, MC-10), and `move_block` / `delete_block` (`Op::MoveBlock` within a page or into
  another one, `Op::DeleteBlock` as a tombstone: each one undo step, MC-8, MC-9; a move keeps
  every id, so `((references))` still resolve). Real clients have written through both paths
  (slice 3.4): Claude Code on the modern era through `/v1/op` in the open app, and Codex on
  the 2025-06-18 handshake on the headless path
  (`docs/verification/docs-mcp-write-check/`).
- **v1.2+ / P2 (§4.2, Appendix A):** query blocks (`{{query: …}}`, Roam syntax, SQL-backed, `/Query`)
  with queryable `key:: value` attributes; graph view (Cmd+Shift+G, force-directed, local/global,
  journal toggle); attachments (drop files or `/Attach a file`, image previews, file chips,
  synced and E2E-encrypted, exported); PDF viewer with rectangle highlights stored as quote
  blocks (macOS, PDFKit); WASM plugin runtime; local HTTP automation API + `utopia://` links;
  SQLCipher encryption at rest with an unlock screen; mobile core (`utopia-ffi`) and SwiftUI
  shell (§14.3 scope).

Known limits (pick these up deliberately, don't paper over them):
- `ut` on Linux (ADR 0014): `credentials.json` is read, changed and replaced with no lock, so
  two `sync setup` runs for different graphs at once can lose one credential; there is no Linux
  store for `UTOPIA_DB_PASSPHRASE`; `sync launchd-plist` is macOS-only (docs/sync-modes.md has a
  systemd unit); `ut --help` keeps the app's column width, so descriptions shift 4 columns.
- Keystroke→frame in `bench --ui` measures input handling + full render/layout/paint; it does
  not include GPU present or waiting for the display refresh.
- Settings → Sync: `prepare_sync_destination` exports every shard on the UI thread when the
  destination changes (a one-time cost proportional to the graph, run from Apply); "Test
  connection" on a shared folder creates its `pages/` directory; Nearby Mac hosting and mDNS
  discovery were verified by tests and by eye in the UI, not between two real Macs; the
  shot script's clicks only land on a freshly painted frame, so scripted captures of the
  pane take a throwaway `shot` after each click.
- The first launch after a fresh build takes ~1 s (binary page-in plus runtime Metal shader
  compilation, because the Metal toolchain is not installed); later launches draw in ~150 ms.
- Screenshots come from the app capturing its own window (`UTOPIA_SHOT_SCRIPT`); colours and
  typography are checked by eye, not by tests.
- Settings → Help embeds its screenshots with `include_bytes!` (~1.2 MB), so they go stale when
  the UI changes: rerun `cargo run -p xtask -- help-shots` and look at the PNGs. They are shown
  at a fixed 640 px width with the capture's own height ratio, because gpui only learns an
  image's aspect ratio a frame after layout.
- The README's pictures (`docs/images`, the stills and `demo.gif`) go stale the same way:
  `cargo run -p xtask -- readme-shots` captures them from the help graph with the default
  settings (it needs ffmpeg, and an unlocked screen: a locked Mac captures black frames).
- `open_settings` is deferred (`cx.defer`): gpui refuses to update a window from inside its own
  update, and global action handlers run during one, so Cmd+, pressed while Settings is focused
  would otherwise open a second Settings window instead of raising the first.
- The iPhone app (`docs/ios-prd.md`) has run on the iPhone 18 Pro simulator (iOS 27.2) and not on
  a device. Widgets and the Control Center/Action button control (IO-2) are not built, so a
  capture always starts from the app's own icon. The day log's time column shows equal times for
  seeded blocks: a block's creation time is its ULID's timestamp, so `Store::set_clock` cannot
  fake a day for a screenshot. Android: Kotlin bindings generate; no Compose shell (the PRD
  ships iOS first, §14.2, M4) and no kotlinc here to compile them.
- PDF highlights select text the way a reader does (per-line rectangles + quoted text); Option-drag
  highlights an area instead. New highlights go under the block the PDF was opened from. PDF
  viewing needs macOS (elsewhere PDFs open in the system viewer).
- `utopia://` was verified once with the user's permission: `target/Utopia.app` registered,
  links sent with `open` and AppleScript `open location` against a scratch graph, then the bundle
  was unregistered (`lsregister -u`). A link brings the window to the front, so anything typed
  meanwhile lands in the focused block.
- Plugins run in an interpreter (wasmi): fine for commands, slow for heavy computation.
- Query `{attr: …}` matches the attribute block itself, not the page or parent that owns it.
- LAN direct uses the HTTP relay protocol over TCP, not QUIC as sketched in §12.7.4.
- The folder transport re-reads every file of a changed page (imports are idempotent) and
  never compacts, because it must not delete other devices' files.
- Very large flat sibling lists (≫5k blocks under one parent) make each insert O(n).
- MCP: `tools/list` does not depend on the level (only `--read-only` removes the write
  tools), because legacy clients do not ask for the list again. `append_blocks` writes every
  line as a bullet: the `[ ] `, `## ` and `> ` prefixes `read_page` writes for to-dos,
  headings and quotes stay in the block's text. An
  outline of more than 2,000 blocks, or deeper than 12 levels (FR-2.9), is refused and writes
  nothing. `update_block` refuses an empty `text` (it never wipes a block), takes the
  outline's marker, trailing newline and — from a line that carried a marker — the `- ` and
  kind prefixes off the `text` it is sent (text the agent wrote itself is left as given),
  needs the block to be a to-do or become one to set `checked`, and `heading` keeps the level
  of a block that already is a heading. Its `if_rev` is a hash of the text only, so a change
  of kind or done state under the agent does not make it stale; `move_block` and
  `delete_block` take no `if_rev` at all, so they do not notice an edit between the agent's
  read and its call. `move_block` without `parent` or `after` puts the block first on the
  page, which is where `outline::move_to` puts it, and its answer says where it landed; a
  move into the block's own subtree or past depth 12 (FR-2.9) fails with no write.
  `delete_block` can empty a page: the last block of a page can become a tombstone.
- MCP, app closed: a call does not sync (the write waits in the outbox for the next launch)
  and leaves no undo step (the undo stack is the store's memory, and the store the call
  opened is dropped before it answers, so `delete_block` says the app cannot undo it), and an
  encrypted graph needs a passphrase the process can read
  (`UTOPIA_DB_PASSPHRASE` or the keychain) because `stdin` is the protocol channel and no
  prompt is possible. A cancel keeps back the answer but does not reverse a write the owner
  already started. `serve-api` and `sync --continuous` read the access file once at start, so
  their `/v1/op` level follows the file only on restart; the app re-levels its own server at
  once.
- MCP: Claude Desktop has not been run against `utopia mcp` (its entry needs
  `claude_desktop_config.json` and a restart, so the owner adds it). Codex and Claude Code
  were, and their protocol eras are in `docs/mcp-design.md` §6. Codex's default
  `approval: never` refuses `update_block` and `delete_block`, because both carry
  `destructiveHint: true`: a Codex run needs `--approve-for-me` or an approval policy that can
  ask (§10). `append_blocks` and `move_block` are not destructive and run under it.
- MCP: an agent's `update_block` on the block the owner has the caret in replaces that line
  and keeps the caret: the editor shows the agent's text, the next typed character lands
  after it, and no keystroke is dropped (measured in the real app, slice 3.4; the UI tests
  `mc_10_a_write_to_the_caret_block_keeps_the_caret` and
  `mc_10_a_stale_if_rev_keeps_the_typed_text_while_the_caret_is_in_the_block` pin it). The
  text typed before the call is replaced unless the call carried an `if_rev` from before it,
  which is refused with 409 and writes nothing. An input-method composition is the one case
  where the composed text wins over the agent's write (from the code, not measured).
- Stacked panes (DESIGN.md §4.3): gpui has no `position: sticky`, so `pane_stack::Geometry`
  places each pane; when the window cannot hold every spine plus a whole pane, the oldest
  spines fold under one another at the left edge (the newest pane always shows whole).
  gpui 0.2 cannot rotate text, so a spine sets its title one grapheme per line, upright.
  There is no accessibility tree, so panes are not landmarks and the toggle has no
  `aria-pressed`. gpui hands a click to every hitbox under the pointer: each pane
  `occlude()`s the panes it covers and forwards the sideways wheel itself — keep that, or a
  click lands in a covered pane too. Spines and off-screen panes do not draw their page,
  except the one holding the edit or a focused text field (rename, alias, filter); every
  focus move goes through `Workspace::set_focus` / `apply_plan`, which end the edit in the
  pane focus leaves. A scroller narrower than 560 px (both sidebars open in a small
  window) shrinks the pane to fit (`Geometry::fit`). In Single mode the hidden entries keep their views but
  are inactive (`PageView::set_active`): they rebuild when shown, not on every change.

In progress: assistant access over MCP (PRD §13.8). `docs/mcp-handoff.md` has the state of
each pull request, the procedure, and a brief for each pull request that is not started;
`docs/mcp-design.md` is the design. Read the handoff file before you continue that work.

Beyond the PRD's scope ladder there is nothing scheduled; new feature requests go through the
PRD first (risk R8).

## Working agreement for agents

- Before editing a crate, read its `lib.rs` module list and the PRD section it implements.
- After any change: build, test, clippy, fmt (see above). Never leave the workspace not compiling.
- When the PRD is ambiguous, pick the reading that preserves data and identity, note the choice in an
  ADR (`docs/adr/`) with the PRD section, and mention it in your summary.
- Do not commit, push, or create branches unless the user asks.
