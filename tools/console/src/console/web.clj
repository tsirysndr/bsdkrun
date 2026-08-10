(ns console.web
  "The web UI (web/) — a Docker-Desktop-style SPA talking to a bsdkrund
  GraphQL API. See `web/package.json` for the scripts these wrap."
  (:require [console.shell :as sh]))

(defn dev
  "`bun run dev` — Vite dev server."
  []
  (sh/sh ["bun" "run" "dev"] {:dir "web"}))

(defn build
  "`bun run build` — tsc + vite build into web/dist. `console.build/web`
  wraps this same script via the Makefile; use whichever you're already
  thinking in terms of."
  []
  (sh/sh ["bun" "run" "build"] {:dir "web"}))

(defn typecheck
  "`bun run typecheck` — tsc --noEmit."
  []
  (sh/sh ["bun" "run" "typecheck"] {:dir "web"}))

(defn preview
  "`bun run preview` — serve the built web/dist."
  []
  (sh/sh ["bun" "run" "preview"] {:dir "web"}))
