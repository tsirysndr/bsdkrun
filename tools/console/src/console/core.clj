(ns console.core
  "bsdkrun console — a centralized REPL for every operational command in the
  monorepo (the CLI/daemon/supervisor build cycle, all six SDKs, web, and
  desktop).

  Quick start (REPL):
      (require '[console.core :as c])
      (c/help)   ;; or (c/ls)

  Or as a one-shot:
      $ bb help
      $ bb release
      $ bb sdk:test clojure"
  (:require [console.shell :as sh]))

(def ^:private registry
  "Hand-written index of every command grouped by namespace. Keeps `(help)`
  cheap and discoverable — namespaces are still loaded lazily."
  [{:group "build" :ns 'console.build
    :cmds [[:build          "make build — debug build + codesign"]
           [:release        "make release — release build + codesign"]
           [:sign           "make sign — recodesign the debug binaries"]
           [:sign-release   "make sign-release — recodesign the release binaries"]
           [:web            "make web — build the web SPA into web/dist"]
           [:daemon         "make daemon — release build bsdkrun-daemon + -supervisor"]
           [:agent          "make agent — cross-compile the in-guest exec agent"]
           [:agent-linux    "make agent-linux"]
           [:agent-freebsd  "make agent-freebsd"]
           [:agent-netbsd   "make agent-netbsd"]
           [:run            "make run ARGS=... — build then run. Args: forwarded to bsdkrun"]
           [:test           "make test (= make e2e) — boot the FreeBSD image under a PTY"]
           [:clean          "make clean — cargo clean"]]}

   {:group "sdk" :ns 'console.sdk
    :cmds [[:deps        "Fetch deps. lang (atom) ∈ :elixir :gleam. Usage: (deps :elixir)"]
           [:test        "Run one SDK's tests. lang ∈ :clojure :ruby :python :elixir :gleam :typescript :go :rust :scala. Usage: (test :clojure)"]
           [:lint        "Lint. lang ∈ :python. Usage: (lint :python)"]
           [:build       "Build the distributable artifact. lang ∈ :clojure :ruby :python :typescript :scala. Usage: (build :clojure)"]
           [:install     "Install locally. lang ∈ :clojure (~/.m2). Usage: (install :clojure)"]
           [:publish     "Publish to the registry. lang ∈ :clojure :ruby :python :typescript :elixir :gleam :rust :scala. Usage: (publish :clojure)"]
           [:test-all    "Every SDK's unit-test suite in turn"]
           [:dir         "The repo-root-relative dir for lang. Usage: (dir :ruby)"]]}

   {:group "web" :ns 'console.web
    :cmds [[:dev        "bun run dev — Vite dev server"]
           [:build      "bun run build — into web/dist"]
           [:typecheck  "bun run typecheck"]
           [:preview    "bun run preview"]]}

   {:group "desktop" :ns 'console.desktop
    :cmds [[:dev          "bun run dev — renderer only"]
           [:build        "bun run build — renderer only"]
           [:tauri-dev    "bun run tauri dev — full native app shell"]
           [:tauri-build  "bun run tauri build — distributable bundle"]
           [:tauri        "escape hatch: any `tauri` subcommand. Args: subcommand [args...]"]]}])

(defn- pad [s n] (let [s (str s)] (str s (apply str (repeat (max 0 (- n (count s))) " ")))))

(defn ls
  "Print every registered command, grouped by namespace, with a one-liner."
  []
  (doseq [{:keys [group ns cmds]} registry]
    (println)
    (println (str "── " group "  (" ns ") ──"))
    (doseq [[sym desc] cmds]
      (println " " (pad sym 12) "  " desc)))
  :ok)

(defn help
  "Pretty banner + ls. Use this from the REPL for a quick tour."
  []
  (println)
  (println "bsdkrun Console — REPL-driven ops for the whole monorepo")
  (println "    (require '[console.build :as build])")
  (println "    (build/release)")
  (println "    (require '[console.sdk :as sdk])")
  (println "    (sdk/test :clojure)   ;; lang is always a keyword (atom)")
  (println)
  (println "Commands:")
  (ls)
  (println)
  (println "From shell:   bb <task>     (see `bb tasks`)")
  (println "Repo root:   " (sh/repo-root))
  :ok)

(defn dispatch
  "Entry point for `clj -X console.core/dispatch :cmd :sdk/test :args [:clojure]`."
  [{:keys [cmd args] :or {args []}}]
  (let [[grp sym] ((juxt namespace name) cmd)
        ns-sym    (symbol (str "console." grp))]
    (require ns-sym)
    (let [f (ns-resolve ns-sym (symbol sym))]
      (when-not f
        (throw (ex-info (str "Unknown command: " cmd) {:cmd cmd})))
      (apply f args))))
