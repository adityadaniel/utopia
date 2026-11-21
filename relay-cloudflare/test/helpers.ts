// Test helpers. Requests go through the Worker exactly as a client's would, with
// genuinely zstd-compressed bodies (`node:zlib` at level 3, the level the Rust client's
// `zstd::encode_all` uses in `crates/utopia-sync/src/http.rs`).
//
// Every test makes its own graph id, so its Durable Object starts empty no matter what
// another test wrote: this runner does not roll Durable Object storage back per test.

import { SELF, env, runInDurableObject } from "cloudflare:test";
import { zstdCompressSync } from "node:zlib";

import type { Env as RelayEnv, GraphObject } from "../src/graph";
import {
  b64encode,
  type Manifest,
  type PullResponse,
  type PushResponse,
  type UpdateIn,
} from "../src/protocol";

export const TOKEN = "alpha";
export const OTHER_TOKEN = "beta";
export const PID = "page-1";
export const BASE = "https://relay.test";

export function newGraphId(): string {
  return `graph-${crypto.randomUUID()}`;
}

export function encode(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

export function decode(body: ArrayBuffer | Uint8Array): string {
  return new TextDecoder().decode(body);
}

export function b64(bytes: Uint8Array): string {
  return b64encode(bytes);
}

/** A request to the Worker; `token: null` sends no Authorization header. */
export async function call(
  path: string,
  init: RequestInit = {},
  token: string | null = TOKEN,
): Promise<Response> {
  const headers = new Headers(init.headers);
  if (token !== null) headers.set("authorization", `Bearer ${token}`);
  return SELF.fetch(`${BASE}${path}`, { ...init, headers });
}

/** One graph, reached the way a client reaches it. */
export class Graph {
  readonly gid: string;

  constructor(gid: string = newGraphId()) {
    this.gid = gid;
  }

  url(path: string): string {
    return `/v1/graphs/${this.gid}/${path}`;
  }

  stub(): DurableObjectStub<GraphObject> {
    return env.GRAPH.get(env.GRAPH.idFromName(this.gid));
  }

  /** Run `fn` inside the Durable Object: instance state and SQL, for assertions. */
  async inDo<R>(
    fn: (instance: GraphObject, state: DurableObjectState) => R | Promise<R>,
  ): Promise<R> {
    return runInDurableObject(this.stub(), fn);
  }

  /** The Durable Object's own interface, bypassing the Worker: quota override tests. */
  async direct(path: string, init: RequestInit = {}): Promise<Response> {
    return this.stub().fetch(`https://graph${path}`, init);
  }

  async call(
    path: string,
    init: RequestInit = {},
    token: string | null = TOKEN,
  ): Promise<Response> {
    return call(this.url(path), init, token);
  }

  async push(updates: UpdateIn[], pid: string = PID): Promise<Response> {
    const { body, headers } = zstdBody({ updates });
    return this.call(`pages/${pid}/updates`, { method: "POST", body, headers });
  }

  async pushOutcome(
    updates: UpdateIn[],
    pid: string = PID,
  ): Promise<PushResponse> {
    const res = await this.push(updates, pid);
    if (res.status !== 200) {
      throw new Error(`push failed: ${res.status} ${await res.text()}`);
    }
    return (await res.json()) as PushResponse;
  }

  async pull(since = 0, pid: string = PID): Promise<PullResponse> {
    const res = await this.call(`pages/${pid}/updates?since=${since}`);
    if (res.status !== 200) {
      throw new Error(`pull failed: ${res.status} ${await res.text()}`);
    }
    return (await res.json()) as PullResponse;
  }

  async snapshot(
    coversSeq: number,
    payload: string | Uint8Array,
    pid: string = PID,
  ): Promise<Response> {
    const bytes = typeof payload === "string" ? encode(payload) : payload;
    const { body, headers } = zstdBody({ covers_seq: coversSeq, payload: b64(bytes) });
    return this.call(`pages/${pid}/snapshot`, { method: "POST", body, headers });
  }

  async manifest(
    headers: Record<string, string> = {},
    token: string | null = TOKEN,
  ): Promise<Response> {
    return this.call("manifest", { headers }, token);
  }

  async readManifest(): Promise<Manifest> {
    return (await this.manifest()).json() as Promise<Manifest>;
  }

  async putBlob(hash: string, bytes: Uint8Array): Promise<Response> {
    return this.call(`blobs/${hash}`, { method: "PUT", body: bytes });
  }

  async getBlob(hash: string): Promise<Response> {
    return this.call(`blobs/${hash}`);
  }

  async logRows(): Promise<{ seq: number; payload_len: number }[]> {
    return this.inDo((_instance, state) =>
      state.storage.sql
        .exec<{ seq: number; payload_len: number }>(
          "SELECT seq, payload_len FROM page_log ORDER BY seq",
        )
        .toArray()
        .map((row) => ({ seq: row.seq, payload_len: row.payload_len })),
    );
  }

  async snapshotRow(): Promise<{ seq: number; payload_len: number } | null> {
    return this.inDo((_instance, state) => {
      const row = state.storage.sql
        .exec<{ seq: number; payload_len: number }>(
          "SELECT seq, payload_len FROM page_snapshot",
        )
        .toArray()[0];
      return row === undefined ? null : { seq: row.seq, payload_len: row.payload_len };
    });
  }
}

export function update(
  id: string,
  payload: string | Uint8Array,
  clientSeq = 1,
  frontier = "",
): UpdateIn {
  const bytes = typeof payload === "string" ? encode(payload) : payload;
  return { id, client_seq: clientSeq, frontier, payload: b64(bytes) };
}

/**
 * zstd-compress `raw`. Cloudflare's `node:zlib` zstd encoder fails on inputs of 64 KiB or
 * more ("Unexpected end of file"), so the body goes out as concatenated 32 KiB frames — a
 * valid zstd stream that `fzstd` decodes like any other. The Worker's decoder is the one
 * the Rust client's single large frame reaches too.
 */
export function zstdCompress(raw: Uint8Array): Uint8Array {
  const FRAME = 32 * 1024;
  const frames: Uint8Array[] = [];
  for (let i = 0; i < raw.length; i += FRAME) {
    frames.push(new Uint8Array(zstdCompressSync(raw.subarray(i, i + FRAME))));
  }
  if (frames.length === 0) {
    frames.push(new Uint8Array(zstdCompressSync(new Uint8Array(0))));
  }
  const total = frames.reduce((sum, frame) => sum + frame.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const frame of frames) {
    out.set(frame, offset);
    offset += frame.length;
  }
  return out;
}

/** A zstd body, as every client POST is. */
export function zstdBody(value: unknown): {
  body: Uint8Array;
  headers: Record<string, string>;
} {
  const raw = encode(JSON.stringify(value));
  return {
    body: zstdCompress(raw),
    headers: { "content-encoding": "zstd", "content-type": "application/json" },
  };
}

/** A deterministic pseudo-random byte string (a payload of `size` bytes). */
export function pseudoRandom(size: number, seed = 7): Uint8Array {
  const out = new Uint8Array(size);
  let state = seed >>> 0;
  for (let i = 0; i < size; i++) {
    state = (state * 1664525 + 1013904223) >>> 0;
    out[i] = (state >>> 24) & 0xff;
  }
  return out;
}

export function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/** Base64 → bytes, for assertions on a pull response. */
export function unb64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** One request the Worker sent to a graph's Durable Object (`recordingEnv`). */
export interface ObjectCall {
  url: string;
  headers: [string, string][];
  body: unknown;
}

/**
 * The deployment's bindings with `GRAPH` wrapped so every request the Worker makes to a
 * Durable Object is recorded before it is sent on unchanged. For calling `handle` directly:
 * the internal URL and headers are otherwise invisible to a test.
 */
export function recordingEnv(vars: Partial<RelayEnv> = {}): {
  relayEnv: RelayEnv;
  calls: ObjectCall[];
} {
  const calls: ObjectCall[] = [];
  const namespace = env.GRAPH;
  const GRAPH = {
    idFromName: (name: string) => namespace.idFromName(name),
    get: (id: DurableObjectId) => {
      const stub = namespace.get(id);
      return {
        fetch: (input: string, init: RequestInit = {}) => {
          calls.push({
            url: input,
            headers: [...new Headers(init.headers).entries()],
            body: init.body,
          });
          return stub.fetch(input, init);
        },
      };
    },
  } as unknown as RelayEnv["GRAPH"];
  return { relayEnv: { ...env, ...vars, GRAPH }, calls };
}

/** A body of `chunks` chunks of `chunkBytes` each, sent without a Content-Length. */
export function chunkedBody(chunks: number, chunkBytes: number, seed = 1): ReadableStream<Uint8Array> {
  let sent = 0;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      if (sent === chunks) {
        controller.close();
        return;
      }
      controller.enqueue(pseudoRandom(chunkBytes, seed + sent));
      sent += 1;
    },
  });
}

export function sha256Hex(bytes: Uint8Array): Promise<string> {
  return crypto.subtle.digest("SHA-256", bytes).then((d) =>
    Array.from(new Uint8Array(d), (b) => b.toString(16).padStart(2, "0")).join(""),
  );
}
