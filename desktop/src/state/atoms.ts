import { atom } from "jotai";
import type { ViewKey } from "../lib/types";

// UI-only state. Server data (machines / images / volumes / probe / settings)
// lives in TanStack Query — see src/lib/queries.ts.

// Active sidebar view.
export const viewAtom = atom<ViewKey>("machines");

// Whether the left sidebar is shown (toggle from the top bar).
export const sidebarVisibleAtom = atom<boolean>(true);

// Per-view filter text (the toolbar search box).
export const filterAtom = atom<string>("");

// The machine whose detail drawer is open (null ⇒ closed).
export const selectedMachineAtom = atom<string | null>(null);

// Bottom-docked interactive terminal panel: one entry per open terminal tab.
export interface TerminalTab {
  id: string; // unique tab id
  machineId: string;
}
export const terminalTabsAtom = atom<TerminalTab[]>([]);
export const activeTerminalAtom = atom<string | null>(null);
export const terminalFullscreenAtom = atom<boolean>(false);
// Hide the docked terminal panel without closing its tabs (topbar toggle).
export const terminalCollapsedAtom = atom<boolean>(false);

// Docked terminal panel height in px (drag-to-resize; persists across opens).
export const terminalHeightAtom = atom<number>(320);

let termSeq = 1;

/** Sentinel machineId for a terminal running on the host (not a guest). */
export const HOST_MACHINE = "__host__";

/** Open a NEW terminal tab for a machine (never replaces an existing one). */
export const openTerminalAtom = atom(null, (get, set, machineId: string) => {
  const id = `term-tab-${termSeq++}`;
  set(terminalTabsAtom, [...get(terminalTabsAtom), { id, machineId }]);
  set(activeTerminalAtom, id);
  set(terminalCollapsedAtom, false);
});

/** Open a NEW terminal tab on the host machine. */
export const openHostTerminalAtom = atom(null, (get, set) => {
  const id = `term-tab-${termSeq++}`;
  set(terminalTabsAtom, [
    ...get(terminalTabsAtom),
    { id, machineId: HOST_MACHINE },
  ]);
  set(activeTerminalAtom, id);
  set(terminalCollapsedAtom, false);
});

/** Close a tab; activate a neighbour, and drop out of fullscreen if it was last. */
export const closeTerminalAtom = atom(null, (get, set, tabId: string) => {
  const tabs = get(terminalTabsAtom);
  const idx = tabs.findIndex((t) => t.id === tabId);
  const next = tabs.filter((t) => t.id !== tabId);
  set(terminalTabsAtom, next);
  if (get(activeTerminalAtom) === tabId) {
    const neighbour = next[idx] || next[idx - 1] || null;
    set(activeTerminalAtom, neighbour ? neighbour.id : null);
  }
  if (next.length === 0) set(terminalFullscreenAtom, false);
});

// Global overlays.
export const newFlavorOpenAtom = atom<boolean>(false);
export const runOpenAtom = atom<boolean>(false);
export const settingsOpenAtom = atom<boolean>(false);
export const paletteOpenAtom = atom<boolean>(false);
export const shortcutsOpenAtom = atom<boolean>(false);
export const cliModalOpenAtom = atom<boolean>(false);

// Prefill for the Run dialog (e.g. launched from an image row).
export const runPrefillAtom = atom<{ kind?: string; image?: string } | null>(
  null,
);

// The machine being snapshotted into a flavor (`bsdkrun commit`); null ⇒ closed.
export interface CommitTarget {
  id: string;
  label: string; // friendly name/image for the dialog copy
}
export const commitTargetAtom = atom<CommitTarget | null>(null);

// Live state of a streaming flavor launch/build (the progress modal). null ⇒ closed.
export interface LaunchState {
  launchId: string;
  name: string;
  /** "launch" boots a machine; "build" only pre-builds the provisioning cache. */
  mode: "launch" | "build";
  lines: string[];
  status: "running" | "done" | "error";
  machineId?: string | null;
  error?: string | null;
}
export const launchStateAtom = atom<LaunchState | null>(null);
