import { useEffect, useRef } from "react";
import { useSetAtom } from "jotai";
import { onMenuAction } from "../lib/api";
import { api } from "../lib/api";
import { useAtomValue } from "jotai";
import {
  openHostTerminalAtom,
  paletteOpenAtom,
  runOpenAtom,
  settingsOpenAtom,
  shortcutsOpenAtom,
  agentPanelOpenAtom,
  terminalCollapsedAtom,
  terminalTabsAtom,
  viewAtom,
} from "../state/atoms";
import { useMachines, useRefreshAll } from "../lib/queries";
import { useToast } from "../state/toast";
import type { ViewKey } from "../lib/types";

/** Open a link in a new tab. `noopener` so the page cannot reach back via window.opener. */
const openExternal = (url: string) => {
  window.open(url, "_blank", "noopener,noreferrer");
  return Promise.resolve();
};

const DOCS_URL = "https://github.com/tsirysndr/bsdkrun";

/**
 * Wires global keyboard shortcuts and native-menu actions to app state.
 * Single-key shortcuts are ignored while typing in a field; the ⌘-accelerators
 * come in through the native menu (`menu://action`).
 */
export function useShortcuts() {
  const setPalette = useSetAtom(paletteOpenAtom);
  const setRun = useSetAtom(runOpenAtom);
  const setSettings = useSetAtom(settingsOpenAtom);
  const setShortcuts = useSetAtom(shortcutsOpenAtom);
  const setView = useSetAtom(viewAtom);
  const setAgentPanel = useSetAtom(agentPanelOpenAtom);
  const openHostTerminal = useSetAtom(openHostTerminalAtom);
  const setTermCollapsed = useSetAtom(terminalCollapsedAtom);
  const termTabs = useAtomValue(terminalTabsAtom);
  const refreshAll = useRefreshAll();
  const { data: machines = [] } = useMachines();
  const toast = useToast();

  const machinesRef = useRef(machines);
  machinesRef.current = machines;
  const refreshRef = useRef(refreshAll);
  refreshRef.current = refreshAll;

  // Toggle the bottom terminal panel (open a host terminal if none exist).
  const toggleTerminalRef = useRef(() => {});
  toggleTerminalRef.current = () => {
    if (termTabs.length === 0) openHostTerminal();
    else setTermCollapsed((c) => !c);
  };

  // Shared action dispatcher for both keyboard + menu.
  const dispatchRef = useRef((action: string) => {
    switch (action) {
      case "palette":
        setPalette(true);
        break;
      case "run":
        setRun(true);
        break;
      case "refresh":
        refreshRef.current();
        break;
      case "settings":
        setSettings(true);
        break;
      case "agent-panel":
        setAgentPanel((v) => !v);
        break;
      case "shortcuts":
        setShortcuts(true);
        break;
      case "docs":
        openExternal(DOCS_URL).catch(() => {});
        break;
      case "stop-all": {
        const running = machinesRef.current.filter((m) => m.running);
        if (running.length === 0) {
          toast("info", "No running machines");
          break;
        }
        Promise.allSettled(running.map((m) => api.stopMachine(m.id))).then(() => {
          toast("success", `Stopped ${running.length} machine(s)`);
          refreshRef.current();
        });
        break;
      }
      default:
        if (action.startsWith("nav:")) {
          setView(action.slice(4) as ViewKey);
        }
    }
  });

  // Native menu → actions.
  useEffect(() => {
    const p = onMenuAction((a) => dispatchRef.current(a));
    return () => {
      p.then((un) => un());
    };
  }, []);

  // Keyboard shortcuts.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      const typing =
        !!t &&
        (t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.isContentEditable);

      // ⌘K / Ctrl+K works everywhere.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette(true);
        return;
      }
      // ⌘J / Ctrl+J toggles the AI agent panel, everywhere — including while
      // typing, since the panel's own terminal counts as an input.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "j") {
        e.preventDefault();
        setAgentPanel((v) => !v);
        return;
      }
      // Ctrl+` toggles the bottom terminal panel (VS Code-style), everywhere.
      if ((e.ctrlKey || e.metaKey) && e.key === "`") {
        e.preventDefault();
        toggleTerminalRef.current();
        return;
      }
      if (typing || e.metaKey || e.ctrlKey || e.altKey) return;

      switch (e.key) {
        case "/":
          e.preventDefault();
          setPalette(true);
          break;
        case "?":
          e.preventDefault();
          setShortcuts(true);
          break;
        case "r":
        case "R":
          refreshRef.current();
          break;
        case "n":
        case "N":
          setRun(true);
          break;
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [setPalette, setShortcuts, setRun]);
}
