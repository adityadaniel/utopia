# MCP support: design

Status: proposal, written on 2026-11-10. No code has changed.

Goal: an AI client reads and writes the graph through the Model Context Protocol (MCP).
The first clients are Claude Desktop, Claude Code and Codex.

The PRD has no section for this feature. Risk R8 says that a new feature goes through the
PRD first, so decision D8 below comes before the code.

## 1. Decisions

### 1.1 Decided

| Date | Decision |
|---|---|
| 2026-11-10 | Access is Off by default. Only a deliberate change in Settings turns it on. |
| 2026-11-10 | The first clients are Claude Desktop, Claude Code and Codex. |
| 2026-11-10 | The server reads and writes. Read-only is a level, not the only mode. |

### 1.2 Open: these need the owner

Each item gives a recommendation. Answer with the id and the option, for example "D1 A".

**D1. Does the server work when the app is closed?**

- A. Yes, but only after a second switch, `when_closed`, which is off by default. *Recommended.*
- B. No. The app must be open. This removes the headless path (§2.3) and is the smallest design.
- C. Yes, each time access is on. No second switch.

The answer "default off if the app is not running" was read as option A. Option B is the
safe choice if that reading is wrong.

**D2. One owner for each graph.**

- A. Add the owner lock (§2.2). A second process on the same graph stops with a message. *Recommended.*
- B. No lock. The MCP server only works through the open app, and the hazard in §9 stays.

Option A changes current behaviour: a second app process, `serve-api`, `capture` and
`import-roam` stop or forward the work when the app has the graph open.

**D3. The protocol layer.**

- A. Hand-written JSON-RPC, for the legacy revisions and for 2026-07-28. *Recommended.*
- B. The `rmcp` crate. Less protocol code, but more dependencies and an async runtime in the
  stdio process.

**D4. The tools in the first version.**

- A. The ten tools in §4. *Recommended.*
- B. The six read tools only. Write tools come after a period of use.
- C. The ten tools and also page rename and page trash.

**D5. The scope of the access setting.**

- A. One setting for each graph. *Recommended.*
- B. One setting for all graphs.

**D6. Client setup.**

- A. Utopia shows the command or the config entry, and the owner copies it. *Recommended.*
- B. Utopia also writes the entry into the config file of the client.

**D7. Undo.**

- A. A write from an agent is one step on the undo stack of the app. Cmd+Z reverses it. This
  is how the automation API operates today. *Recommended.*
- B. Agent writes stay off the undo stack of the app.

**D8. The PRD section.**

- A. Add a row to the §16 scope table and a group of requirement ids for MCP before
  phase 1 starts. Tests cite those ids. *Recommended.*
- B. Start phase 1 and write the PRD section at the same time.

## 2. Architecture

### 2.1 Summary

- One new subcommand: `utopia mcp`. It is an MCP server on stdio.
- All three clients start a stdio server from a `command` and `args` entry, so one binary
  serves them all.
- The server adds no new write path. Each tool call becomes a `utopia_api::Op` and runs
  through `utopia_api::handle` (`crates/utopia-api/src/lib.rs:272`), as rule 13 requires.
- Only one process writes to a graph at a time. A new owner lock enforces this.

```
Claude Desktop ─┐                       ┌─ app open ───▶ loopback API ─▶ ApiCall inbox ─▶ handle(store, Op)
Claude Code ────┼─ stdio ─▶ utopia mcp ─┤
Codex ──────────┘  (one process each)   └─ app closed ─▶ open Store for one call ───────▶ handle(store, Op)
```

### 2.2 The owner rule

`OwnerLock` is new, in `utopia-store`. It is an exclusive `flock` on a `<graph>.owner`
sidecar file. The sidecar follows `FileLock` in `crates/utopia-store/src/crypt.rs`, which
explains why a lock on the database file itself is not possible on macOS.

| Process | Lock use |
|---|---|
| App, `serve-api` | Holds the lock for its full life. Runs the loopback API. |
| `utopia mcp`, for each tool call | Tries the lock and does not wait. |
| → the lock is free | The headless path (§2.3), if `when_closed` permits it. |
| → the lock is held | Sends the `Op` to the owner through the loopback API. |

- The lock is the truth. The discovery file is only an address.
- The discovery file moves from `<config>/api.json` to
  `<config>/api/<sha256-of-canonical-graph-path>.json` (mode 0600), one for each graph. It
  holds `{url, token}` as it does today.
- There is no handoff protocol. The kernel releases the lock when a process stops, so a
  crash leaves no stale lock.
- The app takes the lock before `Store::open_with_key` (`crates/utopia-app/src/main.rs:393`).
  It tries again for 2 seconds, because a headless call holds the lock for a short time.
- If the lock stays held, the app shows "This graph is already open in Utopia" and quits.
  It does not open the store.
- `capture` and `import-roam` use the same rule: they send the work to the owner, or stop
  with the same message.
- `doctor` without `--rebuild` and `export` only read. They do not take the lock.

### 2.3 The headless path (app closed)

For one tool call: take the lock, open the `Store`, run `handle`, flush, close the store,
release the lock.

- It uses `cli::open_store` (`crates/utopia-app/src/cli.rs:54`), so an encrypted graph
  gets its passphrase from `UTOPIA_DB_PASSPHRASE` or the Keychain. If there is no
  passphrase, the call fails with a clear message. It never shows a prompt, because
  `stdin` is the protocol channel.
- It does not sync. Writes go to the outbox and the app sends them at its next start.
- The cost to open the store for each call is not measured. Phase 1 adds it to `xtask bench`.

### 2.4 Changes to existing code

- `utopia-api`: add `POST /v1/op`. It accepts the `Op` JSON that plugins already send, so the
  bridge does not map each tool to a REST route.
- `utopia-app`: start the loopback API when MCP access is on, not only when `UTOPIA_API` is
  set (`main.rs:452`). With access Off, start-up stays as it is today.
- The `ApiCall` inbox (`docs/state-machines/api-call-inbox.md`) stays as it is. Its wait of
  30 s is below the Codex default tool timeout of 60 s.

## 3. New crate: `crates/utopia-mcp`

Dependency direction: `app → mcp → api → store`.

| File | Contents |
|---|---|
| `protocol.rs` | JSON-RPC, one message for each line. `serve(reader, writer, backend)` is generic, so tests use in-memory pipes. |
| `tools.rs` | The tool catalog: name, description, JSON schema, annotations, and the mapping to an `Op`. |
| `backend.rs` | `Direct` (the store, for one call) and `Remote` (the loopback API, with `ureq`). |

### 3.1 Protocol

The newest MCP revision is 2026-07-28. It removes the `initialize` handshake. Clients of
the two eras exist, so the server supports the two.

| Era | What the server does |
|---|---|
| Legacy (2025-11-25 and earlier) | Answers `initialize`, `ping`, `tools/list`, `tools/call`. It accepts `notifications/initialized`. |
| Modern (2026-07-28) | Answers `server/discover`. Reads the protocol version from `_meta.io.modelcontextprotocol/*` on each request. Returns error `-32022` with its supported list for a version it does not know. Adds `ttlMs` and `cacheScope` to `tools/list`. |

- An `initialize` request selects the legacy rules for the life of the process. A request
  that carries the modern `_meta` fields gets the modern rules.
- The server never sends a request to the client.
- `stdout` carries protocol messages only. Logs go to `stderr`. No code on this path can
  call `println!`.
- The server exits when `stdin` closes.
- The server does not send an answer for a request that the client cancelled with
  `notifications/cancelled`.
- The server opens no store at start-up, so it answers inside the Codex start-up limit of
  10 s.
- The exact `_meta` key names were not read for this design. Read the 2026-07-28 schema
  before the parser is written.

## 4. Tools

| Tool | Kind | Op | Status |
|---|---|---|---|
| `search` | read | `Search` | exists |
| `list_pages` | read | `ListPages` | exists |
| `read_page` | read | `GetPage`, `Journal` | exists; new output format |
| `read_block` | read | `GetBlock` | exists; add children |
| `get_backlinks` | read | `Backlinks` | exists |
| `run_query` | read | `Query` | exists |
| `append_blocks` | write | new `AppendOutline` | new |
| `update_block` | write | `UpdateBlock` | add `kind`, `checked`, `if_rev` |
| `move_block` | write | new `MoveBlock` | new |
| `delete_block` | write | new `DeleteBlock` | new |

- **`read_page`** accepts a title, an alias, `today`, `yesterday`, or an ISO date. It
  returns an indented Markdown outline. Each line ends with the block id.
- A day without a page returns an empty outline, not an error (FR-1.2). A read never makes
  a page.
- A large page is cut at a block limit. The result tells the agent how to read a subtree
  with `read_block`.
- **`append_blocks`** accepts a Markdown outline. It uses `markdown::parse_outline`
  (`crates/utopia-core/src/markdown.rs:12`) and commits one transaction and one undo step.
  With `create`, it makes the page if the title does not resolve.
- **`move_block`** uses `outline::move_to` and `Store::move_block_to_page`.
- **`delete_block`** uses `outline::delete_subtree`. It makes a tombstone (rule 5), and undo
  restores it.
- **`if_rev`** is an optional short hash of the text that the agent read. If the block
  changed after the read, the call fails and the agent reads again (§2.3.7).
- Each tool sets `readOnlyHint` or `destructiveHint`. The Codex `writes` approval mode and
  the Claude permission prompts use these hints.
- `Op::kind()` (`crates/utopia-api/src/lib.rs:134`) must classify each new variant, or the
  code does not compile. Plugins and MCP use the same classification.
- The server `instructions` text is a short syntax guide: `[[Page]]`, `#tag`,
  `((block-id))`, `key:: value`.
- Page text is data. It goes into tool results only, never into a tool description or the
  `instructions` text.

## 5. Access control

One file for each graph: `<config>/mcp/<sha256-of-canonical-graph-path>.json`.

```json
{"version": 1, "access": "off", "when_closed": false}
```

| Setting | Values | Default | Effect |
|---|---|---|---|
| `access` | `off`, `read`, `write` | `off` | No tool works until the owner sets it in Settings. |
| `when_closed` | `true`, `false` | `false` | When the app is closed, each tool call fails with "Open Utopia". |

- The file follows the sync settings pattern: an atomic write, never in the CRDT (rule 9).
- A file that is missing, malformed or of an unknown version means Off for the two
  settings.
- No environment variable and no client flag can turn access on.
- `utopia mcp --read-only` lowers the level for one client. It cannot raise it.
- The process that runs the `Op` enforces the level with `Op::kind()`. The app does this
  for the bridge, and `utopia mcp` does it on the headless path.
- When access is Off, each tool call returns an error that names the Settings location.
  `tools/list` still answers, so the client shows the server as connected.
- This setting controls the intent of the owner. It is not a barrier against other local
  processes, which can already read the graph file.
- The bridge authenticates with the token from the discovery file, as API clients do today.
- The error text of the inbox (429, 503, 504) goes to the agent unchanged. It already says
  if the store changed.

## 6. Client setup

`<bin>` is `/Applications/Utopia.app/Contents/MacOS/utopia`. Each entry pins `--graph`, so
an agent never writes to a graph that the owner did not select.

| Client | Setup |
|---|---|
| Claude Code | `claude mcp add utopia --scope user -- <bin> mcp --graph <file>` |
| Claude Desktop | Add `"utopia": {"command": "<bin>", "args": ["mcp", "--graph", "<file>"]}` below `mcpServers` in `~/Library/Application Support/Claude/claude_desktop_config.json`. Start Claude Desktop again. |
| Codex | `codex mcp add utopia -- <bin> mcp --graph <file>`. This writes `[mcp_servers.utopia]` to `~/.codex/config.toml`. |

- `utopia mcp config <client>` prints the correct entry with the real path of the binary.
- A new Settings tab, **Assistants**, shows the two settings and one copy control for each
  client. It uses the tokens in `DESIGN.md`.
- The Claude Code and Claude Desktop details in this table come from third-party guides,
  not from Anthropic documentation. Do a test with each client in phase 1.

## 7. Tests and verification

- `crates/utopia-mcp/tests/`: drive `serve` with in-memory pipes. Do one script for each
  era, and name the tests after the PRD ids from D8.
- After each write tool, assert `Store::doctor` is clean. This keeps "incremental
  projection == full rebuild" (rule 2).
- Access: a table test of each tool against `off`, `read` and `write`, and against
  `when_closed`.
- Owner lock: two processes on one graph. The second one forwards or stops, and a stopped
  owner leaves no stale lock.
- The real clients: add the server to Claude Code, Claude Desktop and Codex against a
  scratch graph, and do one read and one write from each.
- The Assistants tab: `UTOPIA_SHOT_SCRIPT` screenshots in light and dark, as "Verifying a
  change" in `AGENTS.md` requires.

## 8. Phases

1. **Core.** `OwnerLock`, `POST /v1/op`, the per-graph discovery file, the `utopia-mcp`
   crate, the access file, the six read tools, `utopia mcp`, `utopia mcp config`.
2. **Write.** The four write tools and the new `Op` variants.
3. **Settings.** The Assistants tab.
4. **Later.** Streamable HTTP in the app, a `.mcpb` bundle for Claude Desktop, page rename
   and trash, attachments, a notice in the app for each agent write, forwarding a
   `utopia://` link from a second process to the owner.

## 9. Findings in the current code

These are true on `main` today, before this design.

**A second process on one graph can start, and no code stops it.**

- `run_app` opens a window with no instance check (`crates/utopia-app/src/main.rs:763`).
- `Store::open` takes only a shared lock (`crates/utopia-store/src/lib.rs:222`), so two
  processes open one graph with no error.
- The bundle `Info.plist` has no `LSMultipleInstancesProhibited` key
  (`xtask/src/main.rs:548`).
- One process has one main window and one store (`main.rs:393` is the only open in the
  app).
- macOS brings the open app to the front when the bundle starts again from Finder, the
  Dock or `open`. This is why a second instance does not occur in normal use.

It can start in these ways:

- `open -n Utopia.app`.
- The binary started directly from a terminal.
- `cargo run -p utopia-app` or `make dev` on the graph that the installed app has open.
- `utopia utopia://…` from a terminal, which calls `run_app` in a new process
  (`main.rs:881`).
- `serve-api`, `capture` or `import-roam` while the app is open.

**Why it is a hazard.** The store keeps each page shard in memory after the first load
(`load_doc`, `crates/utopia-store/src/lib.rs:847`). No check for writes from a second
process was found. Each process then writes projections from its own copy. SQLite WAL mode
with `busy_timeout` keeps the database file intact, so the risk is to the projections and
to what the app shows, not to the file.

**Why the cap is for each graph, not for the app.** The verification procedure runs the
app on a scratch graph while the real graph is open. A cap on the app breaks that.
`LSMultipleInstancesProhibited` also does not stop a binary that starts directly.

## 10. Limits

- **No sync when the app is closed.** See §2.3.
- **Encrypted graphs on the headless path** need a passphrase that the process can read
  without a prompt.
- **Not measured:** the cost to open the store for each headless call.
- **Not examined:** what occurs when an agent updates the block that has the caret.
  `UpdateBlock` has this behaviour today.
- **Not read:** the exact `_meta` key names of the 2026-07-28 revision, and the Anthropic
  documentation for the two Claude clients.
- **Stdio only.** A client that cannot start a local process cannot connect in the first
  version.

## 11. Sources

- MCP 2026-07-28, versioning and compatibility:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning.md>
- MCP 2026-07-28, stdio transport:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio.md>
- MCP 2026-07-28, transports:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/transports>
- The 2026-07-28 specification, announcement:
  <https://blog.modelcontextprotocol.io/posts/2026-07-28/>
- Codex, Model Context Protocol: <https://developers.openai.com/codex/mcp>
- Claude Desktop config guide (third party):
  <https://www.getmesa.com/blog/configure-mcp-servers-claude-desktop>
- Claude Code MCP guide (third party):
  <https://www.builder.io/blog/claude-code-mcp-servers>
