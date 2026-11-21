# Utopia relay on Cloudflare Workers

A hosted relay for Utopia sync (PRD §12.7 item 1, §12.8), speaking the **same protocol**
as `crates/utopia-syncd`: Workers for routing and auth, one SQLite-backed Durable Object per
graph for the page logs, R2 for attachments, and the WebSocket Hibernation API for the
`{"page_id","head_seq"}` notices. No app code changes: a client points Settings → Sync at
this Worker's URL instead of at `utopia-syncd`.

The deployment and operations guide — secrets, `wrangler deploy`, the R2 bucket and its
retention limits, quotas, what it costs — is `docs/self-host-relay-cloudflare.md`. This README
covers the code and its tests.

Nothing in this directory deploys anything by itself. Everything here is local:
`npm test` runs in workerd through `@cloudflare/vitest-pool-workers`, and `npm run dev`
serves the Worker on 127.0.0.1 with a simulated R2 bucket and Durable Object storage.

## Commands

```bash
npm install          # once; dependencies live only in this directory
npm test             # vitest, in the real workerd runtime (Durable Objects, R2, WebSockets)
npm run typecheck    # npx tsc --noEmit
npm run dev          # wrangler dev --local on http://localhost:8787
```

For local development with auth (the default refuses every request until a token is set):

```bash
npm run dev -- --var RELAY_TOKENS:dev-token --persist-to ../target/cf-relay-dev
curl -H 'Authorization: Bearer dev-token' http://127.0.0.1:8787/v1/graphs/g1/manifest
```

`ALLOW_ANONYMOUS=1` turns auth off for local development (see "Differences from
`utopia-syncd`" below).

Three scripts run the real Utopia client against a local Worker (nothing is deployed, and
they stop wrangler when they finish):

```bash
scripts/test-against-rust-client.sh   # the Rust sync client: 12 scenarios, plus a quota run
scripts/e2e-two-instances.sh          # two daemons, E2E on, edits through utopia mcp
scripts/measure-requests.sh           # Worker requests/min for one device, idle vs active
```

The deployment and operations guide is `../docs/self-host-relay-cloudflare.md`.

## Layout

| File | Responsibility |
|---|---|
| `src/index.ts` | The Worker entry point: the default `fetch` handler and the exported Durable Object class, and nothing else — `wrangler` turns every other value export into a binding, so the logic lives in `relay.ts`. |
| `src/relay.ts` | Routing, auth (`RELAY_TOKENS`, `ALLOW_ANONYMOUS`), id/hash validation, body limit, zstd decoding, then forwarding to the graph's object. |
| `src/graph.ts` | `GraphObject`, one per graph: the SQLite storage API schema and every rule of `utopia_syncd::Db` (push/pull/snapshot/manifest/blobs/notices), on the Hibernation API. |
| `src/protocol.ts` | The v1 wire types of `crates/utopia-proto`, base64, and the 400s serde would produce. |
| `test/` | Protocol tests (vitest, real workerd). `helpers.ts` is the client: every request is a zstd body through the Worker, as the app sends them. |
| `wrangler.jsonc` | The DO binding + `new_sqlite_classes` migration, the R2 binding, `RELAY_TOKENS` and `QUOTA_BYTES`. |
| `scripts/` | The Rust-client, two-instance and request-count scripts above (bash; they use the shared `CARGO_TARGET_DIR` and a built `utopia` binary). |

## Routes

Exactly `utopia_syncd::router()`'s. Unknown paths are 404, a known path with the wrong
method is 405, errors are plain text with the Rust status codes (401, 400, 413, 500).

| Route | Behaviour |
|---|---|
| `GET /v1/graphs/{gid}/manifest` | `rev`, `protocol_version: 1`, pages, `instance`; `ETag: "<rev>"`, 304 on a matching `If-None-Match`. |
| `POST /v1/graphs/{gid}/pages/{pid}/updates` | `{accepted, head_seq, duplicate}`; dedupes by change id inside one transaction, before quota; accepts a change id repeated in one batch as a duplicate. |
| `GET /v1/graphs/{gid}/pages/{pid}/updates?since=N` | Snapshot (base64) only when `since < snapshot.seq`, then updates with `seq > since`. |
| `POST /v1/graphs/{gid}/pages/{pid}/snapshot` | 204; 400 `snapshot covers unknown updates`; a snapshot not newer than the stored one is a silent no-op. Truncates the covered log rows. |
| `GET /v1/graphs/{gid}/events` | WebSocket upgrade; notices over `acceptWebSocket` (Hibernation API). Incoming frames are ignored. |
| `PUT|GET /v1/graphs/{gid}/blobs/{hash}` | Write-once, 201 or 200; bytes in R2, 404 when absent. |
| `GET /healthz` | `ok`, unauthenticated. |

`gid`/`pid` must be 1–128 characters of `[A-Za-z0-9_-]`; `hash` must be 64 lowercase hex.
Both are checked before the Durable Object is created or addressed. Bodies may carry
`Content-Encoding: zstd` (every client POST does); the limit is 64 MiB plus 64 KiB,
enforced while the body arrives.

## Differences from `utopia-syncd`, and why

1. **An empty token list means "refuse everyone", not "auth off".** `utopia-syncd` treats
   `UTOPIA_SYNCD_TOKENS` unset as local-development auth off. Here a relay whose
   `RELAY_TOKENS` secret failed to load must not silently accept the internet, so auth is
   off only when `ALLOW_ANONYMOUS=1` is set explicitly (in `wrangler dev`, never in a
   deploy). Anonymous callers are the owner `anonymous`.
2. **No `graph_id` columns.** A Durable Object *is* the graph, named `idFromName(gid)`, and
   its storage is private to it. The same tables, minus the graph key.
3. **Attachments live in R2** under `graphs/{gid}/blobs/{hash}`. The SQLite side keeps an
   index row (`hash`, `size`, `received_at`) for quota and write-once, so `graph_bytes`
   counts bytes it can see without reading R2. A crash between the R2 write and the index
   insert leaves an unreferenced object, never an index row without bytes. Uploads are
   serialised: an R2 `await` leaves the object's input gate open, so two concurrent uploads
   could otherwise both pass the existence and quota checks. If an object is removed behind
   its index row (an R2 lifecycle rule, a bucket restore) a re-upload puts the bytes back,
   still answered 200 because nothing new is indexed.
4. **Payloads are chunked.** A SQLite row (key plus value) may not exceed about 2 MB, so
   any update or snapshot payload is stored in `payload_chunk` rows of 512 KiB. The
   chunking is invisible to the protocol; a 5 MiB encrypted snapshot round-trips (tested).
5. **Snapshots are not quota-checked**, matching `utopia_syncd::Db::put_snapshot`, but the
   bytes they leave behind are counted afterwards — so a client can always compact.
6. **No hash check on blobs**, matching the Rust relay: blobs may be ciphertext and the
   client verifies after decrypting (`tampered_blob_is_rejected`). Storing bytes under a
   hash they do not match is allowed and tested.
7. **Identity is the token**, as in Rust (`client_id` is the part of the change id before
   `:`). Tokens are per relay, not per graph: any accepted token can read and write any
   graph, exactly as with `UTOPIA_SYNCD_TOKENS`.

Also unchanged from Rust, and worth knowing: the body limit applies to the **compressed**
bytes, and the decompressed size is not capped (a small zstd body can expand far beyond
64 MiB, and a Durable Object has 128 MB of memory). `utopia-syncd`'s `zstd::decode_all` has
the same property; capping it here would be a protocol change, so it is left for a later
slice to decide together with the Rust relay.

## Tests

`npm test` runs in workerd, not in Node with mocks: real Durable Objects on the SQLite
storage API, a real R2 binding, real WebSockets. Ported from
`crates/utopia-syncd/tests/http.rs` and `utopia_syncd`'s own tests:

- `test/protocol.test.ts` — `manifest_is_conditional_and_pushes_are_idempotent`,
  `ac_12_3_a_change_id_repeated_in_one_batch_is_a_duplicate`,
  `snapshots_compact_the_log_and_bootstrap_new_devices`, the snapshot refusals, the
  instance id, 404/405/HEAD, id and hash validation, malformed JSON/zstd/shape, zstd bodies.
- `test/quota.test.ts` — `ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota`,
  `quota_rejects_and_keeps_outbox` (the relay side),
  `ac_12_6_a_lost_ack_is_replayed_not_rejected_by_quota`, the accounting of log,
  snapshot and attachment bytes, and two concurrent uploads against one quota.
- `test/blob.test.ts` — write-once (including two concurrent uploads of one hash), R2
  location, 404, repair of an object removed behind its index row, per-graph dedupe, quota,
  no hash check.
- `test/realtime.test.ts` — `realtime_notice_on_push`, one notice per accepted push to every
  subscriber, none for a duplicate, sockets registered with the Hibernation API, incoming
  messages ignored.
- `test/large.test.ts` — a 5 MiB snapshot and a 1.5 MiB update round-trip, with no chunk
  row anywhere near the 2 MB limit.
- `test/unit.test.ts` — the decisions made before storage: auth policy (including the two
  `RELAY_TOKENS` empty-list cases), id/hash validation, the body limit, routing, base64
  across chunk boundaries, and the 400s for malformed push requests.

Two notes on the harness:

- **Storage is not rolled back between tests** in this runner, so every test makes its own
  graph id (`new Graph()`): its Durable Object starts empty whatever another test wrote.
- **Test bodies are real zstd frames** made with `node:zlib` (Cloudflare's encoder refuses
  inputs of 64 KiB or more, so `helpers.ts` emits concatenated 32 KiB frames — a valid
  stream). The 5 MiB path and the "a body only decodes when decompressed" check pin that
  the Worker really decompresses rather than passing bytes through.
