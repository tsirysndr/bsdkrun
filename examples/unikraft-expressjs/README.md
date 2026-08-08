# unikraft-expressjs

An [ExpressJS](https://expressjs.com/) server on Node 24, running as a Unikraft
unikernel. Ported from [`unikraft/catalog`'s
`expressjs4.18-node24-base`](https://github.com/unikraft/catalog/tree/main/examples/expressjs4.18-node24-base)
to build for **arm64** and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 --cmdline "elfloader -- /usr/bin/node /usr/src/server.js"
```

## Status: does not run yet

The kernel boots, seeds entropy, brings up networking (DHCP), and loads the ELF —
but `node` then faults during libc startup, branching to a null GOT entry
(`br x17` with `x17 == 0`, an unresolved PLT relocation). See
[../../library/unikraft-base/README.md](../../library/unikraft-base/README.md)
for what has been ruled out and what the next debugging step is. Everything in
this directory is otherwise complete and builds cleanly.

## Differences from upstream

**No `runtime: base:latest`.** Upstream pulls a prebuilt kernel that is published
for x86_64 only, so this Kraftfile builds that same runtime from source instead.
That is also why the kconfig block here is large — it is `library/base` from the
catalog, plus the arm64 fixes described in the base README.

**The Dockerfile resolves its libraries instead of listing them.** Upstream
hardcodes `/lib/ld-musl-x86_64.so.1`; on arm64 the musl loader is
`ld-musl-aarch64.so.1`, a different filename, so no substitution fixes it in
place. Asking `ldd` keeps it correct on both architectures and picks up anything
a future node build starts needing.

## Memory

**`--mem 2048`.** The default 512 MiB is not enough and fails during boot with a
misleading error:

```
ERR: [libukcpio] /./usr/bin/node: Failed to load content: Input/output error (5)
CRIT: [libvfscore] Failed to extract cpio archive to /: -3
```

That is memory exhaustion, not I/O. The root filesystem is embedded in the kernel
image *and* has to be unpacked into a RAM filesystem, so it is resident twice —
about 254 MiB before V8 allocates anything, out of 512.

Trimming the image would help: the `node` binary alone is 126 MiB unstripped, and
`node_modules` ships READMEs and history files.

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel, so the
program to run has to be given explicitly. The format is

```
<argv0> -- <application argv>
```

Everything before `--` is parsed as kernel library parameters and **the first
word is skipped** (Unikraft treats it as the program name), so the leading
placeholder is not optional — dropping it silently feeds your first parameter to
the parser as `argv[0]`.
