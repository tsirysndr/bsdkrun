(ns bsdkrun.sandbox
  "The machine lifecycle. A \"sandbox\" here is just a plain map, e.g.
  `{:id \"abc123\" :ssh-port 2222}` — there is no object, no class. Create
  one with [[create!]], reconnect with [[get]], or enumerate with [[list]].

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

(defn get
  "Reconnect to an existing machine by id (a unique prefix is enough).

  Throws `errors/sandbox-not-found` if nothing matches."
  [id]
  (let [found (some (fn [m] (when (or (= (:id m) id) (str/starts-with? (:id m) id)) m))
                     (list {:all true}))]
    (if found
      {:id (:id found)}
      (throw (errors/sandbox-not-found id)))))

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

  Returns `{:stdout ... :stderr ... :exit-code ... :command \"...\"}`."
  ([sandbox command] (exec! sandbox command {}))
  ([sandbox command {:keys [args env tty stdin cwd throw-on-error log-level]
                      :or {args [] env {} tty false throw-on-error false log-level 0}}]
   (let [argv0 (if (sequential? command) (vec command) (into [command] args))
         argv (if cwd
                (into ["/bin/sh" "-c" "cd \"$1\" && shift && exec \"$@\"" "sh" cwd] argv0)
                argv0)
         cli (cond-> ["exec"] tty (conj "-t"))
         cli (into cli (mapcat (fn [[k v]] ["-e" (str (name k) "=" v)]) env))
         cli (into cli (into [(:id sandbox)] argv))
         res (process/run cli {:stdin stdin :log-level log-level})
         result {:stdout (:stdout res)
                 :stderr (:stderr res)
                 :exit-code (:exit-code res)
                 :command (str "exec " (str/join " " argv))}]
     (if throw-on-error
       (types/throw-if-failed! result)
       result))))

(defn run-command!
  "Vercel-Sandbox-style alias for [[exec!]]: a program plus its args."
  ([sandbox command] (run-command! sandbox command [] {}))
  ([sandbox command args] (run-command! sandbox command args {}))
  ([sandbox command args opts]
   (exec! sandbox command (assoc opts :args args))))

(defn logs
  "Read the machine's console log. `{:boot true}` shows bsdkrun's own boot
  log instead of the console."
  ([sandbox] (logs sandbox {}))
  ([sandbox {:keys [boot]}]
   (let [argv (cond-> ["logs"] boot (conj "--boot"))
         argv (conj argv (:id sandbox))]
     (:stdout (process/run argv)))))

(defn shell!
  "Attach an interactive shell to the machine (inherits the terminal).
  Returns true if the shell exited zero."
  [sandbox]
  (process/spawn-interactive! ["shell" (:id sandbox)]))

(defn status
  "This machine's current status row, or nil if it's gone."
  [sandbox]
  (some (fn [m] (when (= (:id m) (:id sandbox)) m)) (list {:all true})))

(defn running?
  "Whether the machine is currently running."
  [sandbox]
  (boolean (:running (status sandbox))))

(defn- lifecycle!
  "Run a fire-and-forget lifecycle CLI command, throwing on failure."
  [argv label]
  (process/run! argv {:label label})
  nil)

(defn stop!
  "Stop the machine. BSD guests are cleanly powered off; Linux is SIGTERM'd."
  [sandbox]
  (lifecycle! ["stop" (:id sandbox)] "bsdkrun stop"))

(defn start!
  "Restart a stopped machine in place (same id, disk/rootfs). Boots detached."
  [sandbox]
  (lifecycle! ["start" (:id sandbox)] "bsdkrun start"))

(defn remove!
  "Remove the machine and its state. `{:force true}` stops it first if
  running."
  ([sandbox] (remove! sandbox {}))
  ([sandbox {:keys [force]}]
   (let [argv (cond-> ["rm"] force (conj "--force"))
         argv (conj argv (:id sandbox))]
     (lifecycle! argv "bsdkrun rm"))))

(defn update!
  "Change the recorded vCPU / RAM. Applies on the next [[start!]]."
  ([sandbox] (update! sandbox {}))
  ([sandbox {:keys [cpus mem]}]
   (let [argv (cond-> ["update" (:id sandbox)]
                (some? cpus) (into ["--cpus" (str cpus)])
                (some? mem) (into ["--mem" (str mem)]))]
     (lifecycle! argv "bsdkrun update"))))

(defn connect-network!
  "Join or switch this machine to a global network. Applies on next
  [[start!]]."
  [sandbox network]
  (lifecycle! ["network" "connect" (:id sandbox) network] "bsdkrun network connect"))

(defn disconnect-network!
  "Detach this machine from its network. Applies on next [[start!]]."
  [sandbox]
  (lifecycle! ["network" "disconnect" (:id sandbox)] "bsdkrun network disconnect"))

(defn- agent!
  "Run an in-guest agent CLI family (`ssh`, `tailscale`), throwing on
  failure."
  [sandbox family action {:keys [env] :or {env {}}}]
  (let [res (process/run (into [family (:id sandbox)] action) {:env env})
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
  ([sandbox] (ssh-setup! sandbox {}))
  ([sandbox {:keys [user key]}]
   (let [action (cond-> ["setup"] user (into ["--user" user]))
         action (into action (mapcat (fn [k] ["--key" k]) (util/as-seq key)))]
     (agent! sandbox "ssh" action {}))))

(defn tailscale-up!
  "Put the guest on your tailnet (`tailscale setup`).

  `opts`: `:authkey` (tailnet auth key, sent as `TS_AUTHKEY`), `:hostname`
  (machine name on the tailnet), `:args` (extra args passed through to
  `tailscale up`)."
  ([sandbox] (tailscale-up! sandbox {}))
  ([sandbox {:keys [authkey hostname args]}]
   (let [action (cond-> ["setup"] hostname (into ["--hostname" hostname]))
         action (into action (or args []))]
     (agent! sandbox "tailscale" action {:env (if authkey {"TS_AUTHKEY" authkey} {})}))))
