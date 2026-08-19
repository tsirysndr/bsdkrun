<p align="center">
  <img src="../.github/assets/ci.png" alt="bsdkrun Desktop CI/CD screen — a workflow run with per-step status, timings and live logs" width="900">
</p>

# bsdkrun ci

[![e2e (bsdkrun ci / KVM)](https://github.com/tsirysndr/bsdkrun/actions/workflows/e2e-ci.yml/badge.svg)](https://github.com/tsirysndr/bsdkrun/actions/workflows/e2e-ci.yml)

Run [tangled](https://tangled.org) spindle CI workflows in bsdkrun microVMs —
locally, from one command, with nothing installed but bsdkrun itself.

```sh
bsdkrun ci run            # run every workflow that matches (manual trigger)
bsdkrun ci ls             # list workflows and whether they'd match
bsdkrun ci serve          # accept spindle pipeline records over HTTP
```

This directory is the tool itself: a Go binary compiled by `core/build.rs` and
embedded into `bsdkrun` exactly as `pack/` is. **An end user never needs Go** —
`bsdkrun ci` extracts and executes it, and the tool drives VMs through the
bsdkrun CLI itself, pointed back at the very binary that launched it
(`$BSDKRUN_BIN`).

## Why this exists

Spindle runs a repository's `.tangled/workflows/*.yml` when a knot sees a push.
That is the right place for CI to run — and the wrong place to *iterate* on it.
The push-edit-push loop for debugging a workflow is miserable everywhere, and
spindle's microvm engine only runs on Linux hosts with KVM.

`bsdkrun ci` runs the same files, in real microVMs, on the machine in front of
you. A workflow that passes here is a workflow spindle will run the same way,
because the parts that could disagree are not reimplemented:

- **The schema and `when:` matching are tangled's own Go package**
  (`tangled.org/core/workflow`), imported, not transcribed. Glob semantics,
  constraint defaults, pull-request action types — all upstream's code.
- **The environment is spindle's**: `CI=true` and the full `TANGLED_*` set
  (`TANGLED_COMMIT_SHA`, `TANGLED_REF`, `TANGLED_PR_TARGET_BRANCH`, manual
  inputs as `TANGLED_INPUT_*`, …), derived the same way.
- **The layout is spindle's**: steps run serially in one VM from
  `/tangled/workspace`, `HOME=/tangled/home`, system steps (nix config, clone)
  ahead of user steps.
- **The log format is spindle's**: `--json` emits its LogLine records
  (`kind: control|data`, `step_status`, streams), so anything that consumes a
  spindle log stream consumes this.

## What a run does

For each matching workflow:

1. **Image.** The `dependencies:` list becomes a [nixery.dev](https://nixery.dev)
   image — `nixery.dev/[arm64/]<deps…>/bash/git/coreutils/util-linux/nix` — the
   same mapping spindle's nixery engine uses (plus `util-linux`, because a
   microVM mounts `/proc` and its shares itself and needs a `mount(8)` to do it
   with; containers get that from the runtime). Both dependency spellings are
   accepted: the plain list (`engine: microvm`) and the registry map
   (`engine: nixery`). Custom registries (`github:owner/repo/rev`) install via
   `nix profile add` inside the VM. An `image:` that reads as an OCI reference
   (`ubuntu:24.04`, `ghcr.io/org/img`) boots that image directly instead — no
   nixery, no nix; the runner only ensures git is present for the clone. If
   nixery cannot serve the image (its server-side build can outlast the
   gateway timeout on big dependency sets), the run falls back to the pinned
   `nixos/nix` image and installs the dependencies with `nix profile add`.
2. **VM.** bsdkrun boots the image as a microVM (2 CPUs / 2048 MiB by default;
   `--cpus`, `--mem`). The repository is mounted **read-only** at
   `/tangled/source` — a CI step cannot write to the checkout that triggered it.
3. **Clone.** The HEAD commit is fetched from that mount into
   `/tangled/workspace` by SHA, honouring `clone:` options (depth, submodules,
   tags, skip). **The dirty working tree never runs** — CI that quietly tested
   uncommitted changes would pass locally and fail everywhere else. Commit
   first.
4. **Steps**, serially, each from the workspace, with workflow + step
   environment applied. First failure stops the workflow; the VM is destroyed
   unless `--keep`, which leaves it for `bsdkrun shell <id>`.

## Examples

Two runnable copies of the same bun test suite, one per image strategy:

- [`examples/ci-bun-ubuntu`](../examples/ci-bun-ubuntu) — `image: ubuntu:24.04`,
  a plain Ubuntu microVM; the workflow installs bun itself, no nixery involved.
- [`examples/ci-bun-nixery`](../examples/ci-bun-nixery) — `dependencies: [bun]`,
  the toolchain arrives in a nixery image and the workflow is a single step.

Each README shows the copy-out-and-run procedure (CI runs the HEAD commit, so
the example needs its own git repository).

## Triggers

A spindle gets trigger metadata from a knot event; a local run synthesizes the
same shape from the checkout:

| Flag                       | Simulates                                                                    |
| -------------------------- | ---------------------------------------------------------------------------- |
| *(default)* `--event manual` | Manual dispatch of HEAD. Constraints are skipped, as spindle skips them.    |
| `--event push`             | A push of `HEAD~1..HEAD` to the current branch; `paths:` matches those files. |
| `--event pull_request`     | A PR from the current branch onto `--branch` (default branch otherwise).      |

Naming workflows (`bsdkrun ci run test lint`) or passing files (`-f wf.yml`)
selects them directly and skips `when:` matching — naming *is* the selection.

Identity fields a local checkout does not have (knot, DIDs) are filled with
recognizable placeholders (`localhost`, `did:local:…`) rather than left empty.

## Foreign platforms

`bsdkrun ci` also runs the configs of the major CI platforms locally, in the
same microVMs. When a repository has no `.tangled/workflows`, the well-known
files are probed automatically; `--platform` forces one:

| Platform     | Config                                                                | Example                                        |
| ------------ | --------------------------------------------------------------------- | ---------------------------------------------- |
| `github`     | `.github/workflows/*.yml` (Forgejo and Gitea Actions directories too) | [`examples/ci-github`](../examples/ci-github)         |
| `gitlab`     | `.gitlab-ci.yml`                                                      | [`examples/ci-gitlab`](../examples/ci-gitlab)         |
| `woodpecker` | `.woodpecker/*.yml` or `.woodpecker.yml`                              | [`examples/ci-woodpecker`](../examples/ci-woodpecker) |
| `drone`      | `.drone.yml` (multi-document files included)                          | [`examples/ci-drone`](../examples/ci-drone)           |
| `circleci`   | `.circleci/config.yml`                                                | [`examples/ci-circleci`](../examples/ci-circleci)     |
| `buildkite`  | `.buildkite/pipeline.yml`                                             | [`examples/ci-buildkite`](../examples/ci-buildkite)   |
| `semaphore`  | `.semaphore/semaphore.yml`                                            | [`examples/ci-semaphore`](../examples/ci-semaphore)   |
| `jenkins`    | `Jenkinsfile` (declarative pipelines only)                            | [`examples/ci-jenkins`](../examples/ci-jenkins)       |
| `azure`      | `azure-pipelines.yml`                                                 | [`examples/ci-azure`](../examples/ci-azure)           |
| `codebuild`  | `buildspec.yml` (see the CodePipeline note below)                     | [`examples/ci-codebuild`](../examples/ci-codebuild)   |
| `tekton`     | `.tekton/*.yaml` (Task / Pipeline / PipelineRun manifests)            | [`examples/ci-tekton`](../examples/ci-tekton)         |
| `travis`     | `.travis.yml`                                                         | [`examples/ci-travis`](../examples/ci-travis)         |

```sh
bsdkrun ci ls                       # what was detected, and which jobs run
bsdkrun ci run                      # every runnable job, in dependency order
bsdkrun ci run build test           # just these jobs
bsdkrun ci run --platform gitlab    # force a platform when several coexist
```

Each platform's *jobs* translate — image (or `ubuntu:24.04` when none),
environment, script steps, `needs`/`requires`/stage ordering — plus the
platform's identity env (`GITHUB_SHA`, `CI_PROJECT_DIR`, `DRONE_COMMIT_SHA`,
…) pointed at the runner's own workspace. Secrets, the TUI, tracing and the
log stream all work the same as for native workflows.

**GitHub Actions `uses:` steps run for real.** Any JavaScript or composite
action executes: the runner fetches the action's `action.yml` (cached
host-side) to learn what it is, clones it into the guest at its ref,
provisions a node runtime once per job, and executes it under the genuine
Actions protocol — `INPUT_*` from `with:` (defaults included, expression
defaults evaluated where the subset allows and dropped rather than
mistranslated otherwise), and `GITHUB_ENV` / `GITHUB_PATH` /
`GITHUB_OUTPUT` command files whose effects persist into every later step,
`run:` steps included. `${{ steps.<id>.outputs.* }}` resolves in the guest,
where the outputs live. Inject `--secret GITHUB_TOKEN` to authenticate
actions that call the GitHub API. Container actions are refused visibly (a
microVM runs no Docker daemon), as are `pre`/`post` hooks — stated limits.
See [`examples/ci-github-actions`](../examples/ci-github-actions), which
runs the actual `oven-sh/setup-bun@v2`.

**Buildkite plugins run for real.** A Buildkite plugin is a git repository
of shell hooks, so the runner clones it at its ref, exports its
configuration as `BUILDKITE_PLUGIN_<NAME>_<KEY>` (nested maps flattened,
arrays indexed — Buildkite's own scheme), and wraps the step's command in
the agent's own hook order: `environment` sourced, `pre-command`, the
command, `post-command` with the exit status preserved. See
[`examples/ci-buildkite-plugins`](../examples/ci-buildkite-plugins).

**Jenkins runs itself when translation is not enough.** A Jenkins plugin
is Java living inside Jenkins' runtime — so for scripted pipelines,
`script { }` blocks and plugin steps, the runner assembles a real headless
Jenkins in the guest ([Jenkinsfile
Runner](https://github.com/jenkinsci/jenkinsfile-runner): multi-arch JDK
image, pinned `jenkins.war`, plugins resolved by the official
plugin-installation-manager, your repo's `plugins.txt` included) and
executes the Jenkinsfile in it. Purely structural pipelines keep the fast
translation path. See
[`examples/ci-jenkins-real`](../examples/ci-jenkins-real).

**Drone plugins run for real, without a Docker daemon.** A Drone plugin is
a container image whose settings arrive as `PLUGIN_*` env and whose
entrypoint does the work — so the runner pulls the plugin image's rootfs
host-side at plan time (same cache, same registry resilience), mounts it
read-only into the guest, overlays it writable, binds the workspace at
`/drone/src`, and chroot-executes the entrypoint with Drone's exact
settings flattening. No shell needed inside the image — scratch plus one
static binary works. Plugins that need an actual Docker daemon
(`plugins/docker`) fail inside with their own error. See
[`examples/ci-drone-plugins`](../examples/ci-drone-plugins), which runs
the real `plugins/download`. Woodpecker pipelines get the identical
machinery — same `PLUGIN_*` protocol, plus Woodpecker's `CI_*` identity
variables — via [`examples/ci-woodpecker-plugins`](../examples/ci-woodpecker-plugins).

**CircleCI orbs expand for real.** An orb is YAML in a public registry —
commands, jobs and executors, parameterized with `<< parameters.x >>` —
so the runner fetches the source at plan time (partial versions like `@3`
resolve registry-side) and expands it: orb jobs referenced from workflows
become full jobs with their executor's docker image, orb commands used as
steps inline their runs with parameters substituted, and `when`/`unless`
branches are decided from the resolved values. Cache, workspace and
artifact steps become visible no-ops; an orb that fails to fetch degrades
to visible skips, never silence. See
[`examples/ci-circleci-orbs`](../examples/ci-circleci-orbs), which runs
the real `circleci/shellcheck` orb.

**Tekton resolves catalog tasks and honors per-step images.** A `taskRef`
naming a task the checkout does not carry is fetched from the tektoncd
catalog — by version from the catalog repo, or newest via Artifact Hub —
exactly as the hub resolver would in a cluster, array params included. And
because Tekton gives every step its own container, a step whose image
differs from the one the VM booted runs chrooted into its own pulled
rootfs over the shared workspace, rather than being announced as
unsupported. See
[`examples/ci-tekton-catalog`](../examples/ci-tekton-catalog).

What deliberately does not translate elsewhere is announced rather than
faked: human gates and cross-pipeline triggers become
visible skipped steps in the timeline; matrix strategies run once and say
so; **jobs that ask for windows or macos are skipped** — a Linux microVM
cannot become another OS, and a green checkmark on a lie helps nobody.
Images without bash (alpine) run their steps under `sh`, exactly as
GitLab's own runner would.

AWS CodePipeline deserves a note: a CodePipeline definition is pure
orchestration — its actions *reference* CodeBuild projects, Lambda functions
and deploy providers, and contain no commands. What a laptop can truthfully
run is the CodeBuild project's `buildspec.yml`, so that is what translates
(phases in their fixed order); `parameter-store` and `secrets-manager`
values live in AWS and are announced as unresolved rather than faked —
inject them with `--secret` when a step needs them.

Jenkins deserves its own footnote: only **declarative** Jenkinsfiles
translate, parsed by a small structural tokenizer — not a Groovy
implementation, because none is needed for the declarative skeleton and
none short of Jenkins itself would suffice for the scripted dialect. A
scripted pipeline (`node { ... }`) is refused with a clear error rather
than mistranslated, and `environment` values that are Groovy expressions
(`credentials(...)`) are dropped rather than faked.

## No config at all? Project detection

When a repository has no CI configuration of any kind — and whenever
`--detect` asks for it explicitly — `bsdkrun ci` reads the project itself
and generates a workflow on the fly, the way `bsdkrun pack` detects what to
build. One provider per language, specific markers first (bun's lockfile
beats package.json; composer.json beats package.json), each choosing an
official image and emitting install/test/build steps — **tests run before
the build**, and a step that would fail vacuously (a test runner with no
tests) is only generated when its subject exists.

Detected today: go, rust, nodejs, bun, deno, python, ruby, php, elixir,
gleam, zig, clojure, dotnet, crystal, haskell.

```
$ bsdkrun ci
detected go project (go.mod) — no CI configuration found, workflow generated:
  build · image golang:1.23-alpine
    1. go test
    2. go build
```

Everything the generated workflow says is announced before anything boots —
a workflow the operator has not read must be shown, not sprung.

**Deploy detection**: the secrets say where a project ships. When the
injected secrets include a known deploy token — `RAILWAY_TOKEN`,
`FLY_API_TOKEN`, `CLOUDFLARE_API_TOKEN`, `VERCEL_TOKEN`,
`NETLIFY_AUTH_TOKEN`, `DENO_DEPLOY_TOKEN`, `KOYEB_TOKEN`,
`HEROKU_API_KEY` — the generated workflow gains a deploy step (first match
wins; runners-up are announced). `--dry-run` makes the step announce the
exact command instead of running it:

```
$ bsdkrun ci --dry-run --secret RAILWAY_TOKEN
deploy: railway (RAILWAY_TOKEN detected) [dry-run]
...
  ▶ deploy (railway) [dry-run]
    [dry-run] would deploy to railway (RAILWAY_TOKEN detected): railway up --detach
```

Detection keys on the secret's *name*, never its value, and only generated
workflows gain the step — a committed CI config already says what it
deploys.

## Secrets

Spindle injects a repository's vault secrets as environment variables into
every step. A local run has no vault, so the values come from you:

```sh
bsdkrun ci run --secret NPM_TOKEN            # value from the host environment
bsdkrun ci run --secret API_KEY=xyz          # explicit value
bsdkrun ci run --secrets-file .env.ci        # dotenv file (repeatable)
```

`.tangled/secrets.env` in the workflow root is loaded automatically when it
exists — **gitignore it**: the clone step runs the committed tree, and a
committed secrets file would ride into every guest.

Secrets reach every step's environment (they beat the workflow's committed
`environment:`, a step's own env stays most specific), and their values are
masked as `***` in every output — human, `--json`, the TUI, the desktop and
web screens — including their base64 encodings, so `echo "$TOKEN" | base64`
leaks nothing either. Masking follows spindle's own SecretMask semantics.

## OpenTelemetry tracing

Every run is a trace: one root span per workflow, one child span per step
(the boot included), each span carrying the workflow, repository and step
attributes plus the error message when a step fails.

Spans are **always recorded locally**, into the engine's SQLite — no
collector required:

```sh
bsdkrun ci traces               # recent runs: trace id, workflow, status, duration
bsdkrun ci spans <trace-id>     # that run's steps, with per-span timings
```

The daemon exposes the same history over GraphQL (`ciTraces`,
`ciTraceSpans`), which is what the desktop/web CI screen's **Trace** view
renders — a live waterfall that fills in span by span while the run is
still going.

<p align="center">
  <img src="../.github/assets/traces.png" alt="Trace view — a waterfall of the run's spans, one per step, boot included" width="900">
</p>

To export to a real collector (Jaeger, Grafana Tempo, anything OTLP), point
the standard variable — or the flag — at it:

```sh
bsdkrun ci run --otlp http://localhost:4318
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 bsdkrun ci run
```

Each span is POSTed to `/v1/traces` (OTLP/HTTP JSON) **the moment it ends**,
so a collector's live view fills in step by step; the root span lands last
and closes the trace. Export is fire-and-forget with a short timeout: a slow
collector never slows the build it is observing, and a dead one never fails
it. The encoder is hand-rolled — one file, no OpenTelemetry SDK dependency —
because a CI runner needs exactly one shape of span.

## `bsdkrun ci serve`

The server half: a runner that accepts spindle's own `sh.tangled.pipeline`
records over HTTP and executes each workflow in a microVM.

```sh
bsdkrun ci serve --bind 0.0.0.0:8517
curl -X POST localhost:8517/pipelines -d @pipeline.json   # → {"id":"run-1"}
curl localhost:8517/pipelines/run-1                       # status + step results
curl localhost:8517/pipelines/run-1/logs                  # spindle LogLine JSON
```

Deliberately the *runner seam*, not the whole spindle: jetstream ingestion,
XRPC, secrets and AT-proto record publishing stay with spindle (or
[tack](https://github.com/mitchellh/tack)). This serves the piece bsdkrun is
uniquely placed to provide — executing a pipeline in real VMs — behind an
interface small enough to point either of them at, or curl by hand. In serve
mode the clone fetches from the knot URL in the record's trigger metadata.

## Workflows from code

Every [bsdkrun SDK](../sdk) can define workflows as code and run them — the YAML is
generated, byte-compatible with what spindle parses, and never has to be
written by hand:

```go
// Go
bsdkrun.Workflow("test").OnPush("main").Deps("go", "gcc").
    Step("test", "go test ./...").Run()
```

```elixir
# Elixir
Bsdkrun.CI.workflow("test")
|> Bsdkrun.CI.on_push("main")
|> Bsdkrun.CI.deps(["elixir", "erlang"])
|> Bsdkrun.CI.step("test", "mix test")
|> Bsdkrun.CI.run()
```

`yaml()` renders the file, `save(repo)` commits it to `.tangled/workflows/` for
spindle to run on push, and `run()` executes it immediately in a microVM
without touching the repository. The same surface exists in TypeScript,
Python, Rust, Ruby, Clojure, Gleam and Scala.

## Environment

| Variable                       | Effect                                                                 |
| ------------------------------ | ---------------------------------------------------------------------- |
| `BSDKRUN_BIN`                  | The bsdkrun binary the SDK drives (set automatically by `bsdkrun ci`). |
| `BSDKRUN_CI_NIXERY`            | A self-hosted nixery instance instead of `nixery.dev`.                 |
| `OTEL_EXPORTER_OTLP_ENDPOINT`  | Export one OpenTelemetry span per step to this collector (see above).  |

## Building

`cargo build` at the repo root compiles this automatically when Go ≥ 1.25 is on
PATH (see `core/build.rs::ensure_ci_binary`); without Go, bsdkrun builds
normally and `bsdkrun ci` explains what is missing. Under nix, the flake's
`ciBin` derivation builds it and `preBuild` drops it into `core/src/ci-bin/`.

The tool drives VMs with its own thin CLI driver (`driver.go`), not the Go
SDK: a module `replace` to `../sdk/go` looked right but froze the SDK inside
nix's fixed-output vendor step, shipping stale code that go.sum could not see
change. The CLI flags it drives are the same public surface every SDK uses.
