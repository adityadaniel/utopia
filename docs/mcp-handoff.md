# MCP support: handoff

Written on 2026-11-16. This file lets a different AI agent, on a different Mac, continue
the MCP work. It has the state of the work, the procedure, the contracts between the pull
requests, and one brief for each pull request that is not started.

## 1. How to use this file

Read in this order:

1. `AGENTS.md`: the rules of the repository, the commands, "Verifying a change".
2. `docs/mcp-design.md`: the design. Each decision is final (§1.1).
3. `PRD.md` §13.8: the requirement ids MC-1 to MC-14. Tests and doc comments cite them.
4. This file: §2 (state), §4 (procedure), §5 (contracts), then the brief of your pull
   request in §6.

When a pull request opens or merges, change the table in §2. When the code shows that a
statement here or in the design is incorrect, correct the statement in the same pull
request and say so in its description.

## 2. State on 2026-11-17

The work was a chain of ten pull requests, each with the branch before it as its base. All
ten are merged into `main`, one merge commit each, bottom-up, with `main`'s CI green after
each merge. Before #68, `main` was merged into the rest of the chain; the only conflict was
the status paragraph of `docs/mcp-design.md`.

| PR | Slice | Branch | State |
|---|---|---|---|
| #60 | Phase 0: decisions, plan, PRD §13.8 | `docs/mcp-plan` | Merged (`be84400`) |
| #61 | 1.1 Owner lock | `feat/mcp-owner-lock` | Merged (`768a6f8`) |
| #62 | 1.2 `POST /v1/op`, access file, discovery file for each graph | `feat/mcp-op-route` | Merged (`85a998d`) |
| #65 | 1.3 `utopia-mcp` crate: protocol and six read tools | `feat/mcp-read-tools` | Merged (`73cd5bf`) |
| #66 | 1.4 `utopia mcp` subcommand, `Remote` backend, headless path | `feat/mcp-subcommand` | Merged (`b7daca4`) |
| #67 | 2.1 Settings → Assistants tab | `feat/mcp-assistants-tab` | Merged (`680a180`) |
| #68 | 2.2 Documentation | `docs/mcp-docs` | Merged (`00a70df`) |
| #69 | 3.1 `append_blocks` | `feat/mcp-append-blocks` | Merged (`4ffa2ea`) |
| #70 | 3.2 `update_block` | `feat/mcp-update-block` | Merged (`fd070fb`) |
| #71 | 3.3 `move_block`, `delete_block` | `feat/mcp-move-delete` | Merged (`cc485de`) |
| #72 | 3.4 Write check with the real clients | `docs/mcp-write-check` | Merged (`115c42c`) |

`make check` on `main` at `115c42c`: 965 passed, 0 failed. The branches are deleted.

Still open: the Claude Desktop run of the write check (§6, PR 10) needs the owner, because
it takes an entry in the owner's config file. #72 has the exact entry and the steps.

## 3. Work done

### 3.1 Phase 0 (#60, merged)

- The design of 2026-11-10 was checked against `main`, the MCP 2026-07-28 specification
  and the documentation of Claude Code, Claude Desktop and Codex. `docs/mcp-design.md` §1.3
  lists each correction.
- The owner answered the ten decisions D1 to D10 with option A (§1.1 of the design).
- `PRD.md` got §13.8 with MC-1 to MC-14, the scope rows in §4.1, §4.2 and Appendix A, and
  the notes in §2.2 and §15.6.

### 3.2 Slice 1.1, owner lock (#61, open)

What it adds:

- `utopia_store::OwnerLock`: an exclusive `flock` on a `<graph>.owner` sidecar. The path is
  canonical, and `.owner` is appended to it.
  - `OwnerLock::sidecar(graph) -> PathBuf`
  - `OwnerLock::try_acquire(graph) -> Result<Option<OwnerLock>>`: no wait. `None` means
    that a different process owns the graph.
  - `OwnerLock::acquire_within(graph, Duration) -> Result<Option<OwnerLock>>`: tries again
    each 50 ms.
- `crates/utopia-app/src/cli.rs`: `ALREADY_OPEN` ("This graph is already open in Utopia"),
  `OWNER_WAIT` (2 s), `take_owner(graph)`, `Owned { store, _owner }` and
  `open_owned(graph)`.
- Who takes the lock: the app (before `Store::open_with_key`, for the life of its store),
  `serve-api` (its full life), `capture`, `import-roam` and `doctor --rebuild` (for the
  call). `doctor`, `export`, `encrypt`, `decrypt` and `Store::open` do not take it.
- Tests: five unit tests in `crates/utopia-store/src/owner.rs` and three process tests in
  `crates/utopia-app/tests/owner_lock.rs`. The process tests start the real binary with
  `UTOPIA_GRAPH`, `UTOPIA_CONFIG_DIR` and `HOME` in a temporary directory. Reuse that
  `Scratch` pattern.
- Screenshots: `docs/verification/feat-mcp-owner-lock/`.

What is different from the design text:

- §2.2 says that the app "shows the message and quits". The window stays open with the
  message, as it does for each other failure to open a graph. It holds no store and no
  lock.
- The 2 s retry runs on the main thread before the first frame. It waits only while a
  different process owns the graph.

Behaviour that changed:

- `utopia capture` from a terminal stops while the app has the graph open. Slice 1.2 sends
  the capture to the app.
- `utopia utopia://…` from a terminal while the app is open shows "already open" in a new
  window. The forward of the link is in phase 4 of the design.

One trap that the slice found: `flock` belongs to the open file description. A second
`try_acquire` in the same process fails while the first lock is held. The unlock screen for
an encrypted graph keeps its lock and gives it to the second attempt.

### 3.3 Slice 1.2, Op route (#62, open)

What it adds:

- `utopia_store::graph_key(graph) -> String`: the SHA-256 hex of the canonical graph path.
  It is the one definition of "the key of a graph in the config directory". The sync
  settings used the same formula and now call this function.
- `utopia_api::Op` also derives `Serialize`, with the JSON shape that it reads.
- `utopia_api::access`:
  - `Level` (`Off`, `Read`, `Write`), `Level::permits(OpKind)`, and
    `Level::refusal(OpKind)`, which gives the text of a 403.
  - `Access { level, when_closed }`, `access_file(config, graph)` (the path
    `<config>/mcp/<graph_key>.json`), `Access::load(path) -> (Access, Option<notice>)` and
    `Access::save(path)` (atomic).
  - A file that is missing, malformed, of an unknown version, with an unknown value or
    with an unknown key means Off. A missing file gives no notice.
- `POST /v1/op`: one `Op` as JSON. The order is: the request checks (`Host`, `Origin`,
  token), then the body (400 if it is not an `Op`), then the level (403), then the same
  inbox as each other route. A refused operation never goes into the queue. The REST
  routes do not use the level.
- `ApiServer::level()` and `ApiServer::set_level(Level)`. A new server is at Off.
- The discovery file is now `<config>/api/<graph_key>.json` (mode 0600):
  `utopia_api::discovery_file(config, graph)`, `read_discovery(path)`, `Discovery`.
- `utopia_api::Client`: a blocking client for the API of an owner. `from_discovery(path)`,
  `new(Discovery)`, `capture(text)`, `op(&Op)`. `ClientError` is `NotListening` (nothing
  was sent), `Status { status, message }` (the text of the server, not changed) or
  `Transport`.
- `crates/utopia-app/src/api_launch.rs`: `plan(env_api, access) -> Option<Launch>` (the
  decision, with no I/O), `load_access`, `serve`, `start`.
- `utopia capture` sends its text to the owner with `POST /v1/capture` when a different
  process owns the graph.

Rules that this slice fixed:

- The API starts when `UTOPIA_API` is set, or when the access file says `read` or
  `write`. With the level Off and no `UTOPIA_API`, there is no server and no socket.
- Only the access file sets the level. `UTOPIA_API=1` with no access file gives the REST
  routes as before, and `/v1/op` answers 403.
- The app and `serve-api` read the access file one time, at start. Slice 2.1 makes the
  change live.
- The two 403 texts name Settings → Assistants. Do not change them without a reason: the
  tests compare them.

Known limits of this slice:

- A `capture` that runs after the app took the owner lock and before the app wrote the
  discovery file stops with "already open". It writes nothing.
- Nothing removes the discovery file at exit. The token protects a stale address.
- The proof of the launch path is in the description of #62. Nothing on the screen
  changed.

### 3.4 Changes to the plan that the work made

- The forward of `capture` to the owner moved from slice 1.1 to slice 1.2. The forward
  needs the discovery file for each graph. The old `<config>/api.json` has no graph in its
  name, so a forward could write to an incorrect graph.
- The access file code is in `utopia-api` (`utopia_api::access`), not in `utopia-mcp`. The
  app needs it in slice 1.2, before the `utopia-mcp` crate exists.
- Settings (phase 2) comes before the write tools (phase 3). This is decision D9.

## 4. How to continue

### 4.1 Set up the machine

```bash
git clone git@github.com:adityadaniel/utopia.git u-topia && cd u-topia
source ~/.cargo/env                              # rustup is not on PATH by default
export DEVELOPER_DIR="$(xcode-select -p)"
export CARGO_INCREMENTAL=0
gh auth status                                    # the pull requests need gh
```

- The Metal toolchain is not necessary (`runtime_shaders`). See "Environment setup" in
  `AGENTS.md`.
- The first build compiles GPUI and takes several minutes. Run it in the background.
- One full `make check` (build, test, clippy) fills approximately 11 GB in `target/`. Each
  worktree has its own `target/`. Make sure that the disk has room before the first build.
- `make check` on a warm build directory takes a few minutes. Most of that is the tests.

### 4.2 Branches

`AGENTS.md` says: for a feature, use a new worktree. All ten slices can use one worktree,
because they are done one at a time. One worktree also means one build directory.

```bash
git fetch origin
git worktree add .claude/worktrees/mcp origin/<tip>      # <tip>: the newest branch in §2
cd .claude/worktrees/mcp
git switch -c <next branch>                               # the name from the table in §2
```

- The base of a pull request is the branch before it in the table:
  `gh pr create --base <branch before> --head <next branch>`.
- CI runs for each pull request that changes code, for each base branch.
- The repository merges with merge commits. When the owner merges a pull request into
  `main`, change the base of the next one: `gh pr edit <number> --base main`. A rebase is
  not necessary with merge commits. If a pull request is squashed, rebase the branches
  above it: `git rebase --onto origin/main <old base> <branch>`.
- A defect in a slice that is already in an open pull request: correct it on the newest
  branch, and write it in "Bugs found on the way" of that pull request. Do not rewrite a
  branch that has an open pull request.

### 4.3 The cycle for one pull request

1. Read the brief in §6 and the files that it names.
2. Write the code and the tests. Name each test after its requirement, for example
   `mc_7_a_read_never_makes_a_page`.
3. `make check` from the worktree root: fmt, build, test, clippy with `-D warnings`.
4. If something on the screen or in the behaviour of the real app changed, do "Verifying a
   change" from `AGENTS.md`: seed a scratch graph, drive the real app with
   `UTOPIA_SHOT_SCRIPT`, read each PNG, save it with `sips -Z 1200` under
   `docs/verification/<branch name with - for />/`.
5. Commit with a Conventional Commits message. Run `make check` again if you changed
   something after step 3.
6. Push the branch. Open the pull request with `.github/pull_request_template.md`: what
   and why with the MC ids, the screenshots, the output of the checks, the bugs found, and
   what is not done.
7. Change the table in §2 of this file.

### 4.4 If one agent directs other agents

The owner asked for this arrangement: an orchestrator that plans and checks, and an
implementor (Claude Opus, medium effort) that writes one slice.

- The orchestrator gives the implementor the brief from §6, the worktree path, the branch,
  the build environment and the rules in §4.5. The implementor commits. It does not push
  and does not open the pull request.
- The implementor reports: each changed file, each decision that the brief did not make,
  the names of the new tests, the last lines of `make check` copied, the screenshots, and
  what it did not do.
- The orchestrator reads the full diff, runs `make check` again, looks at each screenshot,
  then pushes and opens the pull request. A report is not a proof.

### 4.5 Rules that protect the owner's notes and machine

- **Scratch paths for each process.** `utopia capture` and the app find a graph in this
  order: `--graph` or `UTOPIA_GRAPH`, the last graph in the config directory, the only
  graph in `~/Documents/Utopia`. Each process that an agent or a test starts must have
  `--graph` or `UTOPIA_GRAPH`, and `UTOPIA_CONFIG_DIR`, on scratch paths. A process test
  also sets `HOME`. A process without them can write into the owner's real notes.
- **Seed, do not type.** Make test graphs with the `Store` or with
  `cargo run -p xtask -- seed-tasks <graph>`, in a directory below `target/`.
- **No merge without the owner.** The owner merges the pull requests, or tells the agent
  to merge.
- **No change to the config of an AI client.** Do not edit `~/.claude.json`,
  `claude_desktop_config.json` or `~/.codex/config.toml`. For a test, give the client its
  server for that run only (§6, PR 4). Ask the owner before a test with Claude Desktop.
- **Stop each process that you start.** A `serve-api` or an app that stays alive keeps the
  owner lock of its scratch graph.
- **Disk.** Look at `df -h .` before a first build. Do not make a second build directory.

### 4.6 Notes about the tools

- In a Claude Code session that is isolated in a worktree, the harness refuses a shell
  command that has `git` together with other commands. Run each git command alone.
- On the owner's machines a hook (`rtk`) rewrites shell commands. If the permission check
  quotes a rewritten command, run exactly that command.

## 5. Contracts between the pull requests

Each pull request must leave these things for the next one. Names can change if the
implementation has a good reason, but then this section changes in the same pull request.

| After | The code has |
|---|---|
| PR 1 | `utopia_store::OwnerLock` (`sidecar`, `try_acquire`, `acquire_within`). In `utopia-app`: `cli::ALREADY_OPEN`, `cli::OWNER_WAIT`, `cli::take_owner`, `cli::open_owned`. |
| PR 2 | `utopia_store::graph_key`. `utopia_api::Op` with `Serialize`. `utopia_api::{Level, Access}`, `access::access_file`, `Access::load`, `Access::save`. `POST /v1/op` with the level check. `ApiServer::{level, set_level}`. `utopia_api::{discovery_file, read_discovery, Discovery, Client, ClientError}`. In `utopia-app`: `api_launch::{plan, load_access, serve, start}`. |
| PR 3 | Crate `utopia-mcp`: `serve(reader, writer, backend, options)` for the two protocol eras; the `Backend` trait; `Direct` (takes the owner lock for one call, opens the store with an opener that the caller gives, enforces the level, runs `utopia_api::handle`); a distinct error when a different process owns the graph; the tool catalog with the six read tools; the outline format with its id marker; the size budget; `rev` in the result of `read_block`. |
| PR 4 | `utopia mcp --graph <file> [--read-only]` and `utopia mcp config <client>`; `Remote` backend over `/v1/op`; the rule that selects `Direct` or `Remote` for each call; one function that makes the config entry for each client, in a place that `utopia-ui` can call; the cost of a headless call in `xtask bench`. |
| PR 5 | Settings → Assistants writes the access file and changes the level of the running server with no restart. The app can start and stop the loopback API while it runs. |
| PR 6 | The documentation agrees with the code. |
| PR 7 | `Op::AppendOutline` and the tool `append_blocks`. The id marker of the outline format is removed from text that an agent sends back. |
| PR 8 | `Op::UpdateBlock` with `kind`, `checked`, `if_rev`. `rev` is one function, used by the read tools and by the check. |
| PR 9 | `Op::MoveBlock`, `Op::DeleteBlock` and their tools. |

Rules for all of them:

- Dependency direction: `app → ui → (plugin → api) → store → doc → core`, and
  `app → mcp → api → store`. Nothing imports upward.
- Each tool call is one `utopia_api::Op` that `utopia_api::handle` runs (rule 13, MC-3).
  There is no second write path.
- Each new `Op` variant is classified in `Op::kind()`. The test
  `ops_classify_reads_and_writes_consistently` stays green. Plugins use the same
  classification.
- Each new field of an existing `Op` has a serde default. The JSON that plugins and HTTP
  clients send today stays valid.
- Page text goes only into tool results (MC-11). Tool names, descriptions, schemas and the
  `instructions` text are constants.
- No `println!` on the `utopia mcp` path. `stdout` carries protocol messages only.

## 6. Briefs for the pull requests that are not started

### PR 3 — slice 1.3: the `utopia-mcp` crate, the protocol and six read tools

Branch `feat/mcp-read-tools`, base `feat/mcp-op-route`. Requirements: MC-1, MC-2, MC-6,
MC-7, MC-11, and the part of MC-14 about `server/discover` and `initialize`.

Read: design §3 (all of it, §3.2 has the wire rules), §4, §4.1, §4.2, §5;
`crates/utopia-api/src/lib.rs` (`Op`, `Op::kind`, `handle`, `page_json`, `block_json`);
`crates/utopia-api/src/access.rs`.

Build:

- A new workspace member `crates/utopia-mcp`. It depends on `utopia-api`, `utopia-store`,
  `utopia-core`, `serde` and `serde_json`. It has no `gpui` and no async runtime.
- `protocol.rs`: `serve(reader, writer, backend, options)`. One JSON-RPC 2.0 message for
  each line.
  - One thread reads `stdin` and a second thread runs the calls, so the server sees
    `notifications/cancelled` (`params.requestId`) while a call runs. A cancelled call
    that is in the queue does not run. A cancelled call that runs gets no answer.
  - Legacy era (2025-11-25, 2025-06-18, 2025-03-26, 2024-11-05): `initialize` answers with
    the version of the client if the server supports it, and with 2025-11-25 if not. The
    result has `protocolVersion`, `capabilities: {"tools": {}}`, `serverInfo` and
    `instructions`. The server accepts `notifications/initialized` and answers `ping`,
    `tools/list` and `tools/call`. Legacy results have no `resultType`, `ttlMs` or
    `cacheScope`.
  - Modern era (2026-07-28): each request has `params._meta` with
    `io.modelcontextprotocol/protocolVersion` and
    `io.modelcontextprotocol/clientCapabilities`. Without one of them the answer is error
    `-32602`. A version that the server does not support gets `-32022` with
    `data: {"supported": [...], "requested": "..."}`. `server/discover` returns
    `supportedVersions`, `capabilities`, `instructions`, `ttlMs` and `cacheScope`. Each
    result has `"resultType": "complete"` and
    `_meta["io.modelcontextprotocol/serverInfo"]`. `tools/list` has `"ttlMs": 3600000` and
    `"cacheScope": "public"`.
  - An `initialize` request selects the legacy rules for the life of the process. A
    request with the modern `_meta` keys gets the modern rules.
  - A method that the server does not know gets `-32601`. A notification never gets an
    answer. The server sends no request to the client and never answers
    `input_required`.
  - The server exits when `stdin` closes. It opens no store before the first tool call.
  - The server writes one line to `stderr` for the first request: the era and the name of
    the client. PR 4 uses it to record the era of each real client.
- `tools.rs`: the catalog. Each entry has the name, the description, the JSON schema, the
  four hints of design §4.1 and the mapping to an `Op`.

  | Tool | Arguments | Op |
  |---|---|---|
  | `search` | `query`, `limit` | `Search` |
  | `list_pages` | `include_daily` | `ListPages` |
  | `read_page` | `page`: a title, an alias, `today`, `yesterday` or an ISO date | `Journal` for a date, `GetPage` for the others |
  | `read_block` | `id` | `GetBlock`, with the subtree |
  | `get_backlinks` | `page` | `Backlinks` |
  | `run_query` | `query`, `limit` | `Query` |

  - `read_page` and `read_block` return an indented Markdown outline. Each line ends with
    a marker that has the block id. The marker must not be Utopia markup: `((id))` is a
    block reference, so an agent that sends the line back would make a reference. Select
    a marker, document it in the `instructions` text, and write it down in §5 for PR 7.
  - A day without a page gives an empty outline (FR-1.2). `Op::Journal` answers 404 for
    such a day; the tool changes that answer. A read never makes a page.
  - Each result is cut at 24 KB of UTF-8 text, at a block boundary. It then gives the
    number of blocks that it did not return and the call that returns them.
  - `read_block` also returns `rev`: a short hash of the text of the block. Put the
    function in `utopia-api`, because PR 8 uses the same one for `if_rev`.
  - The `instructions` text is a short guide to the syntax: `[[Page]]`, `#tag`,
    `((block-id))`, `key:: value`, and the id marker.
- `backend.rs`: the `Backend` trait (one `Op` in, a JSON value or an error with a status
  and a message out) and `Direct`.
  - `Direct` does one call in this order: load the access file; take the owner lock with
    no wait; open the store with the opener that the caller gave; run
    `utopia_api::handle`; flush; close the store; release the lock.
  - If a different process owns the graph, `Direct` returns a distinct error. PR 4 uses it
    to select `Remote`.
  - `Direct` is the headless path. If `when_closed` is off, it returns "Open Utopia" and
    opens nothing (MC-5).
  - It loads the access file for each call, so a change applies with no restart of the
    client.
- The order of the checks for `tools/call`: a tool that is not in the catalog gets the
  protocol error `-32602`. All other failures are a result with `"isError": true` and a
  text for the model: arguments that are not valid, access Off (the text names Settings →
  Assistants), a write tool at level `read`, a block that does not exist.
- `options.read_only` lowers the level to `read` for this process and removes the write
  tools from `tools/list`. The access file never changes the list.
- Permitted change in `utopia-api`: `Op::GetBlock` gets a field with a default for the
  subtree. No other result of an existing op changes.

Do not build: the subcommand, the `Remote` backend, the write tools, the Settings tab.

Tests, with in-memory pipes:

- One script for each era, from the first message to a tool call.
- `-32022` for a version that is not supported; `-32602` without the `_meta` keys;
  `-32601` for a method that is not known; no answer for a cancelled call; `serve` returns
  at end of input.
- A table of each tool against `off`, `read` and `write`, and against `when_closed`.
- A read never makes a page: the number of pages is the same before and after
  `read_page today`, and `doctor` is clean.
- A large page is cut at the budget and tells how to read the remainder.
- MC-11: `tools/list` and the `instructions` text are the same for two graphs with
  different contents.

Documentation in this pull request: the row for `utopia-mcp` in the workspace layout table
of `AGENTS.md` and in PRD §13.1.

### PR 4 — slice 1.4: the `utopia mcp` subcommand, `Remote`, the headless path

Branch `feat/mcp-subcommand`, base `feat/mcp-read-tools`. Requirements: MC-5, MC-13, MC-14.

Read: design §2.3, §6, §7; `crates/utopia-app/src/main.rs` (`main`, the subcommand
dispatch), `crates/utopia-app/src/cli.rs` (`open_store`, `USAGE`), `xtask/src/main.rs`
(`bench`).

Build:

- `Remote` backend in `utopia-mcp`: it sends the `Op` to the owner with
  `utopia_api::Client::op` and the discovery file of the graph
  (`utopia_api::discovery_file`). The status and the text of an error go to the
  agent with no change (403, 429, 503, 504).
- The rule for each call:
  1. Load the access file. Off: return the Off text. No lock, no connection.
  2. Try `Direct`. If the lock is free, this is the headless call (or "Open Utopia").
  3. If a different process owns the graph, use `Remote`.
  4. If `Remote` cannot connect (no discovery file, or `ClientError::NotListening`), the
     owner runs no API. It can be a short CLI write, so try step 2 again for 2 s. Then
     return a text that tells the user to open Utopia again. Until PR 5, the app reads
     the access file only at start, so a level that changed while the app runs gives
     this case.
- `utopia mcp --graph <file> [--read-only]` in `utopia-app`.
  - `--graph` is necessary (MC-13). Without it the command stops with a usage error. It
    never uses `paths::resolve_graph`.
  - It opens no store at start-up. `server/discover` and `initialize` answer in less than
    1 s (MC-14).
  - The opener for `Direct` must never show a prompt, because `stdin` is the protocol
    channel. `cli::open_store` asks for a passphrase when it finds none. For `utopia mcp`
    an encrypted graph without `UTOPIA_DB_PASSPHRASE` or a Keychain item fails with a
    clear text.
- `utopia mcp config <client> [--graph <file>]` prints the entry for `claude-code`,
  `claude-desktop` or `codex`, with the real path of the binary
  (`std::env::current_exe`). The texts are in design §6. Put the function where
  `utopia-ui` can call it, because PR 5 shows the same texts.
- `xtask bench`: the cost of one headless call (open the store, one `search`, close) on
  the bench graph. Report the number in the pull request. Set the budget after that first
  measurement and write it in the design §10.
- `cli::USAGE` and the commands in `AGENTS.md`.

Tests: process tests in `crates/utopia-app/tests/` that start `utopia mcp` with pipes on
`stdin` and `stdout`, with the `Scratch` pattern of `owner_lock.rs`.

- The two eras, from the first message to one `search`.
- Access Off gives the Off text. `read` gives results.
- App closed: `when_closed` off gives "Open Utopia"; on gives results, and `doctor` is
  clean afterwards.
- A `serve-api` child as the owner: the call goes through the bridge.
- `--read-only`. No `--graph`: the exit code is not zero.
- Each line on `stdout` is a JSON-RPC message. End of input makes the process exit.

Check with the real clients, on a scratch graph and a scratch config directory. These
runs use the owner's accounts. Do not change a global client config (§4.5).

- Claude Code: give the server for one run with a config file, for example
  `claude --mcp-config <file.json> --strict-mcp-config -p "<a prompt that asks for one search>"`.
  Confirm the flags with `claude --help`. Run it one time as it is and one time with
  `MCP_PROTOCOL_NEGOTIATION=legacy`.
- Codex: give the server for one run with config overrides, for example
  `codex exec -c 'mcp_servers.utopia.command="<bin>"' -c 'mcp_servers.utopia.args=["mcp","--graph","<file>"]' "<prompt>"`.
  Confirm the flags with `codex --help`.
- Claude Desktop needs an entry in its config file and a restart. Give the owner the
  entry from `utopia mcp config claude-desktop` and ask the owner to do this test.
- Record the era that each client used (the `stderr` line from PR 3) in design §6, and
  remove the "Not known" line from design §10.

### PR 5 — slice 2.1: Settings → Assistants

Branch `feat/mcp-assistants-tab`, base `feat/mcp-subcommand`. Requirement: MC-12.

Read: `DESIGN.md` (all colours, sizes and radii are tokens; §2.4), `docs/copy-cleanup.md`,
`crates/utopia-ui/src/settings.rs` (`Tab`, `render_sync`),
`crates/utopia-ui/src/sync_setup.rs` and `crates/utopia-app/src/sync_setup.rs`
(`Coordinator`), `crates/utopia-ui/tests/settings.rs`, `crates/utopia-ui/tests/layout.rs`.

Build:

- `Tab::Assistants` in `settings.rs`: the label, the glyph, the id, `Tab::all()`, the
  render branch.
- The Sync tab is the pattern. `utopia-ui` cannot import `utopia-app`, so the tab is a
  view that sends a request, and a coordinator in `utopia-app` applies it:
  `SyncSetup` + `SetupHandle` + `Request` in `utopia-ui`, `Coordinator` in `utopia-app`.
- The controls:
  - The level: Off, Read, Read and write.
  - The switch for `when_closed`.
  - One copy control for each client: the Claude Code command, the Claude Desktop entry,
    the Codex command. The texts come from the function of PR 4.
  - The privacy sentence, adjacent to the level: text that an assistant reads goes to that
    assistant and to its provider.
  - One line of state: the bridge runs or not. A notice when the access file was not
    valid.
- The tab shows the setting of the graph that is open (decision D5).
- The coordinator: save the access file (atomic), then start or stop the loopback API and
  set the level of the server, with no restart.
  - Today the `ApiServer` is an `Option` that the inbox task in `load_graph` captured. It
    must become state that the inbox loop reads each time, so that a server that starts
    later is drained.
  - Level Off without `UTOPIA_API`: drop the server and remove the discovery file. Level
    Off with `UTOPIA_API`: keep the server and set its level to Off.
  - Use the launch decision function of PR 2. Do not write the rule a second time.
- The settings are local and never sync (rule 9).

Tests: the tab switch and the controls in `crates/utopia-ui/tests/settings.rs`; the
geometry in `layout.rs` with `debug_selector`; the apply rule as a function that a unit
test calls with no window.

Verify with the real app:

- `UTOPIA_SHOT_SCRIPT`: `keys cmd-,; wait 1200; target last; wait 500; …; shot <png>`.
  A click lands only on a frame that was painted, so take a `shot` that you discard after
  each click (`AGENTS.md`, known limits).
- Capture the tab at Off and at Read and write, in light and dark
  (`UTOPIA_APPEARANCE=light|dark`). Read each PNG.
- Set Read in the window: the discovery file appears and a `search` through
  `curl … /v1/op` works. Set Off: the socket is closed.
- Save the screenshots under `docs/verification/feat-mcp-assistants-tab/`.

### PR 6 — slice 2.2: documentation

Branch `docs/mcp-docs`, base `feat/mcp-assistants-tab`. It has one code change (the Help
topic), so `make check` and CI are necessary.

- `AGENTS.md`: the scope (what is built), the commands, the files
  (`<config>/mcp/<key>.json`, `<config>/api/<key>.json`), the dependency direction with
  `utopia-mcp`, the Settings tabs (six), the known limits. Correct "P2 (§4.2, §16)": the
  row is in §4.1 and Appendix A.
- `crates/utopia-api/src/lib.rs`: correct the citation of §16 in the first line.
- The Help tour: a topic in `TOPICS` in `crates/utopia-ui/src/help.rs`, and its PNG with
  `cargo run -p xtask -- help-shots`. Read the PNG.
- State machine files in `docs/state-machines/`: a new file for the owner lock, a new
  file for the access level and the life of the loopback API, the new `owner` fields in
  `app-root-startup-and-quit.md`, `/v1/op` and the level check in `api-call-inbox.md`, and
  the index in `README.md`.
- `docs/mcp-design.md`: the status line, §2.2 (the window stays open), §10.
- `README.md`, if it lists the features.

### PR 7 — slice 3.1: `append_blocks`

Branch `feat/mcp-append-blocks`, base `docs/mcp-docs`. Requirements: MC-3, MC-8, MC-9.

Read: `handle` for `Op::AppendBlock` and `Op::Capture`; `utopia_core::markdown::parse_outline`;
`utopia_core::outline::paste` (it already makes one plan from Markdown lines, FR-2.10);
`Store::execute_plan`, `Store::seal_undo`, `Store::commit_block`, `Store::capture`;
`docs/state-machines/store-commit-pipeline.md` and `undo-group-coalescing.md`.

Build:

- `Op::AppendOutline { title, markdown, parent, create }`, classified as a write.
- `handle`: find the page as `AppendBlock` does (`create` makes a page for a title that
  does not resolve). Parse the outline. Make one plan. `seal_undo`, then one commit: one
  transaction and one undo step. Flush.
  - A target that is a date makes the daily page with its first write (FR-1.2). See how
    `Store::capture` does this for today.
  - A page whose only block is empty gets its first line in that block, as `AppendBlock`
    does.
  - Links and tags in the new text must make their references and pages, as they do for
    text from the editor. Anything that reads the index to decide what to write calls
    `sync_index()` first (rule 2).
  - Remove the id marker of the outline format from each line.
  - Refuse an outline that is deeper than 12 levels (FR-2.9) or that has more blocks than
    a limit that you set. Write nothing in that case.
- The tool `append_blocks` in `utopia-mcp`, with the hints of design §4.1.

Tests: `doctor` is clean after the call; one undo removes the full outline; a plugin
without write permission cannot use the new variant; at level `read` the call fails and
writes nothing; a `[[link]]` in the outline gives a backlink; 500 lines stay in the paste
budget of `xtask bench` (AC-2.5).

Verify with the real app: with the app open, send one `append_blocks` call through
`utopia mcp`. Screenshot 1 shows the new blocks. Then `keys cmd-z`. Screenshot 2 shows
that they are gone.

### PR 8 — slice 3.2: `update_block`

Branch `feat/mcp-update-block`, base `feat/mcp-append-blocks`. Requirements: MC-8, MC-10.

Read: `handle` for `Op::UpdateBlock`; `Store::set_block_text`, `Store::set_block_kind`,
`Store::commit_block`.

Build:

- `Op::UpdateBlock` gets `kind`, `checked` and `if_rev`. Each one is optional with a
  default. `{"op": "update_block", "id": …, "text": …}` stays valid, and
  `PATCH /v1/blocks/{id}` does not change.
- `if_rev` is the short hash from `read_block` (PR 3). If it does not agree with the
  block, the call fails and writes nothing (MC-10, §2.3.7). The error tells the agent to
  read the block again.
- A change of text, kind and checked in one call is one undo step.
- The result has the new `rev`.
- The tool `update_block`, with the hints of design §4.1.

Tests: an `if_rev` that does not agree fails with no write; one that agrees writes; a
change of kind; a to-do that is ticked; the old JSON still parses; one undo reverses the
full call; `doctor` is clean.

### PR 9 — slice 3.3: `move_block` and `delete_block`

Branch `feat/mcp-move-delete`, base `feat/mcp-update-block`. Requirements: MC-8, MC-9.

Read: `utopia_core::outline::move_to(st, id, parent, after)`,
`utopia_core::outline::delete_subtree(st, id, now)`,
`Store::move_block_to_page(src, block, dst, parent)`.

Build:

- `Op::MoveBlock`: the block, the target page (optional), the new parent (optional) and
  the sibling that it comes after (optional). The same page uses `outline::move_to`. A
  different page uses `Store::move_block_to_page`. A move into its own subtree or below
  depth 12 fails with a domain error.
- `Op::DeleteBlock`: `outline::delete_subtree`. It makes tombstones (rule 5). The result
  gives the number of blocks. Undo restores them.
- The tools `move_block` and `delete_block`, with the hints of design §4.1.

Tests: a move to a different page keeps the subtree; a `((reference))` to a moved block
still resolves; a delete makes a tombstone and a reference to it shows "deleted block";
undo restores the subtree; `doctor` is clean; at level `read` each call fails and writes
nothing.

Verify with the real app, with screenshots before and after each call.

### PR 10 — slice 3.4: the write check with the real clients

Branch `docs/mcp-write-check`, base `feat/mcp-move-delete`.

- One write from each client on a scratch graph, with the method of PR 4. The owner does
  the Claude Desktop test.
- The block that has the caret: open the app, put the caret in a block and type, then
  send `update_block` for that block through the bridge. Record what the editor shows and
  if a typed character is lost. A lost character breaks §2.3.7. That is a defect to
  correct before the write level is released, not a limit to write down.
- Record the results in design §10. Save the screenshots under
  `docs/verification/docs-mcp-write-check/`.
- Change the scope in `AGENTS.md`: the write tools are built.

## 7. Open points

- **Merge.** No code pull request is merged. The owner decides when.
- **Protocol era of Codex and Claude Desktop.** Not known. PR 4 records it.
- **A cancel does not reverse a write** that the owner already started. The server only
  keeps back the answer (design §3.1).
- **The size budget of 24 KB** is a first value. PR 4 adjusts it with the real clients.
- **Cost of a headless call.** Not measured. PR 4 measures it and sets the budget.
- **Encrypted graph on the headless path.** It needs a passphrase that the process can
  read with no prompt.
- **The Roam column** of the new rows in PRD §4.1 and Appendix A has "—". This was not
  checked.
- **Stale citations of §16** in `AGENTS.md` and `crates/utopia-api/src/lib.rs`. PR 6
  corrects them.
- **Design §2.2** says that the app quits when the graph is already open. The app keeps
  the window with the message. PR 6 corrects the design text, or the owner asks for the
  quit.

## 8. Sources

The specification pages and the client documentation that the design uses are in
`docs/mcp-design.md` §11, with the date on which each one was read.
