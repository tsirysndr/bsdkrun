# unikraft-go

A plain Go [`net/http`](https://pkg.go.dev/net/http) server, running as a
Unikraft unikernel — packaged with `bsdkrun pack` instead of a hand-written
Dockerfile/Kraftfile/build.sh, unlike every other `examples/unikraft-*`.

There is deliberately no `Dockerfile`, `Kraftfile`, or `build.sh` here: `pack`
detects the `go.mod`, builds a static binary with BuildKit, generates the
Kraftfile itself, and drives `kraft build` — the whole pipeline the other
examples do by hand.

```sh
bsdkrun pack .
bsdkrun unikraft . --port 18080:8080 --cmdline "hello -- /hello" -d
```

```console
$ curl localhost:18080/              Bye, World!
$ curl localhost:18080/hey           Buh bye!
$ curl -d ping localhost:18080/echo  ping
```

## Status

**arm64 works** — verified on macOS/Hypervisor.framework: builds, boots, gets
a network interface, and serves all three routes above over a forwarded port.

x86_64 is not yet verified here (this example isn't wired into
`.github/workflows/e2e-unikraft-examples.yml`, which only covers the
hand-written examples).

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd:` for a locally-built kernel — see
[`../unikraft-expressjs/README.md`](../unikraft-expressjs/README.md#--cmdline-is-required)
for why. `pack` prints the right `--cmdline` for you at the end of its run;
the value above (`hello -- /hello`) matches this module's name (`go.mod`'s
`module hello`, which `pack` also uses as the built binary's name).

## What `pack` generates

Running `bsdkrun pack .` here writes (all gitignored, see `.gitignore`):

- `.rootfs-arm64/` — a BuildKit-built rootfs holding just the static `hello`
  binary (`CGO_ENABLED=0`, so nothing to `ldd`-resolve, unlike the Rust
  examples).
- `Kraftfile` — the same shared base kconfig block every other example here
  carries, generated rather than hand-copied.
- `.unikraft/` — the fetched-and-patched Unikraft source tree kraft builds
  against.

Re-running `bsdkrun pack .` reuses the fetched `.unikraft/` tree (most of the
wall-clock on a rebuild) rather than re-fetching it every time.
