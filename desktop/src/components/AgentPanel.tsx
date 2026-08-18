import { useCallback, useState } from "react";
import { useAtom, useAtomValue } from "jotai";
import {
  Button,
  Dropdown,
  DropdownItem,
  DropdownMenu,
  DropdownTrigger,
  Tooltip,
} from "@heroui/react";
import {
  IconArrowsMaximize,
  IconLayoutList,
  IconArrowsMinimize,
  IconChevronDown,
  IconBrandGit,
  IconFolder,
  IconFolderOff,
  IconPlayerPlayFilled,
  IconPlayerStopFilled,
  IconPlus,
  IconSparkles,
  IconX,
} from "@tabler/icons-react";
import {
  agentPanelFullscreenAtom,
  agentPanelOpenAtom,
  agentPanelWidthAtom,
  agentSelectedAtom,
  agentSessionAtom,
  agentWorkspaceAtom,
} from "../state/atoms";
import {
  useAiAgents,
  useAttachAgentSession,
  useStartAgent,
  useStopAgentSession,
} from "../lib/queries";
import { useToast } from "../state/toast";
import { HAS_NATIVE_FOLDER_PICKER, pickWorkspace } from "../lib/api";
import TerminalPane from "./TerminalPane";
import AgentSessionPicker from "./AgentSessionPicker";
import AgentPromptModal from "./AgentPromptModal";

const MIN_W = 340;

/**
 * The right-docked AI agent panel: an agent's TUI, running in a sandbox VM.
 *
 * Two things it must get right, and both shape the code:
 *
 *  * **The session survives hiding.** The panel is hidden with CSS, never
 *    unmounted — `TerminalPane` closes its PTY on unmount, so unmounting would
 *    kill the agent mid-thought every time the toggle was clicked.
 *  * **The first launch of an agent installs a toolchain**, which takes
 *    minutes. That is streamed into the shared progress modal rather than
 *    hidden behind a spinner (see `useStartAgent`).
 */
export default function AgentPanel() {
  const [open, setOpen] = useAtom(agentPanelOpenAtom);
  const [fullscreen, setFullscreen] = useAtom(agentPanelFullscreenAtom);
  const [width, setWidth] = useAtom(agentPanelWidthAtom);
  const [agentId, setAgentId] = useAtom(agentSelectedAtom);
  const [workspace, setWorkspace] = useAtom(agentWorkspaceAtom);
  const session = useAtomValue(agentSessionAtom);
  const { data: agents = [] } = useAiAgents(open);
  const startAgent = useStartAgent();
  const attachSession = useAttachAgentSession();
  const stopSession = useStopAgentSession();
  const [pickerOpen, setPickerOpen] = useState(false);
  const [prompt, setPrompt] = useState<"repo" | "name" | "workspace" | null>(
    null,
  );
  const toast = useToast();

  const agent = agents.find((a) => a.id === agentId);
  // Start the session lazily: opening the panel should not boot a VM until the
  // user has had a chance to pick an agent and a folder.
  const [starting, setStarting] = useState(false);
  const [stopping, setStopping] = useState(false);

  const start = useCallback(
    async (opts?: { newSession?: boolean; name?: string; repo?: string }) => {
      setStarting(true);
      try {
        await startAgent(
          agentId,
          workspace,
          opts?.newSession ?? false,
          opts?.name,
          opts?.repo,
        );
      } catch (e) {
        toast("error", "Could not start the agent", String(e));
      } finally {
        setStarting(false);
      }
    },
    [agentId, workspace, startAgent, toast],
  );

  /**
   * Stop the sandbox this panel is attached to.
   *
   * Stop, not remove: the agent's login lives on its home volume, so the next
   * session picks up where this one left off. `useStopAgentSession` clears the
   * live session atom, which drops the panel back to its idle state — the
   * terminal is showing a dead PTY the moment the VM goes away.
   */
  const stopCurrent = async () => {
    if (!session) return;
    setStopping(true);
    try {
      await stopSession.mutateAsync(session.machineId);
      toast("success", "Sandbox stopped");
    } catch (e) {
      toast("error", "Could not stop the sandbox", String(e));
    } finally {
      setStopping(false);
    }
  };

  /** Apply a chosen folder: a different folder is a different sandbox. */
  const useFolder = async (dir: string) => {
    setWorkspace(dir);
    // Restart so the agent actually sees it, rather than leaving a stale mount
    // behind the same prompt.
    if (session) await startAgent(agentId, dir, true);
  };

  const chooseFolder = async () => {
    // Native dialog where there is one; otherwise the same modal the rest of
    // the panel uses, since a browser cannot hand back a filesystem path.
    if (!HAS_NATIVE_FOLDER_PICKER) {
      setPrompt("workspace");
      return;
    }
    const dir = await pickWorkspace();
    if (dir === null) return; // cancelled
    await useFolder(dir);
  };

  // Drag the left edge to resize.
  const onResizeStart = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = width;
    const maxW = window.innerWidth - 320;
    const onMove = (ev: MouseEvent) => {
      setWidth(Math.max(MIN_W, Math.min(maxW, startW + (startX - ev.clientX))));
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.userSelect = "";
      document.body.style.cursor = "";
    };
    document.body.style.userSelect = "none";
    document.body.style.cursor = "ew-resize";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  // Never unmount while a session lives — see the note above. Before the first
  // session there is nothing to preserve, so a closed panel renders nothing.
  if (!open && !session) return null;

  return (
    <div
      style={fullscreen ? undefined : { width }}
      className={`${!open ? "hidden " : ""}${
        fullscreen
          ? "absolute inset-0 z-30 flex flex-col bg-[#0a0d13]"
          : "relative flex shrink-0 flex-col border-l border-white/10 bg-[#0a0d13]"
      }`}
    >
      {!fullscreen && (
        <div
          onMouseDown={onResizeStart}
          className="group absolute inset-y-0 -left-1 z-10 w-2 cursor-ew-resize"
        >
          <div className="ml-[3px] mt-1/2 h-10 w-0.5 translate-y-[45vh] rounded-full bg-white/15 transition group-hover:bg-primary/60" />
        </div>
      )}

      {/* Header: which agent, which folder, and the session controls. */}
      <div className="flex items-center gap-1.5 border-b border-white/10 bg-content1/70 px-2 py-1.5">
        <Dropdown placement="bottom-start">
          <DropdownTrigger>
            <Button
              size="sm"
              variant="light"
              className="min-w-0 px-2"
              startContent={<IconSparkles size={14} className="text-violet-300" />}
              endContent={<IconChevronDown size={13} />}
            >
              <span className="truncate text-xs">{agent?.label ?? agentId}</span>
            </Button>
          </DropdownTrigger>
          <DropdownMenu
            aria-label="Coding agent"
            selectionMode="single"
            selectedKeys={new Set([agentId])}
            onAction={(k) => {
              const next = String(k);
              setAgentId(next);
              // Switching agents means a different sandbox; start it now so the
              // dropdown reads as the thing that changes what you are talking to.
              if (session) startAgent(next, workspace, false).catch(() => {});
            }}
          >
            {agents.map((a) => (
              <DropdownItem
                key={a.id}
                description={
                  a.installed
                    ? a.description
                    : `${a.description} · installs on first run`
                }
              >
                {a.label}
              </DropdownItem>
            ))}
          </DropdownMenu>
        </Dropdown>

        <Tooltip
          content={
            workspace
              ? `Sharing ${workspace} — click to change`
              : "No folder shared. The agent cannot see your files."
          }
          placement="bottom"
        >
          <Button
            size="sm"
            variant="light"
            className="min-w-0 px-2 text-foreground-400"
            startContent={
              workspace ? (
                <IconFolder size={14} className="text-amber-300" />
              ) : (
                <IconFolderOff size={14} />
              )
            }
            onPress={chooseFolder}
          >
            <span className="max-w-[130px] truncate text-[11px]">
              {workspace ? basename(workspace) : "No folder"}
            </span>
          </Button>
        </Tooltip>

        <div className="flex-1" />

        <Tooltip content="Clone a git repo into a new sandbox" placement="bottom">
          <Button
            isIconOnly
            size="sm"
            variant="light"
            isDisabled={starting}
            onPress={() => setPrompt("repo")}
          >
            <IconBrandGit size={16} />
          </Button>
        </Tooltip>
        {/* One button, because start and stop are the same question asked in
            two states — and the panel only ever has one sandbox attached. */}
        <Tooltip
          content={
            session
              ? "Stop this sandbox (its login is kept)"
              : `Start a ${agent?.label ?? agentId} sandbox`
          }
          placement="bottom"
        >
          <Button
            isIconOnly
            size="sm"
            variant="light"
            isDisabled={starting || stopping}
            isLoading={starting || stopping}
            onPress={session ? stopCurrent : () => start()}
            // Danger-tinted on hover only when it stops: the neighbouring X
            // merely hides the panel, and they are one icon apart.
            // White, not the muted `foreground-400` the other icons use: this
            // one is the panel's primary action, and dimmed it read as
            // disabled next to buttons that genuinely were.
            className={
              session
                ? "text-white data-[hover=true]:text-danger"
                : "text-white data-[hover=true]:text-success"
            }
          >
            {session ? (
              <IconPlayerStopFilled size={15} />
            ) : (
              <IconPlayerPlayFilled size={15} />
            )}
          </Button>
        </Tooltip>
        <Tooltip content="Switch session" placement="bottom">
          <Button
            isIconOnly
            size="sm"
            variant="light"
            onPress={() => setPickerOpen(true)}
          >
            <IconLayoutList size={16} />
          </Button>
        </Tooltip>
        <Tooltip content="New session" placement="bottom">
          <Button
            isIconOnly
            size="sm"
            variant="light"
            isDisabled={starting}
            onPress={() => setPrompt("name")}
          >
            <IconPlus size={16} />
          </Button>
        </Tooltip>
        <Tooltip
          content={fullscreen ? "Exit fullscreen" : "Fullscreen"}
          placement="bottom"
        >
          <Button
            isIconOnly
            size="sm"
            variant="light"
            onPress={() => setFullscreen((f) => !f)}
          >
            {fullscreen ? (
              <IconArrowsMinimize size={16} />
            ) : (
              <IconArrowsMaximize size={16} />
            )}
          </Button>
        </Tooltip>
        <Tooltip content="Hide panel (the session keeps running)" placement="bottom">
          <Button
            isIconOnly
            size="sm"
            variant="light"
            onPress={() => {
              setFullscreen(false);
              setOpen(false);
            }}
          >
            <IconX size={16} />
          </Button>
        </Tooltip>
      </div>

      <div className="relative min-h-0 flex-1 overflow-hidden">
        {session ? (
          // Keyed by the session: a new sandbox remounts the pane (and so opens
          // a new PTY), while a toggle of `open` does not.
          <TerminalPane
            key={session.key}
            machineId={session.machineId}
            command={session.command}
          />
        ) : (
          <Idle
            agentLabel={agent?.label ?? agentId}
            installed={agent?.installed ?? true}
            workspace={workspace}
            starting={starting}
            onStart={() => start()}
          />
        )}
      </div>

      <AgentPromptModal
        open={prompt === "workspace"}
        title="Share a folder"
        placeholder="/srv/code/my-project"
        hint="A path on the machine running the engine — not on this computer."
        icon={IconFolder}
        submitLabel="↵ share"
        onClose={() => setPrompt(null)}
        onSubmit={(dir) => {
          useFolder(dir).catch((e) =>
            toast("error", "Could not share that folder", String(e)),
          );
        }}
      />

      <AgentPromptModal
        open={prompt === "repo"}
        title="Clone a repository"
        placeholder="https://github.com/owner/repo.git"
        hint="Cloned inside the sandbox — no access to your filesystem needed."
        icon={IconBrandGit}
        submitLabel="↵ clone & start"
        onClose={() => setPrompt(null)}
        onSubmit={(repo) => start({ newSession: true, repo })}
      />

      <AgentPromptModal
        open={prompt === "name"}
        title="Name this session"
        placeholder="refactor-auth"
        hint="Optional — press ↵ with an empty name to start one anyway."
        icon={IconPlus}
        submitLabel="↵ start session"
        allowEmpty
        onClose={() => setPrompt(null)}
        onSubmit={(name) => start({ newSession: true, name: name || undefined })}
      />

      <AgentSessionPicker
        open={pickerOpen}
        onClose={() => setPickerOpen(false)}
        onPick={(s) => {
          attachSession(s).catch((e) =>
            toast("error", "Could not open that session", String(e)),
          );
        }}
      />
    </div>
  );
}

/** Nothing running yet: say what will happen, then do it. */
function Idle({
  agentLabel,
  installed,
  workspace,
  starting,
  onStart,
}: {
  agentLabel: string;
  installed: boolean;
  workspace: string | null;
  starting: boolean;
  onStart: () => void;
}) {
  return (
    <div className="grid h-full place-items-center px-6 text-center">
      <div className="flex max-w-[280px] flex-col items-center gap-3">
        <IconSparkles size={28} className="text-violet-300" />
        <div>
          <p className="text-sm font-medium text-foreground">{agentLabel}</p>
          <p className="mt-1 text-xs text-foreground-500">
            Runs in a sandbox VM. It sees{" "}
            {workspace ? (
              <span className="text-foreground-300">{basename(workspace)}</span>
            ) : (
              "no folder"
            )}
            , and nothing else of your machine.
          </p>
          {!installed && (
            <p className="mt-2 text-[11px] text-amber-300">
              First run installs it — that takes a few minutes, with a progress
              log.
            </p>
          )}
        </div>
        <Button
          color="primary"
          size="sm"
          isLoading={starting}
          startContent={!starting && <IconSparkles size={15} />}
          onPress={onStart}
        >
          {starting ? "Starting…" : `Start ${agentLabel}`}
        </Button>
      </div>
    </div>
  );
}

/** The last path component, for a header that has ~130px to work with. */
function basename(p: string): string {
  const parts = p.replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || p;
}
