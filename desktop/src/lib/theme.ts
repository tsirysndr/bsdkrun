import { useAtomValue } from "jotai";
import { themeAtom } from "../state/atoms";

/**
 * The one place per-theme colors live for the parts HeroUI cannot style:
 * xterm terminals (a canvas — CSS variables never reach it) and the SVG
 * content loaders. Everything else follows the theme through HeroUI's slots.
 *
 * The Night Rider values come from the VSCode theme's own JSON where it has
 * them (backgrounds, cursor, selection). It defines no terminal palette, so
 * the ANSI colors are derived from its token accents — the same pink, purple,
 * blue, teal and yellow the editor colors use — rather than invented.
 */

export interface UiTheme {
  /** xterm theme for interactive terminals. */
  term: {
    background: string;
    foreground: string;
    cursor: string;
    cursorAccent: string;
    selectionBackground: string;
    black: string;
    red: string;
    green: string;
    yellow: string;
    blue: string;
    magenta: string;
    cyan: string;
    white: string;
    brightBlack: string;
    brightRed: string;
    brightGreen: string;
    brightYellow: string;
  };
  /** xterm theme for read-only log views (brighter foreground, no cursor). */
  logTerm: {
    background: string;
    foreground: string;
    cursor: string;
    selectionBackground: string;
  };
  /** The class for opaque terminal-backed surfaces (panels, fullscreen). */
  surface: string;
  /** react-content-loader shimmer colors. */
  skeleton: { bg: string; fg: string };
}

const dark: UiTheme = {
  term: {
    background: "#0a0d13",
    foreground: "#dfe4ee",
    cursor: "#7c8bff",
    cursorAccent: "#0a0d13",
    selectionBackground: "rgba(124,139,255,0.35)",
    black: "#11151c",
    red: "#ff6b6b",
    green: "#7ee787",
    yellow: "#f0c674",
    blue: "#79b8ff",
    magenta: "#c398ff",
    cyan: "#66d9e8",
    white: "#c9d1d9",
    brightBlack: "#4b5262",
    brightRed: "#ff8585",
    brightGreen: "#a3f7b5",
    brightYellow: "#ffd479",
  },
  logTerm: {
    background: "#0a0d13",
    foreground: "#e8ecf5",
    cursor: "#0a0d13",
    selectionBackground: "rgba(124,139,255,0.35)",
  },
  surface: "bg-[#0a0d13]",
  skeleton: { bg: "#1b2130", fg: "#2b3446" },
};

const nightRider: UiTheme = {
  term: {
    background: "#171530",
    foreground: "#C9CBDB",
    cursor: "#7EA7FB",
    cursorAccent: "#171530",
    selectionBackground: "rgba(93,64,137,0.55)",
    black: "#1A1837",
    red: "#FF709D",
    green: "#55F0D7",
    yellow: "#FFDB7F",
    blue: "#7DA7FF",
    magenta: "#e591ff",
    cyan: "#71E4FE",
    white: "#C9CBDB",
    brightBlack: "#696292",
    brightRed: "#ff8fb1",
    brightGreen: "#8ff5e3",
    brightYellow: "#ffe6a3",
  },
  logTerm: {
    background: "#171530",
    foreground: "#DCDEF0",
    cursor: "#171530",
    selectionBackground: "rgba(93,64,137,0.55)",
  },
  surface: "bg-[#171530]",
  skeleton: { bg: "#26234E", fg: "#38356A" },
};

export const UI_THEMES: Record<"dark" | "night-rider", UiTheme> = {
  dark,
  "night-rider": nightRider,
};

/** The active theme's non-CSS colors. */
export function useUiTheme(): UiTheme {
  return UI_THEMES[useAtomValue(themeAtom)];
}
