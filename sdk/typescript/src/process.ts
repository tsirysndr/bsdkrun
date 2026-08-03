import { spawn } from "node:child_process";
import process from "node:process";
import { resolveBinary } from "./binary.js";
import { cachedPreflightEnv, ensurePreflight } from "./preflight.js";

export interface RunOptions {
  /** Extra environment variables merged onto the current process env. */
  env?: Record<string, string>;
  /** Data piped to the child's stdin. */
  stdin?: string | Uint8Array;
  /** Abort the child early. */
  signal?: AbortSignal;
  /**
   * bsdkrun's global `--log-level` (0=off .. 5=trace). Defaults to 0 so the
   * SDK's captured output stays clean. Raise it for boot diagnostics.
   */
  logLevel?: number;
}

export interface RawResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

/** Prepend the global `--log-level` flag. */
function withGlobals(args: string[], logLevel: number | undefined): string[] {
  return ["--log-level", String(logLevel ?? 0), ...args];
}

/**
 * Run `bsdkrun <args>` to completion, buffering stdout/stderr. Runtime-agnostic
 * (Node, Deno, Bun) — uses `node:child_process`.
 */
export async function runCli(
  args: string[],
  opts: RunOptions = {},
): Promise<RawResult> {
  const preflightEnv = await ensurePreflight();
  const bin = resolveBinary();
  return new Promise((resolve, reject) => {
    const child = spawn(bin, withGlobals(args, opts.logLevel), {
      env: { ...process.env, ...preflightEnv, ...opts.env },
      stdio: ["pipe", "pipe", "pipe"],
      signal: opts.signal,
    });

    const stdout: Buffer[] = [];
    const stderr: Buffer[] = [];
    child.stdout.on("data", (d: Buffer) => stdout.push(d));
    child.stderr.on("data", (d: Buffer) => stderr.push(d));

    child.on("error", reject);
    child.on("close", (code) => {
      resolve({
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
        exitCode: code ?? 0,
      });
    });

    if (opts.stdin !== undefined && child.stdin) {
      child.stdin.write(opts.stdin);
      child.stdin.end();
    } else {
      child.stdin?.end();
    }
  });
}

export interface SpawnOptions extends RunOptions {
  /** How to wire stdio. Defaults to inherit for an interactive child. */
  stdio?: "inherit" | "pipe";
}

/**
 * Spawn `bsdkrun <args>` without waiting — for interactive sessions
 * (`shell`) or live streams (`logs -f`). Returns the Node ChildProcess.
 */
export function spawnCli(args: string[], opts: SpawnOptions = {}) {
  const bin = resolveBinary();
  const stdio = opts.stdio ?? "inherit";
  return spawn(bin, withGlobals(args, opts.logLevel), {
    env: { ...process.env, ...cachedPreflightEnv(), ...opts.env },
    stdio,
    signal: opts.signal,
  });
}
