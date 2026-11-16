# Verification for `move_block` and `delete_block` (PR 9, slice 3.3)

Five captures of the real window and one run of each real client, all against scratch paths
under `target/verify-move/`. Nothing here used the owner's graph, config directory, Keychain
or client config, and every process was stopped.

## The app open, the calls through the bridge

```bash
cargo run -q -p xtask -- seed-tasks target/verify-move/Demo.utopia
# <config>/mcp/<graph_key>.json = {"version":1,"access":"write","when_closed":true}
UTOPIA_CONFIG_DIR=target/verify-move/config \
UTOPIA_SHOT_SCRIPT="wait 2500; keys escape; wait 300; keys cmd-k; wait 600; type Atlas Launch; …" \
  target/debug/utopia --graph target/verify-move/Demo.utopia
```

While the app ran (it holds the owner lock, so a call goes through `POST /v1/op`, MC-4,
MC-12), one `utopia mcp` process sent one `tools/call` as a JSON-RPC line on `stdin`:

```json
{"name": "move_block",
 "arguments": {"id": "01M7JBNEKD8HSWEE5ZE0Y52NQ4", "page": "Atlas Launch"}}
```

It moved "On conflict-free replicated data types" and its child from Reading to Atlas
Launch. The answer, rendered from the op's result:

```text
Moved 2 blocks to "Atlas Launch": the block and the 1 under it.
It is now the first block of the page.
The moved block is 01M7JBNEKD8HSWEE5ZE0Y52NQ4, the same id it had on its old page; read_block takes it.
```

- **`1-before-move.png`** — Atlas Launch before the call: the properties bar has `status`
  and `owner`, then `## Scope`, `## Risks`, …; no CRDT block.
- **`2-after-move.png`** — the same page after it: the moved block and its child are at the
  top, and Reading's sidebar count is unchanged. Two property blocks now show below the moved
  block as ordinary bullets, because the properties bar only reads the blocks at the very top
  of the page (design §10 records this consequence of "first block of the page").
- **`3-after-move-source.png`** — Reading after the call: the subtree is gone and the last
  block is the one added through `append_blocks`, whose `((01M7…))` reference chip still
  reads the live text "Ana Lovelace keeps the migration notes ›" (the move kept the id).

Then one `delete_block` call on the block that chip names
(`01M7JBNEKDKZCCA6X4BG5FKWAK`), with the app open again:

```json
{"name": "delete_block", "arguments": {"id": "01M7JBNEKDKZCCA6X4BG5FKWAK"}}
```

```text
Deleted the block on "Reading".
They are tombstones, and block 01M7JBNEKDKZCCA6X4BG5FKWAK keeps its id: a ((reference)) to it still resolves and shows a deleted block. Cmd+Z in Utopia restores the whole subtree.
```

- **`5-after-delete.png`** — Reading after the call: "Ana Lovelace keeps the migration notes"
  is off the page and the chip in the block below now reads **deleted block** (rule 5: the row
  and the id stay).
- **`6-after-undo.png`** — after one `keys cmd-z` in the app: the block and its chip are back,
  and the file is byte-identical to `3-after-move-source.png` (same MD5), so the whole delete
  was one undo step (MC-9).

`utopia doctor target/verify-move/Demo.utopia` afterwards: `12 live pages, 39 live blocks,
17 references` / `ok: all invariants hold`, so the projections equal a rebuild after the move
and the delete (§10.4).

## Claude Code 2.1.319 — one `move_block` call

The server was given for this run only; no client config file was touched.

```bash
claude --mcp-config ./claude-mcp.json --strict-mcp-config \
  --allowedTools "mcp__utopia__read_page,mcp__utopia__move_block" -p \
  'Use the utopia MCP tools. Call read_page for the page "Reading". In its answer, find the
   line for the block "On conflict-free replicated data types". Then call move_block exactly
   once, with that block id and page "Atlas Launch". Do not call any other tool and do not
   run any shell command. Then repeat the move_block answer verbatim.'
```

`claude-mcp.json` (in `target/verify-move/clients/`, not a client config):

```json
{"mcpServers":{"utopia":{"command":"/…/target/debug/utopia",
                         "args":["mcp","--graph","/…/target/verify-move/clients/Demo.utopia"],
                         "env":{"UTOPIA_CONFIG_DIR":"/…/target/verify-move/clients/config"}}}}
```

It read the page, took the id from the outline's marker (it also called the client's own
`ToolSearch` first, to load the two tools), and answered with the tool's text:

```text
Moved 2 blocks to "Atlas Launch": the block and the 1 under it.
It is now the first block of the page.
The moved block is 01M7JBR9SP0NRZBHMWHPGT4TS6, the same id it had on its old page; read_block takes it.
```

A `read_page "Atlas Launch"` afterwards showed the moved block and its child at the top with
the same ids, and `read_page "Reading"` no longer had them: the ids survived the move.

## Codex 0.169.0 — one `delete_block` call

Same scratch graph and config directory, the app closed (the headless path, MC-5), the
server given for this run only with `-c` overrides:

```bash
codex exec --approve-for-me \
  -c 'mcp_servers.utopia.command="/…/target/debug/utopia"' \
  -c 'mcp_servers.utopia.args=["mcp","--graph","/…/target/verify-move/clients/Demo.utopia"]' \
  -c 'mcp_servers.utopia.env.UTOPIA_CONFIG_DIR="/…/target/verify-move/clients/config"' \
  'Use the utopia MCP tools. Call read_page for the page "Reading". In its answer, find the
   line for the block "Ana Lovelace keeps the migration notes". Then call delete_block
   exactly once with that block id. Do not run any shell command and do not use any other
   tool. Then repeat the delete_block answer verbatim.'
```

`delete_block` carries `destructiveHint: true` (design §4.1), so this run needs
`--approve-for-me` or an approval policy that can ask, exactly as `update_block` does
(design §10). The script shows the two calls — one `read_page` failed and was retried by the
client — and then Codex repeated the answer:

```text
Deleted the block on "Reading".
They are tombstones, and block 01M7JBR9SPP380QCQR6BWAV0X6 keeps its id: a ((reference)) to
it still resolves and shows a deleted block. Cmd+Z in Utopia restores the whole subtree.
```

A `read_page "Reading"` afterwards had only "A list of what is worth finishing", and `utopia
doctor` on that graph still said `ok: all invariants hold`.

The same call without `--approve-for-me` (Codex's default `approval: never`) is refused
before it reaches the server, and nothing is written:

```text
mcp: utopia/delete_block started
mcp: utopia/delete_block (failed)
MCP tool call requires approval, but approval policy is never
```

`read_page "Reading"` still had the block and `doctor` was still clean after that refusal, so
the destructive hint costs a Codex user an approval flag, not data. `move_block` is not
destructive and runs under the default policy.

## After the review: the fixes, and what they change on screen

Nothing on screen changed. Two answers did, and both were reviewed after the captures
above were taken; the captures and the client runs stay as they happened.

**A delete's undo advice depends on who ran it.** The undo stack is the store's memory, so a
headless call (the app closed, `when_closed` on) has none left when it answers. The tool now
says which of the two happened. Headless, on a fresh scratch graph:

```text
Deleted the block on "Reading".
They are tombstones, and block 01M7JD6HKBYAJHAKE6CY4ZC6KA keeps its id: a ((reference)) to it still resolves and shows a deleted block.
Utopia was closed when this call ran, so it left no undo step: the app cannot bring the subtree back.
```

With an owner holding the graph (`utopia serve-api <graph>` in the background, so the call
goes through `/v1/op`), the same call answers:

```text
Deleted 2 blocks on "Reading": the block and the 1 under it.
They are tombstones, and block 01M7JD6HKB8GQFWG3J2F5X51VA keeps its id: a ((reference)) to it still resolves and shows a deleted block.
Cmd+Z in Utopia restores the whole subtree while the app that ran this call is open; its undo history does not outlive a restart.
```

The delete captured in `5-after-delete.png` ran in the open app, so its `Cmd+Z` sentence was
true; it now carries the "while the app that ran this call is open" clause. `doctor` was
clean after each of these calls too (`ok: all invariants hold`).

**A cross-page move is measured on what is live.** The depth ceiling had counted the rows a
subtree left behind when part of it moved away, so a one-block move into another page could
be refused as "maximum depth exceeded" because of the chain it used to hang from.
`crates/utopia-api/tests/move_delete.rs::mc_8_a_moved_away_subtree_does_not_count_towards_the_depth_ceiling`
covers it; no capture shows it, because the refusal produces no screen.

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
again, and ask it to call `move_block` once (move a block on a scratch graph's page to
another page) and `delete_block` once. Both are one undo step in the app; the delete leaves a
tombstone that `Cmd+Z` restores.
