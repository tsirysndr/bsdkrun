# bsdkrun CLI — full command reference

Every command, its arguments, and flags. Global option on all commands:
`--log-level <0-5>` (libkrun log verbosity, default 1). `bsdkrun -V/--version`.

---

## Running guests

### `linux <IMAGE> [-- <CMD>...]`
Run an OCI image (Docker Hub / any registry) as a Linux machine.

- `<IMAGE>` — OCI ref, e.g. `alpine`, `alpine:3.20`, `ghcr.io/owner/name:tag`.
- `[CMD]...` — command to run instead of the image's default (everything after `--`).
- `--kernel <PATH>` — boot a specific ELF vmlinux / raw arm64 Image (overrides `--kernel-version`).
- `--kernel-version <VER>` — vmlinux-builder release to download+boot (default `7.2`).
- `-d, --detach` — background + print id.
- `--initramfs` — load whole rootfs into RAM instead of virtio-fs (for kernels lacking CONFIG_VIRTIO_FS).
- `-v, --volume <NAME>` — persist rootfs to a named volume (CoW-clones the image first; reuse to keep changes).
- `--mount <HOST:GUEST[:ro]>` — bind-mount a host dir over virtio-fs (repeatable; Linux only).
- `--attach-disk <PATH[:ro]>` — extra virtio-blk disk (repeatable; `:ro` for read-only).
- `--entrypoint <EP>` — override the image entrypoint.
- `-e, --env <K=V>` — set a guest env var (repeatable).
- `--console <DEV>` — guest console device (default `hvc0`; use `ttyS0` only with a matching setup).
- `--no-net` — no NIC.
- `--port <HOST:GUEST>` — forward a host TCP port (repeatable).
- `--mac <AA:BB:CC:DD:EE:FF>` — NIC MAC (default: a fixed locally-administered one).
- `--network <NAME>` — join a global network (create it first).
- `--name <NAME>` — machine name (its DNS name on a `--network`; default a generated name).
- `--cpus <N>` (default 1), `--mem <MiB>` (default 512).

### `freebsd [-- <CMD>...]` and `netbsd [-- <CMD>...]`
Run FreeBSD (EFI boot on macOS; PVH direct boot on Linux/amd64) / NetBSD (direct-kernel boot).
Without `-d`, a `CMD` is one-shot: boot → run (streaming output) → power off → exit with its status.

- `[CMD]...` — command to run via the guest agent once booted. Needs networking (incompatible with `--no-net`).
- `--version <VER>` — FreeBSD: a release like `15.1` (default latest); NetBSD: `10.1` or `current` (default current).
- `--firmware <PATH>` — UEFI firmware (default: krunkit's KRUN_EFI, auto-located).
- `-f, --force` — re-download even if cached.
- `--attach-disk <PATH[:ro]>` — extra virtio-blk disk (repeatable; `:ro` for read-only).
- `-d, --detach` — background + print id.
- `--persist` — boot the disk in place (writes persist to it; one machine at a time) instead of a per-machine CoW clone.
- `-v, --volume <NAME>` — persist disk to a named volume (under `<state>/volumes/<NAME>`).
- `--no-net`, `--port <HOST:GUEST>`, `--mac`, `--network <NAME>`, `--name <NAME>` — as for `linux`.
- `--cpus <N>` (1), `--mem <MiB>` (512).
- `--disk-size <SIZE>` — grow root disk before boot (only enlarges), e.g. `8G`, `4096M`.
- `--verbose` — stream the guest boot console live while waiting for the agent.
- `--repo <URL>` — clone a git repo into the guest after boot and `cd` into it on shell open (installs git if needed).

### `kernel` / `firmware`
Low-level boots. `kernel` boots from a direct kernel image + optional root disk;
`firmware` boots from a UEFI firmware image + root disk. (Used internally by the BSD shortcuts.)

---

## Machine lifecycle

### `ps [-a] [--json]`
List machines. `-a/--all` shows stopped ones too; `--json` for scripting/SDK.
JSON fields include `id, name, image, kind, command, status, running, exit_code, pid,
cpus, mem, volume, state_dir, created_at, finished_at, network, net_ip`.

### `stop <id>`
Stop a running machine. **BSD guests are cleanly powered off** (`shutdown -p now`,
waits for exit) so their UFS is consistent; Linux is SIGTERM'd (writes already on host FS).

### `start <id>`
Restart a stopped machine **in place** — same id, image/resources/volume, and **its own
disk/rootfs** (snapshot + runtime data resume). Re-boots detached, like `docker start`.
Re-joins its recorded `--network` and keeps its IP where possible.

### `update <id> [--cpus N] [--mem M]`
Change the recorded vCPU/RAM. libkrun fixes resources at boot, so it **applies on the next `start`**.

### `rm [-f] <id>...`
Remove machine(s) and their state dir. Refuses a running machine unless `-f` (stops it first).

---

## Interacting with a running machine

### `exec [-t] [-e K=V]... <id> <cmd>...`
Run a command inside the guest via its agent. `-t/--tty` for interactive (like `docker exec -it`);
`-e K=V` sets env (repeatable).

### `cp [-r] <SRC> <DST>`
Copy files between the host and a running machine, like `docker cp`. Exactly one side carries an
`ID:` prefix; `-` is the host's stdin (as SRC) or stdout (as DST). `-r/--recursive` copies a
directory's *contents* into the destination, so `-r ./src ID:/app` leaves the guest's `/app`
holding what `./src` holds.

```sh
bsdkrun cp ./main.py web:/app/main.py     # in (parent dirs are created)
bsdkrun cp web:/var/log/app.log ./        # out (a directory destination keeps the basename)
bsdkrun cp -r ./src web:/app              # a whole tree
cat a.txt | bsdkrun cp - web:/tmp/a       # stream in
bsdkrun cp web:/tmp/a - | wc -c           # stream out
```

The transfer rides the guest's exec agent (`cat`, plus `tar` for `-r`), so the machine must be
running and its image needs a shell — which every image that boots under bsdkrun already has. An
image without `tar` can still copy files one at a time; `-r` reports that specifically.

### `cache save <ID:PATH> --key <KEY> [-c FORMAT] [--force]`
Archive a guest directory and store it under a key. `--compression` is `gzip` (default), `zstd`,
`estargz` or `none`. Saving over an existing key needs `--force`.

### `cache restore <ID[:PATH]> --key <KEY> [--restore-keys PREFIX...]`
Restore a stored tree. Without a path it goes back where it was saved from. `--restore-keys` are
prefixes tried in order when the exact key misses; within a prefix the newest entry wins. **A miss
is not an error** — it prints `cache miss` and exits 0, so a first run needs no `|| true`.

### `cache ls [--json]` / `cache rm <KEY>... | --all`
List and remove entries.

```sh
bsdkrun cache save web:/root/.cargo --key cargo-$(shasum Cargo.lock | cut -c1-12)
bsdkrun cache restore web --key cargo-abc123 --restore-keys cargo-
bsdkrun cache ls
bsdkrun cache rm --all
```

Entries go to the host disk (`<cache>/caches`) by default. For a shared store, set
`BSDKRUN_CACHE_BACKEND=s3` and `BSDKRUN_CACHE_S3_BUCKET`, or write `~/.config/bsdkrun/cache.toml`:

```toml
backend = "s3"

[s3]
bucket   = "my-ci-cache"
region   = "us-east-1"
prefix   = "bsdkrun"                                  # optional
endpoint = "https://<id>.r2.cloudflarestorage.com"    # optional: R2, MinIO, …
```

Credentials come from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` (+ `AWS_SESSION_TOKEN`) only,
never from the file, so the config stays safe to commit. Saving needs `tar` in the guest;
compression happens on the host, so the image needs no compressor of its own.

### `shell <id>`
Attach an interactive console to a running (detached) machine.

### `logs [-f] [--boot] <id>`
Show the guest console log. `-f/--follow` streams live; `--boot` shows bsdkrun's own boot log
(libkrun diagnostics + boot errors) — useful when a machine dies before producing console output.

---

## AI coding agents (`bsdkrun ai`, `bsdkrun claude`, …)

A coding agent in a disposable microVM. It sees the folder you share and nothing
else of your machine. Eight agents: `claude`, `codex`, `gemini`, `opencode`, `crush`,
`copilot`, `kilo`, `qwen`.

Three kinds of state, deliberately separated: the **login** lives on a per-agent volume
mounted at `$HOME` (so a second session does not re-authenticate); **skills** live in one
host directory (`~/.agents/skills`) mounted into every sandbox with each agent's own
skills path symlinked at it; **your code** is shared only when you ask.

Each sandbox ships git, Docker (its own daemon, started automatically — never the host's
socket) and Determinate Nix. The host's git identity is applied, and `~/.ssh` is mounted
read-only so `git push` works.

### `bsdkrun <agent>` (e.g. `bsdkrun claude`)
Boot or reuse a sandbox, share the current directory, and attach to the agent's TUI.
- `--workspace PATH` — share this directory instead of the current one.
- `--no-workspace` — share nothing.
- `--repo URL` — clone a repository *inside* the sandbox and start there. Needs no
  access to your filesystem, so it is how you hand a **remote** engine a codebase.
- `--name NAME` — name the session (shown in `ai ls` and the desktop switcher).
- `--project P` — group it; defaults to the shared folder's (or repo's) name.
- `--new` — a second sandbox against the same saved login.
- `--no-ssh` — do not share `~/.ssh`.
- `-d` — start it in the background and print its id instead of attaching.
- `--cpus N` / `--mem M`.

### `ai agents [--json]`
Every agent, with `installed` (its image is built, so a sandbox boots in a second).

### `ai ls [--json]`
Sandboxes, grouped by project, with state and shared folder.

### `ai start <agent> [OPTIONS]`
The same as the aliases above, naming the agent explicitly.

### `ai stop <agent>` / `ai rm <agent> [--keep-home]`
`stop` powers its sandboxes off; the saved login survives. `rm` removes the sandboxes
*and* the login unless `--keep-home`.

> A sandbox is a machine: `ps`, `logs`, `stop`, `rm` and `exec` all work on its id, and
> a single session can be deleted with `bsdkrun rm <id>`.

> **Where paths resolve.** `--workspace` names a directory on the machine running the
> engine. Driving a remote `bsdkrund`, that is the remote host.

---

### Newer `ai` subcommands

```
bsdkrun ai resume <machine> [-d]        # bring ONE stopped sandbox back, wait for its agent
bsdkrun ai disk [ls] [--watch [SECS]]   # shared Docker/Nix disks, usage, host free space
bsdkrun ai disk grow docker --size 200G # raise a shared store's ceiling (never shrinks)
bsdkrun ai upload [--what skills|ssh|git|workspace] [--agent A] [DIR] [--name N] [--all]
```

- `resume` exists because `ai start` reasons about an *agent* and would boot a second
  sandbox; `bsdkrun start <id>` returns before the guest agent is up and a terminal
  opened then fails with "accepted the connection but sent no output".
- The shared disks are held by ONE running sandbox at a time (two guests writing one
  ext4 image corrupts it); a second sandbox boots with an empty store and says so.
  `BSDKRUN_AI_NO_SHARED_DISKS=1` disables them.
- `upload` exists for remote engines: skills/keys/git identity/project live on the
  laptop, not the VPS. `$HOME` and system directories are refused outright; project
  uploads honour `.gitignore`/`.dockerignore` and cap at 256 MiB / 20k files.

## CI (`bsdkrun ci`) — tangled spindle workflows in microVMs

Runs `.tangled/workflows/*.yml` — the same files tangled's spindle runs — with one
microVM per workflow. The workflow schema and `when:` matching are tangled's own
code (the tool is Go and imports `tangled.org/core/workflow`), so a file spindle
accepts is a file this accepts.

```
bsdkrun ci run [names...]        # run matching workflows (manual trigger by default)
bsdkrun ci run test lint         # naming workflows selects them (skips `when` matching)
bsdkrun ci run -f wf.yml         # an explicit file, from anywhere
bsdkrun ci run --event push      # simulate a push (branch = current, sha = HEAD)
bsdkrun ci run --event pull_request --branch main
bsdkrun ci run --input k=v       # manual-trigger inputs (TANGLED_INPUT_K)
bsdkrun ci run --keep            # keep the VM after a failure (bsdkrun shell <id>)
bsdkrun ci run --json            # spindle LogLine JSON on stdout
bsdkrun ci ls [--event E]        # workflows and whether each matches
bsdkrun ci serve [--bind H:P]    # accept sh.tangled.pipeline records over HTTP
```

Behaviors:
- Runs the repository's **HEAD commit**, never the dirty working tree — commit first.
- The image is built from the workflow's `dependencies:` via nixery.dev
  (`nixery.dev/[arm64/]<deps>/bash/git/coreutils/util-linux/nix`); both `engine:
  nixery` (registry map) and `engine: microvm` (plain list) forms are accepted.
  `BSDKRUN_CI_NIXERY` points at a self-hosted nixery.
- Steps run serially in one VM, from `/tangled/workspace`, with `CI=true` and the
  full `TANGLED_*` env set. System steps (nix config, clone) come first.
- Custom-registry deps (`github:owner/repo/rev`) are installed with `nix profile add`.
- `ci serve` is the runner seam only: POST a pipeline record to `/pipelines`, poll
  `/pipelines/{id}`, read `/pipelines/{id}/logs` (spindle LogLine JSON). The clone
  fetches from the knot URL in the record's trigger metadata.
- Every SDK has a CI builder (`workflow("test").on_push("main").step(...).run()`)
  that generates this YAML and runs it — code-defined workflows, no YAML by hand.

## Prune (`bsdkrun prune`) — reclaim disk

```
bsdkrun prune                    # summary + [y/N] confirmation, then remove
bsdkrun prune --dry-run          # report only
bsdkrun prune -f                 # skip the prompt (required when stdin is not a tty)
bsdkrun prune --only orphan      # only unreferenced rootfs trees (cannot cost anything)
bsdkrun prune --only image       # only unused images
bsdkrun prune --volumes          # also volumes no machine references
bsdkrun prune --all              # also saved cache entries + the OCI layer cache
```

Defaults keep the OCI layer cache (it is what makes re-pulls cheap and it is already
size-bounded) and volumes (the one thing holding non-redownloadable data).

## Docker (`bsdkrun docker`)

A Docker engine in a microVM, with its API served on a host unix socket so the *host's*
`docker` CLI drives it — a Docker Desktop replacement, not a wrapper CLI. There is
exactly one engine VM, always named `bsdkrun-docker`.

Three things it handles that a naive VM-backed Docker does not: the socket (plus a
`bsdkrun` docker context, so no `DOCKER_HOST` is needed), automatic publishing of
container ports onto the host, and sharing `$HOME` into the VM **at the same path** so
`-v $PWD:/app` resolves instead of mounting an empty directory.

### `docker start [OPTIONS]`
Boot or resume the engine. Idempotent — a second `start` resumes the same VM.
- `--cpus N` / `--mem M` — VM sizing.
- `--mount PATH|HOST:GUEST` — share another host directory (repeatable). A bare `PATH`
  mounts at the same path in the guest.
- `--no-home` — do not share `$HOME` (shared by default).
- `--disk-size SIZE` — give the image store a dedicated sparse ext4 disk (e.g. `60G`)
  instead of the host-backed rootfs. Only applies when the VM is created.
- `--publish-bind IP|mirror` — where published container ports bind on the host.
  `mirror` (default) reproduces what the container asked for, as Docker Desktop does.
- `--system-socket` — also point `/var/run/docker.sock` here (asks for sudo once), for
  tools that hardcode it.
- `--no-context` / `--no-activate` — skip creating / selecting the `bsdkrun` context.
- `--timeout SECS` (default 120), `--json`.

### `docker status [--json]`
Engine version, socket, API port, shared directories, image store, context state.

### `docker stop` / `docker rm [-f]`
`stop` powers the VM off; images and containers stay on its disk. `rm` removes the VM,
its image store **and** the docker context.

### `docker ps [-a] [--json]`
Containers, as the engine reports them. `-a` includes stopped ones.

### `docker container <ACTION> <CONTAINER>...`
`start` | `stop` | `restart` | `kill` | `pause` | `unpause` | `rm`.

### `docker logs <CONTAINER> [--tail N]`
One container's logs (stdout+stderr, default 200 lines).

### `docker disk [--size SIZE] [--json]`
Show the image store, or grow it. Growing only enlarges. A *running* engine keeps seeing
the old size until it restarts — virtio-blk pins a device's size at attach time.

### `docker env`
Prints `export DOCKER_HOST=unix://…` for a shell that is not using the docker context
(`eval "$(bsdkrun docker env)"`).

### `docker shell`
A shell in the engine **VM** — not in a container. (`docker exec` is for containers.)

> The forwarded API port is loopback-only and carries no TLS; Docker API access is
> root-equivalent inside the guest, the same trade colima makes. The socket is `0600`.

---

## Machine snapshots (`snapshot` / `branch` / `restore` / `rollback`)

A snapshot is a **copy-on-write clone of a machine's disk state** — instant to take, free until the
two sides diverge. It is *not* a memory image: libkrun has no save-VM API, so a restored machine
boots, it does not resume. What is captured depends on the guest: a Linux rootfs tree, a BSD raw
root disk, or a unikernel image plus its `--mount`ed host directories.

### `snapshot <ID> [NAME] [-d DESC] [--json]`
Capture a machine's disk state. `NAME` defaults to `<machine>-<n>`. **A BSD guest is powered off
first** — a mounted UFS cannot be cloned consistently — so it is left stopped; a Linux guest is only
flushed and keeps running.

### `snapshots [MACHINE] [--json]` / `snapshot ls [MACHINE] [--json]`
List snapshots, newest first; `MACHINE` narrows to one machine's.

### `snapshot rm <SNAPSHOT>...`
Delete snapshots and their data. Machines already branched from them are unaffected.

### `branch <SNAPSHOT|MACHINE> [--name NAME] [-d] [--cpus N] [--mem M] [--port H:G] [--no-ports]`
Boot a **new** machine from a snapshot, printing its id. Naming a *machine* instead snapshots it
first, then branches that — "give me a copy of this machine" in one command. The state is cloned,
never booted in place, so the source is untouched and one snapshot can be branched repeatedly.
Without `--port`, the snapshot's own forwards are inherited, with any host port that is already
taken swapped for a free one.

### `restore <ID> <SNAPSHOT> [-f] [--no-backup]`
Put a machine's disk state back to one of its snapshots. The machine must be stopped; `-f` stops it
first. The state being replaced is snapshotted first (a free CoW clone) unless `--no-backup`, so a
mistaken restore is reversible. The machine is left **stopped** — `start` it to run the restored
state.

### `rollback <ID> [-f] [--no-backup]`
`restore` to the machine's most recent snapshot, without having to name it.

---

## Flavors (reusable environments)

### `commit [-d DESC] <id> <name>`
Snapshot a machine's current state into a named flavor (like `docker commit`). BSD guests are
powered off first for a clean, bootable image; the machine is left stopped (`start` to resume).

### `flavors [--json]`
List flavors: the built-in catalog + your saved snapshots.

### `flavor run [-d] <name> [--cpus N] [--mem M] [--port H:G] [-v NAME] [--repo URL]`
Boot a new machine from a flavor (catalog entry or snapshot). Extra `--port`/`-v` layer on the
flavor's defaults.

### `flavor add --base <BASE> [OPTIONS] <name>`
Define (or update) a custom flavor in `flavors.toml`.
- `--base <BASE>` — an OCI ref (`node:22`) or `freebsd` / `netbsd` (required).
- `--category <CAT>` (default `custom`), `--description <DESC>`.
- `--port <HOST:GUEST>` — default forward (repeatable).
- `--env <K=V>` — default env (repeatable).
- `--nix <PKG>` — nix package to install on an OCI base (repeatable).
- `--provision <CMD>` — provisioning command run in the guest after boot (repeatable, in order).

### `flavor build <name>`
Pre-build a flavor's provisioned rootfs into the cache (so a later `run` is instant). Streams
provisioning output; a no-op if already cached.

### `flavor rm <name>`
Remove a saved snapshot or user flavor (catalog flavors can't be removed).

> `commit` freezes a machine into a reusable *flavor* (a template to boot fresh machines from);
> `snapshot` keeps a point *this* machine can go back to, or be forked from. Different verbs, and
> `flavor rm` does not touch machine snapshots — that is `snapshot rm`.

---

## Global networks (shared subnet `192.168.127.0/24` + internal DNS)

### `network create <name>`
Create a network — starts its shared gvproxy switch. Members join with `--network <name>`.

### `network ls [--json]`
List networks with subnet/gateway/status and running/total member counts.

### `network rm [-f] <name>...`
Stop a network's gvproxy and delete it. Refuses a network with running members unless `-f`.

### `network connect <machine> <network>`
Join/switch a machine to a network. Records membership + clears its old IP; **applies on next
`start`** (a VM's NIC is fixed at boot).

### `network disconnect <machine>`
Detach a machine back to the default isolated stack; applies on next `start`.

### `network sync <network>`
Refresh every running member's `/etc/hosts` with the current membership, so peers resolve by
name (fixes NetBSD, whose resolver rejects the gvproxy DNS's AAAA NXDOMAIN). Joins auto-sync;
this is for refreshing an existing network without restarting members.

---

## Remote access (in-guest agent actions)

### `ssh <id> <action>...`
Key-based SSH setup. Actions: `setup [--user U] [--key K]...`, `add-key --key K...`, `status`.
`--key` accepts a literal public key or a local `.pub` file path; with no `--key`, installs your
local `~/.ssh/id_*.pub`.

### `tailscale <id> <action>...`
Tailscale in the guest. Actions: `setup [--authkey K] [--hostname H]`, `status`, `install`,
`start [--kernel-tun]`. Extra `setup` args pass through to `tailscale up`.

### `systemd <id> <action>...`
systemd as PID 1 in a Linux guest. Actions: `setup` (install + mark for next boot), `status`,
`disable`. Boot on a volume (`-v`) so the change persists.

### `agent update <id>`
Download + install the current agent inside a running guest, over its existing (possibly
outdated) one. The next `exec`/`ssh`/`tailscale` spawns the fresh binary.

---

## Disks, images, volumes, diagnostics

### `store init|status|attach|detach|rm` (macOS only)
Manage the case-sensitive APFS sparsebundle used for Linux OCI rootfs trees and named volumes.
The first Linux launch initializes the default 200 GiB sparse-capacity store automatically when
the macOS cache filesystem is case-insensitive. `init --size <SIZE>` creates it explicitly with a
custom ceiling; `status` reports its state and disk use; `attach`/`detach [-f]` manage the mount;
`rm -f` permanently deletes the store, cached images, and volumes. Stop running machines before
initialization or detachment.

### `image rm [-f] <IMAGE>...`
Remove a **dangling** image (one no machine references) and its extracted rootfs.
Refused when a machine still uses it — that machine's rootfs is what would go — unless
`-f`, after which it will not boot. `images --json` reports `used_by`.

### `images [--json]`
List downloaded images.

### `volume ls` / `volume rm <name>...`
List / remove persistent volumes (and their data).

### `fetch [--os freebsd|netbsd] [--version V] [--dir DIR] [-f]`
Download a BSD arm64 image and prepare it for booting. `--dir` links the cache-backed image
into a directory (default `images`).

### `versions`
List the arm64 builds available to `fetch`.

### `grow --disk <PATH> --size <SIZE>`
Grow a raw disk image (only enlarges, e.g. `8G`/`4096M`); the guest expands its root FS on next boot.

### `probe`
Check that libkrun links and a context/HVF can be initialized (connectivity/health check).

---

### `doctor [--json]`
Check that this host can run machines and print what to fix. Covers the host tools bsdkrun shells
out to (`curl`, `tar`), the hypervisor, the macOS code signature and its hypervisor entitlement,
gvproxy, the state/cache directories, the case-sensitive store, and the cache backend. Exits 1 if
anything failed, so CI can gate on it.

---

## Environment variables

- `BSDKRUN_CACHE` — override the cache dir (images/kernels/agent/flavor builds).
- `BSDKRUN_AGENT_VERSION` — release tag to pull the guest agent from (default: this build's version).
- `BSDKRUN_AGENT_LINUX` / `_FREEBSD` / `_NETBSD` — point at a local prebuilt agent binary (dev).
- `BSDKRUN_FREEBSD_CMDLINE` / `BSDKRUN_NETBSD_CMDLINE` — override the BSD kernel command line.
- `KRUN_PVH=1` — PVH direct boot (set automatically for the BSD PVH paths on amd64).
