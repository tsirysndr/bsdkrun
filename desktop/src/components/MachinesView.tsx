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
  IconCamera,
  IconGitBranch,
  IconTrash,
} from "@tabler/icons-react";
import {
  filterAtom,
  openTerminalAtom,
  runOpenAtom,
  branchTargetAtom,
  selectedMachineAtom,
  snapshotTargetAtom,
} from "../state/atoms";
import { ago, exitLabel, fullDate, isUnikraft, kindColor, shortId } from "../lib/format";
import {
  useMachines,
  useRemoveMachine,
  useRestartMachine,
  useStopMachine,
} from "../lib/queries";
import { useToast } from "../state/toast";
import { ConfirmDialog } from "./ConfirmDialog";
import { EmptyState, ViewShell } from "./ViewShell";
import { TableSkeleton } from "./Skeletons";
import { useInfiniteRows } from "../hooks/useInfiniteRows";
import { useListNavigation } from "../hooks/useListNavigation";
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
      <span className="whitespace-nowrap">{exitLabel(m.exit_code)}</span>
    </span>
  );
}

export default function MachinesView() {
  const { data: machines = [], isLoading, refetch } = useMachines();
  const filter = useAtomValue(filterAtom).toLowerCase();
  const setSelected = useSetAtom(selectedMachineAtom);
  const setSnapshotTarget = useSetAtom(snapshotTargetAtom);
  const setBranchTarget = useSetAtom(branchTargetAtom);
  const openTerminal = useSetAtom(openTerminalAtom);
  const setRunOpen = useSetAtom(runOpenAtom);
  const stopMutation = useStopMachine();
  const restartMutation = useRestartMachine();
  const removeMutation = useRemoveMachine();
  const [removeTarget, setRemoveTarget] = useState<Machine | null>(null);
  const toast = useToast();

  // Per-row in-flight op so we can spin only the clicked machine, tracking its
  // KIND ("start" | "stop") so we clear it on reaching the target state — not
  // the wrong one. Set on click, always cleared in the handler's `finally` too.
  const [pending, setPending] = useState<Map<string, "start" | "stop">>(
    new Map(),
  );
  const setRowBusy = (id: string, op: "start" | "stop" | null) =>
    setPending((s) => {
      const n = new Map(s);
      op ? n.set(id, op) : n.delete(id);
      return n;
    });

  // Belt-and-suspenders: clear a spinner as soon as the machine reaches its
  // target state, even if the mutation promise is slow to settle (a cold boot,
  // or a BSD clean-poweroff, can take a while). A "start" clears once the
  // machine is observed running; a "stop" once it's observed NOT running.
  // Crucially, a "stop" must NOT clear while the machine is still running —
  // otherwise the stop spinner vanishes the instant it's clicked.
  useEffect(() => {
    if (pending.size === 0) return;
    const runningIds = new Set(
      machines.filter((m) => m.running).map((m) => m.id),
    );
    setPending((s) => {
      const next = new Map(s);
      for (const [id, op] of s) {
        if (op === "start" && runningIds.has(id)) next.delete(id);
        if (op === "stop" && !runningIds.has(id)) next.delete(id);
      }
      return next.size === s.size ? s : next;
    });
  }, [machines, pending.size]);

  const rows = useMemo(() => {
    const r = machines.filter(
      (m) =>
        !filter ||
        (m.name || "").toLowerCase().includes(filter) ||
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

  // Keyboard navigation: ↑/↓ highlight a row, Enter/L opens it (Logs by
  // default), T opens a terminal on the highlighted running machine.
  const { focusedId } = useListNavigation(
    visibleRows,
    (m) => m.id,
    {
      onEnter: (m) => setSelected(m.id),
      keys: {
        l: (m) => setSelected(m.id),
        t: (m) => m.running && !isUnikraft(m.kind) && openTerminal(m.id),
      },
    },
  );

  const stop = async (m: Machine) => {
    setRowBusy(m.id, "stop");
    try {
      await stopMutation.mutateAsync(m.id);
      await refetch();
      toast("success", `Stopped ${shortId(m.id)}`);
    } catch (e) {
      toast("error", "Failed to stop machine", String(e));
    } finally {
      setRowBusy(m.id, null);
    }
  };

  // In-place restart: re-boots the SAME machine id (bsdkrun start <id>).
  const start = async (m: Machine) => {
    setRowBusy(m.id, "start");
    try {
      await restartMutation.mutateAsync(m.id);
      await refetch();
      toast("success", `Started ${shortId(m.id)}`);
    } catch (e) {
      toast("error", "Failed to start machine", String(e));
    } finally {
      setRowBusy(m.id, null);
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
          hint="Launch a Linux (OCI), FreeBSD, NetBSD or unikernel microVM to get started."
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
        {/* Static children (not the `items` render-prop): HeroUI memoizes the
            items collection, so external state like `pending` referenced in a
            render-prop wouldn't update the row until the data changed. Mapping
            rows directly re-renders them on every state change → instant spinner. */}
        <TableBody>
          {visibleRows.map((m) => {
            const kc = kindColor(m.kind, m.image);
            return (
              <TableRow
                key={m.id}
                data-list-row={m.id}
                className={
                  m.id === focusedId
                    ? "bg-primary/10 shadow-[inset_2px_0_0] shadow-primary"
                    : undefined
                }
              >
                <TableCell>
                  <button
                    className="flex max-w-[340px] items-center gap-3 text-left"
                    onClick={() => setSelected(m.id)}
                  >
                    <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${kc.dot}`} />
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium text-foreground group-hover/tr:text-primary-300">
                        {m.name || m.image || "(disk)"}
                      </span>
                      <span className="block truncate font-mono text-[11px] text-foreground-500">
                        {m.image ? `${m.image} · ` : ""}
                        {shortId(m.id)}
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
                        {/* A unikernel has no shell to attach to. */}
                        <Tooltip
                          content={
                            isUnikraft(m.kind)
                              ? "No shell — a unikernel has no userland"
                              : "Open terminal"
                          }
                          placement="top"
                        >
                          <div>
                            <Button
                              isIconOnly
                              size="sm"
                              variant="light"
                              isDisabled={isUnikraft(m.kind)}
                              onPress={() => openTerminal(m.id)}
                            >
                              <IconTerminal2 size={17} />
                            </Button>
                          </div>
                        </Tooltip>
                        <Tooltip content="Stop" placement="top">
                          <Button
                            isIconOnly
                            size="sm"
                            variant="light"
                            isLoading={pending.has(m.id)}
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
                          isLoading={pending.has(m.id)}
                          onPress={() => start(m)}
                        >
                          <IconPlayerPlayFilled size={15} />
                        </Button>
                      </Tooltip>
                    )}
                    <Tooltip
                      content="Branch — boot a copy of this machine"
                      placement="top"
                    >
                      <Button
                        isIconOnly
                        size="sm"
                        variant="light"
                        className="text-violet-300"
                        onPress={() =>
                          setBranchTarget({
                            snapshot: m.id,
                            label: m.name || m.image || shortId(m.id),
                            fromMachine: true,
                            kind: m.kind,
                            running: m.running,
                            ports: m.ports.map((p) => `${p.host}:${p.guest}`),
                          })
                        }
                      >
                        <IconGitBranch size={16} />
                      </Button>
                    </Tooltip>
                    <Tooltip content="Take a snapshot" placement="top">
                      <Button
                        isIconOnly
                        size="sm"
                        variant="light"
                        className="text-sky-300"
                        onPress={() =>
                          setSnapshotTarget({
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
          })}
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
            <span className="font-mono text-foreground-400">
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
