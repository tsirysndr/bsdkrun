(ns console.desktop
  "The desktop app (desktop/) — a Tauri-based Docker-Desktop-style GUI. See
  `desktop/package.json` for the scripts these wrap."
  (:require [console.shell :as sh]))

(defn dev
  "`bun run dev` — Vite dev server (renderer only; use `tauri-dev` for the
  full native shell)."
  []
  (sh/sh ["bun" "run" "dev"] {:dir "desktop"}))

(defn build
  "`bun run build` — tsc + vite build of the renderer."
  []
  (sh/sh ["bun" "run" "build"] {:dir "desktop"}))

(defn tauri
  "`bun run tauri <args...>` — escape hatch for any Tauri CLI subcommand."
  [& args]
  (sh/sh (into ["bun" "run" "tauri"] args) {:dir "desktop"}))

(defn tauri-dev
  "`bun run tauri dev` — the full native app shell, live-reloading."
  []
  (tauri "dev"))

(defn tauri-build
  "`bun run tauri build` — a distributable native bundle."
  []
  (tauri "build"))
