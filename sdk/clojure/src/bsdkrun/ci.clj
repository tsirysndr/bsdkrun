(ns bsdkrun.ci
  "CI workflows defined in code instead of YAML.

  The workflow is plain data — a map built up threading-style — and the YAML
  emitted from it is exactly the file `bsdkrun ci` (and tangled's spindle)
  consumes: `yaml` renders it, `save!` commits it to `.tangled/workflows/`,
  and `run!` executes it in a microVM without a file ever touching the
  repository.

      (-> (ci/workflow \"test\")
          (ci/on-push \"main\")
          (ci/deps \"clojure\" \"jdk21\")
          (ci/env \"CI_FROM\" \"sdk\")
          (ci/step \"deps\" \"clojure -P\")
          (ci/step \"test\" \"clojure -X:test\")
          (ci/run!))

  Code is the source of truth and YAML the wire format, in that order — which
  is why `save!` writes a generated-file header: a hand-edit there will be
  overwritten by the next save."
  (:refer-clojure :exclude [run!])
  (:require [bsdkrun.process :as process]
            [clojure.data.json :as json]
            [clojure.java.io :as io]
            [clojure.string :as str]))

(defn workflow
  "Start a CI workflow definition."
  [name]
  {:name name
   :engine "nixery"
   :when []
   :deps {}
   :env {}
   :steps []})

(defn engine
  "Override the engine (`nixery` by default)."
  [wf engine]
  (assoc wf :engine engine))

(defn on-push
  "Add a push trigger for the given branches."
  [wf & branches]
  (update wf :when conj {:events ["push"] :branches (vec branches)}))

(defn on-pull-request
  "Add a pull_request trigger targeting the given branches."
  [wf & branches]
  (update wf :when conj {:events ["pull_request"] :branches (vec branches)}))

(defn deps
  "Add nixpkgs dependencies — the toolchain the steps run against."
  [wf & packages]
  (update-in wf [:deps "nixpkgs"] (fnil into []) packages))

(defn deps-from
  "Add dependencies from a custom registry (a flake reference)."
  [wf registry & packages]
  (update-in wf [:deps registry] (fnil into []) packages))

(defn env
  "Set a workflow-level environment variable."
  [wf k v]
  (assoc-in wf [:env k] v))

(defn step
  "Append a step; steps run serially in one VM, from the workspace root.
  An optional trailing map sets step-scoped environment variables."
  ([wf name command] (step wf name command nil))
  ([wf name command step-env]
   (update wf :steps conj {:name name :command command :env (or step-env {})})))

(defn clone-depth
  "Set the clone depth (default 1)."
  [wf depth]
  (assoc wf :clone-depth depth))

(defn skip-clone
  "Skip the checkout entirely."
  [wf]
  (assoc wf :clone-skip true))

(defn file-name
  "The workflow file name `save!` writes: `<name>.yml`."
  [{:keys [name]}]
  (if (re-find #"\.ya?ml$" name) name (str name ".yml")))

;; A JSON string literal is a valid YAML scalar by construction, which is what
;; lets this namespace emit YAML without a YAML library.
(defn- q [s] (json/write-str s :escape-slash false))

(defn- when-section [{:keys [when]}]
  (clojure.core/when (seq when)
    (str/join "\n"
              (cons "when:"
                    (mapcat (fn [{:keys [events branches]}]
                              (cons (str "  - event: [" (str/join ", " (map q events)) "]")
                                    (case (count branches)
                                      0 []
                                      1 [(str "    branch: " (q (first branches)))]
                                      [(str "    branch: ["
                                            (str/join ", " (map q branches)) "]")])))
                            when)))))

(defn- deps-section [{:keys [deps]}]
  (when (seq deps)
    (str/join "\n"
              (cons "dependencies:"
                    (mapcat (fn [reg]
                              (cons (str "  " (q reg) ":")
                                    (map #(str "    - " (q %)) (get deps reg))))
                            (sort (keys deps)))))))

(defn- env-lines [m indent]
  (map #(str indent % ": " (q (get m %))) (sort (keys m))))

(defn- clone-section [{:keys [clone-skip clone-depth]}]
  (when (or clone-skip clone-depth)
    (str/join "\n"
              (cond-> ["clone:"]
                clone-skip (conj "  skip: true")
                clone-depth (conj (str "  depth: " clone-depth))))))

;; A literal block when it round-trips byte-for-byte; a JSON string when it
;; cannot (trailing spaces, carriage returns) — never a silent alteration.
(defn- command-lines [command]
  (let [block-safe? (and (seq command)
                         (not (str/includes? command "\r"))
                         (every? #(= % (str/replace % #" +$" ""))
                                 (str/split command #"\n" -1)))]
    (if block-safe?
      (cons "    command: |"
            (map #(str "      " %)
                 (str/split (str/replace command #"\n+$" "") #"\n" -1)))
      [(str "    command: " (q command))])))

(defn- steps-section [{:keys [steps]}]
  (str/join "\n"
            (cons "steps:"
                  (mapcat (fn [{:keys [name command env]}]
                            (concat [(str "  - name: " (q name))]
                                    (command-lines command)
                                    (when (seq env)
                                      (cons "    environment:"
                                            (env-lines env "      ")))))
                          steps))))

(defn yaml
  "Render the workflow file. Scalars are emitted as JSON strings — valid YAML
  by construction — and commands as literal blocks when safe."
  [wf]
  (str (str/join "\n\n"
                 (remove nil?
                         [(when-section wf)
                          (str "engine: " (:engine wf))
                          (deps-section wf)
                          (when (seq (:env wf))
                            (str/join "\n" (cons "environment:"
                                                 (env-lines (:env wf) "  "))))
                          (clone-section wf)
                          (steps-section wf)]))
       "\n"))

(defn save!
  "Write into `<repo>/.tangled/workflows/` and return the path."
  [wf repo]
  (let [dir (io/file repo ".tangled" "workflows")]
    (.mkdirs dir)
    (let [f (io/file dir (file-name wf))]
      (spit f (str "# Generated by the bsdkrun SDK — edit the code that save!'d it instead.\n"
                   (yaml wf)))
      (.getPath f))))

(defn run!
  "Execute the workflow in a microVM, streaming output. The YAML never
  touches the repository — it goes to a temp file and `bsdkrun ci run -f`.
  Returns true when every step passed; throws when the run fails.

  Options: `:dir` — the repository to run against (default: cwd)."
  ([wf] (run! wf {}))
  ([wf {:keys [dir]}]
   (let [tmp (doto (io/file (System/getProperty "java.io.tmpdir")
                            (str "bsdkrun-ci-" (System/nanoTime)))
               (.mkdirs))
         file (io/file tmp (file-name wf))]
     (try
       (spit file (yaml wf))
       (let [args (cond-> ["ci" "run" "-f" (.getPath file)]
                    dir (into ["-w" dir]))
             ok? (process/spawn-interactive! args)]
         (when-not ok?
           (throw (ex-info (str "workflow " (:name wf) " failed")
                           {:workflow (:name wf)})))
         true)
       (finally
         (.delete file)
         (.delete tmp))))))
