# unikraft-caddy

[Caddy](https://caddyserver.com/) 2.7.6 (built against Go 1.21), running as a
Unikraft unikernel. Ported from [`unikraft-cloud/examples`'s
`caddy2.7-go1.21`](https://github.com/unikraft-cloud/examples/tree/main/caddy2.7-go1.21)
to build for **arm64** as well as x86_64 and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 512 --port 18081:2015 \
  --cmdline "elfloader -- /usr/bin/caddy run --config /etc/caddy/Caddyfile"
```

```console
$ curl http://127.0.0.1:18081/
Hello, world!
```

## Status

**arm64 works.** The unikernel boots, DHCPs an address, caddy starts, serves
its admin API on `localhost:2019`, and `GET /` returns `Hello, world!` over
the forwarded port. x86_64 has never been run;
`.github/workflows/e2e-unikraft-examples.yml` runs it as `strict: false`
until its first green run.

No argv trampoline was needed. Unlike `../unikraft-redis`'s `redis-server`,
which fatals on the first stray word libkrun appends past the kernel
cmdline's `--` stop sequence, caddy's cobra-based CLI tolerates it: booting
`elfloader -- /usr/bin/caddy run --config /etc/caddy/Caddyfile` directly
works, so there is no `start.sh` and no bundled busybox shell.

Two harmless `SO_REUSEPORT` errors appear in the boot log
(`setting SO_REUSEPORT ... "error": "protocol not available"`, once for the
admin endpoint on `127.0.0.1:2019` and once for the `:2015` listener) —
Unikraft's socket layer does not implement that setsockopt. Caddy logs the
error and binds the socket without it anyway; nothing else depends on it in
this single-instance setup.

## Differences from upstream

**No `runtime: base-compat:latest`.** Upstream pulls a prebuilt Unikraft Cloud
kernel, which is published for x86_64 only. The Kraftfile here builds the
equivalent runtime (`library/base` from `unikraft/catalog`) from source, plus
the arm64 fixes in `../../library/unikraft-base`.

**Fully static binary, no cgo.** Upstream cross-compiles with cgo
(`-linkmode external -extldflags -static-pie`), which needs a C toolchain for
the target platform. This port builds with `CGO_ENABLED=0 GOOS=linux
GOARCH=$TARGETARCH go build -tags netgo -ldflags "-s -w"` instead: no C
toolchain to cross-compile, and the resulting binary has no ELF interpreter
or dynamic library dependencies, so — unlike `../unikraft-php` and
`../unikraft-redis` — the Dockerfile needs no `ldd`-resolution step at all.
`file` on the built binary confirms `statically linked`.

**`docker buildx build --platform`, not a pinned `--platform=linux/x86_64`.**
`build.sh` (unchanged from the other examples here) builds the rootfs for the
*target* architecture; `TARGETARCH` is supplied automatically by buildx, no
`--build-arg` needed.

**No shipped `/etc/hosts`.** Upstream's `scratch` stage copies a static
`/etc/hosts` with a `127.0.0.1 localhost` line. This repo's app-elfloader
autogenerates `/etc/resolv.conf`, `/etc/hosts` and `/etc/hostname` at boot
(`CONFIG_APPELFLOADER_AUTOGEN_*` in the Kraftfile), so it is redundant here.

**`Caddyfile` and `index.html` are upstream's, verbatim** — a `:2015` site
root serving `/var/www` with gzip, templates, and the file server.

## Layout

| file         | role                                                           |
|--------------|-----------------------------------------------------------------|
| `Dockerfile` | rootfs: builds a static `caddy` binary, no `ldd` step needed    |
| `Caddyfile`  | `:2015`, `root * /var/www`, gzip, templates, file_server        |
| `index.html` | upstream's `Hello, world!` page                                 |
| `Kraftfile`  | the from-source base runtime + elfloader                        |
| `build.sh`   | two-phase build; see `../unikraft-postgres/build.sh`             |
