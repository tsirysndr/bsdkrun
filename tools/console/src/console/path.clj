(ns console.path
  "Repo-root discovery. Lives in its own namespace so every other
  `console.*` namespace can depend on it without forming a cycle."
  (:require [babashka.fs :as fs]))

(defn repo-root
  "Walk up from cwd until we find the bsdkrun monorepo root, identified by a
  `Makefile` next to the workspace `Cargo.toml` — the one place in the tree
  both exist side by side (every other `Cargo.toml` here — core/, agent/,
  daemon/, supervisor/ — has no sibling Makefile)."
  []
  (loop [dir (fs/absolutize (fs/cwd))]
    (cond
      (nil? dir)
      (throw (ex-info "Could not locate bsdkrun repo root" {:cwd (str (fs/cwd))}))

      (and (fs/exists? (fs/path dir "Makefile"))
           (fs/exists? (fs/path dir "Cargo.toml")))
      (str dir)

      :else (recur (fs/parent dir)))))
