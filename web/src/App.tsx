import { useState } from "react";
import { useAtomValue } from "jotai";
import { sidebarVisibleAtom, viewAtom } from "./state/atoms";
import { getConnection } from "./lib/connection";
import ConnectPane from "./components/ConnectPane";
import { useShortcuts } from "./hooks/useShortcuts";
import { useNoAutofill } from "./hooks/useNoAutofill";
import TopBar from "./components/TopBar";
import Sidebar from "./components/Sidebar";
import MachinesView from "./components/MachinesView";
import ImagesView from "./components/ImagesView";
import VolumesView from "./components/VolumesView";
import SnapshotsView from "./components/SnapshotsView";
import ContainersView from "./components/ContainersView";
import CicdView from "./components/CicdView";
import AgentPanel from "./components/AgentPanel";
import FlavorsView from "./components/FlavorsView";
import NetworksView from "./components/NetworksView";
import RunDialog from "./components/RunDialog";
import CommitDialog from "./components/CommitDialog";
import SnapshotDialog from "./components/SnapshotDialog";
import BranchDialog from "./components/BranchDialog";
import EditResourcesDialog from "./components/EditResourcesDialog";
import EditNetworkDialog from "./components/EditNetworkDialog";
import NewFlavorModal from "./components/NewFlavorModal";
import LaunchProgressModal from "./components/LaunchProgressModal";
import MachineDetail from "./components/MachineDetail";
import CommandPalette from "./components/CommandPalette";
import ShortcutsModal from "./components/ShortcutsModal";
import SettingsModal from "./components/SettingsModal";
import CliModal from "./components/CliModal";
import TerminalPanel from "./components/TerminalPanel";
import StatusBar from "./components/StatusBar";
import { Toaster } from "./state/toast";

export default function App() {
  const [connected, setConnected] = useState(() => getConnection() !== null);
  if (!connected) return <ConnectPane onConnected={() => setConnected(true)} />;
  return <Workspace />;
}

function Workspace() {
  const view = useAtomValue(viewAtom);
  const sidebarVisible = useAtomValue(sidebarVisibleAtom);
  useShortcuts();
  useNoAutofill();

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
            {view === "containers" && <ContainersView />}
            {view === "snapshots" && <SnapshotsView />}
            {view === "flavors" && <FlavorsView />}
            {view === "cicd" && <CicdView />}
            {view === "networks" && <NetworksView />}
          </div>
          <AgentPanel />
        </div>
        <TerminalPanel />
      </div>
      <StatusBar />

      {/* Overlays */}
      <RunDialog />
      <CommitDialog />
      <SnapshotDialog />
      <BranchDialog />
      <EditResourcesDialog />
      <EditNetworkDialog />
      <NewFlavorModal />
      <LaunchProgressModal />
      <MachineDetail />
      <CommandPalette />
      <ShortcutsModal />
      <SettingsModal />
      <CliModal />
      <Toaster />
    </div>
  );
}
