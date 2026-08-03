import { accessSync, constants, existsSync } from "node:fs";
import { delimiter, dirname, join } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { BinaryNotFoundError } from "./errors.js";

let override: string | undefined;

/**
 * Force the SDK to use a specific `bsdkrun` binary, bypassing discovery.
 * Handy in tests or when running against a locally built debug binary.
 */
export function setBinaryPath(path: string): void {
  override = path;
  resolved = undefined;
}

/** Cross-runtime PATH lookup for an executable (Node/Deno/Bun). */
function which(name: string): string | undefined {
  const path = process.env.PATH ?? "";
  const exts = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT").split(";")
    : [""];
  for (const dir of path.split(delimiter)) {
    if (!dir) continue;
    for (const ext of exts) {
      const full = join(dir, name + ext);
      try {
        accessSync(full, constants.X_OK);
        return full;
      } catch {
        // not here / not executable — keep looking
      }
    }
  }
  return undefined;
}

/** Candidate locations, in priority order. */
function candidates(): string[] {
  const out: string[] = [];
  if (override) out.push(override);

  const env = process.env.BSDKRUN_BIN;
  if (env) out.push(env);

  // A `bsdkrun` already on PATH wins over in-repo builds.
  const onPath = which("bsdkrun");
  if (onPath) out.push(onPath);

  // In-repo dev: this file lives at sdk/typescript/src/binary.ts, so the repo
  // root is three levels up. Prefer a release build, fall back to debug.
  const here = dirname(fileURLToPath(import.meta.url));
  const repoRoot = join(here, "..", "..", "..");
  out.push(join(repoRoot, "target", "release", "bsdkrun"));
  out.push(join(repoRoot, "target", "debug", "bsdkrun"));

  return out;
}

let resolved: string | undefined;

/** Resolve (and cache) the path to the `bsdkrun` binary, or throw. */
export function resolveBinary(): string {
  if (resolved) return resolved;

  const searched = candidates();
  for (const c of searched) {
    if (c.includes("/") || c.includes("\\")) {
      if (existsSync(c)) {
        resolved = c;
        return c;
      }
    } else if (which(c)) {
      resolved = c;
      return c;
    }
  }
  throw new BinaryNotFoundError(searched);
}

/** Reset cached discovery state (mainly for tests). */
export function resetBinaryCache(): void {
  resolved = undefined;
  override = undefined;
}
