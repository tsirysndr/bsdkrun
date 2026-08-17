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
import type {
  AiAgent,
  AiSession,
  CommandResult,
  DockerContainer,
  DockerStatus,
  SandboxInfo,
  ShellOutput,
  ShellSessionInfo,
  SnapshotInfo,
} from "./types.js";

/** `Client.fromEnv()` reads these. */
export const URL_ENV = "BSDKRUN_URL";
export const TOKEN_ENV = "BSDKRUN_TOKEN";

// ---------------------------------------------------------------------------
// GraphQL input shapes — transcribed field-for-field from the `InputObject`s
// in daemon/src/graphql.rs (RunLinuxInput ~384, RunBsdInput ~406,
// RunNanosInput ~429, RunUnikraftInput ~445, RunSolo5Input ~465, RunOsvInput
// ~498, RunFlavorInput ~541, NetInput ~360, BsdOs ~324). Rust's `snake_case`
// fields arrive here
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
  attachDisk?: string[];
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

/**
 * Solo5 (MirageOS): runs under the `solo5-hvt` tender rather than libkrun.
 * The unikernel declares its own network and block devices in its `MFT1`
 * manifest note, so only what the host alone can know is asked for. Like
 * Unikraft there is no disk and no agent, so no volume/persist/repo/command
 * fields.
 */
export interface RunSolo5Options {
  /**
   * A `.hvt` binary, or a project directory whose `dist/` holds one (where
   * `mirage build` leaves it). Defaults to `"."`.
   */
  path?: string;
  /** Always a single vCPU — a value above 1 is warned about and ignored. */
  cpus?: number;
  mem?: number;
  net?: NetOptions;
  /**
   * Backing files for declared block devices, each `"NAME=FILE"`. The
   * `NAME=` may be omitted when the unikernel declares exactly one.
   */
  block?: string[];
  /** Arguments passed to the unikernel itself, e.g. MirageOS's `"--ipv4=10.0.0.2/24"`. */
  args?: string[];
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
  cpus mem volume stateDir createdAt finishedAt network netIp origin
  ports { bind host guest }
`;
const AI_AGENT_FIELDS = `id label flavor description installed running`;
const AI_SESSION_FIELDS = `id name agent running workspace createdAt`;

const DOCKER_STATUS_FIELDS = `
  running machineId machineRunning socket socketReady apiPort version
  containers images mounts disk diskSize
`;
const DOCKER_CONTAINER_FIELDS = `id name image command state status ports created`;

const SNAPSHOT_FIELDS = `
  id name machineId machineName kind image path parent description
  cpus mem size createdAt ports { bind host guest }
`;
const COMMAND_RESULT_FIELDS = `exitCode stdout stderr`;

/** Options for {@link Client.aiStart}. */
export interface AiStartOptions {
  cpus?: number;
  mem?: number;
  /** A directory **on the engine's host** to share, at the same path. */
  workspace?: string;
  /** Boot a second sandbox rather than reusing the running one. */
  new?: boolean;
}

/** Options for {@link Client.dockerStart}. All optional — the empty object is
 * what `bsdkrun docker start` with no flags does. */
export interface DockerStartOptions {
  cpus?: number;
  mem?: number;
  /** Host directories to share, each `PATH` or `HOST:GUEST`. */
  mounts?: string[];
  /** Do not share `$HOME` (shared by default). */
  noHome?: boolean;
  /** Where published ports bind on the host: `mirror` (default) or an IP. */
  publishBind?: string;
  /** Give the image store a dedicated disk of this size, e.g. `60G`. */
  diskSize?: string;
}

/** Options for {@link Client.branch}. */
export interface BranchOptions {
  /** Name for the new machine; generated when absent. */
  name?: string;
  /** Defaults to what the snapshot recorded. */
  cpus?: number;
  /** Defaults to what the snapshot recorded. */
  mem?: number;
  /** Host↔guest forwards, each `"[BIND:]HOST:GUEST"`. Empty inherits the snapshot's. */
  ports?: string[];
  /** Forward nothing, ignoring what the snapshot recorded. */
  noPorts?: boolean;
}

/** A GraphQL `Snapshot` as the SDK's {@link SnapshotInfo}. */
function toSnapshot(s: Record<string, any>): SnapshotInfo {
  return {
    id: String(s.id),
    name: String(s.name),
    machineId: String(s.machineId ?? ""),
    machineName: String(s.machineName ?? ""),
    kind: String(s.kind ?? ""),
    image: String(s.image ?? ""),
    path: String(s.path ?? ""),
    parent: (s.parent as string | null) ?? null,
    description: String(s.description ?? ""),
    cpus: Number(s.cpus ?? 0),
    mem: Number(s.mem ?? 0),
    ports: (s.ports as SnapshotInfo["ports"]) ?? [],
    size: (s.size as string | null) ?? null,
    createdAt: Number(s.createdAt ?? 0),
  };
}
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

  // ---- ai agents ----------------------------------------------------------------
  //
  // A sandbox is a machine, so its terminal is the ordinary `shell(machineId,
  // { command })` — `aiShellCommand` supplies the argv.

  /** The coding agents, and whether each one's sandbox image is built. */
  async aiAgents(): Promise<AiAgent[]> {
    const d = await this.request<{ aiAgents: AiAgent[] }>(
      `{ aiAgents { ${AI_AGENT_FIELDS} } }`,
    );
    return d.aiAgents;
  }

  /** Agent sandboxes, newest first. */
  async aiSessions(): Promise<AiSession[]> {
    const d = await this.request<{ aiSessions: AiSession[] }>(
      `{ aiSessions { ${AI_SESSION_FIELDS} } }`,
    );
    return d.aiSessions;
  }

  /**
   * Start (or reuse) a sandbox; returns its machine id.
   *
   * `workspace` is a path **on the engine's host** — a remote daemon cannot
   * see your own filesystem. `new` boots a second sandbox against the same
   * saved login instead of reusing the running one.
   */
  async aiStart(agent: string, opts: AiStartOptions = {}): Promise<string> {
    const d = await this.request<{ aiStart: string }>(
      `mutation($i:AiStartInput!){ aiStart(input:$i) }`,
      {
        i: {
          agent,
          cpus: opts.cpus ?? null,
          mem: opts.mem ?? null,
          workspace: opts.workspace ?? null,
          new: opts.new ?? false,
        },
      },
    );
    return d.aiStart;
  }

  /** The argv that starts the agent's TUI — pass it to {@link Client.shell}. */
  async aiShellCommand(agent: string, machineId: string): Promise<string[]> {
    const d = await this.request<{ aiShellCommand: string[] }>(
      `query($a:String!,$m:String!){ aiShellCommand(agent:$a, machineId:$m) }`,
      { a: agent, m: machineId },
    );
    return d.aiShellCommand;
  }

  /** Stop an agent's sandboxes. Its saved login survives. */
  async aiStop(agent: string): Promise<CommandResult> {
    const d = await this.request<{ aiStop: CommandResult }>(
      `mutation($a:String!){ aiStop(agent:$a){ ${COMMAND_RESULT_FIELDS} } }`,
      { a: agent },
    );
    return d.aiStop;
  }

  /** Remove an agent's sandboxes, and unless `keepHome` its saved login too. */
  async aiRemove(agent: string, keepHome = false): Promise<CommandResult> {
    const d = await this.request<{ aiRemove: CommandResult }>(
      `mutation($a:String!,$k:Boolean!){ aiRemove(agent:$a, keepHome:$k){ ${COMMAND_RESULT_FIELDS} } }`,
      { a: agent, k: keepHome },
    );
    return d.aiRemove;
  }

  // ---- docker -----------------------------------------------------------------
  //
  // bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
  // socket. These drive the same engine the host's `docker` CLI does.

  /** Is the Docker engine up, and where is its socket? */
  async dockerStatus(): Promise<DockerStatus> {
    const d = await this.request<{ dockerStatus: DockerStatus }>(
      `{ dockerStatus { ${DOCKER_STATUS_FIELDS} } }`,
    );
    return d.dockerStatus;
  }

  /** Containers in the engine. `all: false` lists only running ones. */
  async dockerContainers(all = true): Promise<DockerContainer[]> {
    const d = await this.request<{ dockerContainers: DockerContainer[] }>(
      `query($all:Boolean!){ dockerContainers(all:$all){ ${DOCKER_CONTAINER_FIELDS} } }`,
      { all },
    );
    return d.dockerContainers;
  }

  /**
   * Start (or resume) the engine, returning its status once it answers.
   *
   * Idempotent: the VM has a fixed name, so this resumes the existing one
   * rather than creating a second.
   */
  async dockerStart(opts: DockerStartOptions = {}): Promise<DockerStatus> {
    const d = await this.request<{ dockerStart: DockerStatus }>(
      `mutation($i:DockerStartInput!){ dockerStart(input:$i){ ${DOCKER_STATUS_FIELDS} } }`,
      {
        i: {
          cpus: opts.cpus ?? null,
          mem: opts.mem ?? null,
          mounts: opts.mounts ?? [],
          noHome: opts.noHome ?? false,
          publishBind: opts.publishBind ?? null,
          diskSize: opts.diskSize ?? null,
        },
      },
    );
    return d.dockerStart;
  }

  /** Stop the engine. Images and containers stay on its disk. */
  async dockerStop(): Promise<CommandResult> {
    const d = await this.request<{ dockerStop: CommandResult }>(
      `mutation{ dockerStop{ ${COMMAND_RESULT_FIELDS} } }`,
    );
    return d.dockerStop;
  }

  /** start | stop | restart | kill | pause | unpause | rm. */
  async dockerContainer(
    action: string,
    ids: string | string[],
  ): Promise<CommandResult> {
    const d = await this.request<{ dockerContainer: CommandResult }>(
      `mutation($a:String!,$i:[String!]!){ dockerContainer(action:$a, ids:$i){ ${COMMAND_RESULT_FIELDS} } }`,
      { a: action, i: Array.isArray(ids) ? ids : [ids] },
    );
    return d.dockerContainer;
  }

  /** One container's logs (stdout+stderr, most recent `tail` lines). */
  async dockerLogs(id: string, tail = 200): Promise<string> {
    const d = await this.request<{ dockerContainerLogs: string }>(
      `query($i:String!,$t:Int!){ dockerContainerLogs(id:$i, tail:$t) }`,
      { i: id, t: tail },
    );
    return d.dockerContainerLogs;
  }

  // ---- snapshots ------------------------------------------------------------
  //
  // A snapshot is a copy-on-write clone of a machine's disk state: instant to
  // take, free until the two sides diverge. `branch` boots a new machine from
  // one; `restore`/`rollback` put one back over the machine it came from.

  /** Snapshots, newest first — all of them, or one machine's. */
  async snapshots(machine?: string): Promise<SnapshotInfo[]> {
    const d = await this.request<{ snapshots: any[] }>(
      `query($machine:String){ snapshots(machine:$machine){ ${SNAPSHOT_FIELDS} } }`,
      { machine: machine ?? null },
    );
    return d.snapshots.map(toSnapshot);
  }

  /**
   * Capture a machine's disk state. `name` defaults to `<machine>-<n>`.
   *
   * A BSD guest is powered off first — a mounted UFS cannot be cloned
   * consistently — so the machine is left stopped; call {@link start} to
   * bring it back.
   */
  async snapshot(id: string, name?: string, description = ""): Promise<SnapshotInfo> {
    const d = await this.request<{ snapshotMachine: any }>(
      `mutation($id:String!,$name:String,$description:String!){
         snapshotMachine(id:$id, name:$name, description:$description){ ${SNAPSHOT_FIELDS} }
       }`,
      { id, name: name ?? null, description },
    );
    return toSnapshot(d.snapshotMachine);
  }

  /** Delete snapshots and their data. Machines branched from them are unaffected. */
  async removeSnapshots(names: string[]): Promise<CommandResult> {
    const d = await this.request<{ removeSnapshots: CommandResult }>(
      `mutation($names:[String!]!){ removeSnapshots(names:$names){ ${COMMAND_RESULT_FIELDS} } }`,
      { names },
    );
    return d.removeSnapshots;
  }

  /**
   * Put a machine's disk state back to one of its snapshots.
   *
   * `force` (default) stops the machine first — it holds the very files being
   * replaced. `backup` (default) snapshots the state being overwritten, which
   * is a CoW clone and therefore free. The machine is left stopped.
   */
  async restore(
    id: string,
    snapshot: string,
    opts: { force?: boolean; backup?: boolean } = {},
  ): Promise<CommandResult> {
    const d = await this.request<{ restoreMachine: CommandResult }>(
      `mutation($id:String!,$snapshot:String!,$force:Boolean!,$backup:Boolean!){
         restoreMachine(id:$id, snapshot:$snapshot, force:$force, backup:$backup){ ${COMMAND_RESULT_FIELDS} }
       }`,
      { id, snapshot, force: opts.force ?? true, backup: opts.backup ?? true },
    );
    return d.restoreMachine;
  }

  /** Restore a machine to its most recent snapshot. */
  async rollback(
    id: string,
    opts: { force?: boolean; backup?: boolean } = {},
  ): Promise<CommandResult> {
    const d = await this.request<{ rollbackMachine: CommandResult }>(
      `mutation($id:String!,$force:Boolean!,$backup:Boolean!){
         rollbackMachine(id:$id, force:$force, backup:$backup){ ${COMMAND_RESULT_FIELDS} }
       }`,
      { id, force: opts.force ?? true, backup: opts.backup ?? true },
    );
    return d.rollbackMachine;
  }

  /**
   * Boot a NEW machine from a snapshot, returning its id.
   *
   * The snapshot is cloned, never booted in place, so the machine it came from
   * is untouched and one snapshot can be branched any number of times. With no
   * `ports`, the snapshot's forwards are inherited — with any host port that is
   * already taken swapped for a free one.
   */
  async branch(snapshot: string, opts: BranchOptions = {}): Promise<string> {
    const d = await this.request<{ branchSnapshot: string }>(
      `mutation($input:BranchInput!){ branchSnapshot(input:$input) }`,
      {
        input: {
          snapshot,
          name: opts.name ?? null,
          cpus: opts.cpus ?? null,
          mem: opts.mem ?? null,
          ports: opts.ports ?? [],
          noPorts: opts.noPorts ?? false,
        },
      },
    );
    return d.branchSnapshot;
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

  async runSolo5(opts: RunSolo5Options): Promise<string> {
    const d = await this.request<{ runSolo5: string }>(
      `mutation($i:RunSolo5Input!){ runSolo5(input:$i) }`,
      { i: opts },
    );
    return d.runSolo5;
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
