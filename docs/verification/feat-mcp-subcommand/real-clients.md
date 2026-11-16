# Real-client check for `utopia mcp` (PR 4, slice 1.4)

Two clients (Claude Code 2.1.319 and Codex 0.169.0), each given the server for that run
only, on a scratch graph and a scratch config directory. No client config file was edited:
Claude Code took `--mcp-config` with `--strict-mcp-config`, Codex took `-c` overrides.

```bash
cargo run -q -p xtask -- seed-tasks target/mcp-subcommand/demo.utopia
# <config>/mcp/<graph_key>.json = {"version":1,"access":"read","when_closed":true}
#   (when_closed false for the runs with the app open)
```

The server was the binary built from this branch,
`/Users/adityadaniel/Developer/utopia/target/debug/utopia`, with
`UTOPIA_CONFIG_DIR=target/mcp-subcommand/config`.

## App closed (the headless path, MC-5)

`when_closed` on. Each call took the owner lock, opened the store for that call, ran the
`Op` and closed it again.

### Claude Code — 2026-07-28, `server/discover`

```bash
claude --debug mcp --mcp-config target/mcp-subcommand/claude.json --strict-mcp-config \
  --allowedTools mcp__utopia__search -p \
  'Use only the "utopia" MCP server and its `search` tool: call search with query "Atlas"
   once and report the page titles it returned and the first block line verbatim.'
```

`claude.json`:

```json
{"mcpServers":{"utopia":{"command":"/…/target/debug/utopia",
                         "args":["mcp","--graph","/…/target/mcp-subcommand/demo.utopia"]}}}
```

The server's `stderr` line is in `~/.claude/debug/<session>.txt` (Claude Code logs a stdio
server's `stderr` there in a `--debug` run):

```text
[ERROR] MCP server "utopia" Server stderr: utopia mcp: era=modern protocol=2026-07-28 client="claude-code" version="2.1.319"
```

Answer:

```text
I called `search` once with "Atlas".

**Page titles returned:**
- atlas
- Atlas Launch

**First block line, verbatim:**
- November 16th, 2026: #atlas #beta {id:01M7HWZF7BKHXMZQQ2CGETGQ10}
```

### Claude Code with `MCP_PROTOCOL_NEGOTIATION=legacy` — 2025-11-25 handshake

Same command with `MCP_PROTOCOL_NEGOTIATION=legacy` in the environment:

```text
[ERROR] MCP server "utopia" Server stderr: utopia mcp: era=legacy protocol=2025-11-25 client="claude-code" version="2.1.319"
```

The search returned the same two pages, so both eras of the same client work against the
same `utopia mcp` process type.

### Codex — 2025-06-18 handshake

```bash
codex exec -C target/mcp-subcommand -s read-only \
  -c 'mcp_servers.utopia.command="/…/target/debug/utopia"' \
  -c 'mcp_servers.utopia.args=["mcp","--graph","/…/target/mcp-subcommand/demo.utopia"]' \
  -c 'mcp_servers.utopia.env.UTOPIA_CONFIG_DIR="/…/target/mcp-subcommand/config"' \
  'Use only the "utopia" MCP server and its `search` tool: call search with query "Atlas"
   once, then report the page titles it returned and the first block line verbatim.'
```

The server's `stderr` line is in Codex's own log with `RUST_LOG=debug`:

```text
INFO codex_rmcp_client::stdio_server_launcher: MCP server stderr (/…/target/debug/utopia):
  utopia mcp: era=legacy protocol=2025-06-18 client="codex-mcp-client" version="0.169.0"
```

Answer:

```text
mcp: utopia/search completed
Page titles: **atlas**, **Atlas Launch**
```

Note: Codex does not pass its own environment to a stdio server. Without the
`mcp_servers.utopia.env.UTOPIA_CONFIG_DIR` override the server read the default config
directory, found no access file there, and the tool answered the Off text — which is the
behaviour MC-1 asks for, but not the graph this run meant. Claude Code passes the
environment through.

## App open (the bridge, MC-4, MC-2)

The app held the graph and ran its API (`utopia --graph …`, `assistant access: Read`,
discovery file written). `when_closed` was off for these runs, so a headless call would
have answered "Open Utopia": a result can only have come from `/v1/op`.

- Claude Code (2026-07-28): `era=modern protocol=2026-07-28 client="claude-code"
  version="2.1.319"`, and the search returned `atlas` and `Atlas Launch`.
- Codex (2025-06-18 handshake): `era=legacy protocol=2025-06-18 client="codex-mcp-client"
  version="0.169.0"`, and the search returned `atlas` and `Atlas Launch`.

The app was stopped after each run (the owner lock of the scratch graph is free again).

## Claude Desktop — for the owner to run

Not run here: it needs an entry in `claude_desktop_config.json` and a restart, and that file
is the owner's. The entry, from `utopia mcp config claude-desktop --graph <file>` (run the
command from the app bundle so the printed path is the bundle's):

```json
"utopia": {
  "command": "/Applications/Utopia.app/Contents/MacOS/utopia",
  "args": ["mcp", "--graph", "/Users/<owner>/Documents/Utopia/<graph>.utopia"]
}
```

Put it below `mcpServers` in
`~/Library/Application Support/Claude/claude_desktop_config.json`, start Claude Desktop
again, ask it to search, and read the era from
`~/Library/Logs/Claude/mcp-server-utopia.log` (the design §6). Assistant access must be on
for the graph, and the app must be open or `when_closed` on.
