# bsdkrun Console

A **single Clojure REPL for every operational command in the monorepo.**
Instead of remembering whether a given piece of work is a `make` target, a
`cargo` invocation buried in `Makefile`, a `bun run` script in `web/` or
`desktop/`, or one of six different test runners under `sdk/*`, you sit in
one REPL and call functions:

```clojure
user=> (build/release)
user=> (sdk/test :clojure)
user=> (sdk/publish :clojure)
user=> (web/dev)
user=> (desktop/tauri-dev)
```

Every command is a thin Clojure wrapper around the existing tool (`make`,
`cargo`, `bun`, `rake`, `uv`, `mix`, `gleam`, `clojure`). Nothing in the
underlying scripts changes — the console is purely a **discoverable,
composable front door**.

**Every `sdk/*` command takes the language as a plain keyword** — an *atom*,
in the Lisp sense, not a call per language — so `(sdk/test :clojure)`,
`(sdk/build :ruby)`, `(sdk/publish :gleam)` are all the same shape. A string
works too (`(sdk/test "clojure")`), which is how `bb`'s CLI args arrive.

---

## Why a console?

The repo has build/test entry points spread across `make`, `cargo`, Bun
(`web/`, `desktop/`), and eight SDK toolchains (`sdk/typescript`,
`sdk/python`, `sdk/ruby`, `sdk/elixir`, `sdk/gleam`, `sdk/clojure`,
`sdk/go`, `sdk/rust`). There is
no single `--help` that lists them all; you have to read the root
`Makefile`, `web/package.json`, `desktop/package.json`, and each SDK's own
build config to know what exists.

The console gives you:

- **One catalog** — `(help)` or `bb help` prints every command, grouped.
- **REPL-driven ops** — call functions, background a daemon and keep
  working, chain build steps together.
- **One-shot CLI too** — `bb release` / `bb sdk:test clojure` work without
  booting a REPL.
- **Docstrings everywhere** — `(doc build/release)` tells you exactly what
  it runs.
- **Versioned toolchain** — mise locks JDK + Clojure + Babashka so everyone
  runs the same versions.

---

## Layout

```
tools/console/
├── .mise.toml              # locks java=21, clojure, babashka
├── deps.edn                # JVM Clojure project (nREPL via :dev alias)
├── bb.edn                  # Babashka tasks (one per command)
├── README.md                # ← you are here
├── dev/
│   └── user.clj             # auto-loaded REPL helpers
└── src/console/
    ├── core.clj             # registry, (help), (ls), dispatch
    ├── shell.clj            # process helpers (sh, sh!, sh*)
    ├── path.clj              # repo-root discovery
    ├── build.clj             # root Makefile wrappers (build/release/sign/...)
    ├── sdk.clj                # all six sdk/* language packages (lang is a keyword)
    ├── web.clj                # web/ (Vite SPA)
    └── desktop.clj            # desktop/ (Tauri app)
```

---

## Prerequisites

The project pins its own toolchain via [mise](https://mise.jdx.dev/). From
this directory:

```bash
cd tools/console
mise install     # installs JDK 21, Clojure, Babashka
```

That writes nothing outside this directory — versions are recorded in
`.mise.toml` and selected automatically whenever you `cd` here (assuming you
have mise's shell hook enabled).

You still need the **underlying toolchain** the scripts shell out to:

| Used by                                   | Tool                                       |
| ------------------------------------------ | ------------------------------------------- |
| `build`, `daemon`, `agent`                | `cargo` (+ `cargo zigbuild` for `agent`)    |
| `web`, `desktop`                           | `bun` (or `npm`)                            |
| `sdk/test :ruby`, `sdk/publish :ruby`     | `ruby` + `rake`, `gem`                      |
| `sdk/test :python`, `sdk/publish :python` | `uv`                                        |
| `sdk/test :elixir`, `sdk/publish :elixir` | `elixir` + `mix` (+ `mix hex.user` login)   |
| `sdk/test :gleam`, `sdk/publish :gleam`   | `gleam` (+ Hex auth for publish)            |
| `sdk/test :typescript`                     | `bun`, `npm` (for `publish`)                |
| `sdk/test :clojure`, `sdk/publish :clojure` | `clojure`/`clj` CLI (+ `CLOJARS_USERNAME`/`CLOJARS_PASSWORD` for publish) |
| `sdk/test :go`                             | `go`                                        |
| `sdk/test :rust`, `sdk/publish :rust`     | `cargo` (+ `cargo login` for publish)       |

The console will surface a useful error (the underlying tool's own "not
found") if any of these are missing.

---

## Two ways to use it

### 1. REPL (recommended for ops sessions)

Two flavors depending on whether you live in a terminal or an editor:

```bash
cd tools/console

clj -M:rebel    # pretty terminal REPL (rebel-readline: syntax highlighting,
                # multi-line editing, inline docs, tab-completion)

clj -M:dev      # nREPL on :7888 — connect from CIDER / Calva / Cursive
```

Or from the repo root, the shortcut script does the `cd` for you:

```bash
./console        # same as: cd tools/console && clj -M:rebel
```

Both aliases include `dev/` on the classpath, so `dev/user.clj` auto-loads
and every console namespace is preloaded under short aliases:

```clojure
user=> (help)                          ;; full command catalog
user=> (ls)                            ;; same, no banner
user=> (doc sdk/test)                  ;; docstring for a command
user=> (sdk/test :clojure)             ;; run it — lang is a keyword

;; Background a daemon, keep working:
user=> (def d (sh/sh* ["target/release/bsdkrund"]))
user=> (.destroyForcibly ^Process (:proc d))
```

### 2. Babashka one-shots (recommended for shell pipelines & CI)

Starts in ~50 ms, no JVM:

```bash
cd tools/console
bb help                                # list commands
bb tasks                               # list bb tasks
bb release                             # make release
bb sdk:test clojure                    # run the Clojure SDK's test suite
bb sdk:publish clojure                 # deploy the Clojure SDK to Clojars
bb run ps --all                        # make run ARGS="ps --all"
```

Both runtimes share the same source tree under `src/`, so a wrapper added
in `sdk.clj` is immediately callable from either.

---

## Command catalog

Run `(help)` / `bb help` for the live list. Snapshot:

| Group     | Command                              | What it does                                                |
| --------- | -------------------------------------- | -------------------------------------------------------------- |
| `build`   | `build`                               | `make build` — debug build + codesign                        |
|           | `release`                             | `make release` — release build + codesign                    |
|           | `sign` / `sign-release`               | (re)codesign debug / release binaries                        |
|           | `web`                                 | `make web` — build the web SPA into `web/dist`                |
|           | `daemon`                              | `make daemon` — release build daemon + supervisor              |
|           | `agent` / `agent-linux` / `agent-freebsd` / `agent-netbsd` | cross-compile the in-guest exec agent    |
|           | `run [args...]`                       | `make run ARGS=...` — build then run                          |
|           | `test`                                | `make test` — boot FreeBSD under a PTY (e2e)                    |
|           | `clean`                               | `cargo clean`                                                   |
| `sdk`     | `deps <lang>`                         | fetch deps. lang ∈ `:elixir` `:gleam`                          |
|           | `test <lang>`                         | run tests. lang ∈ `:clojure` `:ruby` `:python` `:elixir` `:gleam` `:typescript` `:go` `:rust` |
|           | `lint <lang>`                         | lint. lang ∈ `:python`                                          |
|           | `build <lang>`                        | build the artifact. lang ∈ `:clojure` `:ruby` `:python` `:typescript` |
|           | `install <lang>`                      | install locally. lang ∈ `:clojure` (`~/.m2`)                    |
|           | `publish <lang> [args...]`            | push to the registry. lang ∈ `:clojure` `:ruby` `:python` `:typescript` `:elixir` `:gleam` `:rust` |
|           | `test-all`                            | every SDK's unit-test suite in turn                              |
| `web`     | `dev` / `build` / `typecheck` / `preview` | web/ (Vite SPA)                                          |
| `desktop` | `dev` / `build` / `tauri-dev` / `tauri-build` | desktop/ (Tauri app)                                 |

---

## Publishing an SDK

`(sdk/publish lang & args)` pushes a package to its registry — Clojars
(clojure), RubyGems (ruby), PyPI (python), npm (typescript), Hex (elixir,
gleam), crates.io (rust). It builds first where the registry needs a built
artifact (ruby, python, typescript), then pushes. Go has no registry push —
a module is published by tagging the repo (`sdk/go/vX.Y.Z`).

```clojure
(sdk/publish :clojure)              ;; clj -T:build deploy (Clojars)
(sdk/publish :ruby)                 ;; gem build + gem push
(sdk/publish :python)               ;; uv build + uv publish
(sdk/publish :typescript)           ;; bun run build + npm publish
(sdk/publish :elixir)               ;; mix hex.publish
(sdk/publish :gleam "--dry-run")    ;; extra args pass through
```

```bash
bb sdk:publish clojure
bb sdk:publish gleam --dry-run
```

⚠️ **Not sandboxed and not dry-run by default** — this really pushes to the
public registry. Each publisher expects its own credentials already set up
(`CLOJARS_USERNAME`/`CLOJARS_PASSWORD`, `gem`/`npm` login, `uv publish`'s
token, `mix hex.user auth`) — the console does not manage any of them.

---

## Adding a new command

1. Pick (or create) the right namespace in `src/console/`.
2. Add a `defn` that shells out via `console.shell`:
   ```clojure
   (defn my-new-script
     "One-liner explaining what it does."
     []
     (sh/sh ["cargo" "run" "-p" "my-crate"]))
   ```
   For a per-language command, add a `case` branch to the relevant function
   in `sdk.clj` instead of a new top-level `defn` — keep the "lang is a
   keyword atom" shape consistent.
3. Add an entry to the `registry` in `src/console/core.clj` so it shows up
   in `(help)`.
4. Optionally add a task in `bb.edn` so it has a `bb my-new-script`
   shortcut.

That's it — no compilation step, the REPL picks it up on next
`(require ... :reload)`.

---

## Design notes

- **Wrappers, not reimplementations.** Every wrapper calls the existing
  script (`make`, `bun run`, `rake`, …). This means CI and console behavior
  are identical and there is never a second implementation to keep in sync.
- **Lang is always a keyword atom, not a family of functions.** `sdk.clj`
  has one `test`/`build`/`publish`/... function each, dispatching on a
  `lang` keyword via `case`, rather than `clojure-test`/`ruby-test`/...
  — adding a language is a new `case` branch, not a new function name to
  remember.
- **Shared `src/` between clj and bb.** The source tree lives at `src/`;
  both `deps.edn` and `bb.edn` point at it. Don't import anything Babashka
  can't load (no AOT-only libs, no native-image-incompatible deps) —
  `babashka.process`/`babashka.fs` cover everything the wrappers here need.
- **Repo root via marker files.** `console.path/repo-root` walks up looking
  for a `Makefile` next to the workspace `Cargo.toml` — the one place in
  the tree both exist side by side — so commands work no matter where the
  REPL was started.
- **Foreground by default.** Long-running things (`build/run`, a daemon)
  inherit stdio, so Ctrl-C kills them. Use `console.shell/sh*` to
  background a process from the REPL.

---

## Troubleshooting

- **`clojure: command not found`** — run `mise install` in this directory.
- **`bb: command not found`** — same; locked in `.mise.toml` as `babashka`
  (the binary it installs is still called `bb`).
- **`Could not locate bsdkrun repo root`** — you started the REPL outside
  the monorepo. `cd` into the repo (or any subdir) first.
- **`unknown lang :foo`** — check `sdk.clj`'s `sdk-dir` map / the command
  catalog above for which languages a given `sdk/*` command supports; not
  every command applies to every language (e.g. only `:python` has `lint`).
- **A wrapped command fails with "not found"** — install the underlying
  tool (see the Prerequisites table above); the console does not
  auto-provision anything.

---

## What this is *not*

- Not a replacement for the `Makefile` or `package.json` scripts. Those
  still work; the console just calls them.
- Not a new build system. `cargo`/`bun`/`make` still own the actual builds.
- Not a config layer. It does not read or write any secrets/env files.
