// The message-handling core of a `graphql-transport-ws` client, extracted as
// pure logic (parsed JSON messages in, outgoing frames out via a `send`
// callback) so it can be unit-tested without standing up a real WebSocket —
// see test/graphql-protocol.test.ts.
//
// This mirrors web/src/lib/graphql.ts's `ensureSocket`/`subscribe` handling
// exactly, but as an instantiable class rather than module-level globals: the
// web app only ever talks to one daemon, so module state is fine there — an
// SDK process can hold several `Client`s, so each needs its own subscription
// bookkeeping (see client.ts, which owns one `SubscriptionManager` per
// instance plus the actual socket).

import { AuthError, GraphQLError } from "./errors.js";

export interface SubscriptionHandlers {
  onNext(data: any): void;
  onError(e: Error): void;
  onComplete(): void;
}

/**
 * Tracks subscribe/unsubscribe bookkeeping and dispatches incoming
 * `graphql-transport-ws` protocol frames. Does not own a socket — the caller
 * feeds it parsed messages via {@link handleMessage} and receives outgoing
 * frames through the `send` callback given to the constructor.
 */
export class SubscriptionManager {
  /** Whether `connection_ack` has been received on the current connection. */
  acked = false;
  #nextId = 1;
  #subs = new Map<string, SubscriptionHandlers>();
  /** Subscriptions started before `connection_ack`, flushed once it arrives. */
  #pending: Array<() => void> = [];

  constructor(private readonly send: (msg: unknown) => void) {}

  /** Number of subscriptions currently tracked (pending or live). */
  get size(): number {
    return this.#subs.size;
  }

  /**
   * Start a subscription. Returns its id; end it with {@link stop}.
   *
   * If `connection_ack` has not arrived yet, the wire `subscribe` message is
   * queued and sent once it does — but the id is reserved immediately, so a
   * {@link stop} issued before the ack still cancels it correctly.
   */
  start(
    query: string,
    variables: Record<string, unknown>,
    handlers: Partial<SubscriptionHandlers>,
  ): string {
    const id = String(this.#nextId++);
    const sub: SubscriptionHandlers = {
      onNext: handlers.onNext ?? (() => {}),
      onError: handlers.onError ?? (() => {}),
      onComplete: handlers.onComplete ?? (() => {}),
    };
    this.#subs.set(id, sub);

    const doStart = () => {
      if (!this.#subs.has(id)) return; // unsubscribed before the ack arrived
      this.send({ id, type: "subscribe", payload: { query, variables } });
    };
    if (this.acked) doStart();
    else this.#pending.push(doStart);

    return id;
  }

  /** Unsubscribe `id`, sending `{type:"complete"}`. Returns false if already gone. */
  stop(id: string): boolean {
    if (!this.#subs.delete(id)) return false;
    this.send({ id, type: "complete" });
    return true;
  }

  /** Feed one parsed incoming message through the protocol. */
  handleMessage(msg: any): void {
    switch (msg?.type) {
      case "connection_ack": {
        this.acked = true;
        const queued = this.#pending;
        this.#pending = [];
        queued.forEach((f) => f());
        break;
      }
      case "next": {
        const sub = this.#subs.get(msg.id);
        sub?.onNext(msg.payload?.data);
        break;
      }
      case "error": {
        const sub = this.#subs.get(msg.id);
        this.#subs.delete(msg.id);
        const detail = Array.isArray(msg.payload)
          ? msg.payload.map((e: any) => e.message).join("; ")
          : JSON.stringify(msg.payload);
        sub?.onError(new GraphQLError(detail));
        break;
      }
      case "complete": {
        const sub = this.#subs.get(msg.id);
        this.#subs.delete(msg.id);
        sub?.onComplete();
        break;
      }
      case "ping":
        this.send({ type: "pong" });
        break;
    }
  }

  /**
   * The socket closed. Deliver an error to every tracked subscription — an
   * {@link AuthError} if `connection_ack` never arrived (the daemon closes the
   * socket itself on a bad token), a generic {@link GraphQLError} otherwise —
   * then reset so the next connection starts clean.
   */
  handleClose(): void {
    const err = this.acked
      ? new GraphQLError("the connection to the daemon was closed")
      : new AuthError();
    const open = [...this.#subs.values()];
    this.#subs.clear();
    this.#pending = [];
    this.acked = false;
    open.forEach((s) => s.onError(err));
  }
}
