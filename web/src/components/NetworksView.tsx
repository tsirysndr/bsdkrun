import { useMemo, useState } from "react";
import {
  Button,
  Chip,
  Drawer,
  DrawerBody,
  DrawerContent,
  DrawerHeader,
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
import { useSetAtom } from "jotai";
import {
  IconAddressBook,
  IconNetwork,
  IconPlus,
  IconSearch,
  IconServer,
  IconTrash,
} from "@tabler/icons-react";
import { ago, fullDate, kindColor, shortId } from "../lib/format";
import { EmptyState, ViewShell } from "./ViewShell";
import { CardGridSkeleton } from "./Skeletons";
import { ConfirmDialog } from "./ConfirmDialog";
import {
  useCreateNetwork,
  useMachines,
  useNetworks,
  useRemoveNetwork,
  useSyncNetwork,
} from "../lib/queries";
import { selectedMachineAtom } from "../state/atoms";
import { useToast } from "../state/toast";
import type { Machine, Network } from "../lib/types";
import ImageRef from "./ImageRef";

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

/** A single member row in the network drawer — opens the machine detail. */
function MemberRow({ m, onOpen }: { m: Machine; onOpen: () => void }) {
  const kc = kindColor(m.kind, m.image);
  return (
    <button
      onClick={onOpen}
      className="flex w-full items-center gap-3 rounded-lg border border-white/5 bg-content2/40 px-3 py-2.5 text-left transition hover:border-white/15 hover:bg-content2/80"
    >
      <span
        className={`h-2 w-2 shrink-0 rounded-full ${m.running ? "bg-emerald-400" : "bg-foreground-600"}`}
        title={m.running ? "running" : "stopped"}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-medium text-foreground">
            {m.name || shortId(m.id)}
          </span>
          <span
            className={`shrink-0 rounded border px-1.5 py-px text-[9px] font-medium uppercase ${kc.className}`}
          >
            {kc.label}
          </span>
        </div>
        <ImageRef value={m.image} className="font-mono text-[11px] text-foreground-500" />
      </div>
      <span className="shrink-0 font-mono text-xs text-cyan-300/90">
        {m.net_ip || "—"}
      </span>
    </button>
  );
}

/** Drawer listing a network's members, with a quick filter search. */
function NetworkMembersDrawer({
  network,
  onClose,
}: {
  network: Network | null;
  onClose: () => void;
}) {
  const { data: machines = [] } = useMachines();
  const setSelected = useSetAtom(selectedMachineAtom);
  const sync = useSyncNetwork();
  const toast = useToast();
  const [q, setQ] = useState("");

  const onSync = async () => {
    if (!network) return;
    try {
      await sync.mutateAsync(network.name);
      toast(
        "success",
        "Name resolution refreshed",
        `Members of ${network.name} can now resolve each other by name.`,
      );
    } catch (e) {
      toast("error", "Couldn't refresh name resolution", String(e));
    }
  };

  const members = useMemo(
    () =>
      machines
        .filter((m) => network && m.network === network.name)
        .sort((a, b) => (a.net_ip || "").localeCompare(b.net_ip || "", undefined, { numeric: true })),
    [machines, network],
  );

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return members;
    return members.filter((m) =>
      [m.name, m.id, m.net_ip, m.image].some((f) => f?.toLowerCase().includes(needle)),
    );
  }, [members, q]);

  const openMachine = (id: string) => {
    setSelected(id);
    onClose();
  };

  return (
    <Drawer
      isOpen={network !== null}
      onClose={onClose}
      size="md"
      backdrop="opaque"
      shouldBlockScroll={false}
      classNames={{
        base: "h-[100dvh] max-h-full border-l border-white/10 bg-content1",
        wrapper: "h-[100dvh]",
      }}
    >
      <DrawerContent>
        {network && (
          <>
            <DrawerHeader className="flex flex-col gap-3 border-b border-white/10">
              <div className="flex items-center gap-3 pr-8">
                <div className="grid h-10 w-10 shrink-0 place-items-center rounded-lg bg-cyan-500/15 text-cyan-300">
                  <IconNetwork size={20} />
                </div>
                <div className="min-w-0">
                  <h2 className="truncate text-base font-semibold text-foreground">
                    {network.name}
                  </h2>
                  <p className="font-mono text-[11px] text-foreground-500">
                    {network.subnet} · gw {network.gateway} ·{" "}
                    {members.length} member{members.length === 1 ? "" : "s"}
                  </p>
                </div>
                <Tooltip
                  content="Refresh name resolution (rewrites members' /etc/hosts)"
                  placement="bottom"
                >
                  <Button
                    size="sm"
                    variant="flat"
                    className="ml-auto shrink-0"
                    isLoading={sync.isPending}
                    startContent={
                      !sync.isPending && <IconAddressBook size={15} />
                    }
                    onPress={onSync}
                  >
                    Fix names
                  </Button>
                </Tooltip>
              </div>
              <Input
                size="sm"
                autoFocus
                value={q}
                onValueChange={setQ}
                placeholder="Filter members by name, IP, image…"
                variant="bordered"
                startContent={<IconSearch size={15} className="text-foreground-500" />}
                isClearable
                onClear={() => setQ("")}
                classNames={{ inputWrapper: "border-white/10" }}
              />
            </DrawerHeader>
            <DrawerBody className="gap-2 py-4">
              {members.length === 0 ? (
                <div className="mt-10 flex flex-col items-center gap-2 text-center text-foreground-500">
                  <IconServer size={26} />
                  <p className="text-sm">No machines on this network yet.</p>
                  <p className="text-xs">
                    Start a machine with this network, or attach one from its detail
                    panel.
                  </p>
                </div>
              ) : filtered.length === 0 ? (
                <p className="mt-8 text-center text-sm text-foreground-500">
                  No members match “{q}”.
                </p>
              ) : (
                filtered.map((m) => (
                  <MemberRow key={m.id} m={m} onOpen={() => openMachine(m.id)} />
                ))
              )}
            </DrawerBody>
          </>
        )}
      </DrawerContent>
    </Drawer>
  );
}

export default function NetworksView() {
  const { data: networks = [], isLoading } = useNetworks();
  const remove = useRemoveNetwork();
  const toast = useToast();
  const [createOpen, setCreateOpen] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<Network | null>(null);
  const [openNetwork, setOpenNetwork] = useState<Network | null>(null);

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
            role="button"
            tabIndex={0}
            onClick={() => setOpenNetwork(n)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                setOpenNetwork(n);
              }
            }}
            className="group/card flex cursor-pointer flex-col rounded-xl border border-white/10 bg-content1/50 p-4 outline-none transition hover:border-white/20 hover:bg-content1/80 focus-visible:border-white/30"
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
              <span onClick={(e) => e.stopPropagation()}>
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
              </span>
            </div>
          </div>
        ))}
      </div>

      <NewNetworkDialog open={createOpen} onClose={() => setCreateOpen(false)} />
      <NetworkMembersDrawer
        network={openNetwork}
        onClose={() => setOpenNetwork(null)}
      />
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
