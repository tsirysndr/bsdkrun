# bsdkrun Desktop

A **Docker Desktop-style GUI** for [bsdkrun](https://github.com/tsirysndr/bsdkrun) —
run FreeBSD, NetBSD, and Linux (OCI) microVMs on macOS and Linux, with a live
console, an interactive terminal, and one-click machine management.

It's a thin, native front end: the GUI drives the `bsdkrun` CLI (using its
`--json` output) — there's no daemon and no duplicated logic. bsdkrun still does
all the real work (libkrun / Hypervisor.framework / KVM).

<p align="center">
  <img src="../.github/assets/desktop.png" alt="bsdkrun Desktop — machines list with a tabbed terminal panel" width="900">
</p>

## Features

- **Machines** — list running/stopped microVMs, launch, stop, and inspect them.
- **Run dialog** — Linux (any OCI image), FreeBSD, or NetBSD with vCPUs, memory,
  named volumes, port forwards, bind mounts, and command overrides.
- **Interactive terminal** — a real PTY into the guest (`bsdkrun exec -t`),
  rendered with **xterm.js** in a beautiful **JetBrains Mono**.
- **Live logs** — the guest console streamed live (`logs -f`), with a toggle to
  bsdkrun's own boot diagnostics.
- **Images & Volumes** — browse the image cache and manage persistent volumes.
- **Raycast-style command palette** (`/` or `⌘K`) and a full keyboard-shortcut
  map (`?`).
- **Native application menu** (macOS top bar / window menu) for the important
  actions, plus a native, draggable overlay title bar.

## Tech stack

| Layer        | Choice                                                        |
| ------------ | ------------------------------------------------------------- |
| Shell        | [Tauri v2](https://tauri.app) (Rust) — async commands, tokio  |
| PTY / IPC    | `portable-pty` + Tauri events                                 |
| UI           | React + TypeScript + Vite                                     |
| Components   | [HeroUI](https://heroui.com) + Tailwind CSS                   |
| Icons        | [Tabler Icons](https://tabler.io/icons)                       |
| Data caching | [TanStack Query](https://tanstack.com/query)                  |
| UI state     | [Jotai](https://jotai.org)                                    |
| Forms        | React Hook Form + Zod                                         |
| Terminal     | xterm.js + JetBrains Mono                                     |

## Prerequisites

- **[bsdkrun](https://github.com/tsirysndr/bsdkrun) installed** and working
  (`brew install tsirysndr/tap/bsdkrun`, `npm i -g @bsdkrun/cli`, or a local
  build). The app auto-detects it on `PATH` / Homebrew; you can also point at a
  specific binary in **Settings**.
- **[Rust](https://rustup.rs)** (stable) and a Tauri toolchain.
- **[Bun](https://bun.sh)** and **Node 24** — pinned via [`mise`](https://mise.jdx.dev)
  (`mise install`).

## Develop

```sh
mise install          # Node 24 + Bun (or install them yourself)
bun install
bun run tauri dev     # launches the app with HMR
```

## Build

```sh
bun run tauri build   # produces a signed .app / installer under src-tauri/target
```

> On macOS, the packaged app must be codesigned with the Hypervisor entitlement
> to boot VMs — the same requirement as the `bsdkrun` CLI. See the main repo's
> README for details.

## Keyboard shortcuts

| Key            | Action                 |
| -------------- | ---------------------- |
| `/` · `⌘K`     | Command palette        |
| `?`            | Shortcuts help         |
| `R`            | Refresh                |
| `N` · `⌘N`     | Run new machine        |
| `⌘1/2/3`       | Machines / Images / Volumes |
| `⌘⇧S`          | Stop all running       |
| `Esc`          | Close dialog / palette |

## How it maps to the CLI

| GUI action          | CLI                              |
| ------------------- | -------------------------------- |
| Machines list       | `bsdkrun ps -a --json`           |
| Images list         | `bsdkrun images --json`          |
| Volumes list        | `bsdkrun volume ls --json`       |
| Run                 | `bsdkrun <kind> -d …`            |
| Stop                | `bsdkrun stop <id>`              |
| Terminal            | `bsdkrun exec -t <id> /bin/sh`   |
| Logs (live)         | `bsdkrun logs -f <id>`           |
| Remove volume       | `bsdkrun volume rm -f <name>`    |
| Engine status       | `bsdkrun probe`                  |

## License

Same as the parent project — see the repository root.
