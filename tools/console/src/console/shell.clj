(ns console.shell
  "Shell-out helpers shared by every command wrapper.

  Three flavors:
    `sh`   — inherit stdio (you see live output, exit-code returned)
    `sh!`  — capture (returns {:out :err :exit}), throws on non-zero
    `sh*`  — background (returns a process handle you can deref)

  Every call defaults `:dir` to the repo root, so a wrapper works no matter
  where the REPL/`bb` invocation started — pass an explicit `:dir` (relative
  to the repo root, e.g. \"sdk/ruby\") to run inside a subproject instead."
  (:require [babashka.fs :as fs]
            [babashka.process :as p]
            [clojure.string :as str]
            [console.path :as path]))

(defn repo-root
  "Re-exported for callers that want the repo root."
  []
  (path/repo-root))

(defn- resolve-dir [dir]
  (if dir
    (str (fs/path (path/repo-root) dir))
    (path/repo-root)))

(defn- in-repo [opts]
  (merge {:inherit true} opts {:dir (resolve-dir (:dir opts))}))

(defn- argv [cmd]
  (cond
    (vector? cmd) cmd
    (string? cmd) (str/split cmd #"\s+")
    :else (throw (ex-info "cmd must be a string or a vector" {:cmd cmd}))))

(defn sh
  "Run a command with inherited stdio. Returns the exit code."
  ([cmd] (sh cmd {}))
  ([cmd opts]
   (:exit @(p/process (argv cmd) (in-repo opts)))))

(defn sh!
  "Like `sh` but captures stdout/stderr and throws on non-zero exit."
  ([cmd] (sh! cmd {}))
  ([cmd opts]
   (let [opts (merge {:out :string :err :string} opts {:dir (resolve-dir (:dir opts))})]
     @(p/process (argv cmd) opts))))

(defn sh*
  "Run in the background. Returns a process handle (deref for exit info,
  `(.destroyForcibly (:proc handle))` to kill it)."
  ([cmd] (sh* cmd {}))
  ([cmd opts]
   (p/process (argv cmd) (in-repo opts))))
