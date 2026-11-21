// Large payloads: a Durable Object's SQLite rows are limited to about 2 MB (key plus
// value), so payloads are stored in 512 KiB chunks. These tests prove an encrypted
// snapshot of the size a real graph produces still round-trips byte for byte.

import { describe, expect, it } from "vitest";

import {
  Graph,
  bytesEqual,
  pseudoRandom,
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
