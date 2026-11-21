// Quota (§12.6, §12.8): the relay refuses new bytes over the per-graph limit, and a
// replayed batch is deduped before quota so a lost response cannot turn into a permanent
// 413. The client-side half of `quota_rejects_and_keeps_the_outbox` (the outbox keeps the
// edits) belongs to `utopia-sync`; what is pinned here is the relay's side.

import { describe, expect, it } from "vitest";

import type { PushResponse } from "../src/protocol";
import { PID, Graph, pseudoRandom, sha256Hex, update } from "./helpers";

/** The Durable Object's own interface, with a quota of this test's choosing. */
async function pushWithQuota(
  graph: Graph,
  updates: ReturnType<typeof update>[],
  quota: number,
  pid: string = PID,
): Promise<Response> {
  return graph.direct(
    `/push?pid=${pid}&quota=${quota}`,
    { method: "POST", body: JSON.stringify({ updates }) },
  );
}

async function putBlobWithQuota(
  graph: Graph,
  hash: string,
  bytes: Uint8Array,
  quota: number,
): Promise<Response> {
  return graph.direct(`/blob?hash=${hash}&quota=${quota}`, {
    method: "PUT",
    body: bytes,
  });
}

async function storedBytes(graph: Graph): Promise<number> {
  return graph.inDo((_instance, state) => {
    const row = state.storage.sql
      .exec<{ bytes: number }>(
        `SELECT ifnull((SELECT sum(payload_len) FROM page_log), 0)
              + ifnull((SELECT sum(payload_len) FROM page_snapshot), 0)
              + ifnull((SELECT sum(size) FROM blob), 0) AS bytes`,
      )
      .one();
    return row.bytes;
  });
}

describe("quota", () => {
  it("ac_12_6_a_replayed_batch_is_a_duplicate_even_at_the_quota", async () => {
    // §12.6: a batch the relay stored but whose response was lost is re-pushed. Dedupe
    // runs before quota, so its own bytes cannot make it a permanent 413.
    const graph = new Graph();
    const req = [update("client:1", "x".repeat(1000))];
    const first = await pushWithQuota(graph, req, 1500);
    expect((await first.json()) as PushResponse).toEqual({
      accepted: 1,
      head_seq: 1,
      duplicate: 0,
    });
    const used = await storedBytes(graph);
    expect(used).toBe(1000);

    // The response was lost: the same batch is pushed again when no new byte fits.
    const again = await pushWithQuota(graph, req, used);
    expect(again.status).toBe(200);
    expect((await again.json()) as PushResponse).toEqual({
      accepted: 0,
      head_seq: 1,
      duplicate: 1,
    });
    expect(await storedBytes(graph)).toBe(1000);

    // Genuinely new bytes are still refused.
    const fresh = await pushWithQuota(graph, [update("client:2", "y".repeat(10))], used);
    expect(fresh.status).toBe(413);
    expect(await fresh.text()).toBe("quota exceeded");
    expect(await storedBytes(graph)).toBe(1000);
    expect(await graph.logRows()).toHaveLength(1);
  });

  it("concurrent uploads cannot both fit a quota neither fits alone", async () => {
    // `putBlob` reads the graph's used bytes, awaits R2, and only then inserts the index row
    // that the quota counts. Two uploads racing through that await each see the same total
    // and both are admitted, storing twice the limit (§12.6). Serialised, the second sees
    // the first's row and is refused. The payloads are 1 MiB so the requests overlap.
    const SIZE = 1 << 20;
    const quota = SIZE + SIZE / 2;
    const overfull: number[] = [];
    for (let round = 0; round < 10; round++) {
      const graph = new Graph();
      const a = pseudoRandom(SIZE, 300 + round);
      const b = pseudoRandom(SIZE, 400 + round);
      const [ra, rb] = await Promise.all([
        putBlobWithQuota(graph, await sha256Hex(a), a, quota),
        putBlobWithQuota(graph, await sha256Hex(b), b, quota),
      ]);
      // Both bodies are read, or the runtime warns about an unread response.
      const [textA, textB] = [await ra.text(), await rb.text()];
      if ([ra.status, rb.status].sort().join() !== "201,413") {
        overfull.push(round);
      } else {
        expect(ra.status === 413 ? textA : textB).toBe("quota exceeded");
      }
      expect(await storedBytes(graph)).toBeLessThanOrEqual(quota);
      expect(await storedBytes(graph)).toBe(SIZE);
    }
    expect(overfull, "rounds that stored over the quota").toEqual([]);
  });

  it("the direct interface also answers 400 to a bad body, never 500", async () => {
    // The Worker validates before forwarding, so this only pins the Durable Object's own
    // contract (used by these tests for quota overrides).
    const graph = new Graph();
    const notJson = await graph.direct(`/push?pid=${PID}&quota=1000`, {
      method: "POST",
      body: "{",
    });
    expect(notJson.status).toBe(400);
    const badBase64 = await graph.direct(`/push?pid=${PID}&quota=1000`, {
      method: "POST",
      body: JSON.stringify({
        updates: [{ id: "c:1", client_seq: 1, frontier: "", payload: "!!!" }],
      }),
    });
    expect(badBase64.status).toBe(400);
    const badSnapshot = await graph.direct(`/snapshot?pid=${PID}`, {
      method: "POST",
      body: JSON.stringify({ covers_seq: 0, payload: "!!!" }),
    });
    expect(badSnapshot.status).toBe(400);
    expect(await graph.logRows()).toEqual([]);
  });

  it("quota_rejects_and_keeps_the_outbox (relay side)", async () => {
    // The relay answers 413 and stores nothing; the client keeps the edits queued
    // (`SyncError::Transport` with 413, `quota_rejects_and_keeps_the_outbox` in
    // `crates/utopia-syncd/tests/contract/mod.rs`).
    const graph = new Graph();
    const res = await pushWithQuota(graph, [update("big:1", "x".repeat(500))], 64);
    expect(res.status).toBe(413);
    expect(await res.text()).toBe("quota exceeded");

    expect(await graph.logRows()).toEqual([]);
    const manifest = await graph.readManifest();
    expect(manifest.pages).toEqual([]);
    expect(manifest.rev).toBe(0);
    expect(await storedBytes(graph)).toBe(0);
  });

  it("ac_12_6_a_lost_ack_is_replayed_not_rejected_by_quota", async () => {
    // §12.6, the relay's side: a batch whose response was lost fills the quota exactly;
    // its replay is acknowledged and charged nothing, while a new byte is refused.
    const graph = new Graph();
    const batch = [update("deviceA:1", "a".repeat(400)), update("deviceA:2", "b".repeat(400), 2)];
    const first = await pushWithQuota(graph, batch, 800);
    expect(first.status).toBe(200);
    const used = await storedBytes(graph);
    expect(used).toBe(800);

    const replay = await pushWithQuota(graph, batch, used);
    expect(replay.status).toBe(200);
    expect((await replay.json()) as PushResponse).toEqual({
      accepted: 0,
      head_seq: 2,
      duplicate: 2,
    });
    expect(await storedBytes(graph)).toBe(800);

    // One byte over is one byte over.
    expect((await pushWithQuota(graph, [update("deviceA:3", "c", 3)], used)).status).toBe(
      413,
    );
  });

  it("quota counts log bytes, snapshot bytes and attachment sizes together", async () => {
    const graph = new Graph();
    expect((await pushWithQuota(graph, [update("c:1", "x".repeat(100))], 1000)).status).toBe(
      200,
    );
    expect(await storedBytes(graph)).toBe(100);
    // A snapshot is stored (the Rust relay does not quota-check `put_snapshot`); it also
    // truncates the log rows it covers, and its own bytes count against the graph.
    expect((await graph.snapshot(1, "s".repeat(500))).status).toBe(204);
    expect(await storedBytes(graph)).toBe(500);

    // 500 used of 1000: a 600-byte attachment does not fit, a 400-byte one does.
    const hash = "b".repeat(64);
    expect((await putBlobWithQuota(graph, hash, pseudoRandom(600), 1000)).status).toBe(413);
    expect(await storedBytes(graph)).toBe(500);
    expect((await putBlobWithQuota(graph, hash, pseudoRandom(400), 1000)).status).toBe(201);
    expect(await storedBytes(graph)).toBe(900);
    // ... and then only 100 bytes are left.
    const otherHash = "c".repeat(64);
    expect((await putBlobWithQuota(graph, otherHash, pseudoRandom(200), 1000)).status).toBe(
      413,
    );
    expect((await putBlobWithQuota(graph, otherHash, pseudoRandom(100), 1000)).status).toBe(
      201,
    );
    expect(await storedBytes(graph)).toBe(1000);
  });

  it("snapshots are not quota-checked, as in the Rust relay", async () => {
    // `utopia_syncd::Db::put_snapshot` stores without consulting the quota; the bytes it
    // adds are counted afterwards (see the previous test). Parity over preference: a
    // client that compacts its own log must always be able to.
    const graph = new Graph();
    await pushWithQuota(graph, [update("c:1", "x".repeat(10))], 50);
    const big = pseudoRandom(4096);
    expect((await graph.snapshot(1, big)).status).toBe(204);
    expect((await storedBytes(graph))).toBe(4096);
  });
});
