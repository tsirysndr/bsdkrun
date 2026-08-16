(ns console.sdk
  "Unified per-language commands for the `sdk/*` packages — the same
  commands `.github/workflows/sdk-unit.yml` / `e2e-sdk.yml` run in CI.

  Every command here is a single function that takes a language as a plain
  keyword (an *atom*, in the Lisp sense — a bare scalar, not a call per
  language): `(test :clojure)`, `(build :ruby)`, `(publish :gleam)`. A
  string works too (`(test \"clojure\")`) — handy from `bb`, where CLI args
  always arrive as strings.

  `:refer-clojure :exclude [test]` — `test` shadows the rarely-used
  `clojure.core/test` (runs a var's `:test` metadata function); the name
  worth keeping here is the CI-mirroring one."
  (:refer-clojure :exclude [test])
  (:require [babashka.fs :as fs]
            [clojure.string :as str]
            [console.shell :as sh]))

(def ^:private sdk-dir
  "Where each language's SDK lives, relative to the repo root."
  {:clojure    "sdk/clojure"
   :ruby       "sdk/ruby"
   :python     "sdk/python"
   :elixir     "sdk/elixir"
   :gleam      "sdk/gleam"
   :typescript "sdk/typescript"
   :go         "sdk/go"
   :rust       "sdk/rust"
   :scala      "sdk/scala"})

(defn- ->lang
  "Coerce a language argument to a keyword, so both `:clojure` and
  `\"clojure\"` work, and validate it against `sdk-dir`."
  [lang]
  (let [k (keyword (name lang))]
    (when-not (contains? sdk-dir k)
      (throw (ex-info (str "unknown lang " k " (expected one of "
                           (str/join " " (sort (keys sdk-dir))) ")")
                      {:lang k})))
    k))

(defn dir
  "The repo-root-relative directory for `lang`. Usage: (dir :ruby)"
  [lang]
  (get sdk-dir (->lang lang)))

(defn- abs-dir [lang] (str (fs/path (sh/repo-root) (dir lang))))

(defn- run-steps
  "Run each `cmd` (a vector, run with `:dir dir`) in order, stopping at the
  first non-zero exit. Returns the first non-zero exit code, or 0 once every
  step has succeeded — the same exit-code convention every other command in
  this console returns."
  [dir & cmds]
  (loop [cmds cmds]
    (if (empty? cmds)
      0
      (let [code (sh/sh (first cmds) {:dir dir})]
        (if (zero? code) (recur (rest cmds)) code)))))

(defn deps
  "Install/fetch dependencies. lang ∈ :elixir :gleam — the other four
  resolve deps as part of their own test/build step, so there is nothing
  separate to run for them. Usage: (deps :elixir)"
  [lang]
  (case (->lang lang)
    :elixir (sh/sh ["mix" "deps.get"] {:dir (dir :elixir)})
    :gleam  (sh/sh ["gleam" "deps" "download"] {:dir (dir :gleam)})
    (throw (ex-info (str "deps: no separate deps step for " (name lang)) {:lang lang}))))

(defn test
  "Run one SDK's unit-test suite — the same command CI runs. lang ∈
  :clojure :ruby :python :elixir :gleam :typescript :go :rust :scala.
  Usage: (test :clojure)"
  [lang]
  (case (->lang lang)
    :clojure    (sh/sh ["clojure" "-M:test"] {:dir (dir :clojure)})
    :ruby       (sh/sh ["rake" "test"] {:dir (dir :ruby)})
    :python     (sh/sh ["uv" "run" "pytest"] {:dir (dir :python)})
    :elixir     (run-steps (dir :elixir)
                            ["mix" "deps.get"]
                            ["mix" "compile" "--warnings-as-errors"]
                            ["mix" "test"])
    :gleam      (run-steps (dir :gleam)
                            ["gleam" "deps" "download"]
                            ["gleam" "format" "--check" "src" "test"]
                            ["gleam" "test"])
    :typescript (sh/sh ["bun" "test"] {:dir (dir :typescript)})
    :go         (run-steps (dir :go)
                            ["go" "vet" "./..."]
                            ["go" "test" "./..."])
    :rust       (run-steps (dir :rust)
                            ["cargo" "fmt" "--check"]
                            ["cargo" "clippy" "--" "-D" "warnings"]
                            ["cargo" "test"])
    ;; `compile` is also the warnings gate: build.sbt sets -Xfatal-warnings,
    ;; the equivalent of elixir's --warnings-as-errors above.
    :scala      (run-steps (dir :scala)
                            ["sbt" "-batch" "compile" "Test/compile"]
                            ["sbt" "-batch" "test"])))

(defn lint
  "lang ∈ :python — ruff check + ruff format --check + mypy, the same three
  gates CI runs before pytest. Usage: (lint :python)"
  [lang]
  (case (->lang lang)
    :python (run-steps (dir :python)
                        ["uv" "run" "ruff" "check"]
                        ["uv" "run" "ruff" "format" "--check"]
                        ["uv" "run" "mypy"])
    (throw (ex-info (str "lint: no lint step wired for " (name lang)) {:lang lang}))))

(defn build
  "Build one SDK's distributable artifact. lang ∈ :clojure (jar) :ruby
  (.gem) :python (wheel + sdist) :typescript (tsc) :scala (jar + sources +
  javadoc). Usage: (build :clojure)"
  [lang]
  (case (->lang lang)
    :clojure    (sh/sh ["clj" "-T:build" "jar"] {:dir (dir :clojure)})
    :ruby       (sh/sh ["gem" "build" "bsdkrun.gemspec"] {:dir (dir :ruby)})
    :python     (sh/sh ["uv" "build"] {:dir (dir :python)})
    :typescript (sh/sh ["bun" "run" "build"] {:dir (dir :typescript)})
    ;; Maven Central requires the -sources and -javadoc jars, so `package`
    ;; alone would not be a publishable build.
    :scala      (run-steps (dir :scala)
                            ["sbt" "-batch" "package" "packageSrc" "packageDoc"])
    (throw (ex-info (str "build: no build step wired for " (name lang)) {:lang lang}))))

(defn install
  "lang ∈ :clojure — install the built jar to the local ~/.m2.
  Usage: (install :clojure)"
  [lang]
  (case (->lang lang)
    :clojure (sh/sh ["clj" "-T:build" "install"] {:dir (dir :clojure)})
    (throw (ex-info (str "install: no install step wired for " (name lang)) {:lang lang}))))

(defn- publish-ruby
  "gem build, then gem push whichever bsdkrun-*.gem that produced (sorted,
  so a stale gem left over from a previous build can't be pushed by
  mistake)."
  [args]
  (let [code (build :ruby)]
    (if-not (zero? code)
      code
      (let [gemfile (->> (fs/glob (abs-dir :ruby) "bsdkrun-*.gem")
                          (sort-by str)
                          last)]
        (when-not gemfile
          (throw (ex-info "gem build did not produce a .gem file" {:dir (abs-dir :ruby)})))
        (sh/sh (into ["gem" "push" (str (fs/file-name gemfile))] (map str args))
               {:dir (dir :ruby)})))))

(defn publish
  "Publish one SDK to its registry — Clojars/RubyGems/PyPI/npm/Hex/crates.io.
  lang ∈ :clojure :ruby :python :typescript :elixir :gleam :rust :scala. Extra args
  pass through to the underlying publisher (e.g. a tag, or --dry-run where
  the tool supports one).

  ⚠️ NOT sandboxed and NOT dry-run by default — this really pushes to the
  public registry. Each publisher expects its own credentials already
  configured (CLOJARS_USERNAME/CLOJARS_PASSWORD, `gem` / `npm` / `uv` /
  `mix hex.user` login, etc.) — the console does not manage any of them.

  Usage: (publish :clojure) | (publish :gleam \"--dry-run\")"
  [lang & args]
  (case (->lang lang)
    :clojure    (sh/sh (into ["clj" "-T:build" "deploy"] (map str args)) {:dir (dir :clojure)})
    :ruby       (publish-ruby args)
    :python     (run-steps (dir :python)
                            ["uv" "build"]
                            (into ["uv" "publish"] (map str args)))
    :typescript (run-steps (dir :typescript)
                            ["bun" "run" "build"]
                            (into ["npm" "publish"] (map str args)))
    :elixir     (sh/sh (into ["mix" "hex.publish"] (map str args)) {:dir (dir :elixir)})
    :gleam      (sh/sh (into ["gleam" "publish"] (map str args)) {:dir (dir :gleam)})
    :rust       (sh/sh (into ["cargo" "publish"] (map str args)) {:dir (dir :rust)})
    ;; Two steps: `publishSigned` stages a signed bundle, then the bundle is
    ;; POSTed to the Central Portal. NOT `sonatypeBundleRelease` — sbt-sonatype
    ;; speaks the legacy Nexus staging API, which the Portal does not serve, so
    ;; that task dies on a 400 for an unsupported endpoint. Needs a published
    ;; GPG key and CENTRAL_TOKEN_USERNAME / CENTRAL_TOKEN_PASSWORD.
    :scala      (run-steps (dir :scala)
                            ["sbt" "-batch" "publishSigned"]
                            (into ["./publish-central.sh"] (map str args)))
    ;; Go has no registry push — a module is "published" by tagging the repo
    ;; (sdk/go/vX.Y.Z) and letting the proxy fetch it.
    (throw (ex-info (str "publish: no publish step wired for " (name lang)) {:lang lang}))))

(defn test-all
  "Run every SDK's unit-test suite in turn, stopping at the first failure
  (lazy `every?` over `test` — languages after the failing one never run).
  Mirrors `.github/workflows/sdk-unit.yml` end to end, minus the TypeScript
  e2e suite (that one boots a real guest — see `e2e-sdk.yml`). Returns
  whether every suite passed."
  []
  (every? zero? (map test [:python :ruby :elixir :gleam :clojure :go :rust])))
