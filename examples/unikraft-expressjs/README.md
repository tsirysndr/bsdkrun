# unikraft-expressjs

An [ExpressJS](https://expressjs.com/) server on Node 24, running as a Unikraft
unikernel. Ported from [`unikraft/catalog`'s
`expressjs4.18-node24-base`](https://github.com/unikraft/catalog/tree/main/examples/expressjs4.18-node24-base)
to build for **arm64** and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 2048 --cmdline "elfloader -- /usr/bin/node /usr/src/server.js"
```

## Status

**Both architectures work.** The unikernel boots, DHCPs an address, node
starts, and the server answers over a forwarded port:

```console
$ curl http://127.0.0.1:3000/
Bye, World!
```

x86_64 is covered end to end by `.github/workflows/e2e-unikraft-examples.yml`.
arm64 is verified on macOS/Hypervisor.framework.

The loader is [app-elfloader-rs](https://github.com/tsirysndr/app-elfloader), a
Rust rewrite of upstream `app-elfloader`; the Kraftfile pulls it like any other
library. Set `ELFLOADER_RS=/path/to/checkout` to build a working copy instead.

### What arm64 needed

Getting node to run turned up four bugs in Unikraft, none of them in the
loader — the C `app-elfloader` failed at exactly the same points. All are
applied by `../../library/unikraft-base/patches/apply.sh`:

| # | bug |
|---|-----|
| 10 | `invalidate_icache_range()` invalidated the I-cache *before* cleaning the D-cache, and strode by the wrong line size. Both halves are wrong for publishing newly written code. |
| 11 | `ukvmem` did no I-cache maintenance at all when demand-paging executable pages, so `.text` could be fetched stale from whatever previously used the frame. Linux does this in `set_pte_at()`. |
| 12 | **`struct stat` was the x86_64 layout on every architecture** (144 bytes vs arm64's 128 — the header carried a `FIXME` saying so). vfscore's `vn_stat()` starts with `memset(st, 0, sizeof(struct stat))` on the *application's* buffer, so every `stat()`/`fstat()` wrote 16 zero bytes past the end of it. In musl's `load_library()` that is the saved frame pointer and return address, so **any** binary loading a second shared library returned to address 0. |
| 13 | The arm64 signal trampoline had **never assembled** (`and sp, sp, #~0xf`; SP is not a valid `AND` operand), so `CONFIG_LIBPOSIX_PROCESS_SIGNAL` could not be enabled and no CPU fault reached the application as a signal. node needs it: OpenSSL probes for the SM3 extension by *executing* an SM3 instruction under `sigsetjmp` and catching `SIGILL`. |
| 14 | **W^X was enforced with `SCTLR_EL1.WXN`**, a control for the entire EL1&0 regime rather than a per-page attribute. While it is set no page can be both writable and executable — including application pages — so V8's RWX code space took an instruction abort on the first jump into JIT-ed code. x86_64 has no such global bit, which is why it was never affected. The patch marks kernel regions execute-never through their PTEs instead, exactly as x86_64 already did. |

`repro/` documents how these were found: a one-second reproducer instead of a
two-minute node boot, QEMU under both HVF and TCG (which is what exonerated
libkrun), and the gdb stub.

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
