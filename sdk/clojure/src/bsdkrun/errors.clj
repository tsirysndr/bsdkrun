(ns bsdkrun.errors
  "The single `ex-info` convention used by every `bsdkrun.*` namespace: a
  plain `ex-info` whose `ex-data` carries a `:bsdkrun/error` kind key plus
  whatever detail a caller needs to `case`/pattern-match on — no exception
  class hierarchy, which would be un-idiomatic Clojure.

  Every namespace in this SDK throws these via e.g.
  `(throw (errors/command-failed {...}))` rather than a bare
  `(throw (Exception. ...))`."
  (:require [clojure.string :as str]))

(defn binary-not-found
  "The `bsdkrun` binary could not be located on the host.

  `searched` is the ordered list of candidate paths that were probed."
  [searched]
  (ex-info
   (str "could not find the \"bsdkrun\" binary. Set BSDKRUN_BIN, add it to "
        "PATH, or call bsdkrun.binary/set-override!. Looked in: "
        (str/join ", " searched))
   {:bsdkrun/error :binary-not-found
    :searched (vec searched)}))

(defn command-failed
  "A `bsdkrun` invocation exited non-zero.

  `opts` is a map with `:exit-code`, `:stdout`, `:stderr`, `:command` (a
  human label for the invocation) — the same shape as a `bsdkrun.process`
  result plus `:command`, so it can be built straight from one."
  [{:keys [exit-code stdout stderr command]}]
  (let [trimmed (str/trim (or stderr ""))
        message (cond-> (str "command failed (exit " exit-code "): " command)
                  (seq trimmed) (str "\n" trimmed))]
    (ex-info message
             {:bsdkrun/error :command-failed
              :exit-code exit-code
              :stdout stdout
              :stderr stderr
              :command command})))

(defn sandbox-not-found
  "No machine matched the given id / prefix."
  [id]
  (ex-info (str "no sandbox found matching id " (pr-str id))
           {:bsdkrun/error :sandbox-not-found
            :id id}))

(defn missing-option
  "A required create-option key was absent (mirrors Ruby's `Hash#fetch`
  raising when a required option like `:image`/`:kernel`/`:firmware`/`:disk`
  is missing from the options map passed to `bsdkrun.args/build-create-args`)."
  [option]
  (ex-info (str "missing required option " option)
           {:bsdkrun/error :missing-option
            :option option}))

(defn unknown-os
  "`build-create-args` was called with an `:os` it doesn't know how to build
  argv for."
  [os]
  (ex-info (str "unknown os: " (pr-str os))
           {:bsdkrun/error :unknown-os
            :os os}))

(defn graphql-error
  "A GraphQL request to a remote `bsdkrund` daemon failed — a transport
  failure, a non-JSON response, or a `body[\"errors\"]` entry that was not an
  auth failure. Mirrors `web/src/lib/graphql.ts`'s `GraphQLError`.

  `code` is the error's `extensions.code`, when the daemon set one."
  ([message] (graphql-error message nil))
  ([message code]
   (ex-info message
            {:bsdkrun/error :graphql-error
             :code code})))

(defn auth-error
  "The daemon rejected our token — an HTTP 401, a GraphQL error whose
  `extensions.code` is `\"UNAUTHENTICATED\"`, or a websocket that closed
  before `connection_ack` ever arrived. Mirrors `web/src/lib/graphql.ts`'s
  `AuthError`."
  ([] (auth-error "the daemon rejected this token"))
  ([message]
   (ex-info message
            {:bsdkrun/error :auth-error
             :code "UNAUTHENTICATED"})))

(defn missing-config
  "`bsdkrun.client/client-from-env` was called with `BSDKRUN_URL` unset, or
  set without `BSDKRUN_TOKEN` — an error, not a silent unauthenticated
  fallback. Mirrors `daemon/src/client.rs`'s `RemoteConfig::from_env` rule."
  [message]
  (ex-info message {:bsdkrun/error :missing-config}))
