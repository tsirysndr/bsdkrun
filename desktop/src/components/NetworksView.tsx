import { useState } from "react";
import {
  Button,
  Chip,
  Input,
  Modal,
  ModalBody,
  ModalContent,
  ModalFooter,
  ModalHeader,
  Tooltip,
} from "@heroui/react";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { IconNetwork, IconPlus, IconTrash } from "@tabler/icons-react";
import { ago, fullDate } from "../lib/format";
import { EmptyState, ViewShell } from "./ViewShell";
import { CardGridSkeleton } from "./Skeletons";
import { ConfirmDialog } from "./ConfirmDialog";
import {
  useCreateNetwork,
  useNetworks,
  useRemoveNetwork,
} from "../lib/queries";
import { useToast } from "../state/toast";
import type { Network } from "../lib/types";

const schema = z.object({
  name: z
    .string()
    .min(1, "A name is required")
    .max(40, "Too long")
    .regex(/^[a-zA-Z0-9._-]+$/, "Letters, digits, . _ - only"),
});
type FormValues = z.infer<typeof schema>;

function NewNetworkDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const create = useCreateNetwork();
  const toast = useToast();
  const {
    register,
    handleSubmit,
    reset,
    formState: { errors, isSubmitting },
  } = useForm<FormValues>({ resolver: zodResolver(schema), defaultValues: { name: "" } });

  const onSubmit = handleSubmit(async (v) => {
    try {
      await create.mutateAsync(v.name);
      toast("success", `Network “${v.name}” created`);
      reset({ name: "" });
      onClose();
    } catch (e) {
      toast("error", "Couldn't create network", String(e));
    }
  });

  return (
    <Modal
      isOpen={open}
      onClose={() => {
        if (!isSubmitting) onClose();
      }}
      size="sm"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{ base: "border border-white/10 bg-content1" }}
    >
      <ModalContent>
        <form onSubmit={onSubmit}>
          <ModalHeader className="flex items-center gap-2 text-base">
            <IconNetwork size={18} className="text-primary" />
            New network
          </ModalHeader>
          <ModalBody className="gap-3">
            <p className="text-xs text-foreground-500">
              Machines started with this network share a subnet and reach each
              other by IP and by name (its internal DNS).
            </p>
            <Input
              autoFocus
              size="sm"
              label="Network name"
              placeholder="devnet"
              variant="bordered"
              isInvalid={!!errors.name}
              errorMessage={errors.name?.message}
              classNames={{ inputWrapper: "border-white/10" }}
              {...register("name")}
            />
          </ModalBody>
          <ModalFooter>
            <Button variant="light" size="sm" isDisabled={isSubmitting} onPress={onClose}>
              Cancel
            </Button>
            <Button type="submit" size="sm" color="primary" isLoading={isSubmitting}>
              Create
            </Button>
          </ModalFooter>
        </form>
      </ModalContent>
    </Modal>
  );
}

export default function NetworksView() {
  const { data: networks = [], isLoading } = useNetworks();
  const remove = useRemoveNetwork();
  const toast = useToast();
  const [createOpen, setCreateOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<Network | null>(null);

  const newBtn = (
    <Button
      color="primary"
      variant="flat"
      startContent={<IconPlus size={16} />}
      onPress={() => setCreateOpen(true)}
    >
      New Network
    </Button>
  );

  const confirmDelete = () => {
    const n = pendingDelete;
    if (!n) return;
    return remove
      .mutateAsync({ name: n.name, force: true })
      .then(() => toast("success", `Removed ${n.name}`))
      .catch((e) => toast("error", `Couldn't remove ${n.name}`, String(e)))
      .finally(() => setPendingDelete(null));
  };

  if (isLoading && networks.length === 0) {
    return (
      <ViewShell title="Networks" subtitle="Shared subnets with internal DNS">
        <CardGridSkeleton cards={3} />
      </ViewShell>
    );
  }

  if (networks.length === 0) {
    return (
      <>
        <ViewShell title="Networks" subtitle="Shared subnets with internal DNS" actions={newBtn}>
          <EmptyState
            icon={<IconNetwork size={28} />}
            title="No networks yet"
            hint="Create a network, then start machines with it — they'll share a subnet and resolve each other by name (like docker compose)."
            action={newBtn}
          />
        </ViewShell>
        <NewNetworkDialog open={createOpen} onClose={() => setCreateOpen(false)} />
      </>
    );
  }

  return (
    <ViewShell
      title="Networks"
      subtitle={`${networks.length} network${networks.length > 1 ? "s" : ""} — shared subnet + internal DNS`}
      actions={newBtn}
    >
      <div className="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-3">
        {networks.map((n) => (
          <div
            key={n.name}
            className="group/card flex flex-col rounded-xl border border-white/10 bg-content1/50 p-4 transition hover:border-white/20 hover:bg-content1/80"
          >
            <div className="flex items-start gap-3">
              <div className="grid h-11 w-11 shrink-0 place-items-center rounded-lg bg-cyan-500/15 text-cyan-300">
                <IconNetwork size={22} />
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <h3 className="truncate text-sm font-semibold text-foreground">{n.name}</h3>
                  <span
                    className={`flex items-center gap-1 text-[10px] font-medium ${
                      n.up ? "text-emerald-300" : "text-foreground-500"
                    }`}
                  >
                    <span
                      className={`h-1.5 w-1.5 rounded-full ${n.up ? "bg-emerald-400" : "bg-foreground-600"}`}
                    />
                    {n.up ? "up" : "down"}
                  </span>
                </div>
                <p className="mt-0.5 font-mono text-[11px] text-foreground-500">
                  {n.subnet} · gw {n.gateway}
                </p>
              </div>
            </div>

            <div className="mt-3 flex items-center gap-2 border-t border-white/5 pt-3">
              <Chip size="sm" variant="flat" className="h-6 bg-white/5 text-foreground-400">
                {n.running} running / {n.members} member{n.members === 1 ? "" : "s"}
              </Chip>
              <span className="min-w-0 flex-1 truncate text-right font-mono text-[10px] text-foreground-600">
                {n.created_at ? (
                  <Tooltip content={fullDate(n.created_at)} placement="top">
                    <span className="cursor-default">created {ago(n.created_at)}</span>
                  </Tooltip>
                ) : null}
              </span>
              <Tooltip
                content={n.running > 0 ? "Has running members" : "Delete network"}
                placement="top"
              >
                <Button
                  isIconOnly
                  size="sm"
                  variant="light"
                  className="h-7 w-7 min-w-7 text-foreground-500 opacity-0 transition group-hover/card:opacity-100 hover:text-danger"
                  onPress={() => setPendingDelete(n)}
                >
                  <IconTrash size={14} />
                </Button>
              </Tooltip>
            </div>
          </div>
        ))}
      </div>

      <NewNetworkDialog open={createOpen} onClose={() => setCreateOpen(false)} />
      <ConfirmDialog
        open={pendingDelete !== null}
        title="Delete network"
        danger
        confirmLabel="Delete"
        body={
          <>
            Remove the network{" "}
            <span className="font-medium text-foreground">{pendingDelete?.name}</span>
            {pendingDelete && pendingDelete.running > 0 ? (
              <>
                {" "}— it has <b>{pendingDelete.running}</b> running member(s), which
                will lose the network on their next start.
              </>
            ) : (
              <>?</>
            )}
          </>
        }
        onConfirm={confirmDelete}
        onClose={() => setPendingDelete(null)}
      />
    </ViewShell>
  );
}
