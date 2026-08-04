---
name: bsdkrun-cli
description: Reference for the bsdkrun CLI — a Firecracker-style microVM launcher for FreeBSD, NetBSD, and Linux (OCI) guests on macOS/Linux, built on libkrun. Use when running, managing, networking, snapshotting, or troubleshooting bsdkrun machines, or when writing commands/scripts against the CLI. Covers every subcommand and its flags.
---

# bsdkrun CLI

`bsdkrun` runs lightweight microVMs (FreeBSD / NetBSD / Linux OCI images) on macOS
(Hypervisor.framework) and Linux (KVM), built on **libkrun**. It's Docker-like:
`run → ps → exec/shell → stop → start → rm`, plus snapshots (`commit`/flavors),
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
- `bsdkrun shell <id>` — attach an interactive console to a detached machine.
- `bsdkrun logs [-f] [--boot] <id>` — show the console log (`--boot` = bsdkrun's own boot log).

Snapshots & flavors (reusable environments):
- `bsdkrun commit <id> <name>` — snapshot a machine into a flavor (like `docker commit`).
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
- `bsdkrun images [--json]` — list downloaded images.
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
- `--no-net` — boot with no NIC (disables the agent → no `exec`/`shell`).
- `--repo <URL>` — clone a git repo into the guest and `cd` into it on shell open.
- Linux only: `--mount HOST:GUEST[:ro]`, `--entrypoint`, `-e K=V`, `--initramfs`, `--kernel(-version)`.
- BSD only: `--version`, `--persist`, `--attach-disk PATH[:ro]`, `--disk-size 8G`, `--verbose`.

## Key behaviors to remember

- **`start` resumes the machine's own storage** (its disk / rootfs), not a fresh base
  image — snapshot and runtime data survive stop/start.
- **`stop` cleanly powers off BSD guests** (`shutdown -p now`) so their UFS is
  consistent; it takes a few seconds. Linux writes go straight to the host FS.
- **NetBSD name resolution** relies on `/etc/hosts` sync (its resolver rejects the
  gvproxy DNS's AAAA NXDOMAIN). Joins auto-sync; use `network sync` to refresh an
  existing network without restarting members.
- **Snapshots of BSD guests** power the guest off first for a clean, bootable image.
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
