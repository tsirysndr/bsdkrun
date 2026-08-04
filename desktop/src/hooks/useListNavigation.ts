import { useEffect, useRef, useState } from "react";

type KeyHandlers<T> = {
  /** Enter (and, if provided, treated as the primary action). */
  onEnter?: (item: T) => void;
  /** Extra single-key actions, keyed by lowercased key (e.g. { t: openTerminal }). */
  keys?: Record<string, (item: T) => void>;
};

/**
 * Shared keyboard navigation for a list view: ↑/↓ highlight rows, Enter runs
 * the primary action, and optional single-key shortcuts act on the highlighted
 * row. Ignored while typing or when a dialog/drawer is open. Rows must carry a
 * `data-list-row={id}` attribute so the highlighted one can be scrolled in view.
 */
export function useListNavigation<T>(
  rows: T[],
  getId: (item: T) => string,
  handlers: KeyHandlers<T> = {},
) {
  const [focusedId, setFocusedId] = useState<string | null>(null);

  const ref = useRef({ rows, focusedId, handlers, getId });
  ref.current = { rows, focusedId, handlers, getId };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (
        t &&
        (t.tagName === "INPUT" ||
          t.tagName === "TEXTAREA" ||
          t.isContentEditable)
      )
        return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      if (document.querySelector('[role="dialog"]')) return; // a modal owns keys

      const { rows: list, focusedId: cur, handlers: h, getId: id } = ref.current;
      if (!list.length) return;
      const idx = list.findIndex((m) => id(m) === cur);

      if (e.key === "ArrowDown") {
        e.preventDefault();
        setFocusedId(id(list[idx < 0 ? 0 : Math.min(idx + 1, list.length - 1)]));
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        setFocusedId(id(list[idx < 0 ? list.length - 1 : Math.max(idx - 1, 0)]));
      } else if (e.key === "Enter") {
        const m = list.find((x) => id(x) === cur);
        if (m && h.onEnter) {
          e.preventDefault();
          h.onEnter(m);
        }
      } else {
        const fn = h.keys?.[e.key.toLowerCase()];
        const m = list.find((x) => id(x) === cur);
        if (fn && m) {
          e.preventDefault();
          fn(m);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Keep the highlighted row scrolled into view.
  useEffect(() => {
    if (!focusedId) return;
    document
      .querySelector(`[data-list-row="${focusedId}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [focusedId]);

  return { focusedId, setFocusedId };
}
