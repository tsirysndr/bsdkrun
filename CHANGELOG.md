# Changelog

Notable changes to `bsdkrun` and its SDKs. Roughly [Keep a Changelog][kac], and
[semantic versioning][semver] — with the caveat that this is 0.x, so a minor
may still change behaviour.

Entries before 0.10.0 were written from the git history after the fact, so they
describe what each release contained rather than what its notes said at the
time.

**The SDKs version independently of the CLI.** They talk to `bsdkrund` over
gRPC/GraphQL and gain features on their own schedule, so `bsdkrun 0.10.0` does
not imply `bsdkrun-sdk 0.10.0`. Each release below lists the SDK versions that
shipped alongside it.

[kac]: https://keepachangelog.com/en/1.1.0/
[semver]: https://semver.org/spec/v2.0.0.html

## [Unreleased]

### Added

- **GitHub Actions `uses:` steps execute for real.** A genuine actions
  runner for JavaScript and composite actions: action.yml fetched and
  parsed at plan time, the action cloned into the guest at its ref, a node
  runtime provisioned once per job, and the real Actions protocol
  throughout — `INPUT_*` with expression-aware defaults, GITHUB_ENV /
  GITHUB_PATH / GITHUB_OUTPUT command files persisting across steps, and
  run-time resolution of step outputs. Container actions and pre/post
  hooks are refused visibly. Verified by running the actual
  oven-sh/setup-bun@v2 end to end.
- **Tekton catalog tasks resolve, and per-step images run.** A `taskRef`
  the checkout does not carry is fetched from the tektoncd catalog (by
  version from the catalog repo, newest via Artifact Hub) the way the hub
  resolver would, with array params expanding into argv elements. Steps
  declaring an image other than the VM's now chroot into their own pulled
  rootfs over the shared workspace instead of being announced as
  unsupported.
- **CircleCI orbs expand for real** — orb source is fetched from the
  registry at plan time and expanded with full parameter substitution:
  orb jobs become jobs (executor resolved to the VM image), orb commands
  inline their steps, `when`/`unless` conditions (scalar, equal, and, or,
  not) are evaluated from resolved values. Cache/workspace/artifact steps
  are visible no-ops; unfetchable orbs degrade to visible skips.
- **Drone plugins execute for real, daemon-less** — the plugin image's
  rootfs is pulled host-side (new `bsdkrun image pull --json` verb),
  mounted read-only into the guest, overlaid writable, and the entrypoint
  chroot-executed with Drone's PLUGIN_* settings flattening, the workspace
  bound at /drone/src. Scratch-plus-one-binary plugin images work; daemon
  plugins fail inside with their own error. Woodpecker settings steps ride
  the same path, with `CI=woodpecker` and `CI_*` identity alongside the
  drone-compatible `PLUGIN_*`/`DRONE_*` set.
- **Buildkite plugins execute for real** — cloned at their ref, configured
  through Buildkite's own BUILDKITE_PLUGIN_* env flattening, hooks wrapped
  around the command in the agent's order (environment sourced,
  pre-command, command, post-command with exit status preserved).
- **Jenkins runs itself when translation is not enough.** Scripted
  pipelines, script blocks and plugin steps execute under Jenkinsfile
  Runner — a real headless Jenkins assembled in the guest (pinned war,
  plugins resolved by the official plugin manager, the repo's plugins.txt
  honored). Structural pipelines keep the fast translation path.
- **Project detection: CI with no config at all.** When a repository has no
  recognizable CI configuration (or `--detect` forces it), `bsdkrun ci`
  detects the project — go, rust, nodejs, bun, deno, python, ruby, php,
  elixir, gleam, zig, clojure, dotnet, crystal, haskell — and generates and
  runs a workflow on the fly, announcing the detected language, marker and
  every step before anything boots. Tests run before the build, and steps
  that would fail vacuously are only generated when their subject exists.
  Providers live one-per-package under `ci/project`, mirroring pack's
  structure.

- **Deploy detection from secret names.** A generated workflow gains a
  deploy step when the injected secrets name a target — RAILWAY_TOKEN,
  FLY_API_TOKEN, CLOUDFLARE_API_TOKEN, VERCEL_TOKEN, NETLIFY_AUTH_TOKEN,
  DENO_DEPLOY_TOKEN, KOYEB_TOKEN, HEROKU_API_KEY — first match wins,
  runners-up are announced, and `--dry-run` makes the step announce the
  exact command instead of running it. Generated workflows only: a
  committed CI config already says what it deploys. One target per file
  under `ci/project/deploy`.

### Fixed

- **Steps no longer run login shells.** The guest agent already hands every
  exec the image's ENV (toolchain PATHs included), and `-l` made
  /etc/profile *reset* PATH — golang:alpine's `go` vanished from steps
  while plain execs saw it. The prepare step also republishes the exec
  environment into /etc/profile.d for anyone shelling into a kept VM.

## [0.11.1] — 2026-08-19

CI grows outward: the major platforms' configs run locally, secrets reach
every surface, and one example per platform holds it all to account on CI.

### Added

- **Foreign CI platforms run locally.** `bsdkrun ci` now translates and runs
  GitHub Actions (plus Forgejo/Gitea), GitLab CI, Woodpecker, Drone,
  CircleCI, Buildkite, Semaphore, Jenkins (declarative pipelines,
  via a structural parser — scripted Groovy is refused, not mistranslated),
  Azure Pipelines, AWS CodeBuild (the runnable half of CodePipeline),
  Tekton and Travis configs in microVMs — auto-detected from
  their well-known files, or forced with `--platform`. Jobs translate
  (images, env, ordering, platform identity variables); what cannot
  translate becomes a visible skip, and non-Linux jobs are skipped outright.
- **Secrets for CI runs.** `--secret KEY[=VALUE]`, `--secrets-file`, and an
  auto-loaded (gitignored) `.tangled/secrets.env` inject environment variables
  into every step; values — and their base64 encodings — are masked as `***`
  in all output, following spindle's own masking semantics.
- **Secrets & env in the desktop/web CI screens.** A per-repository editor
  (dotenv text, stored locally) injects into every step of that repo's runs
  — native and foreign platforms alike. Values travel as environment
  (`BSDKRUN_CI_SECRETS`), never argv; the daemon's `runCi` subscription
  gained a `secrets` argument for the web app.
- **Platform visibility everywhere.** The CI screens show a platform badge
  next to the workflow list, `bsdkrun ci run` prints `workflow name
  [gitlab]` (the TUI shows the same tag), and `ci ls --json` emits one
  unified shape for native and foreign listings, `platform` included.
- Twelve runnable examples, one per platform (`examples/ci-github` …
  `ci-tekton`), each asserted end to end by the e2e sweep on x86_64/KVM.

## [0.11.0] — 2026-08-18

The CI release: tangled spindle workflows running in microVMs — from one
command, from every SDK, from every screen — with tracing to match.

### Added

- **`bsdkrun ci` — a local CI runner for [tangled](https://tangled.org)
  spindle workflows.** It runs a repository's `.tangled/workflows/*.yml` in
  real microVMs: the schema and `when:` matching are tangled's own Go package
  (imported, not transcribed), the environment, layout and log format are
  spindle's, and a manual run executes HEAD — never the dirty working tree.
  `dependencies:` become a nixery image; `bsdkrun ci serve` accepts spindle's
  `sh.tangled.pipeline` records over HTTP as the server half.
- **Workflows as code in every SDK.** All nine SDKs gained a CI builder:
  `yaml()` renders spindle-compatible YAML, `save(repo)` commits it to
  `.tangled/workflows/`, `run()` executes it in a microVM immediately.
- **CI/CD screens everywhere.** The desktop and web apps gained a CI/CD view
  (workflow list, live step timeline, run history, log search/export), and
  the terminal dashboard gained tabs with a CI/CD tab of its own.
- **OpenTelemetry tracing.** One trace per run, one span per step, always
  recorded into the engine's SQLite (`ci traces`, `ci spans`, the daemon's
  `ciTraces`/`ciTraceSpans`, the UIs' live Trace waterfall) and exported live
  over OTLP/HTTP when `--otlp` / `OTEL_EXPORTER_OTLP_ENDPOINT` is set.
- **`bsdkrun ci run` renders a live TUI on a terminal** (Bubble Tea): one row
  per step with realtime durations in seconds-with-milliseconds, a spinner on
  the running step and a dimmed tail of its output, failures keeping their
  last lines on screen. `--plain` (or piping) keeps line output; the TUI
  consumes the same spindle LogLine stream `--json` prints.
- **`bsdkrun ci prune [--images]`** reclaims everything CI runs leave behind:
  leftover `bsdkrun-ci-*` VMs, the shared ci-checkouts clones and — with
  `--images` — the cached nixery image rootfs, after a summary and a y/N
  confirmation (`-y`/`-f` skips it).
- **`bsdkrun ci [path]`**, and monorepo awareness: a directory positional is
  the workspace, and a directory carrying its own `.tangled/workflows` inside
  a larger repository runs *its* workflows, with user steps starting from
  that subdirectory of the clone.
- **`bsdkrun ci` runs plain OCI images.** A workflow `image:` that reads as a
  reference (`ubuntu:24.04`, `ghcr.io/org/img`) boots that image directly — no
  nixery, no nix machinery; the runner only ensures git is present for the
  clone. Two runnable examples cover both strategies (`examples/ci-bun-ubuntu`,
  `examples/ci-bun-nixery`), and an e2e workflow runs them on CI.
- **CI runs can be stopped from every surface.** The TUI already had `x`; the
  desktop and web CI screens gain a Stop button (new `launch_cancel` command /
  `ciCancel` GraphQL mutation).
- **TUI repo picker.** `o` on the CI/CD tab opens a modal to browse for a git
  checkout or paste a URL to clone — into the same `ci-checkouts` layout the
  daemon uses, so all frontends share one checkout per repository.
- CI screens: ANSI colors render in logs (instead of leaking escape codes),
  search matches highlight in the text, the recent-runs list scrolls and pages,
  recents are searchable, and trace spans glow in fixed neon on both themes.
- Long image references everywhere render one-line, ellipsized, with the full
  reference in a tooltip.

### Fixed

- **A CI boot now waits for the guest agent** before the first step; a cached
  image booted fast enough to hit "the guest agent accepted the connection but
  sent no output".
- **The nixos/nix fallback image works again**: it ships `/etc/nix/nix.conf` as
  a symlink into the read-only store, which virtio-fs refuses to append to even
  for guest root. The nix-config step replaces it with a writable copy first.
- **Registry outages no longer kill runs whose image is already cached.** The
  resolver remembers which digest each reference last resolved to and boots the
  last successfully pulled copy when the registry answers 5xx; manifest fetches
  also retry 5xx before giving up.
- **Steps stream their output in real time** in JSON mode — previously a
  step's log arrived only when it exited, so a long `nix build` looked hung
  in the desktop/web CI screens and then dumped everything at once.
- The CI clone step marks the source mount `safe.directory` (modern git refuses
  a repository owned by another uid, which a virtio-fs mount always is).
- `bsdkrun ci --help` prints the usage text instead of `run`'s flag dump.

### SDKs

| SDK        | Version |
| ---------- | ------- |
| typescript | 0.7.0   |
| python     | 0.6.0   |
| ruby       | 0.6.0   |
| elixir     | 0.6.0   |
| clojure    | 0.5.0   |
| rust       | 0.4.0   |
| go         | 0.4.0   |
| gleam      | 1.6.0   |
| scala      | 0.3.0   |

All nine gained the CI workflow builder; the Go SDK also gained streaming VM
creation (`CreateStreaming`). The Scala, Clojure and Elixir README install
snippets now name current versions.

## [0.10.0] — 2026-08-18

The AI-sandbox release: coding agents in disposable microVMs, a Docker engine
you can point the real `docker` CLI at, and copy-on-write machine snapshots.

### Added

- **AI agent sandboxes.** `bsdkrun claude` (or `codex`, `gemini`, `opencode`,
  `crush`, `copilot`, `kilo`, `qwen`, `kiro`) boots a microVM from that agent's
  flavor, shares the directory you ran it in, and drops you into its TUI. The
  agent can do anything it likes in there and cannot reach the rest of the
  machine. A per-agent home volume keeps its login, so you authenticate once
  per agent rather than once per session.
- **A right-docked agent panel** in the desktop app and the web UI, with
  projects, a session switcher, `--new` sessions, fullscreen, and a start/stop
  control. Sessions survive hiding the panel.
- **Shared skills.** One host directory (`~/.agents/skills`) is mounted into
  every sandbox, so a skill installed once — on the host or by an agent inside
  a sandbox — is visible to all of them.
- **Shared Docker and Nix stores** (`bsdkrun ai disk`). Both are disks rather
  than per-sandbox directories, so an image pulled or a derivation realised in
  one session is still there in the next. `ai disk grow docker --size 200G`
  raises a ceiling; `--watch` monitors free space while an agent works.
- **Uploads for remote engines** (`bsdkrun ai upload`). Driving a VPS leaves
  your skills, keys, git identity and project on the laptop; this sends them
  across. `.gitignore` and `.dockerignore` decide what a project upload
  contains.
- **Docker compatibility.** `bsdkrun docker` runs a `docker:dind` microVM and
  serves its API on a host unix socket, so the host's own `docker` CLI drives
  it — with automatic port publishing and `$HOME` shared, and an optional
  `/var/run/docker.sock`. A Containers view ships in both UIs.
- **Snapshots, branches and restores.** `bsdkrun snapshot`, `snapshots`,
  `branch`, `restore` and `rollback` capture and fork a machine's disk state
  as copy-on-write clones (APFS `clonefile`, Linux `--reflink`), for Linux,
  FreeBSD, NetBSD and unikraft guests. Exposed in both UIs and every SDK.
- **`bsdkrun ai resume <machine>`** brings one stopped sandbox back and waits
  for its guest agent, so a terminal opened straight after actually works.
- **New flavors:** `kiro-cli`, `dragonfly`, `uv`, `mise`, `frankenphp`,
  `mariadb`, `openclaw`, `nanoclaw`, `gemini`, `kilo`, `qwen`.
- **Prebuilt flavor images** on `ghcr.io/tsirysndr`, pulled on first launch
  instead of provisioning a VM, with the Dockerfiles generated from the flavor
  catalog and a CI check that they have not drifted.
- **`bsdkrun image rm`** removes dangling images.
- A Scala 3 SDK, published to Maven Central.

### Changed

- Agent sandbox images are built `FROM node:24`, and install the agent CLI
  **last** so `gh`, Docker and Nix stay cached when an agent releases.
- Every agent sandbox now ships `gh`, Docker, Nix and git preinstalled, and
  inherits the host's git identity and (read-only) SSH keys.
- The Docker VM has a fixed name, so `bsdkrun docker` reuses one engine rather
  than accumulating them.
- The `gleam` flavor uses the project's official image instead of Debian + Nix.
- Progress modals stream the OCI pull, the flavor build and a `git clone`, and
  dismiss themselves on success.

### Fixed

- Nix could not install inside a container build: it needs
  `--extra-conf "sandbox = false"`, which the installer's own Docker
  instructions specify. The step also swallowed its own failure, so images
  shipped with no `nix` in them and said nothing.
- `bsdkrun start` dropped a machine's `-e` environment and `--mount` shares on
  resume, so a restarted Docker VM came back on TLS 2376 and shares came back
  empty.
- `bsdkrun commit` resolved a Linux rootfs by hand and failed on macOS's
  case-sensitive store.
- Terminal log views were unreadable: HeroUI's foreground scale inverts in dark
  mode, so the "light" token was near-black.

### SDKs

| SDK        | Version |
| ---------- | ------- |
| typescript | 0.6.0   |
| python     | 0.5.0   |
| ruby       | 0.5.0   |
| elixir     | 0.5.0   |
| clojure    | 0.4.0   |
| rust       | 0.3.0   |
| go         | 0.3.0   |
| gleam      | 1.5.0   |
| scala      | 0.2.0   |

All nine gained the snapshot, Docker and AI-agent surfaces.

## [0.9.0] — 2026-08-15

### Added

- `bsdkrun cp` copies files and directories between host and guest.
- `bsdkrun cache` saves and restores a guest directory under a key — on host
  disk or in S3, in gzip, zstd, estargz or none.
- `bsdkrun doctor` reports whether this host can run machines at all.
- `--attach-disk` attaches virtio-blk disks to Linux guests.
- Environment variables can be passed at machine creation, from every SDK.

### Fixed

- A partially extracted OCI rootfs is never cached.

### SDKs

typescript 0.5.0 · python 0.4.0 · ruby 0.4.0 · elixir 0.4.0 · clojure 0.3.0 ·
rust 0.2.0 · go 0.2.0 · gleam 1.4.0. Each gained `fs` and `cache` namespaces
and create-time `env`.

## [0.8.0] — 2026-08-12

### Added

- **MirageOS/Solo5 unikernels** (`bsdkrun solo5`), run through an embedded
  `solo5-hvt` tender — a hypervisor front end of its own, not libkrun.
- **Local HTTPS machine domains** (`bsdkrun domains`), with `domains ca` for
  tools that bypass the system trust store.
- A ratatui dashboard (`bsdkrun tui`).

### Fixed

- unikraft x86_64 guests were thirty years fast, from a double-counted epoch.
- `SO_REUSEPORT` and TLS under frankenphp.

Patch releases **0.8.1** (2026-08-12) and **0.8.2** (2026-08-15) followed.

## [0.7.0] — 2026-08-10

### Added

- **`bsdkrun pack`** — railpack for Unikraft: detect a project, plan it, build
  it through BuildKit, and boot the result as a unikernel.
- SDKs for **Ruby, Elixir, Gleam and Clojure**, each with a remote client for
  `bsdkrund`'s GraphQL API.
- A centralized Clojure/Babashka console for the monorepo.

### Fixed

- `docker:dind` boots: cgroup2 is mounted and a leaf cgroup delegated.
- One unsupported virtio device no longer hides all the others.

## [0.6.0] — 2026-08-07

### Added

- **`bsdkrund`**, a token-authenticated gRPC daemon, with a **GraphQL API**
  beside it.
- **A web UI**, served by `bsdkrun ui`.
- The desktop app can drive a remote daemon from the same settings field.
- `bsdkrun` checks `/dev/kvm` on Linux before booting.

### Fixed

- BSD guests get a usable `TERM` on interactive exec, and a real `HOME`.
- `agent update` installs the right OS's agent.

## [0.5.0] — 2026-08-04

### Added

- **Global networks** — a shared L2 subnet with internal DNS, so machines
  resolve each other by name. Editable from the CLI and the desktop app, with a
  members drawer and quick filter.

### Fixed

- `bsdkrun start` resumes a machine's own disk instead of reverting to the base
  image — which had been silently replacing snapshot data.
- Peers resolve by name on NetBSD, via a synced `/etc/hosts`.

Patch releases **0.5.1** and **0.5.2** followed on 2026-08-06 (Linux fd limit).

## [0.4.0] — 2026-08-04

### Added

- **The desktop app**: a Docker Desktop-style GUI for bsdkrun, with flavors,
  streaming launches, repo clone, host terminals, a status bar, a system tray,
  keyboard navigation and infinite scroll.
- In-place restart, friendly machine names, `agent update`, and editing a
  machine's CPU and RAM.

### Changed

- Linux rootfs extraction uses `clonefile(2)` for the whole tree — roughly 10x
  faster to boot a nix image.

## [0.3.0] — 2026-08-03

### Added

- **Key-based SSH setup** for all guests (`ssh setup` / `add-key` / `status`),
  driven by the in-guest agent.
- **Tailscale management** built into the agent, wrapped by
  `bsdkrun tailscale`.
- systemd as PID 1 for Linux guests, configured from the agent.

### Fixed

- FreeBSD and NetBSD compile `PermitRootLogin=no` as the *default*, so the
  agent appends an override rather than assuming.
- FreeBSD amd64 virtio-mmio discovery, and TSC init via CPUID leaf rather than
  the kernel cmdline.

Patch releases **0.3.1** and **0.3.2** followed the same day; 0.3.2 fixed
booting nix-based and empty-`/dev` OCI images.

## [0.2.0] — 2026-08-02

### Added

- **NetBSD amd64** support, via a bundled FFS rootfs and MICROVM kernel, and
  NetBSD direct-kernel boot with no firmware.
- FreeBSD defaults to bsdkrun's bundled arm64 image.
- `bsdkrun logs` surfaces the boot log.
- The nix flake builds on macOS, importing Homebrew's libkrun into the store.

### Fixed

- Linux volumes are backed by a writable virtio-fs root rather than overlayfs.

## [0.1.0] — 2026-08-01

The first release: boot FreeBSD, NetBSD and Linux (OCI) guests on libkrun.

### Added

- `bsdkrun freebsd` / `netbsd` / `linux` / `kernel` / `firmware` boot modes.
- `exec` and `shell` into a running guest, over a TCP agent.
- Machine management for BSD guests, with copy-on-write disk clones.
- `bsdkrun fetch` downloads and caches guest images; `ps` reports
  Docker-style status.
- A serial console, image caching under `$HOME`, and an e2e boot test.
