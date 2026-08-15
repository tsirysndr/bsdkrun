(ns bsdkrun.cache
  "Cached guest directories.

  Entries are keyed, so a rebuild can pick up where the last one left off:

      (require '[bsdkrun.cache :as cache])

      (let [hit (cache/restore \"web\" {:key k :restore-keys [\"deps-\"]})]
        (when-not (:restored hit)
          (sandbox/exec \"web\" [\"npm\" \"ci\"])
          (cache/save \"web\" \"/app/node_modules\" {:key k})))

  Where entries live — host disk or S3 — is host configuration, not an SDK
  concern: set `BSDKRUN_CACHE_BACKEND` / `BSDKRUN_CACHE_S3_*`, or write
  `~/.config/bsdkrun/cache.toml`."
  (:require [clojure.data.json :as json]
            [clojure.string :as str]
            [bsdkrun.process :as process]))

(defn- decode
  "Parse the CLI's JSON, keywordising keys. An empty body means `empty`."
  [text empty]
  (let [body (str/trim (or text ""))]
    (json/read-str (if (str/blank? body) empty body) :key-fn keyword)))

(defn- run-json [args label empty]
  (-> (process/run! args {:label label}) :stdout (decode empty)))

(defn save
  "Archive the guest directory at `path` under `:key`.

  Options: `:key` (required), `:compression` (\"gzip\" by default, or \"zstd\",
  \"estargz\", \"none\"), `:force` to replace an existing entry. Returns the
  stored entry."
  [id path {:keys [key compression force] :or {compression "gzip"}}]
  (cond-> ["cache" "save" (str id ":" path) "--key" key "--json"]
    (not= compression "gzip") (into ["--compression" compression])
    force (conj "--force")
    :always (run-json "bsdkrun cache save" "{}")))

(defn restore
  "Restore a stored tree.

  Options: `:key` (required), `:path` (defaults to where it was saved from),
  `:restore-keys` — prefixes tried in order when the key misses. A miss is not
  an error: check `:restored` on the result."
  [id {:keys [key path restore-keys]}]
  (let [target (if path (str id ":" path) id)]
    (cond-> ["cache" "restore" target "--key" key "--json"]
      (seq restore-keys) (into (cons "--restore-keys" restore-keys))
      :always (run-json "bsdkrun cache restore" "{}"))))

(defn ls
  "Every stored cache entry, newest first."
  []
  (run-json ["cache" "ls" "--json"] "bsdkrun cache ls" "[]"))

(defn rm
  "Remove entries by key, or every one of them with `{:all true}`. Returns nil."
  ([keys] (rm keys {}))
  ([keys {:keys [all]}]
   (process/run! (if all
                   ["cache" "rm" "--all"]
                   (into ["cache" "rm"] (if (string? keys) [keys] keys)))
                 {:label "bsdkrun cache rm"})
   nil))
