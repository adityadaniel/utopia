# Documentation for the MCP work: verification for PR 6 (slice 2.2)

Scratch paths only: the graph under `target/`, `UTOPIA_CONFIG_DIR` in the same place, no
client config file edited.

```bash
cargo run -q -p xtask -- seed-tasks target/pr6-clients/Demo.utopia
# <config>/mcp/<graph key>.json = {"version": 1, "access": "read", "when_closed": true}
```

The server was `/Users/adityadaniel/Developer/utopia/target/debug/utopia` (the shared
`CARGO_TARGET_DIR`), built from this branch. No app was running, so every call took the
headless path (MC-5).

## The Help topic

`cargo run -p xtask -- help-shots` regenerates every capture, including the new
`assistants.png` for the Assistant Access topic, from `target/help-shots/Demo.utopia`.

| File | What it proves |
|---|---|
| `help-assistant-access-light.png` | The new topic in Settings → Help (light theme): the "Assistant Access" chip selected, the three paragraphs, and the embedded picture of Settings → Assistants showing the level control, the privacy sentence and the bridge line. Scrolled to the picture, because the window is shorter than the topic. |
| `help-assistant-access-dark.png` | The same topic in the dark theme, with the same tokens and the same picture. |

The first attempt showed the picture's top edge only: the Settings window is 760 points
tall and the topic is longer, so the detail pane needs a `scroll` step before the `shot`.
The second attempt selected the chip but the shot was taken on a frame the click had not
yet repainted; PR 5's note (a throwaway `shot` after each click) is the fix and is now in
`xtask help-shots`.

The picture inside the topic is a capture of the Settings window, so its client entries name
the paths of the machine that ran `help-shots`. `help-shots` runs the app from
`target/help-shots/Utopia.app/Contents/MacOS/utopia` instead of `target/debug/utopia` for
that reason; the graph in the entries is still the mock `target/help-shots/Demo.utopia`.

## Real clients (one run each, server given on the command line)

Access `read`, `when_closed` on, the app closed. Neither `~/.claude.json`,
`claude_desktop_config.json` nor `~/.codex/config.toml` was touched.

### Claude Code 2.1.319 — 2026-07-28, `server/discover`

```bash
UTOPIA_CONFIG_DIR=target/pr6-clients/config claude --debug \
  --mcp-config target/pr6-clients/claude.json --strict-mcp-config \
  --allowedTools mcp__utopia__search -p \
  'Use only the "utopia" MCP server and its `search` tool: call search with query "Atlas"
   once, then report the page titles it returned and the first block line verbatim.'
```

`claude.json` is `{"mcpServers":{"utopia":{"command":"<bin>","args":["mcp","--graph","<file>"]}}}`.
The server's `stderr` line, from `~/.claude/debug/<session>.txt`:

```text
[ERROR] MCP server "utopia" Server stderr: utopia mcp: era=modern protocol=2026-07-28 client="claude-code" version="2.1.319"
```

Answer:

```text
I ran one `search` for "Atlas" on the utopia MCP server.

**Page titles returned:**
- atlas
- Atlas Launch

**First block line, verbatim:**
- November 16th, 2026: #atlas #beta {id:01M7J3NA1XT5SF16T2D3RPY4QA}
```

### Codex 0.169.0 — 2025-06-18 handshake

```bash
codex exec -C target/pr6-clients -s read-only \
  -c 'mcp_servers.utopia.command="<bin>"' \
  -c 'mcp_servers.utopia.args=["mcp","--graph","<file>"]' \
  -c 'mcp_servers.utopia.env.UTOPIA_CONFIG_DIR="<config>"' \
  'Use only the "utopia" MCP server and its `search` tool: call search with query "Atlas"
   once, then report the page titles it returned and the first block line verbatim.'
```

With `RUST_LOG=debug`, Codex logs the server's `stderr`:

```text
INFO codex_rmcp_client::stdio_server_launcher: MCP server stderr (/…/target/debug/utopia): utopia mcp: era=legacy protocol=2025-06-18 client="codex-mcp-client" version="0.169.0"
```

Answer:

```text
Page titles: **atlas**, **Atlas Launch**

First block line verbatim:
- November 16th, 2026: #atlas #beta {id:01M7J3NA1XT5SF16T2D3RPY4QA}
```

### Claude Desktop — not run here

It needs an entry in `~/Library/Application Support/Claude/claude_desktop_config.json` and a
restart, and that file is the owner's. The entry to add, from `utopia mcp config
claude-desktop --graph <file>` (the path is whatever the installed app is):

```json
"utopia": {
  "command": "/Applications/Utopia.app/Contents/MacOS/utopia",
  "args": ["mcp", "--graph", "/Users/<owner>/Documents/Utopia/<graph>.utopia"]
}
```

Put it below `mcpServers`, start Claude Desktop again, set assistant access for that graph in
Settings → Assistants (and `when_closed` if the app will be closed), and ask it to search.
The protocol era is in `~/Library/Logs/Claude/mcp-server-utopia.log`.

## Stopped processes

No `utopia`, `utopia mcp`, `serve-api` or app process was left running (`pgrep -fl utopia`
is empty); the scratch graph's owner lock is free (`<graph>.owner` stays by design).
