import { useEffect, useMemo, useRef, useState } from "react";
import { Modal, ModalContent, Kbd } from "@heroui/react";
import { useAtom, useAtomValue, useSetAtom } from "jotai";
import {
  IconSearch,
  IconServer2,
  IconStack2,
  IconDatabase,
  IconApps,
  IconMoon,
  IconRocket,
  IconPlus,
  IconRefresh,
  IconSettings,
  IconKeyboard,
  IconPlayerStopFilled,
  IconPlayerPlayFilled,
  IconBrandDocker,
  IconCamera,
  IconSparkles,
  IconCpu,
  IconGitBranch,
  IconTerminal2,
  IconCornerDownLeft,
} from "@tabler/icons-react";
import {
  agentPanelOpenAtom,
branchTargetAtom,
ciRepoAtom,
ciReposAtom,
commitTargetAtom,
editResourcesAtom,
openTerminalAtom,
paletteOpenAtom,
runOpenAtom,
selectedMachineAtom,
settingsOpenAtom,
shortcutsOpenAtom,
snapshotTargetAtom,
themeAtom,
viewAtom,
} from "../state/atoms";
import {
  useAiAgents,
  useFlavors,
  useMachines,
  useRefreshAll,
  useRestartMachine,
  useSnapshots,
  useStopMachine,
} from "../lib/queries";
import { useLaunchFlavor } from "../hooks/useLaunchFlavor";
import { useAiSessions, useAttachAgentSession, useStartAgent } from "../lib/queries";
import { useToast } from "../state/toast";
import { isUnikraft, kindColor, shortId } from "../lib/format";
import type { ViewKey } from "../lib/types";

interface Command {
  id: string;
  title: string;
  subtitle?: string;
  section: string;
  icon: typeof IconSearch;
  keywords?: string;
  run: () => void;
  /** If set, ⌘↵ on this row opens an interactive terminal for this machine. */
  terminalFor?: string;
  /** Machine rows carry running state so the palette can show a status dot. */
  running?: boolean;
}

export default function CommandPalette() {
  const [open, setOpen] = useAtom(paletteOpenAtom);
  const { data: machines = [] } = useMachines();
  const { data: flavors = [] } = useFlavors();
  const { data: snapshots = [] } = useSnapshots();
  const { data: aiAgents = [] } = useAiAgents();
  const { data: aiSessions = [] } = useAiSessions();
  const startAgent = useStartAgent();
  const attachSession = useAttachAgentSession();
  const setAgentPanel = useSetAtom(agentPanelOpenAtom);
  const setView = useSetAtom(viewAtom);
  const [theme, setTheme] = useAtom(themeAtom);
  const ciRepos = useAtomValue(ciReposAtom);
  const setCiRepo = useSetAtom(ciRepoAtom);
  const setRunOpen = useSetAtom(runOpenAtom);
  const setSettingsOpen = useSetAtom(settingsOpenAtom);
  const setShortcutsOpen = useSetAtom(shortcutsOpenAtom);
  const setSelected = useSetAtom(selectedMachineAtom);
  const setCommitTarget = useSetAtom(commitTargetAtom);
  const setSnapshotTarget = useSetAtom(snapshotTargetAtom);
  const setBranchTarget = useSetAtom(branchTargetAtom);
  const setEditResources = useSetAtom(editResourcesAtom);
  const openTerm = useSetAtom(openTerminalAtom);
  const refreshAll = useRefreshAll();
  const stopMutation = useStopMachine();
  const restartMutation = useRestartMachine();
  const launchFlavor = useLaunchFlavor();
  const toast = useToast();

  const openTerminal = (id: string) => {
    openTerm(id);
    setOpen(false);
  };

  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const nav = (v: ViewKey) => () => {
    setView(v);
    setOpen(false);
  };

  const commands = useMemo<Command[]>(() => {
    const base: Command[] = [
      {
        id: "run",
        title: "Run new machine",
        subtitle: "Launch a Linux, BSD or unikernel microVM",
        section: "Actions",
        icon: IconPlus,
        keywords: "start create new docker run",
        run: () => {
          setOpen(false);
          setRunOpen(true);
        },
      },
      {
        id: "refresh",
        title: "Refresh",
        section: "Actions",
        icon: IconRefresh,
        keywords: "reload update",
        run: () => {
          refreshAll();
          setOpen(false);
        },
      },
      {
        id: "nav-machines",
        title: "Go to Machines",
        section: "Navigate",
        icon: IconServer2,
        keywords: "containers vms",
        run: nav("machines"),
      },
      {
        id: "nav-images",
        title: "Go to Images",
        section: "Navigate",
        icon: IconStack2,
        run: nav("images"),
      },
      {
        id: "nav-volumes",
        title: "Go to Volumes",
        section: "Navigate",
        icon: IconDatabase,
        run: nav("volumes"),
      },
      {
        id: "agent-panel",
        title: "Toggle the AI agent panel",
        section: "Navigate",
        icon: IconSparkles,
        keywords: "ai agent claude codex gemini panel sandbox assistant",
        run: () => {
          setOpen(false);
          setAgentPanel((v) => !v);
        },
      },
      {
        id: "nav-containers",
        title: "Go to Containers",
        section: "Navigate",
        icon: IconBrandDocker,
        keywords: "docker container compose engine dind image run",
        run: nav("containers"),
      },
      {
        id: "nav-snapshots",
        title: "Go to Snapshots",
        section: "Navigate",
        icon: IconCamera,
        keywords: "snapshot restore rollback branch clone state",
        run: nav("snapshots"),
      },
      {
        id: "nav-flavors",
        title: "Go to Flavors",
        section: "Navigate",
        icon: IconApps,
        keywords: "environments stacks catalog snapshots presets",
        run: nav("flavors"),
      },
      {
        id: "theme-switch",
        title: theme === "night-rider" ? "Switch to Classic Dark theme" : "Switch to Night Rider theme",
        section: "Appearance",
        icon: IconMoon,
        keywords: "theme dark night rider colors appearance switch style",
        run: () => {
          setOpen(false);
          setTheme(theme === "night-rider" ? "dark" : "night-rider");
        },
      },
      ...ciRepos.map((r: string) => ({
        id: `ci-repo-${r}`,
        title: `CI: ${r.split("/").filter(Boolean).pop()}`,
        section: "CI/CD",
        icon: IconRocket,
        keywords: `ci cd run workflow pipeline repo ${r}`,
        run: () => {
          setOpen(false);
          setCiRepo(r);
          nav("cicd")();
        },
      })),
      {
        id: "nav-cicd",
        title: "Go to CI/CD",
        section: "Navigate",
        icon: IconRocket,
        keywords: "ci cd pipeline workflow spindle tangled build test run steps",
        run: nav("cicd"),
      },
      {
        id: "nav-networks",
        title: "Go to Networks",
        section: "Navigate",
        icon: IconApps,
        keywords: "network subnet dns global compose connectivity",
        run: nav("networks"),
      },
      {
        id: "settings",
        title: "Open Settings",
        section: "Navigate",
        icon: IconSettings,
        run: () => {
          setOpen(false);
          setSettingsOpen(true);
        },
      },
      {
        id: "shortcuts",
        title: "Keyboard Shortcuts",
        section: "Navigate",
        icon: IconKeyboard,
        run: () => {
          setOpen(false);
          setShortcutsOpen(true);
        },
      },
    ];

    const machineCmds: Command[] = machines.flatMap((m) => {
      const label = m.name || m.image || shortId(m.id);
      const guest = kindColor(m.kind, m.image).label;
      const resources = `${guest} · ${m.cpus ?? "?"} vCPU · ${m.mem ?? "?"} MiB${
        m.volume ? ` · vol:${m.volume}` : ""
      }`;
      const cmds: Command[] = [
        {
          id: `open-${m.id}`,
          title: `Open ${label}`,
          subtitle: `${shortId(m.id)} · ${resources}`,
          section: "Machines",
          icon: IconTerminal2,
          keywords: `${m.id} ${m.image} ${m.kind} logs terminal details ${m.running ? "running" : "exited stopped"}`,
          // A unikernel has no shell: don't offer to open a terminal on it.
          terminalFor: m.running && !isUnikraft(m.kind) ? m.id : undefined,
          running: m.running,
          run: () => {
            setSelected(m.id);
            setOpen(false);
          },
        },
      ];
      cmds.push({
        id: `branch-machine-${m.id}`,
        title: `Branch ${label}`,
        subtitle: `${shortId(m.id)} · a disposable copy of this machine`,
        section: "Machines",
        icon: IconGitBranch,
        keywords: `${m.id} branch clone copy fork duplicate snapshot`,
        run: () => {
          setOpen(false);
          setBranchTarget({
            snapshot: m.id,
            label,
            fromMachine: true,
            kind: m.kind,
            running: m.running,
            ports: m.ports.map((p) => `${p.host}:${p.guest}`),
          });
        },
      });
      cmds.push({
        id: `take-snapshot-${m.id}`,
        title: `Take a snapshot of ${label}`,
        subtitle: shortId(m.id),
        section: "Machines",
        icon: IconCamera,
        keywords: `${m.id} snapshot save capture state restore rollback branch`,
        run: () => {
          setOpen(false);
          setSnapshotTarget({ id: m.id, label, kind: m.kind, running: m.running });
        },
      });
      // A unikernel has no rootfs to turn into a reusable flavor.
      if (!isUnikraft(m.kind)) {
        cmds.push({
          id: `snapshot-${m.id}`,
          title: `Commit ${label} as a flavor`,
          subtitle: shortId(m.id),
          section: "Machines",
          icon: IconCamera,
          keywords: `${m.id} commit flavor save capture`,
          run: () => {
            setOpen(false);
            setCommitTarget({ id: m.id, label, kind: m.kind, running: m.running });
          },
        });
      }
      cmds.push({
        id: `resources-${m.id}`,
        title: `Edit resources · ${label}`,
        subtitle: `${m.cpus ?? "?"} vCPU · ${m.mem ?? "?"} MiB`,
        section: "Machines",
        icon: IconCpu,
        keywords: `${m.id} cpu ram memory resources edit vcpu`,
        run: () => {
          setOpen(false);
          setEditResources({
            id: m.id,
            label,
            cpus: m.cpus ?? 1,
            mem: m.mem ?? 512,
            running: m.running,
          });
        },
      });
      if (m.running) {
        cmds.push({
          id: `stop-${m.id}`,
          title: `Stop ${label}`,
          subtitle: shortId(m.id),
          section: "Machines",
          icon: IconPlayerStopFilled,
          keywords: `${m.id} kill halt`,
          run: async () => {
            setOpen(false);
            try {
              await stopMutation.mutateAsync(m.id);
              toast("success", `Stopped ${shortId(m.id)}`);
            } catch (e) {
              toast("error", "Failed to stop", String(e));
            }
          },
        });
      } else {
        cmds.push({
          id: `start-${m.id}`,
          title: `Start ${label}`,
          subtitle: shortId(m.id),
          section: "Machines",
          icon: IconPlayerPlayFilled,
          keywords: `${m.id} run boot launch`,
          run: async () => {
            setOpen(false);
            try {
              await restartMutation.mutateAsync(m.id);
              toast("success", `Started ${shortId(m.id)}`);
            } catch (e) {
              toast("error", "Failed to start", String(e));
            }
          },
        });
      }
      return cmds;
    });

    // Branching is the palette-shaped half of a snapshot: one keystroke to a
    // disposable copy of a known-good machine.
    const snapshotCmds: Command[] = snapshots.map((s) => ({
      id: `branch-${s.id}`,
      title: `Branch ${s.name}`,
      subtitle: `${s.machine_name || shortId(s.machine_id)} · ${s.description || s.kind}`,
      section: "Snapshots",
      icon: IconGitBranch,
      keywords: `${s.name} ${s.machine_name} ${s.kind} branch clone copy snapshot boot`,
      run: () => {
        setOpen(false);
        setBranchTarget({
          snapshot: s.name,
          label: s.name,
          fromMachine: false,
          kind: s.kind,
          ports: s.ports.map((p) => `${p.host}:${p.guest}`),
        });
      },
    }));

    // One entry per agent: the fastest path from "I want Claude" to a prompt.
    const agentCmds: Command[] = aiAgents.map((a) => ({
      id: `ai-${a.id}`,
      title: `New ${a.label} session`,
      subtitle: a.installed
        ? a.description
        : `${a.description} · installs on first run`,
      section: "AI agents",
      icon: IconSparkles,
      keywords: `ai agent sandbox ${a.id} ${a.flavor} chat assistant coding`,
      run: () => {
        setOpen(false);
        startAgent(a.id, null, true).catch(() => {});
      },
    }));

    // Live sessions, grouped under their project in the subtitle so a search
    // for the project name finds every session on that codebase.
    const sessionCmds: Command[] = aiSessions.map((s) => {
      const name = s.label || s.name;
      const project = s.project || "no project";
      const agentLabel =
        aiAgents.find((a) => a.id === s.agent)?.label ?? s.agent;
      return {
        id: `ai-session-${s.id}`,
        title: `Open ${name}`,
        subtitle: `${project} · ${agentLabel} · ${s.running ? "running" : "stopped"}`,
        section: "AI sessions",
        icon: IconSparkles,
        keywords: `ai session ${name} ${project} ${s.agent} ${s.workspace ?? ""} ${
          s.running ? "running" : "stopped"
        }`,
        running: s.running,
        run: () => {
          setOpen(false);
          attachSession(s).catch(() => {});
        },
      };
    });

    const flavorCmds: Command[] = flavors.map((f) => ({
      id: `flavor-${f.source}-${f.name}`,
      title: `Launch ${f.name}`,
      subtitle: f.description || f.base,
      section: "Flavors",
      icon: IconRocket,
      keywords: `${f.category} ${f.base} ${f.kind} ${f.source} ${f.method} launch boot flavor environment`,
      run: () => {
        setOpen(false);
        // Streams pull/build/boot progress in the launch modal.
        launchFlavor(f.name);
      },
    }));

    return [
      ...base,
      ...sessionCmds,
      ...agentCmds,
      ...machineCmds,
      ...snapshotCmds,
      ...flavorCmds,
    ];
  }, [machines, snapshots, flavors, aiAgents, aiSessions]);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    return commands.filter((c) =>
      `${c.title} ${c.subtitle || ""} ${c.keywords || ""} ${c.section}`
        .toLowerCase()
        .includes(q),
    );
  }, [commands, query]);

  // Reset when opening.
  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
      // preventScroll: focusing without it scrolls the document in WKWebView,
      // shifting the whole page (top bar + nav) out of view.
      setTimeout(() => inputRef.current?.focus({ preventScroll: true }), 40);
    }
  }, [open]);

  useEffect(() => {
    setActive(0);
  }, [query]);

  // Keep the active item visible by scrolling ONLY the list container. Using
  // element.scrollIntoView() here would scroll the document root in WKWebView
  // (breaking the layout) and never scroll the list itself.
  useEffect(() => {
    const list = listRef.current;
    const el = list?.querySelector<HTMLElement>(`[data-idx="${active}"]`);
    if (!list || !el) return;
    const top = el.offsetTop;
    const bottom = top + el.offsetHeight;
    if (top < list.scrollTop) {
      list.scrollTop = top - 8;
    } else if (bottom > list.scrollTop + list.clientHeight) {
      list.scrollTop = bottom - list.clientHeight + 8;
    }
  }, [active]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const cmd = filtered[active];
      if ((e.metaKey || e.ctrlKey) && cmd?.terminalFor) {
        openTerminal(cmd.terminalFor);
      } else {
        cmd?.run();
      }
    }
  };

  // Group by section preserving order.
  const groups = useMemo(() => {
    const order: string[] = [];
    const map = new Map<string, { cmd: Command; idx: number }[]>();
    filtered.forEach((cmd, idx) => {
      if (!map.has(cmd.section)) {
        map.set(cmd.section, []);
        order.push(cmd.section);
      }
      map.get(cmd.section)!.push({ cmd, idx });
    });
    return order.map((s) => ({ section: s, items: map.get(s)! }));
  }, [filtered]);

  return (
    <Modal
      isOpen={open}
      onClose={() => setOpen(false)}
      hideCloseButton
      backdrop="opaque"
      size="xl"
      placement="top"
      shouldBlockScroll={false}
      classNames={{
        base: "border border-white/10 bg-content1/95 mt-[12vh]",
        body: "p-0",
      }}
    >
      <ModalContent>
        <div onKeyDown={onKeyDown}>
          <div className="flex items-center gap-3 border-b border-white/10 px-4 py-3">
            <IconSearch size={18} className="text-foreground-400" />
            <input
              ref={inputRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search machines, run commands…"
              className="flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-foreground-500"
            />
            <Kbd className="bg-content2/60 text-foreground-400">esc</Kbd>
          </div>
          <div ref={listRef} className="max-h-[52vh] overflow-auto p-2">
            {filtered.length === 0 ? (
              <div className="px-3 py-8 text-center text-sm text-foreground-500">
                No matching commands
              </div>
            ) : (
              groups.map((g) => (
                <div key={g.section} className="mb-1">
                  <div className="px-3 py-1 text-[11px] font-medium uppercase tracking-wider text-foreground-600">
                    {g.section}
                  </div>
                  {g.items.map(({ cmd, idx }) => {
                    const Icon = cmd.icon;
                    const isActive = idx === active;
                    return (
                      <button
                        key={cmd.id}
                        data-idx={idx}
                        onMouseMove={() => setActive(idx)}
                        onClick={() => cmd.run()}
                        className={`flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition ${
                          isActive ? "bg-primary/20" : "hover:bg-white/5"
                        }`}
                      >
                        <Icon
                          size={17}
                          className={isActive ? "text-primary-300" : "text-foreground-400"}
                        />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm text-foreground">
                            {cmd.title}
                          </span>
                          {cmd.subtitle && (
                            <span className="block truncate font-mono text-[11px] text-foreground-500">
                              {cmd.subtitle}
                            </span>
                          )}
                        </span>
                        {cmd.running === true && (
                          <span className="flex shrink-0 items-center gap-1.5 text-[11px] font-medium text-emerald-300">
                            <span className="relative flex h-2 w-2 shrink-0">
                              <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400/70" />
                              <span className="relative inline-flex h-2 w-2 rounded-full bg-emerald-400" />
                            </span>
                            running
                          </span>
                        )}
                        {cmd.running === false && (
                          <span className="flex shrink-0 items-center gap-1.5 text-[11px] text-foreground-500">
                            <span className="h-2 w-2 shrink-0 rounded-full bg-foreground-600" />
                            exited
                          </span>
                        )}
                        {isActive && (
                          <IconCornerDownLeft
                            size={14}
                            className="shrink-0 text-foreground-500"
                          />
                        )}
                      </button>
                    );
                  })}
                </div>
              ))
            )}
          </div>

          <div className="flex items-center gap-4 border-t border-white/10 px-4 py-2 text-[11px] text-foreground-500">
            <span className="flex items-center gap-1.5">
              <Kbd className="bg-content2/60 text-foreground-400">↑</Kbd>
              <Kbd className="bg-content2/60 text-foreground-400">↓</Kbd>
              navigate
            </span>
            <span className="flex items-center gap-1.5">
              <Kbd className="bg-content2/60 text-foreground-400">↵</Kbd>
              select
            </span>
            <span className="flex items-center gap-1.5">
              <Kbd keys={["command"]} className="bg-content2/60 text-foreground-400">
                ↵
              </Kbd>
              terminal
            </span>
            <span className="flex items-center gap-1.5">
              <Kbd className="bg-content2/60 text-foreground-400">esc</Kbd>
              close
            </span>
            <span className="ml-auto flex items-center gap-1.5">
              <Kbd className="bg-content2/60 text-foreground-400">?</Kbd>
              all shortcuts
            </span>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
