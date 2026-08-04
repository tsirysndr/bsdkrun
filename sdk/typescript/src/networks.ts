import { CommandFailedError } from "./errors.js";
import { runCli } from "./process.js";
import { CommandResult } from "./shell.js";
import { Sandbox } from "./sandbox.js";
import type { NetworkInfo, SandboxInfo } from "./types.js";

function mapNetwork(row: Record<string, unknown>): NetworkInfo {
  return {
    name: String(row.name),
    subnet: String(row.subnet),
    gateway: String(row.gateway),
    members: Number(row.members ?? 0),
    running: Number(row.running ?? 0),
    up: Boolean(row.up),
    createdAt: row.created_at == null ? null : Number(row.created_at),
  };
}

async function run(args: string[], label: string): Promise<CommandResult> {
  const res = await runCli(args);
  const result = new CommandResult(res.stdout, res.stderr, res.exitCode, label);
  if (res.exitCode !== 0) throw new CommandFailedError(result);
  return result;
}

/** List global networks and their member counts. */
export async function listNetworks(): Promise<NetworkInfo[]> {
  const res = await run(["network", "ls", "--json"], "bsdkrun network ls");
  const rows = JSON.parse(res.stdout || "[]") as Record<string, unknown>[];
  return rows.map(mapNetwork);
}

/** Create a global network (starts its shared switch). */
export async function createNetwork(name: string): Promise<void> {
  await run(["network", "create", name], "bsdkrun network create");
}

/** Remove one or more networks. `force` allows removal with running members. */
export async function removeNetwork(
  names: string | string[],
  opts: { force?: boolean } = {},
): Promise<void> {
  const list = Array.isArray(names) ? names : [names];
  const args = ["network", "rm"];
  if (opts.force) args.push("--force");
  args.push(...list);
  await run(args, "bsdkrun network rm");
}

/**
 * Join or switch a machine to a network (by id or name). Applies on the
 * machine's next {@link Sandbox.start}.
 */
export async function connectNetwork(
  machine: string,
  network: string,
): Promise<void> {
  await run(
    ["network", "connect", machine, network],
    "bsdkrun network connect",
  );
}

/** Detach a machine from its network. Applies on its next start. */
export async function disconnectNetwork(machine: string): Promise<void> {
  await run(["network", "disconnect", machine], "bsdkrun network disconnect");
}

/**
 * Refresh members' `/etc/hosts` with the current membership so peers resolve by
 * name (notably NetBSD). Joins auto-sync; use this to fix an existing network
 * without restarting members.
 */
export async function syncNetwork(network: string): Promise<void> {
  await run(["network", "sync", network], "bsdkrun network sync");
}

/** The machines currently attached to `network` (running or stopped). */
export async function networkMembers(network: string): Promise<SandboxInfo[]> {
  const all = await Sandbox.list({ all: true });
  return all.filter((m) => m.network === network);
}

/** Namespace grouping global-network operations. */
export const networks = {
  list: listNetworks,
  create: createNetwork,
  remove: removeNetwork,
  connect: connectNetwork,
  disconnect: disconnectNetwork,
  sync: syncNetwork,
  members: networkMembers,
};
