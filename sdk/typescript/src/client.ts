// A remote-client mode: talk directly to a `bsdkrund` daemon's GraphQL API
// instead of shelling out to a local `bsdkrun` binary (see sandbox.ts).
//
// This is one of five parallel language SDKs (TS/Python/Ruby/Elixir/Gleam)
// implementing the exact same wire contract, so the transport logic here is
// ported faithfully from the reference implementation — the web app's
// web/src/lib/graphql.ts (`gql`/`ensureSocket`/`subscribe`) and
// web/src/lib/connection.ts (`normalizeUrl`) — rather than reinvented. The one
// structural difference: the web app is a single-connection browser page and
// keeps its socket/subscription state in module-level globals, but an SDK
// process can hold several `Client`s at once, so all of that state is scoped
// to a `Client` instance instead (a field, not a module global). The
// `graphql-transport-ws` message-handling itself is further split out into
// `SubscriptionManager` (graphql-protocol.ts) so it can be unit-tested
// without a real socket.
//
// GraphQL query/mutation/subscription shapes mirror `daemon/src/graphql.rs`
// 1:1 — same field names, same input object shapes — so a caller can paste
// field names straight out of the GraphQL docs/GraphiQL.

import { AuthError, BsdkrunError, GraphQLError } from "./errors.js";
import { SubscriptionManager } from "./graphql-protocol.js";
import { fromGraphQLMachine } from "./sandbox.js";
import type { CommandResult, SandboxInfo, ShellOutput, ShellSessionInfo } from "./types.js";

/** `Client.fromEnv()` reads these. */
export const URL_ENV = "BSDKRUN_URL";
export const TOKEN_ENV = "BSDKRUN_TOKEN";

// ---------------------------------------------------------------------------
// GraphQL input shapes — transcribed field-for-field from the `InputObject`s
// in daemon/src/graphql.rs (RunLinuxInput ~384, RunBsdInput ~406,
// RunNanosInput ~428, RunUnikraftInput ~449, RunOsvInput ~467, RunFlavorInput
// ~506, NetInput ~360, BsdOs ~324). Rust's `snake_case` fields arrive here
// already `camelCase` (async-graphql's default rename), so these match the
// wire 1:1 with no local renaming — unlike `CreateOptions` in types.ts, which
// shapes CLI arguments instead.
// ---------------------------------------------------------------------------

/** Networking options shared by every `run*` mutation that takes a `net` field. */
export interface NetOptions {
  noNet?: boolean;
  /** Host->guest TCP forwards, each `"HOST:GUEST"`. */
  ports?: string[];
  mac?: string;
  network?: string;
  name?: string;
}

/** The GraphQL `BsdOs` enum's wire values. */
export type BsdOsInput = "FREEBSD" | "NETBSD";

export interface RunLinuxOptions {
  image: string;
  cpus?: number;
  mem?: number;
  net?: NetOptions;
  volume?: string;
  mounts?: string[];
  env?: string[];
  entrypoint?: string;
  initramfs?: boolean;
  kernel?: string;
  kernelVersion?: string;
  console?: string;
  repo?: string;
  command?: string[];
}

export interface RunBsdOptions {
  os: BsdOsInput;
  version?: string;
  cpus?: number;
  mem?: number;
  net?: NetOptions;
  volume?: string;
  persist?: boolean;
  force?: boolean;
  firmware?: string;
  attachDisk?: string[];
  diskSize?: string;
  repo?: string;
  command?: string[];
}

/** Nanos has no agent (no exec/shell), but it does have a root disk, so `persist` applies. */
export interface RunNanosOptions {
  /** A path, or a bare name in `~/.ops/images` (what `ops build -i` makes). */
  image: string;
  cpus?: number;
  mem?: number;
  net?: NetOptions;
  /** Nanos kernel override (Linux hosts). */
  kernel?: string;
  cmdline?: string;
  persist?: boolean;
}

/** A unikernel has no disk and no agent, so this carries none of the volume/persist/repo/command fields. */
export interface RunUnikraftOptions {
  /** A `kraft` project directory or a built unikernel image. Defaults to `"."`. */
  path?: string;
  cpus?: number;
  mem?: number;
  net?: NetOptions;
  /** Kernel command line; Unikraft hands it to the application as argv. */
  cmdline?: string;
  initramfs?: string;
  /** Persistent volumes over virtio-fs, each `"HOST:GUEST"` with an absolute guest path. */
  mounts?: string[];
}

/** OSv: like Nanos there is no agent, but it does have a root filesystem, so the disk options apply. */
export interface RunOsvOptions {
  /** An aarch64 `loader.img`, or on x86_64 the loader ELF, which needs `disk`. */
  image: string;
  cpus?: number;
  mem?: number;
  net?: NetOptions;
  /** The application to run and its arguments, e.g. `"/hello.so"`. */
  cmdline?: string;
  /** Root disk (raw). Required on x86_64. */
  disk?: string;
  /** Boot the kernel alone, with no root filesystem to mount. */
  noDisk?: boolean;
  /** Extra disks as virtio-blk, each `"PATH"` or `"PATH:ro"`. */
  attachDisk?: string[];
  /** `"v2"` (the default, what OSv v0.57.0 needs) or `"v3"`. aarch64 only. */
  gic?: string;
  persist?: boolean;
  volume?: string;
}

export interface RunFlavorOptions {
  name: string;
  cpus?: number;
  mem?: number;
  ports?: string[];
  volume?: string;
  repo?: string;
}

// ---------------------------------------------------------------------------
// Interactive shell handle
// ---------------------------------------------------------------------------

/**
 * A live interactive session opened by {@link Client.shell}. `write`/`resize`/
 * `close` are fire-and-forget mutations (transport failures are swallowed —
 * there is nowhere synchronous to report them; a failed `write` surfaces as
 * the session going quiet, and a dead connection surfaces via {@link onExit}).
 */
export interface ShellSession {
  readonly id: string;
  /** Send input. Strings are UTF-8 encoded before being base64'd onto the wire. */
  write(data: Uint8Array | string): void;
  resize(rows: number, cols: number): void;
  /** End the session. Idempotent. */
  close(): void;
  /** Called with each output chunk as it arrives. */
  onOutput(cb: (chunk: Uint8Array) => void): void;
  /** Called once, when the session's command exits (or the connection drops). */
  onExit(cb: (code: number) => void): void;
}

// ---------------------------------------------------------------------------
// field selections
// ---------------------------------------------------------------------------

/** Matches web/src/lib/api.ts's `MACHINE_FIELDS` fragment exactly. */
const MACHINE_FIELDS = `
  id name image kind command status running exitCode pid detached
  cpus mem volume stateDir createdAt finishedAt network netIp
  ports { bind host guest }
`;
const COMMAND_RESULT_FIELDS = `exitCode stdout stderr`;
const SHELL_OUTPUT_FIELDS = `dataBase64 exitCode`;

// ---------------------------------------------------------------------------
// base64 <-> bytes
// ---------------------------------------------------------------------------
//
// This SDK already depends on Node's `Buffer` throughout (agent-protocol.ts,
// terminal.ts, process.ts), and it's a global under Node, Bun and Deno's
// Node-compat layer, so it's used here too rather than hand-rolling
// atob/btoa — one less thing to keep correct for arbitrary binary data.

function bytesToBase64(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("base64");
}

function base64ToBytes(b64: string): Uint8Array {
  return Buffer.from(b64, "base64");
}

function concatBytes(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.length;
  }
  return out;
}

// ---------------------------------------------------------------------------
// URL handling
// ---------------------------------------------------------------------------

/**
 * Accept what people actually paste and turn it into the endpoint URL.
 * Ported verbatim from web/src/lib/connection.ts's `normalizeUrl`.
 */
export function normalizeUrl(input: string): string {
  let s = input.trim();
  if (!s) return s;
  if (!/^https?:\/\//i.test(s)) s = `http://${s}`;
  s = s.replace(/\/+$/, "");
  if (!/\/graphql$/i.test(s)) s = `${s}/graphql`;
  return s;
}

/** `http(s)://host:port/graphql` -> `ws(s)://host:port/graphql/ws`. */
function wsUrl(url: string): string {
  const u = new URL(url);
  u.protocol = u.protocol === "https:" ? "wss:" : "ws:";
  u.pathname = u.pathname.replace(/\/*$/, "") + "/ws";
  return u.toString();
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/**
 * A connection to a remote `bsdkrund` daemon, over its GraphQL API — queries
 * and mutations over HTTP, subscriptions (exec/shell/log-follow) over a
 * shared `graphql-transport-ws` socket opened lazily on first use.
 *
 * ```ts
 * const client = Client.fromEnv(); // BSDKRUN_URL + BSDKRUN_TOKEN
 * const machines = await client.list();
 * const id = await client.runLinux({ image: "alpine" });
 * const { exitCode, output } = await client.exec(id, ["uname", "-a"]);
 * ```
 */
export class Client {
  readonly #url: string;
  readonly #token: string;
  #socket: WebSocket | null = null;
  readonly #protocol: SubscriptionManager;

  constructor(config: { url: string; token: string }) {
    this.#url = normalizeUrl(config.url);
    this.#token = config.token;
    this.#protocol = new SubscriptionManager((msg) => {
      const ws = this.#socket;
      if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(msg));
    });
  }

  /**
   * Build a `Client` from `BSDKRUN_URL` / `BSDKRUN_TOKEN`. A host set without
   * a token is an error, not a silent unauthenticated fallback — the same
   * rule `daemon/src/client.rs`'s `RemoteConfig::from_env` applies to the
   * gRPC `BSDKRUN_HOST`/`BSDKRUN_TOKEN` pair (different env vars: this is a
   * GraphQL endpoint URL, not a gRPC host:port).
   */
  static fromEnv(): Client {
    const url = process.env[URL_ENV]?.trim();
    if (!url) {
      throw new BsdkrunError(`${URL_ENV} is not set — nothing to connect to`);
    }
    const token = process.env[TOKEN_ENV]?.trim();
    if (!token) {
      throw new BsdkrunError(`${URL_ENV} is set but ${TOKEN_ENV} is not`);
    }
    return new Client({ url, token });
  }

  // ---- transport: HTTP (queries + mutations) ------------------------------

  /**
   * Run a query or mutation against the daemon. The escape hatch: every
   * typed method below is implemented in terms of this one.
   */
  async request<T = any>(
    query: string,
    variables: Record<string, unknown> = {},
  ): Promise<T> {
    let res: Response;
    try {
      res = await fetch(this.#url, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          authorization: `Bearer ${this.#token}`,
        },
        body: JSON.stringify({ query, variables }),
      });
    } catch (e) {
      // fetch only rejects on a transport failure — the daemon is not
      // reachable at the configured URL.
      throw new GraphQLError(
        `cannot reach the bsdkrun daemon at ${this.#url} — ${(e as Error).message}`,
      );
    }

    if (res.status === 401) throw new AuthError();

    // `Response.json()` resolves `unknown` under bun-types (no DOM lib is
    // loaded, which is where the usual `Promise<any>` signature comes from);
    // cast explicitly rather than let a truthy-narrow of `unknown` collapse
    // to `{}` below and reject every property access on the parsed body.
    const body = (await res.json().catch(() => null)) as any;
    if (!body) {
      throw new GraphQLError(`the daemon returned a non-JSON response (${res.status})`);
    }
    if (body.errors?.length) {
      const first = body.errors[0];
      const code = first.extensions?.code;
      if (code === "UNAUTHENTICATED") throw new AuthError(first.message);
      throw new GraphQLError(first.message, code);
    }
    return body.data as T;
  }

  // ---- transport: WebSocket (subscriptions) --------------------------------

  #ensureSocket(): WebSocket {
    if (this.#socket && this.#socket.readyState <= WebSocket.OPEN) return this.#socket;

    const ws = new WebSocket(wsUrl(this.#url), "graphql-transport-ws");
    this.#socket = ws;

    ws.onopen = () => {
      // A plain WebSocket handshake cannot carry custom headers, so the token
      // travels in connection_init instead; the daemon checks it before any
      // operation runs.
      ws.send(
        JSON.stringify({
          type: "connection_init",
          payload: { authorization: `Bearer ${this.#token}` },
        }),
      );
    };

    ws.onmessage = (ev: { data: unknown }) => {
      let msg: any;
      try {
        msg = JSON.parse(ev.data as string);
      } catch {
        return;
      }
      this.#protocol.handleMessage(msg);
    };

    ws.onclose = () => {
      this.#protocol.handleClose();
      if (this.#socket === ws) this.#socket = null;
    };

    ws.onerror = () => {
      /* onclose always follows; reported there so it is not reported twice. */
    };

    return ws;
  }

  #closeSocket(): void {
    const s = this.#socket;
    this.#socket = null;
    if (s && s.readyState <= WebSocket.OPEN) s.close();
  }

  /**
   * Start a subscription. Returns an unsubscribe function. `onNext` receives
   * the `data` object, so a caller reads its own field off it exactly as it
   * appears in the document — the escape hatch alongside {@link request}.
   */
  subscribe(
    query: string,
    variables: Record<string, unknown>,
    handlers: { onNext(data: any): void; onError(e: Error): void; onComplete(): void },
  ): () => void {
    let ws: WebSocket;
    try {
      ws = this.#ensureSocket();
    } catch (e) {
      queueMicrotask(() => handlers.onError(e as Error));
      return () => {};
    }
    void ws;

    const id = this.#protocol.start(query, variables, handlers);

    return () => {
      if (!this.#protocol.stop(id)) return;
      if (this.#protocol.size === 0) this.#closeSocket();
    };
  }

  // ---- lifecycle / listing --------------------------------------------------

  async list(all = false): Promise<SandboxInfo[]> {
    const d = await this.request<{ machines: any[] }>(
      `query($all:Boolean!){ machines(all:$all){ ${MACHINE_FIELDS} } }`,
      { all },
    );
    return d.machines.map(fromGraphQLMachine);
  }

  async get(id: string): Promise<SandboxInfo | null> {
    const d = await this.request<{ machine: any | null }>(
      `query($id:String!){ machine(id:$id){ ${MACHINE_FIELDS} } }`,
      { id },
    );
    return d.machine ? fromGraphQLMachine(d.machine) : null;
  }

  async stop(id: string): Promise<CommandResult> {
    const d = await this.request<{ stopMachine: CommandResult }>(
      `mutation($id:String!){ stopMachine(id:$id){ ${COMMAND_RESULT_FIELDS} } }`,
      { id },
    );
    return d.stopMachine;
  }

  async start(id: string): Promise<CommandResult> {
    const d = await this.request<{ startMachine: CommandResult }>(
      `mutation($id:String!){ startMachine(id:$id){ ${COMMAND_RESULT_FIELDS} } }`,
      { id },
    );
    return d.startMachine;
  }

  async remove(ids: string[], force = false): Promise<CommandResult> {
    const d = await this.request<{ removeMachines: CommandResult }>(
      `mutation($ids:[String!]!,$force:Boolean!){
         removeMachines(ids:$ids, force:$force){ ${COMMAND_RESULT_FIELDS} }
       }`,
      { ids, force },
    );
    return d.removeMachines;
  }

  async update(id: string, opts: { cpus?: number; mem?: number } = {}): Promise<CommandResult> {
    const d = await this.request<{ updateMachine: CommandResult }>(
      `mutation($id:String!,$cpus:Int,$mem:Int){
         updateMachine(id:$id, cpus:$cpus, mem:$mem){ ${COMMAND_RESULT_FIELDS} }
       }`,
      { id, cpus: opts.cpus ?? null, mem: opts.mem ?? null },
    );
    return d.updateMachine;
  }

  async commit(id: string, name: string, description = ""): Promise<CommandResult> {
    const d = await this.request<{ commitMachine: CommandResult }>(
      `mutation($id:String!,$name:String!,$description:String!){
         commitMachine(id:$id, name:$name, description:$description){ ${COMMAND_RESULT_FIELDS} }
       }`,
      { id, name, description },
    );
    return d.commitMachine;
  }

  async logs(id: string, boot = false): Promise<string> {
    const d = await this.request<{ machineLogs: string }>(
      `query($id:String!,$boot:Boolean!){ machineLogs(id:$id, boot:$boot) }`,
      { id, boot },
    );
    return d.machineLogs;
  }

  /** Follow a machine's console (`machineLogs` subscription). Returns an unsubscribe fn. */
  followLogs(
    id: string,
    opts: { follow?: boolean; boot?: boolean } = {},
    handlers: { onData(chunk: ShellOutput): void; onError(e: Error): void; onComplete(): void },
  ): () => void {
    return this.subscribe(
      `subscription($id:String!,$follow:Boolean!,$boot:Boolean!){
         machineLogs(id:$id, follow:$follow, boot:$boot){ ${SHELL_OUTPUT_FIELDS} }
       }`,
      { id, follow: opts.follow ?? true, boot: opts.boot ?? false },
      {
        onNext: (data) => {
          const p = data?.machineLogs;
          if (!p) return;
          handlers.onData({
            data: p.dataBase64 ? base64ToBytes(p.dataBase64) : null,
            exitCode: p.exitCode ?? null,
          });
        },
        onError: (e) => handlers.onError(e),
        onComplete: () => handlers.onComplete(),
      },
    );
  }

  // ---- booting ---------------------------------------------------------------

  async runLinux(opts: RunLinuxOptions): Promise<string> {
    const d = await this.request<{ runLinux: string }>(
      `mutation($i:RunLinuxInput!){ runLinux(input:$i) }`,
      { i: opts },
    );
    return d.runLinux;
  }

  async runBsd(opts: RunBsdOptions): Promise<string> {
    const d = await this.request<{ runBsd: string }>(
      `mutation($i:RunBsdInput!){ runBsd(input:$i) }`,
      { i: opts },
    );
    return d.runBsd;
  }

  async runNanos(opts: RunNanosOptions): Promise<string> {
    const d = await this.request<{ runNanos: string }>(
      `mutation($i:RunNanosInput!){ runNanos(input:$i) }`,
      { i: opts },
    );
    return d.runNanos;
  }

  async runUnikraft(opts: RunUnikraftOptions): Promise<string> {
    const d = await this.request<{ runUnikraft: string }>(
      `mutation($i:RunUnikraftInput!){ runUnikraft(input:$i) }`,
      { i: opts },
    );
    return d.runUnikraft;
  }

  async runOsv(opts: RunOsvOptions): Promise<string> {
    const d = await this.request<{ runOsv: string }>(
      `mutation($i:RunOsvInput!){ runOsv(input:$i) }`,
      { i: opts },
    );
    return d.runOsv;
  }

  async runFlavor(opts: RunFlavorOptions): Promise<string> {
    const d = await this.request<{ runFlavor: string }>(
      `mutation($i:RunFlavorInput!){ runFlavor(input:$i) }`,
      { i: opts },
    );
    return d.runFlavor;
  }

  // ---- exec / interactive shell ----------------------------------------------

  async #openShell(
    machineId: string,
    command: string[],
    env: string[],
    rows: number,
    cols: number,
  ): Promise<ShellSessionInfo> {
    const d = await this.request<{ openShell: ShellSessionInfo }>(
      `mutation($machineId:String!,$command:[String!]!,$env:[String!]!,$rows:Int!,$cols:Int!){
         openShell(machineId:$machineId, command:$command, env:$env, rows:$rows, cols:$cols){
           id machineId finished truncated
         }
       }`,
      { machineId, command, env, rows, cols },
    );
    return d.openShell;
  }

  async #closeShell(sessionId: string): Promise<void> {
    await this.request(
      `mutation($sessionId:String!){ closeShell(sessionId:$sessionId) }`,
      { sessionId },
    ).catch(() => {
      /* already gone — closing twice is not an error */
    });
  }

  /**
   * Run a command to completion: `openShell` (with `command` set, so it runs
   * that instead of an interactive login shell) -> subscribe to its output ->
   * wait for the exit code -> `closeShell`. See daemon/README.md's "Interactive
   * shells over GraphQL" for why it's three operations, not one — a
   * subscription cannot carry input, so a terminal (and this one-shot
   * variant of it) is always assembled from a mutation, a subscription, and
   * further mutations. `closeShell` runs from a `finally`, so it happens
   * whether the command finished, the subscription errored, or the caller's
   * code above threw.
   */
  async exec(
    id: string,
    command: string[],
    opts: { env?: string[] } = {},
  ): Promise<{ exitCode: number; output: Uint8Array }> {
    const session = await this.#openShell(id, command, opts.env ?? [], 24, 80);
    const chunks: Uint8Array[] = [];
    try {
      const exitCode = await new Promise<number>((resolve, reject) => {
        const unsub = this.subscribe(
          `subscription($sessionId:String!){ shellOutput(sessionId:$sessionId){ ${SHELL_OUTPUT_FIELDS} } }`,
          { sessionId: session.id },
          {
            onNext: (data) => {
              const p = data?.shellOutput;
              if (!p) return;
              if (p.dataBase64) chunks.push(base64ToBytes(p.dataBase64));
              if (p.exitCode !== null && p.exitCode !== undefined) {
                unsub();
                resolve(p.exitCode);
              }
            },
            onError: (e) => {
              unsub();
              reject(e);
            },
            // The daemon always yields an ExitCode frame right before ending
            // the stream (daemon/src/shell.rs's Session::subscribe breaks
            // immediately after it), so a `complete` with no exit code seen
            // yet means the session was torn down from elsewhere mid-command.
            // Resolve rather than hang forever; -1 flags the abnormal end.
            onComplete: () => resolve(-1),
          },
        );
      });
      return { exitCode, output: concatBytes(chunks) };
    } finally {
      await this.#closeShell(session.id);
    }
  }

  /**
   * Open a live interactive session. Same `openShell` + `shellOutput`
   * subscription as {@link exec}, but returns a handle instead of collecting
   * to completion.
   */
  async shell(
    id: string,
    opts: { command?: string[]; env?: string[]; rows?: number; cols?: number } = {},
  ): Promise<ShellSession> {
    const session = await this.#openShell(
      id,
      opts.command ?? [],
      opts.env ?? [],
      opts.rows ?? 24,
      opts.cols ?? 80,
    );

    const outputCbs = new Set<(chunk: Uint8Array) => void>();
    const exitCbs = new Set<(code: number) => void>();
    let exited = false;
    let exitCode = -1;
    // The daemon buffers shellOutput from the moment the session opens (see
    // daemon/README.md — "a correctness requirement, not an optimization"),
    // precisely because the subscribe() below is necessarily a separate
    // round trip from the openShell mutation above it. Mirror that here: a
    // caller can `await client.shell(id)` and only get around to calling
    // `onOutput`/`onExit` after further awaits, by which point events may
    // already have arrived — so every event is buffered regardless of
    // whether a listener exists yet, and replayed to each callback as it
    // registers (not just the first one), rather than only forwarded live.
    const outputBuffer: Uint8Array[] = [];

    const finish = (code: number) => {
      if (exited) return;
      exited = true;
      exitCode = code;
      exitCbs.forEach((cb) => cb(code));
    };

    const unsub = this.subscribe(
      `subscription($sessionId:String!){ shellOutput(sessionId:$sessionId){ ${SHELL_OUTPUT_FIELDS} } }`,
      { sessionId: session.id },
      {
        onNext: (data) => {
          const p = data?.shellOutput;
          if (!p) return;
          if (p.dataBase64) {
            const bytes = base64ToBytes(p.dataBase64);
            outputBuffer.push(bytes);
            outputCbs.forEach((cb) => cb(bytes));
          }
          if (p.exitCode !== null && p.exitCode !== undefined) finish(p.exitCode);
        },
        onError: () => finish(-1),
        onComplete: () => finish(-1),
      },
    );

    return {
      id: session.id,
      write: (data) => {
        const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
        this.request(
          `mutation($sessionId:String!,$dataBase64:String!){
             sendShellInput(sessionId:$sessionId, dataBase64:$dataBase64)
           }`,
          { sessionId: session.id, dataBase64: bytesToBase64(bytes) },
        ).catch(() => {
          /* nowhere synchronous to report this; the session going quiet is the signal */
        });
      },
      resize: (rows, cols) => {
        this.request(
          `mutation($sessionId:String!,$rows:Int!,$cols:Int!){
             resizeShell(sessionId:$sessionId, rows:$rows, cols:$cols)
           }`,
          { sessionId: session.id, rows, cols },
        ).catch(() => {});
      },
      close: () => {
        unsub();
        void this.#closeShell(session.id);
      },
      onOutput: (cb) => {
        outputCbs.add(cb);
        for (const bytes of outputBuffer) cb(bytes);
      },
      onExit: (cb) => {
        exitCbs.add(cb);
        if (exited) cb(exitCode);
      },
    };
  }
}
