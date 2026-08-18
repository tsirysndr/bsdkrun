import { useCallback, useEffect, useRef, useState } from "react";
import { useAtom } from "jotai";
import {
  Button,
  Chip,
  Select,
  SelectItem,
  Spinner,
  Tooltip,
} from "@heroui/react";
import {
  IconBrandGit,
  IconCheck,
  IconChevronDown,
  IconChevronRight,
  IconCircle,
  IconFolder,
  IconPlayerPlayFilled,
  IconRocket,
  IconTerminal2,
  IconX,
} from "@tabler/icons-react";
import {
  api,
  onFlavorDone,
  onFlavorLog,
  HAS_NATIVE_FOLDER_PICKER,
  pickWorkspace,
} from "../lib/api";
import { useToast } from "../state/toast";
import { ciRepoAtom, ciRunsAtom } from "../state/atoms";
import type { CiRun, CiStep, CiWorkflowInfo } from "../lib/types";
import AgentPromptModal from "./AgentPromptModal";
import { ago } from "../lib/format";

/**
 * The CI/CD screen: run a repository's tangled spindle workflows
 * (`.tangled/workflows/*.yml`) in microVMs, watch the steps live, and keep a
 * history of recent runs.
 *
 * The runner streams spindle's LogLine JSON (`bsdkrun ci run --json`) through
 * the same `flavor://log` events every other launch uses; this screen parses
 * that stream into a step timeline. Runs are kept client-side — they are a
 * viewing convenience, not a system of record, and the engine deliberately
 * stays stateless about them.
 */

const EVENTS = [
  { key: "manual", label: "Manual" },
  { key: "push", label: "Push" },
  { key: "pull_request", label: "Pull request" },
];

/** How many runs history keeps, and how many log lines per step. Both caps
 * exist because this persists to localStorage, which is small and slow. */
const MAX_RUNS = 20;
const MAX_LINES = 500;

export default function CicdView() {
  const [repo, setRepo] = useAtom(ciRepoAtom);
  const [runs, setRuns] = useAtom(ciRunsAtom);
  const [event, setEvent] = useState("manual");
  const [workflows, setWorkflows] = useState<CiWorkflowInfo[]>([]);
  const [wfError, setWfError] = useState<string | null>(null);
  const [loadingWf, setLoadingWf] = useState(false);
  const [selectedRun, setSelectedRun] = useState<string | null>(null);
  const [prompt, setPrompt] = useState<"clone" | "dir" | null>(null);
  const [cloning, setCloning] = useState(false);
  const toast = useToast();

  const refreshWorkflows = useCallback(
    async (dir: string, ev: string) => {
      if (!dir) {
        setWorkflows([]);
        return;
      }
      setLoadingWf(true);
      setWfError(null);
      try {
        setWorkflows(await api.ciWorkflows(dir, ev));
      } catch (e) {
        setWorkflows([]);
        setWfError(String(e));
      } finally {
        setLoadingWf(false);
      }
    },
    [],
  );

  useEffect(() => {
    refreshWorkflows(repo, event);
  }, [repo, event, refreshWorkflows]);

  // One pair of listeners for every run: events carry the run's id.
  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    onFlavorLog((p) => {
      if (!p.launch_id.startsWith("ci-")) return;
      setRuns((rs) => rs.map((r) => (r.id === p.launch_id ? applyLine(r, p.line) : r)));
    }).then((u) => unlisteners.push(u));
    onFlavorDone((p) => {
      if (!p.launch_id.startsWith("ci-")) return;
      setRuns((rs) =>
        rs.map((r) =>
          r.id === p.launch_id
            ? finishRun(r, p.error ?? null)
            : r,
        ),
      );
    }).then((u) => unlisteners.push(u));
    return () => unlisteners.forEach((u) => u());
  }, [setRuns]);

  const chooseFolder = async () => {
    if (!HAS_NATIVE_FOLDER_PICKER) {
      setPrompt("dir");
      return;
    }
    const dir = await pickWorkspace();
    if (dir) setRepo(dir);
  };

  const clone = async (url: string) => {
    setCloning(true);
    try {
      const dir = await api.ciClone(url);
      setRepo(dir);
      toast("success", "Repository ready", dir);
    } catch (e) {
      toast("error", "Clone failed", String(e));
    } finally {
      setCloning(false);
    }
  };

  const startRun = async (names: string[]) => {
    if (!repo) {
      toast("error", "Pick a repository first");
      return;
    }
    const id = `ci-${Date.now()}`;
    const run: CiRun = {
      id,
      dir: repo,
      names,
      event,
      status: "running",
      startedAt: Date.now(),
      steps: [],
    };
    setRuns((rs) => [run, ...rs].slice(0, MAX_RUNS));
    setSelectedRun(id);
    try {
      await api.ciRun(id, repo, names, event);
    } catch (e) {
      setRuns((rs) =>
        rs.map((r) => (r.id === id ? finishRun(r, String(e)) : r)),
      );
    }
  };

  const run = runs.find((r) => r.id === selectedRun) ?? runs[0] ?? null;
  const repoName = repo ? repo.split("/").filter(Boolean).pop() : null;

  return (
    <div className="flex h-full min-h-0">
      {/* Left: repository + workflows + history */}
      <div className="flex w-80 shrink-0 flex-col border-r border-white/10">
        <div className="border-b border-white/10 p-3">
          <div className="mb-2 flex items-center gap-2">
            <IconRocket size={16} className="text-primary" />
            <span className="text-sm font-medium">CI/CD</span>
          </div>
          <div className="flex items-center gap-1.5">
            <Button
              size="sm"
              variant="flat"
              className="min-w-0 flex-1 justify-start px-2"
              startContent={<IconFolder size={14} className="shrink-0 text-amber-300" />}
              onPress={chooseFolder}
            >
              <span className="truncate text-[11px]">
                {repoName ?? "Choose a repository…"}
              </span>
            </Button>
            <Tooltip content="Clone a git URL" placement="bottom">
              <Button
                isIconOnly
                size="sm"
                variant="flat"
                isLoading={cloning}
                onPress={() => setPrompt("clone")}
              >
                <IconBrandGit size={14} />
              </Button>
            </Tooltip>
          </div>
          <Select
            aria-label="Trigger event"
            size="sm"
            className="mt-2"
            selectedKeys={[event]}
            onSelectionChange={(k) => {
              const v = [...k][0];
              if (typeof v === "string") setEvent(v);
            }}
          >
            {EVENTS.map((e) => (
              <SelectItem key={e.key}>{e.label}</SelectItem>
            ))}
          </Select>
        </div>

        <div className="border-b border-white/10 p-3">
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-[11px] font-medium uppercase tracking-wider text-foreground-500">
              Workflows
            </span>
            {workflows.some((w) => w.matches) && (
              <Button
                size="sm"
                variant="flat"
                color="primary"
                className="h-6 px-2 text-[11px]"
                onPress={() => startRun([])}
              >
                Run matching
              </Button>
            )}
          </div>
          {loadingWf ? (
            <Spinner size="sm" />
          ) : wfError ? (
            <p className="text-[11px] text-danger">{wfError}</p>
          ) : workflows.length === 0 ? (
            <p className="text-[11px] text-foreground-500">
              {repo
                ? "No .tangled/workflows in this repository."
                : "Pick a repository or clone one to see its workflows."}
            </p>
          ) : (
            workflows.map((w) => (
              <div
                key={w.name}
                className="group flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-white/5"
              >
                <span
                  title={w.matches ? "Matches this trigger" : "Does not match"}
                  className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                    w.matches ? "bg-emerald-400" : "bg-foreground-600"
                  }`}
                />
                <span className="min-w-0 flex-1 truncate text-xs">{w.name}</span>
                <span className="shrink-0 text-[10px] text-foreground-600">{w.engine}</span>
                <Tooltip content={`Run ${w.name}`} placement="top">
                  <button
                    onClick={() => startRun([w.name])}
                    className="rounded p-1 text-foreground-400 opacity-0 transition group-hover:opacity-100 hover:bg-white/10 hover:text-success"
                  >
                    <IconPlayerPlayFilled size={12} />
                  </button>
                </Tooltip>
              </div>
            ))
          )}
        </div>

        <div className="min-h-0 flex-1 overflow-auto p-3">
          <span className="text-[11px] font-medium uppercase tracking-wider text-foreground-500">
            Recent runs
          </span>
          {runs.length === 0 ? (
            <p className="mt-2 text-[11px] text-foreground-500">
              Nothing yet — trigger a workflow.
            </p>
          ) : (
            runs.map((r) => (
              <div
                key={r.id}
                onClick={() => setSelectedRun(r.id)}
                className={`mt-1.5 cursor-pointer rounded-lg border p-2 ${
                  run?.id === r.id
                    ? "border-primary/40 bg-primary/10"
                    : "border-white/10 hover:bg-white/5"
                }`}
              >
                <div className="flex items-center gap-2">
                  <StatusDot status={r.status} />
                  <span className="min-w-0 flex-1 truncate text-xs">
                    {r.names.length ? r.names.join(", ") : "all matching"}
                  </span>
                  <span className="shrink-0 text-[10px] text-foreground-600">
                    {ago(new Date(r.startedAt).toISOString())}
                  </span>
                </div>
                <div className="mt-0.5 truncate text-[10px] text-foreground-500">
                  {r.dir.split("/").filter(Boolean).pop()} · {r.event}
                  {r.finishedAt
                    ? ` · ${Math.round((r.finishedAt - r.startedAt) / 1000)}s`
                    : ""}
                </div>
              </div>
            ))
          )}
        </div>
      </div>

      {/* Right: the selected run's step timeline */}
      <div className="min-w-0 flex-1 overflow-auto p-4">
        {!run ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-foreground-500">
            <IconRocket size={28} />
            <p className="text-sm">Run a workflow to see its steps here.</p>
          </div>
        ) : (
          <RunDetail run={run} />
        )}
      </div>

      <AgentPromptModal
        open={prompt === "clone"}
        title="Clone a repository for CI"
        placeholder="https://github.com/owner/repo.git"
        icon={IconBrandGit}
        hint="Cloned on the engine's host; its workflows run from HEAD."
        submitLabel="↵ clone"
        onSubmit={(url) => clone(url)}
        onClose={() => setPrompt(null)}
      />
      <AgentPromptModal
        open={prompt === "dir"}
        title="Repository path"
        placeholder="/path/to/repository (on the engine's host)"
        icon={IconFolder}
        hint="The directory is resolved on the machine running the engine."
        submitLabel="↵ use"
        onSubmit={(dir) => setRepo(dir)}
        onClose={() => setPrompt(null)}
      />
    </div>
  );
}

function StatusDot({ status }: { status: CiRun["status"] }) {
  if (status === "running") return <Spinner size="sm" className="scale-50" />;
  return (
    <span
      className={`h-2 w-2 shrink-0 rounded-full ${
        status === "success" ? "bg-emerald-400" : "bg-danger"
      }`}
    />
  );
}

function RunDetail({ run }: { run: CiRun }) {
  return (
    <div className="mx-auto max-w-3xl">
      <div className="mb-4 flex items-center gap-3">
        <StatusDot status={run.status} />
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-base font-medium">
            {run.names.length ? run.names.join(", ") : "All matching workflows"}
          </h2>
          <p className="truncate text-xs text-foreground-500">
            {run.dir} · {run.event} · {new Date(run.startedAt).toLocaleString()}
          </p>
        </div>
        <Chip
          size="sm"
          variant="flat"
          color={
            run.status === "success"
              ? "success"
              : run.status === "failed"
                ? "danger"
                : "primary"
          }
        >
          {run.status}
        </Chip>
      </div>

      {run.error && run.steps.every((s) => s.status !== "failed") && (
        <p className="mb-3 rounded-lg border border-danger/40 bg-danger/10 p-3 text-xs text-danger">
          {run.error}
        </p>
      )}

      <div className="relative">
        {/* The timeline spine — what makes a list of steps read as a run. */}
        <div className="absolute bottom-4 left-[13px] top-4 w-px bg-white/10" />
        {run.steps.map((s) => (
          <StepCard key={s.id} step={s} />
        ))}
        {run.steps.length === 0 && run.status === "running" && (
          <div className="flex items-center gap-2 py-2 pl-8 text-xs text-foreground-500">
            <Spinner size="sm" /> booting the VM…
          </div>
        )}
      </div>
    </div>
  );
}

function StepCard({ step }: { step: CiStep }) {
  // Failures open themselves: the log is the only place the reason lives.
  const [open, setOpen] = useState<boolean | null>(null);
  const expanded = open ?? (step.status === "failed" || step.status === "running");
  const logRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [step.lines.length]);

  return (
    <div className="relative mb-2 pl-8">
      <div className="absolute left-1 top-2.5">
        {step.status === "running" ? (
          <Spinner size="sm" className="scale-75" />
        ) : step.status === "ok" ? (
          <span className="flex h-[22px] w-[22px] items-center justify-center rounded-full bg-emerald-400/20 text-emerald-400">
            <IconCheck size={13} />
          </span>
        ) : step.status === "failed" ? (
          <span className="flex h-[22px] w-[22px] items-center justify-center rounded-full bg-danger/20 text-danger">
            <IconX size={13} />
          </span>
        ) : (
          <span className="flex h-[22px] w-[22px] items-center justify-center text-foreground-600">
            <IconCircle size={13} />
          </span>
        )}
      </div>
      <div
        className={`rounded-xl border ${
          step.status === "failed"
            ? "border-danger/40"
            : step.status === "running"
              ? "border-primary/40"
              : "border-white/10"
        } bg-content1/50`}
      >
        <button
          onClick={() => setOpen(!expanded)}
          className="flex w-full items-center gap-2 px-3 py-2 text-left"
        >
          {expanded ? (
            <IconChevronDown size={13} className="shrink-0 text-foreground-500" />
          ) : (
            <IconChevronRight size={13} className="shrink-0 text-foreground-500" />
          )}
          <span className="min-w-0 flex-1 truncate text-sm">{step.name}</span>
          {step.system && (
            <Chip size="sm" variant="flat" className="h-5 text-[10px]">
              setup
            </Chip>
          )}
          {step.durationMs != null && (
            <span className="shrink-0 text-[11px] text-foreground-500">
              {step.durationMs >= 1000
                ? `${(step.durationMs / 1000).toFixed(1)}s`
                : `${step.durationMs}ms`}
            </span>
          )}
        </button>
        {expanded && (
          <div
            ref={logRef}
            className="max-h-72 overflow-auto border-t border-white/10 px-3 py-2 font-mono text-[12px] leading-[1.6] text-foreground"
          >
            {step.lines.length === 0 ? (
              <span className="flex items-center gap-2 text-foreground-500">
                <IconTerminal2 size={12} /> no output
              </span>
            ) : (
              step.lines.map((l, i) => (
                <div
                  key={i}
                  className={`whitespace-pre-wrap break-words ${
                    l.stream === "stderr" ? "text-amber-200/90" : ""
                  }`}
                >
                  {l.content}
                </div>
              ))
            )}
          </div>
        )}
      </div>
    </div>
  );
}

// ---- stream parsing --------------------------------------------------------
//
// `bsdkrun ci run --json` emits spindle LogLine JSON, one object per line:
// control lines open/close steps, data lines carry output. Anything that does
// not parse (the runner's own stderr, git noise) lands on the most recent step
// so nothing is silently dropped.

function applyLine(run: CiRun, raw: string): CiRun {
  let parsed: any;
  try {
    parsed = JSON.parse(raw);
  } catch {
    parsed = null;
  }

  const steps = [...run.steps];
  if (parsed && parsed.kind === "control") {
    const id = parsed.step_id as number;
    if (parsed.step_status === "start") {
      steps.push({
        id,
        name: String(parsed.content ?? `step ${id}`),
        system: parsed.step_kind === 0,
        status: "running",
        lines: [],
        startedAt: Date.now(),
      });
    } else {
      const i = steps.findIndex((s) => s.id === id);
      if (i >= 0 && steps[i].status === "running") {
        steps[i] = {
          ...steps[i],
          status: "ok",
          durationMs: Date.now() - (steps[i].startedAt ?? Date.now()),
        };
      }
    }
  } else if (parsed && parsed.kind === "data") {
    const i = steps.findIndex((s) => s.id === parsed.step_id);
    const line = { content: String(parsed.content ?? ""), stream: String(parsed.stream ?? "stdout") };
    if (i >= 0) {
      steps[i] = {
        ...steps[i],
        lines: [...steps[i].lines, line].slice(-MAX_LINES),
      };
    }
  } else if (raw.trim()) {
    // Not a LogLine: runner/stderr chatter. Attach to the last step.
    const i = steps.length - 1;
    if (i >= 0) {
      steps[i] = {
        ...steps[i],
        lines: [...steps[i].lines, { content: raw, stream: "stderr" }].slice(-MAX_LINES),
      };
    }
  }
  return { ...run, steps };
}

function finishRun(run: CiRun, error: string | null): CiRun {
  const steps = run.steps.map((s) =>
    s.status === "running"
      ? {
          ...s,
          // A step still open when the run ends is the one that failed.
          status: error ? ("failed" as const) : ("ok" as const),
          durationMs: Date.now() - (s.startedAt ?? Date.now()),
        }
      : s,
  );
  return {
    ...run,
    steps,
    status: error ? "failed" : "success",
    error: error ?? undefined,
    finishedAt: Date.now(),
  };
}
