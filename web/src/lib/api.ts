// The GraphQL implementation of the app's backend surface.
//
// This is the only file that knows the app talks to a daemon over GraphQL.
// It deliberately keeps the same shape the desktop app's Tauri `api` had —
// same function names, same arguments, same event callbacks — so every view,
// dialog and hook is shared verbatim between the two and the UI is identical.
//
// Streaming (terminals, logs, launch progress) arrives on GraphQL
// subscriptions and is re-published through a tiny event bus, because the
// desktop's components were written against `listen("term://data", …)`.

import { gql, subscribe } from "./graphql";
import { getConnection, setConnection, type Connection } from "./connection";
import type {
  AiAgent,
  AiSession,
  Flavor,
  Image,
  Machine,
  Network,
  NewFlavor,
  ProbeResult,
  RunSpec,
  DockerContainer,
  DockerStatus,
  Settings,
  Snapshot,
  SystemStats,
  VersionEntry,
  Volume,
} from "./types";

// ---- a minimal event bus, mirroring Tauri's listen/emit --------------------

export type UnlistenFn = () => void;

type Handler = (payload: any) => void;
const listeners = new Map<string, Set<Handler>>();

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((h) => h(payload));
}

function listen<T>(event: string, cb: (p: T) => void): Promise<UnlistenFn> {
  let set = listeners.get(event);
  if (!set) listeners.set(event, (set = new Set()));
  set.add(cb as Handler);
  return Promise.resolve(() => {
    set!.delete(cb as Handler);
  });
}

// ---- fragments -------------------------------------------------------------

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

const DOCKER_CONTAINER_FIELDS = `
  id name image command state status ports created
`;

const SNAPSHOT_FIELDS = `
  id name machineId machineName kind image path parent description
  cpus mem size createdAt ports { bind host guest }
`;

/**
 * The schema is camelCase (GraphQL convention) while the components were
 * written against the CLI's snake_case JSON. Rather than touch every view,
 * the few differing fields are renamed back here.
 */
function toMachine(m: any): Machine {
  return {
    id: m.id,
    name: m.name,
    image: m.image,
    kind: m.kind,
    command: m.command,
    status: m.status,
    running: m.running,
    exit_code: m.exitCode ?? null,
    pid: m.pid ?? null,
    detached: m.detached,
    cpus: m.cpus ?? null,
    mem: m.mem ?? null,
    volume: m.volume ?? null,
    state_dir: m.stateDir ?? null,
    created_at: m.createdAt ?? null,
    finished_at: m.finishedAt ?? null,
    network: m.network ?? null,
    net_ip: m.netIp ?? null,
    origin: m.origin ?? null,
    ports: (m.ports ?? []).map((p: any) => ({
      bind: p.bind,
      host: p.host,
      guest: p.guest,
    })),
  };
}

const toImage = (i: any): Image => ({
  id: i.id,
  reference: i.reference,
  digest: i.digest ?? null,
  size: i.size,
  rootfs: i.rootfs ?? null,
  created_at: i.createdAt ?? null,
  used_by: i.usedBy ?? [],
});

const toVolume = (v: any): Volume => ({
  name: v.name,
  guest: v.guest ?? null,
  base: v.base ?? null,
  path: v.path ?? null,
  size: v.size ?? null,
  created_at: v.createdAt ?? null,
  tracked: v.tracked,
});

const toSnapshot = (s: any): Snapshot => ({
  id: s.id,
  name: s.name,
  machine_id: s.machineId,
  machine_name: s.machineName ?? "",
  kind: s.kind,
  image: s.image,
  path: s.path,
  parent: s.parent ?? null,
  description: s.description ?? "",
  cpus: s.cpus,
  mem: s.mem,
  ports: (s.ports ?? []).map((p: any) => ({
    bind: p.bind,
    host: p.host,
    guest: p.guest,
  })),
  size: s.size ?? null,
  created_at: s.createdAt,
});

/**
 * The daemon never creates a `docker context` (that belongs to whoever runs
 * the client, not to the host with the VM), so those two flags are false here
 * rather than absent — the UI reads them to decide what to tell the user.
 */
const toDockerStatus = (s: any): DockerStatus => ({
  running: s.running,
  machine_id: s.machineId ?? null,
  machine_running: s.machineRunning,
  socket: s.socket,
  socket_ready: s.socketReady,
  api_port: s.apiPort ?? null,
  version: s.version ?? null,
  containers: s.containers ?? null,
  images: s.images ?? null,
  mounts: s.mounts ?? [],
  context: false,
  context_active: false,
  system_socket: false,
  disk: s.disk ?? null,
  disk_size: s.diskSize ?? null,
});

const toDockerContainer = (c: any): DockerContainer => ({
  id: c.id,
  name: c.name,
  image: c.image,
  command: c.command ?? "",
  state: c.state,
  status: c.status,
  ports: c.ports ?? [],
  created: c.created ?? 0,
});

const toFlavor = (f: any): Flavor => ({ ...f, created_at: f.createdAt ?? null });
const toNetwork = (n: any): Network => ({ ...n, created_at: n.createdAt ?? null });

/** The CLI accepts `HOST:GUEST` port strings; drop the blanks the form leaves. */
const clean = (xs: string[] | undefined) => (xs ?? []).filter((x) => x && x.trim() !== "");
const orNull = (s: string | null | undefined) => (s && s.trim() !== "" ? s.trim() : null);

// ---- RunSpec -> GraphQL inputs --------------------------------------------

function linuxInput(spec: RunSpec) {
  return {
    image: spec.image ?? "",
    cpus: spec.cpus ?? null,
    mem: spec.mem ?? null,
    net: {
      noNet: spec.no_net,
      ports: clean(spec.ports),
      network: orNull(spec.network),
      name: orNull(spec.name),
    },
    volume: orNull(spec.volume),
    mounts: clean(spec.mounts),
    attachDisk: clean(spec.attach_disks),
    entrypoint: orNull(spec.entrypoint),
    initramfs: spec.initramfs,
    repo: orNull(spec.repo),
    command: spec.command ?? [],
  };
}

function bsdInput(spec: RunSpec) {
  return {
    os: spec.kind === "netbsd" ? "NETBSD" : "FREEBSD",
    version: orNull(spec.version),
    cpus: spec.cpus ?? null,
    mem: spec.mem ?? null,
    net: {
      noNet: spec.no_net,
      ports: clean(spec.ports),
      network: orNull(spec.network),
      name: orNull(spec.name),
    },
    volume: orNull(spec.volume),
    attachDisk: clean(spec.attach_disks),
    diskSize: orNull(spec.disk_size),
    repo: orNull(spec.repo),
    command: spec.command ?? [],
  };
}

/**
 * A unikernel has no disk and no agent, so no volume/repo/command. `mounts`
 * is the exception: virtio-fs shares, which need neither.
 */
function unikraftInput(spec: RunSpec) {
  return {
    path: orNull(spec.path),
    cpus: spec.cpus ?? null,
    mem: spec.mem ?? null,
    net: {
      noNet: spec.no_net,
      ports: clean(spec.ports),
      network: orNull(spec.network),
      name: orNull(spec.name),
    },
    cmdline: orNull(spec.cmdline),
    mounts: clean(spec.mounts),
  };
}

/**
 * OSv: no agent (no repo/command), but it does have a root filesystem, so the
 * disk options apply — `disk` in particular is how an x86_64 guest is given
 * one, its loader ELF being kernel only.
 */
function osvInput(spec: RunSpec) {
  return {
    image: spec.image ?? "",
    cpus: spec.cpus ?? null,
    mem: spec.mem ?? null,
    net: {
      noNet: spec.no_net,
      ports: clean(spec.ports),
      network: orNull(spec.network),
      name: orNull(spec.name),
    },
    cmdline: orNull(spec.cmdline),
    disk: orNull(spec.disk),
    gic: orNull(spec.gic),
    persist: spec.persist ?? false,
    volume: orNull(spec.volume),
  };
}

/**
 * Solo5 (MirageOS): runs under the solo5-hvt tender rather than libkrun, but
 * it is one more agent-less unikernel over the wire — no volume/repo/command.
 * The unikernel declares its own devices, so only the block backing files and
 * the guest's own arguments are carried. Always single-vCPU; the daemon warns
 * about and ignores anything above 1.
 */
function solo5Input(spec: RunSpec) {
  return {
    path: orNull(spec.path),
    cpus: spec.cpus ?? null,
    mem: spec.mem ?? null,
    net: {
      noNet: spec.no_net,
      ports: clean(spec.ports),
      network: orNull(spec.network),
      name: orNull(spec.name),
    },
    block: clean(spec.blocks),
    args: spec.args ?? [],
  };
}

/** Nanos: no agent (no volume/repo/command); image + cmdline only for now. */
function nanosInput(spec: RunSpec) {
  return {
    image: spec.image ?? "",
    cpus: spec.cpus ?? null,
    mem: spec.mem ?? null,
    net: {
      noNet: spec.no_net,
      ports: clean(spec.ports),
      network: orNull(spec.network),
      name: orNull(spec.name),
    },
    cmdline: orNull(spec.cmdline),
  };
}

const termUnsubs = new Map<string, UnlistenFn>();
const logUnsubs = new Map<string, UnlistenFn>();
const launchUnsubs = new Map<string, UnlistenFn>();

/** Shared by launchFlavor / launchMachine / buildFlavor. */
function streamLaunch(
  launchId: string,
  field: string,
  document: string,
  variables: Record<string, unknown>,
) {
  const unsub = subscribe(document, variables, {
    onNext: (data) => {
      const e = data?.[field];
      if (!e) return;
      if (e.line) emit("flavor://log", { launch_id: launchId, line: e.line });
      if (e.machineId || e.error) {
        emit("flavor://done", {
          launch_id: launchId,
          id: e.machineId ?? null,
          error: e.error ?? null,
        });
      }
    },
    onError: (err) =>
      emit("flavor://done", { launch_id: launchId, id: null, error: err.message }),
    onComplete: () => {
      launchUnsubs.delete(launchId);
    },
  });
  launchUnsubs.set(launchId, unsub);
}

export const api = {
  // ---- connection settings (replaces the desktop's binary/cache paths) -----

  getSettings: async (): Promise<Settings> => {
    const c = getConnection();
    return { url: c?.url ?? "", token: c?.token ?? "" };
  },
  setSettings: async (url: string, token: string): Promise<Settings> => {
    setConnection({ url, token } as Connection);
    const c = getConnection()!;
    return { url: c.url, token: c.token };
  },

  /** Is the configured daemon reachable, and what is it driving? */
  probe: async (): Promise<ProbeResult> => {
    try {
      const d = await gql<{ info: any }>(
        `{ info { daemonVersion cliVersion cliPath os arch } }`,
      );
      return {
        ok: true,
        message: `${d.info.cliVersion} on ${d.info.os}/${d.info.arch} (daemon ${d.info.daemonVersion})`,
        binary: d.info.cliPath,
      };
    } catch (e) {
      return { ok: false, message: (e as Error).message, binary: null };
    }
  },

  // A tray only exists in the desktop shell.
  setTrayStatus: async (_ok: boolean, _detail: string) => {},

  // ---- lists ---------------------------------------------------------------

  listMachines: async (all: boolean): Promise<Machine[]> => {
    const d = await gql<{ machines: any[] }>(
      `query($all:Boolean!){ machines(all:$all){ ${MACHINE_FIELDS} } }`,
      { all },
    );
    return d.machines.map(toMachine);
  },

  /** Only a dangling image can go; the engine refuses one still in use. */
  removeImage: async (id: string) => {
    const d = await gql<{ removeImages: { exitCode: number; stderr: string } }>(
      `mutation($i:[String!]!){ removeImages(ids:$i){ exitCode stderr } }`,
      { i: [id] },
    );
    if (d.removeImages.exitCode !== 0)
      throw new Error(d.removeImages.stderr.trim());
  },

  listImages: async (): Promise<Image[]> => {
    const d = await gql<{ images: any[] }>(
      `{ images { id reference digest size rootfs createdAt usedBy } }`,
    );
    return d.images.map(toImage);
  },

  listVolumes: async (): Promise<Volume[]> => {
    const d = await gql<{ volumes: any[] }>(
      `{ volumes { name guest base path size createdAt tracked } }`,
    );
    return d.volumes.map(toVolume);
  },

  listVersions: async (os: string): Promise<VersionEntry[]> => {
    const d = await gql<{ versions: VersionEntry[] }>(
      `query($os:BsdOs!){ versions(os:$os){ version latest } }`,
      { os: os.toUpperCase() },
    );
    return d.versions;
  },

  listFlavors: async (): Promise<Flavor[]> => {
    const d = await gql<{ flavors: any[] }>(
      `{ flavors { name source kind base category method description ports nix createdAt } }`,
    );
    return d.flavors.map(toFlavor);
  },

  listNetworks: async (): Promise<Network[]> => {
    const d = await gql<{ networks: any[] }>(
      `{ networks { name subnet gateway members running up createdAt } }`,
    );
    return d.networks.map(toNetwork);
  },

  systemStats: async (): Promise<SystemStats> => {
    const d = await gql<{ systemStats: any }>(
      `{ systemStats { cpu memUsed memTotal vmDisk vmCount } }`,
    );
    return {
      cpu: d.systemStats.cpu,
      mem_used: d.systemStats.memUsed,
      mem_total: d.systemStats.memTotal,
      vm_disk: d.systemStats.vmDisk,
      vm_count: d.systemStats.vmCount,
    };
  },

  // ---- networks ------------------------------------------------------------

  createNetwork: async (name: string) => {
    await gql(`mutation($n:String!){ createNetwork(name:$n){ exitCode stderr } }`, { n: name });
  },
  removeNetwork: async (name: string, force: boolean) => {
    await gql(
      `mutation($n:[String!]!,$f:Boolean!){ removeNetworks(names:$n, force:$f){ exitCode stderr } }`,
      { n: [name], f: force },
    );
  },
  syncNetwork: async (name: string) => {
    await gql(`mutation($n:String!){ syncNetwork(network:$n){ exitCode stderr } }`, { n: name });
  },

  // ---- flavors -------------------------------------------------------------

  runFlavor: async (name: string, ports: string[], volume: string | null) => {
    const d = await gql<{ runFlavor: string }>(
      `mutation($i:RunFlavorInput!){ runFlavor(input:$i) }`,
      { i: { name, ports: clean(ports), volume: orNull(volume) } },
    );
    return d.runFlavor;
  },

  launchFlavor: async (
    launchId: string,
    name: string,
    ports: string[],
    volume: string | null,
    repo: string | null,
  ) => {
    streamLaunch(
      launchId,
      "launchFlavor",
      `subscription($i:RunFlavorInput!){ launchFlavor(input:$i){ line machineId error } }`,
      { i: { name, ports: clean(ports), volume: orNull(volume), repo: orNull(repo) } },
    );
  },

  buildFlavor: async (launchId: string, name: string) => {
    streamLaunch(
      launchId,
      "buildFlavor",
      `subscription($n:String!){ buildFlavor(name:$n){ line machineId error } }`,
      { n: name },
    );
  },

  createFlavor: async (spec: NewFlavor) => {
    await gql(`mutation($i:AddFlavorInput!){ addFlavor(input:$i){ exitCode stderr } }`, {
      i: {
        name: spec.name,
        base: spec.base,
        category: spec.category,
        description: spec.description,
        ports: clean(spec.ports),
        env: clean(spec.env),
        nix: clean(spec.nix),
        provision: clean(spec.provision),
      },
    });
  },

  commitMachine: async (id: string, name: string, description: string) => {
    const d = await gql<{ commitMachine: { exitCode: number; stderr: string } }>(
      `mutation($i:String!,$n:String!,$d:String!){ commitMachine(id:$i,name:$n,description:$d){ exitCode stderr } }`,
      { i: id, n: name, d: description },
    );
    if (d.commitMachine.exitCode !== 0) throw new Error(d.commitMachine.stderr.trim());
    return name;
  },

  // ---- ai agents --------------------------------------------------------------

  aiAgents: async (): Promise<AiAgent[]> => {
    const d = await gql<{ aiAgents: AiAgent[] }>(
      `{ aiAgents { ${AI_AGENT_FIELDS} } }`,
    );
    return d.aiAgents;
  },

  aiSessions: async (): Promise<AiSession[]> => {
    const d = await gql<{ aiSessions: any[] }>(
      `{ aiSessions { ${AI_SESSION_FIELDS} } }`,
    );
    return d.aiSessions.map((s) => ({ ...s, created_at: s.createdAt ?? "" }));
  },

  aiStart: async (
    agent: string,
    workspace: string | null,
    newSession: boolean,
    name?: string,
    repo?: string,
  ): Promise<string> => {
    const d = await gql<{ aiStart: string }>(
      `mutation($i:AiStartInput!){ aiStart(input:$i) }`,
      {
        i: {
          agent,
          workspace: orNull(workspace ?? undefined),
          new: newSession,
          name: orNull(name),
          repo: orNull(repo),
        },
      },
    );
    return d.aiStart;
  },

  /**
   * The web app has no streaming boot channel of its own, so a first launch
   * simply takes as long as it takes; the panel shows an installing state
   * rather than a progress log.
   */
  /**
   * Start a sandbox, streaming the image pull, the flavor build and the boot.
   *
   * Not `aiStart`: that mutation returns only when the sandbox is up, and the
   * caller waits on a `flavor://done` this never emitted — so the panel hung
   * rather than opening a terminal. It is a subscription for the same reason
   * `launchFlavor` is.
   */
  launchAgent: async (
    launchId: string,
    agent: string,
    workspace: string | null,
    newSession: boolean,
    name?: string,
    repo?: string,
  ): Promise<void> => {
    streamLaunch(
      launchId,
      "launchAgent",
      `subscription($i:AiStartInput!){ launchAgent(input:$i){ line machineId error } }`,
      {
        i: {
          agent,
          workspace: orNull(workspace),
          new: newSession,
          name: orNull(name),
          repo: orNull(repo),
        },
      },
    );
  },

  /** The CI workflows in a repository, and whether each matches an event. */
  ciWorkflows: async (dir: string, event: string) => {
    const d = await gql<{ ciWorkflows: string }>(
      `query($d:String!,$e:String!){ ciWorkflows(dir:$d, event:$e) }`,
      { d: dir, e: event },
    );
    // The daemon passes the tool's own JSON through rather than re-declaring
    // its shape in the schema.
    return JSON.parse(d.ciWorkflows || "[]") as import("./types").CiWorkflowInfo[];
  },
  /** Run CI workflows, streaming spindle LogLine JSON via flavor:// events. */
  ciRun: async (
    launchId: string,
    dir: string,
    names: string[],
    event: string,
  ): Promise<void> => {
    streamLaunch(
      launchId,
      "runCi",
      `subscription($d:String!,$n:[String!]!,$e:String!){ runCi(dir:$d, names:$n, event:$e){ line machineId error } }`,
      { d: dir, n: names, e: event },
    );
  },
  /** Clone (or update) a repository on the engine's host for CI. */
  ciClone: async (url: string) => {
    const d = await gql<{ ciClone: string }>(
      `mutation($u:String!){ ciClone(url:$u) }`,
      { u: url },
    );
    return d.ciClone;
  },
  /** Resume one stopped sandbox, streaming its boot. */
  resumeAgent: async (launchId: string, machine: string): Promise<void> => {
    streamLaunch(
      launchId,
      "resumeAgent",
      `subscription($m:String!){ resumeAgent(machine:$m){ line machineId error } }`,
      { m: machine },
    );
  },

  /** The argv that starts the agent's TUI in a sandbox. */
  aiShellCommand: async (agent: string, machineId: string): Promise<string[]> => {
    const d = await gql<{ aiShellCommand: string[] }>(
      `query($a:String!,$m:String!){ aiShellCommand(agent:$a, machineId:$m) }`,
      { a: agent, m: machineId },
    );
    return d.aiShellCommand;
  },

  aiStop: async (agent: string) => {
    const d = await gql<{ aiStop: { exitCode: number; stderr: string } }>(
      `mutation($a:String!){ aiStop(agent:$a){ exitCode stderr } }`,
      { a: agent },
    );
    if (d.aiStop.exitCode !== 0) throw new Error(d.aiStop.stderr.trim());
  },

  aiRemove: async (agent: string, keepHome: boolean) => {
    const d = await gql<{ aiRemove: { exitCode: number; stderr: string } }>(
      `mutation($a:String!,$k:Boolean!){ aiRemove(agent:$a, keepHome:$k){ exitCode stderr } }`,
      { a: agent, k: keepHome },
    );
    if (d.aiRemove.exitCode !== 0) throw new Error(d.aiRemove.stderr.trim());
  },

  // ---- docker ---------------------------------------------------------------

  dockerStatus: async (): Promise<DockerStatus> => {
    const d = await gql<{ dockerStatus: any }>(
      `{ dockerStatus { ${DOCKER_STATUS_FIELDS} } }`,
    );
    return toDockerStatus(d.dockerStatus);
  },

  dockerContainers: async (all: boolean): Promise<DockerContainer[]> => {
    const d = await gql<{ dockerContainers: any[] }>(
      `query($all:Boolean!){ dockerContainers(all:$all){ ${DOCKER_CONTAINER_FIELDS} } }`,
      { all },
    );
    return d.dockerContainers.map(toDockerContainer);
  },

  dockerStart: async (
    cpus?: number,
    mem?: number,
    diskSize?: string,
  ): Promise<DockerStatus> => {
    const d = await gql<{ dockerStart: any }>(
      `mutation($i:DockerStartInput!){ dockerStart(input:$i){ ${DOCKER_STATUS_FIELDS} } }`,
      { i: { cpus: cpus ?? null, mem: mem ?? null, diskSize: orNull(diskSize) } },
    );
    return toDockerStatus(d.dockerStart);
  },

  dockerStop: async () => {
    const d = await gql<{ dockerStop: { exitCode: number; stderr: string } }>(
      `mutation{ dockerStop{ exitCode stderr } }`,
    );
    if (d.dockerStop.exitCode !== 0) throw new Error(d.dockerStop.stderr.trim());
  },

  dockerContainer: async (action: string, id: string): Promise<string> => {
    const d = await gql<{
      dockerContainer: { exitCode: number; stdout: string; stderr: string };
    }>(
      `mutation($a:String!,$i:[String!]!){ dockerContainer(action:$a, ids:$i){ exitCode stdout stderr } }`,
      { a: action, i: [id] },
    );
    if (d.dockerContainer.exitCode !== 0)
      throw new Error(d.dockerContainer.stderr.trim());
    return d.dockerContainer.stdout.trim();
  },

  dockerLogs: async (id: string, tail: number): Promise<string> => {
    const d = await gql<{ dockerContainerLogs: string }>(
      `query($i:String!,$t:Int!){ dockerContainerLogs(id:$i, tail:$t) }`,
      { i: id, t: tail },
    );
    return d.dockerContainerLogs;
  },

  // ---- snapshots -----------------------------------------------------------

  listSnapshots: async (machine?: string | null): Promise<Snapshot[]> => {
    const d = await gql<{ snapshots: any[] }>(
      `query($m:String){ snapshots(machine:$m){ ${SNAPSHOT_FIELDS} } }`,
      { m: machine ?? null },
    );
    return d.snapshots.map(toSnapshot);
  },

  /** Returns the snapshot's name — generated when none was given. */
  snapshotMachine: async (
    id: string,
    name: string | null,
    description: string,
  ): Promise<string> => {
    const d = await gql<{ snapshotMachine: { name: string } }>(
      `mutation($i:String!,$n:String,$d:String!){ snapshotMachine(id:$i,name:$n,description:$d){ name } }`,
      { i: id, n: orNull(name), d: description },
    );
    return d.snapshotMachine.name;
  },

  removeSnapshot: async (name: string) => {
    const d = await gql<{ removeSnapshots: { exitCode: number; stderr: string } }>(
      `mutation($n:[String!]!){ removeSnapshots(names:$n){ exitCode stderr } }`,
      { n: [name] },
    );
    if (d.removeSnapshots.exitCode !== 0)
      throw new Error(d.removeSnapshots.stderr.trim());
  },

  /** Leaves the machine stopped; start it to run the restored state. */
  restoreMachine: async (id: string, snapshot: string, backup: boolean) => {
    const d = await gql<{
      restoreMachine: { exitCode: number; stdout: string; stderr: string };
    }>(
      `mutation($i:String!,$s:String!,$b:Boolean!){ restoreMachine(id:$i,snapshot:$s,force:true,backup:$b){ exitCode stdout stderr } }`,
      { i: id, s: snapshot, b: backup },
    );
    if (d.restoreMachine.exitCode !== 0)
      throw new Error(d.restoreMachine.stderr.trim());
    return d.restoreMachine.stdout.trim();
  },

  rollbackMachine: async (id: string, backup: boolean) => {
    const d = await gql<{
      rollbackMachine: { exitCode: number; stdout: string; stderr: string };
    }>(
      `mutation($i:String!,$b:Boolean!){ rollbackMachine(id:$i,force:true,backup:$b){ exitCode stdout stderr } }`,
      { i: id, b: backup },
    );
    if (d.rollbackMachine.exitCode !== 0)
      throw new Error(d.rollbackMachine.stderr.trim());
    return d.rollbackMachine.stdout.trim();
  },

  /** Boots a new machine from a snapshot; returns its id. */
  branchSnapshot: async (
    snapshot: string,
    name: string | null,
    ports: string[],
  ): Promise<string> => {
    const d = await gql<{ branchSnapshot: string }>(
      `mutation($i:BranchInput!){ branchSnapshot(input:$i) }`,
      {
        i: {
          snapshot,
          name: orNull(name),
          ports: clean(ports),
          // An empty `ports` inherits the snapshot's; only an explicit request
          // for none should drop them.
          noPorts: false,
        },
      },
    );
    return d.branchSnapshot;
  },

  removeFlavor: async (name: string, force: boolean) => {
    await gql(
      `mutation($n:[String!]!,$f:Boolean!){ removeFlavors(names:$n, force:$f){ exitCode stderr } }`,
      { n: [name], f: force },
    );
  },

  // ---- machines ------------------------------------------------------------

  runMachine: async (spec: RunSpec): Promise<string> => {
    if (spec.kind === "linux") {
      const d = await gql<{ runLinux: string }>(
        `mutation($i:RunLinuxInput!){ runLinux(input:$i) }`,
        { i: linuxInput(spec) },
      );
      return d.runLinux;
    }
    if (spec.kind === "osv") {
      const d = await gql<{ runOsv: string }>(
        `mutation($i:RunOsvInput!){ runOsv(input:$i) }`,
        { i: osvInput(spec) },
      );
      return d.runOsv;
    }
    if (spec.kind === "nanos") {
      const d = await gql<{ runNanos: string }>(
        `mutation($i:RunNanosInput!){ runNanos(input:$i) }`,
        { i: nanosInput(spec) },
      );
      return d.runNanos;
    }
    if (spec.kind === "unikraft") {
      const d = await gql<{ runUnikraft: string }>(
        `mutation($i:RunUnikraftInput!){ runUnikraft(input:$i) }`,
        { i: unikraftInput(spec) },
      );
      return d.runUnikraft;
    }
    if (spec.kind === "solo5") {
      const d = await gql<{ runSolo5: string }>(
        `mutation($i:RunSolo5Input!){ runSolo5(input:$i) }`,
        { i: solo5Input(spec) },
      );
      return d.runSolo5;
    }
    const d = await gql<{ runBsd: string }>(`mutation($i:RunBsdInput!){ runBsd(input:$i) }`, {
      i: bsdInput(spec),
    });
    return d.runBsd;
  },

  launchMachine: async (launchId: string, spec: RunSpec) => {
    if (spec.kind === "linux") {
      streamLaunch(
        launchId,
        "launchLinux",
        `subscription($i:RunLinuxInput!){ launchLinux(input:$i){ line machineId error } }`,
        { i: linuxInput(spec) },
      );
    } else if (spec.kind === "nanos") {
      streamLaunch(
        launchId,
        "launchNanos",
        `subscription($i:RunNanosInput!){ launchNanos(input:$i){ line machineId error } }`,
        { i: nanosInput(spec) },
      );
    } else if (spec.kind === "unikraft") {
      streamLaunch(
        launchId,
        "launchUnikraft",
        `subscription($i:RunUnikraftInput!){ launchUnikraft(input:$i){ line machineId error } }`,
        { i: unikraftInput(spec) },
      );
    } else if (spec.kind === "solo5") {
      streamLaunch(
        launchId,
        "launchSolo5",
        `subscription($i:RunSolo5Input!){ launchSolo5(input:$i){ line machineId error } }`,
        { i: solo5Input(spec) },
      );
    } else if (spec.kind === "osv") {
      streamLaunch(
        launchId,
        "launchOsv",
        `subscription($i:RunOsvInput!){ launchOsv(input:$i){ line machineId error } }`,
        { i: osvInput(spec) },
      );
    } else {
      streamLaunch(
        launchId,
        "launchBsd",
        `subscription($i:RunBsdInput!){ launchBsd(input:$i){ line machineId error } }`,
        { i: bsdInput(spec) },
      );
    }
  },

  updateMachine: async (id: string, cpus: number, mem: number) => {
    await gql(
      `mutation($i:String!,$c:Int!,$m:Int!){ updateMachine(id:$i,cpus:$c,mem:$m){ exitCode stderr } }`,
      { i: id, c: cpus, m: mem },
    );
  },

  updateMachineNetwork: async (id: string, network: string | null) => {
    if (network && network.trim() !== "") {
      await gql(
        `mutation($m:String!,$n:String!){ connectNetwork(machine:$m,network:$n){ exitCode stderr } }`,
        { m: id, n: network },
      );
    } else {
      await gql(`mutation($m:String!){ disconnectNetwork(machine:$m){ exitCode stderr } }`, {
        m: id,
      });
    }
  },

  stopMachine: async (id: string) => {
    await gql(`mutation($i:String!){ stopMachine(id:$i){ exitCode stderr } }`, { i: id });
  },

  restartMachine: async (id: string) => {
    await gql(`mutation($i:String!){ startMachine(id:$i){ exitCode stderr } }`, { i: id });
    return id;
  },

  removeMachine: async (id: string, force: boolean) => {
    await gql(
      `mutation($i:[String!]!,$f:Boolean!){ removeMachines(ids:$i, force:$f){ exitCode stderr } }`,
      { i: [id], f: force },
    );
  },

  removeVolume: async (name: string, force: boolean) => {
    await gql(
      `mutation($n:[String!]!,$f:Boolean!){ removeVolumes(names:$n, force:$f){ exitCode stderr } }`,
      { n: [name], f: force },
    );
  },

  // ---- guest tools ---------------------------------------------------------

  machineLogs: async (id: string, boot: boolean) => {
    const d = await gql<{ machineLogs: string }>(
      `query($i:String!,$b:Boolean!){ machineLogs(id:$i, boot:$b) }`,
      { i: id, b: boot },
    );
    return d.machineLogs;
  },

  sshAction: (id: string, args: string[]) => guestTool(id, "SSH", args),
  tailscaleAction: (id: string, args: string[]) => guestTool(id, "TAILSCALE", args),

  updateAgent: async (id: string) => {
    const d = await gql<{ updateAgent: { exitCode: number; stdout: string; stderr: string } }>(
      `mutation($i:String!){ updateAgent(id:$i){ exitCode stdout stderr } }`,
      { i: id },
    );
    return combine(d.updateAgent);
  },

  // ---- log streaming -------------------------------------------------------

  startLogStream: async (id: string) => {
    logUnsubs.get(id)?.();
    const unsub = subscribe(
      `subscription($i:String!){ machineLogs(id:$i, follow:true){ dataBase64 exitCode } }`,
      { i: id },
      {
        onNext: (data) => {
          const p = data?.machineLogs;
          if (!p) return;
          if (p.dataBase64) {
            // The components render lines, so split here rather than in each.
            decodeText(p.dataBase64)
              .split("\n")
              .filter((l) => l !== "")
              .forEach((line) => emit("log://line", { id, line }));
          }
          if (p.exitCode !== null && p.exitCode !== undefined) emit("log://end", { id });
        },
        onError: () => emit("log://end", { id }),
        onComplete: () => emit("log://end", { id }),
      },
    );
    logUnsubs.set(id, unsub);
  },

  stopLogStream: async (id: string) => {
    logUnsubs.get(id)?.();
    logUnsubs.delete(id);
  },

  // ---- terminals -----------------------------------------------------------

  termOpen: async (id: string, command: string[], rows: number, cols: number) => {
    const d = await gql<{ openShell: { id: string } }>(
      `mutation($m:String!,$c:[String!]!,$r:Int!,$k:Int!){
         openShell(machineId:$m, command:$c, rows:$r, cols:$k){ id }
       }`,
      { m: id, c: command ?? [], r: rows, k: cols },
    );
    const session = d.openShell.id;

    // Subscribe only after the session exists. Output produced in between is
    // buffered by the daemon and replayed, so the prompt is never lost.
    const unsub = subscribe(
      `subscription($s:String!){ shellOutput(sessionId:$s){ dataBase64 exitCode } }`,
      { s: session },
      {
        onNext: (data) => {
          const p = data?.shellOutput;
          if (!p) return;
          if (p.dataBase64) {
            emit("term://data", { session, bytes: decodeBytes(p.dataBase64) });
          }
          if (p.exitCode !== null && p.exitCode !== undefined) {
            emit("term://exit", { session, code: p.exitCode });
          }
        },
        onError: () => emit("term://exit", { session, code: null }),
        onComplete: () => emit("term://exit", { session, code: null }),
      },
    );
    termUnsubs.set(session, unsub);
    return session;
  },

  // A host shell would mean running arbitrary commands on the daemon's machine,
  // outside any guest. The daemon deliberately offers no such thing.
  termOpenHost: async (_rows: number, _cols: number): Promise<string> => {
    throw new Error("a host terminal is not available over the web API");
  },

  termWrite: async (session: string, data: string) => {
    await gql(`mutation($s:String!,$d:String!){ sendShellInput(sessionId:$s, dataBase64:$d) }`, {
      s: session,
      d: encodeText(data),
    });
  },

  termResize: async (session: string, rows: number, cols: number) => {
    await gql(
      `mutation($s:String!,$r:Int!,$c:Int!){ resizeShell(sessionId:$s, rows:$r, cols:$c) }`,
      { s: session, r: rows, c: cols },
    );
  },

  termClose: async (session: string) => {
    termUnsubs.get(session)?.();
    termUnsubs.delete(session);
    await gql(`mutation($s:String!){ closeShell(sessionId:$s) }`, { s: session }).catch(() => {
      /* already gone — closing twice is not an error */
    });
  },
};

async function guestTool(id: string, tool: string, args: string[]) {
  const d = await gql<{ guestTool: { exitCode: number; stdout: string; stderr: string } }>(
    `mutation($i:String!,$t:GuestTool!,$a:[String!]!){
       guestTool(id:$i, tool:$t, args:$a){ exitCode stdout stderr }
     }`,
    { i: id, t: tool, a: args },
  );
  return combine(d.guestTool);
}

/** What the desktop showed: both streams, since a non-zero exit is often the answer. */
function combine(r: { exitCode: number; stdout: string; stderr: string }) {
  const parts = [r.stdout.trimEnd(), r.stderr.trimEnd()].filter((s) => s !== "");
  return parts.length ? parts.join("\n") : `(no output — exit ${r.exitCode})`;
}

// ---- base64 <-> bytes ------------------------------------------------------

function decodeBytes(b64: string): number[] {
  const bin = atob(b64);
  const out = new Array<number>(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function decodeText(b64: string): string {
  return new TextDecoder().decode(Uint8Array.from(decodeBytes(b64)));
}

function encodeText(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  bytes.forEach((b) => (bin += String.fromCharCode(b)));
  return btoa(bin);
}

// ---- event payloads (same shapes the components already expect) ------------

export interface TermData {
  session: string;
  bytes: number[];
}
export interface TermExit {
  session: string;
  code: number | null;
}
export interface LogLine {
  id: string;
  line: string;
}
export interface LogEnd {
  id: string;
}
export interface FlavorLog {
  launch_id: string;
  line: string;
}
export interface FlavorDone {
  launch_id: string;
  id: string | null;
  error: string | null;
}

export const onTermData = (cb: (p: TermData) => void) => listen<TermData>("term://data", cb);
export const onTermExit = (cb: (p: TermExit) => void) => listen<TermExit>("term://exit", cb);
export const onLogLine = (cb: (p: LogLine) => void) => listen<LogLine>("log://line", cb);
export const onLogEnd = (cb: (p: LogEnd) => void) => listen<LogEnd>("log://end", cb);
export const onFlavorLog = (cb: (p: FlavorLog) => void) => listen<FlavorLog>("flavor://log", cb);
export const onFlavorDone = (cb: (p: FlavorDone) => void) =>
  listen<FlavorDone>("flavor://done", cb);
/** Native menus are a desktop-shell feature; nothing emits this on the web. */
/**
 * A browser cannot open a directory picker that yields a *path*, and the path
 * has to exist on the daemon's host anyway — which is not this machine when
 * the daemon is remote. The panel asks for it in a modal instead.
 */
export const HAS_NATIVE_FOLDER_PICKER = false;

/** Never called in this build; see `HAS_NATIVE_FOLDER_PICKER`. */
export async function pickWorkspace(): Promise<string | null> {
  return null;
}

export const onMenuAction = (_cb: (action: string) => void): Promise<UnlistenFn> =>
  Promise.resolve(() => {});
