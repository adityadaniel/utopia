// Unit tests for the pieces the Worker decides before a Durable Object is touched: auth
// policy, id/hash validation, routing, the body limit and the wire codec. These need no
// storage, so they call the functions directly.

import { describe, expect, it } from "vitest";

import type { Env } from "../src/graph";
import {
  MAX_BODY_BYTES,
  acceptedTokens,
  authorize,
  bodyTooLarge,
  parseRoute,
  quotaFrom,
  sinceFrom,
  validHash,
  validId,
} from "../src/relay";
import { b64decode, b64encode, BadRequest, parsePushRequest } from "../src/protocol";

function env(vars: Partial<Env> = {}): Env {
  return vars as Env;
}

function headers(init: Record<string, string> = {}): Headers {
  return new Headers(init);
}

describe("auth policy", () => {
  it("accepts only a configured bearer token, and identifies the caller by it", () => {
    const tokens = env({ RELAY_TOKENS: "alpha,beta" });
    expect(authorize(tokens, headers({ authorization: "Bearer alpha" }))).toEqual({
      ok: true,
      owner: "alpha",
    });
    expect(authorize(tokens, headers({ authorization: "Bearer beta" }))).toEqual({
      ok: true,
      owner: "beta",
    });
    for (const header of [undefined, "", "alpha", "Bearer", "Bearer gamma", "bearer alpha"]) {
      const h = headers(header === undefined ? {} : { authorization: header });
      expect(authorize(tokens, h)).toEqual({ ok: false });
    }
    // Whitespace around a comma-separated list is not part of a token.
    expect(acceptedTokens(env({ RELAY_TOKENS: " alpha , beta ," }))).toEqual([
      "alpha",
      "beta",
    ]);
  });

  it("an empty token list is auth off only with ALLOW_ANONYMOUS=1", () => {
    // A deliberate difference from utopia-syncd, whose empty list disables auth: a hosted
    // relay that silently accepts everyone because a secret failed to load is worse than
    // one that refuses everyone until it is configured.
    expect(authorize(env({ RELAY_TOKENS: "" }), headers())).toEqual({ ok: false });
    expect(authorize(env({}), headers())).toEqual({ ok: false });
    expect(
      authorize(env({ RELAY_TOKENS: "", ALLOW_ANONYMOUS: "0" }), headers()),
    ).toEqual({ ok: false });
    expect(
      authorize(env({ RELAY_TOKENS: "", ALLOW_ANONYMOUS: "1" }), headers()),
    ).toEqual({ ok: true, owner: "anonymous" });
    // Configured tokens win over anonymous mode, even with no Authorization header.
    expect(
      authorize(
        env({ RELAY_TOKENS: "alpha", ALLOW_ANONYMOUS: "1" }),
        headers({ authorization: "Bearer alpha" }),
      ),
    ).toEqual({ ok: true, owner: "alpha" });
    expect(
      authorize(env({ RELAY_TOKENS: "alpha", ALLOW_ANONYMOUS: "1" }), headers()),
    ).toEqual({ ok: false });
  });
});

describe("validation helpers", () => {
  it("ids are 1–128 characters of [A-Za-z0-9_-], hashes 64 lowercase hex", () => {
    expect(validId("abc-DEF_123")).toBe(true);
    expect(validId("x".repeat(128))).toBe(true);
    expect(validId("")).toBe(false);
    expect(validId("x".repeat(129))).toBe(false);
    expect(validId("a b")).toBe(false);
    expect(validId("a/b")).toBe(false);
    expect(validId("é")).toBe(false);

    expect(validHash("a".repeat(64))).toBe(true);
    expect(validHash("A".repeat(64))).toBe(false);
    expect(validHash("a".repeat(63))).toBe(false);
    expect(validHash("a".repeat(65))).toBe(false);
    expect(validHash("g".repeat(64))).toBe(false);
  });

  it("the body limit is 64 MiB plus the encryption envelope", () => {
    expect(MAX_BODY_BYTES).toBe((64 << 20) + (64 << 10));
    expect(bodyTooLarge(null)).toBe(false);
    expect(bodyTooLarge("")).toBe(false);
    expect(bodyTooLarge("not a number")).toBe(false);
    expect(bodyTooLarge(String(MAX_BODY_BYTES))).toBe(false);
    expect(bodyTooLarge(String(MAX_BODY_BYTES + 1))).toBe(true);
  });

  it("the quota falls back to 1 GiB, and the since cursor is a u64 or nothing", () => {
    expect(quotaFrom(env({}))).toBe(1 << 30);
    expect(quotaFrom(env({ QUOTA_BYTES: "" }))).toBe(1 << 30);
    expect(quotaFrom(env({ QUOTA_BYTES: "  " }))).toBe(1 << 30);
    expect(quotaFrom(env({ QUOTA_BYTES: "not a number" }))).toBe(1 << 30);
    expect(quotaFrom(env({ QUOTA_BYTES: "-1" }))).toBe(1 << 30);
    expect(quotaFrom(env({ QUOTA_BYTES: "0" }))).toBe(0);
    expect(quotaFrom(env({ QUOTA_BYTES: "12345" }))).toBe(12345);

    const query = (q: string) => sinceFrom(new URL(`https://relay.test/x${q}`));
    expect(query("")).toBe(0);
    expect(query("?since=0")).toBe(0);
    expect(query("?since=42")).toBe(42);
    for (const bad of ["?since=", "?since=-1", "?since=1.5", "?since=abc"]) {
      expect(() => query(bad)).toThrow(BadRequest);
    }
  });

  it("routes claim exactly the paths utopia-syncd's router does", () => {
    expect(parseRoute("/healthz")).toEqual({ kind: "healthz" });
    expect(parseRoute("/healthz/")).toBeNull();
    expect(parseRoute("/v1/graphs/g/manifest")).toEqual({
      kind: "manifest",
      gid: "g",
    });
    expect(parseRoute("/v1/graphs/g/pages/p/updates")).toEqual({
      kind: "updates",
      gid: "g",
      pid: "p",
    });
    expect(parseRoute("/v1/graphs/g/pages/p/snapshot")).toEqual({
      kind: "snapshot",
      gid: "g",
      pid: "p",
    });
    expect(parseRoute("/v1/graphs/g/events")).toEqual({ kind: "events", gid: "g" });
    expect(parseRoute(`/v1/graphs/g/blobs/${"a".repeat(64)}`)).toEqual({
      kind: "blob",
      gid: "g",
      hash: "a".repeat(64),
    });
    expect(parseRoute("/v1/graphs/g/unknown")).toBeNull();
    expect(parseRoute("/v1/graphs/g/pages/p/updates/more")).toBeNull();
    expect(parseRoute("/")).toBeNull();
    // Percent-encoded path segments are decoded before validation.
    expect(parseRoute("/v1/graphs/bad%20id/manifest")).toEqual({
      kind: "manifest",
      gid: "bad id",
    });
  });
});

describe("wire codec", () => {
  it("base64 matches utopia_proto::encode/decode, including across chunk boundaries", () => {
    // The Rust encoder pads and decodes unpadded input.
    expect(b64encode(new TextEncoder().encode("Man"))).toBe("TWFu");
    expect(b64encode(new TextEncoder().encode("Ma"))).toBe("TWE=");
    expect(new TextDecoder().decode(b64decode("TWE"))).toBe("Ma");
    // Large payloads are encoded in chunks (the 5 MiB snapshot tests cover the storage
    // path; this pins the boundary arithmetic).
    for (const size of [0, 1, 2, 3, 8189, 8190, 8191, 16380, 100000]) {
      const bytes = new Uint8Array(size);
      for (let i = 0; i < size; i++) bytes[i] = (i * 31 + 7) & 0xff;
      const round = b64decode(b64encode(bytes));
      expect(round.length).toBe(size);
      expect(Array.from(round.slice(0, 16))).toEqual(Array.from(bytes.slice(0, 16)));
      expect(Array.from(round.slice(-16))).toEqual(Array.from(bytes.slice(-16)));
    }
  });

  it("a push request with a missing or mistyped field is a 400, not a crash", () => {
    const bad: unknown[] = [
      {},
      { updates: null },
      { updates: [{}] },
      { updates: [{ id: 1, client_seq: 1, frontier: "", payload: "" }] },
      { updates: [{ id: "a", client_seq: 1.5, frontier: "", payload: "" }] },
      { updates: [{ id: "a", client_seq: 1, frontier: "", payload: "!" }] },
    ];
    for (const value of bad) {
      expect(() => parsePushRequest(value)).toThrow(BadRequest);
    }
    expect(
      parsePushRequest({
        updates: [{ id: "a:1", client_seq: 1, frontier: "", payload: "" }],
      }),
    ).toEqual({
      updates: [{ id: "a:1", client_seq: 1, frontier: "", payload: "" }],
    });
  });
});
