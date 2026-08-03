import { buildCreateArgs } from "./args.ts";
import { CommandFailedError, SandboxNotFoundError } from "./errors.ts";
import { runCli, spawnCli } from "./process.ts";
import {
  CommandResult,
  createSh,
  type Sh,
  type ShellRunOptions,
} from "./shell.ts";
import type { CreateOptions, SandboxInfo } from "./types.ts";

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
    createdAt: Number(row.created_at),
    finishedAt: num(row.finished_at),
  };
}

/**
 * A handle to a running (or stopped) bsdkrun microVM. Create one with
 * {@link Sandbox.create}, reconnect with {@link Sandbox.get}, or enumerate with
 * {@link Sandbox.list}.
 *
 * ```ts
 * const box = await Sandbox.create({ os: "linux", image: "alpine" });
 * await box.sh`echo hello`;
 * await box.exec(["uname", "-a"]);
 * await box.stop();
 * ```
 */
export class Sandbox {
  /** The machine's Docker-style short id. */
  readonly id: string;
  /** Host port forwarded to the guest's SSH, if the boot banner reported one. */
  readonly sshPort?: number;

  /** Run a shell script in the guest via a tagged template. */
  readonly sh: Sh;

  private constructor(id: string, sshPort?: number) {
    this.id = id;
    this.sshPort = sshPort;
    this.sh = createSh((script, opts) => this.#shRunner(script, opts));
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
    const match = all.find((m) => m.id === id || m.id.startsWith(id));
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
   * await box.exec(["ls", "-la", "/etc"]);
   * await box.exec("node", { args: ["-e", "console.log(1)"], env: { X: "1" } });
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
   * const { stdout } = await box.runCommand("uname", ["-a"]);
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

  /** Stop the machine (SIGTERM). */
  async stop(): Promise<void> {
    const res = await runCli(["stop", this.id]);
    if (res.exitCode !== 0) {
      throw new CommandFailedError(
        new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun stop"),
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
   * Manage key-based SSH in the guest: `setup`, `add-key`, `status`.
   *
   * ```ts
   * await box.ssh(["setup"]);                 // install local ~/.ssh/*.pub keys
   * await box.ssh(["add-key", "--key", key]); // append a key
   * ```
   */
  ssh(action: string[]): Promise<CommandResult> {
    return this.#agent("ssh", action);
  }

  /**
   * Manage tailscale in the guest: `setup`, `status`, `install`, `start`.
   * Pass an auth key via the option (forwarded as `TS_AUTHKEY`) or inline args.
   */
  tailscale(
    action: string[],
    opts: { authkey?: string } = {},
  ): Promise<CommandResult> {
    const env = opts.authkey ? { TS_AUTHKEY: opts.authkey } : undefined;
    return this.#agent("tailscale", action, env);
  }

  /** Configure systemd as PID 1: `setup`, `status`, `disable`. */
  systemd(action: string[]): Promise<CommandResult> {
    return this.#agent("systemd", action);
  }
}
