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

**Both architectures work.** arm64 is verified on macOS/Hypervisor.framework;
x86_64 is verified on every push by `.github/workflows/e2e-pack.yml`, which
packs this example with `bsdkrun pack` and boots it under KVM. Both serve all
three routes above over a forwarded port.

x86_64 did not work until the `-T` relocation in
`pack/internal/plan/go.go`. It is worth recording why, because it affects any
Go binary here and it fails in a way that leaves no evidence at all.

`go build` emits a **non-PIE `ET_EXEC`**, so its load addresses are fixed
rather than relocatable, and on amd64 they start at `0x400000` (4 MiB). The
Unikraft `fc` kernel links at ~1 MiB and grows past 4 MiB once the rootfs is
embedded into the image — so the loader maps the application on top of the
running kernel. `bsdkrun pack . --loader-debug` showed it getting exactly
two segments in:

```
loading /hello as hello
app: segment 2 r-x -> 0x400000..0x621000
app: segment 3 r-- -> 0x621000..0x858000
                      <- segment 4 (rw-) never mapped
```

It survives the two read-only segments and dies on the writable one, whose
BSS is zero-filled — over the kernel itself. Nothing is ever printed and no
crash screen appears, because what would print them is what got overwritten.

arm64 links at `0x10000` with the kernel 2 GiB away, which is why Go always
worked there. Every *other* example works on x86_64 because they are all
dynamically linked, so the loader relocates them freely into ukvmem's range.
Only a static non-PIE binary demands a fixed low address, and Go is the only
thing here that produces one — `../unikraft-caddy` is built the hand-written
way and hits the same wall (still unfixed there, and still `strict: false` in
`e2e-unikraft-examples.yml`), so this was never specific to `pack`.

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
