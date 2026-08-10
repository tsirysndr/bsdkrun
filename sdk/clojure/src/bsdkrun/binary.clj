(ns bsdkrun.binary
  "Locates the `bsdkrun` binary on the host and caches the result.

  Resolution order (first match wins):

    1. an explicit override set via [[set-override!]]
    2. the `BSDKRUN_BIN` environment variable
    3. `bsdkrun` on `PATH`
    4. an in-repo dev build: `<repo>/target/release/bsdkrun`, then
       `<repo>/target/debug/bsdkrun`

  [[candidates]] and [[resolve]] both take an optional opts map
  (`:override`, `:bsdkrun-bin`, `:path`, `:repo-root`) so the discovery logic
  is unit-testable without mutating real process/environment state — the JVM
  has no supported, portable way to change `System/getenv` for the current
  process, unlike Ruby's `ENV[...]=`. Any key left out of the map falls back
  to the real host state (the override atom, `$BSDKRUN_BIN`, `$PATH`, and the
  monorepo root inferred from this namespace's own source location)."
  (:refer-clojure :exclude [reset! resolve])
  (:require [clojure.java.io :as io]
            [clojure.string :as str]
            [bsdkrun.errors :as errors])
  (:import [java.io File]))

(def ^:private state (atom {:override nil :resolved nil}))

(defn set-override!
  "Force the SDK to use a specific `bsdkrun` binary, bypassing discovery."
  [path]
  (swap! state assoc :override path :resolved nil)
  nil)

(defn override
  "The current explicit override, if any."
  []
  (:override @state))

(defn reset!
  "Reset cached discovery state and any override (mainly for tests)."
  []
  (swap! state (constantly {:override nil :resolved nil}))
  nil)

(defn- path-like?
  [^String candidate]
  (str/includes? candidate File/separator))

(defn- executable-file?
  [^File f]
  (and (.isFile f) (.canExecute f)))

(defn- which
  "Cross-platform PATH lookup for an executable name. `path-env` is a raw
  `PATH`-style string (entries separated by `File/pathSeparator`); defaults
  to the real `$PATH`. Returns the resolved absolute path, or nil."
  ([name] (which name (System/getenv "PATH")))
  ([name path-env]
   (let [sep (java.util.regex.Pattern/quote File/pathSeparator)
         dirs (str/split (or path-env "") (re-pattern sep))]
     (some (fn [dir]
             (when (seq dir)
               (let [f (io/file dir name)]
                 (when (executable-file? f) (.getPath f)))))
           dirs))))

(defn- resource-dir
  "The directory containing this namespace's own source file on disk, or nil
  if it can't be resolved to a plain file (e.g. running from inside a jar)."
  []
  (when-let [res (io/resource "bsdkrun/binary.clj")]
    (when (= "file" (.getProtocol res))
      (.getParentFile (io/file (.toURI res))))))

(defn- repo-root-dir
  "The monorepo root, four directories up from this namespace's source
  directory: `sdk/clojure/src/bsdkrun` -> `sdk/clojure/src` -> `sdk/clojure`
  -> `sdk` -> repo root. Same depth Ruby's `binary.rb` walks up from
  `sdk/ruby/lib/bsdkrun`. Returns nil when [[resource-dir]] can't be
  determined."
  []
  (when-let [dir (resource-dir)]
    (-> ^File dir .getParentFile .getParentFile .getParentFile .getParentFile)))

(defn- default-opts
  []
  {:override (:override @state)
   :bsdkrun-bin (let [v (System/getenv "BSDKRUN_BIN")] (when (seq v) v))
   :path (System/getenv "PATH")
   :repo-root (repo-root-dir)})

(defn candidates
  "Candidate `bsdkrun` binary locations, in priority order, as a vector of
  path strings. A pure function of `opts` (see the namespace docstring for
  the supported keys and their real-host defaults)."
  ([] (candidates {}))
  ([opts]
   (let [{:keys [override bsdkrun-bin path repo-root]} (merge (default-opts) opts)
         on-path (which "bsdkrun" path)]
     (cond-> []
       override (conj override)
       bsdkrun-bin (conj bsdkrun-bin)
       on-path (conj on-path)
       repo-root (conj (.getPath (io/file repo-root "target" "release" "bsdkrun")))
       repo-root (conj (.getPath (io/file repo-root "target" "debug" "bsdkrun")))))))

(defn resolve
  "Resolve (and cache) the path to the `bsdkrun` binary.

  Throws `ex-info` (kind `:bsdkrun/binary-not-found`, see `bsdkrun.errors`)
  listing everything that was searched if nothing matched."
  ([] (resolve {}))
  ([opts]
   (or (:resolved @state)
       (let [merged (merge (default-opts) opts)
             searched (candidates merged)
             found (some (fn [candidate]
                            (if (path-like? candidate)
                              (when (.exists (io/file candidate)) candidate)
                              (which candidate (:path merged))))
                          searched)]
         (if found
           (do (swap! state assoc :resolved found) found)
           (throw (errors/binary-not-found searched)))))))
