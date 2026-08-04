import { useSetAtom } from "jotai";
import { useCallback } from "react";
import { api } from "../lib/api";
import { launchStateAtom } from "../state/atoms";
import type { RunSpec } from "../lib/types";

/** A short unique id for correlating a launch's streamed events. */
function newLaunchId(): string {
  const c = globalThis.crypto;
  if (c && "randomUUID" in c) return c.randomUUID();
  return `launch-${Math.floor(performance.now())}-${Math.floor(Math.random() * 1e6)}`;
}

/**
 * Start a *streaming* flavor launch: opens the progress modal immediately and
 * returns at once. The modal ([`LaunchProgressModal`]) subscribes to the
 * `flavor://log` / `flavor://done` events to show pull / build / boot progress.
 */
export function useLaunchFlavor() {
  const setLaunch = useSetAtom(launchStateAtom);
  return useCallback(
    (
      name: string,
      opts?: { ports?: string[]; volume?: string | null; repo?: string | null },
    ) => {
      const launchId = newLaunchId();
      setLaunch({ launchId, name, mode: "launch", lines: [], status: "running" });
      api
        .launchFlavor(
          launchId,
          name,
          opts?.ports ?? [],
          opts?.volume ?? null,
          opts?.repo ?? null,
        )
        .catch((e) =>
          setLaunch((s) =>
            s && s.launchId === launchId
              ? { ...s, status: "error", error: String(e) }
              : s,
          ),
        );
    },
    [setLaunch],
  );
}

/**
 * Launch a machine from the Run dialog with STREAMING progress — so an OCI pull
 * or a BSD image/kernel download shows live logs in the progress modal instead
 * of a silent spinner. `name` is a display label (the image ref or guest OS).
 */
export function useLaunchMachine() {
  const setLaunch = useSetAtom(launchStateAtom);
  return useCallback(
    (name: string, spec: RunSpec) => {
      const launchId = newLaunchId();
      setLaunch({ launchId, name, mode: "launch", lines: [], status: "running" });
      api.launchMachine(launchId, spec).catch((e) =>
        setLaunch((s) =>
          s && s.launchId === launchId
            ? { ...s, status: "error", error: String(e) }
            : s,
        ),
      );
    },
    [setLaunch],
  );
}

/**
 * Pre-build a flavor's provisioning into the cache, streaming the build logs in
 * the same progress modal (mode "build"). Used right after saving a flavor.
 */
export function useBuildFlavor() {
  const setLaunch = useSetAtom(launchStateAtom);
  return useCallback(
    (name: string) => {
      const launchId = newLaunchId();
      setLaunch({ launchId, name, mode: "build", lines: [], status: "running" });
      api.buildFlavor(launchId, name).catch((e) =>
        setLaunch((s) =>
          s && s.launchId === launchId
            ? { ...s, status: "error", error: String(e) }
            : s,
        ),
      );
    },
    [setLaunch],
  );
}
