# Verification for `update_block` (PR 8, slice 3.2)

Three captures of the real window, and one run of each real client against a scratch graph.
Nothing here used the owner's graph, config directory, Keychain or client config.

## The app, with the app as the owner of the call

```bash
# a fresh scratch graph and the access file for it
cargo run -q -p xtask -- seed-tasks target/verify-shot/Demo.utopia
# <config>/mcp/<graph_key>.json = {"version":1,"access":"write","when_closed":true}
```

`UTOPIA_SHOT_SCRIPT`, `UTOPIA_CONFIG_DIR` and `--graph` all pointed under
`target/verify-shot/`. The script opened the Home page and took `1-before.png`, then waited
12 s; in that window one `utopia mcp` call sent this `tools/call` — one JSON-RPC line on
`stdin` — to the running app (the app held the owner lock, so the call went through
`POST /v1/op`, MC-4, MC-12):

```json
{"name": "update_block",
 "arguments": {"id": "01M7J7YGDEXTQZ12XZY8NAD2QG",
               "text": "Water the plants and the herbs",
               "checked": true,
               "if_rev": "7ddd4ff0de21"}}
```

The answer, which the tool rendered from the op's result:

```text
Updated the block. It now reads:
- [x] Water the plants and the herbs {id:01M7J7YGDEXTQZ12XZY8NAD2QG}
rev: db010b2b4f04
Send that rev as `if_rev` to change this block again without reading it first.
```

- **`1-before.png`** — Home: "Water the plants" and "Renew the passport" are open to-dos,
  Tasks shows 8.
- **`2-updated.png`** — the same page after the call: the block reads "Water the plants and
  the herbs", ticked and struck through, and Tasks shows 7. The app redrew because the call
  ran inside it.
- **`3-undone-after-undo.png`** — after one `keys cmd-z`: byte-identical to `1-before.png`
  (same MD5), so the text and the tick were one undo step (MC-9).

`utopia doctor target/verify-shot/Demo.utopia` afterwards: `12 live pages, 38 live blocks,
16 references` / `ok: all invariants hold`.

## Claude Code 2.1.319 — one run

The server was given for this run only; no client config file was touched.

```bash
claude --mcp-config ./claude-mcp.json --strict-mcp-config \
  --allowedTools "mcp__utopia__read_page,mcp__utopia__read_block,mcp__utopia__update_block" -p \
  'Use the utopia MCP tools. Call read_page for the page "Home". Then call read_block for
   the to-do block "Water the plants" to get its rev. Then call update_block exactly once
   with that block id, checked true, and that rev as if_rev. Do not call any other tool and
   do not run any shell command. Then repeat the update_block answer verbatim.'
```

`claude-mcp.json` (in `target/verify-clients/`, not a client config):

```json
{"mcpServers":{"utopia":{"command":"/…/target/debug/utopia",
                         "args":["mcp","--graph","/…/target/verify-clients/Demo.utopia"],
                         "env":{"UTOPIA_CONFIG_DIR":"/…/target/verify-clients/config"}}}}
```

It read the block, sent its rev back as `if_rev`, and answered:

```text
I called `update_block` once and the to-do is now checked. It used block id
`01M7J7ZZ1FH0G3XDHHRTQNGQVB` and `if_rev` `7ddd4ff0de21`. Here is its answer, word for word:

Updated the block. It now reads:
- [x] Water the plants {id:01M7J7ZZ1FH0G3XDHHRTQNGQVB}
rev: 7ddd4ff0de21
Send that rev as `if_rev` to change this block again without reading it first.

The rev was the same before and after checking the box. `read_block` describes rev as a
hash of the block's text only, so that matches. It also means `if_rev` won't catch another
client changing only the checked state or the kind between a read and a write.
```

That last paragraph is the limit design §10 records: a `rev` covers the text, not the kind
or the done state.

## Codex 0.169.0 — one run

```bash
codex exec --approve-for-me \
  -c 'mcp_servers.utopia.command="/…/target/debug/utopia"' \
  -c 'mcp_servers.utopia.args=["mcp","--graph","/…/target/verify-clients/Demo.utopia"]' \
  -c 'mcp_servers.utopia.env.UTOPIA_CONFIG_DIR="/…/target/verify-clients/config"' \
  'Use the utopia MCP tools. Call read_page for the page "Home". Then call read_block for
   the to-do block "Renew the passport" to get its rev. Then call update_block exactly once
   with that block id, checked true, and that rev as if_rev. Do not run any shell command
   and do not use any other tool. Then repeat the update_block answer verbatim.'
```

```text
mcp: utopia/read_page started
mcp: utopia/read_page (completed)
mcp: utopia/read_block started
mcp: utopia/read_block (completed)
mcp: utopia/update_block started
mcp: utopia/update_block (completed)

Updated the block. It now reads:
- [x] Renew the passport {id:01M7J7ZZ1FEFFHWT2ZQR2F06C6}
rev: 9f24f8952b01
Send that rev as `if_rev` to change this block again without reading it first.
```

**A finding.** The same run without `--approve-for-me` failed before the call was made:

```text
mcp: utopia/update_block started
mcp: utopia/update_block (failed)
MCP tool call requires approval, but approval policy is never
```

`update_block` is marked `destructiveHint: true` (design §4.1), and Codex's `exec` default
is `approval: never`, which cannot ask, so Codex refuses a destructive MCP tool outright.
`append_blocks` (not destructive) ran under that same default in slice 3.1. So a Codex user
needs an approval policy that can ask, or `--approve-for-me`, for this tool. The hints are
the design's, not this slice's; design §10 now records the consequence.

## What the graph holds afterwards

```text
- ## This week {id:01M7J7ZZ1FFMWWKGKQ6TN1T09H}
  - [x] Water the plants {id:01M7J7ZZ1FH0G3XDHHRTQNGQVB}
  - [x] Renew the passport {id:01M7J7ZZ1FEFFHWT2ZQR2F06C6}

12 live pages, 38 live blocks, 16 references
ok: all invariants hold
```

Both clients wrote through the headless path (the app was closed, `when_closed` on): each
call took the owner lock, opened the store for that call, ran `Op::UpdateBlock`, and closed
it again (MC-5). The eras are the ones design §6 records for these versions (Claude Code
2026-07-28, Codex the 2025-06-18 handshake); the reverse direction — the `stderr` line per
process — was not re-measured in this run.

## Claude Desktop — for the owner to run

Not run here: it needs an entry in `claude_desktop_config.json` and a restart, and that file
is the owner's. The entry, from `utopia mcp config claude-desktop --graph <file>` (run from
the app bundle so the printed path is the bundle's):

```json
"utopia": {
  "command": "/Applications/Utopia.app/Contents/MacOS/utopia",
  "args": ["mcp", "--graph", "/absolute/path/to/graph.utopia"]
}
```

Add it below `mcpServers` in
`~/Library/Application Support/Claude/claude_desktop_config.json`, start Claude Desktop
again, and ask it to tick a to-do on a page of a scratch graph with `update_block`.
