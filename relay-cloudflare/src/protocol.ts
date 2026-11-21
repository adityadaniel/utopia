//! The v1 wire format (PRD §12.3), as `crates/utopia-proto/src/lib.rs` defines it.
//!
//! Field names, integer widths and base64 are the Rust types' serde output: `id` (not
//! `change_id`) on an update, `head_seq`/`duplicate` on a push, `snapshot` omitted when
//! there is none, `instance` omitted when empty. Payloads are opaque bytes — CRDT updates
//! or ciphertext — so nothing here parses them.

export const PROTOCOL_VERSION = 1;

export interface ManifestPage {
  page_id: string;
  head_seq: number;
  frontier: string;
  snapshot_seq: number;
  tail_bytes: number;
  updated_at: number;
  deleted: boolean;
}

export interface Manifest {
  rev: number;
  protocol_version: number;
  pages: ManifestPage[];
  instance?: string;
}

export interface UpdateIn {
  id: string;
  client_seq: number;
  frontier: string;
  /** base64 */
  payload: string;
}

export interface PushRequest {
  updates: UpdateIn[];
}

export interface PushResponse {
  accepted: number;
  head_seq: number;
  duplicate: number;
}

export interface UpdateOut {
  seq: number;
  id: string;
  frontier: string;
  /** base64 */
  payload: string;
}

export interface PullResponse {
  /** base64; absent when the requested range is still in the log */
  snapshot?: string;
  updates: UpdateOut[];
  head_seq: number;
}

export interface SnapshotRequest {
  covers_seq: number;
  /** base64 */
  payload: string;
}

/** A request body the relay refuses with 400, carrying the message Rust's `ApiError` sends. */
export class BadRequest extends Error {}

/** Chunked for the call stack; a multiple of 3 so each chunk's base64 concatenates. */
const B64_AT_A_TIME = 8190;

/** Standard base64 with padding, exactly `utopia_proto::encode`. */
export function b64encode(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += B64_AT_A_TIME) {
    out += btoa(String.fromCharCode(...bytes.subarray(i, i + B64_AT_A_TIME)));
  }
  return out;
}

/** Standard base64; like `utopia_proto::decode`, unpadded input is accepted. */
export function b64decode(s: string): Uint8Array {
  const padded = s + "=".repeat((4 - (s.length % 4)) % 4);
  const bin = atob(padded);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function fail(what: string, detail: string): never {
  throw new BadRequest(`${what}: ${detail}`);
}

function str(o: Record<string, unknown>, key: string): string {
  const v = o[key];
  if (typeof v !== "string") fail("bad body", `"${key}" must be a string`);
  return v;
}

function uint(o: Record<string, unknown>, key: string): number {
  const v = o[key];
  if (typeof v !== "number" || !Number.isSafeInteger(v) || v < 0) {
    fail("bad body", `"${key}" must be a non-negative integer`);
  }
  return v;
}

function int(o: Record<string, unknown>, key: string): number {
  const v = o[key];
  if (typeof v !== "number" || !Number.isSafeInteger(v)) {
    fail("bad body", `"${key}" must be an integer`);
  }
  return v;
}

function payload(o: Record<string, unknown>, key: string): string {
  const s = str(o, key);
  try {
    b64decode(s);
  } catch {
    fail("bad body", `"${key}" is not base64`);
  }
  return s;
}

/**
 * Decode a request body as `serde_json` would: a missing or mistyped field is the 400 the
 * Rust relay answers (`decode_body`), never a 500.
 */
export function parseJsonRequest<T>(raw: Uint8Array, parse: (v: unknown) => T): T {
  let value: unknown;
  try {
    value = JSON.parse(new TextDecoder().decode(raw));
  } catch (e) {
    throw new BadRequest(e instanceof Error ? e.message : String(e));
  }
  return parse(value);
}

export function parsePushRequest(value: unknown): PushRequest {
  if (!isRecord(value)) fail("bad body", "expected an object");
  const updates = value["updates"];
  if (!Array.isArray(updates)) fail("bad body", `"updates" must be an array`);
  return {
    updates: updates.map((u) => {
      if (!isRecord(u)) fail("bad body", "an update must be an object");
      return {
        id: str(u, "id"),
        client_seq: int(u, "client_seq"),
        frontier: str(u, "frontier"),
        payload: payload(u, "payload"),
      };
    }),
  };
}

export function parseSnapshotRequest(value: unknown): SnapshotRequest {
  if (!isRecord(value)) fail("bad body", "expected an object");
  return {
    covers_seq: uint(value, "covers_seq"),
    payload: payload(value, "payload"),
  };
}
