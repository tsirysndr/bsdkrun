# unikraft-actix

An [Actix](https://actix.rs/) web server in Rust, running as a Unikraft
unikernel. Ported from [`unikraft/catalog`'s
`actix4-rust1.75`](https://github.com/unikraft/catalog/tree/main/examples/actix4-rust1.75)
to build for **arm64** and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 1024 --cmdline "elfloader -- /server"
```

## Status: never built

Unlike the sibling ExpressJS example, this one has **not been compiled or booted
even once**. The files are a direct port and are believed correct, but nothing
here is verified. Expect it to need the same treatment as its sibling, and at
minimum to hit the same unresolved application-startup fault described in
[../../library/unikraft-base/README.md](../../library/unikraft-base/README.md).

## Differences from upstream

**No `runtime: base:latest`.** Upstream pulls a prebuilt kernel published for
x86_64 only, so this Kraftfile builds that runtime from source. See the base
README for the arm64 fixes that requires.

**The Dockerfile resolves its libraries instead of listing them.** Upstream
hardcodes `/lib/x86_64-linux-gnu/...` and `/lib64/ld-linux-x86-64.so.2`; on arm64
those are `/lib/aarch64-linux-gnu/...` and `/lib/ld-linux-aarch64.so.1` — a
different directory *and* a different filename. `ldd` reports whatever the target
architecture actually needs.

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel. The
format is `<argv0> -- <application argv>`; the first word before `--` is skipped
by Unikraft's parameter parser, so the placeholder is required. See the ExpressJS
README for details.
