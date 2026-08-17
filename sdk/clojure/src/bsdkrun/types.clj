(ns bsdkrun.types
  "Pure functions turning a parsed CLI JSON row (a Clojure map with *string*
  keys, as `clojure.data.json/read-str` produces without `:key-fn keyword` —
  the CLI's JSON is already snake_case) into a kebab-case-keyword map. One
  function per row shape, mirroring `sdk/ruby/lib/bsdkrun/types.rb`.

  Also provides the `Result`-map helpers: since a bsdkrun.sandbox exec/agent
  result here is a plain map (`{:stdout ... :stderr ... :exit-code ...
  :command ...}`), not an object with methods, [[ok?]], [[text]], [[json]],
  [[lines]] and [[throw-if-failed!]] are given as functions operating on that
  map instead."
  (:require [clojure.data.json :as json]
            [clojure.string :as str]
            [bsdkrun.errors :as errors]))

(defn- to-int
  "Coerce a JSON-decoded value to a long, defaulting to 0 for nil/other —
  mirrors Ruby's `nil.to_i => 0` / `Integer#to_i` / `Float#to_i` coercions
  used throughout `types.rb`."
  [v]
  (cond
    (nil? v) 0
    (number? v) (long v)
    (string? v) (or (some-> (re-find #"^-?\d+" v) Long/parseLong) 0)
    :else 0))

(defn- to-int-or-nil
  [v]
  (when (some? v) (to-int v)))

(defn read-json-rows
  "Parse `s` (raw CLI stdout) as a JSON array, defaulting to `[]` for empty
  output — mirrors every `list`-style call site's
  `JSON.parse(stdout.empty? ? \"[]\" : stdout)`."
  [s]
  (if (seq s) (json/read-str s) []))

(defn port-forward-from-row
  "A host<->guest TCP port forward, as reported nested under `ports` in a
  `ps --json` row."
  [row]
  {:bind (str (get row "bind"))
   :host (to-int (get row "host"))
   :guest (to-int (get row "guest"))})

(defn sandbox-info-from-row
  "A machine as reported by `bsdkrun ps --json`."
  [row]
  (let [running (boolean (get row "running"))]
    {:id (str (get row "id"))
     :name (get row "name")
     :image (str (get row "image"))
     :kind (str (get row "kind"))
     :command (str (or (get row "command") ""))
     :status (if running "running" "exited")
     :running running
     :exit-code (to-int-or-nil (get row "exit_code"))
     :pid (to-int-or-nil (get row "pid"))
     :detached (boolean (get row "detached"))
     :cpus (to-int (get row "cpus"))
     :mem (to-int (get row "mem"))
     :volume (get row "volume")
     :state-dir (str (get row "state_dir"))
     :network (get row "network")
     :net-ip (get row "net_ip")
     :created-at (to-int (get row "created_at"))
     :finished-at (to-int-or-nil (get row "finished_at"))
     :ports (mapv port-forward-from-row (or (get row "ports") []))}))

(defn image-info-from-row
  "An image as reported by `bsdkrun images --json`."
  [row]
  {:id (str (get row "id"))
   :reference (str (get row "reference"))
   :digest (str (get row "digest"))
   :size (to-int (get row "size"))
   :rootfs (str (get row "rootfs"))
   :created-at (to-int (get row "created_at"))})

(defn volume-info-from-row
  "A volume as reported by `bsdkrun volume ls --json`."
  [row]
  (let [created (get row "created_at")]
    {:name (str (get row "name"))
     :guest (get row "guest")
     :base (get row "base")
     :path (str (get row "path"))
     :size (str (get row "size"))
     :created-at (when (some? created) (to-int created))
     :tracked (boolean (get row "tracked"))}))

(defn network-info-from-row
  "A global network as reported by `bsdkrun network ls --json`."
  [row]
  (let [created (get row "created_at")]
    {:name (str (get row "name"))
     :subnet (str (get row "subnet"))
     :gateway (str (get row "gateway"))
     :members (to-int (or (get row "members") 0))
     :running (to-int (or (get row "running") 0))
     :up (boolean (get row "up"))
     :created-at (when (some? created) (to-int created))}))

(defn sandbox-info-from-graphql
  "A machine as reported by the daemon's GraphQL API — a `Machine` object
  decoded straight from JSON (camelCase string keys), unlike
  [[sandbox-info-from-row]]'s snake_case CLI rows. Same kebab-case-keyword
  shape either way, so callers of `bsdkrun.sandbox` and `bsdkrun.client` see
  identical `SandboxInfo` maps regardless of transport.

  `m` is a `MACHINE_FIELDS`-shaped map (see `bsdkrun.client`), e.g. from
  `bsdkrun.client/list-machines` or `bsdkrun.client/get-machine`."
  [m]
  (let [running (boolean (get m "running"))]
    {:id (str (get m "id"))
     :name (get m "name")
     :image (str (get m "image"))
     :kind (str (get m "kind"))
     :command (str (or (get m "command") ""))
     :status (str (get m "status"))
     :running running
     :exit-code (to-int-or-nil (get m "exitCode"))
     :pid (to-int-or-nil (get m "pid"))
     :detached (boolean (get m "detached"))
     :cpus (to-int (get m "cpus"))
     :mem (to-int (get m "mem"))
     :volume (get m "volume")
     :state-dir (str (get m "stateDir"))
     :network (get m "network")
     :net-ip (get m "netIp")
     :created-at (to-int (get m "createdAt"))
     :finished-at (to-int-or-nil (get m "finishedAt"))
     :origin (get m "origin")
     :ports (mapv port-forward-from-row (or (get m "ports") []))}))

(defn snapshot-info-from-graphql
  "A machine snapshot as reported by the daemon's GraphQL API.

  A snapshot is a copy-on-write clone of one machine's disk state, not a
  memory image — the files the guest wrote, not what it was executing.
  `bsdkrun.client/branch!` boots a new machine from one;
  `bsdkrun.client/restore!` puts one back over the machine it came from."
  [s]
  {:id (str (get s "id"))
   :name (str (get s "name"))
   :machine-id (str (or (get s "machineId") ""))
   :machine-name (str (or (get s "machineName") ""))
   :kind (str (or (get s "kind") ""))
   :image (str (or (get s "image") ""))
   :path (str (or (get s "path") ""))
   :parent (get s "parent")
   :description (str (or (get s "description") ""))
   :cpus (to-int (get s "cpus"))
   :mem (to-int (get s "mem"))
   :ports (mapv port-forward-from-row (or (get s "ports") []))
   :size (get s "size")
   :created-at (to-int (get s "createdAt"))})

(defn command-result-from-graphql
  "The outcome of a `bsdkrun.client` lifecycle mutation (`stopMachine`,
  `startMachine`, `removeMachines`, `updateMachine`, `commitMachine`, ...). A
  non-zero `:exit-code` is a value to inspect, not necessarily a failure —
  mirrors `daemon/src/graphql.rs`'s `CommandResult`."
  [r]
  {:exit-code (to-int (get r "exitCode"))
   :stdout (str (or (get r "stdout") ""))
   :stderr (str (or (get r "stderr") ""))})

(defn shell-session-info-from-graphql
  "An open interactive shell session, as reported by `openShell` / the
  `shellSessions` query. Mirrors `daemon/src/graphql.rs`'s
  `ShellSessionInfo`."
  [s]
  {:id (str (get s "id"))
   :machine-id (str (get s "machineId"))
   :finished (boolean (get s "finished"))
   :truncated (boolean (get s "truncated"))})

;; --- Result-map helpers -----------------------------------------------

(defn ok?
  "Whether a Result map succeeded (exit 0)."
  [result]
  (zero? (:exit-code result)))

(defn text
  "A Result's stdout with trailing newlines trimmed — the common case."
  [result]
  (str/replace (:stdout result) #"\n+\z" ""))

(defn json
  "A Result's stdout, parsed as JSON."
  [result]
  (json/read-str (:stdout result)))

(defn lines
  "A Result's non-empty stdout lines."
  [result]
  (->> (str/split (:stdout result) #"\n")
       (remove empty?)))

(defn throw-if-failed!
  "Throw `errors/command-failed` if `result` exited non-zero; otherwise
  return it unchanged."
  [result]
  (if (ok? result)
    result
    (throw (errors/command-failed result))))
