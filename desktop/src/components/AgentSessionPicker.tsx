import { useEffect, useMemo, useRef, useState } from "react";
import { Modal, ModalContent, Kbd, Tooltip } from "@heroui/react";
import { useAtomValue } from "jotai";
import {
  IconFolder,
  IconPlayerStopFilled,
  IconSearch,
  IconSparkles,
  IconTrash,
} from "@tabler/icons-react";
import { agentSessionAtom } from "../state/atoms";
import {
  useAiAgents,
  useAiSessions,
  useDeleteAgentSession,
  useStopAgentSession,
} from "../lib/queries";
import { ago } from "../lib/format";
import { useToast } from "../state/toast";
import type { AiSession } from "../lib/types";

/**
 * Switch between agent sessions, Raycast-style: type to filter, ↑/↓ to move,
 * ↵ to attach.
 *
 * Sessions are grouped by project — several sessions against one codebase are
 * views of the same work, and that is how they are looked for. A running one
 * carries a green dot, because "which of these is still alive" is the first
 * thing you need to know and the slowest thing to work out from a list.
 */
export default function AgentSessionPicker({
  open,
  onClose,
  onPick,
}: {
  open: boolean;
  onClose: () => void;
  onPick: (session: AiSession) => void;
}) {
  const { data: sessions = [] } = useAiSessions(open);
  const { data: agents = [] } = useAiAgents(open);
  const live = useAtomValue(agentSessionAtom);
  const stop = useStopAgentSession();
  const remove = useDeleteAgentSession();
  const toast = useToast();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const label = (s: AiSession) => s.label || s.name;
  const agentLabel = (id: string) =>
    agents.find((a) => a.id === id)?.label ?? id;

  // Flattened for keyboard navigation, but rendered grouped: the headers are
  // markers in the same list rather than a nested structure, which keeps ↑/↓
  // from having to know about groups at all.
  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    const matches = sessions.filter(
      (s) =>
        !q ||
        label(s).toLowerCase().includes(q) ||
        s.agent.toLowerCase().includes(q) ||
        (s.project ?? "").toLowerCase().includes(q) ||
        (s.workspace ?? "").toLowerCase().includes(q),
    );
    const groups = new Map<string, AiSession[]>();
    for (const s of matches) {
      const key = s.project || "No project";
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(s);
    }
    return [...groups.entries()].flatMap(([project, items]) => [
      { kind: "header" as const, project },
      ...items.map((session) => ({ kind: "session" as const, session })),
    ]);
  }, [sessions, query, agents]);

  const pickable = rows.filter((r) => r.kind === "session");

  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
    }
  }, [open]);
  useEffect(() => setActive(0), [query]);

  // Keep the highlighted row in view without scrolling the page.
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(
      `[data-idx="${active}"]`,
    );
    if (!el || !listRef.current) return;
    const box = listRef.current.getBoundingClientRect();
    const row = el.getBoundingClientRect();
    if (row.top < box.top) listRef.current.scrollTop -= box.top - row.top;
    else if (row.bottom > box.bottom)
      listRef.current.scrollTop += row.bottom - box.bottom;
  }, [active]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, pickable.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const row = pickable[active];
      if (row?.kind === "session") {
        onPick(row.session);
        onClose();
      }
    }
  };

  const doStop = async (s: AiSession) => {
    try {
      await stop.mutateAsync(s.id);
      toast("success", `Stopped ${label(s)}`);
    } catch (e) {
      toast("error", "Could not stop the session", String(e));
    }
  };

  const doDelete = async (s: AiSession) => {
    try {
      // Stopped first: removing a running machine needs force, and a session
      // being deleted mid-thought should shut down rather than be killed.
      if (s.running) await stop.mutateAsync(s.id);
      await remove.mutateAsync(s.id);
      toast("success", `Deleted ${label(s)}`);
    } catch (e) {
      toast("error", "Could not delete the session", String(e));
    }
  };

  return (
    <Modal
      isOpen={open}
      onClose={onClose}
      hideCloseButton
      // Deliberately identical chrome to the command palette: same backdrop,
      // same placement, same borderless input. Two search modals that look
      // different read as two different features.
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
              autoFocus
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search sessions and projects…"
              className="flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-foreground-500"
            />
            <Kbd className="bg-content2/60 text-foreground-400">esc</Kbd>
          </div>

          <div ref={listRef} className="max-h-[52vh] overflow-auto p-2">
            {pickable.length === 0 ? (
              <p className="px-3 py-8 text-center text-sm text-foreground-500">
                {sessions.length === 0
                  ? "No sessions yet. Start one from the panel."
                  : "Nothing matches that."}
              </p>
            ) : (
              (() => {
                let idx = -1;
                return rows.map((row, i) => {
                  if (row.kind === "header") {
                    return (
                      <div
                        key={`h-${row.project}-${i}`}
                        className="px-3 py-1 text-[11px] font-medium uppercase tracking-wider text-foreground-600"
                      >
                        {row.project}
                      </div>
                    );
                  }
                  idx += 1;
                  const s = row.session;
                  const isActive = idx === active;
                  const isLive = live?.machineId === s.id;
                  const myIdx = idx;
                  return (
                    <div
                      key={s.id}
                      data-idx={myIdx}
                      onMouseEnter={() => setActive(myIdx)}
                      onClick={() => {
                        onPick(s);
                        onClose();
                      }}
                      className={`group flex cursor-pointer items-center gap-2.5 rounded-lg px-2.5 py-2 ${
                        isActive ? "bg-primary/15" : "hover:bg-white/5"
                      }`}
                    >
                      <span
                        title={s.running ? "Running" : "Stopped"}
                        className={`h-2 w-2 shrink-0 rounded-full ${
                          s.running ? "bg-emerald-400" : "bg-foreground-600"
                        }`}
                      />
                      <IconSparkles
                        size={14}
                        className="shrink-0 text-violet-300"
                      />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2">
                          <span className="truncate text-sm text-foreground">
                            {label(s)}
                          </span>
                          {isLive && (
                            <span className="shrink-0 rounded bg-primary/20 px-1.5 py-0.5 text-[10px] text-primary">
                              open
                            </span>
                          )}
                        </div>
                        <div className="flex items-center gap-1.5 truncate text-[11px] text-foreground-500">
                          <span>{agentLabel(s.agent)}</span>
                          {s.workspace && (
                            <>
                              <IconFolder size={11} className="shrink-0" />
                              <span className="truncate">{s.workspace}</span>
                            </>
                          )}
                          <span className="shrink-0">
                            · {ago(String(s.created_at))}
                          </span>
                        </div>
                      </div>

                      <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition group-hover:opacity-100">
                        {s.running && (
                          <Tooltip content="Stop" placement="top">
                            <button
                              onClick={(e) => {
                                e.stopPropagation();
                                doStop(s);
                              }}
                              className="rounded p-1 text-foreground-400 hover:bg-white/10 hover:text-foreground"
                            >
                              <IconPlayerStopFilled size={13} />
                            </button>
                          </Tooltip>
                        )}
                        <Tooltip content="Delete session" placement="top">
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              doDelete(s);
                            }}
                            className="rounded p-1 text-foreground-400 hover:bg-white/10 hover:text-danger"
                          >
                            <IconTrash size={13} />
                          </button>
                        </Tooltip>
                      </div>
                    </div>
                  );
                });
              })()
            )}
          </div>

          <div className="flex items-center gap-3 border-t border-white/10 px-3 py-1.5 text-[10px] text-foreground-600">
            <span>↑↓ navigate</span>
            <span>↵ open</span>
            <span>esc close</span>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
