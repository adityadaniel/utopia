# Assistants tab: verification for PR 5 (slice 2.1)

Scratch graph, scratch config, never the owner's real graph or config:

```bash
cargo run -p xtask -- seed-tasks target/mcp-assistants/Demo.utopia
# UTOPIA_CONFIG_DIR=target/mcp-assistants/config
# --graph /Users/<dev>/…/target/mcp-assistants/Demo.utopia (absolute in the entries)
```

The app was driven with `UTOPIA_SHOT_SCRIPT`; each click follows a throwaway `shot`, because a
click only lands on a frame that was painted. Screenshots are downscaled to 1200 px wide.

| File | What it proves |
|---|---|
| `off-light.png` | The tab at Off in the light theme: the three levels with Off selected, the privacy sentence, "The bridge is off.", the `when_closed` switch off, and the three client entries (each pins this binary and the absolute graph path). |
| `read-write-light.png` | The tab at Read and write in the light theme: the level is selected and the bridge line reads "The bridge runs at http://127.0.0.1:59272." — the API started from the click, with no restart. |
| `off-dark.png` | The same tab at Off in the dark theme (the same tokens, contrast-checked). |
| `read-write-dark.png` | Read and write in the dark theme, with the bridge running at its own address. |
| `when-closed-light.png` | The `when_closed` switch on (light). Clicking it saved `"when_closed": true` in the same access file; the level stayed Off, so no socket started, which is the rule (`Action::SetLevel`/`Nothing` never follow `when_closed`). |

The screenshots come from the app capturing its own window, so the real window server was
used; a black PNG would have meant a locked screen (it did not happen).

## The level moves the loopback API (MC-12)

All of it in one running app, with the window open; `curl` is the owner of the graph's API,
checked against the discovery file the app writes (`<config>/api/<graph key>.json`):

1. Opened at **Off** (no access file): no discovery file, no socket.
2. Clicked **Read** in the window:
   - `<config>/mcp/<graph key>.json` became `{"version":1,"access":"read","when_closed":false}`
     (atomic save);
   - the discovery file appeared;
   - `POST /v1/op {"op":"search","q":"atlas"}` answered **200** with the `atlas` and
     `Atlas Launch` hits;
   - `POST /v1/op {"op":"append_block",…}` answered **403** with the text that names
     Settings → Assistants (MC-2).
3. Clicked **Off** in the same window:
   - the access file became `{"version":1,"access":"off",…}`;
   - the discovery file was removed at once;
   - the old URL refused the connection (`HTTP 000`, socket closed) within 20 ms, and the
     app kept running. The server thread is joined on the background executor, never on the
     UI thread, and queued calls are cancelled first (`ApiSlot::stop`): with a call the app
     had not drained, the click returns at once and the handler answers 503
     (`mc_12_turning_access_off_does_not_wait_for_a_queued_call`). A request that reaches
     the handler during the shutdown is refused at level Off.
4. Clicked **Read and write** in the same window:
   - the access file became `{"version":1,"access":"write",…}`;
   - a new discovery file and socket appeared;
   - `search` answered **200** and `append_block` answered **200**.

`UTOPIA_API` was not set for any of these runs, so Off really dropped the server; the
`api_launch::plan` unit test covers the other branch (Off with `UTOPIA_API` keeps the socket
at level Off).

The `when_closed` switch was clicked in its own run: the access file became
`{"version":1,"access":"off","when_closed":true}` — the switch is saved, and it does not
start a socket by itself (a setting for a call that runs with the app closed, MC-5).

## Real clients (one run each, server given on the command line)

Scratch graph and config; the client configurations were **not** edited
(`--mcp-config` + `--strict-mcp-config` for Claude Code, `-c` overrides for Codex). Access was
`read`, `when_closed` was on, so each call took the headless path (the app was closed).

### Claude Code 2.1.319

```bash
UTOPIA_CONFIG_DIR=target/mcp-assistants/config claude \
  --mcp-config target/mcp-assistants/claude.json --strict-mcp-config \
  --allowedTools mcp__utopia__search -p \
  'Use only the "utopia" MCP server and its `search` tool: call search with query "Atlas"
   once and report the page titles it returned and the first block line verbatim.'
```

```text
**Page titles returned:**
- atlas
- Atlas Launch

**First block line, verbatim:**
- November 16th, 2026: Met [[Ana Lovelace]] about the [[Atlas Launch]] timeline {id:01M7HZWFG1SQYV00P2QP9P06P1}
```

### Codex 0.169.0

```bash
codex exec -C target/mcp-assistants -s read-only \
  -c 'mcp_servers.utopia.command="/…/target/debug/utopia"' \
  -c 'mcp_servers.utopia.args=["mcp","--graph","/…/target/mcp-assistants/Demo.utopia"]' \
  -c 'mcp_servers.utopia.env.UTOPIA_CONFIG_DIR="/…/target/mcp-assistants/config"' \
  'Use only the "utopia" MCP server and its `search` tool: call search with query "Atlas"
   once, then report the page titles it returned and the first block line verbatim.'
```

```text
Page titles: **atlas**, **Atlas Launch**

First block line verbatim:

- November 16th, 2026: Met [[Ana Lovelace]] about the [[Atlas Launch]] timeline {id:01M7HZWFG1SQYV00P2QP9P06P1}
```

### Claude Desktop — for the owner to run

Not run here: it needs an entry in `claude_desktop_config.json` and a restart, and that file
is the owner's. The tab shows the object from PR 4, which Settings → Assistants copies; the
entry is run from the app bundle so the printed path is the bundle's:

```json
"utopia": {
  "command": "/Applications/Utopia.app/Contents/MacOS/utopia",
  "args": ["mcp", "--graph", "/Users/<owner>/Documents/Utopia/<graph>.utopia"]
}
```

Put it below `mcpServers` in
`~/Library/Application Support/Claude/claude_desktop_config.json`, start Claude Desktop again,
turn assistant access on for that graph in Settings → Assistants (or set `when_closed`), and
ask it to search. The era is in `~/Library/Logs/Claude/mcp-server-utopia.log`.

## Stopped processes

Every app run was scripted and exited at its last step; no `serve-api`, `utopia mcp` or app
process was left running, and the scratch graph's owner lock is free (`flock` is released by
the kernel when the process ends; the `<graph>.owner` file stays by design).
