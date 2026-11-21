//! One Durable Object per graph (PRD §12.8), on the SQLite storage API.
//!
//! This is `utopia_syncd::Db` ported: the same per-page append-only log, the same
//! dedupe/quota/seq/snapshot/manifest rules, and the same `{"page_id","head_seq"}`
//! realtime notice. The relay never parses payloads, so it needs no CRDT and works on
//! ciphertext (§11.7).
//!
//! Two things differ, both forced by the platform:
//! - There are no `graph_id` columns: a Durable Object *is* the graph, named by its id
//!   (`idFromName(gid)`), and its storage is private to it. Attachment bytes live in R2
//!   under `graphs/{gid}/blobs/{hash}`, and the SQLite side keeps only the index row
//!   (`hash`, `size`, `received_at`) that quota and write-once need.
//! - A SQLite row (keys plus values) may not exceed ~2 MB, so payloads are stored in
//!   `payload_chunk` rows of 512 KiB however small they are. A update or snapshot is
//!   written and read as one payload; the chunking is invisible to the protocol.

import { DurableObject } from "cloudflare:workers";

import {
  BadRequest,
  PROTOCOL_VERSION,
  b64decode,
  b64encode,
  type Manifest,
  type ManifestPage,
  type PullResponse,
  type PushRequest,
  type PushResponse,
  type SnapshotRequest,
  type UpdateIn,
} from "./protocol";

/** 512 KiB: comfortably under the ~2 MB row limit, few rows for a small update. */
const CHUNK_BYTES = 512 * 1024;

/** `utopia-syncd`'s default (1 GiB); the Worker passes `QUOTA_BYTES` per request. */
export const DEFAULT_QUOTA_BYTES = 1 << 30;

/**
 * The internal header the Worker uses to tell the object an attachment's size, so the quota
 * is checked before a byte of a streamed body is read (`GraphObject.putBlob`).
 */
export const BLOB_SIZE_HEADER = "x-utopia-blob-size";

const SCHEMA = [
  // The Rust relay's `graph` table also has `owner_id`, the caller's bearer token in plain
  // text, which nothing reads. This relay never receives the token, so it has no such
  // column (relay-cloudflare/README.md, "Differences").
  `CREATE TABLE IF NOT EXISTS graph (
     id INTEGER PRIMARY KEY CHECK (id = 1),
     created_at INTEGER NOT NULL
   )`,
  `CREATE TABLE IF NOT EXISTS page_log (
     page_id TEXT NOT NULL,
     seq INTEGER NOT NULL,
     change_id TEXT NOT NULL UNIQUE,
     client_id TEXT,
     client_seq INTEGER,
     frontier TEXT,
     payload_id TEXT NOT NULL,
     payload_len INTEGER NOT NULL,
     received_at INTEGER NOT NULL,
     PRIMARY KEY (page_id, seq)
   )`,
  `CREATE TABLE IF NOT EXISTS page_snapshot (
     page_id TEXT PRIMARY KEY,
     seq INTEGER NOT NULL,
     frontier TEXT,
     payload_id TEXT NOT NULL,
     payload_len INTEGER NOT NULL
   )`,
  `CREATE TABLE IF NOT EXISTS payload_chunk (
     payload_id TEXT NOT NULL,
     idx INTEGER NOT NULL,
     bytes BLOB NOT NULL,
     PRIMARY KEY (payload_id, idx)
   )`,
  `CREATE TABLE IF NOT EXISTS graph_rev (
     id INTEGER PRIMARY KEY CHECK (id = 1),
     rev INTEGER NOT NULL
   )`,
  `CREATE TABLE IF NOT EXISTS relay_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)`,
  // The index of what R2 holds; `size` is what quota charges (the Rust relay sums
  // `length(payload)` the same way).
  `CREATE TABLE IF NOT EXISTS blob (
     hash TEXT PRIMARY KEY,
     size INTEGER NOT NULL,
     received_at INTEGER NOT NULL
   )`,
];

/** The caller's quota was exceeded: 413 `quota exceeded`, nothing stored (§12.6). */
export class QuotaExceeded extends Error {
  constructor() {
    super("quota exceeded");
  }
}

function nowMs(): number {
  return Date.now();
}

function randomHex(bytes: number): string {
  const buf = new Uint8Array(bytes);
  crypto.getRandomValues(buf);
  return Array.from(buf, (b) => b.toString(16).padStart(2, "0")).join("");
}

function initSchema(sql: SqlStorage): void {
  for (const statement of SCHEMA) sql.exec(statement);
  // One id per relay store, kept for the life of this object's storage: a client tells a
  // restarted log from the one it pulled by it (§12.6). Equivalent to the Rust
  // `INSERT OR IGNORE ... randomblob(16)`.
  const existing = sql
    .exec<{ value: string }>(
      "SELECT value FROM relay_meta WHERE key = 'instance'",
    )
    .toArray();
  if (existing.length === 0) {
    sql.exec(
      "INSERT OR IGNORE INTO relay_meta(key, value) VALUES ('instance', ?)",
      randomHex(16),
    );
  }
}

function instanceId(sql: SqlStorage): string {
  const row = sql
    .exec<{ value: string }>(
      "SELECT value FROM relay_meta WHERE key = 'instance'",
    )
    .toArray()[0];
  return row?.value ?? "";
}

function revOf(sql: SqlStorage): number {
  const row = sql
    .exec<{ rev: number }>("SELECT rev FROM graph_rev WHERE id = 1")
    .toArray()[0];
  return row?.rev ?? 0;
}

function bumpRev(sql: SqlStorage): void {
  sql.exec(
    `INSERT INTO graph_rev(id, rev) VALUES (1, 1)
     ON CONFLICT(id) DO UPDATE SET rev = rev + 1`,
  );
}

/** `max(log max seq, snapshot seq)`: the next seq this page would take. */
function headOf(sql: SqlStorage, pid: string): number {
  const row = sql
    .exec<{ head: number }>(
      `SELECT max(ifnull((SELECT max(seq) FROM page_log WHERE page_id = ?1), 0),
                  ifnull((SELECT seq FROM page_snapshot WHERE page_id = ?1), 0)) AS head`,
      pid,
    )
    .one();
  return row.head;
}

/** Bytes stored for this graph: logs + snapshots + attachment sizes, for quota. */
function graphBytes(sql: SqlStorage): number {
  const row = sql
    .exec<{ bytes: number }>(
      `SELECT ifnull((SELECT sum(payload_len) FROM page_log), 0)
            + ifnull((SELECT sum(payload_len) FROM page_snapshot), 0)
            + ifnull((SELECT sum(size) FROM blob), 0) AS bytes`,
    )
    .one();
  return row.bytes;
}

function chunkPayload(sql: SqlStorage, bytes: Uint8Array): string {
  const id = crypto.randomUUID();
  const chunks = Math.max(1, Math.ceil(bytes.length / CHUNK_BYTES));
  for (let idx = 0; idx < chunks; idx++) {
    sql.exec(
      "INSERT INTO payload_chunk(payload_id, idx, bytes) VALUES (?, ?, ?)",
      id,
      idx,
      bytes.subarray(idx * CHUNK_BYTES, (idx + 1) * CHUNK_BYTES),
    );
  }
  return id;
}

function readPayload(sql: SqlStorage, id: string, len: number): Uint8Array {
  const rows = sql
    .exec<{ bytes: ArrayBuffer }>(
      "SELECT bytes FROM payload_chunk WHERE payload_id = ? ORDER BY idx",
      id,
    )
    .toArray();
  const out = new Uint8Array(len);
  let offset = 0;
  for (const row of rows) {
    const chunk = new Uint8Array(row.bytes);
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

function deletePayload(sql: SqlStorage, id: string): void {
  sql.exec("DELETE FROM payload_chunk WHERE payload_id = ?", id);
}

function json(body: unknown, status = 200, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

function text(status: number, body: string, headers: Record<string, string> = {}): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", ...headers },
  });
}

function quotaFrom(raw: string | null): number {
  if (raw === null || raw.trim() === "") return DEFAULT_QUOTA_BYTES;
  const n = Number(raw);
  return Number.isSafeInteger(n) && n >= 0 ? n : DEFAULT_QUOTA_BYTES;
}

/**
 * The Durable Object's own body parsing. The Worker has already validated the JSON and the
 * base64, so this only matters for the direct interface the tests use — and keeps a bad
 * body a 400 there too, as `serde` would answer in Rust.
 */
async function jsonBody<T>(request: Request): Promise<T> {
  try {
    return (await request.json()) as T;
  } catch (e) {
    throw new BadRequest(e instanceof Error ? e.message : String(e));
  }
}

/**
 * An attachment's size as the Worker declared it (`BLOB_SIZE_HEADER`), or the body's own
 * `Content-Length` on the direct interface; `null` when neither is there.
 */
function declaredBlobSize(headers: Headers): number | null {
  const raw = headers.get(BLOB_SIZE_HEADER) ?? headers.get("content-length");
  if (raw === null) return null;
  if (!/^\d+$/.test(raw) || !Number.isSafeInteger(Number(raw))) {
    throw new BadRequest(`invalid attachment size ${JSON.stringify(raw)}`);
  }
  return Number(raw);
}

/** An attachment body: a stream of a declared size, bytes already read, or nothing. */
type BlobBody = ReadableStream<Uint8Array> | Uint8Array | null;

/**
 * Read a body that will not be stored, without holding it: the client sent all of it before
 * reading the answer (`ureq … send_bytes`), as the Rust relay reads every body in full.
 */
async function discard(body: BlobBody): Promise<void> {
  if (body === null || body instanceof Uint8Array) return;
  try {
    await body.pipeTo(new WritableStream());
  } catch {
    // The client went away; nothing was going to be stored anyway.
  }
}

/**
 * Copy `body` into `writable`, one chunk at a time, refusing a body that is not exactly
 * `size` bytes before the stream sees a byte too many (or closes a byte short). On any
 * failure both ends are released, so nothing is left waiting and the R2 put fails.
 */
async function pump(
  body: ReadableStream<Uint8Array>,
  writable: WritableStream<Uint8Array>,
  size: number,
): Promise<void> {
  const reader = body.getReader();
  const writer = writable.getWriter();
  let seen = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      seen += value.length;
      if (seen > size) break;
      await writer.write(value);
    }
    // A body that is not the size it declared is the client's error: axum answers a body
    // it cannot read with 400 too.
    if (seen !== size) {
      throw new BadRequest(`attachment body is not the ${size} bytes it declared`);
    }
    await writer.close();
  } catch (e) {
    await writer.abort(e).catch(() => undefined);
    await reader.cancel(e).catch(() => undefined);
    throw e;
  }
}

/** Close codes `WebSocket.close` may send: 1005, 1006 and 1015 are reserved (RFC 6455 §7.4). */
function sendableCloseCode(code: number): boolean {
  return (
    code === 1000 ||
    (code >= 1001 && code <= 1003) ||
    (code >= 1007 && code <= 1011) ||
    (code >= 3000 && code <= 4999)
  );
}

function decodePayload(payload: string): Uint8Array {
  try {
    return b64decode(payload);
  } catch {
    throw new BadRequest('bad body: "payload" is not base64');
  }
}

export interface Env {
  GRAPH: DurableObjectNamespace<GraphObject>;
  BLOBS: R2Bucket;
  RELAY_TOKENS?: string;
  ALLOW_ANONYMOUS?: string;
  QUOTA_BYTES?: string;
}

/**
 * The per-graph relay state. Its `fetch` is an internal interface (the Worker has already
 * authenticated and validated), which is also what the tests drive directly when they need
 * a quota other than the deployment's.
 */
export class GraphObject extends DurableObject<Env> {
  /**
   * Bytes of uploads that passed their quota check but are not indexed yet. Only in memory:
   * if this object is evicted mid-upload nothing was indexed either, so there is nothing to
   * reserve.
   */
  private inFlightBlobBytes = 0;
  /**
   * Tail of the upload queue: an R2 `await` opens this object's input gate, so two uploads
   * can otherwise interleave between check and insert (`putBlob`). The Rust relay serialises
   * every write on one SQLite writer the same way.
   */
  private uploads: Promise<unknown> = Promise.resolve();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // The schema and the instance id exist before any request runs.
    ctx.blockConcurrencyWhile(async () => {
      initSchema(ctx.storage.sql);
    });
  }

  override async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    try {
      switch (url.pathname) {
        case "/manifest": {
          const manifest = this.manifest();
          const etag = `"${manifest.rev}"`;
          if (request.headers.get("if-none-match") === etag) {
            return new Response(null, { status: 304, headers: { etag } });
          }
          return json(manifest, 200, { etag });
        }
        case "/push": {
          const pid = url.searchParams.get("pid") ?? "";
          const quota = quotaFrom(url.searchParams.get("quota"));
          const req = await jsonBody<PushRequest>(request);
          const resp = this.push(pid, req, quota);
          // Announce only what was accepted; a duplicate batch changes nothing (§12.3).
          if (resp.accepted > 0) this.notify(pid, resp.head_seq);
          return json(resp);
        }
        case "/pull": {
          const pid = url.searchParams.get("pid") ?? "";
          const since = Number(url.searchParams.get("since") ?? "0");
          return json(this.pull(pid, Number.isFinite(since) && since > 0 ? since : 0));
        }
        case "/snapshot": {
          const pid = url.searchParams.get("pid") ?? "";
          const req = await jsonBody<SnapshotRequest>(request);
          this.putSnapshot(pid, req);
          return new Response(null, { status: 204 });
        }
        case "/blob": {
          const hash = url.searchParams.get("hash") ?? "";
          if (request.method === "PUT") {
            const quota = quotaFrom(url.searchParams.get("quota"));
            const size = declaredBlobSize(request.headers);
            // One upload at a time: see `putBlob`. A body whose size nobody declared (only
            // the direct interface can send one) is read inside the queue, so an upload
            // waiting its turn holds nothing.
            const created = await this.serializeUpload(async () => {
              if (size !== null) return this.putBlob(hash, request.body, size, quota);
              const bytes = new Uint8Array(await request.arrayBuffer());
              return this.putBlob(hash, bytes, bytes.length, quota);
            });
            return new Response(null, { status: created ? 201 : 200 });
          }
          const body = await this.getBlob(hash);
          if (body === null) return text(404, "not found");
          return new Response(body, {
            status: 200,
            headers: { "content-type": "application/octet-stream" },
          });
        }
        case "/events":
          return this.events();
        default:
          return text(404, "not found");
      }
    } catch (e) {
      if (e instanceof BadRequest) return text(400, e.message);
      if (e instanceof QuotaExceeded) return text(413, "quota exceeded");
      return text(500, e instanceof Error ? e.message : String(e));
    }
  }

  // -- storage ---------------------------------------------------------------------

  /**
   * Run `work` after every upload queued before it, and keep the queue alive if one throws.
   * A failed upload still releases the queue for the next, and the only state it may have
   * left behind is an unreferenced R2 object (never an index row without bytes).
   */
  private serializeUpload<T>(work: () => Promise<T>): Promise<T> {
    const run = this.uploads.then(work);
    this.uploads = run.catch(() => undefined);
    return run;
  }

  /** Quota bytes in use, including uploads still on their way into R2 (`putBlob`). */
  private usedBytes(): number {
    return graphBytes(this.ctx.storage.sql) + this.inFlightBlobBytes;
  }

  private manifest(): Manifest {
    const sql = this.ctx.storage.sql;
    const pages = new Map<string, ManifestPage>();
    for (const row of sql
      .exec<{
        page_id: string;
        head_seq: number;
        tail_bytes: number;
        updated_at: number;
      }>(
        `SELECT page_id, max(seq) AS head_seq, sum(payload_len) AS tail_bytes,
                max(received_at) AS updated_at
         FROM page_log GROUP BY page_id`,
      )
      .toArray()) {
      pages.set(row.page_id, {
        page_id: row.page_id,
        head_seq: row.head_seq,
        frontier: "",
        snapshot_seq: 0,
        tail_bytes: row.tail_bytes,
        updated_at: row.updated_at,
        deleted: false,
      });
    }
    for (const row of sql
      .exec<{ page_id: string; seq: number }>("SELECT page_id, seq FROM page_snapshot")
      .toArray()) {
      const page = pages.get(row.page_id);
      if (page === undefined) {
        pages.set(row.page_id, {
          page_id: row.page_id,
          head_seq: row.seq,
          frontier: "",
          snapshot_seq: row.seq,
          tail_bytes: 0,
          updated_at: 0,
          deleted: false,
        });
      } else {
        page.snapshot_seq = row.seq;
        page.head_seq = Math.max(page.head_seq, row.seq);
      }
    }
    const ordered = [...pages.values()].sort((a, b) =>
      a.page_id < b.page_id ? -1 : a.page_id > b.page_id ? 1 : 0,
    );
    for (const page of ordered) {
      const row = sql
        .exec<{ frontier: string | null }>(
          `SELECT frontier FROM page_log WHERE page_id = ? ORDER BY seq DESC LIMIT 1`,
          page.page_id,
        )
        .toArray()[0];
      page.frontier = row?.frontier ?? "";
    }
    const instance = instanceId(sql);
    return {
      rev: revOf(sql),
      protocol_version: PROTOCOL_VERSION,
      pages: ordered,
      ...(instance === "" ? {} : { instance }),
    };
  }

  /**
   * Store a batch (§12.3 step 2). Dedupe runs inside the transaction and before quota: a
   * re-pushed batch whose response was lost is entirely duplicates, and counting its bytes
   * again would refuse it forever (§12.6). A change id repeated within the request is a
   * duplicate too — the first copy is stored, the second would break the UNIQUE index.
   */
  private push(pid: string, req: PushRequest, quota: number): PushResponse {
    const sql = this.ctx.storage.sql;
    return this.ctx.storage.transactionSync(() => {
      sql.exec("INSERT OR IGNORE INTO graph(id, created_at) VALUES (1, ?)", nowMs());
      let head = headOf(sql, pid);
      let accepted = 0;
      let duplicate = 0;
      let newBytes = 0;
      const seen = new Set<string>();
      const fresh: { update: UpdateIn; bytes: Uint8Array }[] = [];
      for (const update of req.updates) {
        const exists = sql
          .exec<{ present: number }>(
            "SELECT EXISTS(SELECT 1 FROM page_log WHERE change_id = ?) AS present",
            update.id,
          )
          .one();
        if (exists.present !== 0 || seen.has(update.id)) {
          duplicate += 1;
          continue;
        }
        seen.add(update.id);
        const bytes = decodePayload(update.payload);
        newBytes += bytes.length;
        fresh.push({ update, bytes });
      }
      if (this.usedBytes() + newBytes > quota) throw new QuotaExceeded();
      for (const { update, bytes } of fresh) {
        head += 1;
        const payloadId = chunkPayload(sql, bytes);
        sql.exec(
          `INSERT INTO page_log(page_id, seq, change_id, client_id, client_seq, frontier,
             payload_id, payload_len, received_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
          pid,
          head,
          update.id,
          // Owner identity is the client id prefix of the change id (as in Rust).
          update.id.split(":")[0] ?? "",
          update.client_seq,
          update.frontier,
          payloadId,
          bytes.length,
          nowMs(),
        );
        accepted += 1;
      }
      if (accepted > 0) bumpRev(sql);
      return { accepted, head_seq: head, duplicate };
    });
  }

  private pull(pid: string, since: number): PullResponse {
    const sql = this.ctx.storage.sql;
    const snap = sql
      .exec<{ seq: number; payload_id: string; payload_len: number }>(
        "SELECT seq, payload_id, payload_len FROM page_snapshot WHERE page_id = ?",
        pid,
      )
      .toArray()[0];
    // When the requested range was truncated, the snapshot replaces it.
    const snapshot =
      snap !== undefined && since < snap.seq
        ? b64encode(readPayload(sql, snap.payload_id, snap.payload_len))
        : undefined;
    const updates = sql
      .exec<{
        seq: number;
        change_id: string;
        frontier: string | null;
        payload_id: string;
        payload_len: number;
      }>(
        `SELECT seq, change_id, ifnull(frontier, '') AS frontier, payload_id, payload_len
         FROM page_log WHERE page_id = ? AND seq > ? ORDER BY seq`,
        pid,
        since,
      )
      .toArray()
      .map((row) => ({
        seq: row.seq,
        id: row.change_id,
        frontier: row.frontier ?? "",
        payload: b64encode(readPayload(sql, row.payload_id, row.payload_len)),
      }));
    const response: PullResponse = { updates, head_seq: headOf(sql, pid) };
    if (snapshot !== undefined) response.snapshot = snapshot;
    return response;
  }

  private putSnapshot(pid: string, req: SnapshotRequest): void {
    const sql = this.ctx.storage.sql;
    this.ctx.storage.transactionSync(() => {
      const head = headOf(sql, pid);
      if (req.covers_seq > head) {
        throw new BadRequest("snapshot covers unknown updates");
      }
      const current = sql
        .exec<{ seq: number; payload_id: string }>(
          "SELECT seq, payload_id FROM page_snapshot WHERE page_id = ?",
          pid,
        )
        .toArray()[0];
      // A snapshot not newer than the stored one is a silent no-op: no rev bump either.
      if (current !== undefined && current.seq >= req.covers_seq) return;
      const bytes = decodePayload(req.payload);
      const payloadId = chunkPayload(sql, bytes);
      sql.exec(
        `INSERT INTO page_snapshot(page_id, seq, frontier, payload_id, payload_len)
         VALUES (?, ?, '', ?, ?)
         ON CONFLICT(page_id) DO UPDATE SET
           seq = excluded.seq, payload_id = excluded.payload_id,
           payload_len = excluded.payload_len`,
        pid,
        req.covers_seq,
        payloadId,
        bytes.length,
      );
      if (current !== undefined) deletePayload(sql, current.payload_id);
      // The snapshot covers these updates, so they are no longer needed (§11.7).
      for (const row of sql
        .exec<{ payload_id: string }>(
          "SELECT payload_id FROM page_log WHERE page_id = ? AND seq <= ?",
          pid,
          req.covers_seq,
        )
        .toArray()) {
        deletePayload(sql, row.payload_id);
      }
      sql.exec(
        "DELETE FROM page_log WHERE page_id = ? AND seq <= ?",
        pid,
        req.covers_seq,
      );
      bumpRev(sql);
    });
  }

  // -- attachments -----------------------------------------------------------------

  /**
   * The graph id, which names this object (`idFromName(gid)`) and its R2 prefix. There is
   * no fallback: an object without a name (one made by `newUniqueId`, or a runtime that
   * stopped passing the name) would otherwise share one prefix with every other such
   * object, and write-once would then answer one graph's upload with another's bytes.
   */
  private get graphKey(): string {
    const name = this.ctx.id.name;
    if (name === undefined || name === "") {
      throw new Error(
        "this graph object has no name: it must be reached through idFromName(gid), " +
          "which names its attachments' R2 prefix",
      );
    }
    return name;
  }

  private blobKey(hash: string): string {
    return `graphs/${this.graphKey}/blobs/${hash}`;
  }

  /**
   * Write-once: an existing hash is kept as stored and answered 200. The bytes are not
   * checked against the hash — they may be ciphertext, and the client verifies after
   * decrypting (`tampered_blob_is_rejected` in `crates/utopia-syncd/tests/contract/mod.rs`).
   *
   * An index row whose object is gone is repaired instead: the bytes a re-upload carries are
   * put back, still answered 200 because nothing new is indexed. R2 can lose an object while
   * the Durable Object keeps the row (an R2 lifecycle rule aimed at `graphs/`, a bucket
   * restore), and a relay that trusted the row would answer 404 forever while refusing the one
   * upload that could heal it.
   *
   * The body streams into R2 and is never held here: the quota is checked against the
   * declared `size` before a byte is read, and `writeObject` makes R2 refuse a body that is
   * not exactly that size. A body that is not stored (a duplicate, a quota refusal) is read
   * and dropped.
   *
   * The whole check → put → index sequence runs under [`Self::serializeUpload`], because the
   * `await` on R2 leaves this object's input gate open: without it two requests for one hash
   * both saw no row and the loser's bytes overwrote R2 while its index insert was ignored,
   * and two uploads each fitted a quota neither fits with the other (§12.6, rule 12).
   *
   * The upload's size is reserved against the quota across that await
   * (`inFlightBlobBytes`), so a push that lands meanwhile cannot admit bytes the upload is
   * about to take.
   */
  private async putBlob(
    hash: string,
    body: BlobBody,
    size: number,
    quota: number,
  ): Promise<boolean> {
    // First, so a nameless object fails before it reads or stores anything.
    const key = this.blobKey(hash);
    const sql = this.ctx.storage.sql;
    const existing = sql
      .exec<{ present: number }>(
        "SELECT EXISTS(SELECT 1 FROM blob WHERE hash = ?) AS present",
        hash,
      )
      .one();
    if (existing.present !== 0) {
      // The index remembers these bytes; R2 should still hold them. If it does not, this is
      // the only chance to put them back, and `head` is one cheap call on a replay.
      if ((await this.env.BLOBS.head(key)) !== null) {
        await discard(body);
        return false;
      }
      await this.writeObject(key, body, size);
      return false;
    }
    if (this.usedBytes() + size > quota) {
      await discard(body);
      throw new QuotaExceeded();
    }
    sql.exec("INSERT OR IGNORE INTO graph(id, created_at) VALUES (1, ?)", nowMs());
    // Bytes first, index second: an interrupted write leaves no index row whose bytes are
    // missing. The reservation keeps those bytes out of another request's quota headroom
    // while they are only in flight.
    this.inFlightBlobBytes += size;
    try {
      await this.writeObject(key, body, size);
    } finally {
      this.inFlightBlobBytes -= size;
    }
    const inserted = sql.exec(
      "INSERT OR IGNORE INTO blob(hash, size, received_at) VALUES (?, ?, ?)",
      hash,
      size,
      nowMs(),
    );
    // `rowsWritten` is SQLite's `changes()`: 0 when a concurrent writer indexed the same
    // hash first, which mirrors the Rust relay's `INSERT OR IGNORE` result. Uploads are
    // serialised, so that is now only a replay of a hash indexed earlier.
    return inserted.rowsWritten > 0;
  }

  /**
   * Put `size` bytes into R2 under `key`. A stream goes through a `FixedLengthStream`: R2
   * needs a stream of known length. `pump` stops a body that turns out shorter or longer
   * than declared, which fails the put, so R2 stores nothing for it.
   */
  private async writeObject(key: string, body: BlobBody, size: number): Promise<void> {
    if (body === null || body instanceof Uint8Array) {
      const bytes = body ?? new Uint8Array(0);
      if (bytes.length !== size) {
        throw new BadRequest(`attachment body is not the ${size} bytes it declared`);
      }
      await this.env.BLOBS.put(key, bytes);
      return;
    }
    const { readable, writable } = new FixedLengthStream(size);
    const [put, pumped] = await Promise.allSettled([
      this.env.BLOBS.put(key, readable),
      pump(body, writable, size),
    ]);
    if (pumped.status === "rejected") throw pumped.reason;
    if (put.status === "rejected") throw put.reason;
  }

  private async getBlob(hash: string): Promise<ReadableStream | null> {
    const key = this.blobKey(hash);
    const row = this.ctx.storage.sql
      .exec<{ present: number }>(
        "SELECT EXISTS(SELECT 1 FROM blob WHERE hash = ?) AS present",
        hash,
      )
      .one();
    if (row.present === 0) return null;
    const object = await this.env.BLOBS.get(key);
    return object === null ? null : object.body;
  }

  // -- realtime (§12.3 "Realtime") -------------------------------------------------

  /**
   * The socket only carries `{"page_id","head_seq"}` invalidation notices; the client then
   * pulls over HTTP. A missing notice is harmless (the next poll catches up).
   *
   * Hibernation API: `acceptWebSocket`, never `accept()`, so an idle graph holds no
   * billable duration and the object can be evicted with its subscribers attached. Nothing
   * per-socket is kept in memory.
   */
  private events(): Response {
    const pair = new WebSocketPair();
    const client = pair[0];
    const server = pair[1];
    this.ctx.acceptWebSocket(server);
    return new Response(null, { status: 101, webSocket: client });
  }

  private notify(pid: string, head: number): void {
    const message = JSON.stringify({ page_id: pid, head_seq: head });
    for (const socket of this.ctx.getWebSockets()) {
      try {
        socket.send(message);
      } catch {
        // A socket that closed between the write and the notice: clients poll anyway.
      }
    }
  }

  override async webSocketMessage(): Promise<void> {
    // Incoming messages are ignored: notices only travel one way.
  }

  /**
   * Complete the closing handshake, and never throw. The client's code is echoed when it may
   * be sent; 1005 (a close frame with no code), 1006 (the connection dropped, which is how
   * the Rust client's listener stops) and 1015 are reserved and `close` refuses them, so
   * those are answered with 1000. Echoing no reason keeps clear of the 123-byte limit. A
   * socket that is already closed (the runtime may have answered the close frame itself)
   * makes `close` throw, which is equally nothing to act on: no per-socket state is held.
   */
  override async webSocketClose(
    socket: WebSocket,
    code: number,
    _reason: string,
    _wasClean: boolean,
  ): Promise<void> {
    try {
      socket.close(sendableCloseCode(code) ? code : 1000);
    } catch {
      // Already closed, or closing: nothing left to answer.
    }
  }

  override async webSocketError(): Promise<void> {
    // Nothing to clean up: no per-socket state is held.
  }
}
