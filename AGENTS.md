# AGENTS.md — working on Utopia

Utopia is a local-first, Roam-style outliner for the desktop, written in Rust with
a GPUI front end, a SQLite store, and Loro CRDT documents. **`PRD.md` is the spec.**
Read the section you are touching before changing behaviour, and cite requirement
ids (`FR-2.5`, `AC-4.6`, `§10.3.1`) in tests and doc comments.

## Visual system

**`DESIGN.md` is the visual spec ("Quiet paper").** Read it before changing anything on
screen. Every colour, type role, spacing and radius is a named token in
`crates/utopia-ui/src/theme.rs`; components use the tokens and never raw colours, sizes or
shadows (§2.4). The dark theme defines the same token names with contrast-checked values and
follows the system appearance. **IBM Plex Sans is the default UI and body font; IBM Plex Mono
is the label/count font** (the Appearance setting still offers System and Plex Sans). Geometry
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

Run the app with `cargo run -p utopia-app` (binary name `utopia`). The graph is chosen in
this order: `--graph <file>`, `UTOPIA_GRAPH`, the last graph opened (recorded in the config
dir), the only `*.utopia` in `~/Documents/Utopia`; otherwise onboarding asks for a name (§9.3).
`UTOPIA_CONFIG_DIR` overrides the config dir (`keymap.json`, `settings.json`, `last_graph`); point
both env vars at a scratch folder when trying the app so you never touch the user's real graph.
`settings.json` holds local UI preferences (never synced, rule 9), a versioned object:
`{"version": 1, "font": "system" | "ibm-plex-sans", "density": "comfortable" | "compact",
"ref_children": 2, "attribute-blocks": "compact" | "blocks", "pinned": [],
"right_sidebar_width": 380.0}` (`utopia_ui::appearance`); a missing file or
an unknown value means the default, logged; it is written atomically and keeps keys it does
not know.

Headless commands (`cargo run -p utopia-app -- <cmd>`): `doctor <file> [--rebuild]` checks
the §10.4 invariants (exit 1 on problems); `export <file> <dir>` writes Markdown (attachments in
`blobs/`), `graph.json` and `graph.opml`; `import-roam <file> <roam.json>` imports a Roam export;
`capture <text…>` appends to today's note; `encrypt` / `decrypt <file>` switch encryption at rest
(`UTOPIA_DB_PASSPHRASE`, `UTOPIA_DB_NEW_PASSPHRASE`, keychain service `utopia-db`, or a
prompt); `serve-api <file> [addr]` runs the automation API without a window; a
`utopia://…` argument opens that link.

**Looking at the UI.** `UTOPIA_SHOT_SCRIPT` drives the real window and captures it to PNG (an
app may capture its own window without Screen Recording permission), e.g.

```bash
UTOPIA_CONFIG_DIR=/tmp/t UTOPIA_SHOT_SCRIPT="type Met [[Ana]]; keys escape; wait 300; shot /tmp/t/a.png; keys cmd-k; type ana; shot /tmp/t/b.png" \
  cargo run -p utopia-app -- --graph /tmp/t/g.utopia
```

Steps (`;` or newline separated): `keys <k…>`, `type <text>` (lines are trimmed; a second space
after `type` keeps a leading one), `wait <ms>`, `scroll <dy>` (a real AppKit wheel event in the
main pane; negative scrolls down), `click <x> <y>` (a real AppKit left click in points
from the top left of the content, so halve a Retina PNG's pixels and take off the 28 pt title
bar), `hover <x> <y>` (move the pointer there without pressing, for hover states), `shot <png>`,
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
app and writes `{url, token}` to `<config>/api.json` (mode 0600). Routes are documented in
`crates/utopia-api/src/lib.rs`. `cargo run -p xtask -- bundle` builds `target/Utopia.app`, whose
Info.plist claims the `utopia://` scheme (registering it with LaunchServices happens when the
bundle is opened, so don't open it on a user's machine without asking).

Plugins (P2): directories under `<config>/plugins/<name>/` with `plugin.json` + a `.wasm`/`.wat`
module; commands appear in the slash menu. See `crates/utopia-plugin/src/lib.rs` for the ABI and
`examples/plugins/word-count` for a complete one.

Mobile (§14): `cargo run -p xtask -- mobile` builds `utopia-ffi`, generates Swift bindings
(UniFFI) into `target/mobile/gen`, runs `mobile/ios/Check` against the real core, builds the
SwiftUI shell for macOS (`target/mobile/UtopiaMobile-macos`, `UTOPIA_MOBILE_GRAPH`,
`UTOPIA_TODAY`, `UTOPIA_SHOT=<png>`), type-checks it for the iOS Simulator SDK, and generates
Kotlin bindings (`target/mobile/kotlin`) for an Android shell. `--ios` also
links the simulator app; it needs `rustup target add aarch64-apple-ios-sim` and an installed
iOS Simulator runtime (neither is present here; both are large downloads, so ask first).

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
  macOS Keychain services `utopia-sync` / `utopia-sync-passphrase`, account = graph id.
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

CI (`.github/workflows/ci.yml`) runs fmt, build, test and clippy on macOS for every PR; it is
the floor, not the evidence. A PR is reviewable when its description carries:

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
| `crates/utopia-sync` | Sync client: `Transport` trait, `SyncEngine` (prepare → `exchange` → apply), `HttpTransport`, `FolderTransport`, `e2e::Encrypted`, `realtime::listen`, in-memory reference relay | `utopia-core`, `utopia-doc`, `utopia-store`, `utopia-proto` |
| `crates/utopia-syncd` | Relay server (axum, §12.8): per-page logs, dedupe, auth, quota, snapshots, WebSocket notices; `lan` = in-process relay + mDNS. **No CRDT dependency** | `utopia-proto`, `rusqlite` |
| `crates/utopia-api` | Local automation API (loopback HTTP, bearer token) and `utopia://` link parsing; `handle(store, Op)` is the one implementation shared with plugins | `utopia-core`, `utopia-store`, `axum` |
| `crates/utopia-plugin` | WASM plugin sandbox (wasmi: fuel, memory cap, permissioned API calls) | `utopia-api`, `utopia-store`, `wasmi` |
| `crates/utopia-ui` | GPUI components: block editor (IME via `EntityInputHandler`), outline view, chips, panels, sidebar, background sync runner, graph view, PDF viewer (PDFKit on macOS), Settings window (keybinding editor, shortcut sheet, Help tour) | `utopia-core`, `utopia-store`, `utopia-sync`, `utopia-plugin`, `gpui` |
| `crates/utopia-app` | Binary: window, keymap, actions, app assembly, CLI subcommands, startup probe, screenshot scripts, API/link inbox, unlock screen | everything above |
| `crates/utopia-ffi` | Mobile core: UniFFI surface over the store and sync (§14) | `utopia-store`, `utopia-sync`, `uniffi` |
| `mobile/ios` | SwiftUI shell (`UtopiaMobile`) and the Swift binding check | the generated bindings |
| `xtask` | Dev tasks: `bench` (budgets), `profile-merge`, `bundle`, `mobile`, `help-shots` | anything |

Dependency direction is one-way: `app → ui → (plugin → api) → store → doc → core`;
`ffi → sync → store`. If you find yourself importing upward, move the type down instead.

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
    proven locally, with a comment.
12. **Blobs are content-addressed and verified (§10.3).** Attachments are written once under
    their SHA-256 and rejected on sync if the bytes don't hash to the name. Blocks reference them
    only through markup (`[name](blob:<sha256>)`, `![…]`, `{{pdf: …}}`, `{{highlight: …}}`).
13. **Every API, plugin and FFI write goes through the same command path** as the editor
    (`utopia_api::handle` / `Store` methods), so undo, references and sync behave identically.
14. **Schema changes are migrations** (`schema.rs`, `PRAGMA user_version`), never edits to an
    existing version: graphs on disk must keep opening (§14.4). Check FK-heavy changes against
    `xtask bench`: SQLite cannot use partial indexes for foreign-key checks. Current: v9
   (v6 `attr`, v7 `attachment`, v8 `tombstone`, v9 attachment `state`).

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
- Keep comments sparse and about *why*; the PRD already explains *what*.

## Scope

Implemented:
- **v1 / P0 (§4.2):** daily notes (the launch view is Roam's Daily Notes feed: every calendar
  day from today backwards, editable in place, each with its Linked References and no Unlinked
  References; older days load as you scroll; the sidebar's recent days list only days with an
  entry or a linked reference). A day becomes a page
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
  (to-do, H1–H3, quote, code, numbered; Alt+Enter cycles to-dos), slash menu, unlinked references
  + one-click Link, co-reference and date filters, attributes + page metadata header, aliases UI,
  journal and page templates (`templates/<name>` pages), extract block to page, drag across
  pages, block embeds (`{{embed: ((uid))}}`), quick capture (in-app and system-wide hotkey,
  plus `utopia capture`), split panes (Cmd+Shift+Click), Roam JSON import, OPML export.
- **Sync (§11.7, §12):** HTTP relay and client, background sync with status and merge notices,
  snapshot compaction, Loro-cursor caret preservation, end-to-end encryption (Argon2id +
  XChaCha20-Poly1305), realtime WebSocket notices, folder transport, LAN relay with mDNS.
- **Settings and keybindings (§9.1):** a Settings window (Cmd+,) with five tabs —
  **Keybindings**, where every action can be rebound by pressing the keys and the change
  applies at once; **Shortcuts**, the cheat sheet of everything bound; **Appearance**, the font
  (System, or IBM Plex Sans bundled under the OFL in `crates/utopia-ui/assets/fonts`; code stays
  Menlo), applied to every window at once and kept in `settings.json`; **Sync**, per-graph
  transport setup, Keychain credentials, connection testing and device pairing; and **Help**, a tour of
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
- **v1.2+ / P2 (§4.2, §16):** query blocks (`{{query: …}}`, Roam syntax, SQL-backed, `/Query`)
  with queryable `key:: value` attributes; graph view (Cmd+Shift+G, force-directed, local/global,
  journal toggle); attachments (drop files or `/Attach a file`, image previews, file chips,
  synced and E2E-encrypted, exported); PDF viewer with rectangle highlights stored as quote
  blocks (macOS, PDFKit); WASM plugin runtime; local HTTP automation API + `utopia://` links;
  SQLCipher encryption at rest with an unlock screen; mobile core (`utopia-ffi`) and SwiftUI
  shell (§14.3 scope).

Known limits (pick these up deliberately, don't paper over them):
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
- Settings → Help embeds its screenshots with `include_bytes!` (~1.3 MB), so they go stale when
  the UI changes: rerun `cargo run -p xtask -- help-shots` and look at the PNGs. They are shown
  at a fixed size because gpui only learns an image's aspect ratio a frame after layout.
- `open_settings` is deferred (`cx.defer`): gpui refuses to update a window from inside its own
  update, and global action handlers run during one, so Cmd+, pressed while Settings is focused
  would otherwise open a second Settings window instead of raising the first.
- Mobile has not run on an iOS device or simulator here. `aarch64-apple-ios-sim` is installed and
  the core builds for it; the iOS Simulator runtime is not (download stopped: the user chose to
  focus on desktop first). The Swift bindings run against the real core on macOS and the SwiftUI shell
  type-checks against the iOS SDK. Android: Kotlin bindings generate; no Compose shell (the PRD
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

Beyond the PRD's scope ladder there is nothing scheduled; new feature requests go through the
PRD first (risk R8).

## Working agreement for agents

- Before editing a crate, read its `lib.rs` module list and the PRD section it implements.
- After any change: build, test, clippy, fmt (see above). Never leave the workspace not compiling.
- When the PRD is ambiguous, pick the reading that preserves data and identity, note the choice in a
  doc comment with the PRD section, and mention it in your summary.
- Do not commit, push, or create branches unless the user asks.
