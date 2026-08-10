# unikraft-wasmer

A native Rust HTTP server that hosts the [Wasmer](https://wasmer.io/)
WebAssembly runtime and executes a `wasm32-wasip1` WASI guest module on every
request, running as a Unikraft unikernel. There is **no upstream template**
for this — `unikraft-cloud/examples` has no wasmer example — so this one is
built from scratch, following this repo's existing Rust examples
(`../unikraft-actix`) as closely as possible.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
../../target/release/bsdkrun unikraft . --mem 512 --port 18084:8080 \
  --cmdline "elfloader -- /server"
```

```console
$ curl http://127.0.0.1:18084/
Hello from Wasmer on Unikraft!
```

## Status

**arm64 works, on the first try.** `./build.sh` builds both Rust crates and
the kernel; the unikernel boots, DHCPs an address (`en1`), and the server
answers every request with the guest module's output. Verified with five
back-to-back `curl` requests, each one a fresh wasmer compile-and-execute
(~20-30ms per request end to end, including the HTTP round trip) — see
`## Architecture` for why every request recompiles instead of reusing a
cached result.

`--mem 512` (bsdkrun's default) was enough. No OOM-shaped boot failure was
hit, so there was never a reason to raise it toward the 2048 MiB
`../unikraft-expressjs` needed for V8. The W^X / icache patches in
`../../library/unikraft-base` (entries 10, 11 and 14 in that README — needed
by V8's JIT for the same structural reason wasmer's JIT needs
writable-then-executable pages) were not touched, and no crash was observed
on the first JIT'd call: they did their job generically, exactly as
advertised, with no wasmer-specific patch needed.

Boot prints the same one non-fatal error as `../unikraft-nginx` and others,
before the network comes up — libkrun attaching a device (id 5, almost
certainly the memory balloon) that Unikraft has no driver for on this path;
harmless, and covered in more detail there:

```
ERR:  [libvirtio_bus] <virtio_bus.c @  141>  Failed to find the driver for the virtio device 0x41def7020 (id:5)
ERR:  [libvirtio_mmio] <virtio_mmio.c @  544>  Failed to register the virtio device: -14
ERR:  [libukbus_platform] <platform_bus.c @  118>  Platform Failed to initialize device driver, ret(-14)
```

x86_64 has never been run; `.github/workflows/e2e-unikraft-examples.yml`
should run it as `strict: false` until its first green run, matching the
convention for every other example here that hasn't been tried on that
architecture yet.

**Not part of this example:** WASI networking (still experimental in
wasmer-wasix) — see `## Architecture` for why that's a deliberate scope cut,
not a limitation hit along the way.

## Architecture

A single binary (`app/`) does two things:

1. Embeds a tiny precompiled WASI guest module (`guest/`, a `wasm32-wasip1`
   Rust program that writes one line to stdout and exits) via `include_bytes!`
   at compile time — no filesystem path juggling at boot, and the image ships
   as one file plus its shared libraries.
2. Opens a plain `std::net::TcpListener` on port 8080 and, on every
   connection, compiles and runs the embedded module fresh with `wasmer` +
   `wasmer-wasix` (capturing its WASI stdout into an in-memory pipe via
   `Pipe::channel()`), then writes the captured output back as a minimal
   HTTP/1.1 response.

Only the *host* binary ever touches a real socket — the wasm guest's only
interaction with the outside world is its own stdout, piped in memory. This
is deliberate: WASI's socket extensions in wasmer-wasix are still
experimental, and sidestepping them entirely means this example still
genuinely exercises wasmer's compile-and-execute path — a fresh `Engine` and
`Module` are built on *every* request in `run_guest()` (`app/src/main.rs`),
not cached at boot and replayed — without depending on the least stable part
of the runtime. It's architecturally the same shape as every other server in
this directory: a normal host-side socket loop (`../unikraft-actix`,
`../unikraft-php`) — wasmer is just what runs behind it here instead of
nothing.

Connections are handled one at a time in a sequential accept loop (like
`../unikraft-php/server.php`), not one thread per connection: simplest
possible threading model for a first port to a platform nothing wasm-shaped
had run on before this. A single multi-threaded tokio runtime is built once
in `main()` and entered for the process's whole lifetime, because
wasmer-wasix's `WasiRunner::run_wasm` needs one — entered ahead of time, it
reuses that runtime on every call instead of spinning one up and tearing it
down per request.

argv is never inspected. libkrun appends its own words (`earlycon=...`,
`tsi_hijack`, a bare `--`) past the kernel command line's `--` stop sequence,
and Unikraft hands them to this binary as argv — see
`../unikraft-php/README.md`. A fixed port (8080) is simpler than parsing an
optional override out of argv, and just as correct.

## Compiler backend: singlepass

`wasmer`'s default feature set (`sys-default`) pulls in the `cranelift`
compiler. This example turns that off (`default-features = false` in
`app/Cargo.toml`) and enables `singlepass` instead: it JIT-compiles faster
and produces simpler generated code, which made it the more conservative
choice for a from-scratch port to a platform nothing wasm-shaped has run on
before. It worked on the first try — compiling under Docker buildx for
arm64 (natively, no QEMU emulation needed on this Apple Silicon host) and
executing correctly inside the guest — so cranelift was never tried.

## Versions

`wasmer` 7.2.1 and `wasmer-wasix` 0.702.1 (their version numbers track each
other: `X.Y.Z` on wasmer pairs with `0.X0Y.Z`-ish on wasmer-wasix — pin both
exactly, they are not independently interchangeable). `wasmer-types`,
`virtual-fs` and `virtual-mio` are pulled in directly too, at the same
`0.702.1`/`7.2.1` line, only because their symbols (`ModuleHash`,
`AsyncReadExt`, `block_on`) are used directly in `app/src/main.rs` and
aren't re-exported from the `wasmer_wasix` crate root.

**`rustc` is pinned newer than the other Rust examples**
(`rust:1.97.1-bookworm` vs. actix's `1.75.0-bookworm`, in `Dockerfile`):
`wasmer` 7.2.1 declares `rust-version = "1.93"`.

## Differences from upstream

There is no upstream to differ from — this Kraftfile, Dockerfile, and both
Rust crates were written for this repo. The Kraftfile's `unikraft:` kconfig
block and `libraries:` block are copied verbatim from `../unikraft-actix`
(the closest analog: a compiled Rust binary, no interpreter), for the same
reasons documented there — `runtime: base:latest` is published for x86_64
only, so the runtime is built from source here too, with the arm64 fixes in
`../../library/unikraft-base`.

**The Dockerfile resolves its libraries instead of listing them**, via
`ldd`, exactly like `../unikraft-actix/Dockerfile` — the arm64/x86_64 dynamic
loader paths differ by directory *and* filename, so no static list works for
both.

## Layout

| path         | role                                                                     |
|--------------|---------------------------------------------------------------------------|
| `app/`       | the host: `Cargo.toml` + `src/main.rs` — TCP server, wasmer/WASI runner   |
| `guest/`     | the WASI guest: `Cargo.toml` + `src/main.rs`, built for `wasm32-wasip1`   |
| `Dockerfile` | rootfs: builds both crates, resolves the host binary's libraries via `ldd`|
| `Kraftfile`  | the from-source base runtime + elfloader, copied from `../unikraft-actix` |
| `build.sh`   | two-phase build; copied verbatim from `../unikraft-actix/build.sh`        |
