(ns bsdkrun.sandbox
  "The machine lifecycle. A \"sandbox\" here is just a plain map, e.g.
  `{:id \"abc123\" :ssh-port 2222}` — there is no object, no class. Create
  one with [[create!]], reconnect with [[get]], or enumerate with [[list]].

  Every function below that acts on a machine takes a `ref` first: a sandbox
  map, or a bare machine id/name string (see [[id]]) — `bsdkrun` itself
  resolves a bare id prefix or exact name (`core/src/db.rs`'s
  `find_machine`), so `(sandbox/stop! \"web-1\")` needs no lookup first. And
  since every `ref`-taking function returns either its result or (for
  lifecycle ops with nothing interesting to return) `ref` itself, they thread
  with `->`/`doto`:

  ```clojure
  (-> (sandbox/get \"web-1\")
      sandbox/start!
      (sandbox/exec! [\"uname\" \"-a\"])
      :stdout)

  (doto (sandbox/get \"web-1\")   ; same vm through every step, vm back at the end
    sandbox/start!
    (sandbox/exec! [\"setup.sh\"])
    sandbox/stop!)
  ```

  Mirrors `sdk/ruby/lib/bsdkrun/sandbox.rb`."
  (:refer-clojure :exclude [get list])
  (:require [clojure.string :as str]
            [bsdkrun.args :as args]
            [bsdkrun.errors :as errors]
            [bsdkrun.process :as process]
            [bsdkrun.types :as types]
            [bsdkrun.util :as util]))

(def ^:private id-re #"^[0-9a-f]{6,}$")
(def ^:private ssh-port-re #"ssh -p (\d+)")

(defn id
  "The machine id/name to hand the CLI for `ref` — a sandbox map's `:id`, or
  a bare id/name string, unchanged. Every function below that acts on a
  machine accepts either."
  [ref]
  (if (map? ref) (:id ref) ref))

(defn with-volume
  "Set the persistent volume a machine's rootfs is cloned onto/reused from
  (`-v`/`--volume`) on a *create-options* map — `:volume` is a create-time
  choice, fixed for the machine's lifetime, so this composes with `->`
  before [[create!]], not after it:

  ```clojure
  (-> {:os :linux :image \"alpine\"}
      (sandbox/with-volume \"web\")
      sandbox/create!)
  ```"
  [opts volume]
  (assoc opts :volume volume))

(defn with-network
  "Join a *create-options* map's guest to a global network (`--network`) —
  merges into `:net` rather than replacing it, so it composes with other
  `:net` keys (e.g. `:ports`, `:mac`) already set on `opts`. Composes with
  `->` before [[create!]]:

  ```clojure
  (-> {:os :linux :image \"alpine\"}
      (sandbox/with-network \"devnet\")
      sandbox/create!)
  ```

  To move an *existing* machine between networks, use [[connect-network!]]
  instead (applies on its next [[start!]])."
  [opts network]
  (assoc-in opts [:net :network] network))

(defn create!
  "Boot a new microVM and return `{:id ... :ssh-port ...}` (`:ssh-port` is
  nil unless the boot banner reported one, which only BSD guests do).

  `opts` is a create-options map discriminated on `:os` — see
  `bsdkrun.args/build-create-args`.

  Throws `errors/command-failed` if boot fails or no machine id is printed."
  [opts]
  (let [argv (args/build-create-args opts)
        res (process/run argv {:log-level (:log-level opts 1)})]
    (when-not (zero? (:exit-code res))
      (throw (errors/command-failed (assoc res :command "bsdkrun create"))))
    ;; Detached runs print just the machine id on stdout.
    (let [id (->> (str/split (:stdout res) #"\n")
                  (map str/trim)
                  (filter #(re-matches id-re %))
                  last)]
      (when-not id
        (throw (errors/command-failed
                (assoc res :command "bsdkrun create (no machine id in output)"))))
      (let [m (re-find ssh-port-re (:stderr res))]
        {:id id :ssh-port (when m (Long/parseLong (second m)))}))))

(defn list
  "List machines. `{:all true}` includes exited ones (default running only).

  Returns a vector of sandbox-info maps (see
  `bsdkrun.types/sandbox-info-from-row`)."
  ([] (list {}))
  ([opts]
   (let [argv (cond-> ["ps" "--json"] (:all opts) (conj "--all"))
         res (process/run! argv {:label "bsdkrun ps"})]
     (mapv types/sandbox-info-from-row (types/read-json-rows (:stdout res))))))

(defn- match-ref
  "Find the row in `rows` (sandbox-info maps) matching `ref-str` — exact name
  first (unambiguous, like `docker <name>`), then exact id, then a unique id
  prefix (Docker-style short ids). Mirrors `core/src/db.rs`'s
  `find_machine`."
  [rows ref-str]
  (or (some #(when (= (:name %) ref-str) %) rows)
      (some #(when (= (:id %) ref-str) %) rows)
      (some #(when (str/starts-with? (:id %) ref-str) %) rows)))

(defn get
  "Reconnect to an existing machine by id (a unique prefix is enough) or by
  exact name. `ref` is a sandbox map or a bare id/name string (see [[id]]).

  Throws `errors/sandbox-not-found` if nothing matches."
  [ref]
  (let [ref-str (id ref)
        found (match-ref (list {:all true}) ref-str)]
    (if found
      {:id (:id found)}
      (throw (errors/sandbox-not-found ref-str)))))

(defn exec!
  "Run a command in the guest through its exec agent.

  `command` may be a vector (argv, no shell parsing) or a bare string
  program name; with a string, `:args` supplies its arguments.

  `opts`:
    `:args`            arguments when `command` is a bare string
    `:env`             environment variables map (`-e K=V`)
    `:tty`             allocate a pseudo-TTY in the guest (`-t`)
    `:stdin`           data piped to the command's stdin
    `:cwd`             working directory, emulated via `sh -c 'cd ...'`
    `:throw-on-error`  throw `errors/command-failed` on a non-zero exit
    `:log-level`       per-command bsdkrun log level
    `:on-stdout`       called with each stdout byte-array chunk as it arrives
    `:on-stderr`       called with each stderr byte-array chunk as it arrives

  Returns `{:stdout ... :stderr ... :exit-code ... :command \"...\"}`."
  ([ref command] (exec! ref command {}))
  ([ref command {:keys [args env tty stdin cwd throw-on-error log-level on-stdout on-stderr]
                 :or {args [] env {} tty false throw-on-error false log-level 0}}]
   (let [argv0 (if (sequential? command) (vec command) (into [command] args))
         argv (if cwd
                (into ["/bin/sh" "-c" "cd \"$1\" && shift && exec \"$@\"" "sh" cwd] argv0)
                argv0)
         cli (cond-> ["exec"] tty (conj "-t"))
         cli (into cli (mapcat (fn [[k v]] ["-e" (str (name k) "=" v)]) env))
         cli (into cli (into [(id ref)] argv))
         res (process/run cli {:stdin stdin :log-level log-level
                               :on-stdout on-stdout :on-stderr on-stderr})
         result {:stdout (:stdout res)
                 :stderr (:stderr res)
                 :exit-code (:exit-code res)
                 :command (str "exec " (str/join " " argv))}]
     (if throw-on-error
       (types/throw-if-failed! result)
       result))))

(defn run-command!
  "Vercel-Sandbox-style alias for [[exec!]]: a program plus its args."
  ([ref command] (run-command! ref command [] {}))
  ([ref command args] (run-command! ref command args {}))
  ([ref command args opts]
   (exec! ref command (assoc opts :args args))))

(defn logs
  "Read the machine's console log. `{:boot true}` shows bsdkrun's own boot
  log instead of the console."
  ([ref] (logs ref {}))
  ([ref {:keys [boot]}]
   (let [argv (cond-> ["logs"] boot (conj "--boot"))
         argv (conj argv (id ref))]
     (:stdout (process/run argv)))))

(defn shell!
  "Attach an interactive shell to the machine (inherits the terminal).
  Returns true if the shell exited zero."
  [ref]
  (process/spawn-interactive! ["shell" (id ref)]))

(defn status
  "This machine's current status row, or nil if it's gone. `ref` may be a
  sandbox map or a bare id/name string."
  [ref]
  (match-ref (list {:all true}) (id ref)))

(defn running?
  "Whether the machine is currently running."
  [ref]
  (boolean (:running (status ref))))

(defn- lifecycle!
  "Run a fire-and-forget lifecycle CLI command, throwing on failure. Returns
  `ref` unchanged (not its result) so lifecycle calls compose with
  `->`/`doto` — see the namespace docstring."
  [ref argv label]
  (process/run! argv {:label label})
  ref)

(defn stop!
  "Stop the machine. BSD guests are cleanly powered off; Linux is SIGTERM'd."
  [ref]
  (lifecycle! ref ["stop" (id ref)] "bsdkrun stop"))

(defn start!
  "Restart a stopped machine in place (same id, disk/rootfs). Boots detached."
  [ref]
  (lifecycle! ref ["start" (id ref)] "bsdkrun start"))

(defn remove!
  "Remove the machine and its state. `{:force true}` stops it first if
  running."
  ([ref] (remove! ref {}))
  ([ref {:keys [force]}]
   (let [argv (cond-> ["rm"] force (conj "--force"))
         argv (conj argv (id ref))]
     (lifecycle! ref argv "bsdkrun rm"))))

(defn update!
  "Change the recorded vCPU / RAM. Applies on the next [[start!]]."
  ([ref] (update! ref {}))
  ([ref {:keys [cpus mem]}]
   (let [argv (cond-> ["update" (id ref)]
                (some? cpus) (into ["--cpus" (str cpus)])
                (some? mem) (into ["--mem" (str mem)]))]
     (lifecycle! ref argv "bsdkrun update"))))

(defn connect-network!
  "Join or switch this machine to a global network. Applies on next
  [[start!]]."
  [ref network]
  (lifecycle! ref ["network" "connect" (id ref) network] "bsdkrun network connect"))

(defn disconnect-network!
  "Detach this machine from its network. Applies on next [[start!]]."
  [ref]
  (lifecycle! ref ["network" "disconnect" (id ref)] "bsdkrun network disconnect"))

(defn- agent!
  "Run an in-guest agent CLI family (`ssh`, `tailscale`), throwing on
  failure."
  [ref family action {:keys [env] :or {env {}}}]
  (let [res (process/run (into [family (id ref)] action) {:env env})
        result {:stdout (:stdout res)
                :stderr (:stderr res)
                :exit-code (:exit-code res)
                :command (str family " " (str/join " " action))}]
    (types/throw-if-failed! result)))

(defn ssh-setup!
  "Install SSH keys in the guest via the agent (`ssh setup`). With no keys,
  the CLI installs your local `~/.ssh/*.pub`.

  `opts`: `:user` (target user, default root), `:key` (a literal key or
  `.pub` path, or a vector of them)."
  ([ref] (ssh-setup! ref {}))
  ([ref {:keys [user key]}]
   (let [action (cond-> ["setup"] user (into ["--user" user]))
         action (into action (mapcat (fn [k] ["--key" k]) (util/as-seq key)))]
     (agent! ref "ssh" action {}))))

(defn tailscale-up!
  "Put the guest on your tailnet (`tailscale setup`).

  `opts`: `:authkey` (tailnet auth key, sent as `TS_AUTHKEY`), `:hostname`
  (machine name on the tailnet), `:args` (extra args passed through to
  `tailscale up`)."
  ([ref] (tailscale-up! ref {}))
  ([ref {:keys [authkey hostname args]}]
   (let [action (cond-> ["setup"] hostname (into ["--hostname" hostname]))
         action (into action (or args []))]
     (agent! ref "tailscale" action {:env (if authkey {"TS_AUTHKEY" authkey} {})}))))
