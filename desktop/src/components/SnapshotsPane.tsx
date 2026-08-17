import { useState } from "react";
import { Button, Tooltip } from "@heroui/react";
import { useSetAtom } from "jotai";
import {
  IconCamera,
  IconGitBranch,
  IconHistory,
  IconTrash,
} from "@tabler/icons-react";
import { branchTargetAtom, snapshotTargetAtom } from "../state/atoms";
import { useRemoveSnapshot, useRestoreMachine, useSnapshots } from "../lib/queries";
import { ago, fullDate, shortId } from "../lib/format";
import { useToast } from "../state/toast";
import { ConfirmDialog } from "./ConfirmDialog";
import type { Machine, Snapshot } from "../lib/types";

/**
 * One machine's snapshots, inside its detail drawer: the timeline of states it
 * has been in, newest first, each one restorable or branchable.
 */
export default function SnapshotsPane({ machine }: { machine: Machine }) {
  const { data: snapshots = [], isLoading } = useSnapshots(machine.id);
  const setSnapshotTarget = useSetAtom(snapshotTargetAtom);
  const setBranchTarget = useSetAtom(branchTargetAtom);
  const removeMutation = useRemoveSnapshot();
  const restoreMutation = useRestoreMachine();
  const [toRestore, setToRestore] = useState<Snapshot | null>(null);
  const [toRemove, setToRemove] = useState<Snapshot | null>(null);
  const toast = useToast();

  const label = machine.name || machine.image || shortId(machine.id);

  const take = () =>
    setSnapshotTarget({
      id: machine.id,
      label,
      kind: machine.kind,
      running: machine.running,
    });

  const restore = async () => {
    if (!toRestore) return;
    const s = toRestore;
    setToRestore(null);
    try {
      await restoreMutation.mutateAsync({ id: machine.id, snapshot: s.name });
      toast(
        "success",
        `Restored to ${s.name}`,
        "The machine is stopped — start it to run the restored state.",
      );
    } catch (e) {
      toast("error", "Restore failed", String(e));
    }
  };

  const remove = async () => {
    if (!toRemove) return;
    const s = toRemove;
    setToRemove(null);
    try {
      await removeMutation.mutateAsync(s.name);
      toast("success", `Removed snapshot ${s.name}`);
    } catch (e) {
      toast("error", "Failed to remove snapshot", String(e));
    }
  };

  return (
    <div className="flex h-full flex-col gap-4 p-5">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold">Snapshots</h2>
          <p className="mt-0.5 text-xs text-foreground-500">
            Copy-on-write captures of this machine's disk. Branch one into a
            throwaway machine, or restore it to undo everything since.
          </p>
        </div>
        <Button
          size="sm"
          variant="flat"
          className="text-violet-300"
          startContent={<IconGitBranch size={15} />}
          onPress={() =>
            setBranchTarget({
              snapshot: machine.id,
              label,
              fromMachine: true,
              kind: machine.kind,
              running: machine.running,
              ports: machine.ports.map((p) => `${p.host}:${p.guest}`),
            })
          }
        >
          Branch
        </Button>
        <Button
          size="sm"
          color="primary"
          variant="flat"
          startContent={<IconCamera size={15} />}
          onPress={take}
        >
          Take snapshot
        </Button>
      </div>

      {isLoading && snapshots.length === 0 ? (
        <p className="text-xs text-foreground-500">Loading…</p>
      ) : snapshots.length === 0 ? (
        <div className="rounded-lg border border-dashed border-white/10 px-4 py-8 text-center">
          <p className="text-sm text-foreground-400">No snapshots of {label} yet</p>
          <p className="mt-1 text-xs text-foreground-600">
            Take one before an upgrade, a risky change, or a demo you want to repeat.
          </p>
        </div>
      ) : (
        <ul className="flex flex-col gap-2">
          {snapshots.map((s) => (
            <li
              key={s.id}
              className="flex items-center gap-3 rounded-lg border border-white/10 bg-content2/30 px-3 py-2.5"
            >
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium text-foreground">
                  {s.name}
                </div>
                <div className="truncate text-[11px] text-foreground-500">
                  <Tooltip content={fullDate(s.created_at)} placement="top">
                    <span className="cursor-default">{ago(s.created_at)}</span>
                  </Tooltip>
                  {s.description ? ` · ${s.description}` : ""}
                </div>
              </div>
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
                      ports: s.ports.map((p) => `${p.host}:${p.guest}`),
                    })
                  }
                >
                  <IconGitBranch size={16} />
                </Button>
              </Tooltip>
              <Tooltip content="Restore this machine to this state" placement="top">
                <Button
                  isIconOnly
                  size="sm"
                  variant="light"
                  className="text-amber-300"
                  onPress={() => setToRestore(s)}
                >
                  <IconHistory size={16} />
                </Button>
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
            </li>
          ))}
        </ul>
      )}

      <ConfirmDialog
        open={!!toRestore}
        title="Restore this snapshot?"
        body={
          <>
            <span className="font-mono text-foreground-300">{label}</span> goes back to
            the state saved in{" "}
            <span className="font-mono text-foreground-300">{toRestore?.name}</span>
            {machine.running ? ". It is stopped first" : ""}. Whatever it holds now is
            saved as a new snapshot, so this is undoable.
          </>
        }
        confirmLabel="Restore"
        onConfirm={restore}
        onClose={() => setToRestore(null)}
      />

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
    </div>
  );
}
