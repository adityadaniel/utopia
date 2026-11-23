# Sync incident, 2026-11-23: iPhone lines that never reached the Mac

Status: **fixed** in #100, #102, #103 and #104 (merged 2026-11-23). After the fix, both
devices were updated and the lost lines arrived on the Mac. This document records what
happened, why, how we found it, what changed, and how to debug sync the next time.

## Summary

The iPhone and the Mac synced one graph (`utopia`) through the Cloudflare relay with
end-to-end encryption. Lines written on the Mac appeared on the iPhone. Lines written on the
iPhone never appeared on the Mac. Both devices said "Synced", and no error appeared anywhere.

There were three causes. Each one alone was enough to lose data or to hide the loss:

1. **The sender trusted an acknowledgement from the wrong destination.** The iPhone first
   synced with the mistyped graph id `Utopia`. Its first changes were marked sent there.
   After the fix of the id to `utopia`, nothing sent them again. The iPhone core had no step
   that re-sends the graph when the destination changes.
2. **The receiver dropped changes that it could not apply.** The iPhone's later changes
   reached `utopia`, but they depend on the first ones. Loro kept them as *pending*.
   `LoroPageDoc::import` ignored that, the cursor moved past them, and they were lost. The
   status still said "Synced".
3. **A waiting page could not heal from a snapshot** (found later by the fuzz test). If the
   missing changes arrived only inside another device's compaction snapshot, a device whose
   cursor was already past the snapshot never received them.

## Timeline (2026-11-23, local time)

| Time | Event |
| --- | --- |
| Before 12:44 | The iPhone app (PR #98) syncs with graph id `Utopia` (typo). Its first changes to today's page (`<phone>:1..:3`) go to `Utopia` and are marked sent. |
| 12:44–12:45 | The iPhone writes "Now syncing" and "Anothet one is here". The graph id is changed to `utopia`. |
| 13:54–14:05 | The Mac writes lines; they appear on the iPhone. The iPhone writes "Another shehsj", "Testing from mobile" and an empty line; they do not appear on the Mac. |
| ~14:00 | Investigation starts with `wrangler tail`. |
| ~14:20 | Root causes 1 and 2 confirmed from live data. Fable (high effort) is asked for a fix design, debugging tools and regression checks. |
| Afternoon | Step 1 (#100), step 2 (#102), step 3 (#103) and step 4 (#104) are built and verified. The fuzz test of step 4 finds root cause 3. |
| Evening | All four PRs are merged. After both apps are rebuilt, the iPhone's lines arrive on the Mac. |

## How it was diagnosed

The evidence, in the order we found it:

1. **`wrangler tail` (the relay log).** All requests went to graph `utopia` and returned
   200 or 304. The relay worked. All five POSTs matched the times of Mac edits. The iPhone
   only read the manifest and pushed nothing. Note: both devices use the `ureq` HTTP client,
   and on one Wi-Fi network they share one IP address, so the log alone cannot tell them
   apart.
2. **The Mac's graph file** (read-only `sqlite3`). The cursor for today's page was 90, the
   same as the relay's head. So the Mac had pulled everything, but it had no iPhone blocks.
3. **The Mac's Loro document** (a throwaway dump binary). The version vector had two peers,
   and both were the Mac: one for each app session. There was no iPhone peer.
4. **The relay's update log for the page** (`GET …/updates?since=0` with the token). Each
   update id is `<client_id>:<client_seq>`. The Mac had `:3633..:3718` with no gap. The
   iPhone had **only `:4..:7`**. So the iPhone's `:1..:3` never reached `utopia`.
5. **The code.** `LoroPageDoc::import` ignored `ImportStatus.pending` and returned
   `Ok(false)`. The engine then advanced the cursor. The FFI had no equivalent of the
   desktop's `prepare_sync_destination`.

Today, steps 2–4 are one command: `utopia sync inspect --deep` (see below).

## Root causes in detail

### 1. An acknowledgement is only true for the destination that gave it

The outbox marks a row `sent_at` when a destination accepts it. When the destination
changes (another graph id, another relay), those rows are not on the new destination. The
desktop handles this with `Store::prepare_sync_destination`, which queues a full copy of
every page. The iPhone core (`utopia-ffi`) had no such step, so a change of graph id in the
iPhone's settings left a permanent gap.

A plain "send the old rows again" does not work: the relay ignores a change id that it
already stored, and `utopia-syncd` dedupes change ids across all graphs.

### 2. Loro's pending changes were treated as "nothing new"

Each Loro change depends on the device's earlier changes. A change whose dependencies are
missing goes into Loro's pending queue. The old `import`:

- returned `Ok(false)` ("nothing changed"), so `import_batch` wrote nothing;
- let the sync engine move the cursor past the update;
- kept the pending changes only in memory, and no snapshot contains them.

So the changes were lost at the next restart or compaction. Worse, a later compaction by the
receiving device would upload a snapshot without them, and the relay would delete its copy.

### 3. A waiting page did not read a snapshot behind its cursor

A pull from a cursor returns a snapshot only when the cursor is behind the snapshot. If a
device pulled the waiting changes, and later the sender (which has everything) compacted the
relay, the missing changes existed only in that snapshot. The waiting device never pulled it.
Every device looked caught up, and the page waited for ever. The fuzz test found this with
seed 508.

### Related problems found on the way

- The iPhone shell showed a partial round (failed or waiting pages) as "Synced".
- Relay compaction by the receiving device would have deleted the only relay copy of the
  waiting changes.
- The iPhone app does not sync when it returns to the foreground (AC-9.3). **Still open.**

## What changed

| PR | Change |
| --- | --- |
| #100 | **The receiver keeps waiting changes and reports them.** `PageDoc::waiting()` exports Loro's pending changes. Schema **v11** adds `page_pending`, their only durable copy, which is loaded again with the page. The cursor still advances, so there is no pull loop. A device that holds waiting changes never compacts that page on the relay. A new **blocked** state: the desktop shows "N pages wait for changes from another device", `utopia sync --json` lists `blocked_pages` (exit 1), the FFI returns `Partial { blocked }`, and the iPhone no longer shows a partial round as "Synced". |
| #102 | **The sender sends what the destination lacks.** At first contact with a destination (a pull from 0), the engine measures what the destination holds (`utopia_doc::version_of`; waiting changes do not count) and queues the difference under a **new change id**. A page that the destination does not list at all is sent whole. Every device runs this comparison **once after the upgrade** (`audit_epoch`), and **`utopia sync repair`** runs it on demand. This is what healed the real incident. |
| #103 | **Debugging tools.** `utopia sync inspect` (read-only, with findings and advice), a `doctor` check for waiting pages, and the sync log `<config>/logs/sync.jsonl`. |
| #104 | **Regression checks, and the fix for root cause 3.** A page in `page_pending` is pulled from 0 on every fresh manifest. New: the convergence fuzz test, `make sync-fuzz`, a rule in AGENTS.md and a checkbox in the PR template. |

## How to debug sync next time

The full procedure is in `docs/sync-modes.md`, "Debugging sync". In short:

1. `utopia sync status --graph <file>`. Is the **graph id** the same on every device? (That
   was the trigger of this incident.) Is there a `waiting:` line?
2. `utopia sync inspect --graph <file> --deep` on **each** device. It is read-only and works
   while the app is open. Each finding names the action to take:
   - `waiting_here`: this device holds another device's changes that cannot apply yet.
   - `destination_has_gap`: the destination holds changes whose earlier changes never
     reached it.
   - `this_device_has_more`: this device must send. Run `utopia sync`, then, if the finding
     stays, `utopia sync repair` **on this device**.
   - `destination_has_more`, `not_pulled_yet`, `unsent`: run `utopia sync`.
3. `utopia doctor <file>`. It reports each waiting page.
4. Read the log, for example:
   `tail -f ~/Library/Application\ Support/Utopia/logs/sync.jsonl | jq -c '{ts, source, outcome, waiting}'`.
5. For the relay itself: `npx wrangler tail` in `relay-cloudflare/`, and `GET /healthz`.

Things that misled us, and are worth knowing:

- **Gaps in a client's change ids on one page are normal.** `client_seq` counts the outbox
  of the whole graph. Use the CRDT signal (changes in the destination's copy that wait), which
  is what `inspect` does.
- **A Loro peer id is per process, not per device.** The Mac's doc had two peers from two app
  sessions. Each `utopia capture` run gets a new peer. So `inspect` names the side that must
  act, not the device: run it on each device.
- **The peer and counters of a waiting range name the waiting changes,** not the missing
  ones. The missing changes may come from another peer of the same device.

## Guards against a regression

- **Unit and acceptance tests** (named after PRD ids): the incident replay, the
  graph-id switch with no reset (on the engine and on the iPhone core), the compaction
  guard, pending changes that survive compaction and a restart, `inspect` on both sides of a
  gap, and the snapshot heal. Each fix was checked by a mutation: with the fix removed, its
  tests fail.
- **The convergence fuzz test** (`crates/utopia-sync/tests/convergence.rs`). It uses three
  devices on disk, two relays × two graph ids, switches with and without a reset, restarts,
  compaction, edit bursts past the snapshot threshold, and outbox rows marked sent without a
  push. It asserts three things:
  1. no silent divergence: some device's `inspect` names each page that differs, and a
     device that holds changes it cannot apply says so in its own status;
  2. only the devices that `inspect` says must act run the repair;
  3. everything converges, and `doctor` is clean.

  `make check` runs 20 seeds. **`make sync-fuzz`** runs 2,000 seeds in release (about
  100 s). AGENTS.md asks for it on every change to the sync engine, `utopia-doc`, or the
  store's import and outbox path. A failure prints its seed and steps; rerun one with
  `UTOPIA_SYNC_FUZZ_SEED=<n>`. Runs are random (block ids, peers), so rerun a failing seed a
  few times.

## Rules this incident taught us

1. **An acknowledgement belongs to one destination.** On a new destination, compare what it
   holds with what this device has. Do not trust `sent_at`.
2. **Never ignore `ImportStatus.pending`.** Changes that cannot apply yet must be stored
   durably and reported. They are data, not noise (§2.3.7).
3. **A status must come from the store, not from the last round.** A quiet round must not
   turn a blocked page back into "Synced".
4. **A page that waits must read the destination from 0**, because its missing changes can
   arrive inside a snapshot that is behind the cursor.
5. **To resend, use a new change id.** Relays dedupe on change ids.

## Still open

- The iPhone does not sync when the app comes back to the foreground or leaves the screen
  (`docs/ios-prd.md` AC-9.3).
- There are no diagnostics on iOS: no "Copy diagnostics" row and no sync log on the phone.
- The desktop dot is red for a blocked page, while its status text and the iPhone use the
  warning colour.
- A destination change on the desktop sends the graph twice, once: by
  `prepare_sync_destination` and by the first-contact comparison. Imports are idempotent, so
  this costs only bandwidth.
- The relay contract test `snapshots_compact_the_log_and_bootstrap_new_devices` against
  `wrangler dev` is flaky. It failed once on #100 and passed on a rerun with no code change.
