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

import { DEFAULT_QUOTA_BYTES, type Env } from "./graph";
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

export type Auth = { ok: true; owner: string } | { ok: false };

/**
 * The caller's identity is its token, as in `utopia_syncd::Ctx::auth`. Anonymous callers
 * (`ALLOW_ANONYMOUS=1` with no tokens) are the owner `anonymous`.
 */
export function authorize(env: Env, headers: Headers): Auth {
  const tokens = acceptedTokens(env);
  const bearer = /^Bearer (.+)$/.exec(headers.get("authorization") ?? "");
  if (tokens.length === 0) {
    if (env.ALLOW_ANONYMOUS !== "1") return { ok: false };
    return { ok: true, owner: "anonymous" };
  }
  if (bearer === null) return { ok: false };
  const token = bearer[1] ?? "";
  return tokens.includes(token) ? { ok: true, owner: token } : { ok: false };
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
 * Read a request body, refusing anything over the limit while it arrives: a declared
 * `Content-Length` over the limit is answered before a byte is buffered, and a body
 * without one (chunked) is stopped as soon as it passes the limit.
 */
async function readBody(request: Request): Promise<Uint8Array> {
  if (bodyTooLarge(request.headers.get("content-length"))) {
    throw new PayloadTooLarge();
  }
  if (request.body === null) return new Uint8Array(0);
  const reader = request.body.getReader();
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
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.length;
  }
  return body;
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

  const auth = authorize(env, request.headers);
  if (!auth.ok) return text(401, "unauthorized");
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
          const req = parseJsonRequest(body, parsePushRequest);
          return await stub.fetch(
            `https://graph/push?pid=${encodeURIComponent(route.pid)}` +
              `&owner=${encodeURIComponent(auth.owner)}&quota=${quota}`,
            { method: "POST", body: JSON.stringify(req) },
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
        const req = parseJsonRequest(body, parseSnapshotRequest);
        return await stub.fetch(
          `https://graph/snapshot?pid=${encodeURIComponent(route.pid)}`,
          { method: "POST", body: JSON.stringify(req) },
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
          const body = await readBody(request);
          return await stub.fetch(
            `https://graph/blob?hash=${route.hash}&owner=${encodeURIComponent(auth.owner)}` +
              `&quota=${quota}`,
            { method: "PUT", body },
          );
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
