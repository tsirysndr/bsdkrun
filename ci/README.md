<p align="center">
  <img src="../.github/assets/ci.png" alt="bsdkrun Desktop CI/CD screen — a workflow run with per-step status, timings and live logs" width="900">
</p>

# bsdkrun ci

[![e2e (bsdkrun ci / KVM)](https://github.com/tsirysndr/bsdkrun/actions/workflows/e2e-ci.yml/badge.svg)](https://github.com/tsirysndr/bsdkrun/actions/workflows/e2e-ci.yml)

Run CI in bsdkrun microVMs — locally, from one command, with nothing
installed but bsdkrun itself. **Not only [tangled](https://tangled.org)
workflows:** it reads the configuration your repository already has, from
whichever provider it belongs to.

```sh
bsdkrun ci run            # run every workflow that matches (manual trigger)
bsdkrun ci ls             # list workflows and whether they'd match
bsdkrun ci serve          # be a spindle: serve its full API on libkrun VMs
```

Point it at a repository and it detects the platform and runs it:

| Provider                                | Config it reads                          |
| --------------------------------------- | ---------------------------------------- |
| tangled spindle                         | `.tangled/workflows/*.yml`               |
| GitHub Actions (and Forgejo, Gitea)     | `.github/workflows/*.yml`                |
| GitLab CI                               | `.gitlab-ci.yml`                          |
| CircleCI                                | `.circleci/config.yml`                   |
| Drone / Woodpecker                      | `.drone.yml`, `.woodpecker/*.yml`        |
| Buildkite                               | `.buildkite/pipeline.yml`                |
| Jenkins                                 | `Jenkinsfile`                            |
| Azure Pipelines                         | `azure-pipelines.yml`                    |
| AWS CodeBuild                           | `buildspec.yml`                          |
| Tekton                                  | `.tekton/*.yaml`                         |
| Semaphore                               | `.semaphore/semaphore.yml`               |
| Travis                                  | `.travis.yml`                            |

And the ecosystem around those files runs for real, not in effigy: **GitHub
Actions** `uses:` steps execute (JavaScript and composite actions, the full
`INPUT_*`/`GITHUB_ENV`/`GITHUB_OUTPUT` protocol), **Buildkite plugins** run
their hooks, **Drone and Woodpecker plugins** execute their container images,
**CircleCI orbs** are fetched from the registry and expanded, **Tekton**
resolves catalog tasks, and a **Jenkinsfile** that is a Groovy program is run
by an actual headless Jenkins. A repository with no CI configuration at all
is detected by language and gets a workflow generated for it.

Whatever cannot be translated is announced as a visible skip rather than
quietly dropped — the [foreign platforms](#foreign-platforms) section is
explicit about where each line is drawn.

This directory is the tool itself: a Go binary compiled by `core/build.rs` and
embedded into `bsdkrun` exactly as `pack/` is. **An end user never needs Go** —
`bsdkrun ci` extracts and executes it, and the tool drives VMs through the
bsdkrun CLI itself, pointed back at the very binary that launched it
(`$BSDKRUN_BIN`).

## Contents

- [Why this exists](#why-this-exists)
- [What a run does](#what-a-run-does)
- [Dropping into the machine — `--sh`](#dropping-into-the-machine----sh)
- [Examples](#examples)
- [Triggers](#triggers)
- [Foreign platforms](#foreign-platforms) — the twelve providers, and what runs for real
- [No config at all? Project detection](#no-config-at-all-project-detection)
- [Secrets](#secrets)
- [OpenTelemetry tracing](#opentelemetry-tracing)
- [`bsdkrun ci serve`](#bsdkrun-ci-serve)
- [Self-hosting a spindle](#self-hosting-a-spindle) — run this in place of spindle.tangled.sh
  - [Build](#build)
  - [Required configuration](#required-configuration)
  - [The rest of the configuration](#the-rest-of-the-configuration)
  - [Quick guide: from zero to a running pipeline](#quick-guide-from-zero-to-a-running-pipeline)
  - [What it serves](#what-it-serves)
  - [How work arrives](#how-work-arrives)
- [Workflows from code](#workflows-from-code)
- [Environment](#environment)
- [Building](#building)

## Why this exists

CI runs on someone else's machine, and that is the wrong place to *iterate* on
it. The push-edit-push loop for debugging a workflow is miserable on every
provider: a one-character fix costs a commit and a wait, and the environment
you are debugging is one you cannot open a shell into.

`bsdkrun ci` runs the real files — your `.github/workflows`, your
`.gitlab-ci.yml`, your `Jenkinsfile`, your `.tangled/workflows` — in real
microVMs on the machine in front of you. Where a provider's own runner needs
Linux and a container daemon, this needs neither: libkrun boots the VM on
macOS and Linux alike.

The tangled path holds to a stronger promise, because it can: a workflow that
passes here is one spindle will run the same way, since the parts that could
disagree are not reimplemented:

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

## Dropping into the machine — `--sh`

A failing step's log tells you what broke. The machine tells you why.

```sh
bsdkrun ci run --sh          # --shell and --ssh work too
```

With `--sh` the microVM is **not** destroyed when the workflow ends — pass or
fail — and you land in an interactive shell inside it: same image, same
environment, same workspace with whatever the run left in it, sitting in the
directory the steps ran from. Re-run the failing command, poke at the build
tree, install something and try again. The VM goes away when you exit the
shell; `--keep` keeps it beyond that for a later `bsdkrun shell <id>`.

Without a terminal on stdin (a pipe, a CI job, an editor task) there is
nobody to hand a shell to, so the run says so and leaves the machine up with
the exact commands to attach or remove it — it never blocks waiting for input
that cannot come.

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

That is the small surface — enough to point spindle, [tack](https://github.com/mitchellh/tack)
or curl at. In serve mode the clone fetches from the knot URL in the record's
trigger metadata rather than a mounted checkout.

For the real thing — a server you can put in an existing spindle's place —
see below.

## Self-hosting a spindle

`bsdkrun ci serve` can *be* your spindle: same routes, same request and
response shapes, same service-auth, same SQLite schema, same event stream,
same on-disk log format. That is not a reimplementation — the XRPC handlers,
the ACL, the storage and the log format are spindle's own code, imported from
`tangled.org/core`. What bsdkrun replaces is the one seam spindle leaves open,
`models.Engine`, so workflows run in libkrun microVMs instead of qemu ones.

The appview cannot tell the difference: it verifies a spindle by calling
`sh.tangled.owner`, and every pipeline it later reads comes back through
`sh.tangled.ci.queryPipelines`, `getPipeline` and `subscribePipelineLogs`.

### Build

Spindle support is a build-tag feature, off by default:

```sh
BSDKRUN_CI_SPINDLE=1 cargo build --release
make sign-release            # macOS: re-sign, as after any build
```

**Published releases already have it** — the release workflow builds with
this flag and fails if the support did not make it in — so swapping a spindle
needs no rebuild. The flag is for local builds.

It is a flag at all because spindle's storage is SQLite through
`mattn/go-sqlite3`, which needs cgo, while the embedded `ci` binary is
otherwise built `CGO_ENABLED=0` so it runs against any host libc, musl
included. A cgo build links against the libc it was built on: the published
Linux binary therefore wants a glibc no older than the release runner's. If
you need a binary for a musl host (Alpine) or an older distribution, build it
there, or build without the flag if you only want `bsdkrun ci run`.

A build without the tag accepts `--spindle` and tells you how to get one,
rather than starting a server that silently lacks the API.

### Required configuration

Configuration is spindle's own, read from the same environment variables, so
an existing deployment's env file works unchanged. Two are required:

| Variable                  | Meaning                                                                     |
| ------------------------- | --------------------------------------------------------------------------- |
| `SPINDLE_SERVER_HOSTNAME` | The public hostname. Becomes this service's DID — `did:web:<hostname>` — and every service-auth token must be minted for that audience, so it must match what the appview knows. A port is percent-encoded: `localhost:6555` → `did:web:localhost%3A6555`. |
| `SPINDLE_SERVER_OWNER`    | DID of the owner. Returned by `sh.tangled.owner`, and the only DID that may add spindle members. |

Setting `SPINDLE_SERVER_HOSTNAME` is what turns spindle mode on — no flag
needed, since an operator swapping spindle out already exports it. `--spindle`
forces it.

### The rest of the configuration

| Variable                             | Default                          | Meaning                                            |
| ------------------------------------ | -------------------------------- | -------------------------------------------------- |
| `SPINDLE_SERVER_LISTEN_ADDR`         | `0.0.0.0:6555`                   | Listen address. `--bind` overrides it.             |
| `SPINDLE_SERVER_DB_PATH`             | `spindle.db`                     | SQLite file: pipelines, events, ACL and (by default) secrets. Point it at your existing spindle.db to keep history. |
| `SPINDLE_SERVER_LOG_DIR`             | `/var/log/spindle`               | One JSONL file per workflow, in spindle's format.  |
| `SPINDLE_SERVER_REPO_DIR`            | `repos`                          | Where repositories are sparsely checked out to read `.tangled/workflows`. |
| `SPINDLE_SERVER_QUEUE_SIZE`          | `100`                            | Pending pipelines before new ones are refused.     |
| `SPINDLE_SERVER_MAX_JOB_COUNT`       | `2`                              | Pipelines running at once.                         |
| `SPINDLE_SERVER_SECRETS_PROVIDER`    | `sqlite`                         | `sqlite` or `openbao`.                             |
| `SPINDLE_SERVER_SECRETS_OPENBAO_PROXY_ADDR` | `http://127.0.0.1:8200`   | OpenBao proxy, when that provider is chosen.       |
| `SPINDLE_SERVER_SECRETS_OPENBAO_MOUNT`      | `spindle`                 | OpenBao mount path.                                |
| `SPINDLE_SERVER_PLC_URL`             | `https://plc.directory`          | DID resolution.                                    |
| `SPINDLE_SERVER_DEV`                 | `false`                          | Talk to knots over http/ws instead of https/wss.   |

Host requirements: git ≥ 2.49 (spindle's floor, and the same sparse fetch
happens here — the server refuses to start otherwise), and whatever libkrun
needs to boot a VM. No qemu, no cgroups, no Docker daemon.

```sh
export SPINDLE_SERVER_HOSTNAME=spindle.example.com
export SPINDLE_SERVER_OWNER=did:plc:yourownerdid
export SPINDLE_SERVER_DB_PATH=/var/lib/spindle/spindle.db
export SPINDLE_SERVER_LOG_DIR=/var/log/spindle
bsdkrun ci serve

# it prints its DID — the audience every token must target
curl -s https://spindle.example.com/xrpc/sh.tangled.owner
# {"owner":"did:plc:yourownerdid"}
```

### Quick guide: from zero to a running pipeline

Five minutes, locally, with nothing else installed. This uses the real API —
the same calls the appview makes.

**1. Build with spindle support and start it.**

```sh
BSDKRUN_CI_SPINDLE=1 cargo build --release && make sign-release

export SPINDLE_SERVER_HOSTNAME=localhost:6555
export SPINDLE_SERVER_OWNER=did:plc:yourownerdid   # your own DID
export SPINDLE_SERVER_DB_PATH=./spindle.db
export SPINDLE_SERVER_LOG_DIR=./logs
export SPINDLE_SERVER_LISTEN_ADDR=127.0.0.1:6555
export SPINDLE_SERVER_DEV=true                     # knots over http, for local ones

bsdkrun ci serve
```

The banner prints the DID it derived — `did:web:localhost%3A6555` here. Every
authenticated call must be signed for **that** audience; a mismatch is the
usual cause of a `403 Auth` on an otherwise correct request.

**2. Check it is alive.** No auth needed:

```sh
curl -s localhost:6555/xrpc/sh.tangled.owner
# {"owner":"did:plc:yourownerdid"}
```

If you are pointing a tangled appview at it, this is the call it makes to
verify the spindle — nothing else is required for it to accept the server.

**3. Watch the event stream** in a second terminal. It is a WebSocket, and the
cursor is unix *nanoseconds*; `0` means from the beginning:

```sh
websocat "ws://localhost:6555/events?cursor=0"
```

Every pipeline record and every workflow status transition appears here as it
happens — this is what a dashboard consumes.

**4. Trigger a pipeline.** This one needs service auth: an AT Protocol
service-auth JWT, signed by your DID's key, whose `aud` is the server's DID
and whose `lxm` is the exact method being called. Your PDS mints it — this is
the same call the appview makes:

```sh
mint() {   # mint <lxm>
  curl -s -H "Authorization: Bearer $PDS_ACCESS_JWT" \
    "$PDS_HOST/xrpc/com.atproto.server.getServiceAuth?aud=did:web:localhost%3A6555&lxm=$1&exp=$(( $(date +%s) + 300 ))" \
    | jq -r .token
}
TOKEN=$(mint sh.tangled.ci.triggerPipeline)

curl -s -X POST localhost:6555/xrpc/sh.tangled.ci.triggerPipeline \
  -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
        "repo": "did:plc:yourrepodid",
        "trigger": {
          "$type": "sh.tangled.ci.trigger#manual",
          "sha": "0000000000000000000000000000000000000000",
          "ref": "refs/heads/main"
        }
      }'
# {"pipeline":"at://did:web:localhost%3A6555/sh.tangled.pipeline/3l..."}
```

A token is bound to one method: `lxm` must equal the endpoint you call, so
each of the calls below needs its own. Tokens are short-lived (60s minimum
expiry); mint them per request rather than caching.

The server fetches `.tangled/workflows` from the repo at that SHA, compiles
the workflows whose `when:` matches, boots one microVM per workflow and runs
them. The `sha` must be a full 40-character hash — the API rejects short ones.

**5. Follow the run.**

```sh
# the pipeline and its workflow statuses
curl -s "localhost:6555/xrpc/sh.tangled.ci.queryPipelines?repo=did:plc:yourrepodid" | jq

# live logs (CBOR frames — this is the endpoint the appview uses)
# rkey is the last path segment of the at:// URI above
curl -s "localhost:6555/xrpc/sh.tangled.ci.getPipeline?pipeline=3l..." | jq

# or the raw JSONL stream, one text frame per line
websocat "ws://localhost:6555/logs/localhost%3A6555/3l.../my-workflow"
```

**6. Give a repo a secret.** Note the difference in identifiers, which is easy
to get wrong: secrets take the repo's **AT-URI**, while trigger and query take
its **DID**.

```sh
TOKEN=$(mint sh.tangled.repo.addSecret)   # the helper from step 4

curl -s -X POST localhost:6555/xrpc/sh.tangled.repo.addSecret \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"repo":"at://did:plc:you/sh.tangled.repo/abc","key":"NPM_TOKEN","value":"s3cret"}'
```

Secrets reach steps as environment variables and are masked in the logs. Only
names come back from `listSecrets`; values never leave the store.

**Swapping a running spindle.** Stop it, point `SPINDLE_SERVER_DB_PATH` at its
existing `spindle.db` (repos, ACL, secrets and pipeline history all live
there), keep `SPINDLE_SERVER_HOSTNAME` identical so the DID and every issued
token stay valid, and start `bsdkrun ci serve` on the same address. Clients do
not need to know.

### What it serves

| Route                                          | Auth         |                                                     |
| ---------------------------------------------- | ------------ | --------------------------------------------------- |
| `GET /xrpc/sh.tangled.owner`                    | none         | Owner DID; this is how the appview verifies a spindle. |
| `GET /xrpc/sh.tangled.ci.queryPipelines`        | none         | Pipelines for a repo, cursor-paged.                 |
| `GET /xrpc/sh.tangled.ci.getPipeline`           | none         | One pipeline by rkey.                               |
| `GET /xrpc/sh.tangled.ci.subscribePipelineLogs` | none         | Live logs, CBOR frames over WebSocket.              |
| `POST /xrpc/sh.tangled.ci.triggerPipeline`      | service auth | Compile the repo's workflows at a SHA and run them. |
| `POST /xrpc/sh.tangled.ci.cancelPipeline`       | service auth | Cancel workflows; tears the VMs down.               |
| `POST /xrpc/sh.tangled.repo.addSecret`          | service auth | Store a secret for a repo.                          |
| `POST /xrpc/sh.tangled.repo.removeSecret`       | service auth | Remove one.                                         |
| `GET /xrpc/sh.tangled.repo.listSecrets`         | service auth | Names only — values are never returned.             |
| `GET /events`                                   | none         | WebSocket: every pipeline and status event after `?cursor=<unix-nanos>`. |
| `GET /logs/{knot}/{rkey}/{name}`                | none         | WebSocket: the raw JSONL log stream.                |
| `GET /`                                         | none         | MOTD, including the server's DID.                   |

Engine names are all aliases for the bsdkrun engine: a workflow that says
`engine: nixery` or `engine: microvm` keeps running after the swap instead of
failing as an unknown engine.

### How work arrives

Three ways, and you do not have to choose:

- **A push.** Every knot the server knows about is consumed over a WebSocket,
  and a `sh.tangled.git.refUpdate` compiles the repo's workflows at the new
  SHA and runs the ones whose `when:` matches a push. `git push -o skip-ci`
  (or `ci-skip`) is obeyed. An event is only honoured from the knot the repo
  actually lives on — a knot may not speak for repos it does not host.
- **An assignment.** The jetstream firehose is filtered to the DIDs this
  server cares about; a `sh.tangled.repo` record naming your
  `SPINDLE_SERVER_HOSTNAME` in its `spindle` field records the repo, grants
  its owner the repo policies, and adds its knot to the list above. So
  assigning a repo in tangled is all it takes to start getting its pushes.
  `sh.tangled.spindle.member` records add members — but only when the record
  names this instance *and* the DID that wrote it is allowed to invite.
- **An API call.** `sh.tangled.ci.triggerPipeline`, as above.

Both streams are best-effort: a server that cannot reach the firehose still
serves its API and still runs what it is told to run. The startup banner says
which knots it is listening to, because "none" is the quiet reason a push
never starts anything.

Not yet implemented: pull-request pipelines from `sh.tangled.repo.pull`
records, and the embedded tap relay. A pull request can still be run through
`triggerPipeline` with a `#pullRequest` trigger.

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
