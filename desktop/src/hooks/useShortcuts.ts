import { useEffect, useRef } from "react";
import { useSetAtom } from "jotai";
import { openUrl } from "@tauri-apps/plugin-opener";
import { onMenuAction } from "../lib/api";
import { api } from "../lib/api";
import {
  paletteOpenAtom,
  runOpenAtom,
  settingsOpenAtom,
  shortcutsOpenAtom,
  viewAtom,
} from "../state/atoms";
import { useMachines, useRefreshAll } from "../lib/queries";
import { useToast } from "../state/toast";
import type { ViewKey } from "../lib/types";

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
  const refreshAll = useRefreshAll();
  const { data: machines = [] } = useMachines();
  const toast = useToast();

  const machinesRef = useRef(machines);
  machinesRef.current = machines;
  const refreshRef = useRef(refreshAll);
  refreshRef.current = refreshAll;

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
      case "shortcuts":
        setShortcuts(true);
        break;
      case "docs":
        openUrl(DOCS_URL).catch(() => {});
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
