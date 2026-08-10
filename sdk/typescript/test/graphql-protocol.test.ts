/**
 * Unit tests for the `graphql-transport-ws` message-handling core
 * (`SubscriptionManager`, src/graphql-protocol.ts), driven directly with
 * fake parsed JSON messages — no socket, no server.
 *
 * This is deliberately not an integration test against a real WebSocket.
 * Node 18+ ships a WebSocket *client* but no WebSocket *server*, so testing
 * the protocol end-to-end would need either a dev-only `ws`-server dependency
 * or Bun's built-in `Bun.serve({ websocket })` (this repo's tests already run
 * under `bun test`, so that would work too — see test/client.test.ts, which
 * does use it for `Client.exec`'s sequencing). But `client.ts` was written so
 * the actual message dispatch — subscribe queuing before the ack, next/error/
 * complete routing by id, ping/pong, close semantics — lives entirely in this
 * one dependency-free, socket-free class. Testing it directly here is more
 * precise than going through a real socket (no timing/flakiness) and needs no
 * server of any kind, so it's the primary coverage for the protocol; the
 * Bun.serve-based test covers that `Client` actually wires a real socket to
 * it correctly end to end.
 */
import { describe, expect, test } from "bun:test";
import { AuthError, GraphQLError } from "../src/errors.js";
import { SubscriptionManager } from "../src/graphql-protocol.js";

describe("SubscriptionManager", () => {
  test("queues the subscribe frame until connection_ack, then flushes it", () => {
    const sent: any[] = [];
    const mgr = new SubscriptionManager((msg) => sent.push(msg));

    const id = mgr.start("subscription{x}", { a: 1 }, {});
    expect(sent).toEqual([]);

    mgr.handleMessage({ type: "connection_ack" });
    expect(sent).toEqual([
      { id, type: "subscribe", payload: { query: "subscription{x}", variables: { a: 1 } } },
    ]);
  });

  test("sends the subscribe frame immediately once already acked", () => {
    const sent: any[] = [];
    const mgr = new SubscriptionManager((msg) => sent.push(msg));
    mgr.handleMessage({ type: "connection_ack" });

    const id = mgr.start("subscription{x}", {}, {});
    expect(sent).toEqual([
      { id, type: "subscribe", payload: { query: "subscription{x}", variables: {} } },
    ]);
  });

  test("routes next/error/complete to the subscription with the matching id", () => {
    const mgr = new SubscriptionManager(() => {});
    mgr.handleMessage({ type: "connection_ack" });

    const seenA: any[] = [];
    const seenB: any[] = [];
    const idA = mgr.start("A", {}, { onNext: (d) => seenA.push(d) });
    const idB = mgr.start("B", {}, { onNext: (d) => seenB.push(d) });

    mgr.handleMessage({ type: "next", id: idA, payload: { data: { a: 1 } } });
    mgr.handleMessage({ type: "next", id: idB, payload: { data: { b: 2 } } });

    expect(seenA).toEqual([{ a: 1 }]);
    expect(seenB).toEqual([{ b: 2 }]);
  });

  test("an error frame delivers a GraphQLError joining every message and drops the sub", () => {
    const mgr = new SubscriptionManager(() => {});
    mgr.handleMessage({ type: "connection_ack" });

    let err: Error | undefined;
    const id = mgr.start("A", {}, { onError: (e) => (err = e) });
    mgr.handleMessage({
      type: "error",
      id,
      payload: [{ message: "boom" }, { message: "again" }],
    });

    expect(err).toBeInstanceOf(GraphQLError);
    expect(err?.message).toBe("boom; again");
    expect(mgr.size).toBe(0);
  });

  test("a complete frame calls onComplete and drops the sub", () => {
    const mgr = new SubscriptionManager(() => {});
    mgr.handleMessage({ type: "connection_ack" });

    let completed = false;
    const id = mgr.start("A", {}, { onComplete: () => (completed = true) });
    mgr.handleMessage({ type: "complete", id });

    expect(completed).toBe(true);
    expect(mgr.size).toBe(0);
  });

  test("a ping is answered with a pong", () => {
    const sent: any[] = [];
    const mgr = new SubscriptionManager((msg) => sent.push(msg));
    mgr.handleMessage({ type: "ping" });
    expect(sent).toEqual([{ type: "pong" }]);
  });

  test("stop() sends complete and drops the sub; returns false once already gone", () => {
    const sent: any[] = [];
    const mgr = new SubscriptionManager((msg) => sent.push(msg));
    mgr.handleMessage({ type: "connection_ack" });

    const id = mgr.start("A", {}, {});
    expect(mgr.stop(id)).toBe(true);
    expect(sent.at(-1)).toEqual({ id, type: "complete" });
    expect(mgr.size).toBe(0);
    expect(mgr.stop(id)).toBe(false);
  });

  test("stop() issued before the ack cancels the queued subscribe — only complete is ever sent", () => {
    const sent: any[] = [];
    const mgr = new SubscriptionManager((msg) => sent.push(msg));

    const id = mgr.start("A", {}, {});
    mgr.stop(id);
    mgr.handleMessage({ type: "connection_ack" });

    expect(sent).toEqual([{ id, type: "complete" }]);
  });

  test("handleClose() before any ack delivers AuthError to every tracked (pending or live) sub", () => {
    const mgr = new SubscriptionManager(() => {});
    let err: Error | undefined;
    mgr.start("A", {}, { onError: (e) => (err = e) }); // never acked, so still "pending"
    mgr.handleClose();

    expect(err).toBeInstanceOf(AuthError);
    expect(mgr.size).toBe(0);
  });

  test("handleClose() after an ack delivers a generic GraphQLError, not an AuthError", () => {
    const mgr = new SubscriptionManager(() => {});
    mgr.handleMessage({ type: "connection_ack" });

    let err: Error | undefined;
    mgr.start("A", {}, { onError: (e) => (err = e) });
    mgr.handleClose();

    expect(err).toBeInstanceOf(GraphQLError);
    expect(err).not.toBeInstanceOf(AuthError);
  });

  test("handleClose() resets state so a fresh connection starts clean", () => {
    const sent: any[] = [];
    const mgr = new SubscriptionManager((msg) => sent.push(msg));
    mgr.handleMessage({ type: "connection_ack" });
    mgr.start("A", {}, {});
    mgr.handleClose();

    expect(mgr.acked).toBe(false);

    // A subscribe started on the new connection must queue again, not send
    // immediately — proof `acked` really was reset, not just `size`.
    sent.length = 0;
    const id = mgr.start("B", {}, {});
    expect(sent).toEqual([]);
    mgr.handleMessage({ type: "connection_ack" });
    expect(sent).toEqual([{ id, type: "subscribe", payload: { query: "B", variables: {} } }]);
  });
});
