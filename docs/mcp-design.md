# MCP support: design

Status: design, written on 2026-11-10. The owner answered each decision on 2026-11-16
(§1.1). It is built up to slice 2.2: the owner lock, `POST /v1/op` and the access file, the
`utopia-mcp` crate with the two protocol eras and the six read tools, the `utopia mcp`
subcommand and the headless path, Settings → Assistants, and this documentation. Slice 3.1
adds the first write tool, `append_blocks`; `update_block`, `move_block` and `delete_block`
(§8.4) are not built.
Checked on 2026-11-16 against `main` (`f559022`), the MCP 2026-07-28 specification and the
documentation of the three clients. §1.3 lists what the check changed.

Goal: an AI client reads and writes the graph through the Model Context Protocol (MCP).
The first clients are Claude Desktop, Claude Code and Codex.

Risk R8 says that a new feature goes through the PRD first. The PRD section for this
feature is §13.8, added on 2026-11-16 (D8). PRD §2.2 lists "AI features of any kind" as a
non-goal for v1, and a note there now says how this feature relates to that line (§12.1).

## 1. Decisions

### 1.1 Decided

| Date | Decision |
|---|---|
| 2026-11-10 | Access is Off by default. Only a deliberate change in Settings turns it on. |
| 2026-11-10 | The first clients are Claude Desktop, Claude Code and Codex. |
| 2026-11-10 | The server reads and writes. Read-only is a level, not the only mode. |
| 2026-11-16 | D1 A. The server works when the app is closed only after the `when_closed` switch, which is off by default. |
| 2026-11-16 | D2 A. Each graph has one owner, and the owner lock enforces it (§2.2). |
| 2026-11-16 | D3 A. The protocol layer is hand-written JSON-RPC for the two eras. |
| 2026-11-16 | D4 A. The first version has the ten tools in §4. |
| 2026-11-16 | D5 A. Each graph has its own access setting. |
| 2026-11-16 | D6 A. Utopia shows the command or the config entry, and the owner copies it. |
| 2026-11-16 | D7 A. A write from an agent is one step on the undo stack of the app. |
| 2026-11-16 | D8 A. The PRD scope rows and the requirement ids come before phase 1. They are in PRD §13.8. |
| 2026-11-16 | D9 A. The order of the phases is Core, Settings, Write. |
| 2026-11-16 | D10 A. `capture` sends its work to the owner when the owner runs the API. If not, it stops. |

### 1.2 The options

The owner answered "all A" on 2026-11-16. No decision is open. Each item keeps its options
as the record, and §8.6 shows what a different answer would change.

**D1. Does the server work when the app is closed?**

- A. Yes, but only after a second switch, `when_closed`, which is off by default. *Selected.*
- B. No. The app must be open. This removes the headless path (§2.3) and is the smallest design.
- C. Yes, each time access is on. No second switch.

The first answer, "default off if the app is not running", was read as option A. The
owner confirmed option A on 2026-11-16.

**D2. One owner for each graph.**

- A. Add the owner lock (§2.2). A second process on the same graph stops with a message. *Selected.*
- B. No lock. The MCP server only works through the open app, and the hazard in §9 stays.

Option A changes current behaviour: a second app process, `serve-api`, `capture` and
`import-roam` stop or forward the work when the app has the graph open.

**D3. The protocol layer.**

- A. Hand-written JSON-RPC, for the legacy revisions and for 2026-07-28. *Selected.*
- B. The `rmcp` crate. Less protocol code, but more dependencies and an async runtime in the
  stdio process.

**D4. The tools in the first version.**

- A. The ten tools in §4. *Selected.*
- B. The six read tools only. Write tools come after a period of use.
- C. The ten tools and also page rename and page trash.

**D5. The scope of the access setting.**

- A. One setting for each graph. *Selected.*
- B. One setting for all graphs.

**D6. Client setup.**

- A. Utopia shows the command or the config entry, and the owner copies it. *Selected.*
- B. Utopia also writes the entry into the config file of the client.

**D7. Undo.**

- A. A write from an agent is one step on the undo stack of the app. Cmd+Z reverses it. This
  is how the automation API operates today. *Selected.*
- B. Agent writes stay off the undo stack of the app.

**D8. The PRD section.**

- A. Add the feature to the PRD scope (§4.1, §4.2 and Appendix A) and add a group of
  requirement ids for MCP before phase 1 starts. Tests cite those ids. They are in PRD
  §13.8.
  *Selected.*
- B. Start phase 1 and write the PRD section at the same time.

The first text of this decision named a "§16 scope table". The PRD has no such table:
§16 is Metrics.

**D9. The order of the phases.** (Added on 2026-11-16.)

- A. Core, then Settings, then Write. The owner sets the level in Settings as soon as the
  read tools work. *Selected.*
- B. Core, then Write, then Settings. This was the first order. Until the tab exists, the
  owner edits the access file by hand.

The decision of 2026-11-10 says that only a change in Settings turns access on. Option A
agrees with it before the write tools exist.

**D10. `utopia capture` while the app has the graph open.** (Added on 2026-11-16.)

This follows from D2 A. Today `capture` writes from a second process. With the owner lock
it can only send the work to the owner, and the owner runs the loopback API only when
`UTOPIA_API` is set or MCP access is on (§2.4).

- A. `capture` sends `Op::Capture` to the owner when the owner runs the API. If not, it
  stops with "This graph is already open in Utopia". *Selected.*
- B. The app always runs the loopback API, so `capture` always works. Each app start then
  opens a loopback socket.

### 1.3 Check on 2026-11-16

No code has changed. The check made these changes to this file:

| Item | Result | Where |
|---|---|---|
| Code references | Each named function and type exists on `main`. Six line numbers moved by one or two lines and are corrected. | §2, §9 |
| PRD scope | The PRD has no scope table in §16. The scope is in §4.1, §4.2 and Appendix A. | D8, §12.1 |
| PRD non-goal | §2.2 lists "AI features of any kind" as a non-goal for v1. | §12.1 |
| `_meta` key names | Read from the 2026-07-28 specification. | §3.2 |
| Modern results | Each result needs `resultType`. `tools/list` and `server/discover` must have `ttlMs` and `cacheScope`. | §3.2 |
| Tool hints | `destructiveHint` and `openWorldHint` default to `true`, so each tool sets all four hints. | §4.1 |
| Result size | Claude Code gives a warning above 10,000 tokens and has a limit of 25,000 tokens. The read tools get a size budget. | §4.2 |
| `Op::Journal` | It answers 404 for a day without a page. `read_page` must not. | §4 |
| Tool list | It does not depend on the access file. | §5 |
| Privacy | The Settings tab says that note text goes to the AI client. | §5 |
| Claude Code setup | The options go before the server name. The source is now the Anthropic documentation. | §6 |
| Claude Desktop setup | Confirmed from the MCP documentation. | §6 |
| Phases | Each phase is now a list of slices. Settings moves before Write. | D9, §8 |
| CLI `capture` | The owner lock changes it. | D10, §2.2 |
| Requirement ids | MC-1 to MC-14. They are now in PRD §13.8. | §12 |

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
| → the lock is held and nothing answers there | Tries the lock once more for 2 s (a short CLI write may hold it), then says the graph is open but does not answer (§3.1, step 4). |

- The lock is the truth. The discovery file is only an address.
- The discovery file moves from `<config>/api.json` to
  `<config>/api/<sha256-of-canonical-graph-path>.json` (mode 0600), one for each graph. It
  holds `{url, token}` as it does today.
- There is no handoff protocol. The kernel releases the lock when a process stops, so a
  crash leaves no stale lock.
- The app takes the lock before `Store::open_with_key` (`crates/utopia-app/src/main.rs:413`).
  It tries again for 2 seconds, because a headless call holds the lock for a short time.
- If the lock stays held, the app shows "This graph is already open in Utopia" in its window
  and does not open the store.
- The window stays open with that message, as it does for every other failure to open a
  graph; it holds no store and no lock (slice 1.1).
- `capture` and `import-roam` use the same rule: they send the work to the owner, or stop
  with the same message. `capture` goes to the owner as `Op::Capture` (D10). `import-roam`
  has no `Op`, so it stops when the lock is held.
- `encrypt` and `decrypt` already take an exclusive `FileLock`
  (`crates/utopia-store/src/crypt.rs:198`). They do not change.
- `doctor` without `--rebuild` and `export` only read. They do not take the lock.

### 2.3 The headless path (app closed)

For one tool call: take the lock, open the `Store`, run `handle`, flush, close the store,
release the lock.

- It opens the graph the way `cli::open_store` (`crates/utopia-app/src/cli.rs`) does, except
  that it never prompts: the passphrase of an encrypted graph comes from
  `UTOPIA_DB_PASSPHRASE` or the Keychain, and without one the call fails with a clear
  message, because `stdin` is the protocol channel. (Slice 1.4 made this its own opener,
  `open_graph` in `crates/utopia-app/src/mcp.rs`: `open_store` asks for a passphrase when it
  finds none.)
- It does not sync. Writes go to the outbox and the app sends them at its next start.
- Slice 1.4 measured the cost of one call on the bench graph: 1.4 ms, median of 7, in
  release. `xtask bench` fails above 50 ms (§10).

### 2.4 Changes to existing code

- `utopia-api`: add `POST /v1/op`. It accepts the `Op` JSON that plugins already send, so the
  bridge does not map each tool to a REST route.
- `utopia-app`: start the loopback API when MCP access is on, not only when `UTOPIA_API` is
  set (`main.rs:453`). With access Off, start-up stays as it is today.
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
- To see a cancel while a call runs, one thread reads `stdin` and a second thread runs the
  calls. A cancel does not reverse a write that the owner already started. The server only
  keeps back the answer.

### 3.2 Wire rules of 2026-07-28

These were read on 2026-11-16 (§11).

Each modern request carries these keys in `params._meta`:

| Key | Necessary | Value |
|---|---|---|
| `io.modelcontextprotocol/protocolVersion` | Yes | `"2026-07-28"` |
| `io.modelcontextprotocol/clientCapabilities` | Yes | An object. `{}` is permitted. |
| `io.modelcontextprotocol/clientInfo` | No | `{"name", "version"}` |

- A request without a necessary key gets error `-32602`.
- A version that the server does not support gets error `-32022` with
  `"data": {"supported": [...], "requested": "<version>"}`. The list has each version that
  can be asked for in this request shape, which is `2026-07-28`; `server/discover`'s
  `supportedVersions` has every revision the server speaks (all five). A handshake
  revision cannot be asked for with the `_meta` of 2026-07-28, so offering it there would
  only cause a second refusal.
- Each result has `"resultType": "complete"`. The server never answers `input_required`.
- Each result has `_meta["io.modelcontextprotocol/serverInfo"]` with the name and the
  version of the server.
- `server/discover` returns `supportedVersions`, `capabilities` (`{"tools": {}}`),
  `instructions`, `ttlMs` and `cacheScope`.
- `tools/list` must also have `ttlMs` and `cacheScope`. The tool list is the same for each
  caller and does not change while the process runs (§5), so the values are `3600000` and
  `"public"`.
- A cancel is `notifications/cancelled` with `params.requestId`.
- A tool that is not in the catalog gets the protocol error `-32602`.
- A tool that fails returns a result with `"isError": true` and a text that the model can
  read. Access Off, a block that does not exist and an `if_rev` that does not agree are
  such failures.
- A client of the two eras sends `server/discover` first. If it gets an error that is not
  a modern error, or no answer, it uses `initialize`. Thus the server must answer
  `server/discover` immediately.

The legacy rules:

- The server supports 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05.
- `initialize` answers with the version of the client if the server supports it. If not,
  it answers with 2025-11-25.
- Legacy results keep the legacy shape: no `resultType`, `ttlMs` or `cacheScope`.

## 4. Tools

| Tool | Kind | Op | Status |
|---|---|---|---|
| `search` | read | `Search` | exists |
| `list_pages` | read | `ListPages` | exists |
| `read_page` | read | `GetPage`, `Journal` | exists; new output format |
| `read_block` | read | `GetBlock` | exists; add children |
| `get_backlinks` | read | `Backlinks` | exists |
| `run_query` | read | `Query` | exists |
| `append_blocks` | write | new `AppendOutline` | exists (slice 3.1) |
| `update_block` | write | `UpdateBlock` | add `kind`, `checked`, `if_rev` |
| `move_block` | write | new `MoveBlock` | new |
| `delete_block` | write | new `DeleteBlock` | new |

- **`read_page`** accepts a title, an alias, `today`, `yesterday`, or an ISO date. It
  returns an indented Markdown outline. Each line ends with the block id.
- **The id marker** is `{id:01J8Z3K9V2}`, at the end of the line (at the end of a block's
  last line, when its text runs over several). It cannot be Utopia markup: a marker written
  as `((id))` is a block reference, `[[id]]` a page link and `#id` a tag, so an agent that
  sent a line back would create one. A single `{` is literal text to the inline parser, so
  `{id:…}` is inert in a block that an agent writes back. Slice 3.1 (PR 7) strips the
  marker from each line that `append_blocks` receives.
- A day without a page returns an empty outline, not an error (FR-1.2). A read never makes
  a page. `Op::Journal` answers 404 for such a day today
  (`crates/utopia-api/src/lib.rs:291`). The tool changes that answer into an empty
  outline. The HTTP route stays as it is.
- A large page is cut at the size budget (§4.2). The result tells the agent how to read a
  subtree with `read_block`.
- **`append_blocks`** accepts a Markdown outline. It uses `markdown::parse_outline`
  (`crates/utopia-core/src/markdown.rs:12`) and commits one transaction and one undo step.
  With `create`, it makes the page if the title does not resolve; a title that names a date
  is that day's page (FR-1.2). One call adds at most 2,000 blocks and no line may be deeper
  than 12 levels (FR-2.9); a larger or deeper outline is refused with `isError` and nothing
  is written, not even the page.
- **`move_block`** uses `outline::move_to` and `Store::move_block_to_page`.
- **`delete_block`** uses `outline::delete_subtree`. It makes a tombstone (rule 5), and undo
  restores it.
- **`if_rev`** is an optional short hash of the text that the agent read. If the block
  changed after the read, the call fails and the agent reads again (§2.3.7).
- Each tool sets all four hints (§4.1). The Codex `writes` approval mode and the Claude
  permission prompts use these hints.
- `Op::kind()` (`crates/utopia-api/src/lib.rs:134`) must classify each new variant, or the
  code does not compile. Plugins and MCP use the same classification.
- The server `instructions` text is a short syntax guide: `[[Page]]`, `#tag`,
  `((block-id))`, `key:: value`.
- Page text is data. It goes into tool results only, never into a tool description or the
  `instructions` text.

### 4.1 Tool hints

In the 2026-07-28 schema, `destructiveHint` and `openWorldHint` default to `true`. A tool
that sets only one hint thus tells the client that it is destructive and that it touches
the open world. Each tool sets all four.

| Tool | `readOnlyHint` | `destructiveHint` | `idempotentHint` | `openWorldHint` |
|---|---|---|---|---|
| The six read tools | `true` | `false` | `true` | `false` |
| `append_blocks` | `false` | `false` | `false` | `false` |
| `update_block` | `false` | `true` | `true` | `false` |
| `move_block` | `false` | `false` | `true` | `false` |
| `delete_block` | `false` | `true` | `true` | `false` |

`move_block` loses no text, so this table marks it as not destructive. The owner can
change that.

### 4.2 Size budget

- Claude Code gives a warning for a tool result above 10,000 tokens. Its limit is 25,000
  tokens (`MAX_MCP_OUTPUT_TOKENS`). Codex cuts a result at its own budget
  (`tools.<tool>.output_token_limit`).
- Each read tool cuts its result at 24 KB of UTF-8 text. Slice 1.4 (§8.2) measured it with
  the real clients: the biggest result they received was far below it, and 24 KB is about
  6,000 tokens, under the 10,000-token warning, so the value stands.
- The cut is at a block boundary. The result gives the number of blocks that it did not
  return and the call that returns them. For the two outline tools that call is the same
  tool again with `after`, the id of the last block the result shows: the outline is in
  document order, so the id reaches every omitted sibling, however deep the cut falls. The
  other read tools return a set, not a document, so their cut only says how many were left
  and that a narrower request is the way to see less of the graph.
- `search`, `list_pages`, `get_backlinks` and `run_query` also keep their `limit`.

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
- The tool list does not depend on the access file. Legacy clients do not ask for the list
  again, so a list that follows the file goes stale after a change in Settings. Only
  `--read-only` removes the write tools from the list, for the life of that process.
- Text that a tool returns goes to the AI client, and the client can send it to its model
  provider. Utopia makes no network call for this (PRD §15.6). The Settings tab says this
  in plain words, adjacent to the control that turns access on.
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
| Claude Code | `claude mcp add --scope user utopia -- <bin> mcp --graph <file>` |
| Claude Desktop | Add `"utopia": {"command": "<bin>", "args": ["mcp", "--graph", "<file>"]}` below `mcpServers` in `~/Library/Application Support/Claude/claude_desktop_config.json`. Start Claude Desktop again. |
| Codex | `codex mcp add utopia -- <bin> mcp --graph <file>`. This writes `[mcp_servers.utopia]` to `~/.codex/config.toml`. |

- `utopia mcp config <client>` prints the correct entry with the real path of the binary. A
  relative `--graph` is resolved against the directory the command runs in, so the entry
  never depends on the working directory of the client that reads it (MC-13).
- A new Settings tab, **Assistants**, shows the two settings and one copy control for each
  client. It uses the tokens in `DESIGN.md`.
- The Claude Code row agrees with the Anthropic documentation. Options such as `--scope`
  go before the server name. The `user` scope keeps the entry in `~/.claude.json`.
- With its v2 client runtime, Claude Code asks a stdio server if it supports 2026-07-28
  and uses the handshake if not. `MCP_PROTOCOL_NEGOTIATION=legacy` makes it use the
  handshake each time.
- The Claude Desktop row agrees with the MCP documentation. Claude Desktop writes the
  `stderr` of the server to `~/Library/Logs/Claude/mcp-server-utopia.log`.
- The Codex documentation gives a start-up limit of 10 s (`startup_timeout_sec`) and a
  tool limit of 60 s (`tool_timeout_sec`). The `writes` approval mode asks for each tool
  that is not marked read-only.
- The era each client uses, measured on 2026-11-16 by slice 1.4 against `utopia mcp` on a
  scratch graph (the one `stderr` line `serve` writes for the first request; the runs are in
  `docs/verification/feat-mcp-subcommand/real-clients.md`):

  | Client | Era | `stderr` |
  |---|---|---|
  | Claude Code 2.1.319 | 2026-07-28 (`server/discover`) | `era=modern protocol=2026-07-28 client="claude-code" version="2.1.319"` |
  | Claude Code, `MCP_PROTOCOL_NEGOTIATION=legacy` | 2025-11-25 handshake | `era=legacy protocol=2025-11-25 client="claude-code" version="2.1.319"` |
  | Codex 0.169.0 | 2025-06-18 handshake | `era=legacy protocol=2025-06-18 client="codex-mcp-client" version="0.169.0"` |

  Each one read the graph with the app closed (`when_closed` on) and with the app open
  (through `/v1/op`). Claude Desktop needs an entry in its config file and a restart, so the
  owner adds the entry and runs that test; it is the one era still unknown.
- Where the line goes: Claude Code writes a stdio server's `stderr` to
  `~/.claude/debug/<session>.txt` in `--debug` runs, and Codex shows it with
  `RUST_LOG=debug`. Codex does not pass its own environment to a stdio server, so a run
  against a scratch config directory needs the entry's `env` table
  (`mcp_servers.utopia.env.UTOPIA_CONFIG_DIR`); Claude Code passes the environment through.

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
  scratch graph, and do one read and one write from each. Claude Code can do the test for
  the two eras: run it one time as it is and one time with
  `MCP_PROTOCOL_NEGOTIATION=legacy`.
- The Assistants tab: `UTOPIA_SHOT_SCRIPT` screenshots in light and dark, as "Verifying a
  change" in `AGENTS.md` requires.

## 8. Feature plan

This plan uses the selected option for each decision (§1.1). §8.6 shows what a different
answer would change. Each slice is one branch and one pull request, in its own worktree
(`AGENTS.md`). A slice is complete when `make check` passes and its tests cite the ids in
PRD §13.8. Size is S (below one day), M (one to three days) or L (more).

### 8.1 Phase 0: decisions and the PRD

| Slice | Contents | Complete when |
|---|---|---|
| 0.1 Decisions | The owner answers D1 to D10. | Complete on 2026-11-16 (§1.1). |
| 0.2 PRD | The change in §12.1: the scope rows, the requirement ids, the notes in §2.2 and §15.6. This is documentation only, so `make check` is not necessary. | The PRD change is merged. The text was written on 2026-11-16. |

### 8.2 Phase 1: core, read only

Result: the three clients read a graph through `utopia mcp`. Until phase 2, the owner sets
the level in the access file by hand.

| Slice | Size | Contents | Needs | Proof |
|---|---|---|---|---|
| 1.1 Owner lock | M | `OwnerLock` in `utopia-store`. The app, `serve-api`, `capture` and `import-roam` obey §2.2. The "already open" message. | D2, D10 | A test with two processes on one graph. A stopped owner leaves no lock. `make dev` on a scratch graph starts while a different graph is open. |
| 1.2 Op route | S | `POST /v1/op`. One discovery file for each graph. The owner enforces the level with `Op::kind()`. The app starts the loopback API at start-up when the access file says `read` or `write`. | 1.1 | `crates/utopia-api/tests/http.rs`: each `Op` through the route, the token and `Origin` checks, the level. |
| 1.3 Protocol and read tools | L | The `utopia-mcp` crate: the two eras (§3), the six read tools, the access file, the `Direct` backend. The outline format of `read_page`, the size budget, a day without a page. | 1.2 | Tests with in-memory pipes, one script for each era. A table test of each tool against `off`, `read` and `write`. |
| 1.4 Subcommand | M | `utopia mcp --graph <file> [--read-only]`, `utopia mcp config <client>`, the `Remote` backend, the headless path, the store-open cost in `xtask bench`. | 1.3, D1 | One read from each of the three clients on a scratch graph, with the app open and with the app closed. The era that each client uses is recorded in §6. |

Slice 1.2 moves the discovery file from `<config>/api.json`. A script that reads that file
today must read the new one. The same pull request changes `AGENTS.md`.

### 8.3 Phase 2: Settings

| Slice | Size | Contents | Needs | Proof |
|---|---|---|---|---|
| 2.1 Assistants tab | M | A sixth Settings tab: the level (Off, Read, Read and write), the `when_closed` switch, one copy control for each client, the privacy sentence (§5). A change applies with no restart: the app starts or stops the loopback API. | 1.4, D5, D6 | `crates/utopia-ui/tests/settings.rs` and `layout.rs`. `UTOPIA_SHOT_SCRIPT` screenshots in light and dark. |
| 2.2 Documentation | S | `AGENTS.md` (commands, files, scope), the route table in `crates/utopia-api/src/lib.rs`, an entry in the Help tour, a state machine file for the owner lock and the access level. | 2.1 | The Help screenshots are captured again (`xtask help-shots`). |

### 8.4 Phase 3: write tools

Each slice needs 1.3. By D9 they come after 2.1.

| Slice | Size | Contents | Proof |
|---|---|---|---|
| 3.1 `append_blocks` | M | `Op::AppendOutline`: a Markdown outline, one transaction, one undo step, `create`. | `doctor` is clean after the call. One undo removes the full outline. |
| 3.2 `update_block` | S | `UpdateBlock` gets `kind`, `checked` and `if_rev`. Each new field has a default, so the JSON that plugins and HTTP clients send today stays valid. | A block that changed after the read makes the call fail with no write. |
| 3.3 `move_block`, `delete_block` | M | `Op::MoveBlock` and `Op::DeleteBlock`. | A move to a different page keeps the subtree. A delete makes a tombstone, and undo restores it. |
| 3.4 Write check | S | One write from each client. An agent write to the block that has the caret (§10) is examined and the result is recorded. | Screenshots of the page before and after each write. |

Each new `Op` variant is classified in `Op::kind()`, and
`ops_classify_reads_and_writes_consistently` stays green. The plugin permission check uses
the same classification, so a plugin without write permission cannot use the new variants.

### 8.5 Phase 4: later

Streamable HTTP in the app, a `.mcpb` bundle for Claude Desktop, page rename and trash,
attachments, a notice in the app for each agent write, forwarding a `utopia://` link from a
second process to the owner.

### 8.6 What a different decision changes

| Answer | Effect on the plan |
|---|---|
| D1 B | Remove the headless path and the bench item from 1.4, and the `when_closed` switch from 2.1. `utopia mcp` then uses only the `Remote` backend. The encrypted-graph limit in §10 goes away. |
| D1 C | Remove `when_closed` from the access file and from 2.1. |
| D2 B | Remove slice 1.1. D1 must then be B, because a headless write adjacent to an open app is the hazard in §9. D10 does not apply. |
| D3 B | Slice 1.3 uses `rmcp`. First make sure that it supports 2026-07-28 and the two eras in one process. |
| D4 B | Remove phase 3. The level `write` goes away. |
| D4 C | Add slice 3.5: `Op` variants and tools for page rename and page trash. |
| D5 B | One access file for all graphs. The file name has no hash. |
| D6 B | Slice 2.1 also writes three config formats, in files that Utopia does not own. |
| D7 B | Agent writes need a path that stays off the undo stack. `handle` calls `seal_undo` today, so this is new store work in phase 3. |
| D8 B | Slice 0.2 runs at the same time as 1.1. |
| D9 B | Phase 3 comes before phase 2. |
| D10 B | Slice 1.2 starts the loopback API at each app start. |

### 8.7 Risks to control

| Risk | Slice | Control |
|---|---|---|
| The owner lock stops something that works today: a second process on one graph. | 1.1 | The message names the cause. D10 covers `capture`. The lock is for each graph, so the verification procedure still works. |
| A client uses an era that the server answers incorrectly. | 1.3, 1.4 | One in-memory script for each era. Claude Code tests the two eras (§7). |
| A result is too large for the client. | 1.3 | The size budget (§4.2). |
| Text in a page tells the agent to do something (prompt injection). | All | Page text goes only into results. The levels and the hints. Each write is one undo step. A delete is a tombstone. |
| The owner does not know that note text leaves the Mac. | 2.1 | The privacy sentence (§5). |
| The cost to open the store for each headless call is high. | 1.4 | Measure it first. If it is above the budget, `when_closed` stays off by default and the tab gives the cost. |

## 9. Findings in the current code

These are true on `main` today, before this design.

**A second process on one graph can start, and no code stops it.**

- `run_app` opens a window with no instance check (`crates/utopia-app/src/main.rs:764`).
- `Store::open` takes only a shared lock (`crates/utopia-store/src/lib.rs:224`), so two
  processes open one graph with no error.
- The bundle `Info.plist` has no `LSMultipleInstancesProhibited` key
  (`xtask/src/main.rs:548`).
- One process has one main window and one store (`main.rs:394` is the only open in the
  app).
- macOS brings the open app to the front when the bundle starts again from Finder, the
  Dock or `open`. This is why a second instance does not occur in normal use.

It can start in these ways:

- `open -n Utopia.app`.
- The binary started directly from a terminal.
- `cargo run -p utopia-app` or `make dev` on the graph that the installed app has open.
- `utopia utopia://…` from a terminal, which calls `run_app` in a new process
  (`main.rs:882`).
- `serve-api`, `capture` or `import-roam` while the app is open.

**Why it is a hazard.** The store keeps each page shard in memory after the first load
(`load_doc`, `crates/utopia-store/src/lib.rs:849`). No check for writes from a second
process was found. Each process then writes projections from its own copy. SQLite WAL mode
with `busy_timeout` keeps the database file intact, so the risk is to the projections and
to what the app shows, not to the file.

**Why the cap is for each graph, not for the app.** The verification procedure runs the
app on a scratch graph while the real graph is open. A cap on the app breaks that.
`LSMultipleInstancesProhibited` also does not stop a binary that starts directly.

## 10. Limits

- **No sync when the app is closed.** See §2.3.
- **Three of the four write tools do not exist yet.** Slice 3.1 added `append_blocks`; the
  level `Read and write` permits it, and `/v1/op` permits every write op. `tools/list` offers
  the six read tools and `append_blocks`. Slices 3.2 and 3.3 add `update_block` with `kind`,
  `checked` and `if_rev`, and `move_block` and `delete_block`.
- **`append_blocks` writes every line as a bullet.** The outline is read with
  `markdown::parse_outline`, which takes off the list markers but not the `[ ] `, `[x] `,
  `## ` and `> ` prefixes that `read_page` writes for a to-do, a heading and a quote: those
  stay in the block's text. Slice 3.2's `update_block` with `kind` and `checked` is the way
  to set a block's kind through the bridge until then.
- **A refused `append_blocks` writes nothing.** It does not even create the page `create`
  would have made, and it records no undo step.
- **`serve-api` reads the access file once**, when it starts, so a level changed while it
  runs applies to `/v1/op` when it restarts. The app re-levels its own server at once
  (MC-12); the two processes' levels can differ in between.
- **Encrypted graphs on the headless path** need a passphrase that the process can read
  without a prompt.
- **The headless call costs 1.4 ms** (median of 7, release, `xtask bench` on the
  100k-block graph: read the access file, take the owner lock, open the store, one `search`,
  close). `xtask bench` fails above 50 ms. Nothing is kept open between calls, so a write
  from the headless path is durable when the call returns, but it does not sync (above).
- **Not examined:** what occurs when an agent updates the block that has the caret.
  `UpdateBlock` has this behaviour today. Slice 3.4 examines it.
- **Claude Desktop has not been run** against `utopia mcp`. Its entry needs a config file
  and a restart, so the owner adds the entry (§6) and runs the test. Claude Code
  (2026-07-28, and 2025-11-25 with `MCP_PROTOCOL_NEGOTIATION=legacy`) and Codex (2025-06-18
  handshake) were measured against `utopia mcp` in slice 1.4 and are in §6.
- **A cancel does not reverse a write.** See §3.1.
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
- Codex, Model Context Protocol: <https://developers.openai.com/codex/mcp>. On 2026-11-16
  this address goes to <https://learn.chatgpt.com/docs/extend/mcp>.

Read on 2026-11-16:

- MCP 2026-07-28, base protocol and `_meta`:
  <https://modelcontextprotocol.io/specification/2026-07-28/basic/index.md>
- MCP 2026-07-28, `server/discover`:
  <https://modelcontextprotocol.io/specification/2026-07-28/server/discover.md>
- MCP 2026-07-28, tools:
  <https://modelcontextprotocol.io/specification/2026-07-28/server/tools.md>
- MCP 2026-07-28, caching:
  <https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching.md>
- MCP 2026-07-28, schema (`ToolAnnotations`, `DiscoverResult`, `ListToolsResult`):
  <https://modelcontextprotocol.io/specification/2026-07-28/schema.md>
- Claude Code, MCP (Anthropic): <https://code.claude.com/docs/en/mcp>
- Claude Desktop, local servers (MCP documentation):
  <https://modelcontextprotocol.io/docs/develop/connect-local-servers.md>

The first version of this file used two third-party guides for the Claude clients. The two
sources above replace them.

## 12. Requirements

The owner selected D8 A on 2026-11-16. The requirement ids MC-1 to MC-14 are in `PRD.md`
§13.8, and the PRD is the source for them. Tests cite those ids.

### 12.1 What changed in the PRD

- §2.2: the line "AI features of any kind" stays. A note after the list says that Utopia
  contains no model and makes no call to a model, and that MCP access is off by default.
- §4.1: a new row, "Assistant access (MCP)", in the column "Later (P2)".
- §4.2: "assistant access over MCP" is in the P2 list. The PRD has no later tier.
- §13.8: a new subsection, "Assistant access over MCP (MC)", with the ids.
- §15.6: a new line about the text that goes to an assistant (MC-12).
- Appendix A: a new row with priority P2.

Two ids were not in the first design: MC-13 (`utopia mcp` needs `--graph`) and MC-14 (the
time budgets).

`AGENTS.md` and `crates/utopia-api/src/lib.rs:1` cite §16 for the automation API. That row
is in Appendix A. Slice 2.2 corrects the two citations.
