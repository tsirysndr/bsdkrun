(ns bsdkrun.process
  "Spawns the `bsdkrun` CLI and captures its output.

  Every invocation is prefixed with `--log-level <n>` (default 0) so the
  SDK's captured output stays clean."
  (:refer-clojure :exclude [run!])
  (:require [clojure.java.io :as io]
            [bsdkrun.binary :as binary]
            [bsdkrun.errors :as errors])
  (:import [java.io ByteArrayOutputStream]
           [java.lang ProcessBuilder]))

(defn- slurp-stream
  "Read an InputStream to completion as a UTF-8 string."
  [in on-chunk]
  (let [buf (ByteArrayOutputStream.)]
    (let [chunk (byte-array 8192)]
      (loop []
        (let [n (.read in chunk)]
          (when (pos? n)
            (.write buf chunk 0 n)
            (when on-chunk (on-chunk (java.util.Arrays/copyOf chunk n)))
            (recur)))))
    (.toString buf "UTF-8")))

(defn- build-process
  [args log-level]
  (let [bin (binary/resolve)
        full (into [bin "--log-level" (str log-level)] args)]
    (ProcessBuilder. ^java.util.List (vec full))))

(defn run
  "Run `bsdkrun --log-level <n> <args>` to completion, buffering output.

  `opts`:
    `:env`       extra environment variables merged onto the process env
                 (a map; keys/values are stringified)
    `:stdin`     data piped to the child's stdin (a string), or nil
    `:log-level` bsdkrun's global log level (0=off .. 5=trace), default 0

  Returns `{:stdout ... :stderr ... :exit-code ...}`."
  ([args] (run args {}))
  ([args {:keys [env stdin log-level on-stdout on-stderr] :or {env {} log-level 0}}]
   (let [pb (build-process args log-level)
         penv (.environment pb)]
     (doseq [[k v] env] (.put penv (name k) (str v)))
     (let [proc (.start pb)
           ;; All three streams are pumped concurrently, not stdin-then-outputs
           ;; — a child that starts producing (or blocks on) stdout/stderr
           ;; before it has consumed all of stdin would otherwise deadlock
           ;; against us blocking in a synchronous stdin write. This is the
           ;; same reason Ruby's `Open3.capture3` (what the reference SDK
           ;; uses) pumps all three streams at once internally.
           stdin-fut (future
                       (with-open [out (.getOutputStream proc)]
                         (when stdin
                           (.write out (.getBytes ^String stdin "UTF-8")))
                         (.flush out)))
           stdout-fut (future (slurp-stream (.getInputStream proc) on-stdout))
           stderr-fut (future (slurp-stream (.getErrorStream proc) on-stderr))]
       @stdin-fut
       (let [exit-code (.waitFor proc)]
         {:stdout @stdout-fut :stderr @stderr-fut :exit-code exit-code})))))

(defn- slurp-bytes
  "Read an InputStream to completion as a byte array."
  [in]
  (let [buf (ByteArrayOutputStream.)
        chunk (byte-array 8192)]
    (loop []
      (let [n (.read in chunk)]
        (when (pos? n)
          (.write buf chunk 0 n)
          (recur))))
    (.toByteArray buf)))

(defn run-binary
  "Run `bsdkrun <args>` keeping stdout as a byte array.

  [[run]] decodes stdout as UTF-8, which silently replaces every invalid byte
  — fine for JSON, ruinous for `cp ID:path -` reading a PNG. `:stdin` is a
  byte array here for the same reason. Only stderr is decoded, because it is
  always a message.

  Returns `{:stdout <byte-array> :stderr ... :exit-code ...}`."
  ([args] (run-binary args {}))
  ([args {:keys [env stdin log-level] :or {env {} log-level 0}}]
   (let [pb (build-process args log-level)
         penv (.environment pb)]
     (doseq [[k v] env] (.put penv (name k) (str v)))
     (let [proc (.start pb)
           stdin-fut (future
                       (with-open [out (.getOutputStream proc)]
                         (when stdin (.write out ^bytes stdin))
                         (.flush out)))
           stdout-fut (future (slurp-bytes (.getInputStream proc)))
           stderr-fut (future (slurp-stream (.getErrorStream proc) nil))]
       @stdin-fut
       (let [exit-code (.waitFor proc)]
         {:stdout @stdout-fut :stderr @stderr-fut :exit-code exit-code})))))

(defn run!
  "Run and throw `errors/command-failed` (see `bsdkrun.errors`) on a
  non-zero exit.

  `opts` is the same map [[run]] takes, plus `:label` — a human-readable
  name for the invocation used both in the thrown error and (by callers) as
  a Result's `:command`."
  [args {:keys [label] :as opts}]
  (let [res (run args opts)]
    (if (zero? (:exit-code res))
      res
      (throw (errors/command-failed (assoc res :command label))))))

(defn spawn-interactive!
  "Spawn an interactive `bsdkrun` command inheriting the parent's stdio (for
  `shell`) and block for the exit code.

  Returns true if the command exited zero."
  ([args] (spawn-interactive! args {}))
  ([args {:keys [log-level] :or {log-level 0}}]
   (let [pb (doto (build-process args log-level) (.inheritIO))
         proc (.start pb)]
     (zero? (.waitFor proc)))))
