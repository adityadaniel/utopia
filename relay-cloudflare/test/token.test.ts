// The bearer token is a secret: it authenticates a request at the Worker and goes no
// further. It must not appear in the URL or headers of the Worker's request to a graph's
// Durable Object (those land in request traces and logs), and the object must not store it.
// The relay has no per-owner logic, so nothing needs it past `authorize`.

import { describe, expect, it } from "vitest";

import { handle } from "../src/relay";
import {
  BASE,
  Graph,
  encode,
  recordingEnv,
  sha256Hex,
  update,
  zstdBody,
} from "./helpers";

/** Every user table of a graph's object, as text: blobs read as Latin-1 so bytes match. */
async function storageText(graph: Graph): Promise<string> {
  return graph.inDo((_instance, state) => {
    const sql = state.storage.sql;
    const tables = sql
      .exec<{ name: string }>(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '_cf_%' " +
          "AND name NOT LIKE 'sqlite_%'",
      )
      .toArray()
      .map((row) => row.name);
    expect(tables).toContain("graph");
    const parts: string[] = [];
    for (const table of tables) {
      for (const row of sql.exec(`SELECT * FROM "${table}"`).toArray()) {
        for (const value of Object.values(row)) {
          parts.push(
            value instanceof ArrayBuffer
              ? String.fromCharCode(...new Uint8Array(value).subarray(0, 4096))
              : String(value),
          );
        }
      }
    }
    return parts.join("\n");
  });
}

describe("the bearer token", () => {
  it("never reaches the Durable Object's URL, headers or storage", async () => {
    const token = `canary-token-${crypto.randomUUID()}`;
    const { relayEnv, calls } = recordingEnv({ RELAY_TOKENS: `other-token,${token}` });
    const graph = new Graph();
    const auth = { authorization: `Bearer ${token}` };
    const send = (path: string, init: RequestInit = {}) =>
      handle(
        new Request(`${BASE}${graph.url(path)}`, {
          ...init,
          headers: { ...Object.fromEntries(new Headers(init.headers)), ...auth },
        }),
        relayEnv,
      );

    // Every route that reaches the object: push, pull, snapshot, manifest, blob PUT/GET.
    const push = zstdBody({ updates: [update("c:1", "hello"), update("c:2", "world", 2)] });
    const pushed = await send("pages/page-1/updates", {
      method: "POST",
      body: push.body,
      headers: push.headers,
    });
    expect(pushed.status).toBe(200);
    expect((await send("pages/page-1/updates?since=0")).status).toBe(200);
    const snap = zstdBody({ covers_seq: 1, payload: "c25hcA==" });
    expect(
      (await send("pages/page-1/snapshot", {
        method: "POST",
        body: snap.body,
        headers: snap.headers,
      })).status,
    ).toBe(204);
    expect((await send("manifest")).status).toBe(200);
    const bytes = encode("an attachment");
    const hash = await sha256Hex(bytes);
    // Declared length, as the Rust client always sends it.
    expect(
      (await send(`blobs/${hash}`, {
        method: "PUT",
        body: bytes,
        headers: { "content-length": String(bytes.length) },
      })).status,
    ).toBe(201);
    // And without one (a chunked body), which takes the buffered path.
    const other = encode("another attachment");
    expect(
      (await send(`blobs/${await sha256Hex(other)}`, {
        method: "PUT",
        body: new Blob([other]).stream(),
      })).status,
    ).toBe(201);
    const got = await send(`blobs/${hash}`);
    expect(got.status).toBe(200);
    await got.arrayBuffer();

    expect(calls.length).toBeGreaterThanOrEqual(7);
    for (const call of calls) {
      expect(call.url, "an internal URL carries the token").not.toContain(token);
      for (const [name, value] of call.headers) {
        expect(`${name}: ${value}`, "an internal header carries the token").not.toContain(
          token,
        );
      }
    }
    expect(await storageText(graph), "the object stored the token").not.toContain(token);
  });
});
