// Attachments at the relay (PRD §12.8): opaque bytes, content-addressed, write-once,
// stored in R2 and indexed in the graph's Durable Object for quota and lookups. The relay
// never checks the hash against the body — blobs may be ciphertext, and the client verifies
// after decrypting (`tampered_blob_is_rejected` in `crates/utopia-syncd/tests/http.rs`).

import { env } from "cloudflare:test";
import { describe, expect, it } from "vitest";

import { Graph, bytesEqual, decode, encode, pseudoRandom, sha256Hex } from "./helpers";

async function r2Object(gid: string, hash: string) {
  return env.BLOBS.get(`graphs/${gid}/blobs/${hash}`);
}

describe("blobs", () => {
  it("are write-once, content-addressed and served from R2", async () => {
    const graph = new Graph();
    const bytes = encode("the real notes");
    const hash = await sha256Hex(bytes);

    const created = await graph.putBlob(hash, bytes);
    expect(created.status).toBe(201);

    const got = await graph.getBlob(hash);
    expect(got.status).toBe(200);
    expect(got.headers.get("content-type")).toBe("application/octet-stream");
    expect(bytesEqual(new Uint8Array(await got.arrayBuffer()), bytes)).toBe(true);

    // The bytes are in R2 under graphs/{gid}/blobs/{hash}, one object per attachment.
    const stored = await r2Object(graph.gid, hash);
    expect(stored).not.toBeNull();
    expect(decode((await stored?.arrayBuffer()) ?? new ArrayBuffer(0))).toBe(
      "the real notes",
    );

    // Write-once: the same hash with different bytes is accepted with 200 and not stored
    // over; the first writer wins.
    const again = await graph.putBlob(hash, encode("forged notes"));
    expect(again.status).toBe(200);
    const still = await graph.getBlob(hash);
    expect(bytesEqual(new Uint8Array(await still.arrayBuffer()), bytes)).toBe(true);
    expect(decode((await (await r2Object(graph.gid, hash))?.arrayBuffer()) ?? new ArrayBuffer(0))).toBe(
      "the real notes",
    );

    // Attachment writes do not touch the manifest revision (as in the Rust relay).
    expect((await graph.readManifest()).rev).toBe(0);
  });

  it("a missing blob is a 404 and stores nothing", async () => {
    const graph = new Graph();
    const hash = await sha256Hex(encode("never uploaded"));
    const missing = await graph.getBlob(hash);
    expect(missing.status).toBe(404);
    expect(await r2Object(graph.gid, hash)).toBeNull();
  });

  it("quota counts attachment sizes and refuses without storing anything", async () => {
    const graph = new Graph();
    const tooBig = pseudoRandom(20, 3);
    const hash = await sha256Hex(tooBig);
    const refused = await graph.direct(
      `/blob?hash=${hash}&owner=tester&quota=10`,
      { method: "PUT", body: tooBig },
    );
    expect(refused.status).toBe(413);
    expect(await refused.text()).toBe("quota exceeded");
    expect((await graph.getBlob(hash)).status).toBe(404);
    expect(await r2Object(graph.gid, hash)).toBeNull();

    // What does fit is stored; an existing hash is answered 200 even at a full quota,
    // because a replay writes no new bytes.
    const small = pseudoRandom(5, 4);
    const smallHash = await sha256Hex(small);
    expect(
      (
        await graph.direct(`/blob?hash=${smallHash}&owner=tester&quota=10`, {
          method: "PUT",
          body: small,
        })
      ).status,
    ).toBe(201);
    expect(
      (
        await graph.direct(`/blob?hash=${smallHash}&owner=tester&quota=0`, {
          method: "PUT",
          body: small,
        })
      ).status,
    ).toBe(200);
    expect((await graph.getBlob(smallHash)).status).toBe(200);
  });

  it("does not verify the hash against the body", async () => {
    // Blobs may be ciphertext: the relay stores what it is given, and the client verifies
    // after decrypting (`tampered_blob_is_rejected` in
    // `crates/utopia-syncd/tests/http.rs`). A relay that hashed the body would refuse
    // every encrypted attachment.
    const graph = new Graph();
    const claimed = await sha256Hex(encode("the real notes"));
    const forged = encode("forged notes");
    expect((await graph.putBlob(claimed, forged)).status).toBe(201);
    const got = await graph.getBlob(claimed);
    expect(bytesEqual(new Uint8Array(await got.arrayBuffer()), forged)).toBe(true);
  });

  it("concurrent uploads of one hash store the winner's bytes once", async () => {
    // Write-once has to survive two devices racing for the same hash (AGENTS.md rule 12: an
    // attachment is written once under its SHA-256). `putBlob` awaits R2 between its
    // existence check and its insert, and a Durable Object only closes its input gate for
    // storage operations, so a second request runs inside that await: both see no row, both
    // put bytes, one insert wins (201) and the other is a duplicate (200). The bytes served
    // must still be the 201 writer's — the duplicate overwriting R2 is exactly what
    // write-once forbids. The payloads are 1 MiB so the two requests reliably overlap.
    const graph = new Graph();
    const SIZE = 1 << 20;
    const wrong: number[] = [];
    for (let round = 0; round < 10; round++) {
      const first = pseudoRandom(SIZE, 100 + round);
      const second = pseudoRandom(SIZE, 200 + round);
      const hash = await sha256Hex(first);
      const [ra, rb] = await Promise.all([
        graph.putBlob(hash, first),
        graph.putBlob(hash, second),
      ]);
      expect([ra.status, rb.status].sort()).toEqual([200, 201]);
      const creator = ra.status === 201 ? first : second;
      const got = await graph.getBlob(hash);
      expect(got.status).toBe(200);
      if (!bytesEqual(new Uint8Array(await got.arrayBuffer()), creator)) {
        wrong.push(round);
      }
    }
    expect(wrong, "rounds whose GET served the duplicate's bytes").toEqual([]);

    // One index row per hash, each the size the 201 wrote; attachments never touch the
    // manifest revision.
    const sizes = await graph.inDo((_instance, state) =>
      state.storage.sql
        .exec<{ size: number }>("SELECT size FROM blob")
        .toArray()
        .map((row) => row.size),
    );
    expect(sizes).toEqual(new Array(10).fill(SIZE));
    expect((await graph.readManifest()).rev).toBe(0);
  });

  it("an object removed behind its index row is restored by a re-upload", async () => {
    // The deployment notes used to suggest an R2 lifecycle rule for `graphs/`. An expiry
    // rule cannot tell an abandoned attachment from one a live block still references, and
    // it removes the object while the Durable Object keeps the index row. The relay must
    // then not treat the row as "already stored": a GET is a 404, and the device that still
    // has the bytes must be able to put them back. (The relay cannot detect the loss on its
    // own; without a re-upload the attachment stays broken, which is why expiring a live
    // graph is still not recommended.)
    const graph = new Graph();
    const bytes = encode("a live attachment");
    const hash = await sha256Hex(bytes);
    expect((await graph.putBlob(hash, bytes)).status).toBe(201);

    // What a lifecycle rule does: the object goes, the index row stays.
    await env.BLOBS.delete(`graphs/${graph.gid}/blobs/${hash}`);
    expect((await graph.getBlob(hash)).status).toBe(404);

    // The re-upload writes the bytes again and is still a duplicate (200): the index row
    // and its quota charge are unchanged.
    expect((await graph.putBlob(hash, bytes)).status).toBe(200);
    const got = await graph.getBlob(hash);
    expect(got.status).toBe(200);
    expect(bytesEqual(new Uint8Array(await got.arrayBuffer()), bytes)).toBe(true);
    const stored = await r2Object(graph.gid, hash);
    expect(decode((await stored?.arrayBuffer()) ?? new ArrayBuffer(0))).toBe(
      "a live attachment",
    );

    // Bytes that are present are still never replaced: write-once, not last-writer-wins.
    expect((await graph.putBlob(hash, encode("forged notes"))).status).toBe(200);
    expect(
      decode((await (await r2Object(graph.gid, hash))?.arrayBuffer()) ?? new ArrayBuffer(0)),
    ).toBe("a live attachment");
  });

  it("is deduped per graph, not across graphs", async () => {
    const a = new Graph();
    const b = new Graph();
    const bytes = encode("shared attachment");
    const hash = await sha256Hex(bytes);
    expect((await a.putBlob(hash, bytes)).status).toBe(201);
    expect((await b.getBlob(hash)).status).toBe(404);
    expect((await b.putBlob(hash, bytes)).status).toBe(201);
    expect(decode((await (await b.getBlob(hash)).arrayBuffer()) ?? new ArrayBuffer(0))).toBe(
      "shared attachment",
    );
  });
});
