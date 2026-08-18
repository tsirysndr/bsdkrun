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
import { useAtomValue } from "jotai";
import { IconDatabase, IconTrash } from "@tabler/icons-react";
import { filterAtom } from "../state/atoms";
import { useRemoveVolume, useVolumes } from "../lib/queries";
import { ago, fullDate, kindColor } from "../lib/format";
import { useToast } from "../state/toast";
import { ConfirmDialog } from "./ConfirmDialog";
import { EmptyState, ViewShell } from "./ViewShell";
import { TableSkeleton } from "./Skeletons";
import { useInfiniteRows } from "../hooks/useInfiniteRows";
import { useListNavigation } from "../hooks/useListNavigation";
import type { Volume } from "../lib/types";

export default function VolumesView() {
  const { data: volumes = [], isLoading } = useVolumes();
  const filter = useAtomValue(filterAtom).toLowerCase();
  const removeMutation = useRemoveVolume();
  const [target, setTarget] = useState<Volume | null>(null);
  const toast = useToast();

  const rows = useMemo(
    () => volumes.filter((v) => !filter || v.name.toLowerCase().includes(filter)),
    [volumes, filter],
  );
  const { visible, sentinelRef, hasMore } = useInfiniteRows(rows.length);
  const visibleRows = useMemo(() => rows.slice(0, visible), [rows, visible]);

  // ↑/↓ highlight a volume; Enter or Delete opens the remove confirmation.
  const { focusedId } = useListNavigation(visibleRows, (v) => v.name, {
    onEnter: (v) => setTarget(v),
    keys: { d: (v) => setTarget(v) },
  });

  const remove = async () => {
    if (!target) return;
    try {
      await removeMutation.mutateAsync({ name: target.name, force: true });
      toast("success", `Removed volume ${target.name}`);
    } catch (e) {
      toast("error", "Failed to remove volume", String(e));
    } finally {
      setTarget(null);
    }
  };

  if (isLoading && volumes.length === 0) {
    return (
      <ViewShell title="Volumes" subtitle="Persistent copy-on-write guest disks">
        <TableSkeleton />
      </ViewShell>
    );
  }

  if (volumes.length === 0) {
    return (
      <ViewShell title="Volumes" subtitle="Persistent copy-on-write guest disks">
        <EmptyState
          icon={<IconDatabase size={28} />}
          title="No volumes yet"
          hint="Run a machine with a named volume (-v) to persist its rootfs across reboots."
        />
      </ViewShell>
    );
  }

  return (
    <ViewShell
      title="Volumes"
      subtitle={`${volumes.length} total`}
      searchPlaceholder="Filter volumes…"
    >
      <Table
        removeWrapper
        aria-label="Volumes"
        classNames={{
          th: "bg-transparent text-foreground-500 border-b border-white/10 text-[11px] uppercase tracking-wider",
          td: "py-3 first:rounded-l-lg last:rounded-r-lg",
          tr: "group/tr transition-colors hover:bg-white/[0.03]",
        }}
      >
        <TableHeader>
          <TableColumn>Name</TableColumn>
          <TableColumn>Guest</TableColumn>
          <TableColumn>Base</TableColumn>
          <TableColumn>Size</TableColumn>
          <TableColumn width={150}>Created</TableColumn>
          <TableColumn align="end"> </TableColumn>
        </TableHeader>
        <TableBody>
          {visibleRows.map((v) => {
            const kc = v.guest ? kindColor(v.guest, v.base) : null;
            return (
              <TableRow
                key={v.name}
                data-list-row={v.name}
                className={
                  v.name === focusedId
                    ? "bg-primary/10 shadow-[inset_2px_0_0] shadow-primary"
                    : undefined
                }
              >
                <TableCell>
                  <span className="text-sm font-medium text-foreground">
                    {v.name}
                  </span>
                  {!v.tracked && (
                    <span className="ml-2 rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] text-amber-300">
                      untracked
                    </span>
                  )}
                </TableCell>
                <TableCell>
                  {kc ? (
                    <span
                      className={`inline-flex items-center rounded-md border px-2 py-0.5 text-[11px] font-medium ${kc.className}`}
                    >
                      {kc.label}
                    </span>
                  ) : (
                    <span className="text-xs text-foreground-600">—</span>
                  )}
                </TableCell>
                <TableCell>
                  <span className="font-mono text-[11px] text-foreground-500">
                    {v.base || "—"}
                  </span>
                </TableCell>
                <TableCell>
                  <span className="text-xs text-foreground-400">
                    {v.size ?? "—"}
                  </span>
                </TableCell>
                <TableCell>
                  <Tooltip content={fullDate(v.created_at)} placement="top">
                    <span className="cursor-default whitespace-nowrap text-xs text-foreground-500">
                      {ago(v.created_at)}
                    </span>
                  </Tooltip>
                </TableCell>
                <TableCell>
                  <div className="flex justify-end">
                    <Tooltip content="Remove volume" placement="top">
                      <Button
                        isIconOnly
                        size="sm"
                        variant="light"
                        color="danger"
                        onPress={() => setTarget(v)}
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
        open={!!target}
        title="Remove volume?"
        body={
          <>
            This permanently deletes the data in volume{" "}
            <span className="font-mono text-foreground-400">{target?.name}</span>
            . This cannot be undone.
          </>
        }
        confirmLabel="Remove"
        danger
        onConfirm={remove}
        onClose={() => setTarget(null)}
      />
    </ViewShell>
  );
}
