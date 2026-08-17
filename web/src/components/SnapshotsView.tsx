import { useMemo, useState } from "react";
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
  IconCamera,
  IconGitBranch,
  IconHistory,
  IconTrash,
} from "@tabler/icons-react";
import { branchTargetAtom, filterAtom } from "../state/atoms";
import { useMachines, useRemoveSnapshot, useRestoreMachine, useSnapshots } from "../lib/queries";
import { ago, fullDate, kindColor, shortId } from "../lib/format";
import { useToast } from "../state/toast";
import { ConfirmDialog } from "./ConfirmDialog";
import { EmptyState, ViewShell } from "./ViewShell";
import { TableSkeleton } from "./Skeletons";
import { useInfiniteRows } from "../hooks/useInfiniteRows";
import { useListNavigation } from "../hooks/useListNavigation";
import type { Snapshot } from "../lib/types";

/**
 * Every saved snapshot on this host, with the two things you do with one:
 * **branch** it into a new machine, or **restore** it over the machine it came
 * from. Both are copy-on-write, so neither costs disk until something diverges.
 */
export default function SnapshotsView() {
  const { data: snapshots = [], isLoading } = useSnapshots();
  const { data: machines = [] } = useMachines();
  const filter = useAtomValue(filterAtom).toLowerCase();
  const setBranchTarget = useSetAtom(branchTargetAtom);
  const removeMutation = useRemoveSnapshot();
  const restoreMutation = useRestoreMachine();
  const [toRemove, setToRemove] = useState<Snapshot | null>(null);
  const [toRestore, setToRestore] = useState<Snapshot | null>(null);
  const toast = useToast();

  const rows = useMemo(
    () =>
      snapshots.filter(
        (s) =>
          !filter ||
          s.name.toLowerCase().includes(filter) ||
          s.machine_name.toLowerCase().includes(filter) ||
          s.description.toLowerCase().includes(filter),
      ),
    [snapshots, filter],
  );
  const { visible, sentinelRef, hasMore } = useInfiniteRows(rows.length);
  const visibleRows = useMemo(() => rows.slice(0, visible), [rows, visible]);

  // ↑/↓ highlights a snapshot; Enter branches it, `d` offers to delete it.
  const { focusedId } = useListNavigation(visibleRows, (s) => s.id, {
    onEnter: (s) =>
      setBranchTarget({
        snapshot: s.name,
        label: s.name,
        fromMachine: false,
        kind: s.kind,
        ports: portSpecs(s),
      }),
    keys: { d: (s) => setToRemove(s) },
  });

  /** Whether the machine a snapshot came from still exists. */
  const sourceOf = (s: Snapshot) => machines.find((m) => m.id === s.machine_id) || null;

  const remove = async () => {
    if (!toRemove) return;
    try {
      await removeMutation.mutateAsync(toRemove.name);
      toast("success", `Removed snapshot ${toRemove.name}`);
    } catch (e) {
      toast("error", "Failed to remove snapshot", String(e));
    } finally {
      setToRemove(null);
    }
  };

  const restore = async () => {
    if (!toRestore) return;
    const s = toRestore;
    setToRestore(null);
    try {
      await restoreMutation.mutateAsync({ id: s.machine_id, snapshot: s.name });
      toast(
        "success",
        `Restored ${s.machine_name || shortId(s.machine_id)} to ${s.name}`,
        "The machine is stopped — start it to run the restored state.",
      );
    } catch (e) {
      toast("error", "Restore failed", String(e));
    }
  };

  if (isLoading && snapshots.length === 0) {
    return (
      <ViewShell title="Snapshots" subtitle="Copy-on-write captures of a machine's disk">
        <TableSkeleton />
      </ViewShell>
    );
  }

  if (snapshots.length === 0) {
    return (
      <ViewShell title="Snapshots" subtitle="Copy-on-write captures of a machine's disk">
        <EmptyState
          icon={<IconCamera size={28} />}
          title="No snapshots yet"
          hint="Open a machine and take a snapshot — then branch it into a throwaway copy, or roll the machine back to it."
        />
      </ViewShell>
    );
  }

  return (
    <ViewShell
      title="Snapshots"
      subtitle={`${snapshots.length} total`}
      searchPlaceholder="Filter snapshots…"
    >
      <Table
        removeWrapper
        aria-label="Snapshots"
        classNames={{
          th: "bg-transparent text-foreground-500 border-b border-white/10 text-[11px] uppercase tracking-wider",
          td: "py-3 first:rounded-l-lg last:rounded-r-lg",
          tr: "group/tr transition-colors hover:bg-white/[0.03]",
        }}
      >
        <TableHeader>
          <TableColumn>Name</TableColumn>
          <TableColumn>Machine</TableColumn>
          <TableColumn>Guest</TableColumn>
          <TableColumn>Description</TableColumn>
          <TableColumn width={150}>Created</TableColumn>
          <TableColumn align="end"> </TableColumn>
        </TableHeader>
        <TableBody>
          {visibleRows.map((s) => {
            const kc = kindColor(s.kind, s.image);
            const source = sourceOf(s);
            return (
              <TableRow
                key={s.id}
                data-list-row={s.id}
                className={
                  s.id === focusedId
                    ? "bg-primary/10 shadow-[inset_2px_0_0] shadow-primary"
                    : undefined
                }
              >
                <TableCell>
                  <span className="text-sm font-medium text-foreground">{s.name}</span>
                  {s.parent && (
                    <Tooltip content={`Branched from ${s.parent}`} placement="top">
                      <span className="ml-2 rounded bg-violet-500/15 px-1.5 py-0.5 text-[10px] text-violet-300">
                        {s.parent}
                      </span>
                    </Tooltip>
                  )}
                </TableCell>
                <TableCell>
                  <span className="text-xs text-foreground-400">
                    {s.machine_name || shortId(s.machine_id)}
                  </span>
                  {!source && (
                    <Tooltip content="The machine it came from is gone — you can still branch it" placement="top">
                      <span className="ml-2 rounded bg-white/10 px-1.5 py-0.5 text-[10px] text-foreground-500">
                        removed
                      </span>
                    </Tooltip>
                  )}
                </TableCell>
                <TableCell>
                  <span
                    className={`inline-flex items-center rounded-md border px-2 py-0.5 text-[11px] font-medium ${kc.className}`}
                  >
                    {kc.label}
                  </span>
                </TableCell>
                <TableCell>
                  <span className="text-xs text-foreground-500">
                    {s.description || "—"}
                  </span>
                </TableCell>
                <TableCell>
                  <Tooltip content={fullDate(s.created_at)} placement="top">
                    <span className="cursor-default whitespace-nowrap text-xs text-foreground-500">
                      {ago(s.created_at)}
                    </span>
                  </Tooltip>
                </TableCell>
                <TableCell>
                  <div className="flex justify-end gap-1">
                    <Tooltip content="Boot a new machine from this snapshot" placement="top">
                      <Button
                        isIconOnly
                        size="sm"
                        variant="light"
                        className="text-violet-300"
                        onPress={() =>
                          setBranchTarget({
                            snapshot: s.name,
                            label: s.name,
                            fromMachine: false,
                            kind: s.kind,
                            ports: portSpecs(s),
                          })
                        }
                      >
                        <IconGitBranch size={16} />
                      </Button>
                    </Tooltip>
                    <Tooltip
                      content={
                        source
                          ? "Restore this state over the machine it came from"
                          : "That machine no longer exists"
                      }
                      placement="top"
                    >
                      <div>
                        <Button
                          isIconOnly
                          size="sm"
                          variant="light"
                          className="text-amber-300"
                          isDisabled={!source}
                          onPress={() => setToRestore(s)}
                        >
                          <IconHistory size={16} />
                        </Button>
                      </div>
                    </Tooltip>
                    <Tooltip content="Delete snapshot" placement="top">
                      <Button
                        isIconOnly
                        size="sm"
                        variant="light"
                        color="danger"
                        onPress={() => setToRemove(s)}
                      >
                        <IconTrash size={16} />
                      </Button>
                    </Tooltip>
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
        open={!!toRemove}
        title="Delete snapshot?"
        body={
          <>
            This deletes the saved state{" "}
            <span className="font-mono text-foreground-300">{toRemove?.name}</span>.
            Machines already branched from it are unaffected.
          </>
        }
        confirmLabel="Delete"
        danger
        onConfirm={remove}
        onClose={() => setToRemove(null)}
      />

      <ConfirmDialog
        open={!!toRestore}
        title="Restore this snapshot?"
        body={
          <>
            <span className="font-mono text-foreground-300">
              {toRestore?.machine_name || shortId(toRestore?.machine_id ?? "")}
            </span>{" "}
            goes back to the state saved in{" "}
            <span className="font-mono text-foreground-300">{toRestore?.name}</span>.
            It is stopped first, and whatever it holds now is saved as a new snapshot so
            this is undoable.
          </>
        }
        confirmLabel="Restore"
        onConfirm={restore}
        onClose={() => setToRestore(null)}
      />
    </ViewShell>
  );
}

/** The snapshot's recorded forwards as `HOST:GUEST` strings, for the branch form. */
function portSpecs(s: Snapshot): string[] {
  return s.ports.map((p) => `${p.host}:${p.guest}`);
}
