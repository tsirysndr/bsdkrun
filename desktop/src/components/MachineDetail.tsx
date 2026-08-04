import { useEffect, useState } from "react";
import { ConfirmDialog } from "./ConfirmDialog";
import {
  Button,
  Drawer,
  DrawerBody,
  DrawerContent,
  DrawerHeader,
  Tab,
  Tabs,
  Tooltip,
} from "@heroui/react";
import { useAtom } from "jotai";
import {
  IconPlayerStopFilled,
  IconPlayerPlayFilled,
  IconTerminal2,
  IconFileText,
  IconInfoCircle,
  IconCopy,
  IconCamera,
  IconCpu,
  IconTrash,
  IconSettingsBolt,
} from "@tabler/icons-react";
import { useSetAtom } from "jotai";
import {
  commitTargetAtom,
  editResourcesAtom,
  selectedMachineAtom,
} from "../state/atoms";
import {
  useMachines,
  useRemoveMachine,
  useRestartMachine,
  useStopMachine,
} from "../lib/queries";
import { ago, exitLabel, kindColor, shortId } from "../lib/format";
import { useToast } from "../state/toast";
import TerminalPane from "./TerminalPane";
import LogsPane from "./LogsPane";
import SetupPane from "./SetupPane";
import type { Machine } from "../lib/types";

const EMPTY: string[] = [];

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  const toast = useToast();
  return (
    <div className="flex items-start justify-between gap-4 border-b border-white/5 py-2.5">
      <span className="text-xs text-foreground-500">{label}</span>
      <span
        className={`group flex items-center gap-2 text-right text-sm text-foreground-300 ${mono ? "font-mono text-xs" : ""}`}
      >
        <span className="break-all">{value}</span>
        <button
          className="opacity-0 transition group-hover:opacity-100"
          onClick={() => {
            navigator.clipboard?.writeText(value);
            toast("info", "Copied");
          }}
        >
          <IconCopy size={13} className="text-foreground-500" />
        </button>
      </span>
    </div>
  );
}

function Inspect({ m }: { m: Machine }) {
  return (
    <div className="px-1">
      <Row label="Name" value={m.name || "—"} />
      <Row label="ID" value={m.id} mono />
      <Row label="Image" value={m.image || "—"} />
      <Row label="Guest" value={kindColor(m.kind, m.image).label} />
      <Row label="Command" value={m.command || "—"} mono />
      <Row label="Status" value={m.running ? "Running" : exitLabel(m.exit_code)} />
      <Row label="PID" value={m.pid != null ? String(m.pid) : "—"} />
      <Row label="vCPUs" value={m.cpus != null ? String(m.cpus) : "—"} />
      <Row label="Memory" value={m.mem != null ? `${m.mem} MiB` : "—"} />
      <Row label="Volume" value={m.volume || "—"} />
      <Row label="Created" value={m.created_at || "—"} />
      {m.finished_at && <Row label="Finished" value={m.finished_at} />}
      <Row label="State dir" value={m.state_dir || "—"} mono />
    </div>
  );
}

export default function MachineDetail() {
  const [selected, setSelected] = useAtom(selectedMachineAtom);
  const { data: machines = [] } = useMachines();
  const stopMutation = useStopMachine();
  const restartMutation = useRestartMachine();
  const removeMutation = useRemoveMachine();
  const setCommitTarget = useSetAtom(commitTargetAtom);
  const setEditResources = useSetAtom(editResourcesAtom);
  const toast = useToast();
  const m = machines.find((x) => x.id === selected) || null;
  const [tab, setTab] = useState<string>("logs");
  const [confirmRemove, setConfirmRemove] = useState(false);

  useEffect(() => {
    // Default to Logs whenever a machine is opened.
    if (selected) setTab("logs");
  }, [selected]);

  const stop = async () => {
    if (!m) return;
    try {
      await stopMutation.mutateAsync(m.id);
      toast("success", `Stopped ${shortId(m.id)}`);
    } catch (e) {
      toast("error", "Failed to stop", String(e));
    }
  };

  const start = async () => {
    if (!m) return;
    try {
      await restartMutation.mutateAsync(m.id);
      toast("success", `Started ${shortId(m.id)}`);
    } catch (e) {
      toast("error", "Failed to start", String(e));
    }
  };

  const remove = async () => {
    if (!m) return;
    const id = m.id;
    setConfirmRemove(false);
    setSelected(null);
    try {
      await removeMutation.mutateAsync({ id, force: false });
      toast("success", `Removed ${shortId(id)}`);
    } catch (e) {
      toast("error", "Failed to remove", String(e));
    }
  };

  const kc = m ? kindColor(m.kind, m.image) : null;

  return (
    <>
    <Drawer
      isOpen={!!m}
      onClose={() => setSelected(null)}
      size="3xl"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{
        base: "h-[100dvh] max-h-full border-l border-white/10 bg-content1",
        wrapper: "h-[100dvh]",
      }}
    >
      <DrawerContent>
        {m && kc && (
          <>
            <DrawerHeader className="flex flex-col gap-3 border-b border-white/10">
              <div className="flex items-center gap-3 pr-8">
                <span className={`h-2.5 w-2.5 rounded-full ${kc.dot}`} />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-base font-semibold">
                    {m.name || m.image || "(disk)"}
                  </div>
                  <div className="truncate font-mono text-[11px] font-normal text-foreground-500">
                    {m.image ? `${m.image} · ` : ""}
                    {shortId(m.id)} · {kc.label} ·{" "}
                    {m.running ? "running" : exitLabel(m.exit_code).toLowerCase()} ·{" "}
                    {ago(m.created_at)}
                  </div>
                </div>
                <Tooltip content="Edit CPU / RAM" placement="bottom">
                  <Button
                    isIconOnly
                    size="sm"
                    variant="flat"
                    onPress={() =>
                      setEditResources({
                        id: m.id,
                        label: m.name || m.image || shortId(m.id),
                        cpus: m.cpus ?? 1,
                        mem: m.mem ?? 512,
                        running: m.running,
                      })
                    }
                  >
                    <IconCpu size={16} />
                  </Button>
                </Tooltip>
                <Tooltip content="Snapshot into a flavor" placement="bottom">
                  <Button
                    isIconOnly
                    size="sm"
                    variant="flat"
                    className="text-rose-300"
                    onPress={() =>
                      setCommitTarget({
                        id: m.id,
                        label: m.name || m.image || shortId(m.id),
                        kind: m.kind,
                        running: m.running,
                      })
                    }
                  >
                    <IconCamera size={16} />
                  </Button>
                </Tooltip>
                {m.running ? (
                  <Tooltip content="Stop machine" placement="bottom">
                    <Button
                      size="sm"
                      color="danger"
                      variant="flat"
                      startContent={<IconPlayerStopFilled size={14} />}
                      onPress={stop}
                    >
                      Stop
                    </Button>
                  </Tooltip>
                ) : (
                  <>
                    <Tooltip content="Start machine again" placement="bottom">
                      <Button
                        size="sm"
                        color="success"
                        variant="flat"
                        isLoading={restartMutation.isPending}
                        startContent={
                          !restartMutation.isPending && (
                            <IconPlayerPlayFilled size={14} />
                          )
                        }
                        onPress={start}
                      >
                        Start
                      </Button>
                    </Tooltip>
                    <Tooltip content="Remove machine" placement="bottom">
                      <Button
                        isIconOnly
                        size="sm"
                        color="danger"
                        variant="flat"
                        onPress={() => setConfirmRemove(true)}
                      >
                        <IconTrash size={16} />
                      </Button>
                    </Tooltip>
                  </>
                )}
              </div>
              <Tabs
                aria-label="Detail"
                color="primary"
                variant="underlined"
                selectedKey={tab}
                onSelectionChange={(k) => setTab(k as string)}
                classNames={{ tabList: "gap-5 p-0", cursor: "bg-primary" }}
              >
                <Tab
                  key="logs"
                  title={
                    <span className="flex items-center gap-1.5">
                      <IconFileText size={15} /> Logs
                    </span>
                  }
                />
                <Tab
                  key="terminal"
                  isDisabled={!m.running}
                  title={
                    <span className="flex items-center gap-1.5">
                      <IconTerminal2 size={15} /> Terminal
                    </span>
                  }
                />
                <Tab
                  key="setup"
                  title={
                    <span className="flex items-center gap-1.5">
                      <IconSettingsBolt size={15} /> Setup
                    </span>
                  }
                />
                <Tab
                  key="inspect"
                  title={
                    <span className="flex items-center gap-1.5">
                      <IconInfoCircle size={15} /> Inspect
                    </span>
                  }
                />
              </Tabs>
            </DrawerHeader>
            <DrawerBody className="p-0">
              <div className="h-full min-h-0">
                {tab === "terminal" && m.running && (
                  <div className="h-full overflow-hidden rounded-lg bg-[#0a0d13]">
                    <TerminalPane machineId={m.id} command={EMPTY} />
                  </div>
                )}
                {tab === "logs" && (
                  <div className="h-full overflow-hidden bg-[#0a0d13]">
                    <LogsPane machineId={m.id} />
                  </div>
                )}
                {tab === "setup" &&
                  (m.running ? (
                    <div className="h-full overflow-auto">
                      <SetupPane machineId={m.id} />
                    </div>
                  ) : (
                    <div className="grid h-full place-items-center px-6 text-center text-sm text-foreground-500">
                      Start the machine to set up SSH or Tailscale — these run
                      inside the guest via its agent.
                    </div>
                  ))}
                {tab === "inspect" && (
                  <div className="h-full overflow-auto p-5">
                    <Inspect m={m} />
                  </div>
                )}
              </div>
            </DrawerBody>
          </>
        )}
      </DrawerContent>
    </Drawer>

    <ConfirmDialog
      open={confirmRemove}
      title="Remove machine?"
      body={
        <>
          This deletes machine{" "}
          <span className="font-mono text-foreground-300">
            {m && shortId(m.id)}
          </span>{" "}
          and its state. This cannot be undone.
        </>
      }
      confirmLabel="Remove"
      danger
      onConfirm={remove}
      onClose={() => setConfirmRemove(false)}
    />
    </>
  );
}
