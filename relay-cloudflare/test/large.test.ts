// Large payloads: a Durable Object's SQLite rows are limited to about 2 MB (key plus
// value), so payloads are stored in 512 KiB chunks. These tests prove an encrypted
// snapshot of the size a real graph produces still round-trips byte for byte.

import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import { MAX_BODY_BYTES, handle } from "../src/relay";
import {
  BASE,
  Graph,
  TOKEN,
  bytesEqual,
  chunkedBody,
  pseudoRandom,
  recordingEnv,
  sha256Hex,
  unb64,
  update,
} from "./helpers";

const MIB = 1024 * 1024;

async function chunkSizes(graph: Graph): Promise<number[]> {
  return graph.inDo((_instance, state) =>
    state.storage.sql
      .exec<{ len: number }>(
        "SELECT length(bytes) AS len FROM payload_chunk ORDER BY payload_id, idx",
      )
      .toArray()
      .map((row) => row.len),
  );
}

async function blobSizes(graph: Graph): Promise<number[]> {
  return graph.inDo((_instance, state) =>
    state.storage.sql
      .exec<{ size: number }>("SELECT size FROM blob ORDER BY hash")
      .toArray()
      .map((row) => row.size),
  );
}

describe("chunked payloads", () => {
  it("a 5 MiB snapshot round-trips and no row approaches the 2 MB limit", async () => {
    const graph = new Graph();
    await graph.pushOutcome([update("c:1", "before compaction")]);
    const snapshotBytes = pseudoRandom(5 * MIB, 11);

    expect((await graph.snapshot(1, snapshotBytes)).status).toBe(204);

    // A device pulling from zero gets exactly the bytes that were uploaded.
    const pulled = await graph.pull(0);
    expect(pulled.snapshot).toBeDefined();
    expect(bytesEqual(unb64(pulled.snapshot ?? ""), snapshotBytes)).toBe(true);
    expect(pulled.updates).toEqual([]);
    expect(pulled.head_seq).toBe(1);
    expect(await graph.snapshotRow()).toEqual({ seq: 1, payload_len: 5 * MIB });

    const sizes = await chunkSizes(graph);
    expect(sizes).toHaveLength(10);
    expect(Math.max(...sizes)).toBeLessThanOrEqual(512 * 1024);
    expect(sizes.reduce((a, b) => a + b, 0)).toBe(5 * MIB);

    // The covered log row is gone, chunks included.
    expect(await graph.logRows()).toEqual([]);
  });

  it("an update over the 1 MiB threshold round-trips too", async () => {
    const graph = new Graph();
    const payload = pseudoRandom(MIB + MIB / 2, 5);
    expect((await graph.pushOutcome([update("big:1", payload)])).accepted).toBe(1);

    const pulled = await graph.pull(0);
    expect(pulled.updates).toHaveLength(1);
    expect(bytesEqual(unb64(pulled.updates[0]?.payload ?? ""), payload)).toBe(true);

    const sizes = await chunkSizes(graph);
    expect(sizes).toHaveLength(3);
    expect(Math.max(...sizes)).toBeLessThanOrEqual(512 * 1024);
  });

  it("snapshotting again releases the chunks of the snapshot it replaces", async () => {
    const graph = new Graph();
    await graph.pushOutcome([update("c:1", "one"), update("c:2", "two", 2)]);
    const first = pseudoRandom(2 * MIB, 21);
    const second = pseudoRandom(3 * MIB, 22);
    expect((await graph.snapshot(1, first)).status).toBe(204);
    expect((await graph.snapshot(2, second)).status).toBe(204);

    // Only the second snapshot's chunks remain: no orphaned bytes keep charging quota.
    const total = (await chunkSizes(graph)).reduce((a, b) => a + b, 0);
    expect(total).toBe(3 * MIB);
    expect(bytesEqual(unb64((await graph.pull(0)).snapshot ?? ""), second)).toBe(true);
  });
});

// Attachments up to the 64 MiB body limit. A Worker isolate has 128 MB of memory, so a blob
// PUT must not hold the body in full — once in the Worker and again in the Durable Object
// was ~128 MiB for one 64 MiB upload. With a Content-Length (the Rust client's `ureq …
// send_bytes` always sends one) the body streams from the client through the Worker and the
// object into R2; local workerd does not enforce the memory limit, so these tests pin the
// streaming itself rather than a crash.
describe("streamed attachments", () => {
  it("a 48 MiB attachment round-trips through the Worker", async () => {
    const graph = new Graph();
    const bytes = pseudoRandom(48 * MIB, 31);
    const hash = await sha256Hex(bytes);
    expect((await graph.putBlob(hash, bytes)).status).toBe(201);

    const got = await graph.getBlob(hash);
    expect(got.status).toBe(200);
    const back = new Uint8Array(await got.arrayBuffer());
    expect(back.length).toBe(48 * MIB);
    expect(await sha256Hex(back)).toBe(hash);
    expect(await blobSizes(graph)).toEqual([48 * MIB]);
    expect((await env.BLOBS.head(`graphs/${graph.gid}/blobs/${hash}`))?.size).toBe(48 * MIB);

    // Write-once still holds for a streamed replay.
    expect((await graph.putBlob(hash, bytes)).status).toBe(200);
    expect(await blobSizes(graph)).toEqual([48 * MIB]);
  });

  it("the Worker forwards the request's own stream, not a buffered copy", async () => {
    const { relayEnv, calls } = recordingEnv();
    const graph = new Graph();
    const bytes = pseudoRandom(MIB, 32);
    const hash = await sha256Hex(bytes);
    const res = await handle(
      new Request(`${BASE}${graph.url(`blobs/${hash}`)}`, {
        method: "PUT",
        body: bytes,
        headers: { authorization: `Bearer ${TOKEN}`, "content-length": String(bytes.length) },
      }),
      relayEnv,
    );
    expect(res.status).toBe(201);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.body).toBeInstanceOf(ReadableStream);
    expect(calls[0]?.headers).toContainEqual(["x-utopia-blob-size", String(bytes.length)]);
  });

  it("neither the Worker nor the object buffers a body with a Content-Length", async () => {
    // Every buffering read a Request offers, recorded while one upload runs end to end.
    // The test, the Worker and the Durable Object share one isolate in this runner.
    const graph = new Graph();
    const bytes = pseudoRandom(2 * MIB, 33);
    const hash = await sha256Hex(bytes);
    const proto = Request.prototype as unknown as Record<string, unknown>;
    const methods = ["arrayBuffer", "bytes", "blob", "text", "json", "formData"];
    const saved = methods.map((m) => [m, proto[m]] as const);
    const buffered: string[] = [];
    for (const [name, original] of saved) {
      if (typeof original !== "function") continue;
      proto[name] = function (this: Request, ...args: unknown[]) {
        if (new URL(this.url).pathname.includes("blob")) buffered.push(name);
        return (original as (...a: unknown[]) => unknown).apply(this, args);
      };
    }
    try {
      expect((await graph.putBlob(hash, bytes)).status).toBe(201);
    } finally {
      for (const [name, original] of saved) proto[name] = original;
    }
    expect(buffered, "buffering reads of the upload").toEqual([]);
    const got = await graph.getBlob(hash);
    expect(await sha256Hex(new Uint8Array(await got.arrayBuffer()))).toBe(hash);
  });

  it("a body without a Content-Length is buffered once and still round-trips", async () => {
    const graph = new Graph();
    const chunks = 6;
    const expected = new Uint8Array(chunks * MIB);
    for (let i = 0; i < chunks; i++) expected.set(pseudoRandom(MIB, 40 + i), i * MIB);
    const hash = await sha256Hex(expected);
    const res = await graph.call(`blobs/${hash}`, {
      method: "PUT",
      body: chunkedBody(chunks, MIB, 40),
    });
    expect(res.status).toBe(201);
    const got = await graph.getBlob(hash);
    expect(bytesEqual(new Uint8Array(await got.arrayBuffer()), expected)).toBe(true);
    expect(await blobSizes(graph)).toEqual([chunks * MIB]);
  });

  it("a chunked body over the limit is 413 and stores nothing", async () => {
    const graph = new Graph();
    const hash = "d".repeat(64);
    const res = await graph.call(`blobs/${hash}`, {
      method: "PUT",
      body: chunkedBody(65, MIB, 50),
    });
    expect(res.status).toBe(413);
    expect(await res.text()).toBe("payload too large");
    expect(await blobSizes(graph)).toEqual([]);
    expect(await env.BLOBS.head(`graphs/${graph.gid}/blobs/${hash}`)).toBeNull();
  });

  it("a declared length over the limit is 413 before the object is reached", async () => {
    const { relayEnv, calls } = recordingEnv();
    const graph = new Graph();
    const res = await handle(
      new Request(`${BASE}${graph.url(`blobs/${"e".repeat(64)}`)}`, {
        method: "PUT",
        body: chunkedBody(1, 1024, 60),
        headers: {
          authorization: `Bearer ${TOKEN}`,
          "content-length": String(MAX_BODY_BYTES + 1),
        },
      }),
      relayEnv,
    );
    expect(res.status).toBe(413);
    expect(calls).toEqual([]);
  });

  it("the quota is checked against the declared size before a byte is stored", async () => {
    const { relayEnv } = recordingEnv({ QUOTA_BYTES: String(MIB) });
    const graph = new Graph();
    const bytes = pseudoRandom(2 * MIB, 34);
    const hash = await sha256Hex(bytes);
    const res = await handle(
      new Request(`${BASE}${graph.url(`blobs/${hash}`)}`, {
        method: "PUT",
        body: bytes,
        headers: { authorization: `Bearer ${TOKEN}`, "content-length": String(bytes.length) },
      }),
      relayEnv,
    );
    expect(res.status).toBe(413);
    expect(await res.text()).toBe("quota exceeded");
    expect(await blobSizes(graph)).toEqual([]);
    expect(await env.BLOBS.head(`graphs/${graph.gid}/blobs/${hash}`)).toBeNull();
  });

  it("a body shorter than its declared size is not indexed and frees its reservation", async () => {
    const graph = new Graph();
    const hash = "f".repeat(64);
    const res = await graph.direct(`/blob?hash=${hash}&quota=${10 * MIB}`, {
      method: "PUT",
      body: chunkedBody(1, 1000, 70),
      headers: { "x-utopia-blob-size": String(5000) },
    });
    expect(res.status).toBe(400);
    expect(await res.text()).toBe("attachment body is not the 5000 bytes it declared");
    expect(await blobSizes(graph)).toEqual([]);
    expect(await env.BLOBS.head(`graphs/${graph.gid}/blobs/${hash}`)).toBeNull();
    expect(await graph.inDo((instance) => instance["inFlightBlobBytes"])).toBe(0);
    // Longer than declared fails the same way.
    const long = await graph.direct(`/blob?hash=${hash}&quota=${10 * MIB}`, {
      method: "PUT",
      body: chunkedBody(2, 3000, 72),
      headers: { "x-utopia-blob-size": String(5000) },
    });
    expect(long.status).toBe(400);
    await long.text();
    expect(await blobSizes(graph)).toEqual([]);
    expect(await graph.inDo((instance) => instance["inFlightBlobBytes"])).toBe(0);
    // The queue is still alive, and the whole quota is still there for the next upload.
    const full = pseudoRandom(10 * MIB, 71);
    expect(
      (await graph.direct(`/blob?hash=${hash}&quota=${10 * MIB}`, { method: "PUT", body: full }))
        .status,
    ).toBe(201);
    expect(await blobSizes(graph)).toEqual([10 * MIB]);
  });
});
