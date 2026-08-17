import type { IDisposable, ILink, Terminal } from "@xterm/xterm";

/**
 * Make URLs printed by a guest clickable, using xterm's own
 * `registerLinkProvider`.
 *
 * Deliberately not `@xterm/addon-web-links`, which is a thin wrapper over this
 * same API: `web/node_modules` is a fixed-output nix derivation pinned by
 * `outputHash` per system (`flake.nix`), so adding a dependency silently gets
 * the *old* dependency tree until all three hashes are bumped — the build then
 * fails only in CI, as `TS2307: Cannot find module`. Keeping the terminal
 * panes on packages that are already pinned avoids that entirely.
 */

// Bounded by whitespace and the characters that cannot appear unencoded in a
// URL. `www.` is included because logs print bare hostnames constantly.
const URL_RE = /(https?:\/\/|www\.)[^\s"'`<>()[\]{}]+/g;

/** Trailing punctuation that belongs to the sentence, not the URL. */
const TRAILING = /[.,;:!?'"]+$/;

type Cell = { x: number; y: number };

/**
 * The full logical line containing row `y`, following xterm's wrap flags, plus
 * a map from string offset back to a screen cell.
 *
 * Wrapping has to be followed rather than ignored: these panes are narrow and
 * URLs are long, so a link that stopped at the row edge would almost never be
 * clickable in the case it is most needed.
 */
function logicalLine(
  term: Terminal,
  y: number,
): { text: string; cells: Cell[] } | null {
  const buf = term.buffer.active;
  let start = y;
  // Walk back to the first row of the wrapped group.
  while (start > 1 && buf.getLine(start - 1)?.isWrapped) start -= 1;

  let text = "";
  const cells: Cell[] = [];
  for (let row = start; row <= buf.length; row += 1) {
    const line = buf.getLine(row - 1);
    if (!line) break;
    if (row > start && !line.isWrapped) break;
    const s = line.translateToString(false);
    for (let i = 0; i < s.length; i += 1) {
      text += s[i];
      cells.push({ x: i + 1, y: row });
    }
  }
  return text ? { text, cells } : null;
}

/**
 * Register the provider. Returns a disposable, so a pane that tears its
 * terminal down does not leak one per remount.
 */
export function registerWebLinks(
  term: Terminal,
  open: (uri: string) => void,
): IDisposable {
  return term.registerLinkProvider({
    provideLinks(y, callback) {
      const line = logicalLine(term, y);
      if (!line) {
        callback(undefined);
        return;
      }

      const links: ILink[] = [];
      URL_RE.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = URL_RE.exec(line.text)) !== null) {
        const matched = m[0].replace(TRAILING, "");
        if (!matched) continue;
        const from = line.cells[m.index];
        const to = line.cells[m.index + matched.length - 1];
        if (!from || !to) continue;
        // Only offer links that touch the row being asked about — xterm calls
        // this per row, and returning the whole wrapped group each time would
        // register the same link several times over.
        if (y < from.y || y > to.y) continue;

        const uri = matched.startsWith("www.") ? `https://${matched}` : matched;
        links.push({
          text: matched,
          range: { start: { x: from.x, y: from.y }, end: { x: to.x, y: to.y } },
          activate: () => open(uri),
        });
      }
      callback(links.length ? links : undefined);
    },
  });
}
