import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useCallback } from "react";
import { api } from "./api";
import type { RunSpec } from "./types";

export const qk = {
  machines: ["machines"] as const,
  images: ["images"] as const,
  volumes: ["volumes"] as const,
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
    placeholderData: (prev) => prev,
  });
}

export function useImages() {
  return useQuery({
    queryKey: qk.images,
    queryFn: () => api.listImages(),
    refetchInterval: 10000,
    placeholderData: (prev) => prev,
  });
}

export function useVolumes() {
  return useQuery({
    queryKey: qk.volumes,
    queryFn: () => api.listVolumes(),
    refetchInterval: 10000,
    placeholderData: (prev) => prev,
  });
}

export function useProbe() {
  return useQuery({
    queryKey: qk.probe,
    queryFn: () => api.probe(),
    refetchInterval: 15000,
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

export function useStopMachine() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.stopMachine(id),
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

export function useSaveSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (binaryPath: string) => api.setSettings(binaryPath),
    onSuccess: (s) => {
      qc.setQueryData(qk.settings, s);
      qc.invalidateQueries({ queryKey: qk.probe });
    },
  });
}
