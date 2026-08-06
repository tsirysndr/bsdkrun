// The GraphQL transport: HTTP for queries and mutations, a graphql-ws socket
// for subscriptions.
//
// Hand-rolled rather than pulled from Apollo/urql. All this app needs is
// "send a document, get data back" plus a subscription client, and a small
// dependency-free layer keeps the connection settings (which the user can
// change at runtime, from the UI) trivially easy to re-point.

import { getConnection, type Connection } from "./connection";

export class GraphQLError extends Error {
  readonly code?: string;
  constructor(message: string, code?: string) {
    super(message);
    this.name = "GraphQLError";
    this.code = code;
  }
}

/** Thrown when the daemon rejects our token, so the UI can send you back to setup. */
export class AuthError extends GraphQLError {
  constructor(message = "the daemon rejected this token") {
    super(message, "UNAUTHENTICATED");
    this.name = "AuthError";
  }
}

function httpUrl(conn: Connection): string {
  return conn.url;
}

function wsUrl(conn: Connection): string {
  const u = new URL(conn.url);
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  // The daemon serves subscriptions on the same path with /ws appended.
  u.pathname = u.pathname.replace(/\/*$/, "") + "/ws";
  return u.toString();
}

/** Run a query or mutation. */
export async function gql<T = any>(
  query: string,
  variables: Record<string, unknown> = {},
): Promise<T> {
  const conn = getConnection();
  if (!conn) throw new AuthError("not connected");

  let res: Response;
  try {
    res = await fetch(httpUrl(conn), {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization: `Bearer ${conn.token}`,
      },
      body: JSON.stringify({ query, variables }),
    });
  } catch (e) {
    // fetch only rejects on a transport failure, which for this app almost
    // always means the daemon is not reachable at the configured URL.
    throw new GraphQLError(
      `cannot reach the bsdkrun daemon at ${conn.url} — ${(e as Error).message}`,
    );
  }

  if (res.status === 401) throw new AuthError();

  const body = await res.json().catch(() => null);
  if (!body) throw new GraphQLError(`the daemon returned a non-JSON response (${res.status})`);
  if (body.errors?.length) {
    const first = body.errors[0];
    const code = first.extensions?.code;
    if (code === "UNAUTHENTICATED") throw new AuthError(first.message);
    throw new GraphQLError(first.message, code);
  }
  return body.data as T;
}

// ---- subscriptions ---------------------------------------------------------
//
// A minimal `graphql-transport-ws` client. One socket is shared by every
// subscription, opened lazily and torn down when the last one unsubscribes.

type Sub = {
  onNext: (data: any) => void;
  onError: (e: Error) => void;
  onComplete: () => void;
};

let socket: WebSocket | null = null;
let acked = false;
let nextId = 1;
const subs = new Map<string, Sub>();
/** Subscriptions started before `connection_ack`, flushed once it arrives. */
let pending: Array<() => void> = [];

function closeSocket() {
  pending = [];
  acked = false;
  const s = socket;
  socket = null;
  if (s && s.readyState <= WebSocket.OPEN) s.close();
}

function ensureSocket(): WebSocket {
  if (socket && socket.readyState <= WebSocket.OPEN) return socket;

  const conn = getConnection();
  if (!conn) throw new AuthError("not connected");

  const ws = new WebSocket(wsUrl(conn), "graphql-transport-ws");
  socket = ws;
  acked = false;

  ws.onopen = () => {
    // A browser cannot set headers on a WebSocket handshake, so the token
    // travels in connection_init instead. The daemon checks it before any
    // operation runs.
    ws.send(
      JSON.stringify({
        type: "connection_init",
        payload: { authorization: `Bearer ${conn.token}` },
      }),
    );
  };

  ws.onmessage = (ev) => {
    let msg: any;
    try {
      msg = JSON.parse(ev.data as string);
    } catch {
      return;
    }

    switch (msg.type) {
      case "connection_ack": {
        acked = true;
        const queued = pending;
        pending = [];
        queued.forEach((f) => f());
        break;
      }
      case "next": {
        const sub = subs.get(msg.id);
        if (sub) sub.onNext(msg.payload?.data);
        break;
      }
      case "error": {
        const sub = subs.get(msg.id);
        subs.delete(msg.id);
        const detail = Array.isArray(msg.payload)
          ? msg.payload.map((e: any) => e.message).join("; ")
          : JSON.stringify(msg.payload);
        sub?.onError(new GraphQLError(detail));
        break;
      }
      case "complete": {
        const sub = subs.get(msg.id);
        subs.delete(msg.id);
        sub?.onComplete();
        break;
      }
      case "ping":
        ws.send(JSON.stringify({ type: "pong" }));
        break;
    }
  };

  ws.onclose = () => {
    // The daemon closes the socket when connection_init carries a bad token,
    // so an unacked close is an auth failure rather than a network blip.
    const err = acked
      ? new GraphQLError("the connection to the daemon was closed")
      : new AuthError();
    const open = [...subs.values()];
    subs.clear();
    closeSocket();
    open.forEach((s) => s.onError(err));
  };

  ws.onerror = () => {
    /* onclose always follows; reported there so it is not reported twice. */
  };

  return ws;
}

/**
 * Start a subscription. Returns an unsubscribe function.
 *
 * `onNext` receives the `data` object, so a caller reads its own field off it
 * exactly as it appears in the document.
 */
export function subscribe(
  query: string,
  variables: Record<string, unknown>,
  handlers: Partial<Sub>,
): () => void {
  const id = String(nextId++);
  const sub: Sub = {
    onNext: handlers.onNext ?? (() => {}),
    onError: handlers.onError ?? (() => {}),
    onComplete: handlers.onComplete ?? (() => {}),
  };
  subs.set(id, sub);

  let ws: WebSocket;
  try {
    ws = ensureSocket();
  } catch (e) {
    subs.delete(id);
    queueMicrotask(() => sub.onError(e as Error));
    return () => {};
  }

  const start = () => {
    if (!subs.has(id)) return; // unsubscribed before the ack arrived
    ws.send(JSON.stringify({ id, type: "subscribe", payload: { query, variables } }));
  };
  if (acked) start();
  else pending.push(start);

  return () => {
    if (!subs.delete(id)) return;
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ id, type: "complete" }));
    }
    // Drop the socket once nothing is using it, so a re-point to a different
    // daemon does not keep talking to the old one.
    if (subs.size === 0) closeSocket();
  };
}

/** Force every subscription to reconnect — used when the endpoint changes. */
export function resetTransport() {
  const open = [...subs.values()];
  subs.clear();
  closeSocket();
  open.forEach((s) => s.onComplete());
}
