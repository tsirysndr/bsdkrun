import { CommandFailedError } from "./errors.js";
import { runCli } from "./process.js";
import { CommandResult } from "./shell.js";

/** An archive format a cache entry can be stored in. */
export type Compression = "gzip" | "zstd" | "estargz" | "none";

/** A stored cache entry, as `cache ls` reports it. */
export interface CacheEntry {
  /** The exact key it was saved under. */
  key: string;
  /** Guest path the tree came from. */
  path: string;
  compression: Compression;
  /** Archive size in bytes. */
  size: number;
  /** Unix seconds when it was saved. */
  created: number;
  /** `sha256:…` over the archive. */
  digest: string;
}

/** What a restore did. */
export interface RestoreResult {
  /** Whether anything was found. A miss is not an error. */
  restored: boolean;
  /** The key asked for. */
  requestedKey: string;
  /**
   * The entry actually used. Differs from {@link requestedKey} when a
   * `restoreKeys` prefix matched, and is undefined on a miss.
   */
  key?: string;
  /** Guest path it was restored into. */
  path?: string;
  size?: number;
  compression?: Compression;
  created?: number;
}

export interface SaveOptions {
  /** Key to store under. Make it name the content — a lockfile hash. */
  key: string;
  /** Archive format. Defaults to gzip. */
  compression?: Compression;
  /** Replace an entry that already has this key. */
  force?: boolean;
}

export interface RestoreOptions {
  key: string;
  /**
   * Where to restore to. Defaults to the directory the entry was saved from.
   */
  path?: string;
  /**
   * Prefixes to fall back on when the key misses, most preferred first. Within
   * a prefix the newest matching entry wins.
   */
  restoreKeys?: string[];
}

function mapResult(row: Record<string, unknown>): RestoreResult {
  return {
    restored: Boolean(row.restored),
    requestedKey: String(row.requested_key),
    key: row.key == null ? undefined : String(row.key),
    path: row.path == null ? undefined : String(row.path),
    size: row.size == null ? undefined : Number(row.size),
    compression: row.compression as Compression | undefined,
    created: row.created == null ? undefined : Number(row.created),
  };
}

async function json(args: string[], label: string): Promise<Record<string, unknown>> {
  const res = await runCli(args);
  if (res.exitCode !== 0) {
    throw new CommandFailedError(
      new CommandResult(res.stdout, res.stderr, res.exitCode, label),
    );
  }
  return JSON.parse(res.stdout || "{}") as Record<string, unknown>;
}

/**
 * Cached guest directories for one sandbox.
 *
 * Reached as `sandbox.cache`. Entries are keyed, so a rebuild can pick up where
 * the last one left off:
 *
 * ```ts
 * const hit = await box.cache.restore({ key, restoreKeys: ["deps-"] });
 * if (!hit.restored) {
 *   await box.exec(["npm", "ci"]);
 *   await box.cache.save("/app/node_modules", { key });
 * }
 * ```
 *
 * Where entries live — host disk or S3 — is host configuration, not an SDK
 * concern: set `BSDKRUN_CACHE_BACKEND` / `BSDKRUN_CACHE_S3_*`, or
 * `~/.config/bsdkrun/cache.toml`.
 */
export class Cache {
  constructor(private readonly id: string) {}

  /** Archive a guest directory under a key. */
  async save(path: string, opts: SaveOptions): Promise<CacheEntry> {
    const args = ["cache", "save", `${this.id}:${path}`, "--key", opts.key, "--json"];
    if (opts.compression) args.push("--compression", opts.compression);
    if (opts.force) args.push("--force");
    const row = await json(args, "bsdkrun cache save");
    return {
      key: String(row.key),
      path: String(row.path),
      compression: row.compression as Compression,
      size: Number(row.size),
      created: Number(row.created),
      digest: String(row.digest ?? ""),
    };
  }

  /**
   * Restore a stored tree. **A miss is not an error** — check `restored` on the
   * result rather than catching.
   */
  async restore(opts: RestoreOptions): Promise<RestoreResult> {
    const target = opts.path ? `${this.id}:${opts.path}` : this.id;
    const args = ["cache", "restore", target, "--key", opts.key, "--json"];
    if (opts.restoreKeys?.length) args.push("--restore-keys", ...opts.restoreKeys);
    return mapResult(await json(args, "bsdkrun cache restore"));
  }
}

/** List every stored cache entry, newest first. */
export async function listCaches(): Promise<CacheEntry[]> {
  const res = await runCli(["cache", "ls", "--json"]);
  if (res.exitCode !== 0) {
    throw new CommandFailedError(
      new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun cache ls"),
    );
  }
  const rows = JSON.parse(res.stdout || "[]") as Record<string, unknown>[];
  return rows.map((r) => ({
    key: String(r.key),
    path: String(r.path),
    compression: r.compression as Compression,
    size: Number(r.size),
    created: Number(r.created),
    digest: String(r.digest ?? ""),
  }));
}

/** Remove stored entries by key, or every one of them with `{ all: true }`. */
export async function removeCache(
  keys: string | string[] = [],
  opts: { all?: boolean } = {},
): Promise<void> {
  const list = Array.isArray(keys) ? keys : [keys];
  const args = ["cache", "rm"];
  if (opts.all) args.push("--all");
  else args.push(...list);
  const res = await runCli(args);
  if (res.exitCode !== 0) {
    throw new CommandFailedError(
      new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun cache rm"),
    );
  }
}

/** Namespace grouping host-level cache operations. */
export const caches = {
  list: listCaches,
  remove: removeCache,
};
