import { CommandFailedError } from "./errors.ts";
import { runCli } from "./process.ts";
import { CommandResult } from "./shell.ts";
import type { VolumeInfo } from "./types.ts";

function mapVolume(row: Record<string, unknown>): VolumeInfo {
  return {
    name: String(row.name),
    guest: (row.guest as string | null) ?? null,
    base: (row.base as string | null) ?? null,
    path: String(row.path),
    size: String(row.size),
    createdAt: row.created_at == null ? null : Number(row.created_at),
    tracked: Boolean(row.tracked),
  };
}

/** List persistent volumes. */
export async function listVolumes(): Promise<VolumeInfo[]> {
  const res = await runCli(["volume", "ls", "--json"]);
  if (res.exitCode !== 0) {
    throw new CommandFailedError(
      new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun volume ls"),
    );
  }
  const rows = JSON.parse(res.stdout || "[]") as Record<string, unknown>[];
  return rows.map(mapVolume);
}

/** Remove one or more volumes (and their data). */
export async function removeVolume(
  names: string | string[],
  opts: { force?: boolean } = {},
): Promise<void> {
  const list = Array.isArray(names) ? names : [names];
  const args = ["volume", "rm"];
  if (opts.force) args.push("--force");
  args.push(...list);
  const res = await runCli(args);
  if (res.exitCode !== 0) {
    throw new CommandFailedError(
      new CommandResult(res.stdout, res.stderr, res.exitCode, "bsdkrun volume rm"),
    );
  }
}

/** Namespace grouping volume operations. */
export const volumes = {
  list: listVolumes,
  remove: removeVolume,
};
