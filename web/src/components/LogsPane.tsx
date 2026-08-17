import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { registerWebLinks } from "../lib/xtermLinks";
import { Button, Input, Switch, Tooltip } from "@heroui/react";
import {
  IconSearch,
  IconChevronUp,
  IconChevronDown,
  IconCopy,
  IconX,
} from "@tabler/icons-react";
import { api, onLogLine } from "../lib/api";
import type { UnlistenFn } from "../lib/api";
import { useToast } from "../state/toast";

const THEME = {
  background: "#0a0d13",
  // Near-white rather than the muted grey the rest of the chrome uses: this is
  // the content, not a label, and #c6ccd8 on #0a0d13 reads as dim next to it.
  foreground: "#e8ecf5",
  cursor: "#0a0d13",
  selectionBackground: "rgba(124,139,255,0.35)",
};

/** Read-only console log viewer: initial snapshot + live `logs -f` follow, an
 *  in-buffer quick search, and copy-on-select. Toggle to the boot log for
 *  early-failure diagnostics. */
export default function LogsPane({ machineId }: { machineId: string }) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const searchRef = useRef<SearchAddon | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [boot, setBoot] = useState(false);
  const [follow, setFollow] = useState(true);
  const [query, setQuery] = useState("");
  const [hasSelection, setHasSelection] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const toast = useToast();

  // Cmd/Ctrl+F focuses the log search box (like a browser find-in-page).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") {
        if (!hostRef.current) return;
        e.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    let disposed = false;
    let term: Terminal | null = null;
    let fit: FitAddon | null = null;
    let unlisten: UnlistenFn | null = null;
    let ro: ResizeObserver | null = null;
    let streaming = false;
    setNotice(null);

    (async () => {
      try {
        await (document as any).fonts?.ready;
      } catch {
        /* ignore */
      }
      if (disposed || !hostRef.current) return;

      term = new Terminal({
        fontFamily: '"Agave", ui-monospace, Menlo, monospace',
        fontSize: 14,
        lineHeight: 1.5,
        disableStdin: true,
        cursorInactiveStyle: "none",
        convertEol: true,
        scrollback: 20000,
        theme: THEME,
      });
      fit = new FitAddon();
      const search = new SearchAddon();
      term.loadAddon(fit);
      term.loadAddon(search);
      // A URL in a boot log (a service's address, a docs link) opens where a
      // browser actually exists: the host.
      registerWebLinks(term, (uri) => {
        window.open(uri, "_blank", "noopener,noreferrer");
      });
      term.open(hostRef.current);
      fit.fit();
      termRef.current = term;
      searchRef.current = search;

      // Copy-on-select: reveal a Copy button + support Cmd/Ctrl+C.
      term.onSelectionChange(() => {
        setHasSelection(!!term && term.hasSelection());
      });
      term.attachCustomKeyEventHandler((e) => {
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c" && term?.hasSelection()) {
          navigator.clipboard?.writeText(term.getSelection());
          return false;
        }
        return true;
      });

      ro = new ResizeObserver(() => {
        try {
          fit?.fit();
        } catch {
          /* ignore */
        }
      });
      ro.observe(hostRef.current);

      try {
        const snapshot = await api.machineLogs(machineId, boot);
        if (disposed || !term) return;
        if (snapshot.trim().length === 0) {
          setNotice(
            boot ? "No boot log recorded." : "Console log is empty so far.",
          );
        }
        term.write(snapshot.replace(/\n/g, "\r\n"));
      } catch (e) {
        const msg = String(e);
        if (/no (console|boot) log/i.test(msg) || /detached/i.test(msg)) {
          setNotice(
            boot
              ? "No boot log for this machine."
              : "This machine has no console log — it wasn't started detached, or its state was cleared.",
          );
        } else {
          term.write(`\x1b[38;5;203m${msg}\x1b[0m\r\n`);
        }
      }

      if (follow && !boot) {
        unlisten = await onLogLine((p) => {
          // p.line is a chunk that already contains its newlines (convertEol
          // turns \n into \r\n). Writing per-chunk avoids event-flood freezes.
          if (p.id === machineId && term) term.write(p.line);
        });
        await api.startLogStream(machineId).catch(() => {});
        streaming = true;
      }
    })();

    return () => {
      disposed = true;
      ro?.disconnect();
      unlisten?.();
      if (streaming) api.stopLogStream(machineId).catch(() => {});
      termRef.current = null;
      searchRef.current = null;
      term?.dispose();
    };
  }, [machineId, boot, follow]);

  const find = (dir: "next" | "prev") => {
    const s = searchRef.current;
    if (!s || !query) return;
    const opts = { caseSensitive: false, decorations: undefined } as const;
    if (dir === "next") s.findNext(query, opts);
    else s.findPrevious(query, opts);
  };

  // Search-as-you-type: match incrementally on every keystroke (no Enter).
  useEffect(() => {
    const s = searchRef.current;
    if (!s) return;
    if (!query) {
      s.clearDecorations?.();
      return;
    }
    s.findNext(query, { caseSensitive: false, incremental: true });
  }, [query]);

  const copySelection = () => {
    const t = termRef.current;
    if (t?.hasSelection()) {
      navigator.clipboard?.writeText(t.getSelection());
      toast("info", "Copied selection");
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-2 border-b border-white/10 px-3 py-1.5">
        <Input
          ref={searchInputRef}
          size="sm"
          radius="lg"
          value={query}
          onValueChange={setQuery}
          onKeyDown={(e) => {
            if (e.key === "Enter") find(e.shiftKey ? "prev" : "next");
          }}
          placeholder="Search logs…  (⌘F)"
          startContent={<IconSearch size={14} className="text-foreground-400" />}
          endContent={
            query ? (
              <button onClick={() => setQuery("")} className="text-foreground-400">
                <IconX size={14} />
              </button>
            ) : null
          }
          className="max-w-56"
          classNames={{ inputWrapper: "h-7 min-h-7 bg-content2/60 border border-white/10" }}
        />
        <Button isIconOnly size="sm" variant="light" isDisabled={!query} onPress={() => find("prev")}>
          <IconChevronUp size={16} />
        </Button>
        <Button isIconOnly size="sm" variant="light" isDisabled={!query} onPress={() => find("next")}>
          <IconChevronDown size={16} />
        </Button>

        {hasSelection && (
          <Tooltip content="Copy selection (⌘C)" placement="bottom">
            <Button
              size="sm"
              variant="flat"
              startContent={<IconCopy size={14} />}
              onPress={copySelection}
            >
              Copy
            </Button>
          </Tooltip>
        )}

        <div className="flex-1" />

        <label className="flex items-center gap-2 text-xs text-foreground-400">
          <Switch size="sm" isSelected={follow} onValueChange={setFollow} isDisabled={boot} />
          Follow
        </label>
        <label className="flex items-center gap-2 text-xs text-foreground-400">
          <Switch size="sm" isSelected={boot} onValueChange={setBoot} />
          Boot log
        </label>
      </div>

      <div className="relative min-h-0 flex-1">
        <div ref={hostRef} className="term-host p-2" />
        {notice && (
          <div className="pointer-events-none absolute inset-0 grid place-items-center p-6 text-center text-xs text-foreground-500">
            {notice}
          </div>
        )}
      </div>
    </div>
  );
}
