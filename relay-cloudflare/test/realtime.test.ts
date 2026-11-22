// Realtime (§12.3 "Realtime"): a WebSocket that only carries `{"page_id","head_seq"}`
// invalidation notices, through the Durable Object's Hibernation API.

import { describe, expect, it } from "vitest";

import { PID, Graph, update } from "./helpers";

function nextMessage(socket: WebSocket, timeoutMs = 4000): Promise<string> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("no notice arrived in time")),
      timeoutMs,
    );
    socket.addEventListener(
      "message",
      (event) => {
        clearTimeout(timer);
        resolve(typeof event.data === "string" ? event.data : "<binary>");
      },
      { once: true },
    );
  });
}

function noMessage(socket: WebSocket, ms = 250): Promise<boolean> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(true), ms);
    socket.addEventListener(
      "message",
      () => {
        clearTimeout(timer);
        resolve(false);
      },
      { once: true },
    );
  });
}

async function connect(graph: Graph): Promise<WebSocket> {
  const res = await graph.call("events", { headers: { upgrade: "websocket" } });
  expect(res.status).toBe(101);
  const socket = res.webSocket;
  if (socket === null) throw new Error("no websocket on a 101");
  socket.accept();
  return socket;
}

describe("realtime notices", () => {
  it("realtime_notice_on_push", async () => {
    const graph = new Graph();
    const socket = await connect(graph);
    const notice = nextMessage(socket);

    const pushed = await graph.pushOutcome([update("c:1", "hello")]);
    expect(pushed.accepted).toBe(1);

    expect(JSON.parse(await notice)).toEqual({ page_id: PID, head_seq: pushed.head_seq });
    socket.close(1000, "done");
  });

  it("sends nothing for a duplicate batch, and reaches every subscriber", async () => {
    const graph = new Graph();
    const first = await connect(graph);
    const second = await connect(graph);
    const firstNotice = nextMessage(first);
    const secondNotice = nextMessage(second);

    await graph.pushOutcome([update("c:1", "once")]);
    expect(JSON.parse(await firstNotice)).toEqual({ page_id: PID, head_seq: 1 });
    expect(JSON.parse(await secondNotice)).toEqual({ page_id: PID, head_seq: 1 });

    // A change id the relay already stored changes nothing, so it announces nothing.
    const quietFirst = noMessage(first);
    const quietSecond = noMessage(second);
    const duplicate = await graph.pushOutcome([update("c:1", "once")]);
    expect(duplicate.duplicate).toBe(1);
    expect(await quietFirst).toBe(true);
    expect(await quietSecond).toBe(true);

    // A new page's push carries that page's id and head.
    const other = nextMessage(first);
    await graph.pushOutcome([update("other:1", "x")], "page-2");
    expect(JSON.parse(await other)).toEqual({ page_id: "page-2", head_seq: 1 });

    first.close(1000, "done");
    second.close(1000, "done");
  });

  it("accepts sockets through the Hibernation API, not accept()", async () => {
    // `ctx.acceptWebSocket` is what lets the object be evicted while subscribers stay
    // attached: the sockets are registered with the runtime, not held in memory.
    const graph = new Graph();
    const socket = await connect(graph);
    const live = await graph.inDo((_instance, state) => state.getWebSockets().length);
    expect(live).toBe(1);
    socket.close(1000, "done");
    // Give the close callback a turn before the next assertion.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(await graph.inDo((_instance, state) => state.getWebSockets().length)).toBe(0);
    expect((await graph.pushOutcome([update("c:1", "after close")])).accepted).toBe(1);
  });

  it("the close handler never throws, whatever code the socket closed with", async () => {
    // 1005 (a close frame with no code) and 1006 (the TCP connection dropped, which is how
    // the Rust client's `realtime::Listener` stops) are reserved: they arrive at
    // `webSocketClose` but `WebSocket.close` refuses to send them. 1015 is the same.
    const graph = new Graph();
    const codes = [1000, 1001, 1005, 1006, 1011, 1015, 3000, 4999, 0, 999, 5000];
    const failures = await graph.inDo(async (instance, state) => {
      const failed: string[] = [];
      for (const code of codes) {
        for (const reason of ["", "x".repeat(200)]) {
          const pair = new WebSocketPair();
          const server = pair[1];
          state.acceptWebSocket(server);
          try {
            await instance.webSocketClose(server, code, reason, false);
          } catch (e) {
            failed.push(`${code}/${reason.length}: ${String(e)}`);
          }
          // A second call on the same, now closed, socket is harmless too.
          try {
            await instance.webSocketClose(server, code, reason, true);
          } catch (e) {
            failed.push(`${code}/${reason.length} again: ${String(e)}`);
          }
        }
      }
      return failed;
    });
    expect(failures).toEqual([]);
  });

  it("a client that closes without a code is let go, and the graph keeps working", async () => {
    // End to end: `close()` with no code sends a close frame without one, which reaches
    // the object as 1005. The runtime only logs what the handler throws, so the handler is
    // wrapped on this object's instance to see the code it got and anything it threw.
    const graph = new Graph();
    const seen: { code: number; error: string | null }[] = [];
    await graph.inDo((instance) => {
      const original = instance.webSocketClose.bind(instance);
      instance.webSocketClose = async (ws, code, reason, wasClean) => {
        try {
          await original(ws, code, reason, wasClean);
          seen.push({ code, error: null });
        } catch (e) {
          seen.push({ code, error: String(e) });
        }
      };
    });
    const socket = await connect(graph);
    const stays = await connect(graph);
    socket.close();
    for (let i = 0; i < 80 && seen.length === 0; i++) {
      await new Promise((resolve) => setTimeout(resolve, 25));
    }
    expect(seen).toEqual([{ code: 1005, error: null }]);
    expect(await graph.inDo((_instance, state) => state.getWebSockets().length)).toBe(1);
    const notice = nextMessage(stays);
    expect((await graph.pushOutcome([update("c:1", "after a bare close")])).accepted).toBe(1);
    expect(JSON.parse(await notice)).toEqual({ page_id: PID, head_seq: 1 });
    stays.close(1000, "done");
  });

  it("ignores messages the client sends", async () => {
    const graph = new Graph();
    const socket = await connect(graph);
    socket.send("hello from the client");
    // Still usable, and the object did not throw on the incoming frame.
    const notice = nextMessage(socket);
    await graph.pushOutcome([update("c:1", "x")]);
    expect(JSON.parse(await notice)).toMatchObject({ page_id: PID });
    socket.close(1000, "done");
  });
});
