(ns bsdkrun.client
  "A client that talks to a remote `bsdkrund` daemon's GraphQL API directly —
  `java.net.http.HttpClient` for queries/mutations, its built-in
  `java.net.http.WebSocket` speaking `graphql-transport-ws` for subscriptions
  — instead of shelling out to a local `bsdkrun` binary the way
  `bsdkrun.sandbox` does.

  The wire contract (URL/header shape, error mapping, subscription protocol,
  field names) is locked to match the other bsdkrun SDKs
  (TypeScript/Python/Ruby/Elixir/Gleam) and the web frontend's
  `web/src/lib/graphql.ts` — see that file for the reference implementation
  this one mirrors. Unlike every other SDK here, this one needs no
  hand-rolled WebSocket framing: `java.net.http.WebSocket` is a core JDK API
  (Java 11+).

  A `client` is a plain map, matching every other namespace's convention —
  `{:url ... :token ... :http-client ... :ws-state (atom ...)}`. Build one
  with [[new-client]] or [[client-from-env]]; every function below takes it
  first.

  Example:

  ```clojure
  (require '[bsdkrun.client :as client])

  (def c (client/client-from-env))
  (doseq [m (client/list-machines c)] (println (:id m)))
  (def result (client/exec! c \"abc123\" [\"uname\" \"-a\"]))
  (println (String. (:output result)))
  ```"
  (:require [clojure.data.json :as json]
            [clojure.string :as str]
            [bsdkrun.errors :as errors]
            [bsdkrun.types :as types]
            [bsdkrun.util :as util])
  (:import (java.io ByteArrayOutputStream)
           (java.net URI)
           (java.net.http HttpClient HttpRequest HttpRequest$BodyPublishers
                           HttpResponse HttpResponse$BodyHandlers
                           WebSocket WebSocket$Listener)
           (java.util Base64)))

(def url-env "BSDKRUN_URL")
(def token-env "BSDKRUN_TOKEN")

;; ---------------------------------------------------------------------------
;; config / URL normalization
;; ---------------------------------------------------------------------------

(defn normalize-url
  "Trim, add `http://` if no scheme was given, strip trailing slashes, append
  `/graphql` unless the path already ends with it. Mirrors
  `web/src/lib/connection.ts`'s `normalizeUrl` exactly."
  [input]
  (let [s (str/trim (str input))]
    (if (empty? s)
      s
      (let [s (if (re-find #"(?i)^https?://" s) s (str "http://" s))
            s (str/replace s #"/+$" "")
            s (if (re-find #"(?i)/graphql$" s) s (str s "/graphql"))]
        s))))

(defn ws-url
  "Derive the websocket endpoint from the HTTP one: `http://` -> `ws://`,
  `https://` -> `wss://`, trailing slashes on the path stripped, `/ws`
  appended. Mirrors `web/src/lib/graphql.ts`'s `wsUrl`."
  [http-url]
  (-> http-url
      (str/replace-first #"(?i)^https://" "wss://")
      (str/replace-first #"(?i)^http://" "ws://")
      (str/replace #"/+$" "")
      (str "/ws")))

(defn new-client
  "Build a client from an explicit `{:url ... :token ...}` map. Does not
  connect yet — the HTTP client is cheap to hold, and the websocket is opened
  lazily on first [[subscribe]]."
  [{:keys [url token]}]
  {:url (normalize-url url)
   :token (str token)
   :http-client (HttpClient/newHttpClient)
   :ws-state (atom {:socket nil :acked false :pending [] :subs {} :next-id 0})})

(defn client-from-env
  "Build a client from `BSDKRUN_URL` / `BSDKRUN_TOKEN`.

  A URL set without a token is an error, not a silent unauthenticated
  fallback — mirrors `daemon/src/client.rs`'s `RemoteConfig::from_env` (which
  uses `BSDKRUN_HOST`/`BSDKRUN_TOKEN` for the gRPC client; these are
  GraphQL-specific env vars with a different URL shape, not aliases).

  The 1-arity form takes an explicit `{String -> String}` env map instead of
  reading `System/getenv` — dependency injection for tests, the same
  approach `bsdkrun.binary` uses, since the JVM offers no supported way to
  mutate real process environment variables at runtime.

  Throws `errors/missing-config` if `BSDKRUN_URL` is unset, or set without
  `BSDKRUN_TOKEN`."
  ([] (client-from-env (System/getenv)))
  ([env]
   (let [url (get env url-env)]
     (when (str/blank? url)
       (throw (errors/missing-config (str url-env " is not set; nothing to connect to"))))
     (let [token (get env token-env)]
       (when (str/blank? token)
         (throw (errors/missing-config (str url-env " is set but " token-env " is not"))))
       (new-client {:url url :token token})))))

;; ---------------------------------------------------------------------------
;; HTTP transport (queries + mutations) — the escape hatch, and everything
;; below (except subscriptions) is built on it.
;; ---------------------------------------------------------------------------

(defn request
  "Run an arbitrary query or mutation. Every typed method in this namespace
  is implemented in terms of this — it exists as a public escape hatch for
  documents this SDK has no typed wrapper for yet.

  Returns `body[\"data\"]` (String-keyed, as parsed by `clojure.data.json`).

  Throws `errors/auth-error` on HTTP 401, or a GraphQL error with
  `extensions.code == \"UNAUTHENTICATED\"`. Throws `errors/graphql-error` on
  transport failure, a non-JSON response, or any other GraphQL error."
  ([client query] (request client query {}))
  ([client query variables]
   (let [{:keys [url token http-client]} client
         body (json/write-str {"query" query "variables" (or variables {})})
         req (-> (HttpRequest/newBuilder (URI/create url))
                 (.header "content-type" "application/json")
                 (.header "authorization" (str "Bearer " token))
                 (.POST (HttpRequest$BodyPublishers/ofString body))
                 .build)
         res (try
               (.send ^HttpClient http-client req (HttpResponse$BodyHandlers/ofString))
               (catch Exception e
                 (throw (errors/graphql-error
                         (str "cannot reach the bsdkrun daemon at " url " — " (.getMessage e))))))
         status (.statusCode ^HttpResponse res)]
     (when (= status 401) (throw (errors/auth-error)))
     (let [parsed (try (json/read-str (.body ^HttpResponse res)) (catch Exception _ nil))]
       (when (nil? parsed)
         (throw (errors/graphql-error (str "the daemon returned a non-JSON response (" status ")"))))
       (let [errs (get parsed "errors")]
         (if (and (sequential? errs) (seq errs))
           (let [first-err (first errs)
                 message (str (get first-err "message"))
                 code (get-in first-err ["extensions" "code"])]
             (if (= code "UNAUTHENTICATED")
               (throw (errors/auth-error message))
               (throw (errors/graphql-error message code))))
           (get parsed "data")))))))

;; ---------------------------------------------------------------------------
;; WebSocket transport (subscriptions) — graphql-transport-ws over
;; java.net.http.WebSocket, no hand-rolled framing needed.
;; ---------------------------------------------------------------------------

(defn- send-text!
  "Fire-and-forget a text frame. Swallows failures (e.g. writing after
  close) — callers that need delivery guarantees are the request/response
  paths (HTTP), not this best-effort control-message channel."
  [^WebSocket ws ^String text]
  (try (.sendText ws text true) (catch Exception _ nil)))

(defn- flush-pending!
  "Called on `connection_ack`: atomically mark the connection acked and pull
  everything queued while unacked, then run those `subscribe` sends outside
  the atom swap (never call side-effecting code from inside a `swap!` fn —
  it may be retried under contention)."
  [state]
  (let [[old _new] (swap-vals! state assoc :acked true :pending [])]
    (doseq [f (:pending old)] (f))))

(defn- error-detail
  [payload]
  (if (sequential? payload)
    (str/join "; " (map #(get % "message") payload))
    (str payload)))

(defn- handle-message!
  [client ws msg]
  (let [state (:ws-state client)]
    (case (get msg "type")
      "connection_ack" (flush-pending! state)

      "next" (when-let [sub (get-in @state [:subs (get msg "id")])]
               (try ((:on-next sub) (get-in msg ["payload" "data"]))
                    (catch Exception _ nil)))

      "error" (let [id (get msg "id")
                    [old _new] (swap-vals! state update :subs dissoc id)
                    sub (get-in old [:subs id])]
                (when sub
                  (try ((:on-error sub) (errors/graphql-error (error-detail (get msg "payload"))))
                       (catch Exception _ nil))))

      "complete" (let [id (get msg "id")
                       [old _new] (swap-vals! state update :subs dissoc id)
                       sub (get-in old [:subs id])]
                   (when sub (try ((:on-complete sub)) (catch Exception _ nil))))

      "ping" (send-text! ws (json/write-str {"type" "pong"}))

      nil)))

(defn- handle-disconnect!
  "The socket closed (cleanly or with an error). Whether `connection_ack`
  was ever received decides the reason: an unacked close means the daemon
  rejected our token; an acked close is just \"the connection dropped\".
  Mirrors `web/src/lib/graphql.ts`'s `ws.onclose`."
  [client]
  (let [state (:ws-state client)
        [old _new] (swap-vals! state (fn [s] (assoc s :socket nil :acked false :pending [] :subs {})))
        subs (:subs old)]
    (when (seq subs)
      (let [err (if (:acked old)
                  (errors/graphql-error "the connection to the daemon was closed")
                  (errors/auth-error))]
        (doseq [[_id sub] subs]
          (try ((:on-error sub) err) (catch Exception _ nil)))))))

(defn- make-listener
  ^WebSocket$Listener [client]
  (let [text-buf (atom "")]
    (reify WebSocket$Listener
      (onOpen [_ ws]
        (.request ^WebSocket ws 1)
        ;; java.net.http.WebSocket cannot set headers on the handshake, so
        ;; the token travels in connection_init instead — same reason the
        ;; browser client does this in web/src/lib/graphql.ts.
        (send-text! ws (json/write-str
                         {"type" "connection_init"
                          "payload" {"authorization" (str "Bearer " (:token client))}}))
        nil)
      (onText [_ ws data last]
        (swap! text-buf str data)
        (when last
          (let [raw @text-buf]
            (reset! text-buf "")
            (try
              (handle-message! client ws (json/read-str raw))
              (catch Exception _ nil))))
        (.request ^WebSocket ws 1)
        nil)
      (onClose [_ _ws _status _reason]
        (handle-disconnect! client)
        nil)
      (onError [_ _ws _error]
        (handle-disconnect! client)
        nil))))

(defn- ensure-ws!
  "Open the shared websocket if it is not already open. Connects
  synchronously (`.get` on the `CompletableFuture<WebSocket>`); a handshake
  failure becomes a `graphql-error`."
  ^WebSocket [client]
  (let [state (:ws-state client)]
    (locking state
      (or (:socket @state)
          (let [wsurl (ws-url (:url client))
                listener (make-listener client)
                builder (-> ^HttpClient (:http-client client)
                            .newWebSocketBuilder
                            (.subprotocols "graphql-transport-ws" (make-array String 0)))
                fut (.buildAsync builder (URI/create wsurl) listener)
                ws (try (.get fut)
                        (catch Exception e
                          (throw (errors/graphql-error
                                  (str "cannot reach the bsdkrun daemon at " wsurl " — "
                                       (.getMessage (or (.getCause e) e)))))))]
            (swap! state assoc :socket ws :acked false :pending [])
            ws)))))

(defn- close-ws!
  [client]
  (let [state (:ws-state client)
        [old _new] (swap-vals! state assoc :socket nil :acked false :pending [] :subs {})]
    (when-let [^WebSocket ws (:socket old)]
      (try (.sendClose ws WebSocket/NORMAL_CLOSURE "") (catch Exception _ nil)))))

(defn- do-unsubscribe!
  [client id]
  (let [state (:ws-state client)
        [old _new] (swap-vals! state update :subs dissoc id)]
    (when (contains? (:subs old) id)
      (when-let [ws (:socket @state)]
        (send-text! ws (json/write-str {"id" id "type" "complete"})))
      (when (empty? (:subs @state))
        (close-ws! client)))))

(defn subscribe
  "Start a subscription over the shared websocket (opened lazily on first
  use). `handlers` is `{:on-next :on-error :on-complete}`, all optional.

  A `subscribe` message is queued until `connection_ack` arrives if the
  socket has not acked yet, exactly like `web/src/lib/graphql.ts`'s
  `subscribe`.

  Returns a zero-arg function that unsubscribes (sends `complete`, then
  closes the socket if this was the last live subscription)."
  [client query variables handlers]
  (let [on-next (or (:on-next handlers) (fn [_]))
        on-error (or (:on-error handlers) (fn [_]))
        on-complete (or (:on-complete handlers) (fn []))
        state (:ws-state client)]
    (try
      (let [ws (ensure-ws! client)
            id (str (:next-id (swap! state update :next-id inc)))
            sub {:on-next on-next :on-error on-error :on-complete on-complete}
            start! (fn []
                     (send-text! ws (json/write-str
                                      {"id" id "type" "subscribe"
                                       "payload" {"query" query "variables" (or variables {})}})))
            committed (swap! state
                              (fn [s]
                                (let [s (assoc-in s [:subs id] sub)]
                                  (if (:acked s) s (update s :pending conj start!)))))]
        (when (:acked committed) (start!))
        (fn unsubscribe! [] (do-unsubscribe! client id)))
      (catch Exception e
        (future (on-error e))
        (fn unsubscribe! [] nil)))))

;; ---------------------------------------------------------------------------
;; lifecycle / listing
;; ---------------------------------------------------------------------------

(def ^:private machine-fields
  "The `Machine` field selection shared by `list-machines` / `get-machine` —
  mirrors `web/src/lib/api.ts`'s `MACHINE_FIELDS` fragment exactly."
  "id name image kind command status running exitCode pid detached
   cpus mem volume stateDir createdAt finishedAt network netIp
   ports { bind host guest }")

(defn list-machines
  "`{:all true}` includes stopped machines too (default running only).

  Returns a vector of sandbox-info maps (see
  `bsdkrun.types/sandbox-info-from-graphql`) — the same shape
  `bsdkrun.sandbox/list` returns."
  ([client] (list-machines client {}))
  ([client {:keys [all]}]
   (let [data (request client
                        (str "query($all:Boolean!){ machines(all:$all){ " machine-fields " } }")
                        {:all (boolean all)})]
     (mapv types/sandbox-info-from-graphql (get data "machines")))))

(defn get-machine
  "A machine by id, name, or unique id prefix. Returns nil if no such
  machine exists."
  [client id]
  (let [data (request client
                       (str "query($id:String!){ machine(id:$id){ " machine-fields " } }")
                       {:id id})
        m (get data "machine")]
    (when m (types/sandbox-info-from-graphql m))))

(defn- command-result-mutation!
  [client field query variables]
  (let [data (request client query variables)]
    (types/command-result-from-graphql (get data field))))

(defn stop!
  "Stop the machine. Returns a command-result map."
  [client id]
  (command-result-mutation!
   client "stopMachine"
   "mutation($id:String!){ stopMachine(id:$id){ exitCode stdout stderr } }"
   {:id id}))

(defn start!
  "Restart a stopped machine in place. Returns a command-result map."
  [client id]
  (command-result-mutation!
   client "startMachine"
   "mutation($id:String!){ startMachine(id:$id){ exitCode stdout stderr } }"
   {:id id}))

(defn remove!
  "Remove one or more machines. `ids` is a single id or a collection of
  them. `{:force true}` stops them first if running."
  ([client ids] (remove! client ids {}))
  ([client ids {:keys [force]}]
   (command-result-mutation!
    client "removeMachines"
    "mutation($ids:[String!]!,$force:Boolean!){ removeMachines(ids:$ids, force:$force){ exitCode stdout stderr } }"
    {:ids (vec (util/as-seq ids)) :force (boolean force)})))

(defn update!
  "Change the recorded vCPU / RAM. Applies on the next [[start!]]."
  ([client id] (update! client id {}))
  ([client id {:keys [cpus mem]}]
   (command-result-mutation!
    client "updateMachine"
    "mutation($id:String!,$cpus:Int,$mem:Int){ updateMachine(id:$id, cpus:$cpus, mem:$mem){ exitCode stdout stderr } }"
    {:id id :cpus cpus :mem mem})))

(defn commit!
  "Snapshot a machine into a named flavor, like `docker commit`."
  ([client id name] (commit! client id name ""))
  ([client id name description]
   (command-result-mutation!
    client "commitMachine"
    (str "mutation($id:String!,$name:String!,$description:String!){ "
         "commitMachine(id:$id, name:$name, description:$description){ exitCode stdout stderr } }")
    {:id id :name name :description (or description "")})))

(defn logs
  "One-shot console log fetch. `{:boot true}` shows bsdkrun's own boot log
  instead of the guest console. Use [[follow-logs]] to stream instead."
  ([client id] (logs client id {}))
  ([client id {:keys [boot]}]
   (let [data (request client
                        "query($id:String!,$boot:Boolean!){ machineLogs(id:$id, boot:$boot) }"
                        {:id id :boot (boolean boot)})]
     (get data "machineLogs"))))

(defn- b64-decode
  ^bytes [^String s]
  (.decode (Base64/getDecoder) s))

(defn follow-logs
  "Stream a machine's console log live over a subscription.

  `handlers` is `{:follow :boot :on-data :on-error :on-complete}` — `:follow`
  defaults true, `:boot` defaults false; `:on-data` receives binary-safe
  decoded chunks (a `byte[]`).

  Returns a zero-arg function that stops following."
  [client id handlers]
  (let [{:keys [follow boot on-data on-error on-complete]
         :or {follow true boot false}} handlers]
    (subscribe
     client
     (str "subscription($id:String!,$follow:Boolean!,$boot:Boolean!){ "
          "machineLogs(id:$id, follow:$follow, boot:$boot){ dataBase64 exitCode } }")
     {:id id :follow (boolean follow) :boot (boolean boot)}
     {:on-next (fn [data]
                 (let [payload (get data "machineLogs")]
                   (when-let [b64 (and payload (get payload "dataBase64"))]
                     (when on-data (on-data (b64-decode b64))))))
      :on-error (fn [e] (when on-error (on-error e)))
      :on-complete (fn [] (when on-complete (on-complete)))})))

;; ---------------------------------------------------------------------------
;; booting — six variants, one per daemon mutation. Field names transcribed
;; from daemon/src/graphql.rs's Run*Input structs (async-graphql camelCases
;; them on the wire) — RunLinuxInput (~L384), RunBsdInput (~L406),
;; RunNanosInput (~L428), RunUnikraftInput (~L449), RunOsvInput (~L467),
;; RunFlavorInput (~L506), NetInput (~L360), BsdOs (~L324). `opts` uses
;; kebab-case Clojure keys mapped 1:1 to the camelCase wire fields (e.g.
;; :kernel-version -> kernelVersion), matching bsdkrun.args's existing
;; option-key convention for the local create!.
;; ---------------------------------------------------------------------------

(defn- fetch!
  "Like Ruby's `Hash#fetch` — the required option key must be present (even
  if its value happens to be falsy), or `errors/missing-option` is thrown."
  [opts k]
  (if (contains? opts k)
    (get opts k)
    (throw (errors/missing-option k))))

(defn- net-input
  "`net` is `:no-net`/`:ports`/`:mac`/`:network`/`:name`.
  Returns a `NetInput`-shaped wire map, or nil."
  [net]
  (when net
    {:noNet (boolean (:no-net net))
     :ports (vec (or (:ports net) []))
     :mac (:mac net)
     :network (:network net)
     :name (:name net)}))

(defn- bsd-os-enum
  [os]
  (case (str/lower-case (name os))
    "freebsd" "FREEBSD"
    "netbsd" "NETBSD"
    (throw (errors/unknown-os os))))

(defn- env->list
  "`env` may be a map of `K -> V` or an already-formatted `[\"K=V\" ...]`
  vector, or nil. Returns `[\"K=V\" ...]`, as `openShell`'s `env:` field and
  the `Run*Input`s' `env:` field want."
  [env]
  (cond
    (nil? env) []
    (map? env) (mapv (fn [[k v]] (str (name k) "=" v)) env)
    :else (vec env)))

(defn run-linux!
  "Boot a Linux OCI image. Returns the new machine's id."
  [client opts]
  (let [input {:image (fetch! opts :image)
               :cpus (:cpus opts)
               :mem (:mem opts)
               :net (net-input (:net opts))
               :volume (:volume opts)
               :mounts (vec (or (:mounts opts) []))
               :attachDisk (vec (or (:attach-disk opts) []))
               :env (env->list (:env opts))
               :entrypoint (:entrypoint opts)
               :initramfs (boolean (:initramfs opts))
               :kernel (:kernel opts)
               :kernelVersion (:kernel-version opts)
               :console (:console opts)
               :repo (:repo opts)
               :command (vec (or (:command opts) []))}
        data (request client "mutation($i:RunLinuxInput!){ runLinux(input:$i) }" {:i input})]
    (get data "runLinux")))

(defn run-bsd!
  "Boot a FreeBSD/NetBSD guest. `:os` is `:freebsd`/`:netbsd` (a keyword, or
  the equivalent string). Returns the new machine's id."
  [client opts]
  (let [input {:os (bsd-os-enum (fetch! opts :os))
               :version (:version opts)
               :cpus (:cpus opts)
               :mem (:mem opts)
               :net (net-input (:net opts))
               :volume (:volume opts)
               :persist (boolean (:persist opts))
               :force (boolean (:force opts))
               :firmware (:firmware opts)
               :attachDisk (vec (or (:attach-disk opts) []))
               :diskSize (:disk-size opts)
               :repo (:repo opts)
               :command (vec (or (:command opts) []))}
        data (request client "mutation($i:RunBsdInput!){ runBsd(input:$i) }" {:i input})]
    (get data "runBsd")))

(defn run-nanos!
  "Boot a Nanos unikernel. No agent (no exec!/shell!), but it does have a
  root disk, so `:persist` is the one disk option it takes. Returns the new
  machine's id."
  [client opts]
  (let [input {:image (fetch! opts :image)
               :cpus (:cpus opts)
               :mem (:mem opts)
               :net (net-input (:net opts))
               :kernel (:kernel opts)
               :cmdline (:cmdline opts)
               :persist (boolean (:persist opts))}
        data (request client "mutation($i:RunNanosInput!){ runNanos(input:$i) }" {:i input})]
    (get data "runNanos")))

(defn run-unikraft!
  "Boot a Unikraft unikernel. No disk and no agent, so no
  volume/persist/repo/command options — `:mounts` (virtio-fs shares) is the
  exception, needing neither. Returns the new machine's id."
  [client opts]
  (let [input {:path (:path opts)
               :cpus (:cpus opts)
               :mem (:mem opts)
               :net (net-input (:net opts))
               :cmdline (:cmdline opts)
               :initramfs (:initramfs opts)
               :mounts (vec (or (:mounts opts) []))}
        data (request client "mutation($i:RunUnikraftInput!){ runUnikraft(input:$i) }" {:i input})]
    (get data "runUnikraft")))

(defn run-solo5!
  "Boot a Solo5 (MirageOS) unikernel. Runs under the `solo5-hvt` tender
  rather than libkrun; the unikernel declares its own network and block
  devices in its MFT1 manifest note, so only host-side facts cross: `:path`
  (a `.hvt` binary or a project dir whose `dist/` holds one, default `.`),
  `:block` backing files (\"NAME=FILE\"), and `:args` handed to the unikernel
  itself. Single vCPU always — `:cpus` above 1 is warned about and ignored.
  No disk, no agent. Returns the new machine's id."
  [client opts]
  (let [input {:path (:path opts)
               :cpus (:cpus opts)
               :mem (:mem opts)
               :net (net-input (:net opts))
               :block (vec (or (:block opts) []))
               :args (vec (or (:args opts) []))}
        data (request client "mutation($i:RunSolo5Input!){ runSolo5(input:$i) }" {:i input})]
    (get data "runSolo5")))

(defn run-osv!
  "Boot an OSv unikernel. Like Nanos, no agent, but it does have a root
  filesystem, so the disk options apply — `:disk` in particular, how an
  x86_64 guest gets a filesystem (its loader ELF is kernel only). Returns the
  new machine's id."
  [client opts]
  (let [input {:image (fetch! opts :image)
               :cpus (:cpus opts)
               :mem (:mem opts)
               :net (net-input (:net opts))
               :cmdline (:cmdline opts)
               :disk (:disk opts)
               :noDisk (boolean (:no-disk opts))
               :attachDisk (vec (or (:attach-disk opts) []))
               :gic (some-> (:gic opts) str)
               :persist (boolean (:persist opts))
               :volume (:volume opts)}
        data (request client "mutation($i:RunOsvInput!){ runOsv(input:$i) }" {:i input})]
    (get data "runOsv")))

(defn run-flavor!
  "Boot a saved flavor by name. Returns the new machine's id."
  [client opts]
  (let [input {:name (fetch! opts :name)
               :cpus (:cpus opts)
               :mem (:mem opts)
               :ports (vec (or (:ports opts) []))
               :volume (:volume opts)
               :repo (:repo opts)}
        data (request client "mutation($i:RunFlavorInput!){ runFlavor(input:$i) }" {:i input})]
    (get data "runFlavor")))

;; ---------------------------------------------------------------------------
;; exec / interactive shell
;; ---------------------------------------------------------------------------

(def ^:private open-shell-mutation
  (str "mutation($m:String!,$c:[String!]!,$e:[String!]!,$r:Int!,$k:Int!){ "
       "openShell(machineId:$m, command:$c, env:$e, rows:$r, cols:$k){ id } }"))

(def ^:private shell-output-subscription
  "subscription($s:String!){ shellOutput(sessionId:$s){ dataBase64 exitCode } }")

(defn- close-shell!
  "Idempotent server-side. A request failure here (session already gone,
  machine removed, etc.) must never mask the caller's real result, so it is
  swallowed — this exact bug was found and fixed in another SDK's `exec!`
  during review; get it right here from the start."
  [client session-id]
  (try
    (request client "mutation($s:String!){ closeShell(sessionId:$s) }" {:s session-id})
    (catch clojure.lang.ExceptionInfo _ nil)))

(defn exec!
  "Run a command to completion and collect its output.

  Implemented as the three-operation sequence `daemon/README.md` documents:
  `openShell` (with a `command:`, so the session runs it instead of a login
  shell), THEN subscribe to `shellOutput` (so nothing written in between is
  lost), THEN wait for an exit code. `closeShell` always runs, whether the
  wait succeeded, failed, or timed out.

  `opts`: `:env` — a map of `K -> V`, or a `[\"K=V\" ...]` vector.

  Returns `{:exit-code ... :output <byte[]>}`. Blocks the calling thread."
  ([client id command] (exec! client id command {}))
  ([client id command {:keys [env]}]
   (let [data (request client open-shell-mutation
                        {:m id :c (vec command) :e (env->list env) :r 24 :k 80})
         session-id (get-in data ["openShell" "id"])
         out (ByteArrayOutputStream.)
         result (promise)
         unsubscribe (subscribe
                      client shell-output-subscription {:s session-id}
                      {:on-next (fn [evt]
                                  (let [payload (get evt "shellOutput")]
                                    (when payload
                                      (when-let [b64 (get payload "dataBase64")]
                                        (.write out ^bytes (b64-decode b64)))
                                      (when-some [ec (get payload "exitCode")]
                                        (deliver result {:exit-code ec})))))
                       :on-error (fn [e] (deliver result {:error e}))
                       :on-complete (fn [] (deliver result {:exit-code nil}))})]
     (try
       (let [r @result]
         (when (:error r) (throw (:error r)))
         {:exit-code (:exit-code r) :output (.toByteArray out)})
       (finally
         (unsubscribe)
         (close-shell! client session-id))))))

(defn- to-bytes
  ^bytes [data]
  (cond
    (bytes? data) data
    (string? data) (.getBytes ^String data "UTF-8")
    :else (.getBytes (str data) "UTF-8")))

(defn shell!
  "Open a live interactive session. Unlike [[exec!]], this returns
  immediately with a handle whose `:on-output!`/`:on-exit!` callbacks fire as
  output arrives.

  The `shellOutput` subscription starts as soon as this function sets it up
  — necessarily before the caller can register a callback on the returned
  handle — so any output/exit that arrives in that window is buffered and
  replayed to a callback the moment one is registered, never dropped. (This
  exact race was found and fixed in two other SDKs' `shell()` during
  review.)

  `opts`: `:command` (nil opens a login shell), `:env`, `:rows` (default 24),
  `:cols` (default 80).

  Returns a handle map: `{:id :write! :resize! :close! :on-output!
  :on-exit!}`."
  ([client id] (shell! client id {}))
  ([client id {:keys [command env rows cols] :or {rows 24 cols 80}}]
   (let [data (request client open-shell-mutation
                        {:m id :c (vec (or command [])) :e (env->list env) :r rows :k cols})
         session-id (get-in data ["openShell" "id"])
         out-state (atom {:cb nil :buffered []})
         exit-state (atom {:cb nil :delivered false :code nil})
         deliver-output! (fn [bytes]
                            (let [[_old new] (swap-vals! out-state
                                                          (fn [s] (if (:cb s) s (update s :buffered conj bytes))))]
                              (when-let [cb (:cb new)] (cb bytes))))
         deliver-exit! (fn [code]
                          (let [[old new] (swap-vals! exit-state
                                                       (fn [s] (if (:delivered s) s (assoc s :delivered true :code code))))]
                            (when (and (:cb new) (not (:delivered old)))
                              ((:cb new) code))))
         unsubscribe (subscribe
                      client shell-output-subscription {:s session-id}
                      {:on-next (fn [evt]
                                  (let [payload (get evt "shellOutput")]
                                    (when payload
                                      (when-let [b64 (get payload "dataBase64")]
                                        (deliver-output! (b64-decode b64)))
                                      (when-some [ec (get payload "exitCode")]
                                        (deliver-exit! ec)))))
                       :on-error (fn [_e] (deliver-exit! nil))
                       :on-complete (fn [] nil)})]
     {:id session-id
      :write! (fn [data]
                (request client
                         "mutation($s:String!,$d:String!){ sendShellInput(sessionId:$s, dataBase64:$d) }"
                         {:s session-id :d (.encodeToString (Base64/getEncoder) (to-bytes data))})
                nil)
      :resize! (fn [rows cols]
                 (request client
                          "mutation($s:String!,$r:Int!,$c:Int!){ resizeShell(sessionId:$s, rows:$r, cols:$c) }"
                          {:s session-id :r rows :c cols})
                 nil)
      :close! (fn []
                (unsubscribe)
                (close-shell! client session-id)
                nil)
      :on-output! (fn [f]
                    (let [[old _new] (swap-vals! out-state assoc :cb f :buffered [])]
                      (doseq [bytes (:buffered old)] (f bytes))))
      :on-exit! (fn [f]
                  (let [[old _new] (swap-vals! exit-state assoc :cb f)]
                    (when (:delivered old) (f (:code old)))))})))
