import { atom } from "jotai";
import { atomWithStorage } from "jotai/utils";
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
  kind: string; // guest kind — BSD snapshots power the machine off first
  running: boolean;
}
export const commitTargetAtom = atom<CommitTarget | null>(null);

// The machine being snapshotted (`bsdkrun snapshot`); null ⇒ closed.
export interface SnapshotTarget {
  id: string;
  label: string;
  kind: string; // guest kind — a BSD guest is powered off to capture a clean disk
  running: boolean;
}
export const snapshotTargetAtom = atom<SnapshotTarget | null>(null);

// What is being branched into a new machine; null ⇒ closed.
export interface BranchTarget {
  /**
   * A snapshot name/id, or a machine id — the engine snapshots a machine
   * first, so "branch this machine" needs no separate step in the UI.
   */
  snapshot: string;
  /** What to call the source in the dialog's copy. */
  label: string;
  /** True when `snapshot` names a live machine rather than a saved snapshot. */
  fromMachine: boolean;
  kind: string;
  /** Whether that machine is running — a BSD guest is powered off to snapshot. */
  running?: boolean;
  /** Recorded forwards, offered as the branch's defaults. */
  ports: string[];
}
export const branchTargetAtom = atom<BranchTarget | null>(null);

// The machine whose CPU/RAM is being edited; null ⇒ closed.
export interface EditResourcesTarget {
  id: string;
  label: string;
  cpus: number;
  mem: number;
  running: boolean;
}
export const editResourcesAtom = atom<EditResourcesTarget | null>(null);

// The machine whose global-network membership is being edited; null ⇒ closed.
export interface EditNetworkTarget {
  id: string;
  label: string;
  network: string | null;
  running: boolean;
}
export const editNetworkAtom = atom<EditNetworkTarget | null>(null);

// ---- the right-side AI agent panel ------------------------------------------
//
// The panel keeps its terminal mounted while hidden (see `AgentPanel`), so a
// session survives being toggled away — which is the whole point of docking it
// rather than opening a tab.

/** Whether the right panel is shown. */
export const agentPanelOpenAtom = atom<boolean>(false);

/** Panel width in px (drag-to-resize; persists across toggles). */
export const agentPanelWidthAtom = atom<number>(460);

/** Expand the panel over the whole workspace. */
export const agentPanelFullscreenAtom = atom<boolean>(false);

/** Which agent the dropdown has selected. */
export const agentSelectedAtom = atom<string>("claude");

/** The host directory shared with the agent, or null for an isolated sandbox. */
export const agentWorkspaceAtom = atom<string | null>(null);

/** The live session: the sandbox machine and the argv that starts its TUI. */
export interface AgentSession {
  /** Remounts the terminal when it changes — a new session, a new pane. */
  key: string;
  agent: string;
  machineId: string;
  command: string[];
}
export const agentSessionAtom = atom<AgentSession | null>(null);

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
  /**
   * Dismiss the modal on success instead of waiting to be closed.
   *
   * Set for agent launches, whose result is the terminal appearing behind this
   * modal — a flavor launch instead *offers* its result (the machine id and a
   * button to open it), so it stays. Errors ignore this either way.
   */
  autoClose?: boolean;
}
export const launchStateAtom = atom<LaunchState | null>(null);

// ---- CI/CD ----------------------------------------------------------------

/** The repository the CI screen runs against. Persisted: picking it again on
 * every launch would make the screen feel like it forgot you. */
export const ciRepoAtom = atomWithStorage<string>("bsdkrun-ci-repo", "");

/** Recent CI runs, client-side only — a viewing history, not a system of
 * record; the engine deliberately stays stateless about them. Capped in the
 * view so localStorage stays sane. */
export const ciRunsAtom = atomWithStorage<import("../lib/types").CiRun[]>(
  "bsdkrun-ci-runs",
  [],
);
