# Verification for the write check with the real clients (PR 10, slice 3.4)

Six captures of the real window and one write from each real client a machine can run here,
all against scratch paths under `target/verify-write-check/`. Nothing in this run used the
owner's graph, config directory, Keychain items or client config files: `--mcp-config` with
`--strict-mcp-config` gave Claude Code its server for the run, and `-c` overrides gave Codex
its own. Every process started here was stopped.

The two things the slice asks for are below:

1. **The block that has the caret** (design §10's open point). An agent's `update_block` on
   the block the owner is typing in: what the editor shows, and whether a typed character is
   lost. No character was lost and no defect was found; `crates/utopia-ui/tests/workspace.rs`
   now pins the behaviour (`mc_10_a_write_to_the_caret_block_keeps_the_caret`,
   `mc_10_a_stale_if_rev_keeps_the_typed_text_while_the_caret_is_in_the_block`).
2. **One write from each real client** — Claude Code 2.1.319 through the open app's
   `/v1/op` bridge, and Codex 0.169.0 on the headless path — each on a scratch graph, with
   the server given for that run only. The Claude Desktop run is the owner's (§5).

## The scratch graphs and the access files

```bash
cargo run -q -p xtask -- seed-tasks target/verify-write-check/Caret.utopia
cargo run -q -p xtask -- seed-tasks target/verify-write-check/Caret2.utopia
cargo run -q -p xtask -- seed-tasks target/verify-write-check/clients/Demo.utopia
```

Each graph's access file lives at `<config>/mcp/<graph_key>.json`
(`utopia_store::graph_key`, the SHA-256 hex of the canonical graph path), written on the
scratch config directory only:

```json
{"version": 1, "access": "write", "when_closed": false}
```

`when_closed` is `false` for the runs with the app open and `true` for the headless read and
the Codex run, so a call cannot take the wrong path by accident: with `when_closed` off and
the app stopped, the bridge answers "Open Utopia, or turn on \"Work when closed\" for
assistants…" and nothing is read or written (MC-5).

## 1. The block with the caret

`target/verify-write-check/Caret.utopia`, the app opened on today's daily page with the
caret already in its first block (AC-1.1). The app was driven with `UTOPIA_SHOT_SCRIPT` and
`UTOPIA_CONFIG_DIR` on the scratch paths; one `utopia mcp` process sent one `tools/call` as
a JSON-RPC line on `stdin` while the app held the owner lock, so the write ran in the app
through `POST /v1/op` (MC-4, MC-12):

```bash
UTOPIA_CONFIG_DIR=target/verify-write-check/config-caret \
UTOPIA_SHOT_SCRIPT="wait 2500; type owner typed this: ; wait 2000; \
  shot …/1-caret-before.png; wait 9000; shot …/2-caret-after-update.png; \
  type !; wait 700; shot …/3-caret-after-typing.png" \
  target/debug/utopia --graph target/verify-write-check/Caret.utopia
```

```json
{"name": "update_block",
 "arguments": {"id": "01M7K94BWY7VB81VT4EX0ZZ7CP",
               "text": "The assistant wrote this line while the caret was in the block."}}
```

Answer:

```text
Updated the block. It now reads:
- The assistant wrote this line while the caret was in the block. {id:01M7K94BWY7VB81VT4EX0ZZ7CP}
rev: e4ff482609a9
Send that rev as `if_rev` to change this block again without reading it first.
```

- **`1-caret-before.png`** — the block the caret is in, after the owner typed
  `owner typed this:` into it: the raw text and the links show because the block is being
  edited, and the caret sits between the typed text and `Met [[Ana Lovelace]]…`.
- **`2-caret-after-update.png`** — after the call: the editor shows the agent's line, the
  caret is still in that same block (now after `block.`), and the block is still open for
  typing. The sidebar's `Ana Lovelace` and `Atlas Launch` counts fell by one (5→4, 4→3)
  because the replaced line no longer links them: the store, not just the editor, took the
  write.
- **`3-caret-after-typing.png`** — `!` typed into the app after the call: it lands at the
  end of the agent's line, in the same block. `read_page today` afterwards has
  `The assistant wrote this line while the caret was in the block.!`, so the keystroke after
  the call reached the store.

What the editor does, and why no character is lost:

- Every keystroke is written to the store at once (`BlockEditor::persist` →
  `Store::set_block_text`), so when the call runs, the store already holds what the owner
  typed; the app's store is debounced only in when the commit reaches the writer, not in
  what the in-memory shard says.
- The call replaces the block's text — that is what `update_block` does, the same as
  selecting the line and typing over it — and the app announces the changed page, the editor
  re-reads it (`sync_from_store`) and puts the caret back with its CRDT anchor. Nothing the
  owner typed after the call is dropped, and the editor does not resurrect the old line.
- The owner's line being replaced is the reason `if_rev` exists (MC-10, §2.3.7). With the
  rev the agent read before the owner typed, the same call is refused and writes nothing:

  ```text
  the block changed since it was read: its rev is 7fe2dc1ce4d1, not 2c8e5ca1419d. Read
  the block again with read_block, then send the text and rev it gives you. Nothing was
  written.
  ```

  **`4-typed-after-refused-write.png`** is `Caret2.utopia` after that refusal: the typed
  text and the caret are exactly where they were (the capture 9 s later is byte-identical,
  same MD5 `002854c0…`), and the block count is unchanged. The rev the agent read before
  typing was `2c8e5ca1419d`; the store's was `7fe2dc1ce4d1`.

`utopia doctor` after each graph: `ok: all invariants hold` (MC-3).

## 2. Claude Code 2.1.319 — one write through the bridge

The app stayed open on `target/verify-write-check/clients/Demo.utopia` and ran its API at
`assistant access: Write`; `when_closed` was off, so a call could only run through
`/v1/op`. The server was given for this run only, through a small wrapper that also kept its
one `stderr` line:

```bash
claude --mcp-config target/verify-write-check/clients/claude-mcp.json --strict-mcp-config \
  --allowedTools "mcp__utopia__read_page,mcp__utopia__update_block" -p \
  'Use only the "utopia" MCP server. Call read_page for the page "Home". Then call
   update_block exactly once, with the block id of the to-do "Renew the passport", and text
   "Renew the passport and the visa". Do not call any other tool and do not run any shell
   command. Then repeat the update_block answer verbatim, and nothing else.'
```

`claude-mcp.json` (in `target/verify-write-check/clients/`, not a client config):

```json
{"mcpServers":{"utopia":{"command":"…/clients/utopia-server.sh","args":[],
                         "env":{"UTOPIA_CONFIG_DIR":"…/clients/config"}}}}
```

which is `exec /…/target/debug/utopia mcp --graph /…/clients/Demo.utopia` with its `stderr`
appended to a file. The era came out of that file, in 17 s:

```text
utopia mcp: era=modern protocol=2026-07-28 client="claude-code" version="2.1.319"
```

The answer it repeated:

```text
Updated the block. It now reads:
- [ ] Renew the passport and the visa {id:01M7K9725X5QEEF117C11A5YBR}
rev: 49764996bf00
Send that rev as `if_rev` to change this block again without reading it first.
```

- **`5-clients-home-before.png`** — `Home` in the running app before the call. It already
  shows the Codex write from §3 (two blocks at the end of the page) and the to-do
  `Renew the passport`.
- **`6-clients-home-after-claude-code.png`** — the same window after the call: the to-do
  reads `Renew the passport and the visa`, and every other block is where it was. The app
  redrew because the call ran inside it.

## 3. Codex 0.169.0 — one write on the headless path

Same scratch graph and config directory, the app stopped, `when_closed` on, the server
given for this run only with `-c` overrides (no `~/.codex/config.toml` change):

```bash
codex exec -C target/verify-write-check/clients -s read-only \
  -c 'mcp_servers.utopia.command="…/target/debug/utopia"' \
  -c 'mcp_servers.utopia.args=["mcp","--graph","…/clients/Demo.utopia"]' \
  -c 'mcp_servers.utopia.env.UTOPIA_CONFIG_DIR="…/clients/config"' \
  'Use the utopia MCP tools. Call read_page for the page "Home". Then call append_blocks
   exactly once with page "Home" and markdown "- Codex wrote this line while the app was
   closed" followed by a newline and "  - and this child line". Do not call any other tool
   and do not run any shell command. Then repeat the append_blocks answer verbatim, and
   nothing else.'
```

The era, from Codex's own log (`RUST_LOG=debug`), in 19 s:

```text
INFO codex_rmcp_client::stdio_server_launcher: MCP server stderr (…/target/debug/utopia):
  utopia mcp: era=legacy protocol=2025-06-18 client="codex-mcp-client" version="0.169.0"
```

The answer:

```text
Appended 2 blocks to "Home".
The first new block is 01M7K97KMR20X0NRPDDABGB33E and the last is 01M7K97KMR86TX0KNG54PYH2K1; read_block takes either id.
```

`append_blocks` carries `destructiveHint: false` (design §4.1), so Codex's default
`approval: never` ran it; no approval flag was needed. The two blocks are the ones in
`5-clients-home-before.png`: each call took the owner lock, opened the store for that call,
ran `Op::AppendOutline`, flushed and closed it (MC-5), and the app read them back when it
opened in §2.

## What the graph held afterwards

```text
- ## This week {id:01M7K9725XGM6RFBCG7MGT442E}
  - [ ] Water the plants {id:01M7K9725X9F9CYHRSDMM44W6F}
  - [ ] Renew the passport and the visa {id:01M7K9725X5QEEF117C11A5YBR}
- Codex wrote this line while the app was closed {id:01M7K97KMR20X0NRPDDABGB33E}
  - and this child line {id:01M7K97KMR86TX0KNG54PYH2K1}

12 live pages, 40 live blocks, 16 references
ok: all invariants hold
```

Both clients wrote through a path the code already had: Claude Code through `POST /v1/op`
in the open app, Codex through `Direct` with the app closed. Neither made a second write
path (MC-3, rule 13), and `doctor` is clean after both (rule 2, §10.4).

## Claude Desktop — for the owner to run

Not run here: it needs an entry in `claude_desktop_config.json` and a restart, and that file
is the owner's. The entry, from `utopia mcp config claude-desktop --graph <file>` run from
the app bundle so the printed path is the bundle's:

```json
"utopia": {
  "command": "/Applications/Utopia.app/Contents/MacOS/utopia",
  "args": ["mcp", "--graph", "/absolute/path/to/a/scratch/graph.utopia"]
}
```

Put it below `mcpServers` in
`~/Library/Application Support/Claude/claude_desktop_config.json`, start Claude Desktop
again, and ask it for one write on a scratch graph — for example: “Use the utopia MCP
server. Call read_page for the page "Home", then call update_block once with the id of the
to-do "Water the plants" and text "Water the plants and the herbs". Repeat the answer.”
Assistant access must be Write for that graph, and the app must be open or that graph's
`when_closed` on. The era Claude Desktop used goes in the server's `stderr`, which Claude
Desktop writes to `~/Library/Logs/Claude/mcp-server-utopia.log` (design §6).
