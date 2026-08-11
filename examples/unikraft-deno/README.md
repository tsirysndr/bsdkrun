# unikraft-deno

[Deno](https://deno.com) 2.9 — V8 15, TypeScript 6 — running as a Unikraft
unikernel.

```sh
bsdkrun pack .                # host arch; or: bsdkrun pack . --target x86_64
bsdkrun unikraft . --mem 2048 --port 3000:3000 \
    --cmdline "elfloader -- /usr/bin/deno run --quiet --allow-net /usr/src/server.js"
```

```console
$ curl http://127.0.0.1:3000/
Hello from Deno on Unikraft!

$ curl http://127.0.0.1:3000/info
{"runtime":"deno","version":"2.9.5","v8":"15.0.245.2-rusty","typescript":"6.0.3"}
```

## Status

**Works on arm64**, verified on macOS/Hypervisor.framework. x86_64 builds; it
has not been booted here, since this machine is Apple silicon.

This leans on the whole arm64 stack landed for `../unikraft-expressjs` — in
particular the `struct stat` and `SCTLR_EL1.WXN` fixes in
`../../library/unikraft-base/patches/apply.sh`, without which no dynamically
linked binary loads and no JIT can execute the code it generates.

## `--quiet` is not optional

Without it Deno never gets as far as listening. Its progress bar redraws in a
tight loop — 1.27 million empty `ESC[0G ESC[2K ESC[J` frames in 90 seconds,
about 14,000 a second — and on a single vCPU that starves the thread doing the
actual work. `--quiet` suppresses the progress bar and Deno runs normally.

The redraw throttle is what fails, not the drawing: something in the draw
loop's timing (a sleep or a timed condition wait on the spinner thread) returns
immediately under Unikraft. `--cpus 2` alone does not help, which is consistent
with a broken throttle rather than plain CPU starvation. Worth chasing
separately; `--quiet` is a legitimate flag, not a workaround for a wrong
result.

## JavaScript, not TypeScript

`app/server.js` is plain JS on purpose. `deno run` on a `.ts` file transpiles
through a cache under `DENO_DIR`, and the guest's writable storage is a RAM
filesystem that starts empty. TypeScript would work with `DENO_DIR` pointed
somewhere writable, but it buys nothing for a two-route server.

## Debian, not Alpine

`denoland/deno:alpine` does not ship a musl Deno. It carries a glibc shim under
`/usr/local/lib/glibc` and runs the ordinary glibc binary through it — its own
`ldd` reports a broken relocation there (`_dl_find_object: symbol not found`).
Plain Debian glibc is the simpler and more predictable starting point.

## Harmless noise in the boot log

Deno's SQLite caches want `mmap(MAP_SHARED)` on a file, which Unikraft does not
support (`-95`, `EOPNOTSUPP`):

```
ERR: [libposix_mmap] mmap(addr=0, len=32768, prot=3, flags=1, fd=13, offset=0) failed: -95
Could not initialize cache database '/.cache/deno/v8_code_cache_v2' ...
Failed to open cache file '/.cache/deno/v8_code_cache_v2', performance may be degraded.
```

Deno falls back to in-memory caches and carries on. It costs a little start-up
time on every boot, since nothing is cached across runs.

## Memory

**`--mem 2048`.** The root filesystem is embedded in the kernel image *and*
unpacked into a RAM filesystem, so the 84 MiB of Deno is resident twice before
V8 allocates anything.

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel. The
format is `<argv0> -- <application argv>`; everything before `--` is parsed as
kernel library parameters and **the first word is skipped**, so the leading
`elfloader` placeholder is not optional.

Library parameters are useful here: `env.vars=NAME=value` (up to 8) sets guest
environment variables without rebuilding, which is how the runtime knobs in
this README were tested.
