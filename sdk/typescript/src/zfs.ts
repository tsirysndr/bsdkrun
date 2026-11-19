import { CommandFailedError } from "./errors.js";
import { runCli } from "./process.js";
import { CommandResult } from "./shell.js";
import type { ZfsAttachmentInfo, ZfsBranchInfo, ZfsVolumeInfo } from "./types.js";

function mapBranch(row: Record<string, unknown>): ZfsBranchInfo {
  return {
    name: String(row.name),
    origin: (row.origin as string | null) ?? null,
    createdAt: row.created_at == null ? null : Number(row.created_at),
  };
}

function mapAttachment(row: Record<string, unknown>): ZfsAttachmentInfo {
  const mounts = (row.mounts as Record<string, unknown>[] | undefined) ?? [];
  return {
    machine: String(row.machine),
    machineName: (row.machine_name as string | null) ?? null,
    running: Boolean(row.running),
    mounts: mounts.map((m) => ({ branch: String(m.branch), mountpoint: String(m.mountpoint) })),
  };
}

function mapVolume(row: Record<string, unknown>): ZfsVolumeInfo {
  const branches = (row.branches as Record<string, unknown>[] | undefined) ?? [];
  const attached = (row.attached_to as Record<string, unknown>[] | undefined) ?? [];
  return {
    name: String(row.name),
    path: String(row.path),
    sizeBytes: Number(row.size_bytes ?? 0),
    size: (row.size as string | null) ?? null,
    branches: branches.map(mapBranch),
    attachedTo: attached.map(mapAttachment),
    createdAt: row.created_at == null ? null : Number(row.created_at),
  };
}

async function cli(args: string[], label: string): Promise<string> {
  const res = await runCli(args);
  if (res.exitCode !== 0) {
    throw new CommandFailedError(new CommandResult(res.stdout, res.stderr, res.exitCode, label));
  }
  return res.stdout;
}

/** List ZFS volumes, with their branches and holder. */
export async function listZfsVolumes(): Promise<ZfsVolumeInfo[]> {
  const out = await cli(["zfs", "ls", "--json"], "bsdkrun zfs ls");
  const rows = JSON.parse(out || "[]") as Record<string, unknown>[];
  return rows.map(mapVolume);
}

/** The branches of one volume. */
export async function zfsBranches(volume: string): Promise<ZfsBranchInfo[]> {
  const out = await cli(["zfs", "ls", volume, "--json"], "bsdkrun zfs ls");
  const rows = JSON.parse(out || "[]") as Record<string, unknown>[];
  return rows.map(mapBranch);
}

/**
 * Create a volume: a sparse disk image that becomes a zpool with a `main`
 * branch the first time a guest imports it. `size` is e.g. `"10G"` (default).
 */
export async function createZfsVolume(name: string, opts: { size?: string } = {}): Promise<string> {
  const args = ["zfs", "create", name];
  if (opts.size) args.push("--size", opts.size);
  return (await cli(args, "bsdkrun zfs create")).trim();
}

/**
 * Fork a branch copy-on-write (`zfs snapshot` + `zfs clone`) from `from`
 * (`main` by default). Runs inside the machine holding the volume, or in a
 * short-lived FreeBSD helper, so it can take as long as a BSD boot. Returns
 * the new `VOL@BRANCH`.
 */
export async function branchZfsVolume(
  volume: string,
  name: string,
  opts: { from?: string } = {},
): Promise<string> {
  const args = ["zfs", "branch", volume, name];
  if (opts.from) args.push("--from", opts.from);
  return (await cli(args, "bsdkrun zfs branch")).trim();
}

/**
 * Attach `VOL` or `VOL@BRANCH` to a machine, mounting the branch at `mount`
 * (`/mnt/VOL`, or `/mnt/VOL-BRANCH`, by default). A pool is imported by one
 * machine at a time: a stopped machine picks it up on start, a running holder
 * mounts the branch live, and a running machine that does not hold the pool
 * is refused (no hot-plug). Returns the CLI's one-line report.
 */
export async function attachZfsVolume(
  target: string,
  machine: string,
  opts: { mount?: string } = {},
): Promise<string> {
  const args = ["zfs", "attach", target, machine];
  if (opts.mount) args.push("--mount", opts.mount);
  return (await cli(args, "bsdkrun zfs attach")).trim();
}

/** Detach one branch (`VOL@BRANCH`) or the whole volume (`VOL`) from a machine. */
export async function detachZfsVolume(target: string, machine: string): Promise<string> {
  return (await cli(["zfs", "detach", target, machine], "bsdkrun zfs detach")).trim();
}

/** Destroy branches (`VOL@BRANCH`) or whole volumes (`VOL`) and their images. */
export async function destroyZfsVolume(
  targets: string | string[],
  opts: { force?: boolean } = {},
): Promise<void> {
  const list = Array.isArray(targets) ? targets : [targets];
  const args = ["zfs", "destroy"];
  if (opts.force) args.push("--force");
  args.push(...list);
  await cli(args, "bsdkrun zfs destroy");
}

/** Namespace grouping ZFS volume operations. */
export const zfs = {
  list: listZfsVolumes,
  branches: zfsBranches,
  create: createZfsVolume,
  branch: branchZfsVolume,
  attach: attachZfsVolume,
  detach: detachZfsVolume,
  destroy: destroyZfsVolume,
};
