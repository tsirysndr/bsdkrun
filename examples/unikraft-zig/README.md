# Zig on Unikraft

A Zig HTTP service, cross-compiled to a **static musl binary**.

Zig needs no cross toolchain to target another architecture, which makes it
about the friendliest input a unikernel build can have. The compiler is
installed with [mise](https://mise.jdx.dev) rather than fetched directly, because
Zig's own tarball naming has changed between releases
(`zig-linux-x86_64-<v>` in some, `zig-x86_64-linux-<v>` in others) and mise
knows which is which.

Detected by `build.zig` or any `*.zig` file. `ZIG_VERSION` pins the compiler
and beats any `.tool-versions` or `mise.toml`.

The listener deliberately does **not** set `SO_REUSEADDR`: Unikraft's lwip
does not implement it, and setting it fails the listen outright with
`InvalidProtocolOption`. Nothing is lost — a unikernel is the only thing that
ever binds this port.

There is no `Dockerfile`, no `Kraftfile` and no `build.sh` — `bsdkrun pack`
detects the project and generates all three internally.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "zig -- /usr/bin/server"
```

`pack` prints that command when it finishes.

## Try it

```sh
curl http://<vm-ip>:8080/
```

## Publish it

```sh
bsdkrun pack . --push ghcr.io/you/zig:v1
bsdkrun unikraft ghcr.io/you/zig:v1
```

The second command needs no copy of this directory: the kernel is pulled on
first use and cached, and the argv comes from the image.
