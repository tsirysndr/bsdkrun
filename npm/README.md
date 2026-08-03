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
  `brew tap libkrun/krun && brew install libkrun`.
- **Linux:** **libkrun is bundled** — the archive ships `libkrun.so`/`libkrunfw.so`
  (from the [PVH-enabled fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot))
  next to `bsdkrun`, rpath'd to `$ORIGIN`, so no separate libkrun install is needed.
  You also need KVM access (`/dev/kvm`).

**Guest networking (`gvproxy`) is auto-installed.** Postinstall also downloads the
matching [`gvproxy`](https://github.com/containers/gvisor-tap-vsock) for your
platform into the package and wires `bsdkrun` to it, so user-mode networking works
out of the box. It's best-effort — if the download fails the guest still boots
without a NIC, and you can install gvproxy yourself (`brew install gvproxy`) or set
`BSDKRUN_GVPROXY`. Skip it with `BSDKRUN_SKIP_GVPROXY=1`.

## Environment variables

| Variable                  | Effect                                                        |
| ------------------------- | ------------------------------------------------------------ |
| `BSDKRUN_SKIP_DOWNLOAD`   | Skip the postinstall download entirely.                      |
| `BSDKRUN_BINARY`          | Install a local binary (path) instead of downloading.        |
| `BSDKRUN_DOWNLOAD_BASE`   | Override the release download base URL (for mirrors/testing).|
| `BSDKRUN_SKIP_GVPROXY`    | Skip the gvproxy (user-mode networking) download.            |
| `BSDKRUN_GVPROXY`         | Path to an existing gvproxy; skips its download and is used at runtime. |
| `BSDKRUN_GVPROXY_VERSION` | Pin a gvproxy release tag (default: latest).                 |

## License

MIT
