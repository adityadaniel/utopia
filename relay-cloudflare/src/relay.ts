//! The relay logic in front of one Durable Object per graph: routing, auth, validation,
//! body decoding. `index.ts` is the thin entry point that exports only the handler and the
//! Durable Object class.
//!
//! Routes are `utopia_syncd::router()`'s, exactly (PRD §12.3):
//!
//! - `GET  /v1/graphs/{gid}/manifest`
//! - `POST /v1/graphs/{gid}/pages/{pid}/updates`, `GET …/updates?since=<seq>`
//! - `POST /v1/graphs/{gid}/pages/{pid}/snapshot`
//! - `GET  /v1/graphs/{gid}/events` (WebSocket upgrade)
//! - `PUT|GET /v1/graphs/{gid}/blobs/{hash}`
//! - `GET  /healthz` → `ok`
//!
//! Authentication, id/hash validation and body decoding happen here, before any Durable
//! Object is touched; every request for a graph then goes to `idFromName(gid)`, whose
//! state is that graph's log (PRD §12.8).

import { decompress } from "fzstd";

import { BLOB_SIZE_HEADER, DEFAULT_QUOTA_BYTES, type Env } from "./graph";
import {
  BadRequest,
  parseJsonRequest,
  parsePushRequest,
  parseSnapshotRequest,
} from "./protocol";

/** 64 MiB attachment plus the encryption envelope, as `DefaultBodyLimit` in Rust. */
export const MAX_BODY_BYTES = (64 << 20) + (64 << 10);

/** A body over the limit: 413, decided before the Durable Object sees a byte. */
export class PayloadTooLarge extends Error {
  constructor() {
    super("payload too large");
  }
}

/**
 * Accepted tokens, comma-separated (`RELAY_TOKENS`, a Worker secret in production).
 * Empty means auth is off only with `ALLOW_ANONYMOUS=1` (local development). This is a
 * deliberate difference from `utopia-syncd`, whose empty list disables auth: a hosted
 * relay that silently accepts everyone because a secret failed to load is worse than one
 * that refuses everyone until it is configured.
 */
export function acceptedTokens(env: Env): string[] {
  return (env.RELAY_TOKENS ?? "")
    .split(",")
    .map((t) => t.trim())
    .filter((t) => t.length > 0);
}

/**
 * Whether the request may proceed: its bearer token is one of `RELAY_TOKENS`, or there are
 * no tokens and `ALLOW_ANONYMOUS=1`.
 *
 * The token stops here. `utopia_syncd::Ctx::auth` returns it as the caller's identity and
 * the Rust relay stores it in `graph.owner_id`; nothing ever reads that column, so this relay
 * neither forwards the token to a Durable Object (an internal URL or header ends up in
 * request traces) nor stores it.
 */
export function authorize(env: Env, headers: Headers): boolean {
  const tokens = acceptedTokens(env);
  if (tokens.length === 0) return env.ALLOW_ANONYMOUS === "1";
  const bearer = /^Bearer (.+)$/.exec(headers.get("authorization") ?? "");
  if (bearer === null) return false;
  return tokens.includes(bearer[1] ?? "");
}

/** 1–128 characters of `[A-Za-z0-9_-]`: safe in a URL and in an R2 key. */
export function validId(id: string): boolean {
  return id.length > 0 && id.length <= 128 && /^[A-Za-z0-9_-]+$/.test(id);
}

/** A content hash: 64 lowercase hex characters. */
export function validHash(hash: string): boolean {
  return /^[0-9a-f]{64}$/.test(hash);
}

export function bodyTooLarge(contentLength: string | null): boolean {
  if (contentLength === null || !/^\d+$/.test(contentLength)) return false;
  return Number(contentLength) > MAX_BODY_BYTES;
}

/**
 * The body's declared length, or `null` when it has none (a chunked body). A declared
 * length over the limit is refused here, before a byte is read.
 */
function declaredLength(request: Request): number | null {
  const raw = request.headers.get("content-length");
  if (bodyTooLarge(raw)) throw new PayloadTooLarge();
  if (raw === null || !/^\d+$/.test(raw)) return null;
  return Number(raw);
}

type Route =
  | { kind: "healthz" }
  | { kind: "manifest"; gid: string }
  | { kind: "updates"; gid: string; pid: string }
  | { kind: "snapshot"; gid: string; pid: string }
  | { kind: "events"; gid: string }
  | { kind: "blob"; gid: string; hash: string };

const ALLOWED: Record<Route["kind"], string[]> = {
  healthz: ["GET", "HEAD"],
  manifest: ["GET", "HEAD"],
  updates: ["GET", "HEAD", "POST"],
  snapshot: ["POST"],
  events: ["GET", "HEAD"],
  blob: ["GET", "HEAD", "PUT"],
};

function segment(parts: string[], index: number): string {
  const raw = parts[index] ?? "";
  try {
    return decodeURIComponent(raw);
  } catch {
    // A malformed escape is not an id; leave it for `validId` to refuse with a 400.
    return raw;
  }
}

/** Path → route, or `null` for a path no endpoint claims (404). */
export function parseRoute(pathname: string): Route | null {
  if (pathname === "/healthz") return { kind: "healthz" };
  const parts = pathname.split("/");
  // pathname always starts with "/", so parts[0] is "".
  if (parts[1] !== "v1" || parts[2] !== "graphs") return null;
  const gid = segment(parts, 3);
  if (parts.length === 5 && parts[4] === "manifest") {
    return { kind: "manifest", gid };
  }
  if (parts.length === 5 && parts[4] === "events") {
    return { kind: "events", gid };
  }
  if (parts.length === 6 && parts[4] === "blobs") {
    return { kind: "blob", gid, hash: segment(parts, 5) };
  }
  if (parts.length === 7 && parts[4] === "pages" && parts[6] === "updates") {
    return { kind: "updates", gid, pid: segment(parts, 5) };
  }
  if (parts.length === 7 && parts[4] === "pages" && parts[6] === "snapshot") {
    return { kind: "snapshot", gid, pid: segment(parts, 5) };
  }
  return null;
}

function text(status: number, body: string): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}

function invalid(what: string, value: string): string {
  return `invalid ${what} ${JSON.stringify(value)}`;
}

/**
 * The chunks of a body with no declared length, stopped as soon as they pass the limit.
 * They are kept as they arrived, so the body is held once and never joined.
 */
async function readChunks(
  body: ReadableStream<Uint8Array>,
): Promise<{ chunks: Uint8Array[]; total: number }> {
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > MAX_BODY_BYTES) {
      await reader.cancel();
      throw new PayloadTooLarge();
    }
    chunks.push(value);
  }
  return { chunks, total };
}

/**
 * Read a JSON request body, refusing anything over the limit while it arrives: a declared
 * `Content-Length` over the limit is answered before a byte is read, and a body without one
 * (chunked) is stopped as soon as it passes the limit.
 *
 * A declared length is read straight into one buffer of that size, so the body is held
 * once. A chunked body is held once as its chunks and copied into one buffer at the end
 * (zstd and JSON need contiguous bytes) unless it arrived as a single chunk.
 */
async function readBody(request: Request): Promise<Uint8Array> {
  const declared = declaredLength(request);
  if (request.body === null) return new Uint8Array(0);
  if (declared !== null) {
    const body = new Uint8Array(declared);
    const reader = request.body.getReader();
    let offset = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (offset + value.length > declared) {
        await reader.cancel();
        throw new BadRequest("body is longer than its content-length");
      }
      body.set(value, offset);
      offset += value.length;
    }
    return offset === declared ? body : body.subarray(0, offset);
  }
  const { chunks, total } = await readChunks(request.body);
  if (chunks.length === 1 && chunks[0] !== undefined) return chunks[0];
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.length;
  }
  return body;
}

/**
 * An attachment body for the Durable Object, and its size. With a declared length the
 * request's own stream is handed on, so the Worker never holds the body: the bytes go from
 * the client through the object into R2. A chunked body has no length to check the quota
 * against, so it is read here (held once, as its chunks) and handed on as a stream of those
 * chunks, each released once the object has taken it.
 */
async function blobBody(
  request: Request,
): Promise<{ body: ReadableStream<Uint8Array> | null; size: number }> {
  const declared = declaredLength(request);
  if (request.body === null) return { body: null, size: 0 };
  if (declared !== null) return { body: request.body, size: declared };
  const { chunks, total } = await readChunks(request.body);
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      const next = chunks.shift();
      if (next === undefined) controller.close();
      else controller.enqueue(next);
    },
  });
  return { body, size: total };
}

/**
 * Every client POST is zstd (`Content-Encoding: zstd`); anything else is taken as-is.
 * `utopia_syncd::decode_body` compares the header's bytes to `zstd` the same way.
 */
export function decodeBody(request: Request, raw: Uint8Array): Uint8Array {
  if (request.headers.get("content-encoding") !== "zstd") return raw;
  try {
    return decompress(raw);
  } catch (e) {
    throw new BadRequest(e instanceof Error ? e.message : String(e));
  }
}

/**
 * The per-graph quota: `QUOTA_BYTES` in wrangler.jsonc, or utopia-syncd's 1 GiB default.
 * A missing, empty or unparseable value is the default, never a 0-byte quota.
 */
export function quotaFrom(env: Env): number {
  const raw = env.QUOTA_BYTES;
  if (raw === undefined || raw.trim() === "") return DEFAULT_QUOTA_BYTES;
  const n = Number(raw);
  return Number.isSafeInteger(n) && n >= 0 ? n : DEFAULT_QUOTA_BYTES;
}

/** `since` is a u64 in Rust: absent means 0, and anything else that is not a number is 400. */
export function sinceFrom(url: URL): number {
  const raw = url.searchParams.get("since");
  if (raw === null) return 0;
  if (!/^\d+$/.test(raw) || !Number.isSafeInteger(Number(raw))) {
    throw new BadRequest(`invalid since ${JSON.stringify(raw)}`);
  }
  return Number(raw);
}

/** One request: route, authenticate, validate, then hand it to the graph's object. */
export async function handle(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const route = parseRoute(url.pathname);
  if (route === null) return text(404, "not found");
  if (!ALLOWED[route.kind].includes(request.method)) {
    return text(405, "method not allowed");
  }
  if (route.kind === "healthz") return text(200, "ok");

  if (!authorize(env, request.headers)) return text(401, "unauthorized");
  if (!validId(route.gid)) return text(400, invalid("id", route.gid));

  const stub = env.GRAPH.get(env.GRAPH.idFromName(route.gid));
  const quota = String(quotaFrom(env));

  try {
    switch (route.kind) {
      case "manifest": {
        const ifNoneMatch = request.headers.get("if-none-match");
        return await stub.fetch("https://graph/manifest", {
          headers:
            ifNoneMatch === null ? {} : { "if-none-match": ifNoneMatch },
        });
      }
      case "updates": {
        if (!validId(route.pid)) return text(400, invalid("id", route.pid));
        if (request.method === "POST") {
          const body = decodeBody(request, await readBody(request));
          // Validated here, then forwarded as the bytes that were validated rather than
          // serialised again (one string less in memory).
          parseJsonRequest(body, parsePushRequest);
          return await stub.fetch(
            `https://graph/push?pid=${encodeURIComponent(route.pid)}&quota=${quota}`,
            { method: "POST", body },
          );
        }
        const since = sinceFrom(url);
        return await stub.fetch(
          `https://graph/pull?pid=${encodeURIComponent(route.pid)}&since=${since}`,
        );
      }
      case "snapshot": {
        if (!validId(route.pid)) return text(400, invalid("id", route.pid));
        const body = decodeBody(request, await readBody(request));
        parseJsonRequest(body, parseSnapshotRequest);
        return await stub.fetch(
          `https://graph/snapshot?pid=${encodeURIComponent(route.pid)}`,
          { method: "POST", body },
        );
      }
      case "events": {
        // The Durable Object calls `acceptWebSocket` (Hibernation API); a plain GET is
        // not an upgrade, which axum's extractor also refuses.
        if ((request.headers.get("upgrade") ?? "").toLowerCase() !== "websocket") {
          return text(400, "connection is not a websocket upgrade");
        }
        return await stub.fetch("https://graph/events", {
          headers: { upgrade: "websocket" },
        });
      }
      case "blob": {
        if (!validHash(route.hash)) return text(400, invalid("blob hash", route.hash));
        if (request.method === "PUT") {
          const { body, size } = await blobBody(request);
          return await stub.fetch(`https://graph/blob?hash=${route.hash}&quota=${quota}`, {
            method: "PUT",
            body,
            headers: { [BLOB_SIZE_HEADER]: String(size) },
          });
        }
        return await stub.fetch(`https://graph/blob?hash=${route.hash}`);
      }
    }
  } catch (e) {
    if (e instanceof PayloadTooLarge) return text(413, e.message);
    if (e instanceof BadRequest) return text(400, e.message);
    return text(500, e instanceof Error ? e.message : String(e));
  }
}
