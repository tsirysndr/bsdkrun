import { buildCreateArgs } from "./args.js";
import { readAgentPort } from "./agent-protocol.js";
import { CommandFailedError, SandboxNotFoundError } from "./errors.js";
import { Cache } from "./cache.js";
import { FileSystem } from "./filesystem.js";
import { runCli, spawnCli } from "./process.js";
import {
  CommandResult,
  createSh,
  type Sh,
  type ShellRunOptions,
} from "./shell.js";
import { Terminal, type TerminalOptions } from "./terminal.js";
import type { CreateOptions, SandboxInfo } from "./types.js";

/** Advanced options for {@link Sandbox.exec}. */
export interface ExecOptions {
  /** Arguments, when `command` is passed as a bare program name string. */
  args?: string[];
  /** Environment variables for the command (`-e K=V`). */
  env?: Record<string, string>;
  /** Allocate a pseudo-TTY in the guest (`-t`). */
  tty?: boolean;
  /** Data piped to the command's stdin. */
  stdin?: string | Uint8Array;
  /** Working directory inside the guest (emulated via `sh -c 'cd …'`). */
  cwd?: string;
  /** Abort the command. */
  signal?: AbortSignal;
  /** Throw {@link CommandFailedError} on a non-zero exit (default false). */
  throwOnError?: boolean;
  /** Per-command bsdkrun log level (default 0 — quiet). */
  logLevel?: number;
  /** Receive stdout incrementally while the command runs (it is also captured). */
  onStdout?: (chunk: Uint8Array) => void;
  /** Receive stderr incrementally while the command runs (it is also captured). */
  onStderr?: (chunk: Uint8Array) => void;
}

/** Options for {@link Sandbox.logs}. */
export interface LogsOptions {
  /** Show bsdkrun's own boot log instead of the guest console. */
  boot?: boolean;
}

const ID_RE = /^[0-9a-f]{12}$/;
const SSH_PORT_RE = /ssh -p (\d+)/;

/** Map a `bsdkrun ps --json` row to a typed {@link SandboxInfo}. */
function mapInfo(row: Record<string, unknown>): SandboxInfo {
  const num = (v: unknown): number | null =>
    v == null ? null : Number(v);
  return {
    id: String(row.id),
    name: (row.name as string | null) ?? null,
    image: String(row.image),
    kind: String(row.kind),
    command: String(row.command ?? ""),
    status: row.running ? "running" : "exited",
    running: Boolean(row.running),
    exitCode: num(row.exit_code),
    pid: num(row.pid),
    detached: Boolean(row.detached),
    cpus: Number(row.cpus),
    mem: Number(row.mem),
    volume: (row.volume as string | null) ?? null,
    stateDir: String(row.state_dir),
    network: (row.network as string | null) ?? null,
    netIp: (row.net_ip as string | null) ?? null,
    ports: (row.ports as SandboxInfo["ports"] | undefined) ?? [],
    createdAt: Number(row.created_at),
    finishedAt: num(row.finished_at),
  };
}

/**
 * Map a GraphQL `Machine` object (the `MACHINE_FIELDS` selection in
 * client.ts, matching `daemon/src/graphql.rs`'s `Machine`) to a typed
 * {@link SandboxInfo} — the `Client` counterpart to {@link mapInfo} above.
 *
 * The schema is already camelCase, so this is close to a pass-through; the
 * one real conversion is `createdAt`/`finishedAt`, which the daemon sends as
 * numeric strings (GraphQL has no 64-bit integer type) and `SandboxInfo`
 * wants as numbers, exactly like the CLI's JSON.
 */
export function fromGraphQLMachine(m: Record<string, unknown>): SandboxInfo {
  const num = (v: unknown): number | null => (v == null ? null : Number(v));
  return {
    id: String(m.id),
    name: (m.name as string | null) ?? null,
    image: String(m.image),
    kind: String(m.kind),
    command: String(m.command ?? ""),
    status: m.running ? "running" : "exited",
    running: Boolean(m.running),
    exitCode: num(m.exitCode),
    pid: num(m.pid),
    detached: Boolean(m.detached),
    cpus: Number(m.cpus),
    mem: Number(m.mem),
    volume: (m.volume as string | null) ?? null,
    stateDir: String(m.stateDir),
    network: (m.network as string | null) ?? null,
    netIp: (m.netIp as string | null) ?? null,
    ports: (m.ports as SandboxInfo["ports"] | undefined) ?? [],
    createdAt: Number(m.createdAt),
    finishedAt: num(m.finishedAt),
  };
}

/**
 * A handle to a running (or stopped) bsdkrun microVM. Create one with
 * {@link Sandbox.create}, reconnect with {@link Sandbox.get}, or enumerate with
 * {@link Sandbox.list}.
 *
 * ```ts
 * const sbx = await Sandbox.create({ os: "linux", image: "alpine" });
 * await sbx.sh`echo hello`;
 * await sbx.exec(["uname", "-a"]);
 * await sbx.stop();
 * ```
 */
export class Sandbox {
  /** The machine's Docker-style short id. */
  readonly id: string;
  /** Host port forwarded to the guest's SSH, if the boot banner reported one. */
  readonly sshPort?: number;

  /** Run a shell script in the guest via a tagged template. */
  readonly sh: Sh;

  /** Read and write files in the guest. */
  readonly fs: FileSystem;

  /** Save and restore guest directories under a key. */
  readonly cache: Cache;

  #stateDirCache?: string;

  private constructor(id: string, sshPort?: number) {
    this.id = id;
    this.sshPort = sshPort;
    this.sh = createSh((script, opts) => this.#shRunner(script, opts));
    this.fs = new FileSystem(id);
    this.cache = new Cache(id);
  }

  /** Boot a new microVM and return a handle to it. */
  static async create(opts: CreateOptions): Promise<Sandbox> {
    const res = await runCli(buildCreateArgs(opts), {
      logLevel: opts.logLevel ?? 1,
    });
    if (res.exitCode !== 0) {
      throw new CommandFailedError(
        new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun create"),
      );
    }
    // Detached runs print just the machine id on stdout.
    const id = res.stdout
      .split("\n")
      .map((l) => l.trim())
      .filter((l) => ID_RE.test(l))
      .at(-1);
    if (!id) {
      throw new CommandFailedError(
        new CommandResult(
          res.stdout,
          res.stderr,
          res.exitCode,
          "bsdkrun create (no machine id in output)",
        ),
      );
    }
    const sshPort = res.stderr.match(SSH_PORT_RE)?.[1];
    return new Sandbox(id, sshPort ? Number(sshPort) : undefined);
  }

  /** Reconnect to an existing machine by id (a unique prefix is enough). */
  static async get(id: string): Promise<Sandbox> {
    const all = await Sandbox.list({ all: true });
    const match = all.find((m) => m.id === id || m.id.startsWith(id) || m.name === id);
    if (!match) throw new SandboxNotFoundError(id);
    return new Sandbox(match.id);
  }

  /** List machines. `all: true` includes exited ones (default running only). */
  static async list(opts: { all?: boolean } = {}): Promise<SandboxInfo[]> {
    const args = ["ps", "--json"];
    if (opts.all) args.push("--all");
    const res = await runCli(args);
    if (res.exitCode !== 0) {
      throw new CommandFailedError(
        new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun ps"),
      );
    }
    const rows = JSON.parse(res.stdout || "[]") as Record<string, unknown>[];
    return rows.map(mapInfo);
  }

  /** The runner backing the `sh` tagged template — a shell exec in the guest. */
  #shRunner(script: string, opts: ShellRunOptions): Promise<CommandResult> {
    return this.exec(["/bin/sh", "-c", script], {
      env: opts.env,
      signal: opts.signal,
      throwOnError: false,
    });
  }

  /**
   * Run a command in the guest through its exec agent. The primary programmatic
   * entrypoint — richer than {@link sh}: pass argv directly (no shell parsing),
   * env, a PTY, stdin, or a working directory.
   *
   * ```ts
   * await sbx.exec(["ls", "-la", "/etc"]);
   * await sbx.exec("node", { args: ["-e", "console.log(1)"], env: { X: "1" } });
   * ```
   */
  async exec(
    command: string | string[],
    opts: ExecOptions = {},
  ): Promise<CommandResult> {
    let argv = Array.isArray(command)
      ? [...command]
      : [command, ...(opts.args ?? [])];

    if (opts.cwd) {
      // Emulate a working directory: cd, drop it, then exec the real argv.
      argv = ["/bin/sh", "-c", 'cd "$1" && shift && exec "$@"', "sh", opts.cwd, ...argv];
    }

    const args = ["exec"];
    if (opts.tty) args.push("-t");
    for (const [k, v] of Object.entries(opts.env ?? {})) {
      args.push("-e", `${k}=${v}`);
    }
    args.push(this.id, ...argv);

    const res = await runCli(args, {
      stdin: opts.stdin,
      signal: opts.signal,
      logLevel: opts.logLevel,
      onStdout: opts.onStdout,
      onStderr: opts.onStderr,
    });
    const result = new CommandResult(
      res.stdout,
      res.stderr,
      res.exitCode,
      `exec ${argv.join(" ")}`,
    );
    if (opts.throwOnError) result.throwIfFailed();
    return result;
  }

  /**
   * Vercel-Sandbox-style alias for {@link exec}: a program plus its args.
   *
   * ```ts
   * const { stdout } = await sbx.runCommand("uname", ["-a"]);
   * ```
   */
  runCommand(
    command: string,
    args: string[] = [],
    opts: Omit<ExecOptions, "args"> = {},
  ): Promise<CommandResult> {
    return this.exec(command, { ...opts, args });
  }

  /** Read the machine's console log. */
  async logs(opts: LogsOptions = {}): Promise<string> {
    const args = ["logs"];
    if (opts.boot) args.push("--boot");
    args.push(this.id);
    const res = await runCli(args);
    return res.stdout;
  }

  /**
   * Follow the live console (`logs -f`). Returns the spawned child; consume
   * `child.stdout` or let it inherit the terminal. Kill it to stop following.
   */
  followLogs(opts: LogsOptions & { stdio?: "inherit" | "pipe" } = {}) {
    const args = ["logs", "-f"];
    if (opts.boot) args.push("--boot");
    args.push(this.id);
    return spawnCli(args, { stdio: opts.stdio ?? "pipe" });
  }

  /**
   * Attach an interactive shell to the machine (inherits the terminal). Returns
   * the child process; await `child` completion via `once("exit")`.
   */
  shell() {
    return spawnCli(["shell", this.id], { stdio: "inherit" });
  }

  /** Fetch this machine's current status row, or null if it's gone. */
  async status(): Promise<SandboxInfo | null> {
    const all = await Sandbox.list({ all: true });
    return all.find((m) => m.id === this.id) ?? null;
  }

  /** Whether the machine is currently running. */
  async isRunning(): Promise<boolean> {
    return (await this.status())?.running ?? false;
  }

  /**
   * Stop the machine. BSD guests are cleanly powered off (so their UFS is
   * consistent for the next {@link start}); Linux is SIGTERM'd.
   */
  async stop(): Promise<void> {
    await this.#lifecycle(["stop", this.id], "bsdkrun stop");
  }

  /**
   * Restart a stopped machine in place — same id, image/resources/volume, and
   * its own disk/rootfs, so snapshot + runtime data resume (like `docker
   * start`). Re-joins its recorded network. Boots detached.
   */
  async start(): Promise<void> {
    await this.#lifecycle(["start", this.id], "bsdkrun start");
  }

  /** Remove the machine and its state. `force` stops it first if running. */
  async remove(opts: { force?: boolean } = {}): Promise<void> {
    const args = ["rm"];
    if (opts.force) args.push("--force");
    args.push(this.id);
    await this.#lifecycle(args, "bsdkrun rm");
  }

  /** Change the recorded vCPU / RAM. Applies on the next {@link start}. */
  async update(opts: { cpus?: number; mem?: number }): Promise<void> {
    const args = ["update", this.id];
    if (opts.cpus != null) args.push("--cpus", String(opts.cpus));
    if (opts.mem != null) args.push("--mem", String(opts.mem));
    await this.#lifecycle(args, "bsdkrun update");
  }

  /**
   * Join or switch this machine to a global network. Applies on the next
   * {@link start} (a VM's NIC is fixed at boot). Create the network first with
   * {@link networks.create}.
   */
  async connectNetwork(network: string): Promise<void> {
    await this.#lifecycle(
      ["network", "connect", this.id, network],
      "bsdkrun network connect",
    );
  }

  /** Detach this machine from its network. Applies on the next {@link start}. */
  async disconnectNetwork(): Promise<void> {
    await this.#lifecycle(
      ["network", "disconnect", this.id],
      "bsdkrun network disconnect",
    );
  }

  /** Run a fire-and-forget lifecycle CLI command, throwing on failure. */
  async #lifecycle(args: string[], label: string): Promise<void> {
    const res = await runCli(args);
    if (res.exitCode !== 0) {
      throw new CommandFailedError(
        new CommandResult(res.stdout, res.stderr, res.exitCode, label),
      );
    }
  }

  /** Run an in-guest agent CLI family (`ssh`, `tailscale`, `systemd`). */
  async #agent(
    family: "ssh" | "tailscale" | "systemd",
    action: string[],
    env?: Record<string, string>,
  ): Promise<CommandResult> {
    const res = await runCli([family, this.id, ...action], { env });
    const result = new CommandResult(
      res.stdout,
      res.stderr,
      res.exitCode,
      `${family} ${action.join(" ")}`,
    );
    return result.throwIfFailed();
  }

  /**
   * Key-based SSH in the guest, via the agent. Typed helpers plus `.raw()` for
   * arbitrary args.
   *
   * ```ts
   * await sbx.ssh.setup();                      // install local ~/.ssh/*.pub
   * await sbx.ssh.setup({ user: "tsiry", key: "~/.ssh/work.pub" });
   * await sbx.ssh.addKey("ssh-ed25519 AAAA...");
   * await sbx.ssh.status();
   * ```
   */
  get ssh() {
    const run = (action: string[]) => this.#agent("ssh", action);
    return {
      /** `ssh setup` — install keys (local `~/.ssh/*.pub` when none given). */
      setup: (opts: SshSetupOptions = {}) => {
        const a = ["setup"];
        if (opts.user) a.push("--user", opts.user);
        for (const k of keyList(opts.key)) a.push("--key", k);
        return run(a);
      },
      /** `ssh add-key` — append one or more authorized keys. */
      addKey: (key: string | string[]) => {
        const a = ["add-key"];
        for (const k of keyList(key)) a.push("--key", k);
        return run(a);
      },
      /** `ssh status` — sshd state + installed key count. */
      status: () => run(["status"]),
      /** Escape hatch: pass raw args to `bsdkrun ssh <id> …`. */
      raw: (action: string[]) => run(action),
    };
  }

  /**
   * Tailscale in the guest, via the agent. Installs the OS-native way, runs
   * `tailscaled` (userspace networking by default), and joins your tailnet.
   *
   * ```ts
   * await sbx.tailscale.up({ authkey: "tskey-auth-...", hostname: "web" });
   * await sbx.tailscale.status();
   * ```
   */
  get tailscale() {
    const run = (action: string[], authkey?: string) =>
      this.#agent("tailscale", action, authkey ? { TS_AUTHKEY: authkey } : undefined);
    return {
      /** `tailscale setup` — install + start + `tailscale up` (join a tailnet). */
      up: (opts: TailscaleUpOptions = {}) => {
        const a = ["setup"];
        if (opts.hostname) a.push("--hostname", opts.hostname);
        if (opts.args) a.push(...opts.args);
        return run(a, opts.authkey);
      },
      /** Alias for {@link up}. */
      setup: (opts: TailscaleUpOptions = {}) => this.tailscale.up(opts),
      /** `tailscale install` — install the binaries only. */
      install: () => run(["install"]),
      /** `tailscale start` — start `tailscaled` only. */
      start: (opts: { kernelTun?: boolean } = {}) =>
        run(["start", ...(opts.kernelTun ? ["--kernel-tun"] : [])]),
      /** `tailscale status` — who am I / peers. */
      status: () => run(["status"]),
      /** Escape hatch: pass raw args to `bsdkrun tailscale <id> …`. */
      raw: (action: string[], authkey?: string) => run(action, authkey),
    };
  }

  /**
   * Configure systemd as PID 1 in a Linux guest. Boot on a volume so the change
   * persists across reboots.
   *
   * **Only works on systemd-based Linux guests — debian / ubuntu / fedora.**
   * Alpine (no systemd) and the BSD guests (freebsd / netbsd) don't support it;
   * `setup` errors clearly there. Manage BSD services with `rc.d` instead.
   *
   * ```ts
   * await sbx.systemd.setup();   // install + mark for next boot (debian/ubuntu/fedora)
   * await sbx.systemd.status();
   * ```
   */
  get systemd() {
    const run = (action: string[]) => this.#agent("systemd", action);
    return {
      /** `systemd setup` — install systemd + agent unit, mark for next boot. */
      setup: () => run(["setup"]),
      /** `systemd status` — reports whether PID 1 is systemd. */
      status: () => run(["status"]),
      /** `systemd disable` — remove the marker. */
      disable: () => run(["disable"]),
    };
  }

  /** Resolve (and cache) this machine's state dir, for direct agent access. */
  async #stateDir(): Promise<string> {
    if (this.#stateDirCache) return this.#stateDirCache;
    const info = await this.status();
    if (!info) throw new SandboxNotFoundError(this.id);
    this.#stateDirCache = info.stateDir;
    return info.stateDir;
  }

  /**
   * Open an interactive PTY session in the guest, streamed over the agent's TCP
   * protocol — the browser-terminal entrypoint. Feed {@link Terminal.onData}
   * into xterm.js, forward its input to {@link Terminal.write}, and wire its
   * `onResize` to {@link Terminal.resize}. See `examples/08-browser-terminal`.
   *
   * ```ts
   * const term = await sbx.terminal({ cols: 120, rows: 30 });
   * term.onData((chunk) => process.stdout.write(chunk));
   * term.write("uname -a\n");
   * ```
   */
  async terminal(opts: TerminalOptions = {}): Promise<Terminal> {
    const stateDir = await this.#stateDir();
    const port = readAgentPort(stateDir, this.id);
    return Terminal.open(opts.host ?? "127.0.0.1", port, this.id, opts);
  }
}

/** Normalize a key option (literal key or `.pub` path) to a list. */
function keyList(key: string | string[] | undefined): string[] {
  if (!key) return [];
  return Array.isArray(key) ? key : [key];
}

/** Options for {@link Sandbox.ssh.setup}. */
export interface SshSetupOptions {
  /** Target user (default root). */
  user?: string;
  /**
   * Public key(s): a literal `ssh-...` string or a local `.pub` file path (the
   * CLI inlines file contents). Omit to install your local `~/.ssh/id_*.pub`.
   */
  key?: string | string[];
}

/** Options for {@link Sandbox.tailscale.up}. */
export interface TailscaleUpOptions {
  /** Tailnet auth key (forwarded as `TS_AUTHKEY`, kept off the arg list). */
  authkey?: string;
  /** Machine name on the tailnet. */
  hostname?: string;
  /** Extra args passed through to `tailscale up`. */
  args?: string[];
}
