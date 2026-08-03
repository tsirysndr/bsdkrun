import { useAtomValue } from "jotai";
import { sidebarVisibleAtom, viewAtom } from "./state/atoms";
import { useShortcuts } from "./hooks/useShortcuts";
import TopBar from "./components/TopBar";
import Sidebar from "./components/Sidebar";
import MachinesView from "./components/MachinesView";
import ImagesView from "./components/ImagesView";
import VolumesView from "./components/VolumesView";
import RunDialog from "./components/RunDialog";
import MachineDetail from "./components/MachineDetail";
import CommandPalette from "./components/CommandPalette";
import ShortcutsModal from "./components/ShortcutsModal";
import SettingsModal from "./components/SettingsModal";
import TerminalPanel from "./components/TerminalPanel";
import { Toaster } from "./state/toast";

export default function App() {
  const view = useAtomValue(viewAtom);
  const sidebarVisible = useAtomValue(sidebarVisibleAtom);
  useShortcuts();

  return (
    <div className="flex h-[100dvh] w-screen flex-col overflow-hidden bg-background">
      <TopBar />
      <div className="relative flex min-h-0 flex-1 flex-col">
        <div className="flex min-h-0 flex-1">
          {sidebarVisible && <Sidebar />}
          <div className="min-h-0 min-w-0 flex-1 overflow-hidden">
            {view === "machines" && <MachinesView />}
            {view === "images" && <ImagesView />}
            {view === "volumes" && <VolumesView />}
          </div>
        </div>
        <TerminalPanel />
      </div>

      {/* Overlays */}
      <RunDialog />
      <MachineDetail />
      <CommandPalette />
      <ShortcutsModal />
      <SettingsModal />
      <Toaster />
    </div>
  );
}
