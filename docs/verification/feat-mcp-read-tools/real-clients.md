# Real-client check for the MCP crate (PR 3, slice 1.3)

Two clients, one run each, on a scratch graph and a scratch config directory:

```bash
cargo run -q -p xtask -- seed-tasks target/mcp-clients/demo.utopia
# <config>/mcp/<graph_key>.json = {"version":1,"access":"read","when_closed":true}
```

The server was a temporary stdio harness around `utopia_mcp::serve` and
`utopia_mcp::Direct` (the same code `utopia mcp` will start in PR 4), given to each client
for that one run only. No client config file was edited or left behind.

## Claude Code 2.1.319 — 2026-07-28

```bash
claude --mcp-config target/mcp-clients/claude.json --strict-mcp-config \
  --allowedTools mcp__utopia__search mcp__utopia__read_page \
  -p 'Use the utopia MCP server (and no other tool): call `search` with query "Atlas",
      then `read_page` for the page "Atlas Launch". ...'
```

Server log (the one line PR 4 records the era from):

```text
utopia mcp: era=modern protocol=2026-07-28 client="claude-code" version="2.1.319"
```

Answer:

```text
I called `search` with "Atlas" and `read_page` for "Atlas Launch", using only the utopia
MCP server.

**Page titles `search` returned** (its Pages section):
- atlas
- Atlas Launch

The search also returned 5 matching blocks. They sit on the pages November 16th, 2026
(two blocks), November 15th, 2026, November 12th, 2026 and Ana Lovelace.

**First block line of the `read_page` outline for "Atlas Launch"**, verbatim:
- status:: in progress {id:01M7H9Y6WJE8TQ1MDRT8W8KEVH}
```

## Codex 0.169.0 — 2025-06-18 handshake

```bash
codex exec -C target/mcp-clients/work -s read-only \
  -c 'mcp_servers.utopia.command="/…/target/debug/examples/stdio"' \
  -c 'mcp_servers.utopia.args=["…/demo.utopia","…/config","…/codex-stdio.log"]' \
  'Use the utopia MCP server and no other tool: call search with query "Atlas", then
   read_page for the page "Atlas Launch". ...'
```

Server log:

```text
utopia mcp: era=legacy protocol=2025-06-18 client="codex-mcp-client" version="0.169.0"
```

Answer:

```text
Search returned these page titles:

- atlas
- Atlas Launch

First block line of “Atlas Launch”:
- status:: in progress {id:01M7H9Y6WJE8TQ1MDRT8W8KEVH}
```

Both clients read the graph, both got the `{id:…}` marker unchanged, and neither needed an
answer shape the server does not send. The era each one used is recorded in
`docs/mcp-design.md` §6.
