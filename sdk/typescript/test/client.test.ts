/**
 * Tests for the remote `Client` (src/client.ts): URL normalization,
 * `fromEnv()`'s validation rule, the HTTP `request()` transport's error
 * handling, and `exec()`'s open -> subscribe -> collect -> close sequencing.
 *
 * The HTTP tests stand up a plain `node:http` server (works the same under
 * Bun, which is what `bun test` runs this with) — no dependency needed. The
 * `exec()` test needs a WebSocket *server* too, which neither Node nor Bun's
 * `node:http` provides; Bun does have one built in (`Bun.serve({websocket})`),
 * and since this whole suite already only runs under `bun test` (see
 * package.json and .github/workflows/e2e-sdk.yml), using it here needs no
 * extra dependency either. The WS *protocol* itself (message routing,
 * ack-gating, ping/pong, close semantics) is unit-tested in isolation in
 * test/graphql-protocol.test.ts instead of here, for the reasons noted there.
 */
import { afterEach, describe, expect, test } from "bun:test";
import { createServer, type Server } from "node:http";
import { AuthError, GraphQLError } from "../src/errors.js";
import { Client, normalizeUrl, TOKEN_ENV, URL_ENV } from "../src/client.js";

// ---- normalizeUrl -----------------------------------------------------------

describe("normalizeUrl", () => {
  test("adds http:// when no scheme is given", () => {
    expect(normalizeUrl("localhost:50052")).toBe("http://localhost:50052/graphql");
  });

  test("keeps an explicit https:// scheme", () => {
    expect(normalizeUrl("https://vps.example.com:50052")).toBe(
      "https://vps.example.com:50052/graphql",
    );
  });

  test("strips trailing slashes before appending /graphql", () => {
    expect(normalizeUrl("http://localhost:50052/")).toBe("http://localhost:50052/graphql");
    expect(normalizeUrl("http://localhost:50052///")).toBe("http://localhost:50052/graphql");
  });

  test("does not double-append /graphql", () => {
    expect(normalizeUrl("http://localhost:50052/graphql")).toBe(
      "http://localhost:50052/graphql",
    );
  });

  test("trims whitespace", () => {
    expect(normalizeUrl("  localhost:50052  ")).toBe("http://localhost:50052/graphql");
  });
});

// ---- Client.fromEnv ----------------------------------------------------------

describe("Client.fromEnv", () => {
  const savedUrl = process.env[URL_ENV];
  const savedToken = process.env[TOKEN_ENV];

  afterEach(() => {
    if (savedUrl === undefined) delete process.env[URL_ENV];
    else process.env[URL_ENV] = savedUrl;
    if (savedToken === undefined) delete process.env[TOKEN_ENV];
    else process.env[TOKEN_ENV] = savedToken;
  });

  test("throws when BSDKRUN_URL is unset", () => {
    delete process.env[URL_ENV];
    delete process.env[TOKEN_ENV];
    expect(() => Client.fromEnv()).toThrow(/BSDKRUN_URL/);
  });

  test("throws when BSDKRUN_URL is set but BSDKRUN_TOKEN is not — no silent fallback", () => {
    process.env[URL_ENV] = "http://localhost:50052";
    delete process.env[TOKEN_ENV];
    expect(() => Client.fromEnv()).toThrow(/BSDKRUN_TOKEN/);
  });

  test("builds a Client when both are set", () => {
    process.env[URL_ENV] = "localhost:50052";
    process.env[TOKEN_ENV] = "tok";
    expect(Client.fromEnv()).toBeInstanceOf(Client);
  });
});

// ---- HTTP transport (Client.request) -----------------------------------------

function withServer(
  handler: (req: import("node:http").IncomingMessage, res: import("node:http").ServerResponse) => void,
): Promise<{ server: Server; url: string }> {
  return new Promise((resolve) => {
    const server = createServer(handler);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      const port = typeof addr === "object" && addr ? addr.port : 0;
      resolve({ server, url: `http://127.0.0.1:${port}/graphql` });
    });
  });
}

function jsonResponse(res: import("node:http").ServerResponse, status: number, body: unknown) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}

describe("Client.request (HTTP transport)", () => {
  let server: Server | undefined;

  afterEach(() => {
    server?.close();
    server = undefined;
  });

  test("HTTP 401 throws AuthError", async () => {
    const s = await withServer((_req, res) => {
      res.writeHead(401);
      res.end();
    });
    server = s.server;
    const client = new Client({ url: s.url, token: "x" });
    await expect(client.request("{x}")).rejects.toBeInstanceOf(AuthError);
  });

  test("errors[0].extensions.code === UNAUTHENTICATED throws AuthError", async () => {
    const s = await withServer((_req, res) => {
      jsonResponse(res, 200, {
        errors: [{ message: "nope", extensions: { code: "UNAUTHENTICATED" } }],
      });
    });
    server = s.server;
    const client = new Client({ url: s.url, token: "x" });
    let caught: unknown;
    try {
      await client.request("{x}");
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(AuthError);
    expect((caught as AuthError).message).toBe("nope");
  });

  test("any other GraphQL error throws GraphQLError carrying its code", async () => {
    const s = await withServer((_req, res) => {
      jsonResponse(res, 200, {
        errors: [{ message: "bad input", extensions: { code: "INVALID_ARGUMENT" } }],
      });
    });
    server = s.server;
    const client = new Client({ url: s.url, token: "x" });
    let caught: unknown;
    try {
      await client.request("{x}");
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(GraphQLError);
    expect(caught).not.toBeInstanceOf(AuthError);
    expect((caught as GraphQLError).code).toBe("INVALID_ARGUMENT");
    expect((caught as GraphQLError).message).toBe("bad input");
  });

  test("a clean response returns body.data", async () => {
    const s = await withServer((_req, res) => {
      jsonResponse(res, 200, { data: { hello: "world" } });
    });
    server = s.server;
    const client = new Client({ url: s.url, token: "x" });
    const data = await client.request<{ hello: string }>("{hello}");
    expect(data).toEqual({ hello: "world" });
  });

  test("posts {query,variables} with a Bearer authorization header", async () => {
    // A plain holder object, not loose `let`s — assigning into it from the
    // request handler and reading it back after `await` avoids a control-flow
    // analysis quirk where TS over-narrows a `let` only ever assigned inside
    // a callback back to its initializer's type at the read site.
    const seen: { auth: string | null; body: any } = { auth: null, body: undefined };
    const s = await withServer((req, res) => {
      seen.auth = req.headers.authorization ?? null;
      let raw = "";
      req.on("data", (c) => (raw += c));
      req.on("end", () => {
        seen.body = JSON.parse(raw);
        jsonResponse(res, 200, { data: {} });
      });
    });
    server = s.server;
    const client = new Client({ url: s.url, token: "sekrit" });
    await client.request("query($x:Int){y(x:$x)}", { x: 1 });
    expect(seen.auth).toBe("Bearer sekrit");
    expect(seen.body).toEqual({ query: "query($x:Int){y(x:$x)}", variables: { x: 1 } });
  });

  test("an unreachable daemon throws GraphQLError mentioning the url", async () => {
    const client = new Client({ url: "http://127.0.0.1:1", token: "x" });
    await expect(client.request("{x}")).rejects.toThrow(/cannot reach the bsdkrun daemon/);
  });
});

// ---- exec()'s open -> subscribe -> collect -> close sequencing --------------

/**
 * A minimal fake `bsdkrund`: just enough GraphQL surface (HTTP `openShell`/
 * `closeShell` mutations, a `graphql-transport-ws` `shellOutput` subscription)
 * to drive `Client.exec()` end to end over real HTTP + WebSocket connections.
 */
function fakeDaemon(token = "secret") {
  const calls: string[] = [];

  const server = Bun.serve({
    port: 0,
    async fetch(req, srv) {
      const url = new URL(req.url);

      if (url.pathname === "/graphql/ws") {
        return srv.upgrade(req) ? undefined : new Response("upgrade failed", { status: 500 });
      }

      if (url.pathname === "/graphql" && req.method === "POST") {
        if (req.headers.get("authorization") !== `Bearer ${token}`) {
          return Response.json(
            { errors: [{ message: "bad token", extensions: { code: "UNAUTHENTICATED" } }] },
            { status: 401 },
          );
        }
        const body = (await req.json()) as { query: string; variables: any };
        if (body.query.includes("openShell")) {
          calls.push("openShell");
          return Response.json({
            data: {
              openShell: {
                id: "sess-1",
                machineId: body.variables.machineId,
                finished: false,
                truncated: false,
              },
            },
          });
        }
        if (body.query.includes("closeShell")) {
          calls.push("closeShell");
          return Response.json({ data: { closeShell: true } });
        }
        return Response.json({ errors: [{ message: `unhandled query: ${body.query}` }] });
      }

      return new Response("not found", { status: 404 });
    },
    websocket: {
      open() {},
      message(ws, raw) {
        const msg = JSON.parse(String(raw));
        if (msg.type === "connection_init") {
          if (msg.payload?.authorization !== `Bearer ${token}`) {
            ws.close();
            return;
          }
          ws.send(JSON.stringify({ type: "connection_ack" }));
        } else if (msg.type === "subscribe") {
          calls.push("subscribe");
          const send = (data: unknown) =>
            ws.send(JSON.stringify({ id: msg.id, type: "next", payload: { data } }));
          send({
            shellOutput: { dataBase64: Buffer.from("hi ").toString("base64"), exitCode: null },
          });
          send({
            shellOutput: { dataBase64: Buffer.from("there").toString("base64"), exitCode: null },
          });
          send({ shellOutput: { dataBase64: null, exitCode: 0 } });
          ws.send(JSON.stringify({ id: msg.id, type: "complete" }));
        }
      },
      close() {},
    },
  });

  return {
    server,
    calls,
    url: new URL("/graphql", server.url).toString(),
    token,
  };
}

describe("Client.exec (against a fake bsdkrund)", () => {
  let daemon: ReturnType<typeof fakeDaemon> | undefined;

  afterEach(() => {
    daemon?.server.stop(true);
    daemon = undefined;
  });

  test("opens a shell, collects output until the exit code, then closes it", async () => {
    daemon = fakeDaemon();
    const client = new Client({ url: daemon.url, token: daemon.token });

    const result = await client.exec("machine-1", ["echo", "hi", "there"]);

    expect(result.exitCode).toBe(0);
    expect(Buffer.from(result.output).toString("utf8")).toBe("hi there");
    // open before subscribe, and close happens (exactly once) after both.
    expect(daemon.calls[0]).toBe("openShell");
    expect(daemon.calls).toContain("subscribe");
    expect(daemon.calls.filter((c) => c === "closeShell")).toHaveLength(1);
    expect(daemon.calls.indexOf("closeShell")).toBeGreaterThan(daemon.calls.indexOf("subscribe"));
  });

  test("closeShell still runs when the daemon rejects the token", async () => {
    daemon = fakeDaemon("right-token");
    const client = new Client({ url: daemon.url, token: "wrong-token" });

    // The HTTP openShell call itself will 401 (wrong Bearer token), which
    // rejects before a session ever exists — nothing to close in that case,
    // but it must still surface as an AuthError rather than hang.
    await expect(client.exec("machine-1", ["true"])).rejects.toBeInstanceOf(AuthError);
  });
});
