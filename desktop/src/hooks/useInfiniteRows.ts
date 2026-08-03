import { useEffect, useRef, useState } from "react";

/** Nearest scrollable ancestor (the ViewShell scroll container). */
function scrollParent(el: HTMLElement | null): HTMLElement | null {
  let node = el?.parentElement || null;
  while (node) {
    const oy = getComputedStyle(node).overflowY;
    if (oy === "auto" || oy === "scroll") return node;
    node = node.parentElement;
  }
  return null;
}

/**
 * Client-side infinite scroll: the data arrives all at once (from the CLI), but
 * we only render a growing window of it so large lists (hundreds of machines)
 * stay cheap. Reveals `step` more rows whenever the sentinel scrolls into view.
 *
 * Returns the number of rows to render, a `sentinelRef` to place after the list,
 * and `hasMore`. Render the sentinel only while `hasMore` is true.
 */
export function useInfiniteRows(total: number, step = 40) {
  const [count, setCount] = useState(step);
  const sentinelRef = useRef<HTMLDivElement>(null);

  const visible = Math.min(count, total);
  const hasMore = visible < total;

  useEffect(() => {
    const el = sentinelRef.current;
    if (!el || !hasMore) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) {
          setCount((c) => c + step);
        }
      },
      { root: scrollParent(el), rootMargin: "300px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [hasMore, step, total]);

  return { visible, sentinelRef, hasMore };
}
