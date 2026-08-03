import { useEffect, useMemo, useState } from "react";
import {
  Button,
  Table,
  TableBody,
  TableCell,
  TableColumn,
  TableHeader,
  TableRow,
  Tooltip,
} from "@heroui/react";
import { useAtomValue, useSetAtom } from "jotai";
import {
  IconPlayerStopFilled,
  IconPlayerPlayFilled,
  IconTerminal2,
  IconFileText,
  IconServer2,
  IconCpu,
  IconChevronRight,
  IconTrash,
} from "@tabler/icons-react";
import {
  filterAtom,
  openTerminalAtom,
  runOpenAtom,
  selectedMachineAtom,
} from "../state/atoms";
import { ago, fullDate, kindColor, shortId } from "../lib/format";
import {
  useMachines,
  useRemoveMachine,
  useRunMachine,
  useStopMachine,
} from "../lib/queries";
import { specFromMachine } from "../lib/machine";
import { useToast } from "../state/toast";
import { ConfirmDialog } from "./ConfirmDialog";
import { EmptyState, ViewShell } from "./ViewShell";
import { TableSkeleton } from "./Skeletons";
import { useInfiniteRows } from "../hooks/useInfiniteRows";
import type { Machine } from "../lib/types";

function StatusPill({ m }: { m: Machine }) {
  if (m.running) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs font-medium text-emerald-300">
        <span className="relative flex h-2 w-2 shrink-0">
          <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400/70" />
          <span className="relative inline-flex h-2 w-2 rounded-full bg-emerald-400" />
        </span>
        Running
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-foreground-500">
      <span className="h-2 w-2 shrink-0 rounded-full bg-foreground-600" />
      <span className="whitespace-nowrap">
        Exited{m.exit_code != null ? ` (${m.exit_code})` : ""}
      </span>
    </span>
  );
}

export default function MachinesView() {
  const { data: machines = [], isLoading, refetch } = useMachines();
  const filter = useAtomValue(filterAtom).toLowerCase();
  const setSelected = useSetAtom(selectedMachineAtom);
  const openTerminal = useSetAtom(openTerminalAtom);
  const setRunOpen = useSetAtom(runOpenAtom);
  const stopMutation = useStopMachine();
  const runMutation = useRunMachine();
  const removeMutation = useRemoveMachine();
  const [removeTarget, setRemoveTarget] = useState<Machine | null>(null);
  const toast = useToast();

  // In-flight start/stop ops keyed by the clicked machine id. The spinner stays
  // up until the machine's state is *actually* confirmed by polling — not just
  // when the CLI command returns (stop signals async; start's id prints before
  // the guest has finished booting).
  type Pending = { type: "start" | "stop"; newId?: string; at: number };
  const [pending, setPending] = useState<Record<string, Pending>>({});
  const clearPending = (id: string) =>
    setPending((p) => {
      if (!p[id]) return p;
      const n = { ...p };
      delete n[id];
      return n;
    });

  // Clear a pending op once its target state is observed; toast on confirmation.
  useEffect(() => {
    const done: { id: string; type: "start" | "stop" }[] = [];
    for (const [id, op] of Object.entries(pending)) {
      const timedOut = Date.now() - op.at > 45000;
      if (op.type === "stop") {
        const mm = machines.find((x) => x.id === id);
        if (!mm || !mm.running || timedOut) done.push({ id, type: "stop" });
      } else if (op.newId) {
        const nm = machines.find((x) => x.id === op.newId);
        if ((nm && nm.running) || timedOut) done.push({ id, type: "start" });
      } else if (timedOut) {
        done.push({ id, type: "start" });
      }
    }
    if (done.length) {
      setPending((p) => {
        const n = { ...p };
        done.forEach((d) => delete n[d.id]);
        return n;
      });
      done.forEach((d) =>
        toast(
          "success",
          d.type === "stop" ? "Machine stopped" : "Machine started",
        ),
      );
    }
  }, [machines, pending, toast]);

  // While something is transitioning, poll faster so the spinner clears quickly.
  useEffect(() => {
    if (Object.keys(pending).length === 0) return;
    const t = setInterval(() => refetch(), 1200);
    return () => clearInterval(t);
  }, [pending, refetch]);

  const rows = useMemo(() => {
    const r = machines.filter(
      (m) =>
        !filter ||
        m.id.toLowerCase().includes(filter) ||
        m.image.toLowerCase().includes(filter) ||
        m.command.toLowerCase().includes(filter) ||
        m.kind.toLowerCase().includes(filter),
    );
    // Running first, then most recent.
    return r.sort(
      (a, b) =>
        Number(b.running) - Number(a.running) ||
        (b.created_at || "").localeCompare(a.created_at || ""),
    );
  }, [machines, filter]);

  const { visible, sentinelRef, hasMore } = useInfiniteRows(rows.length);
  const visibleRows = useMemo(() => rows.slice(0, visible), [rows, visible]);

  const stop = async (m: Machine) => {
    setPending((p) => ({ ...p, [m.id]: { type: "stop", at: Date.now() } }));
    try {
      await stopMutation.mutateAsync(m.id);
      refetch();
    } catch (e) {
      toast("error", "Failed to stop machine", String(e));
      clearPending(m.id);
    }
  };

  const start = async (m: Machine) => {
    setPending((p) => ({ ...p, [m.id]: { type: "start", at: Date.now() } }));
    try {
      const id = await runMutation.mutateAsync(specFromMachine(m));
      // Record the new machine id; the effect clears the spinner once it runs.
      setPending((p) =>
        p[m.id] ? { ...p, [m.id]: { ...p[m.id], newId: id } } : p,
      );
      refetch();
    } catch (e) {
      toast("error", "Failed to start machine", String(e));
      clearPending(m.id);
    }
  };

  const remove = async () => {
    if (!removeTarget) return;
    try {
      await removeMutation.mutateAsync({ id: removeTarget.id, force: false });
      toast("success", `Removed ${shortId(removeTarget.id)}`);
    } catch (e) {
      toast("error", "Failed to remove machine", String(e));
    } finally {
      setRemoveTarget(null);
    }
  };

  if (isLoading && machines.length === 0) {
    return (
      <ViewShell title="Machines" subtitle="Running and stopped microVMs">
        <TableSkeleton />
      </ViewShell>
    );
  }

  if (machines.length === 0) {
    return (
      <ViewShell title="Machines" subtitle="Running and stopped microVMs">
        <EmptyState
          icon={<IconServer2 size={28} />}
          title="No machines yet"
          hint="Launch a FreeBSD, NetBSD, or Linux (OCI) microVM to get started."
          action={
            <Button color="primary" variant="shadow" onPress={() => setRunOpen(true)}>
              Run a machine
            </Button>
          }
        />
      </ViewShell>
    );
  }

  return (
    <ViewShell
      title="Machines"
      subtitle={`${machines.filter((m) => m.running).length} running · ${machines.length} total`}
      searchPlaceholder="Filter machines…"
    >
      <Table
        removeWrapper
        aria-label="Machines"
        classNames={{
          th: "bg-transparent text-foreground-500 border-b border-white/10 text-[11px] uppercase tracking-wider",
          td: "py-3 first:rounded-l-lg last:rounded-r-lg",
          tr: "group/tr transition-colors hover:bg-white/[0.03]",
        }}
      >
        <TableHeader>
          <TableColumn>Machine</TableColumn>
          <TableColumn>Guest</TableColumn>
          <TableColumn width={120}>Status</TableColumn>
          <TableColumn>Resources</TableColumn>
          <TableColumn width={150}>Created</TableColumn>
          <TableColumn align="end"> </TableColumn>
        </TableHeader>
        <TableBody items={visibleRows}>
          {(m) => {
            const kc = kindColor(m.kind, m.image);
            return (
              <TableRow key={m.id}>
                <TableCell>
                  <button
                    className="flex max-w-[340px] items-center gap-3 text-left"
                    onClick={() => setSelected(m.id)}
                  >
                    <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${kc.dot}`} />
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium text-foreground group-hover/tr:text-primary-300">
                        {m.image || "(disk)"}
                      </span>
                      <span className="block truncate font-mono text-[11px] text-foreground-500">
                        {shortId(m.id)}
                        {m.command ? ` · ${m.command}` : ""}
                      </span>
                    </span>
                  </button>
                </TableCell>
                <TableCell>
                  <span
                    className={`inline-flex items-center rounded-md border px-2 py-0.5 text-[11px] font-medium ${kc.className}`}
                  >
                    {kc.label}
                  </span>
                </TableCell>
                <TableCell>
                  <div className="min-w-[120px]">
                    <StatusPill m={m} />
                  </div>
                </TableCell>
                <TableCell>
                  <span className="flex max-w-[150px] items-center gap-1.5 truncate text-xs text-foreground-400">
                    <IconCpu size={14} className="shrink-0" />
                    <span className="truncate">
                      {m.cpus ?? "?"} vCPU · {m.mem ?? "?"} MiB
                      {m.volume ? ` · vol:${m.volume}` : ""}
                    </span>
                  </span>
                </TableCell>
                <TableCell>
                  <Tooltip content={fullDate(m.created_at)} placement="top">
                    <span className="cursor-default whitespace-nowrap text-xs text-foreground-500">
                      {ago(m.created_at)}
                    </span>
                  </Tooltip>
                </TableCell>
                <TableCell>
                  <div className="flex items-center justify-end gap-1">
                    {m.running ? (
                      <>
                        <Tooltip content="Open terminal" placement="top">
                          <Button
                            isIconOnly
                            size="sm"
                            variant="light"
                            onPress={() => openTerminal(m.id)}
                          >
                            <IconTerminal2 size={17} />
                          </Button>
                        </Tooltip>
                        <Tooltip content="Stop" placement="top">
                          <Button
                            isIconOnly
                            size="sm"
                            variant="light"
                            isLoading={!!pending[m.id]}
                            onPress={() => stop(m)}
                          >
                            <IconPlayerStopFilled size={15} />
                          </Button>
                        </Tooltip>
                      </>
                    ) : (
                      <Tooltip content="Start" placement="top">
                        <Button
                          isIconOnly
                          size="sm"
                          variant="light"
                          isLoading={!!pending[m.id]}
                          onPress={() => start(m)}
                        >
                          <IconPlayerPlayFilled size={15} />
                        </Button>
                      </Tooltip>
                    )}
                    <Tooltip content="Logs & details" placement="top">
                      <Button
                        isIconOnly
                        size="sm"
                        variant="light"
                        onPress={() => setSelected(m.id)}
                      >
                        {m.running ? (
                          <IconChevronRight size={17} />
                        ) : (
                          <IconFileText size={16} />
                        )}
                      </Button>
                    </Tooltip>
                    {!m.running && (
                      <Tooltip content="Remove" placement="top">
                        <Button
                          isIconOnly
                          size="sm"
                          variant="light"
                          onPress={() => setRemoveTarget(m)}
                        >
                          <IconTrash size={16} />
                        </Button>
                      </Tooltip>
                    )}
                  </div>
                </TableCell>
              </TableRow>
            );
          }}
        </TableBody>
      </Table>

      {hasMore && (
        <div
          ref={sentinelRef}
          className="flex items-center justify-center py-4 text-xs text-foreground-500"
        >
          Loading more… ({visible} of {rows.length})
        </div>
      )}

      <ConfirmDialog
        open={!!removeTarget}
        title="Remove machine?"
        body={
          <>
            This deletes machine{" "}
            <span className="font-mono text-foreground-300">
              {removeTarget && shortId(removeTarget.id)}
            </span>{" "}
            and its state (console log, disk clone). This cannot be undone.
          </>
        }
        confirmLabel="Remove"
        danger
        onConfirm={remove}
        onClose={() => setRemoveTarget(null)}
      />
    </ViewShell>
  );
}
