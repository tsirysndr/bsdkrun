# Swift on Unikraft

A Swift HTTP service, built with Swift Package Manager.

Detected by `Package.swift` or `.swift-version`. The toolchain version comes
from `.swift-version` first, then the `swift-tools-version` declared at the top
of `Package.swift` — a weaker signal, since it describes the manifest format
rather than the compiler, but the only version a package must carry.

Built with `--static-swift-stdlib`, which links the Swift runtime in. libc and
the ICU that Foundation reaches for are still dynamic, so those are resolved into
the rootfs with `ldd`.

The server uses POSIX sockets rather than a framework: every Swift HTTP package
pulls in SwiftNIO and a dozen transitive dependencies, and this example is about
the toolchain reaching the guest.

There is no `Dockerfile`, no `Kraftfile` and no `build.sh` — `bsdkrun pack`
detects the project and generates all three internally.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "swift -- /usr/bin/server"
```

`pack` prints that command when it finishes.

## Try it

```sh
curl http://<vm-ip>:8080/
```

## Publish it

```sh
bsdkrun pack . --push ghcr.io/you/swift:v1
bsdkrun unikraft ghcr.io/you/swift:v1
```

The second command needs no copy of this directory: the kernel is pulled on
first use and cached, and the argv comes from the image.
