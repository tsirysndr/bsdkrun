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
  Flavor,
  Image,
  Machine,
  Network,
  NewFlavor,
  ProbeResult,
  RunSpec,
  Settings,
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
  cpus mem volume stateDir createdAt finishedAt network netIp
  ports { bind host guest }
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

  listImages: async (): Promise<Image[]> => {
    const d = await gql<{ images: any[] }>(
      `{ images { id reference digest size rootfs createdAt } }`,
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
export const onMenuAction = (_cb: (action: string) => void): Promise<UnlistenFn> =>
  Promise.resolve(() => {});
