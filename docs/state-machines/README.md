# Implicit state machines in Utopia

Every place where behaviour depends on a *combination* of flags, `Option`s, counters or
timers without one explicit state type. One file per machine; each lists the states, the
events and guards, where every piece of state lives (`file:line`), the combinations that are
impossible or unhandled with evidence from the code, a mermaid diagram, and a sketch of an
explicit type. Lines refer to the `state-machines` branch at `main f5da7d1`.

## Scoring

Each machine gets three 1–3 scores:

- **Risk** — how likely the flags drift in practice: 3 = a concrete path exists today and is
  reachable by normal use; 2 = reachable but needs an error, race or unusual sequence;
  1 = cosmetic or only reachable through tests/API.
- **Data** — impact on data or identity when it drifts, judged against AGENTS.md rules 1–5
  (block is the atom; CRDT is truth and SQLite a rebuildable projection; resolution at index
  time; frozen identity; never lose a character): 3 = a rule 2/4/5 violation (lost or
  duplicated content, wrong page id, shard/projection disagree); 2 = wrong or duplicated
  write, stale projection, or a sync/attachment that silently stops; 1 = UI-only.
- **Churn** — how often the files holding the state change, from
  `git log --oneline -- <file> | wc -l`: 3 = ≥25 commits (`page_view.rs` 55,
  `workspace.rs` 52, `quick_jump.rs` 30, `settings.rs` 25); 2 = 7–24 (`graph_view.rs` 12,
  `appearance.rs` 11, `editor.rs` 9, `main.rs` 9, `shot.rs` 8, `actions.rs` 7,
  `store/lib.rs` 7); 1 = ≤6 (`writer.rs` 6, `sync_runner.rs` 4, `pdf_view.rs` 4,
  `app_state.rs` 3, `field.rs` 2, `pages.rs` 2, every `utopia-sync`/`syncd`/`ffi`/`plugin`/
  `api`/`core` file 1).

Priority: **high** when the total is ≥ 7; **medium** when the total is 5–6 and Risk ≥ 2;
**low** otherwise. Totals are tie-broken by Data, then Risk.

## High (6)

| Machine | R | D | C | Σ | Why |
|---|---|---|---|---|---|
| [Store commit pipeline](store-commit-pipeline.md) | 3 | 3 | 2 | 8 | `flush` marks a shard persisted before the writer runs; a failed job leaves memory ahead of disk, the sticky error is taken by whoever calls `wait` next, and the UI emits `IndexUpdated` for a commit that failed (rules 2, 5). |
| [Sync engine: outbox and pull cursor](sync-engine-outbox-and-cursor.md) | 3 | 3 | 1 | 7 | All-or-nothing `exchange` loses acks and lets one poison page halt every round; the `seq:<page>` cursor means "relay seq" or "folder file count" depending on transport, so switching transports stops pulls; lost-ack + quota = 413 forever. |
| [Draft daily page](draft-daily-page.md) | 2 | 3 | 2 | 7 | `is_draft` checks `drafts` but not `docs` (the other three readers check both); a promoted draft can collide with a remote page for the same date under another id (rule 4). |
| [Block editor: IME composition](block-editor-ime-composition.md) | 3 | 2 | 2 | 7 | `commit_block` and the destructive actions ignore `marked`; a host-driven `end()` mid-composition wipes the composition while the OS IME is still live. |
| [Sync runner round](sync-runner-round.md) | 2 | 2 | 3 | 7 | `transport: Option` doubles as "in flight"; local store errors read as "Offline — N queued"; prepare failures never back off; a second `enable_sync_with` leaves two loops on one runner. |
| [Daily Notes feed pagination](journal-feed-pagination.md) | 3 | 1 | 3 | 7 | No terminal state: the sentinel row re-requests a batch every frame it is visible, `limit` grows forever through empty years, each batch querying the store. |

## Medium (16)

| Machine | R | D | C | Σ | Why |
|---|---|---|---|---|---|
| [Page lifecycle: live / trashed / merged / purged](page-lifecycle-trash.md) | 2 | 3 | 1 | 6 | Empty Trash removes the only record that an id was tombstoned, so the same uuidv5 is reissued and a peer merges it into the tombstoned doc; trash row and tombstone are two transactions. |
| [Block editor session](block-editor-session.md) | 2 | 2 | 2 | 6 | "Commit on blur" has no blur handler; `target` outlives focus; `anchor` outlives `target`; relative dates stay unexpanded until some host calls `end()`. |
| [App root: startup and quit](app-root-startup-and-quit.md) | 2 | 2 | 2 | 6 | Three quit paths, only one runs `sync_index`; a shot script that fails to parse disables the hotkey and never quits; side tasks live outside `RootState`. |
| [Settings: key capture](settings-key-capture.md) | 2 | 1 | 3 | 6 | Capture survives focus loss, `cmd-escape` binds instead of cancelling, `window` is dead state, one `status` slot for five flows. |
| [Keymap overrides: three copies](keymap-overrides-three-copies.md) | 2 | 1 | 3 | 6 | Invalid keys in `keymap.json` show as the binding in the editor while gpui and Quick Jump use the default; clash detection compares context strings. |
| [Page view rebuild cycle](page-view-rebuild-cycle.md) | 2 | 1 | 3 | 6 | `dirty` is cleared by handler rebuilds then re-set by the deferred `Changed`; a query block makes the view rebuild and re-run queries inside `render` on every store change; `rebuild` skips the `attribute_blocks` resync. |
| [Page view mode](page-view-mode.md) | 2 | 1 | 3 | 6 | `reveal_block` leaves a zoom without `show()`, so popups and the unlinked count go stale; per-page resets are keyed on page only. |
| [Workspace main pane content](workspace-main-pane-content.md) | 2 | 1 | 3 | 6 | `open()` clears the PDF but not the graph: pages opened from the sidebar or Quick Jump while the graph is up are invisible. |
| [Attachment blob state](attachment-blob-state.md) | 2 | 2 | 1 | 5 | `blobs_to_upload` applies `LIMIT` before checking the file exists, so rows without files starve every newer attachment's upload. |
| [FFI mobile sync](ffi-mobile-sync.md) | 2 | 2 | 1 | 5 | The whole network round runs under the store mutex; every error collapses to "Offline". |
| [Plugin run lifecycle](plugin-run-lifecycle.md) | 2 | 2 | 1 | 5 | Host writes are committed mid-run but `changed` pages are dropped if the module then traps, so views are never told. |
| [API call inbox](api-call-inbox.md) | 2 | 2 | 1 | 5 | A call that timed out (504) is still executed later; a client retry duplicates the write. `POST /v1/op` shares the queue, with the assistant access level checked at the door. |
| [One owner for each graph](owner-lock.md) | 2 | 2 | 1 | 5 | The state is an `flock` and a handful of `Option<OwnerLock>` fields: the sidecar's existence is not the state, `flock` is per open file description, and `Running` must drop the store before the lock. |
| [Assistant access: the level and the API's life](assistant-access-and-api-life.md) | 2 | 2 | 1 | 5 | One level in five places (the file, the running server's atomic, `confirmed`, the optimistic view, `UTOPIA_API`), plus an `Option<ApiServer>` in a slot whose `stop` leaves a third state the type does not name. |
| [PDF highlight drag](pdf-highlight-drag.md) | 2 | 2 | 1 | 5 | No view-level mouse-up: an off-page release leaves a ghost selection; a release on another page writes a clamped, wrong highlight block. |
| [Block editor autocomplete](block-editor-autocomplete.md) | 2 | 1 | 2 | 5 | Two definitions of "open" (`is_some` vs non-empty items): Escape on `/xyz` does not leave the block; `accept` uses the live caret against a frozen `start`. |

## Low (15)

| Machine | R | D | C | Σ | Why |
|---|---|---|---|---|---|
| [Realtime WebSocket listener](realtime-listener.md) | 2 | 1 | 1 | 4 | Reconnect backoff never resets; 401 is terminal for notices but not for polling; orphan thread if the engine fails. |
| [Sync compaction snapshots](sync-compaction-snapshots.md) | 2 | 1 | 1 | 4 | `prepare` drains the queue so a failed exchange loses it; desktop and `sync_once` disagree on what "quiet" means, so a queued snapshot waits up to a minute. |
| [Text field: selection, layout, goal, masked](text-field-selection-and-layout.md) | 2 | 1 | 1 | 4 | A stale `goal` resolves on the first keystroke into an empty block; masked inputs never have a valid layout, so clicks and IME bounds are wrong. |
| [Undo group coalescing](undo-group-coalescing.md) | 1 | 2 | 1 | 4 | Groups are popped before fallible work; the stack survives shard rollbacks so inverses reference vanished state. |
| [Encryption at rest](encryption-at-rest-state.md) | 1 | 2 | 1 | 4 | `NOTADB` is read as "encrypted"; `set_encryption` has no open-process guard; no guard for a newer schema version. |
| [Tag-page publication](tag-pages-publication.md) | 1 | 1 | 2 | 4 | `set_date_format`, `empty_trash` and `rebuild_index` bypass the writer and never republish the set. |
| [Graph view layout and drag](graph-view-layout-and-drag.md) | 1 | 1 | 2 | 4 | `local` is a dead dimension of `panel`; hover freezes during a pan. |
| [LAN relay discovery](lan-relay-discovery.md) | 1 | 1 | 2 | 4 | One 5 s discovery; on miss, sync is silently off for the session; `"lan"` is a magic string. |
| [Appearance: applied vs saved](appearance-applied-vs-saved.md) | 1 | 1 | 2 | 4 | Two save paths and two config dirs; `remember` reports success without writing when the dir is empty. |
| [Shot script sequencer](shot-script-sequencer.md) | 1 | 1 | 2 | 4 | Pointer steps swallow a dead-window error, key steps abort the run. |
| [Linked / unlinked references panel](linked-references-panel.md) | 1 | 1 | 3 | 5 | Filter is kept but ignored while zoomed; unlinked count vanishes after `reveal_block`; expanded groups never fold. |
| [Page view popups and hover](page-view-popups-and-hover.md) | 1 | 1 | 3 | 5 | Five independent popup flags, two can occlude at once; rename and alias inputs can both be open; subscriptions leak. |
| [Right sidebar panes and resize](right-sidebar-panes.md) | 1 | 1 | 3 | 5 | Width lives in three places; a drag that ends without mouse-up is applied but never saved. |
| [Quick jump palette](quick-jump-palette.md) | 1 | 1 | 3 | 5 | Partial `(event, mode)` match drops pairs silently; a command can run during a redirect. |
| [New-day rollover](new-day-rollover.md) | 1 | 1 | 3 | 5 | Go-to-today does not clear the banner; the calendar month never follows the rollover. |

## Cross-cutting observations

- **`Option<T>` as a phase flag** appears in the sync runner (`transport`), the editor
  (`target`, `idle`, `ac`), the store (`tag_pages`, `drafts`), and the app root (side tasks).
  Each works until a second reader interprets `None` differently.
- **Two copies of one fact** is the most common drift source: sync on/off (three holders),
  keymap overrides (three), sidebar width (three), filter text (two), `edits` + `idle`,
  `dirty_since` + `pending`, and the assistant access level (the access file, the running
  server's atomic, the tab's optimistic value, the coordinator's confirmed one).
- **Error paths skip the cleanup that success paths do**: `flush` after `mem::take`,
  `exchange` after a successful push, `run` after a committed plugin write, `execute_multi`
  removing a doc but not its draft entry.
- The three machines that touch rules 2, 4 and 5 (commit pipeline, draft daily, page
  lifecycle) all hinge on the same gap: the in-memory shard's belief about durability is
  updated before, and independently of, the writer thread's result.
