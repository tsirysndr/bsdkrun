import { useMemo, useState } from "react";
import { Button, Chip, Tooltip } from "@heroui/react";
import { useAtomValue } from "jotai";
import {
  IconApps,
  IconBox,
  IconBrandDocker,
  IconBrandGithubCopilot,
  IconBrandGolang,
  IconBrandLaravel,
  IconBrandMysql,
  IconBrandNodejs,
  IconBrandOpenai,
  IconBrandPhp,
  IconBrandPython,
  IconCamera,
  IconCode,
  IconDatabase,
  IconFlame,
  IconHexagon,
  IconPackage,
  IconPlayerPlayFilled,
  IconPlus,
  IconRobot,
  IconServer,
  IconSnowflake,
  IconSparkles,
  IconTerminal,
  IconTerminal2,
  IconTrash,
  IconWorldWww,
} from "@tabler/icons-react";
import { useSetAtom } from "jotai";
import { filterAtom, launchStateAtom, newFlavorOpenAtom } from "../state/atoms";
import { useFlavors, useRemoveFlavor } from "../lib/queries";
import { useLaunchFlavor } from "../hooks/useLaunchFlavor";
import { useToast } from "../state/toast";
import { ago, fullDate } from "../lib/format";
import { EmptyState, ViewShell } from "./ViewShell";
import { CardGridSkeleton } from "./Skeletons";
import { ConfirmDialog } from "./ConfirmDialog";
import type { Flavor, FlavorMethod } from "../lib/types";

type TablerIcon = typeof IconCode;

// ---- category + method presentation ---------------------------------------

interface CatMeta {
  label: string;
  icon: TablerIcon;
  /** Tailwind classes for the icon tile (bg + text). */
  tile: string;
}

const CATEGORY: Record<string, CatMeta> = {
  language: { label: "Languages", icon: IconCode, tile: "bg-indigo-500/15 text-indigo-300" },
  ai: { label: "AI agents", icon: IconSparkles, tile: "bg-fuchsia-500/15 text-fuchsia-300" },
  runtime: { label: "Runtimes", icon: IconBox, tile: "bg-cyan-500/15 text-cyan-300" },
  service: { label: "Services", icon: IconDatabase, tile: "bg-emerald-500/15 text-emerald-300" },
  web: { label: "Web servers", icon: IconWorldWww, tile: "bg-teal-500/15 text-teal-300" },
  os: { label: "Operating systems", icon: IconTerminal2, tile: "bg-amber-500/15 text-amber-300" },
  snapshot: { label: "Your snapshots", icon: IconCamera, tile: "bg-rose-500/15 text-rose-300" },
  custom: { label: "Your flavors", icon: IconApps, tile: "bg-slate-400/15 text-slate-300" },
};

const CATEGORY_ORDER = [
  "snapshot",
  "custom",
  "language",
  "ai",
  "runtime",
  "service",
  "web",
  "os",
];

function catMeta(cat: string): CatMeta {
  return CATEGORY[cat] ?? CATEGORY.custom;
}

interface MethodMeta {
  label: string;
  icon: TablerIcon;
  className: string;
}

const METHOD: Record<FlavorMethod, MethodMeta> = {
  docker: {
    label: "OCI image",
    icon: IconBrandDocker,
    className: "bg-sky-500/15 text-sky-300 border-sky-500/25",
  },
  nix: {
    label: "Nix",
    icon: IconSnowflake,
    className: "bg-violet-500/15 text-violet-300 border-violet-500/25",
  },
  system: {
    label: "System pkgs",
    icon: IconPackage,
    className: "bg-emerald-500/15 text-emerald-300 border-emerald-500/25",
  },
  snapshot: {
    label: "Snapshot",
    icon: IconCamera,
    className: "bg-rose-500/15 text-rose-300 border-rose-500/25",
  },
};

// Per-flavor brand icon (falls back to the category icon).
const FLAVOR_ICON: Record<string, TablerIcon> = {
  node: IconBrandNodejs,
  python: IconBrandPython,
  php: IconBrandPhp,
  laravel: IconBrandLaravel,
  symfony: IconBrandPhp,
  elixir: IconFlame,
  phoenix: IconFlame,
  gleam: IconSparkles,
  clojure: IconHexagon,
  go: IconBrandGolang,
  "claude-code": IconSparkles,
  codex: IconBrandOpenai,
  opencode: IconTerminal,
  crush: IconRobot,
  copilot: IconBrandGithubCopilot,
  postgres: IconDatabase,
  mysql: IconBrandMysql,
  redis: IconDatabase,
  nginx: IconServer,
  apache: IconServer,
  caddy: IconWorldWww,
  nix: IconSnowflake,
  docker: IconBrandDocker,
  freebsd: IconTerminal2,
  netbsd: IconTerminal2,
};

function flavorIcon(f: Flavor): TablerIcon {
  return FLAVOR_ICON[f.name] ?? catMeta(f.category).icon;
}

/** A themed pill for the guest kind. */
function kindPill(kind: string): { label: string; className: string } {
  if (kind === "freebsd") return { label: "FreeBSD", className: "text-red-300" };
  if (kind === "netbsd") return { label: "NetBSD", className: "text-orange-300" };
  return { label: "Linux", className: "text-sky-300" };
}

// ---- card ------------------------------------------------------------------

function FlavorCard({
  flavor,
  launching,
  onLaunch,
  onDelete,
}: {
  flavor: Flavor;
  launching: boolean;
  onLaunch: (f: Flavor) => void;
  onDelete: (f: Flavor) => void;
}) {
  const Icon = flavorIcon(flavor);
  const tile = catMeta(flavor.category).tile;
  const method = METHOD[flavor.method] ?? METHOD.docker;
  const MethodIcon = method.icon;
  const kind = kindPill(flavor.kind);
  const removable = flavor.source === "snapshot" || flavor.source === "user";

  return (
    <div className="group/card relative flex flex-col rounded-xl border border-white/10 bg-content1/50 p-4 transition hover:border-white/20 hover:bg-content1/80 hover:shadow-lg hover:shadow-black/20">
      <div className="flex items-start gap-3">
        <div className={`grid h-11 w-11 shrink-0 place-items-center rounded-lg ${tile}`}>
          <Icon size={22} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <h3 className="truncate text-sm font-semibold text-foreground">
              {flavor.name}
            </h3>
            <span className={`text-[10px] font-medium ${kind.className}`}>
              {kind.label}
            </span>
          </div>
          <p className="mt-0.5 line-clamp-2 text-xs leading-relaxed text-foreground-500">
            {flavor.description || flavor.base}
          </p>
        </div>
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        <span
          className={`inline-flex items-center gap-1 rounded-md border px-1.5 py-0.5 text-[10px] font-medium ${method.className}`}
        >
          <MethodIcon size={11} />
          {method.label}
        </span>
        {flavor.ports.slice(0, 3).map((p) => (
          <span
            key={p}
            className="rounded-md bg-white/5 px-1.5 py-0.5 font-mono text-[10px] text-foreground-400"
          >
            {p}
          </span>
        ))}
        {flavor.nix.length > 0 && (
          <span className="rounded-md bg-white/5 px-1.5 py-0.5 text-[10px] text-foreground-400">
            {flavor.nix.length} nix pkg{flavor.nix.length > 1 ? "s" : ""}
          </span>
        )}
      </div>

      <div className="mt-3 flex items-center gap-2 border-t border-white/5 pt-3">
        <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-foreground-600">
          {flavor.source === "snapshot" && flavor.created_at ? (
            <Tooltip content={fullDate(flavor.created_at)} placement="top">
              <span className="cursor-default">saved {ago(flavor.created_at)}</span>
            </Tooltip>
          ) : (
            flavor.base
          )}
        </span>
        {removable && (
          <Tooltip
            content={flavor.source === "snapshot" ? "Delete snapshot" : "Delete flavor"}
            placement="top"
          >
            <Button
              isIconOnly
              size="sm"
              variant="light"
              className="h-7 w-7 min-w-7 text-foreground-500 opacity-0 transition group-hover/card:opacity-100 hover:text-danger"
              onPress={() => onDelete(flavor)}
            >
              <IconTrash size={14} />
            </Button>
          </Tooltip>
        )}
        <Button
          size="sm"
          color="primary"
          variant="flat"
          className="h-7"
          isLoading={launching}
          startContent={!launching && <IconPlayerPlayFilled size={13} />}
          onPress={() => onLaunch(flavor)}
        >
          {launching ? "Starting" : "Launch"}
        </Button>
      </div>
    </div>
  );
}

// ---- view ------------------------------------------------------------------

export default function FlavorsView() {
  const { data: flavors = [], isLoading } = useFlavors();
  const filter = useAtomValue(filterAtom).toLowerCase();
  const toast = useToast();
  const launchFlavor = useLaunchFlavor();
  const launch = useAtomValue(launchStateAtom);
  const removeFlavor = useRemoveFlavor();
  const openNewFlavor = useSetAtom(newFlavorOpenAtom);

  const [pendingDelete, setPendingDelete] = useState<Flavor | null>(null);
  // A card shows a spinner while its flavor is the one actively launching.
  const launchingName =
    launch?.status === "running" ? launch.name : undefined;

  const rows = useMemo(
    () =>
      flavors.filter(
        (f) =>
          !filter ||
          f.name.toLowerCase().includes(filter) ||
          f.description.toLowerCase().includes(filter) ||
          f.category.toLowerCase().includes(filter) ||
          f.base.toLowerCase().includes(filter),
      ),
    [flavors, filter],
  );

  // Group into ordered category sections.
  const groups = useMemo(() => {
    const by = new Map<string, Flavor[]>();
    for (const f of rows) {
      const key = CATEGORY[f.category] ? f.category : "custom";
      (by.get(key) ?? by.set(key, []).get(key)!).push(f);
    }
    const known = CATEGORY_ORDER.filter((c) => by.has(c));
    const extra = [...by.keys()].filter((c) => !CATEGORY_ORDER.includes(c));
    return [...known, ...extra].map((c) => [c, by.get(c)!] as const);
  }, [rows]);

  const onLaunch = (f: Flavor) => {
    // Streams progress in the launch modal (pull → build → boot).
    launchFlavor(f.name);
  };

  const confirmDelete = () => {
    const f = pendingDelete;
    if (!f) return;
    return removeFlavor
      .mutateAsync({ name: f.name, force: true })
      .then(() => toast("success", `Removed ${f.name}`))
      .catch((e) => toast("error", `Couldn't remove ${f.name}`, String(e)))
      .finally(() => setPendingDelete(null));
  };

  if (isLoading && flavors.length === 0) {
    return (
      <ViewShell title="Flavors" subtitle="Preconfigured environments & snapshots">
        <CardGridSkeleton />
      </ViewShell>
    );
  }

  if (flavors.length === 0) {
    return (
      <ViewShell title="Flavors" subtitle="Preconfigured environments & snapshots">
        <EmptyState
          icon={<IconApps size={28} />}
          title="No flavors found"
          hint="The bsdkrun catalog couldn't be read. Check the binary path in Settings — or define your own flavor."
          action={
            <Button
              color="primary"
              variant="flat"
              startContent={<IconPlus size={16} />}
              onPress={() => openNewFlavor(true)}
            >
              New Flavor
            </Button>
          }
        />
      </ViewShell>
    );
  }

  return (
    <ViewShell
      title="Flavors"
      subtitle={`${flavors.length} environments — one-click dev stacks, services & AI agents`}
      searchPlaceholder="Filter flavors…"
      actions={
        <Button
          radius="md"
          startContent={<IconPlus size={16} />}
          onPress={() => openNewFlavor(true)}
          className="border border-violet-400/40 bg-gradient-to-r from-violet-600 to-fuchsia-500 font-medium text-white shadow-lg shadow-violet-500/30 transition hover:from-violet-500 hover:to-fuchsia-400 hover:shadow-violet-500/50"
        >
          New Flavor
        </Button>
      }
    >
      {rows.length === 0 ? (
        <div className="grid h-40 place-items-center text-sm text-foreground-500">
          No flavors match “{filter}”.
        </div>
      ) : (
        <div className="flex flex-col gap-7 pb-4">
          {groups.map(([cat, items]) => {
            const meta = catMeta(cat);
            const CatIcon = meta.icon;
            return (
              <section key={cat}>
                <div className="mb-3 flex items-center gap-2">
                  <CatIcon size={16} className="text-foreground-400" />
                  <h2 className="text-xs font-semibold uppercase tracking-wider text-foreground-400">
                    {meta.label}
                  </h2>
                  <Chip size="sm" variant="flat" className="h-5 bg-white/5 text-foreground-500">
                    {items.length}
                  </Chip>
                </div>
                <div className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] gap-3">
                  {items.map((f) => (
                    <FlavorCard
                      key={`${f.source}:${f.name}`}
                      flavor={f}
                      launching={launchingName === f.name}
                      onLaunch={onLaunch}
                      onDelete={setPendingDelete}
                    />
                  ))}
                </div>
              </section>
            );
          })}
        </div>
      )}

      <ConfirmDialog
        open={pendingDelete !== null}
        title={pendingDelete?.source === "user" ? "Delete flavor" : "Delete snapshot"}
        danger
        confirmLabel="Delete"
        body={
          pendingDelete?.source === "user" ? (
            <>
              Remove the flavor{" "}
              <span className="font-medium text-foreground">{pendingDelete?.name}</span>{" "}
              from your flavors.toml? This can't be undone.
            </>
          ) : (
            <>
              Remove the snapshot{" "}
              <span className="font-medium text-foreground">{pendingDelete?.name}</span>{" "}
              and its cloned data? This can't be undone.
            </>
          )
        }
        onConfirm={confirmDelete}
        onClose={() => setPendingDelete(null)}
      />
    </ViewShell>
  );
}
