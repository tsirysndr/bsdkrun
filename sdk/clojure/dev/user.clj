(ns user
  "Loaded automatically by `clj -M:rebel` (Clojure's REPL auto-loads a `user`
  namespace found on the classpath) — the SDK's namespaces, preloaded under
  short aliases, plus a couple of interactive-session conveniences. Mirrors
  `sdk/ruby/bin/console`'s preload, adapted to a REPL instead of an IRB
  session.

  ```sh
  clj -M:rebel
  ```

  ```clojure
  user=> (ps)                          ; every machine, exited ones included
  user=> (last-machine)                ; the newest one, or nil
  user=> (sandbox/create! {:os :linux :image \"alpine\"})
  user=> (def c (client/client-from-env))
  user=> (client/list-machines c)
  ```

  Pass `-Dbsdkrun.bin=/path/to/bsdkrun` on the `java` command line (or set
  `BSDKRUN_BIN` in the environment before starting the REPL) to drive a
  locally built binary for the session — this namespace does not set an
  override itself, so `bsdkrun.binary`'s normal discovery order applies."
  (:require [bsdkrun.sandbox :as sandbox]
            [bsdkrun.images :as images]
            [bsdkrun.volumes :as volumes]
            [bsdkrun.networks :as networks]
            [bsdkrun.system :as system]
            [bsdkrun.args :as args]
            [bsdkrun.types :as types]
            [bsdkrun.errors :as errors]
            [bsdkrun.client :as client]))

(defn ps
  "Every machine, exited ones included — `bsdkrun.sandbox/list` with
  `:all true`."
  []
  (sandbox/list {:all true}))

(defn last-machine
  "The most recently created machine (by `:created-at`), or nil if there are
  none."
  []
  (->> (ps) (sort-by :created-at) last))

(println "bsdkrun dev REPL — sandbox, images, volumes, networks, system, args,")
(println "types, errors, and client are required and aliased. Try (ps) or")
(println "(last-machine).")
