# Real-client check for `append_blocks` (PR 7, slice 3.1)

Two clients (Claude Code 2.1.319 and Codex 0.169.0), each given the server for that run
only, on a scratch graph and a scratch config directory. No client config file was edited:
Claude Code took `--mcp-config` with `--strict-mcp-config`, Codex took `-c` overrides.

```bash
cargo run -q -p xtask --release -- seed-tasks target/verify-clients/Demo.utopia
# <config>/mcp/<graph_key>.json = {"version":1,"access":"write","when_closed":true}
```

The server was the release binary of this branch,
`/Users/adityadaniel/Developer/utopia/target/release/utopia`, with
`UTOPIA_CONFIG_DIR=target/verify-clients/config`. The app was closed, so each call took the
owner lock, opened the store for that call, ran `Op::AppendOutline` and closed it again
(MC-5, the headless path).

## Claude Code — one `append_blocks` call

```bash
claude --mcp-config ./claude-mcp.json --strict-mcp-config \
  --allowedTools mcp__utopia__append_blocks -p \
  'Call the utopia tool append_blocks exactly once with page "today" and markdown
   "- Written by Claude Code through the MCP bridge\n  - one nested line".
   Do not call any other tool. Then repeat the tool'"'"'s answer verbatim.'
```

`claude-mcp.json`:

```json
{"mcpServers":{"utopia":{"command":"/…/target/release/utopia",
                         "args":["mcp","--graph","/…/target/verify-clients/Demo.utopia"],
                         "env":{"UTOPIA_CONFIG_DIR":"/…/target/verify-clients/config"}}}}
```

Answer:

```text
Appended 2 blocks to "November 16th, 2026".
The first new block is 01M7J5GPM6ZXGQB3G5BDFK8PW1 and the last is
01M7J5GPM63WT47PEPHF3958B9; read_block takes either id.
```

## Codex — one `append_blocks` call

```bash
codex exec \
  -c 'mcp_servers.utopia.command="/…/target/release/utopia"' \
  -c 'mcp_servers.utopia.args=["mcp","--graph","/…/target/verify-clients/Demo.utopia"]' \
  -c 'mcp_servers.utopia.env.UTOPIA_CONFIG_DIR="/…/target/verify-clients/config"' \
  'Use the utopia MCP tool append_blocks once, with page "today" and markdown
   "- Written by Codex through the MCP bridge". Do not run any shell command and do not
   use any other tool. Then repeat the tool'"'"'s answer verbatim.'
```

Codex called it under its default approval policy (`approval: never` in the run's header)
without an approval prompt, which is what `append_blocks` marked not read-only and not
destructive is meant to allow:

```text
mcp: utopia/append_blocks started
mcp: utopia/append_blocks (completed)
Appended 1 block to "November 16th, 2026".
The new block is 01M7J5H6P2XN3KYPBXFNB3BY5H; read_block takes that id.
```

## What the graph holds afterwards

`read_page today` through a third `utopia mcp` process, then `utopia doctor`:

```text
- Written by Claude Code through the MCP bridge {id:01M7J5GPM6ZXGQB3G5BDFK8PW1}
  - one nested line {id:01M7J5GPM63WT47PEPHF3958B9}
- Written by Codex through the MCP bridge {id:01M7J5H6P2XN3KYPBXFNB3BY5H}

12 live pages, 41 live blocks, 16 references
ok: all invariants hold
```

The nesting Claude Code sent landed as a child block; the marker some of the text carried
was stripped (it is not in the stored text). The eras the two clients use are the ones
`docs/mcp-design.md` §6 records for these versions (Claude Code 2026-07-28, Codex the
2025-06-18 handshake).

## App open, the same tool through the bridge

`UTOPIA_SHOT_SCRIPT` ran the app on `target/verify-append/Demo.utopia` (access `write`),
and one `utopia mcp` call sent `append_blocks` while it was open: the app ran it through
`POST /v1/op` and redrew, which is `2-appended.png`. `keys cmd-z` then took the whole
outline off in one step, which is `3-undone-after-undo.png` (identical to `1-before.png`).

## Claude Desktop — for the owner to run

Not run here: it needs an entry in `claude_desktop_config.json` and a restart, and that file
is the owner's. The entry, from `utopia mcp config claude-desktop --graph <file>` (run the
command from the app bundle so the printed path is the bundle's):

```json
"utopia": {
  "command": "/Applications/Utopia.app/Contents/MacOS/utopia",
  "args": ["mcp", "--graph", "/absolute/path/to/graph.utopia"]
}
```

Add it below `mcpServers` in
`~/Library/Application Support/Claude/claude_desktop_config.json`, start Claude Desktop
again, and ask it to call `append_blocks` on a page of a scratch graph.

## Re-run after the review fixes

The two review findings (the markdown argument's indentation, and the outline sharing one
transaction with the pages its links make) changed the write path, so both verifications
were repeated with the fixed binary:

- The app run behind `1-before.png` / `2-appended.png` / `3-undone-after-undo.png` was
  repeated on a freshly seeded graph; the three captures are byte-identical to the ones in
  this directory (same MD5), so one Cmd+Z still takes the whole outline off.
- One Codex 0.169.0 run on a fresh scratch graph (`target/verify-clients2`), app closed:
  it answered `Appended 2 blocks to "November 16th, 2026"`, `read_page today` shows

  ```text
  - Written by Codex after the atomicity fix {id:01M7J6XAH0XJVQT3JAK03AM7F4}
    - with a child {id:01M7J6XAH0AGA27SQETN5BSY4Z}
  ```

  and `utopia doctor` says `ok: all invariants hold`.
