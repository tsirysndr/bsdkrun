import { useEffect } from "react";

const ATTRS: Record<string, string> = {
  autocomplete: "off",
  autocorrect: "off",
  autocapitalize: "off",
  spellcheck: "false",
  // Ask password managers (1Password / LastPass) to ignore these fields too.
  "data-1p-ignore": "true",
  "data-lpignore": "true",
  "data-form-type": "other",
};

function strip(el: Element) {
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    for (const [k, v] of Object.entries(ATTRS)) {
      if (el.getAttribute(k) !== v) el.setAttribute(k, v);
    }
  }
}

/**
 * Globally disable browser autocomplete / autofill / spellcheck on every text
 * field. HeroUI renders its own native inputs, so rather than thread props
 * through each component we stamp the attributes on mount and on any input that
 * gets added later (via a MutationObserver).
 */
export function useNoAutofill() {
  useEffect(() => {
    const scan = (root: ParentNode) =>
      root.querySelectorAll("input, textarea").forEach(strip);

    scan(document);
    const obs = new MutationObserver((records) => {
      for (const r of records) {
        r.addedNodes.forEach((n) => {
          if (n instanceof Element) {
            strip(n);
            scan(n);
          }
        });
      }
    });
    obs.observe(document.body, { childList: true, subtree: true });
    return () => obs.disconnect();
  }, []);
}
