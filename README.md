# Utopia

A local-first, Roam-style outliner for the desktop, written in Rust with a
[GPUI](https://www.gpui.rs) front end. Every bullet is a block you can link to, pages are
created the moment you type `[[Something]]`, and backlinks are computed for you. There are no
folders and nothing to file. Each graph is a single SQLite file on your disk, with
[Loro](https://loro.dev) CRDT documents underneath so that edits from several devices merge
without losing text. Sync is optional, and the relay is one you host yourself.

![Typing a [[link]] into today's note, then jumping to the linked page and finding the new block under Linked References](docs/images/demo.gif)

*Typing a `[[link]]` into today's note, then pressing Cmd+K to jump to that page: the new block
is already in its Linked References.*

## A short tour

**Daily Notes.** The app opens on a feed of every day from today backwards. Each day is
editable in place and shows its own Linked References. Relative dates such as `[[yesterday]]` or
`[[next sunday]]` become a link to that day when the block is committed.

![Daily Notes feed: today's note links to yesterday, which lists it under Linked References](docs/images/daily-notes.png)

**Outline, links and backlinks.** Blocks nest without limit. `[[Page]]`, `#tag` and
`#[[Multi Word]]` all create the same link. `key:: value` attributes appear in a header at
the top of the page. Under every page are its Linked References (with filters) and its
Unlinked References, where one click turns a mention into a link.

![The Atlas Launch page: attributes header, nested outline, to-dos, quote, and Linked References](docs/images/outline.png)

![The Ana Lovelace page, with every block that mentions her grouped by page](docs/images/backlinks.png)

**Quick jump (Cmd+K).** Open a page by name, or create it by typing a name that doesn't
exist yet.

![Quick jump with "atlas" typed](docs/images/quick-jump.png)

**Full-text search (Cmd+Shift+F).** Searches every block in the graph (SQLite FTS5).

![Search for "migration" finding blocks on a daily note and on a project page](docs/images/search.png)

**Graph view (Cmd+Shift+G).** A force-directed map of pages and links. It can show all
pages or only those around the current page, with journal pages toggled on or off.

![Graph view of the demo graph](docs/images/graph.png)

**Settings → Keybindings (Cmd+,).** You can rebind any action by pressing the new keys, and
the change takes effect immediately. Settings also has a Shortcuts cheat sheet and a Help
tour.

![Settings window on the Keybindings tab](docs/images/settings-keybindings.png)

Also implemented: to-dos, headings, quotes, code and numbered blocks; a slash menu; block
references, transclusion and embeds; the right sidebar (Shift-click a link) and split panes;
rename that merges duplicate pages and rewrites their links; journal and page templates;
quick capture (in-app and system-wide); query blocks; attachments and a PDF viewer with
highlights; undo/redo; Markdown, JSON and OPML export; Roam JSON import; optional encryption
at rest; sync through a self-hostable relay, a shared folder or the LAN, with end-to-end
encryption; a local HTTP automation API with `utopia://` links; assistant access over MCP, a
stdio server for Claude Code, Claude Desktop and Codex that is off until you turn it on in
Settings → Assistants; and WASM plugins.
`AGENTS.md` → *Scope* has the full list.

## Build and run from source

You need:

- macOS. It is the only platform built and tested; CI runs on macOS 15.
- [rustup](https://rustup.rs) with a recent stable Rust (the workspace requires 1.85 or later).
- The Xcode Command Line Tools (`xcode-select --install`). The Metal toolchain is **not**
  needed, because GPUI compiles its shaders at runtime. Full Xcode is only needed for the
  iOS checks in `cargo run -p xtask -- mobile`.

```bash
git clone https://github.com/adityadaniel/utopia.git
cd utopia
source ~/.cargo/env
export DEVELOPER_DIR="$(xcode-select -p)"

make dev       # debug build on a throwaway graph in target/scratch (first build takes minutes)
make run       # debug build on your usual graph
make install   # release build of Utopia.app, ad-hoc signed, copied into /Applications
make check     # fmt, build, test, clippy: what CI runs
```

Run `make` with no target to list every task. A graph is one `.utopia` file. Utopia opens the
graph given by `--graph <file>` or `UTOPIA_GRAPH`. Without either, it reopens the last graph
you used, or the only one in `~/Documents/Utopia`, and otherwise asks you to name a new one.
Opening the installed app registers the `utopia://` URL scheme.

## Headless commands

The same binary runs without a window, which is how scripts and agents work on a graph:

```bash
utopia doctor Notes.utopia               # check the §10.4 invariants
utopia export Notes.utopia out/          # Markdown, graph.json and graph.opml
utopia import-roam Notes.utopia roam.json
utopia capture "a line for today's note"  # goes to the open app when it owns the graph
utopia mcp --graph Notes.utopia          # MCP server on stdio for Claude Code, Claude Desktop, Codex

utopia sync setup --graph Notes.utopia --mode relay \
  --url https://relay.example --graph-id notes --token-stdin --passphrase-stdin
utopia sync --graph Notes.utopia         # one sync to completion
utopia sync status --graph Notes.utopia  # mode, target, graph id, secrets present, queue
utopia sync config --graph Notes.utopia --off
```

`utopia sync setup` writes the same per-graph settings file Settings → Sync writes and stores
the token and E2E passphrase only in the Keychain; they are read from stdin (no echo on a
terminal, or `--token-stdin` / `--passphrase-stdin` for scripts), never from arguments.
`utopia sync` exits 0 synced, 1 failed, 2 when sync is Off for the graph, and 3 when another
process owns the graph (the open app syncs it). [`docs/sync-modes.md`](docs/sync-modes.md)
has the agent/`launchd` workflow, and `utopia sync --help` has every option.

### `ut`: the commands without the app

`ut` is the same set of commands in a binary with no window toolkit (no GPUI), so you can copy
it onto a server, Linux included, and sync or serve a graph with no desktop app:

```bash
make install-ut                          # builds target/release/ut, copies it to ~/.local/bin
make install-ut PREFIX=/usr/local        # or /usr/local/bin (may need sudo)

ut sync setup --graph ~/notes.utopia --mode relay --url https://relay.example --graph-id notes
ut sync --graph ~/notes.utopia
ut sync --continuous --graph ~/notes.utopia   # keep syncing; serves the API for `ut mcp`
```

Every `utopia <command>` above works as `ut <command>`; only opening the window and
`utopia://` links need the app. On macOS `ut` keeps the token and passphrase in the Keychain,
as the app does. Linux has no Keychain, so there they go in `credentials.json` in the config
directory (`$UTOPIA_CONFIG_DIR`, else `$XDG_CONFIG_HOME/utopia`, else `~/.config/utopia`), a
file only you can read (mode 0600); `ut` refuses the file if others can read it. `UTOPIA_SYNC_TOKEN` and `UTOPIA_SYNC_PASSPHRASE` still override
both for one run.

## Keyboard essentials

These are the defaults. Change any of them in Settings → Keybindings; Cmd+/ shows the full
list.

| Keys | Action |
|---|---|
| Cmd+Shift+D | Daily Notes feed |
| Cmd+Shift+T / Cmd+Shift+Y | Today / Yesterday |
| Cmd+[ / Cmd+] | Previous / next day |
| Cmd+K | Quick jump to a page (or create it) |
| Cmd+Shift+F | Full-text search |
| Enter / Shift+Enter | New block / line break inside the block |
| Tab / Shift+Tab | Indent / outdent |
| Cmd+Shift+Up / Down | Move the block, with its children |
| Cmd+. | Collapse or expand |
| Cmd+Alt+Down / Up | Zoom into a block / zoom out |
| Alt+Enter | Cycle to-do state |
| Cmd+Shift+C | Copy a `((block reference))` |
| Cmd+Z / Cmd+Shift+Z | Undo / redo |
| Cmd+\\ / Cmd+Shift+\\ | Toggle the left / right sidebar |
| Cmd+Shift+B | Toggle references under the page |
| Cmd+Shift+G | Graph view |
| Cmd+Shift+Space | Quick capture into today's note |
| Cmd+, | Settings |

## Project layout

The workspace is split into crates under `crates/`: `utopia-core` (domain), `utopia-doc`
(CRDT), `utopia-store` (SQLite), `utopia-sync` / `utopia-syncd` (sync client and relay),
`utopia-ui` (GPUI), `utopia-app` (the binary), plus `utopia-mcp`, `utopia-api`,
`utopia-plugin` and `utopia-ffi`. [`AGENTS.md`](AGENTS.md) explains the layout, the
architecture rules, headless commands such as `doctor`, `export` and `import-roam`, and how
to drive the app for screenshots. [`PRD.md`](PRD.md) is the spec.

## Status and limits

- Version 0.1.0. Desktop on macOS comes first. The PRD also targets Windows and Linux, but
  nothing is built or tested for them yet.
- Mobile: the Rust core builds for iOS, and the SwiftUI shell type-checks against the iOS SDK,
  but it has not been run on a device or a simulator. Android has generated Kotlin bindings
  and no app.
- `make app` / `make install` produce an ad-hoc signed bundle. It is not notarized.
- The PDF viewer needs macOS (PDFKit). Plugins run in an interpreter, so they are fine for
  commands but slow for heavy computation.
- The rest of the known limits are listed in `AGENTS.md` → *Known limits*.

## License

Apache-2.0, as declared in `Cargo.toml`.
