---
name: bsdkrun-cli
description: Reference for the bsdkrun CLI — a Firecracker-style microVM launcher for FreeBSD, NetBSD, and Linux (OCI) guests on macOS/Linux, built on libkrun. Also replaces Docker Desktop (`bsdkrun docker` runs a Docker engine in a microVM and serves its API on a host socket, so the normal `docker`/`compose`/`buildx` CLIs drive it) and sandboxes AI coding agents (`bsdkrun claude`, `codex`, `gemini`, … each in its own microVM). Use when running, managing, networking, snapshotting, or troubleshooting bsdkrun machines or its Docker engine, or when writing commands/scripts against the CLI. Covers every subcommand and its flags.
license: MIT
metadata:
  author: tsirysndr
  version: "1.0.0"
  homepage: https://github.com/tsirysndr/bsdkrun
---

# bsdkrun CLI

`bsdkrun` runs lightweight microVMs (FreeBSD / NetBSD / Linux OCI images) on macOS
(Hypervisor.framework) and Linux (KVM), built on **libkrun**. It's Docker-like:
`run → ps → exec/shell → stop → start → rm`, plus machine snapshots
(`snapshot`/`branch`/`restore`), reusable flavors (`commit`),
persistent volumes, port forwards, an in-guest agent (for `exec`/`ssh`/tailscale),
and optional **global networks** with internal DNS.

State lives under `<state>/` (`$XDG_STATE_HOME/bsdkrun` or `~/.local/state/bsdkrun`);
downloads cache under `$BSDKRUN_CACHE` (default `~/.cache/bsdkrun`).

**For the full flag list of every command, read `references/cli-reference.md`.**

## Command map

Run a guest:
- `bsdkrun linux <IMAGE> [-- CMD...]` — boot an OCI image (Docker Hub / any registry).
- `bsdkrun freebsd [-- CMD...]` — FreeBSD (EFI boot on macOS, PVH on Linux/amd64).
- `bsdkrun netbsd  [-- CMD...]` — NetBSD (direct-kernel boot).
- `bsdkrun kernel` / `bsdkrun firmware` — low-level boot from a kernel or UEFI image + disk.

Lifecycle:
- `bsdkrun ps [-a] [--json]` — list machines (running, or all with `-a`).
- `bsdkrun stop <id>` — stop a running machine (BSD guests clean-poweroff first).
- `bsdkrun start <id>` — restart a stopped machine in place (same id/disk, resumes its data).
- `bsdkrun update <id> [--cpus N] [--mem M]` — change recorded vCPU/RAM (applies on next `start`).
- `bsdkrun rm [-f] <id>...` — remove machine(s) and their state (`-f` stops first).

Interact:
- `bsdkrun exec [-t] [-e K=V]... <id> <cmd>...` — run a command in a guest (via its agent).
- `bsdkrun cp [-r] <SRC> <DST>` — copy files host<->guest, `docker cp`-style (`ID:PATH`, `-` = stdio).
- `bsdkrun cache save <id>:<path> --key K [-c gzip|zstd|estargz|none]` — archive a guest dir.
- `bsdkrun cache restore <id>[:<path>] --key K [--restore-keys PREFIX...]` — put it back (a miss exits 0).
- `bsdkrun cache ls` / `bsdkrun cache rm <key>... | --all` — list and remove entries.
- `bsdkrun doctor [--json]` — check the host can run machines; exits 1 on any failure.
- `bsdkrun shell <id>` — attach an interactive console to a detached machine.
- `bsdkrun logs [-f] [--boot] <id>` — show the console log (`--boot` = bsdkrun's own boot log).

AI coding agents (sandboxed, one microVM each):
- `bsdkrun claude` — Claude Code in a sandbox that shares the current directory; also
  `codex`, `gemini`, `opencode`, `crush`, `copilot`, `kilo`, `qwen`.
- `bsdkrun ai agents [--json]` — the agents, and whether each one's image is built.
- `bsdkrun ai ls [--json]` — sandboxes, grouped by project.
- `bsdkrun ai start <agent> [--workspace PATH] [--repo URL] [--name N] [--project P] [--new] [-d]`
- `bsdkrun ai stop|rm <agent>` — `rm` also drops the saved login unless `--keep-home`.
- Per-agent `$HOME` volume (login persists), `~/.agents/skills` shared into **every**
  sandbox, host git identity + read-only `~/.ssh` injected, and git/Docker/Nix
  preinstalled. `--no-workspace` shares nothing; `--no-ssh` withholds the keys.
- `bsdkrun ai resume <machine> [-d]` — bring ONE stopped sandbox back (keeps its
  workspace/name/project) and wait for its guest agent; `ai start` would boot a second one.
- `bsdkrun ai disk [ls --watch [SECS]]` — the shared Docker/Nix store disks (one running
  holder at a time), per-sandbox usage, and host free space. `ai disk grow docker --size 200G`.
- `bsdkrun ai upload [--what skills|ssh|git|workspace] [--agent A] [DIR]` — copy local
  skills / keys / git identity / a project onto a REMOTE engine's sandbox ($HOME and
  system dirs are refused; `.gitignore`/`.dockerignore` respected, `--all` overrides).
- Paths resolve on the *engine's* host — for a remote daemon use `--repo` or `ai upload`.

CI (tangled spindle workflows in microVMs):
- `bsdkrun ci run [names...]` — run `.tangled/workflows/*.yml` locally: one microVM per
  workflow (nixery.dev image from its `dependencies:`), clone of HEAD, steps streamed.
  Flags: `--event push|pull_request|manual`, `-f FILE` (explicit file, skips matching),
  `-w DIR`, `--input k=v`, `--keep` (keep the VM after a failure), `--json` (spindle
  LogLine JSON). Runs the HEAD **commit**, never the dirty tree.
- `bsdkrun ci ls [--event E]` — workflows and whether each matches that trigger.
- `bsdkrun ci serve [--bind H:P]` — accept `sh.tangled.pipeline` records over HTTP and
  run them (a spindle-compatible runner for a server).
- Every SDK can define workflows in code (`workflow("test").on_push("main").step(...)`)
  and `.run()` them — YAML is generated, never hand-written.

Docker (a Docker engine in a microVM, driven by the host's own `docker` CLI):
- `bsdkrun docker start [--cpus N] [--mem M] [--disk-size 60G] [--mount PATH]` — boot (or
  resume) the engine, serve its API on a host unix socket, and point `docker` at it via a
  `bsdkrun` context. One VM, always named `bsdkrun-docker`; a second `start` resumes it.
- `bsdkrun docker status [--json]` / `bsdkrun docker stop` / `bsdkrun docker rm [-f]`.
- `bsdkrun docker ps [-a] [--json]` — containers. `bsdkrun docker logs <c> [--tail N]`.
- `bsdkrun docker container <start|stop|restart|kill|pause|unpause|rm> <c>...`.
- `bsdkrun docker disk [--size 100G]` — show or grow the image store.
- `bsdkrun docker env` — the `DOCKER_HOST` line, for a shell not using the context.
- `bsdkrun docker shell` — a shell in the engine **VM** (not in a container).
- Published container ports are mirrored onto the host automatically; `$HOME` is shared
  into the VM at the same path so `-v $PWD:/app` resolves.

Snapshots (a machine's disk state, copy-on-write):
- `bsdkrun snapshot <id> [name] [-d DESC]` — capture a machine's disk state (BSD guests are powered
  off first, and left stopped; Linux guests keep running).
- `bsdkrun snapshots [machine] [--json]` — list snapshots, newest first.
- `bsdkrun snapshot rm <name>...` — delete snapshots and their data.
- `bsdkrun branch <snapshot|machine> [-d] [--name N] [--port H:G]` — boot a NEW machine from a
  snapshot; naming a machine snapshots it first. Prints the new machine's id.
- `bsdkrun restore <id> <snapshot> [-f]` — put a machine back to a snapshot (`-f` stops it first;
  the replaced state is snapshotted first unless `--no-backup`). Left stopped — `start` to run it.
- `bsdkrun rollback <id> [-f]` — restore to the machine's most recent snapshot.

Flavors (reusable environments):
- `bsdkrun commit <id> <name>` — freeze a machine into a flavor (like `docker commit`).
- `bsdkrun flavors [--json]` — list the catalog + your snapshots.
- `bsdkrun flavor run [-d] <name> [--port H:G] [-v NAME]` — boot a machine from a flavor.
- `bsdkrun flavor add --base <ref> [--nix PKG] [--provision CMD]... <name>` — define a custom flavor.
- `bsdkrun flavor build <name>` — pre-build a flavor's provisioned rootfs into the cache.
- `bsdkrun flavor rm <name>` — remove a snapshot / user flavor.

Global networks (shared subnet + internal DNS — like docker-compose):
- `bsdkrun network create <name>` — create a network (starts its shared gvproxy).
- `bsdkrun network ls [--json]` — list networks + member counts.
- `bsdkrun network rm [-f] <name>...` — delete network(s).
- `bsdkrun network connect <machine> <network>` — join/switch a machine (applies on next `start`).
- `bsdkrun network disconnect <machine>` — detach back to isolated (applies on next `start`).
- `bsdkrun network sync <network>` — refresh members' `/etc/hosts` so peers resolve by name.
- Join at boot with `--network <name> [--name <member>]` on `linux`/`freebsd`/`netbsd`.

Remote access (in-guest agent actions):
- `bsdkrun ssh <id> setup|add-key|status [--user U] [--key K]...` — key-based SSH.
- `bsdkrun tailscale <id> setup|status|install|start [--authkey K] [--hostname H]` — tailscale.
- `bsdkrun systemd <id> setup|status|disable` — systemd as PID 1 (Linux; boot on `-v` to persist).
- `bsdkrun agent update <id>` — refresh a stale baked-in guest agent.

Disks, images, volumes:
- `bsdkrun prune [--all] [--volumes] [--only KIND]... [-f] [--dry-run]` — reclaim disk:
  stopped machines, unused images, orphaned rootfs trees. Asks with a summary first;
  `--only orphan` is the selection that cannot cost anything. `--all` adds the OCI layer
  cache; `--volumes` adds unused volumes.
- `bsdkrun images [--json]` — list downloaded images.
- `bsdkrun image rm <id>... [-f]` — remove dangling images (refused while a machine uses one).
- `bsdkrun volume ls | rm <name>...` — manage persistent volumes.
- `bsdkrun fetch [--os freebsd|netbsd] [--version V]` — download + prepare a BSD image.
- `bsdkrun versions` — list downloadable BSD builds.
- `bsdkrun grow --disk <path> --size <8G>` — enlarge a disk image (guest expands on next boot).
- `bsdkrun probe` — check that libkrun links and a VM context can be created.

## Common flags on run commands (`linux`/`freebsd`/`netbsd`)

- `-d, --detach` — run in background, print the id (like `docker run -d`).
- `--name <NAME>` — machine name (also its DNS name on a `--network`).
- `--network <NAME>` — join a global network.
- `--cpus <N>` / `--mem <MiB>` — resources (defaults 1 / 512).
- `--port <HOST:GUEST>` — forward a host TCP port (repeatable), e.g. `--port 2222:22`.
- `-v, --volume <NAME>` — persist the rootfs/disk to a named volume.
- `--attach-disk PATH[:ro]` — attach an extra raw disk image as virtio-blk (repeatable).
- `--no-net` — boot with no NIC (disables the agent → no `exec`/`shell`).
- `--repo <URL>` — clone a git repo into the guest and `cd` into it on shell open.
- Linux only: `--mount HOST:GUEST[:ro]`, `--entrypoint`, `-e K=V`, `--initramfs`, `--kernel(-version)`.
- BSD only: `--version`, `--persist`, `--disk-size 8G`, `--verbose`.

## Key behaviors to remember

- **`start` resumes the machine's own storage** (its disk / rootfs), not a fresh base
  image — snapshot and runtime data survive stop/start.
- **`stop` cleanly powers off BSD guests** (`shutdown -p now`) so their UFS is
  consistent; it takes a few seconds. Linux writes go straight to the host FS.
- **NetBSD name resolution** relies on `/etc/hosts` sync (its resolver rejects the
  gvproxy DNS's AAAA NXDOMAIN). Joins auto-sync; use `network sync` to refresh an
  existing network without restarting members.
- **Snapshots of BSD guests** power the guest off first for a clean, bootable image.
- **Linux storage on macOS is case-sensitive.** The first Linux launch automatically creates a
  sparse APFS store for OCI roots and volumes. Inspect or manage it with `bsdkrun store status`,
  `init`, `attach`, `detach`, and `rm`.
- The desktop app mirrors these: Machines, Images, Volumes, Flavors, and Networks
  views, plus per-machine edit (CPU/RAM, network) and network member browsing.

## Examples

```sh
# Ephemeral Alpine, run one command
bsdkrun linux alpine -- echo hi

# Detached Ubuntu with SSH forwarded, then exec into it
id=$(bsdkrun linux -d --name web --port 2222:22 ubuntu:24.04 -- sleep infinity)
bsdkrun exec -t "$id" bash
bsdkrun ssh "$id" setup            # installs your ~/.ssh/id_*.pub

# FreeBSD, persistent, on a named volume
bsdkrun freebsd -d --name bsd -v bsddata --disk-size 8G

# Two machines on one network, reachable by name
bsdkrun network create devnet
bsdkrun linux -d --network devnet --name db  postgres
bsdkrun linux -d --network devnet --name api myapi   # api can `ping db`

# Snapshot a configured machine and reuse it
bsdkrun commit web my-web-env
bsdkrun flavor run -d my-web-env
```
