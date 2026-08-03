import { spawn } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import process from "node:process";

/**
 * Make sure the host has everything bsdkrun needs *before* the first CLI call,
 * the way the Homebrew formula and CI do it manually:
 *
 * - **macOS** — install Homebrew's `libkrun` (the `libkrun.dylib` the binary
 *   links) if it's missing.
 * - **Linux** — download our PVH-enabled `libkrun` fork (needed for BSD PVH
 *   boots; not packaged anywhere) and point the loader at it via
 *   `LD_LIBRARY_PATH`. `libkrunfw` (the bundled kernel lib it links) is a stock
 *   dependency you install from your distro / nixpkgs.
 *
 * Returns environment overrides to apply when spawning the CLI (empty on
 * macOS). Runs once; disable entirely with `BSDKRUN_NO_AUTO_INSTALL=1`.
 */

/** The PVH libkrun fork release the SDK pins to (override via env). */
const LIBKRUN_VERSION = process.env.BSDKRUN_LIBKRUN_VERSION || "v1.19.4-pvh";

export type PreflightEnv = Record<string, string>;

let done: Promise<PreflightEnv> | undefined;

interface Run {
  code: number;
  stdout: string;
  stderr: string;
}

function run(cmd: string, args: string[]): Promise<Run> {
  return new Promise((resolve) => {
    const child = spawn(cmd, args, { stdio: ["ignore", "pipe", "pipe"] });
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    child.stdout?.on("data", (d) => out.push(d));
    child.stderr?.on("data", (d) => err.push(d));
    child.on("error", () => resolve({ code: 127, stdout: "", stderr: "" }));
    child.on("close", (code) =>
      resolve({
        code: code ?? 0,
        stdout: Buffer.concat(out).toString("utf8"),
        stderr: Buffer.concat(err).toString("utf8"),
      }),
    );
  });
}

function disabled(): boolean {
  const v = process.env.BSDKRUN_NO_AUTO_INSTALL;
  return v === "1" || v === "true";
}

function cacheRoot(): string {
  const base =
    process.env.XDG_CACHE_HOME || join(homedir(), ".cache");
  return join(base, "bsdkrun");
}

// ---------------------------------------------------------------------------
// macOS — Homebrew libkrun
// ---------------------------------------------------------------------------

async function findBrew(): Promise<string | undefined> {
  for (const p of ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]) {
    if (existsSync(p)) return p;
  }
  return (await run("brew", ["--version"])).code === 0 ? "brew" : undefined;
}

async function ensureMacos(): Promise<PreflightEnv> {
  const brew = await findBrew();
  if (!brew) {
    throw new Error(
      "libkrun is required on macOS but Homebrew was not found. Install " +
        "Homebrew (https://brew.sh) and `brew install libkrun`, or set " +
        "BSDKRUN_NO_AUTO_INSTALL=1 if you manage libkrun another way.",
    );
  }

  const prefix = await run(brew, ["--prefix", "libkrun"]);
  if (prefix.code === 0 && existsSync(prefix.stdout.trim())) return {};

  process.stderr.write(
    "[bsdkrun] libkrun not found — installing via Homebrew " +
      "(one-time; set BSDKRUN_NO_AUTO_INSTALL=1 to skip)...\n",
  );
  await run(brew, ["tap", "libkrun/krun"]);
  const inst = await run(brew, ["install", "libkrun"]);
  if (inst.code !== 0) {
    process.stderr.write(
      "[bsdkrun] `brew install libkrun` failed:\n" +
        (inst.stderr || inst.stdout) +
        "\n[bsdkrun] Install it manually: brew install libkrun\n",
    );
    return {};
  }
  process.stderr.write("[bsdkrun] libkrun installed.\n");
  return {};
}

// ---------------------------------------------------------------------------
// Linux — download the PVH libkrun fork
// ---------------------------------------------------------------------------

/** Map Node's `process.arch` to the release's target-triple arch slug. */
export function linuxArchSlug(arch = process.arch): string | undefined {
  if (arch === "x64") return "x86_64";
  if (arch === "arm64") return "aarch64";
  return undefined;
}

/** The download URL for the PVH libkrun tarball for this arch. */
export function libkrunTarballUrl(
  archSlug: string,
  version = LIBKRUN_VERSION,
): string {
  const asset = `libkrun-pvh-${version}-${archSlug}-unknown-linux-gnu.tar.gz`;
  return `https://github.com/tsirysndr/libkrun/releases/download/${version}/${asset}`;
}

/** Prepend `dir` to a `PATH`-style variable value. */
function prepend(existing: string | undefined, dir: string): string {
  return existing ? `${dir}${delimiter}${existing}` : dir;
}

async function ensureLinux(): Promise<PreflightEnv> {
  // Explicit override: a caller-provided extracted lib dir wins, no download.
  const override = process.env.BSDKRUN_LIBKRUN_DIR;
  if (override) {
    return {
      LD_LIBRARY_PATH: prepend(process.env.LD_LIBRARY_PATH, override),
    };
  }

  const archSlug = linuxArchSlug();
  if (!archSlug) return {}; // unknown arch — let the CLI surface any error

  const dir = join(cacheRoot(), "libkrun-pvh", LIBKRUN_VERSION);
  const libDir = join(dir, "lib64");
  const marker = join(libDir, "libkrun.so");

  if (!existsSync(marker)) {
    process.stderr.write(
      `[bsdkrun] downloading PVH libkrun ${LIBKRUN_VERSION} (${archSlug}) ` +
        "(one-time; set BSDKRUN_NO_AUTO_INSTALL=1 to skip)...\n",
    );
    mkdirSync(dir, { recursive: true });
    const url = libkrunTarballUrl(archSlug);
    // curl + tar are ubiquitous on Linux; stream the tarball straight into tar.
    const dl = await run("/bin/sh", [
      "-c",
      `curl -fsSL ${JSON.stringify(url)} | tar xz -C ${JSON.stringify(dir)}`,
    ]);
    if (dl.code !== 0 || !existsSync(marker)) {
      process.stderr.write(
        "[bsdkrun] failed to download PVH libkrun:\n" +
          (dl.stderr || dl.stdout) +
          `\n[bsdkrun] Fetch it manually from ${url} and set ` +
          "BSDKRUN_LIBKRUN_DIR to its lib dir.\n",
      );
      return {};
    }
    process.stderr.write(`[bsdkrun] PVH libkrun ready at ${libDir}\n`);
  }

  return {
    LD_LIBRARY_PATH: prepend(process.env.LD_LIBRARY_PATH, libDir),
  };
}

/**
 * Ensure libkrun is available and return env overrides to spawn the CLI with.
 * A no-op (returns `{}`) when disabled or on an unsupported platform. Cached
 * after the first call.
 */
export function ensurePreflight(): Promise<PreflightEnv> {
  if (done) return done;
  done = (async () => {
    if (disabled()) return {};
    if (process.platform === "darwin") return ensureMacos();
    if (process.platform === "linux") return ensureLinux();
    return {};
  })();
  return done;
}

/** Reset preflight state (mainly for tests). */
export function resetPreflight(): void {
  done = undefined;
}
