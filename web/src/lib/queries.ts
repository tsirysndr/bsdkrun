import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useCallback } from "react";
import { useSetAtom } from "jotai";
import {
  agentPanelOpenAtom,
  agentSelectedAtom,
  agentSessionAtom,
  launchStateAtom,
} from "../state/atoms";
import { api, onFlavorDone } from "./api";
import type { NewFlavor, RunSpec } from "./types";

export const qk = {
  machines: ["machines"] as const,
  images: ["images"] as const,
  volumes: ["volumes"] as const,
  // Every machine's list lives under the same "snapshots" prefix, so one
  // invalidate after a mutation refreshes the global view and each machine's.
  aiAgents: ["ai", "agents"] as const,
  aiSessions: ["ai", "sessions"] as const,
  dockerStatus: ["docker", "status"] as const,
  dockerContainers: (all: boolean) => ["docker", "containers", all] as const,
  snapshots: (machine?: string | null) =>
    machine ? (["snapshots", machine] as const) : (["snapshots"] as const),
  flavors: ["flavors"] as const,
  networks: ["networks"] as const,
  probe: ["probe"] as const,
  settings: ["settings"] as const,
  versions: (os: string) => ["versions", os] as const,
};

// ---- queries ---------------------------------------------------------------

export function useMachines() {
  return useQuery({
    queryKey: qk.machines,
    queryFn: () => api.listMachines(true),
    refetchInterval: 4000,
    // Keep polling even when the window isn't focused — the desktop's global
    // refetchOnWindowFocus is off, so without this the list goes stale after
    // the window loses/regains focus.
    refetchIntervalInBackground: true,
    placeholderData: (prev) => prev,
  });
}

export function useImages() {
  return useQuery({
    queryKey: qk.images,
    queryFn: () => api.listImages(),
    refetchInterval: 10000,
    refetchIntervalInBackground: true,
    placeholderData: (prev) => prev,
  });
}

export function useVolumes() {
  return useQuery({
    queryKey: qk.volumes,
    queryFn: () => api.listVolumes(),
    refetchInterval: 10000,
    refetchIntervalInBackground: true,
    placeholderData: (prev) => prev,
  });
}

/**
 * Snapshots, all or one machine's. Polled gently: they only change when
 * someone takes, branches or removes one.
 */
export function useSnapshots(machine?: string | null) {
  return useQuery({
    queryKey: qk.snapshots(machine),
    queryFn: () => api.listSnapshots(machine ?? null),
    refetchInterval: 15000,
    refetchIntervalInBackground: true,
    staleTime: 5000,
    placeholderData: (prev) => prev,
  });
}

export function useFlavors() {
  return useQuery({
    queryKey: qk.flavors,
    queryFn: () => api.listFlavors(),
    // Catalog is static; snapshots/user flavors change rarely. Poll gently so a
    // new snapshot appears without a manual refresh.
    refetchInterval: 15000,
    refetchIntervalInBackground: true,
    staleTime: 5000,
    placeholderData: (prev) => prev,
  });
}

export function useNetworks() {
  return useQuery({
    queryKey: qk.networks,
    queryFn: () => api.listNetworks(),
    refetchInterval: 8000,
    refetchIntervalInBackground: true,
    placeholderData: (prev) => prev,
  });
}

export function useCreateNetwork() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => api.createNetwork(name),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.networks }),
  });
}

export function useRemoveNetwork() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, force }: { name: string; force: boolean }) =>
      api.removeNetwork(name, force),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.networks }),
  });
}

export function useSyncNetwork() {
  return useMutation({
    mutationFn: (name: string) => api.syncNetwork(name),
  });
}

export function useProbe() {
  return useQuery({
    queryKey: qk.probe,
    queryFn: () => api.probe(),
    refetchInterval: 15000,
    refetchIntervalInBackground: true,
    staleTime: 5000,
  });
}

export function useSettings() {
  return useQuery({
    queryKey: qk.settings,
    queryFn: () => api.getSettings(),
    staleTime: Infinity,
  });
}

export function useSystemStats() {
  return useQuery({
    queryKey: ["system-stats"] as const,
    queryFn: () => api.systemStats(),
    refetchInterval: 2000,
    refetchIntervalInBackground: true,
    placeholderData: (prev) => prev,
  });
}

export function useVersions(os: string, enabled: boolean) {
  return useQuery({
    queryKey: qk.versions(os),
    queryFn: () => api.listVersions(os),
    enabled,
    staleTime: 5 * 60 * 1000,
  });
}

// ---- mutations -------------------------------------------------------------

/** Invalidate all the machine-adjacent lists at once. */
export function useRefreshAll() {
  const qc = useQueryClient();
  return useCallback(() => {
    qc.invalidateQueries({ queryKey: qk.machines });
    qc.invalidateQueries({ queryKey: qk.images });
    qc.invalidateQueries({ queryKey: qk.volumes });
    qc.invalidateQueries({ queryKey: qk.probe });
  }, [qc]);
}

export function useRunMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (spec: RunSpec) => api.runMachine(spec),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.machines });
      qc.invalidateQueries({ queryKey: qk.images });
      qc.invalidateQueries({ queryKey: qk.volumes });
    },
  });
}

export function useUpdateMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, cpus, mem }: { id: string; cpus: number; mem: number }) =>
      api.updateMachine(id, cpus, mem),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.machines }),
  });
}

export function useUpdateMachineNetwork() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, network }: { id: string; network: string | null }) =>
      api.updateMachineNetwork(id, network),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.machines }),
  });
}

export function useStopMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.stopMachine(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.machines }),
  });
}

export function useRestartMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.restartMachine(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.machines }),
  });
}

export function useRemoveMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, force }: { id: string; force: boolean }) =>
      api.removeMachine(id, force),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.machines }),
  });
}

export function useRemoveVolume() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, force }: { name: string; force: boolean }) =>
      api.removeVolume(name, force),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.volumes }),
  });
}

export function useRunFlavor() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      name,
      ports = [],
      volume = null,
    }: {
      name: string;
      ports?: string[];
      volume?: string | null;
    }) => api.runFlavor(name, ports, volume),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.machines });
      qc.invalidateQueries({ queryKey: qk.images });
      qc.invalidateQueries({ queryKey: qk.volumes });
    },
  });
}

export function useCommitMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      name,
      description = "",
    }: {
      id: string;
      name: string;
      description?: string;
    }) => api.commitMachine(id, name, description),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.flavors });
      // A BSD machine is powered off to take a consistent snapshot.
      qc.invalidateQueries({ queryKey: qk.machines });
    },
  });
}

// ---- ai agents ---------------------------------------------------------------

export function useAiAgents(enabled = true) {
  return useQuery({
    queryKey: qk.aiAgents,
    queryFn: () => api.aiAgents(),
    enabled,
    // The "installed" flag flips when a first launch finishes building, and
    // that is what the panel's copy keys off.
    refetchInterval: 15000,
    staleTime: 5000,
    placeholderData: (prev) => prev,
  });
}

export function useAiSessions(enabled = true) {
  return useQuery({
    queryKey: qk.aiSessions,
    queryFn: () => api.aiSessions(),
    enabled,
    refetchInterval: 8000,
    placeholderData: (prev) => prev,
  });
}

/**
 * Start (or reuse) an agent sandbox and point the panel's terminal at it.
 *
 * An agent whose flavor is not built yet installs a toolchain on first run —
 * minutes — so that path goes through the streaming progress modal and resolves
 * when it reports the new machine id. An installed agent boots in about a
 * second and skips the modal entirely.
 */
export function useStartAgent() {
  const qc = useQueryClient();
  const setSession = useSetAtom(agentSessionAtom);
  const setOpen = useSetAtom(agentPanelOpenAtom);
  const setSelected = useSetAtom(agentSelectedAtom);
  const setLaunch = useSetAtom(launchStateAtom);

  return useCallback(
    async (
      agent: string,
      workspace: string | null,
      newSession: boolean,
      name?: string,
    ) => {
      setSelected(agent);
      setOpen(true);

      const agents = await qc.ensureQueryData({
        queryKey: qk.aiAgents,
        queryFn: () => api.aiAgents(),
      });
      const info = agents.find((a) => a.id === agent);
      const label = info?.label ?? agent;

      const machineId = info?.installed
        ? await api.aiStart(agent, workspace, newSession, name)
        : await streamInstall(setLaunch, label, agent, workspace, newSession, name);

      const command = await api.aiShellCommand(agent, machineId);
      setSession({
        // A fresh key remounts the terminal — a new PTY for a new sandbox.
        key: `${machineId}-${Date.now()}`,
        agent,
        machineId,
        command,
      });
      qc.invalidateQueries({ queryKey: ["ai"] });
      qc.invalidateQueries({ queryKey: qk.machines });
      return machineId;
    },
    [qc, setSession, setOpen, setSelected, setLaunch],
  );
}

/**
 * First run of an agent: stream the flavor build into the progress modal, and
 * resolve with the machine id it reports.
 */
function streamInstall(
  setLaunch: (s: any) => void,
  label: string,
  agent: string,
  workspace: string | null,
  newSession: boolean,
  name?: string,
): Promise<string> {
  const launchId = `agent-${agent}-${Date.now()}`;
  setLaunch({
    launchId,
    name: `${label} (installing)`,
    mode: "launch",
    lines: [],
    status: "running",
  });
  return new Promise<string>((resolve, reject) => {
    let unlisten: (() => void) | null = null;
    onFlavorDone((p) => {
      if (p.launch_id !== launchId) return;
      unlisten?.();
      if (p.error || !p.id) {
        reject(new Error(p.error || "the sandbox did not report an id"));
      } else {
        resolve(p.id);
      }
    }).then((u) => {
      unlisten = u;
    });
    api.launchAgent(launchId, agent, workspace, newSession, name).catch((e) => {
      unlisten?.();
      reject(e);
    });
  });
}

/**
 * Attach the panel to an existing sandbox.
 *
 * A session is a machine, so this only has to fetch the agent's argv and point
 * the terminal at it — no boot, and nothing to wait for.
 */
export function useAttachAgentSession() {
  const setSession = useSetAtom(agentSessionAtom);
  const setOpen = useSetAtom(agentPanelOpenAtom);
  const setSelected = useSetAtom(agentSelectedAtom);
  return useCallback(
    async (session: { id: string; agent: string }) => {
      const command = await api.aiShellCommand(session.agent, session.id);
      setSelected(session.agent);
      setOpen(true);
      setSession({
        key: `${session.id}-${Date.now()}`,
        agent: session.agent,
        machineId: session.id,
        command,
      });
    },
    [setSession, setOpen, setSelected],
  );
}

/** Stop one sandbox. It is a machine, so this is the ordinary machine stop. */
export function useStopAgentSession() {
  const qc = useQueryClient();
  const setSession = useSetAtom(agentSessionAtom);
  return useMutation({
    mutationFn: (id: string) => api.stopMachine(id),
    onSuccess: (_r, id) => {
      // The panel is showing a dead terminal if this was the live session.
      setSession((s) => (s?.machineId === id ? null : s));
      qc.invalidateQueries({ queryKey: ["ai"] });
      qc.invalidateQueries({ queryKey: qk.machines });
    },
  });
}

/** Delete one sandbox and its state. Its agent's saved login is untouched. */
export function useDeleteAgentSession() {
  const qc = useQueryClient();
  const setSession = useSetAtom(agentSessionAtom);
  return useMutation({
    mutationFn: (id: string) => api.removeMachine(id, true),
    onSuccess: (_r, id) => {
      setSession((s) => (s?.machineId === id ? null : s));
      qc.invalidateQueries({ queryKey: ["ai"] });
      qc.invalidateQueries({ queryKey: qk.machines });
    },
  });
}

export function useStopAgent() {
  const qc = useQueryClient();
  const setSession = useSetAtom(agentSessionAtom);
  return useMutation({
    mutationFn: (agent: string) => api.aiStop(agent),
    onSuccess: () => {
      setSession(null);
      qc.invalidateQueries({ queryKey: ["ai"] });
      qc.invalidateQueries({ queryKey: qk.machines });
    },
  });
}

// ---- docker -----------------------------------------------------------------

/**
 * The Docker engine's status. Polled briskly while it is starting (the boot
 * takes seconds and the UI is showing a spinner), gently once it is up.
 */
export function useDockerStatus() {
  return useQuery({
    queryKey: qk.dockerStatus,
    queryFn: () => api.dockerStatus(),
    refetchInterval: (q) => (q.state.data?.running ? 10000 : 3000),
    refetchIntervalInBackground: true,
    placeholderData: (prev) => prev,
  });
}

/** Containers, `docker ps`-style. Only polled while the engine is up. */
export function useDockerContainers(all = true, enabled = true) {
  return useQuery({
    queryKey: qk.dockerContainers(all),
    queryFn: () => api.dockerContainers(all),
    enabled,
    refetchInterval: 4000,
    refetchIntervalInBackground: true,
    placeholderData: (prev) => prev,
  });
}

function invalidateDocker(qc: ReturnType<typeof useQueryClient>) {
  qc.invalidateQueries({ queryKey: ["docker"] });
}

export function useDockerStart() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      cpus,
      mem,
      diskSize,
    }: {
      cpus?: number;
      mem?: number;
      diskSize?: string;
    } = {}) => api.dockerStart(cpus, mem, diskSize),
    onSuccess: () => {
      invalidateDocker(qc);
      // The engine is a machine like any other, so it shows up in Machines too.
      qc.invalidateQueries({ queryKey: qk.machines });
    },
  });
}

export function useDockerStop() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api.dockerStop(),
    onSuccess: () => {
      invalidateDocker(qc);
      qc.invalidateQueries({ queryKey: qk.machines });
    },
  });
}

export function useDockerContainerAction() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ action, id }: { action: string; id: string }) =>
      api.dockerContainer(action, id),
    onSuccess: () => invalidateDocker(qc),
  });
}

// ---- snapshots -------------------------------------------------------------

/** Invalidate every snapshot list — the global one and each machine's. */
function invalidateSnapshots(qc: ReturnType<typeof useQueryClient>) {
  qc.invalidateQueries({ queryKey: ["snapshots"] });
}

export function useSnapshotMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      name = null,
      description = "",
    }: {
      id: string;
      name?: string | null;
      description?: string;
    }) => api.snapshotMachine(id, name, description),
    onSuccess: () => {
      invalidateSnapshots(qc);
      // A BSD guest is powered off to capture a consistent disk.
      qc.invalidateQueries({ queryKey: qk.machines });
    },
  });
}

export function useRemoveSnapshot() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => api.removeSnapshot(name),
    onSuccess: () => invalidateSnapshots(qc),
  });
}

export function useRestoreMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      id,
      snapshot,
      backup = true,
    }: {
      id: string;
      snapshot: string;
      backup?: boolean;
    }) => api.restoreMachine(id, snapshot, backup),
    onSuccess: () => {
      // The machine is left stopped, and the safety backup is a new snapshot.
      qc.invalidateQueries({ queryKey: qk.machines });
      invalidateSnapshots(qc);
    },
  });
}

export function useRollbackMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, backup = true }: { id: string; backup?: boolean }) =>
      api.rollbackMachine(id, backup),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.machines });
      invalidateSnapshots(qc);
    },
  });
}

export function useBranchSnapshot() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      snapshot,
      name = null,
      ports = [],
    }: {
      snapshot: string;
      name?: string | null;
      ports?: string[];
    }) => api.branchSnapshot(snapshot, name, ports),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.machines });
      qc.invalidateQueries({ queryKey: qk.volumes });
    },
  });
}

export function useCreateFlavor() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (spec: NewFlavor) => api.createFlavor(spec),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.flavors }),
  });
}

export function useRemoveFlavor() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, force }: { name: string; force: boolean }) =>
      api.removeFlavor(name, force),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.flavors }),
  });
}

export function useSaveSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ url, token }: { url: string; token: string }) =>
      api.setSettings(url, token),
    onSuccess: (s) => {
      qc.setQueryData(qk.settings, s);
      // Pointing at a different daemon invalidates everything we know.
      qc.invalidateQueries();
    },
  });
}
