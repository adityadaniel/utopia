# Self-hosting the Utopia relay on Cloudflare Workers

`relay-cloudflare/` is a second implementation of the relay `utopia-syncd` serves (PRD
§12.7 option 1, §12.8). It speaks the **same protocol** — the same routes, the same JSON
shapes, the same zstd request bodies, the same `{"page_id","head_seq"}` realtime notices —
so a device does not know which one it is talking to. On the Utopia side there is exactly
one change: the **Relay URL** in Settings → Sync (or `--url` for `utopia sync setup`).

Nothing in this repository deploys it. Everything here is local: the TypeScript tests and
the scripts below run real `workerd` through `wrangler dev --local`, with a simulated
R2 bucket and Durable Object storage. Deploying, creating the bucket and setting the token
are steps you run against your own account, listed below.

## Architecture

```
Utopia (Mac)  ──HTTPS/WebSocket──▶  Worker (routing, auth, validation, zstd)
                                         │
                                         ├─ Durable Object (SQLite storage) per graph ─ one graph's page logs,
                                         │                                              snapshots, revision, instance id,
                                         │                                              attachment index
                                         └─ R2 bucket ───────────────────────────────── attachment bytes
```

- **One Worker** (`src/index.ts`, logic in `src/relay.ts`) routes, authenticates, validates
  ids and hashes, enforces the 64 MiB body limit and decodes zstd before a graph is touched.
  The bearer token stops at the Worker: it is not passed to the Durable Object or stored.
  Attachment uploads are streamed through to R2, never held whole in memory (a Worker
  isolate has 128 MB).
- **One SQLite-backed Durable Object per graph** (`src/graph.ts`, class `GraphObject`,
  `idFromName(gid)`): the per-page append-only update log, the snapshot per page, the graph
  revision, the relay's instance id, and an index of the attachments. This is
  `utopia_syncd::Db` ported rule for rule — dedupe by change id before quota, monotonic
  per-page `seq`, snapshot truncation, the conditional manifest.
- **R2** holds attachment bytes under `graphs/{gid}/blobs/{hash}`. The SQLite side keeps
  the hash, size and received-at row so quota and write-once work without reading R2.
- **The WebSocket Hibernation API** carries the realtime notices
  (`acceptWebSocket`/`webSocketMessage`). A socket with no traffic costs no Durable Object
  duration (see [Cost](#cost)).
- The relay never parses payloads. With E2E encryption on it sees ciphertext and an opaque
  attachment hash (PRD §11.7, §15.6), which the e2e script below checks against wrangler's
  own state files.

Why not `utopia-syncd` everywhere? `utopia-syncd` is a binary you have to run on a machine
that is always on and reachable. A Worker is reachable from anywhere without a tunnel, and
the relay keeps no state you have to back up by hand.

## Prerequisites

- Node.js 20+ and npm.
- A Cloudflare account with the **Workers Paid** plan ($5/month minimum) for a deployment:
  SQLite-backed Durable Objects are on the Free plan too, but the plan's daily limits are
  small and the paid plan is what the cost section assumes. Local development needs no
  account.
- `wrangler` 4.x from this directory's `package.json` (`npx wrangler …`).
- Optional: an R2 bucket. R2 has a free tier (10 GB-month, 1M Class A and 10M Class B
  operations per month), but the bucket must be created explicitly.

```bash
cd relay-cloudflare
npm install            # dependencies live only in this directory
npm test               # vitest in real workerd: Durable Objects, R2, WebSockets
npm run typecheck
```

## Configuration (`wrangler.jsonc`)

The repository uses `wrangler.jsonc`; the same fields in `wrangler.toml` (should you prefer
TOML) are named identically.

```jsonc
{
  "name": "utopia-relay",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-15",
  "compatibility_flags": ["nodejs_compat"],
  "durable_objects": {
    "bindings": [{ "name": "GRAPH", "class_name": "GraphObject" }]
  },
  "migrations": [
    { "tag": "v1", "new_sqlite_classes": ["GraphObject"] }
  ],
  "r2_buckets": [
    {
      "binding": "BLOBS",
      "bucket_name": "utopia-relay-blobs",
      "preview_bucket_name": "utopia-relay-blobs"
    }
  ],
  "vars": { "QUOTA_BYTES": "1073741824" }
}
```

- **Durable Object binding + migration.** `new_sqlite_classes` is required: the relay uses
  the SQLite storage API, and a class that is created through a key-value migration cannot
  be moved to it later. The migration tag `v1` is already applied once the Worker exists on
  your account; adding a class or changing its storage backend is a new tag, not an edit.
- **R2 binding.** `bucket_name` is the bucket you create below. `preview_bucket_name` is
  only for `wrangler dev --remote`; `--local` simulates the bucket on disk.
- **`RELAY_TOKENS` is not in `vars`, on purpose.** It is a secret (below), and a var of the
  same name would be deployed in plain text next to it. Local development reads it from
  `relay-cloudflare/.dev.vars` (gitignored; copy `.dev.vars.example`) or from
  `--var RELAY_TOKENS:<token>`; the tests set it in `vitest.config.ts`.
- **`QUOTA_BYTES`** is an ordinary var, not a secret, and defaults to 1 GiB per graph, the
  same as `utopia-syncd`'s default. It counts the update log plus snapshots plus attachment
  sizes.

### Create the R2 bucket

Once per account:

```bash
npx wrangler r2 bucket create utopia-relay-blobs
```

If you rename the bucket, change `bucket_name` to match. **Do not put an R2 lifecycle rule on
objects under `graphs/`.** Deleting attachments from a graph does not exist yet (see
[Differences](#deliberate-differences-from-utopia-syncd)), and an expiry rule cannot tell an
abandoned attachment from one a live block still references: it would delete usable
attachments and leave their index rows behind. The only safe way to reclaim their space today
is to retire a whole graph — every device stops syncing it, its Durable Object is deleted — and
then remove that graph's prefix (`graphs/<graph id>/`), not objects inside a live one. If an
object is lost anyway, a device that still holds the attachment puts it back on its next
upload; nothing else can, and until then it is a 404.

### Set the relay token

The token is the bearer token every client sends and the relay's only notion of identity:
any accepted token can read and write **every graph** on that relay, exactly as with
`UTOPIA_SYNCD_TOKENS`. Generate a long random one and treat it like a password:

```bash
openssl rand -hex 32                       # the token
npx wrangler secret put RELAY_TOKENS       # paste it at the prompt (or pipe it in)
```

`RELAY_TOKENS` is a comma-separated list, so a rotation can run two tokens at once:
`<new>,<old>` until every device has the new one. The secret's value is never printed by
wrangler and never reaches the graph.

Set it **before the first deploy**: until the secret exists, the deployed relay answers
401 to everything (an unset `RELAY_TOKENS` refuses everyone, see below). `wrangler.jsonc`
has no `RELAY_TOKENS` var to shadow it, so the secret is the only value a deployment sees.
For `wrangler dev`, put a throwaway token in `relay-cloudflare/.dev.vars` instead
(`cp .dev.vars.example .dev.vars`); that file is gitignored and is never deployed.

Deliberate difference from `utopia-syncd`: an **empty token list means refuse everyone**,
not "auth off". A relay whose secret failed to load must not silently accept the internet.
Auth is off only when `ALLOW_ANONYMOUS=1` is set explicitly, which is a local-development
knob and must never be in a deploy.

### Optional: the quota

`QUOTA_BYTES` is not a secret. Change it in `wrangler.jsonc` and deploy again:

```jsonc
"vars": { "QUOTA_BYTES": "2147483648" }   // 2 GiB per graph
```

Do not also set it with `wrangler secret put`: a secret and a var of one name conflict
(depending on the wrangler version, a deploy warns about the shadowing or refuses).
A per-graph limit: a push or attachment over it is refused with 413 and the client keeps
its outbox (`quota_rejects_and_keeps_the_outbox`). Dedupe runs before the quota, so a batch
whose response was lost is never refused for its own bytes (§12.6). A missing, empty or
unparseable value is the 1 GiB default, never a zero-byte quota.

## Local test

```bash
cd relay-cloudflare
npm test                                   # vitest, real workerd, a few seconds
cp .dev.vars.example .dev.vars             # RELAY_TOKENS=dev-token, gitignored
npm run dev -- --persist-to .wrangler/state
curl -H 'Authorization: Bearer dev-token' http://127.0.0.1:8787/v1/graphs/g1/manifest
```

Instead of `.dev.vars`, `--var RELAY_TOKENS:dev-token` on the `npm run dev` line works too
(the scripts below do that). Wrangler keeps the local relay's storage in `relay-cloudflare/.wrangler/state` (SQLite
files for each Durable Object, R2 objects, the local trace store). The scripts below write
their own state under `target/` instead: the first gives every run a fresh directory
(`UTOPIA_RELAY_STATE_DIR` names another), so two runs never share one.

`npm test` covers the protocol, quota, blobs, realtime and the chunked large-payload path;
`npm run typecheck` proves the types. Neither needs an account or the network.

Three scripts in `scripts/` run **the real Utopia client** against the Worker this way:

```bash
relay-cloudflare/scripts/test-against-rust-client.sh   # the relay contract + differential test
relay-cloudflare/scripts/e2e-two-instances.sh          # two daemons, E2E on, MCP edits
relay-cloudflare/scripts/measure-requests.sh           # Worker requests/min, idle vs active
```

`test-against-rust-client.sh` starts `wrangler dev --local` on a free port, sets
`RELAY_TOKENS=test-token`, waits for `/healthz`, then runs
`cargo test -p utopia-syncd --test cloudflare --test differential` twice: the client
scenarios and the differential script with a 1 GiB quota, then the quota scenarios with
`QUOTA_BYTES=4096` on a second wrangler instance. It always stops wrangler. It sets
`UTOPIA_RELAY_REQUIRED=1`, so a missing or unreachable Worker fails rather than skips;
without `UTOPIA_RELAY_URL` (and that switch) the Rust tests print a skip and pass, so
`make check` needs no Node. `make relay-check` runs `tsc`, `npm test` and this script in
one go, and `.github/workflows/relay.yml` runs the same steps in CI on every pull request
that touches a relay, the wire format or the sync client.

The scenarios are the relay contract (`crates/utopia-syncd/tests/contract/mod.rs`), which
`tests/http.rs` also runs against `utopia-syncd` on every `cargo test`, so both relays meet
the same list. `tests/differential.rs` sends one script of raw requests to both and
compares the answers; the differences it finds are recorded there and in
`relay-cloudflare/README.md`.

What those tests prove, against the Worker rather than against the Rust relay:

| Test | What it pins |
|---|---|
| `two_machines_converge_over_http` | two stores converge through the Worker, tags and dailies included |
| `manifest_is_conditional_and_pushes_are_idempotent` | `If-None-Match` 304, and a re-pushed id answers `duplicate: 1` |
| `auth_is_enforced` | 401 on a wrong token, `NeedsAuth`, the outbox kept; the right token syncs |
| `ac_12_3_a_change_id_repeated_in_one_batch_is_a_duplicate` | one id twice in one batch: accepted once, stored once |
| `ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota` | dedupe before quota (small-quota run) |
| `ac_12_6_a_lost_ack_is_replayed_not_rejected_by_quota` | a lost response is re-pushed under a quota with room for one copy; the relay stored and charged the batch once (small-quota run) |
| `quota_rejects_and_keeps_the_outbox` | 413 and the outbox survives (small-quota run) |
| `snapshots_compact_the_log_and_bootstrap_new_devices` | the log truncates (`tail_bytes`), a new device bootstraps the latest text |
| `e2e_encrypted_sync_relay_sees_only_ciphertext` | a wrong passphrase fails; a canary string is on the devices and in none of wrangler's state files |
| `realtime_notice_on_push` | the client's `realtime::Listener` receives a notice from the Worker's hibernating socket |
| `attachments_sync_encrypted_and_verified` | an encrypted attachment uploads, downloads, verifies; the canary is not in the state |
| `tampered_blob_is_rejected` | a forged object is never stored on the downloading device |

`e2e-two-instances.sh` is the whole product path: two scratch graphs share one graph id
through one local relay with E2E on, two `utopia sync --continuous` daemons own them, and
every edit goes through `utopia mcp` (the Remote path into the daemon). It checks an edit
in each direction, a concurrent `update_block` on one block from both sides (the merged
text is printed; both inserts survive), that a change reaches the other side in a few
hundred milliseconds while its idle poll is 60 s (so a WebSocket notice, not a poll), and
that none of six canary strings appears anywhere in the relay's state. Attachments have no
CLI/MCP upload path, so upload and download are covered by
`attachments_sync_encrypted_and_verified` above, not by the e2e script.

## Deploy

The checklist, in order, the first time:

1. `npx wrangler r2 bucket create utopia-relay-blobs` ([above](#create-the-r2-bucket)).
2. `npx wrangler secret put RELAY_TOKENS` with a token from `openssl rand -hex 32`
   ([above](#set-the-relay-token)). It is a secret only; `wrangler.jsonc` has no var of
   that name.
3. Optionally, change `vars.QUOTA_BYTES` in `wrangler.jsonc` ([above](#optional-the-quota)).
   It is a var, not a secret.
4. Make sure `ALLOW_ANONYMOUS` is set nowhere a deploy reads it (`wrangler.jsonc`, a
   secret). It belongs only in a local `.dev.vars`, and `.dev.vars` is never deployed.
5. Deploy:

```bash
cd relay-cloudflare
npx wrangler deploy
```

6. Check it: `curl https://utopia-relay.<your-subdomain>.workers.dev/healthz` answers `ok`,
   and a manifest request without the token answers 401.

The first deploy applies the `v1` migration. The output prints the Worker's URL, usually
`https://utopia-relay.<your-subdomain>.workers.dev`. A custom domain works too (Routes →
Custom Domains); the client only needs an HTTPS origin.

Then, on each device, either:

- **Settings → Sync**: choose **Relay**, paste the origin (`https://utopia-relay.…`, no
  `/v1/…` path, no trailing slash is fine), paste the token, optionally the E2E
  passphrase, **Test connection**, **Apply**; or
- **headless**:

```bash
export UTOPIA_SYNC_TOKEN=<the token>          # or pipe it with --token-stdin
export UTOPIA_SYNC_PASSPHRASE=<passphrase>    # the same on every device; omit to stay plaintext
utopia sync setup --graph ~/Documents/Utopia/My.utopia \
  --mode relay --url https://utopia-relay.<your-subdomain>.workers.dev
utopia sync --continuous --graph ~/Documents/Utopia/My.utopia
```

The token and passphrase only ever reach the Keychain (or, for a run, the environment);
they are never written into the graph or the settings file (§15.6). Every device that
should share data needs **the same graph id**, which `sync setup --graph-id` or Settings →
Sync → "Connect another device" gives you.

## Deliberate differences from `utopia-syncd`

Protocol-visible differences: none. These are behavioural, and each is a deliberate choice
(details and tests in `relay-cloudflare/README.md`):

1. **An empty token list refuses everyone** unless `ALLOW_ANONYMOUS=1`; `utopia-syncd`
   treats an empty list as local-development auth off.
2. **No `graph_id` columns.** A Durable Object *is* the graph; its storage is private.
3. **Attachments live in R2**, with an index row in SQLite for quota and write-once.
4. **Payloads are chunked** into 512 KiB rows because a SQLite row cannot exceed ~2 MB. Invisible
   to the protocol; a 5 MiB snapshot round-trips.
5. **Snapshots are not quota-checked** (matching `Db::put_snapshot`), but their bytes count
   afterwards, so a client can always compact.
6. **No hash check on blobs** (matching the Rust relay): blobs may be ciphertext, and the
   client verifies after decrypting.
7. **The token is neither forwarded nor stored.** `utopia-syncd` writes the caller's token in
   plain text to `graph.owner_id` (never read); this relay checks it in the Worker and
   passes it nowhere: not to the Durable Object, not into storage. As there, tokens are per
   relay, not per graph.
8. **Attachment uploads stream** from the client through the Worker and the Durable Object
   into R2 when the request has a `Content-Length` (the Rust client always sends one), so no
   full copy of a 64 MiB attachment is held in memory; a chunked upload is held once in the
   Worker. `utopia-syncd` reads each body into memory.
9. **No attachment garbage collection.** Removing a page or block does not delete its R2
   object, and there is no admin endpoint to. `utopia-syncd` has no GC either. An R2 lifecycle
   rule is not a substitute — it is not reference-aware, so it would delete attachments that
   live blocks still use — and the only safe deletion today is a retired graph's prefix (see
   [Create the R2 bucket](#create-the-r2-bucket)).

Also unchanged from `utopia-syncd`, worth knowing: the 64 MiB body limit applies to the
**compressed** bytes and the decompressed size is not capped. Capping it would be a protocol
change, so it is left for a later slice to decide for both relays together.

## Cost

The numbers below are Cloudflare's published plans and this directory's measured request
rates. They are an estimate, not a bill.

### Included in Workers Paid ($5/month minimum)

| Product | Included | Overage |
|---|---|---|
| Worker requests | 10 million / month | $0.30 / million |
| Worker CPU | 30 million CPU-ms / month | $0.02 / million CPU-ms |
| Durable Object requests | 1 million / month | $0.15 / million |
| Durable Object duration | 400,000 GB-s / month | $12.50 / million GB-s |
| DO SQLite storage | 5 GB-month | $0.20 / GB-month |
| DO SQLite rows read | 25 billion / month | $0.001 / million |
| DO SQLite rows written | 50 million / month | $1.00 / million |
| R2 storage | 10 GB-month free tier | $0.015 / GB-month |
| R2 Class A / Class B ops | 1M / 10M per month (free tier) | $4.50 / $0.36 per million |
| R2 egress | free | — |

Sources: [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/),
[Durable Objects pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/),
[R2 pricing](https://developers.cloudflare.com/r2/pricing/). Check them before relying on a
number: plans change.

### Measured request rates (one device)

`scripts/measure-requests.sh` runs one `utopia sync --continuous` daemon against a local
Worker with its realtime socket open, E2E on, and counts wrangler's own request log over
three-minute windows — idle (no edits), then active (one edit every 10 s, 18 edits).
`MEASURE_*` / `IDLE_SECONDS` / `ACTIVE_SECONDS` / `EDIT_SECONDS` change the windows.

Measured on 2026-11-21 with wrangler 4.166.0 (workerd), one graph, one daemon:

| Window | Route | Requests | Per minute |
|---|---|---:|---:|
| idle, 180 s | `manifest` GET (conditional, 304) | 4 | 1.3 |
| idle, 180 s | everything else | 0 | 0 |
| idle, 180 s | **total** | **4** | **1.3** |
| active, 184 s, 18 edits | `manifest` GET | 55 | 17.9 |
| active | `updates` POST | 18 | 5.9 |
| active | `updates` GET | 18 | 5.9 |
| active | everything else | 0 | 0 |
| active | **total** | **91** | **29.7** |

Two facts explain the shape of it:

- **An idle device with its socket open makes one request a minute**: the daemon still
  polls the conditional manifest on its slow cadence (60 s), and the 304 costs one Worker
  and one Durable Object request. The WebSocket connection itself is one request when it is
  opened (included in neither window above, because it happens at daemon start) and then
  nothing.
- **One edit costs about five requests**: three conditional manifest reads, one push
  (`updates` POST) and one pull (`updates` GET). The log shows the order: the round the
  local write nudges reads the manifest and pushes; the next round reads the manifest and
  pulls the result back; a quiet round then reads the manifest and gets a 304. E2E
  encryption does not change the count, only the bytes.

What it means for a month (30 days at these rates, before rounding):

| Devices | Worker requests / month | Durable Object requests / month | Share of the included 10M / 1M |
|---|---:|---:|---|
| 1 | ~86,000 | ~86,000 | 0.9% / 8.6% |
| 2 | ~172,000 | ~172,000 | 1.7% / 17% |
| 3 | ~258,000 | ~258,000 | 2.6% / 26% |

Assumptions: 30 days, ~1.3 idle polls a minute, and **200 edits a day per device** at five
requests each (6,000 edits → 30,000 requests per device per month, on top of ~56,000 idle
polls). All three devices together stay inside the Workers Paid inclusions and cost the
$5 minimum.

The binding allowance is the **1M Durable Object requests**, not the 10M Worker requests:
an idle device spends ~56k DO requests a month, so roughly 17 devices could idle inside the
included million before overage ($0.15/million). At 200 edits a day, the same million covers
about 11 devices. A device that reconnects often adds one request per WebSocket connection.
Storage is not close to binding: 5 GB-month of DO SQLite is years of text, and the R2 free
tier is 10 GB for attachments.

Notes that matter more than the arithmetic:

- **Hibernation is what keeps the idle socket free.** With the Hibernation API a socket
  that receives or sends nothing is not billed for Durable Object duration, and a request
  is billed only to create the connection. A non-hibernating WebSocket would bill duration
  for every second it is connected. Our client also sends nothing over the socket: it only
  receives notices.
- **WebSocket messages.** Per the Durable Objects pricing footnotes, "a request is needed to
  create a WebSocket connection. There is no charge for outgoing WebSocket messages, nor for
  incoming WebSocket protocol pings", and "for compute requests billing-only, a 20:1 ratio is
  applied to incoming WebSocket messages" (100 incoming messages = 5 requests). This client
  sends no WebSocket messages, so its socket costs one request per connection and nothing
  per notice. A future client that sends pings or acknowledgements would add DO requests at
  that 20:1 ratio. (Verified on the Durable Objects pricing page, footnote 2, on 2026-11-13;
  see the link above.)
- **Worker vs Durable Object requests.** Every client call is one Worker request (10M
  included) and, when it reaches a graph, one Durable Object request plus a subrequest. The
  Worker's own subrequests to the Durable Object are not billed as Worker requests.
- **Rows written.** An edit's push writes the log row, its payload chunk rows, the revision
  row and the page index update; compaction rewrites a snapshot. Tens of thousands of edits a
  month stay far inside 50M rows written. Snapshots (one per page per compaction) rewrite the
  page's state but replace rows rather than accumulate.
- **Storage.** 5 GB of DO SQLite plus the R2 free tier is thousands of pages of notes and
  attachments. The relay does not compact on its own: the compactor is a client, so a graph
  that is never synced by a new client keeps its log until one is.

## Verified where

- `npm test` in `relay-cloudflare/`: the TypeScript suite in real workerd.
- `scripts/test-against-rust-client.sh` (`make relay-check`, and
  `.github/workflows/relay.yml` in CI): the 12 contract scenarios over HTTP and WebSocket, a
  canary scan of wrangler's DO SQLite and R2 state, a small-quota run, and the raw
  differential script against `utopia-syncd`.
- `scripts/e2e-two-instances.sh`: two daemons, E2E encryption, MCP edits, a realtime notice
  against a 60 s poll, and a plaintext scan of the relay's state.
- Not verified here: a deployed Worker, a real R2 bucket, two separate Macs, and the
  Cloudflare dashboard. Those need an account and were deliberately left to you; the local
  runtime is the same workerd, but a deploy also involves the account's quota, the secret, the
  migration and DNS.
