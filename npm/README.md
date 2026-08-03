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

On install, a prebuilt `bsdkrun` binary for your platform is downloaded from the
matching [GitHub release](https://github.com/tsirysndr/bsdkrun/releases) and its
SHA-256 verified.

## Supported platforms

| OS     | Arch          | Notes                          |
| ------ | ------------- | ------------------------------ |
| macOS  | `arm64`       | Apple Silicon only (no Intel)  |
| Linux  | `x64`         | KVM (`/dev/kvm`)               |
| Linux  | `arm64`       | KVM (`/dev/kvm`)               |

Any other platform/arch (Windows, Intel macOS, 32-bit, …) is **unsupported** and
the install will fail with a clear message.

## Runtime prerequisite: libkrun

The binary dynamically links **libkrun** at runtime — install it separately:

- **macOS:** `brew tap libkrun/krun && brew install libkrun`
- **Linux:** build/install libkrun (see the
  [project README](https://github.com/tsirysndr/bsdkrun#prerequisites); amd64 BSD
  PVH boots need the
  [PVH-enabled fork](https://github.com/tsirysndr/libkrun/tree/feat/pvh-boot)).

## Environment variables

| Variable                | Effect                                                       |
| ----------------------- | ------------------------------------------------------------ |
| `BSDKRUN_SKIP_DOWNLOAD` | Skip the postinstall download entirely.                      |
| `BSDKRUN_BINARY`        | Install a local binary (path) instead of downloading.        |
| `BSDKRUN_DOWNLOAD_BASE` | Override the release download base URL (for mirrors/testing).|

## License

MIT
