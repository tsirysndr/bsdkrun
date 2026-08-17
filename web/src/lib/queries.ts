import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useCallback } from "react";
import { api } from "./api";
import type { NewFlavor, RunSpec } from "./types";

export const qk = {
  machines: ["machines"] as const,
  images: ["images"] as const,
  volumes: ["volumes"] as const,
  // Every machine's list lives under the same "snapshots" prefix, so one
  // invalidate after a mutation refreshes the global view and each machine's.
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
