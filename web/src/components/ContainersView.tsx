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
import {
  IconBrandDocker,
  IconPlayerPlayFilled,
  IconPlayerStopFilled,
  IconRefresh,
  IconFileText,
  IconTrash,
} from "@tabler/icons-react";
import { filterAtom } from "../state/atoms";
import {
  useDockerContainerAction,
  useDockerContainers,
  useDockerStart,
  useDockerStatus,
  useDockerStop,
} from "../lib/queries";
import { ago, humanSize } from "../lib/format";
import { useToast } from "../state/toast";
import { ConfirmDialog } from "./ConfirmDialog";
import { EmptyState, ViewShell } from "./ViewShell";
import { TableSkeleton } from "./Skeletons";
import { useInfiniteRows } from "../hooks/useInfiniteRows";
import ContainerLogsModal from "./ContainerLogsModal";
import type { DockerContainer, DockerStatus } from "../lib/types";

/**
 * Docker containers, running in bsdkrun's engine VM.
 *
 * The engine is one `docker:dind` microVM whose API is served on a host unix
 * socket, so these are the same containers the host's `docker ps` shows — this
 * view drives them, and the header says how to reach them from a terminal.
 */
export default function ContainersView() {
  const { data: status, isLoading: statusLoading } = useDockerStatus();
  const running = !!status?.running;
  const { data: containers = [], isLoading } = useDockerContainers(true, running);
  const filter = useAtomValue(filterAtom).toLowerCase();
  const action = useDockerContainerAction();
  const [toRemove, setToRemove] = useState<DockerContainer | null>(null);
  const [logsFor, setLogsFor] = useState<DockerContainer | null>(null);
  const [pending, setPending] = useState<Set<string>>(new Set());
  const toast = useToast();

  const rows = useMemo(
    () =>
      containers.filter(
        (c) =>
          !filter ||
          c.name.toLowerCase().includes(filter) ||
          c.image.toLowerCase().includes(filter) ||
          c.id.toLowerCase().includes(filter),
      ),
    [containers, filter],
  );
  const { visible, sentinelRef, hasMore } = useInfiniteRows(rows.length);
  const visibleRows = useMemo(() => rows.slice(0, visible), [rows, visible]);

  const run = async (c: DockerContainer, verb: string, label: string) => {
    setPending((p) => new Set(p).add(c.id));
    try {
      await action.mutateAsync({ action: verb, id: c.id });
      toast("success", `${label} ${c.name || c.id}`);
    } catch (e) {
      toast("error", `Failed to ${verb} ${c.name || c.id}`, String(e));
    } finally {
      setPending((p) => {
        const next = new Set(p);
        next.delete(c.id);
        return next;
      });
    }
  };

  const remove = async () => {
    if (!toRemove) return;
    const c = toRemove;
    setToRemove(null);
    await run(c, "rm", "Removed");
  };

  if (statusLoading && !status) {
    return (
      <ViewShell title="Containers" subtitle="Docker, in a bsdkrun microVM">
        <TableSkeleton />
      </ViewShell>
    );
  }

  // The engine has to be up before there is anything to list — and starting it
  // is the only useful thing this view can offer until then.
  if (!running) {
    return (
      <ViewShell title="Containers" subtitle="Docker, in a bsdkrun microVM">
        <EngineOffline status={status} />
      </ViewShell>
    );
  }

  return (
    <ViewShell
      title="Containers"
      subtitle={`${containers.filter((c) => c.state === "running").length} running · ${containers.length} total`}
      searchPlaceholder="Filter containers…"
      actions={<EngineBadge status={status} />}
    >
      {isLoading && containers.length === 0 ? (
        <TableSkeleton />
      ) : containers.length === 0 ? (
        <EmptyState
          icon={<IconBrandDocker size={28} />}
          title="No containers yet"
          hint={`The engine is running. Start one with \`docker run\` — the CLI is already pointed at it.`}
        />
      ) : (
        <Table
          removeWrapper
          aria-label="Containers"
          classNames={{
            th: "bg-transparent text-foreground-500 border-b border-white/10 text-[11px] uppercase tracking-wider",
            td: "py-3 first:rounded-l-lg last:rounded-r-lg",
            tr: "group/tr transition-colors hover:bg-white/[0.03]",
          }}
        >
          <TableHeader>
            <TableColumn>Name</TableColumn>
            <TableColumn>Image</TableColumn>
            <TableColumn>Status</TableColumn>
            <TableColumn>Ports</TableColumn>
            <TableColumn width={130}>Created</TableColumn>
            <TableColumn align="end"> </TableColumn>
          </TableHeader>
          <TableBody>
            {visibleRows.map((c) => {
              const up = c.state === "running";
              const busy = pending.has(c.id);
              return (
                <TableRow key={c.id}>
                  <TableCell>
                    <div className="flex flex-col">
                      <span className="text-sm font-medium text-foreground">
                        {c.name || "—"}
                      </span>
                      <span className="font-mono text-[10px] text-foreground-600">
                        {c.id}
                      </span>
                    </div>
                  </TableCell>
                  <TableCell>
                    <span className="font-mono text-[11px] text-foreground-400">
                      {clip(c.image, 28)}
                    </span>
                  </TableCell>
                  <TableCell>
                    <span
                      className={`inline-flex items-center gap-1.5 text-xs font-medium ${
                        up ? "text-emerald-300" : "text-foreground-500"
                      }`}
                    >
                      <span
                        className={`h-2 w-2 shrink-0 rounded-full ${
                          up ? "bg-emerald-400" : "bg-foreground-600"
                        }`}
                      />
                      {c.status || c.state}
                    </span>
                  </TableCell>
                  <TableCell>
                    {c.ports.length > 0 ? (
                      <div className="flex flex-wrap gap-1">
                        {c.ports.map((p) => (
                          <PortChip key={p} spec={p} />
                        ))}
                      </div>
                    ) : (
                      <span className="text-xs text-foreground-600">—</span>
                    )}
                  </TableCell>
                  <TableCell>
                    <span className="whitespace-nowrap text-xs text-foreground-500">
                      {c.created ? ago(String(c.created)) : "—"}
                    </span>
                  </TableCell>
                  <TableCell>
                    <div className="flex justify-end gap-1">
                      <Tooltip content="Logs" placement="top">
                        <Button
                          isIconOnly
                          size="sm"
                          variant="light"
                          onPress={() => setLogsFor(c)}
                        >
                          <IconFileText size={16} />
                        </Button>
                      </Tooltip>
                      {up ? (
                        <>
                          <Tooltip content="Restart" placement="top">
                            <Button
                              isIconOnly
                              size="sm"
                              variant="light"
                              isLoading={busy}
                              onPress={() => run(c, "restart", "Restarted")}
                            >
                              <IconRefresh size={16} />
                            </Button>
                          </Tooltip>
                          <Tooltip content="Stop" placement="top">
                            <Button
                              isIconOnly
                              size="sm"
                              variant="light"
                              isLoading={busy}
                              onPress={() => run(c, "stop", "Stopped")}
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
                            isLoading={busy}
                            onPress={() => run(c, "start", "Started")}
                          >
                            <IconPlayerPlayFilled size={15} />
                          </Button>
                        </Tooltip>
                      )}
                      <Tooltip content="Remove" placement="top">
                        <Button
                          isIconOnly
                          size="sm"
                          variant="light"
                          color="danger"
                          onPress={() => setToRemove(c)}
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
      )}

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
        title="Remove container?"
        body={
          <>
            This removes{" "}
            <span className="font-mono text-foreground-300">
              {toRemove?.name || toRemove?.id}
            </span>{" "}
            and its writable layer. Volumes it declared go too.
          </>
        }
        confirmLabel="Remove"
        danger
        onConfirm={remove}
        onClose={() => setToRemove(null)}
      />

      <ContainerLogsModal
        container={logsFor}
        onClose={() => setLogsFor(null)}
      />
    </ViewShell>
  );
}

/** Trim a long image ref for the table, keeping the tag visible. */
function clip(s: string, n: number): string {
  return s.length <= n ? s : `${s.slice(0, n - 1)}…`;
}

/**
 * A published port, as a link. The whole point of the engine's port publisher
 * is that these are reachable from the host, so make them clickable.
 */
function PortChip({ spec }: { spec: string }) {
  // "8080:80/tcp" — the host port is what a browser can reach.
  const host = spec.split(":")[0];
  return (
    <a
      href={`http://localhost:${host}`}
      target="_blank"
      rel="noreferrer"
      className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-[10px] text-primary transition hover:bg-primary/15"
    >
      {spec}
    </a>
  );
}

/** Where the socket is, so a terminal can be pointed at the same engine. */
function EngineBadge({ status }: { status?: DockerStatus }) {
  const toast = useToast();
  if (!status) return null;
  const hint = status.context_active
    ? "docker context: bsdkrun (active)"
    : `export DOCKER_HOST=unix://${status.socket}`;
  return (
    <Tooltip content={hint} placement="bottom">
      <button
        onClick={() => {
          navigator.clipboard?.writeText(status.socket);
          toast("info", "Socket path copied");
        }}
        className="flex items-center gap-2 rounded-lg border border-white/10 bg-content2/40 px-2.5 py-1.5 text-[11px] text-foreground-400 transition hover:border-primary/30"
      >
        <IconBrandDocker size={14} className="text-sky-300" />
        <span>{status.version ? `Docker ${status.version}` : "engine"}</span>
        {status.disk_size ? (
          <span className="text-foreground-600">
            · {humanSize(status.disk_size)} store
          </span>
        ) : null}
      </button>
    </Tooltip>
  );
}

/** The engine is not up: explain, and offer the one action that helps. */
function EngineOffline({ status }: { status?: DockerStatus }) {
  const start = useDockerStart();
  const stop = useDockerStop();
  const toast = useToast();
  const exists = !!status?.machine_id;

  const go = async () => {
    try {
      const s = await start.mutateAsync({});
      toast(
        "success",
        "Docker engine ready",
        s.context_active
          ? "`docker ps` in a terminal talks to it too"
          : `DOCKER_HOST=unix://${s.socket}`,
      );
    } catch (e) {
      toast("error", "Could not start the Docker engine", String(e));
    }
  };

  return (
    <EmptyState
      icon={<IconBrandDocker size={28} />}
      title={exists ? "Docker engine is stopped" : "No Docker engine yet"}
      hint={
        exists
          ? "Its images and containers are still on disk — starting it brings them back."
          : "Runs Docker in a microVM and points your `docker` CLI at it. Images, compose and buildx all work as they do in Docker Desktop."
      }
      action={
        <div className="flex flex-col items-center gap-2">
          <Button
            color="primary"
            isLoading={start.isPending}
            startContent={!start.isPending && <IconPlayerPlayFilled size={15} />}
            onPress={go}
          >
            {start.isPending
              ? "Starting the engine…"
              : exists
                ? "Start engine"
                : "Start Docker engine"}
          </Button>
          {/* A VM that is up with a dead dockerd is the one case where
              stopping is the way forward, so offer it rather than leaving
              the user with only a button that will keep timing out. */}
          {status?.machine_running && (
            <Button
              size="sm"
              variant="light"
              isLoading={stop.isPending}
              onPress={() => stop.mutateAsync().catch(() => {})}
            >
              The VM is up but not answering — stop it
            </Button>
          )}
        </div>
      }
    />
  );
}
