# @bsdkrun/cli

Install the [`bsdkrun`](https://github.com/tsirysndr/bsdkrun) host binary via npm.

`bsdkrun` is a Firecracker-style **microVM launcher for BSD and Linux (OCI) guests**,
built on [libkrun](https://github.com/containers/libkrun) (Hypervisor.framework on
macOS, KVM on Linux).

## Install

```sh
npm install -g @bsdkrun/cli
# then
bsdkrun --help
```

Or run it without a global install:

```sh
npx @bsdkrun/cli linux alpine -- echo hello
```

On install, a prebuilt `bsdkrun` for your platform is downloaded from the matching
[GitHub release](https://github.com/tsirysndr/bsdkrun/releases) and its SHA-256
verified. The **Linux** archive bundles libkrun (see below); the **macOS** archive
is just the binary and links Homebrew's libkrun.

## Supported platforms

| OS     | Arch          | Notes                          |
| ------ | ------------- | ------------------------------ |
| macOS  | `arm64`       | Apple Silicon only (no Intel)  |
| Linux  | `x64`         | KVM (`/dev/kvm`)               |
| Linux  | `arm64`       | KVM (`/dev/kvm`)               |

Any other platform/arch (Windows, Intel macOS, 32-bit, …) is **unsupported** and
the install will fail with a clear message.

## Runtime prerequisites

- **macOS:** install **libkrun** (the binary links it at runtime):
  `brew tap libkrun/krun && brew install libkrun`. Homebrew pulls in `gvproxy` too.
- **Linux:** **libkrun is bundled** — the archive ships `libkrun.so`/`libkrunfw.so`
  (from the [PVH-enabled fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot))
  next to `bsdkrun`, rpath'd to `$ORIGIN`, so no separate libkrun install is needed.
  Guest **networking** still uses `gvproxy` — put it on `PATH` (or set
  `BSDKRUN_GVPROXY`); boots without networking don't need it. You also need KVM
  access (`/dev/kvm`).

## Environment variables

| Variable                | Effect                                                       |
| ----------------------- | ------------------------------------------------------------ |
| `BSDKRUN_SKIP_DOWNLOAD` | Skip the postinstall download entirely.                      |
| `BSDKRUN_BINARY`        | Install a local binary (path) instead of downloading.        |
| `BSDKRUN_DOWNLOAD_BASE` | Override the release download base URL (for mirrors/testing).|

## License

MIT
