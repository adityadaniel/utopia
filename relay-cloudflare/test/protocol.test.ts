// Protocol behaviour (PRD §12.3, §12.6, §12.8), ported from
// `crates/utopia-syncd/tests/http.rs` and `utopia_syncd::Db`'s own tests.

import { describe, expect, it } from "vitest";

import type { Manifest, PushResponse } from "../src/protocol";
import {
  OTHER_TOKEN,
  PID,
  TOKEN,
  Graph,
  b64,
  call,
  decode,
  encode,
  update,
  unb64,
  zstdBody,
} from "./helpers";

describe("routes", () => {
  it("GET /healthz answers ok without auth, and unknown routes are 404 or 405", async () => {
    const health = await call("/healthz", {}, null);
    expect(health.status).toBe(200);
    expect(await health.text()).toBe("ok");

    const graph = new Graph();
    // Wrong method: the path exists but claims another verb.
    expect((await call("/healthz", { method: "POST" }, null)).status).toBe(405);
    expect((await graph.call("manifest", { method: "POST" })).status).toBe(405);
    expect((await graph.call(`pages/${PID}/snapshot`, { method: "PUT" })).status).toBe(405);
    expect((await graph.call(`blobs/${"a".repeat(64)}`, { method: "POST" })).status).toBe(
      405,
    );

    // Unknown paths.
    expect((await call("/nope")).status).toBe(404);
    expect((await call("/v1/graphs")).status).toBe(404);
    expect((await graph.call(`pages/${PID}/updates/extra`)).status).toBe(404);
    expect((await graph.call(`pages/${PID}`)).status).toBe(404);
    expect((await graph.call("snapshot")).status).toBe(404);

    // HEAD is answered by the GET handlers, as axum's router does, without a body.
    await graph.pushOutcome([update("c:1", "x")]);
    const head = await graph.call("manifest", { method: "HEAD" });
    expect(head.status).toBe(200);
    expect(head.headers.get("etag")).toBe('"1"');
    expect(await head.text()).toBe("");
  });

  it("an /events GET that is not a websocket upgrade is refused", async () => {
    const graph = new Graph();
    const res = await graph.call("events");
    expect(res.status).toBe(400);
  });
});

describe("auth", () => {
  it("auth_is_enforced", async () => {
    const graph = new Graph();
    // Every graph route needs a token; the failure is a plain-text 401.
    for (const token of [null, "wrong"] as const) {
      const res = await graph.manifest({}, token);
      expect(res.status).toBe(401);
      expect(await res.text()).toBe("unauthorized");
      expect((await graph.call(`pages/${PID}/updates`, { method: "POST" }, token)).status).toBe(
        401,
      );
    }
    // A refused request wrote nothing.
    const manifest = await graph.readManifest();
    expect(manifest.pages).toEqual([]);
    expect(manifest.rev).toBe(0);

    // Either configured token is accepted.
    for (const token of [TOKEN, OTHER_TOKEN]) {
      expect((await graph.manifest({}, token)).status).toBe(200);
    }
    expect((await graph.pushOutcome([update("c1:1", "hello")])).accepted).toBe(1);
  });

  it("a token list with no bearer is unauthenticated, never anonymous by accident", async () => {
    // `authorize` refuses when RELAY_TOKENS is set and no bearer is sent — and when it is
    // empty, unless ALLOW_ANONYMOUS="1" (unit-tested in auth.test.ts).
    const graph = new Graph();
    expect(
      (await graph.call(`pages/${PID}/updates`, { method: "POST" }, null)).status,
    ).toBe(401);
  });
});

describe("validation", () => {
  it("ids and hashes are validated before the Durable Object is touched", async () => {
    const badId = await call("/v1/graphs/bad%20id/manifest");
    expect(badId.status).toBe(400);
    expect(await badId.text()).toBe('invalid id "bad id"');

    const tooLong = await call(`/v1/graphs/${"x".repeat(129)}/manifest`);
    expect(tooLong.status).toBe(400);

    const graph = new Graph();
    const badPid = await graph.call("pages/bad%2Fpid/updates");
    expect(badPid.status).toBe(400);
    expect(await badPid.text()).toBe('invalid id "bad/pid"');

    const badHash = await graph.call("blobs/xyz");
    expect(badHash.status).toBe(400);
    expect(await badHash.text()).toBe('invalid blob hash "xyz"');
    expect((await graph.call(`blobs/${"A".repeat(64)}`)).status).toBe(400);

    // Auth runs first: an invalid id without a token is 401, not 400.
    expect((await call("/v1/graphs/bad%20id/manifest", {}, null)).status).toBe(401);
  });

  it("bad JSON, bad zstd and bad field types are 400s, never 500s", async () => {
    const graph = new Graph();
    const post = (init: RequestInit) =>
      graph.call(`pages/${PID}/updates`, { method: "POST", ...init });

    expect(
      (await post({ headers: { "content-type": "application/json" }, body: "{" })).status,
    ).toBe(400);
    expect(
      (await post({ headers: { "content-encoding": "zstd" }, body: encode("not zstd") }))
        .status,
    ).toBe(400);
    expect(
      (await post({ body: JSON.stringify({ updates: "nope" }) })).status,
    ).toBe(400);
    expect(
      (
        await post({
          body: JSON.stringify({ updates: [{ id: "c1:1", frontier: "" }] }),
        })
      ).status,
    ).toBe(400);
    expect(
      (
        await post({
          body: JSON.stringify({
            updates: [{ id: "c1:1", client_seq: 1, frontier: "", payload: "!!!" }],
          }),
        })
      ).status,
    ).toBe(400);

    expect((await graph.call(`pages/${PID}/updates?since=abc`)).status).toBe(400);
    expect((await graph.call(`pages/${PID}/updates?since=-1`)).status).toBe(400);
    expect(
      (
        await graph.call(`pages/${PID}/snapshot`, {
          method: "POST",
          body: JSON.stringify({ covers_seq: -1, payload: "" }),
        })
      ).status,
    ).toBe(400);

    // None of the refused requests left a row behind.
    expect(await graph.logRows()).toEqual([]);
  });

  it("a plain (uncompressed) body is accepted as well", async () => {
    const graph = new Graph();
    const res = await graph.call(`pages/${PID}/updates`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ updates: [update("plain:1", "plain")] }),
    });
    expect(res.status).toBe(200);
    expect((await res.json()) as PushResponse).toMatchObject({ accepted: 1 });
  });
});

describe("manifest and push (§12.3)", () => {
  it("manifest_is_conditional_and_pushes_are_idempotent", async () => {
    const graph = new Graph();
    const payload = "x".repeat(100);
    expect(await graph.pushOutcome([update("client:1", payload)])).toEqual({
      accepted: 1,
      head_seq: 1,
      duplicate: 0,
    });

    const res = await graph.manifest();
    expect(res.status).toBe(200);
    const etag = res.headers.get("etag");
    expect(etag).toBe('"1"');
    const manifest = (await res.json()) as Manifest;
    expect(manifest.protocol_version).toBe(1);
    expect(manifest.rev).toBe(1);
    expect(manifest.pages).toHaveLength(1);
    expect(manifest.pages[0]).toMatchObject({
      page_id: PID,
      head_seq: 1,
      frontier: "",
      snapshot_seq: 0,
      tail_bytes: 100,
      deleted: false,
    });
    expect(manifest.pages[0]?.updated_at).toBeGreaterThan(0);
    // This relay store's identity: 32 hex characters, the shape Rust's randomblob(16) has.
    expect(manifest.instance).toMatch(/^[0-9a-f]{32}$/);

    // Conditional GET: the same rev is 304 with no body.
    const notModified = await graph.manifest({ "if-none-match": etag ?? "" });
    expect(notModified.status).toBe(304);
    expect(await notModified.text()).toBe("");

    // Another rev re-sends the manifest.
    expect((await graph.manifest({ "if-none-match": '"999"' })).status).toBe(200);

    // The pulled update is the pushed one.
    const pulled = await graph.pull(0);
    expect(pulled.head_seq).toBe(1);
    expect(pulled.updates).toHaveLength(1);
    expect(pulled.updates[0]).toMatchObject({ seq: 1, id: "client:1", frontier: "" });
    expect(decode(unb64(pulled.updates[0]?.payload ?? ""))).toBe(payload);
    // No snapshot was stored, so the field is omitted, as serde does.
    expect("snapshot" in pulled).toBe(false);

    // The newest log row's frontier is the page's frontier in the manifest.
    await graph.push([
      { id: "client:2", client_seq: 2, frontier: "cafe", payload: b64(encode("y")) },
    ]);
    expect((await graph.readManifest()).pages[0]?.frontier).toBe("cafe");

    // Pushing the same change id again is a duplicate, not a second row.
    const again = await graph.push([
      { id: "client:1", client_seq: 3, frontier: "ab", payload: b64(encode("other")) },
    ]);
    expect((await again.json()) as PushResponse).toEqual({
      accepted: 0,
      head_seq: 2,
      duplicate: 1,
    });
    const unchanged = await graph.readManifest();
    expect(unchanged.rev).toBe(2);
    expect(unchanged.pages[0]?.tail_bytes).toBe(101);
  });

  it("ac_12_3_a_change_id_repeated_in_one_batch_is_a_duplicate", async () => {
    // §12.3 step 2: the same new change id twice in one request is accepted once, counted
    // as a duplicate and charged to quota once — a second insert would break the UNIQUE
    // index and fail the whole request.
    const graph = new Graph();
    const one = update("client:1", "y".repeat(100));
    expect(await graph.pushOutcome([one, { ...one, client_seq: 2 }])).toEqual({
      accepted: 1,
      head_seq: 1,
      duplicate: 1,
    });
    expect((await graph.readManifest()).pages[0]?.tail_bytes).toBe(100);
    expect((await graph.pull(0)).updates).toHaveLength(1);
  });

  it("seqs continue from the snapshot head, and pages have their own sequences", async () => {
    const graph = new Graph();
    await graph.push([update("p1:1", "a"), update("p1:2", "b", 2)]);
    expect(await graph.pushOutcome([update("p1:3", "c", 3)])).toEqual({
      accepted: 1,
      head_seq: 3,
      duplicate: 0,
    });
    expect(await graph.pushOutcome([update("p2:1", "z")], "page-2")).toEqual({
      accepted: 1,
      head_seq: 1,
      duplicate: 0,
    });
    // The snapshot's seq is the page's head when the log behind it was truncated.
    expect((await graph.snapshot(3, "snap")).status).toBe(204);
    expect(await graph.pushOutcome([update("p1:4", "d", 4)])).toEqual({
      accepted: 1,
      head_seq: 4,
      duplicate: 0,
    });
  });

  it("pulls only what the cursor has not seen", async () => {
    const graph = new Graph();
    await graph.push([update("c:1", "one"), update("c:2", "two", 2)]);
    expect((await graph.pull(1)).updates.map((u) => u.id)).toEqual(["c:2"]);
    expect((await graph.pull(2)).updates).toEqual([]);
    expect((await graph.pull(2)).head_seq).toBe(2);
    // `since` defaults to 0 when the parameter is absent.
    const res = await graph.call(`pages/${PID}/updates`);
    expect(((await res.json()) as { updates: unknown[] }).updates).toHaveLength(2);
  });
});

describe("snapshots (§11.7, §12.3 step 5)", () => {
  it("snapshots_compact_the_log_and_bootstrap_new_devices", async () => {
    const graph = new Graph();
    for (let i = 1; i <= 5; i++) {
      await graph.push([update(`c:${i}`, `rev ${i}`, i)]);
    }
    expect((await graph.snapshot(4, "snapshot-through-4")).status).toBe(204);

    // A device with no cursor gets the snapshot plus the tail.
    const fresh = await graph.pull(0);
    expect(fresh.snapshot).toBeDefined();
    expect(decode(unb64(fresh.snapshot ?? ""))).toBe("snapshot-through-4");
    expect(fresh.updates.map((u) => u.seq)).toEqual([5]);
    expect(fresh.head_seq).toBe(5);

    // A device already past the snapshot does not get it again.
    const incremental = await graph.pull(4);
    expect("snapshot" in incremental).toBe(false);
    expect(incremental.updates.map((u) => u.seq)).toEqual([5]);

    // Covered log rows are gone from storage, not just hidden.
    expect((await graph.logRows()).map((row) => row.seq)).toEqual([5]);

    const manifest = await graph.readManifest();
    expect(manifest.pages[0]).toMatchObject({
      head_seq: 5,
      snapshot_seq: 4,
      // Only the tail is charged, and it is the whole remaining log.
      tail_bytes: "rev 5".length,
    });
    // Five pushes and one stored snapshot.
    expect(manifest.rev).toBe(6);
  });

  it("refuses a snapshot covering unseen updates and ignores a stale one", async () => {
    const graph = new Graph();
    await graph.push([update("c:1", "one"), update("c:2", "two", 2)]);
    const unknown = await graph.snapshot(3, "too far");
    expect(unknown.status).toBe(400);
    expect(await unknown.text()).toBe("snapshot covers unknown updates");

    expect((await graph.snapshot(2, "at-head")).status).toBe(204);
    const revAfterStore = (await graph.readManifest()).rev;

    // The same coverage again, and an older one, are silent no-ops: no rev bump.
    expect((await graph.snapshot(2, "at-head-again")).status).toBe(204);
    expect((await graph.snapshot(1, "older")).status).toBe(204);
    const manifest = await graph.readManifest();
    expect(manifest.rev).toBe(revAfterStore);
    expect(manifest.pages[0]?.snapshot_seq).toBe(2);
    expect(decode(unb64((await graph.pull(0)).snapshot ?? ""))).toBe("at-head");
  });
});

describe("relay identity (§12.6)", () => {
  it("the instance id is stable across requests and distinct per graph", async () => {
    const graph = new Graph();
    const first = await graph.readManifest();
    const second = await graph.readManifest();
    expect(first.instance).toBeTruthy();
    expect(second.instance).toBe(first.instance);

    // A nearby graph id is another Durable Object: an empty log and its own instance.
    const other = await new Graph().readManifest();
    expect(other.instance).toBeTruthy();
    expect(other.instance).not.toBe(first.instance);
    expect(other.pages).toEqual([]);
    expect(other.rev).toBe(0);
  });

  it("a fresh graph's manifest has the fields serde emits and nothing else", async () => {
    const graph = new Graph();
    const raw = (await (await graph.manifest()).json()) as Record<string, unknown>;
    expect(Object.keys(raw).sort()).toEqual([
      "instance",
      "pages",
      "protocol_version",
      "rev",
    ]);
  });
});

describe("zstd bodies", () => {
  it("a compressed body is decompressed, and the stored payload is the plaintext", async () => {
    // Every client POST carries `Content-Encoding: zstd` (a repetitive body, so a server
    // that ignored the header would not see the plaintext at all).
    const graph = new Graph();
    const text = `line ${"abc ".repeat(4000)}`;
    const { body, headers } = zstdBody({ updates: [update("z:1", text)] });
    expect(body.length).toBeLessThan(text.length / 4);
    const pushed = await graph.call(`pages/${PID}/updates`, {
      method: "POST",
      body,
      headers,
    });
    expect(pushed.status).toBe(200);
    const pulled = await graph.pull(0);
    expect(decode(unb64(pulled.updates[0]?.payload ?? ""))).toBe(text);
    expect((await graph.readManifest()).pages[0]?.tail_bytes).toBe(text.length);

    // The same bytes without the header are not JSON: the header is what makes it zstd.
    const headerless = await graph.call(`pages/${PID}/updates`, {
      method: "POST",
      body,
      headers: { "content-type": "application/json" },
    });
    expect(headerless.status).toBe(400);
  });
});
