# Working in this repo

`bsdkrun` runs VMs on libkrun: BSD guests, Linux, and Unikraft unikernels.
`pack/` is a Go tool, embedded in the Rust CLI, that turns a project into a
bootable unikernel — railpack for Unikraft.

## Build and run

```sh
cargo build --release          # always --release; never a debug build
make sign-release              # or the codesign line below
```

**Re-sign after every build.** `cargo build` strips the code signature, and an
unsigned binary fails VM creation on macOS with a bare `EINVAL` from
`krun_start_enter` that says nothing about signing:

```sh
codesign --entitlements bsdkrun.entitlements --force -s - target/release/bsdkrun
```

The desktop app's `binary_path` points at `target/release/bsdkrun`, so it breaks
the same way until you re-sign.

**Only ever build the host architecture locally.** x86_64 goes through the e2e
workflow matrix, where new targets start `strict: false`.

`libkrun` is **locally patched** — Homebrew's dylib is replaced by a local build
carrying an `XATTR_NOFOLLOW` fix. Rebuilding it needs `lld`, `libclang` on the
rpath, and matching feature flags.

## Reading CI results

A job's conclusion proves nothing for a `strict: false` entry: the step swallows
the failure and the job still goes green. **Read the response body the guest
actually served**, not the conclusion. Every packed example asserts on an
`expect:` string for this reason.

Push with `git push origin HEAD:<branch>`. Pushing a branch name alone has
sent stale local commits while the real work sat on `main`, producing a green
run of code that was never tested.

## `bsdkrun pack`

Go at `pack/`, compiled by `core/build.rs`, embedded with `rust_embed`, extracted
to the cache dir and `exec`'d. **An end user never needs Go.** On macOS the
extracted binary is ad-hoc codesigned before exec — an entitled binary can only
`exec` into a signed one, and without it the guest is SIGKILLed (exit 137) with
no crash report naming the cause.

Pipeline: detect → plan → BuildKit LLB → generated Kraftfile → `kraft build`.

### Rules that cost a build to learn

- **`pack` writes a Kraftfile into the directory it packs.** Never run it against
  an example that ships a hand-written one — it overwrites it, and `git add -A`
  then commits the generated file over the original. Build in a `/tmp` copy.
  Generated outputs are gitignored repo-wide as a second line of defence.
- **`pack` embeds its own copy of `library/unikraft-base/patches/`.** It must run
  against a user's project, not a checkout of this repo, so patching only
  `library/` changes nothing for `pack`. **Patch both copies.**
- **Always print the boot command.** Every example, README and `pack` run ends
  with the exact `bsdkrun unikraft ... --cmdline "..."` to copy-paste, because
  `bsdkrun unikraft` does not read the Kraftfile's `cmd:`.
- Print short relative paths (`.`), not long absolute ones.
- `.dockerignore` is still needed with `pack` — it bounds the build context and
  is not tied to the Dockerfile.
- Flags may follow positionals (`pack . --strace`); `main.go` permutes argv
  because Go's `flag` stops at the first positional. Two rounds of diagnosis were
  invalidated by flags silently reading `false`.
- The plain reporter prints the failing script's stderr with a `|` prefix. When a
  build fails, read those lines before theorising — `tail -12` has hidden them.

### Guest constraints every provider hits

| Constraint | Consequence |
| ---------- | ----------- |
| No `fork()` | `clone.c` rejects any clone without `CLONE_VM`: *"Multiple address spaces are not supported"*. php-fpm and anything forking workers cannot run. Use fork+exec (`posix_spawn`) with `APPELFLOADER_MULTIPROCESS`. |
| One CPU, no cgroup | The JVM and .NET size thread pools from a CPU count they cannot read. `providers/jvm` passes the flags that make this survivable; they are load-bearing, not tuning. |
| Non-PIE `ET_EXEC` on amd64 | Go, Crystal, Haskell and Caddy link at `0x400000`, where the `fc` kernel lives. Relink with `-T 0x40000000` (Go), `-Wl,-Ttext-segment=` (Crystal/Haskell). arm64 needs none of this. |
| Rootfs resident twice at boot | It is embedded in the kernel *and* unpacked into ramfs, so size costs RAM twice. jlink the JRE; trim Python's stdlib. |
| No `SO_REUSEADDR` | lwip does not implement it; setting it fails the listen outright. |
| `llb.Image` carries no image config | Base-image `ENV` is absent unless restated. `buildkit.imageEnv` resolves it — four separate CI failures (`PATH`, `CARGO_HOME`, `PHP_INI_DIR`, `JAVA_HOME`) came from this. |

### Adding a provider

Implement `Detect`/`Plan`/`Name`/`StartCommandHelp`, register in
`providers.All()`. Order matters: specific markers before broad ones, and
`static` stays **last** — most frameworks ship an `index.html`, and serving that
instead of running the app fails silently and totally.

Shared behaviour lives in `providers/jvm` (jlink + JVM flags) and
`providers/beam` (ERTS extraction). Base kconfig lives once in
`kraftfile.go` — if 13 examples set a symbol, it belongs there, not in one
provider. `CONFIG_LIBPOSIX_PROCESS_SIGNAL` sat in the node provider alone and
cost a JVM crash to find.

## Patching Unikraft

`library/unikraft-base/patches/apply.sh` **and** `pack/internal/kraft/patches/apply.sh`.
Each section guards with a `grep` marker so it is idempotent, and verifies the
edit landed. Two patches here exist because a guest died before `main()`:

- `CLOCK_PROCESS_CPUTIME_ID` — GHC's runtime asks for it at startup; without it,
  Haskell died with `clock_gettime: Invalid argument` after booting cleanly.
- POSIX timers (`timer_create` and friends) — all five were `-ENOTSUP` stubs. PHP
  arms one for `max_execution_time` and died with *"Could not create timer"*
  before serving. Now a timer table scanned by one polling thread.

- `RLIMIT_STACK` reporting 64 KiB — Unikraft answers with the *kernel* thread
  stack size while the app runs on the elfloader's. glibc's
  `pthread_getattr_np` clamps the main thread's computed stack bounds by that
  rlimit, so any runtime asking glibc for its own stack gets bounds that
  exclude its own stack pointer. CoreCLR died on this as `E_OUTOFMEMORY`; the
  dotnet provider's `/proc/self/maps` `[stack]` line is the other half.

- Wall clock from `epoch.boot=<unix seconds>` — the guest has no working
  source of wall time (January 1970 on arm64, a stale RTC on x86_64), which
  makes every TLS certificate on earth "not yet valid". bsdkrun injects the
  parameter into every unikraft cmdline that has a `--` separator; the patched
  posix-time offsets its wall clock by it. Old kernels ignore it.

When a guest boots and then dies on something that reads like an application
fault, check whether the syscall is a stub before blaming the application.
When nothing at the syscall boundary fails, **differential-trace it**: run the
same binary under Docker with strace, align the two traces, and read what the
good run does next at the point where the guest dies. That is what solved
.NET after seven single-hypothesis attempts failed.

## `bsdkrun solo5` — the guest that isn't a libkrun guest

MirageOS/Solo5 unikernels run under `solo5-hvt`, a **tender that is itself the
hypervisor front end** (Hypervisor.framework on macOS, KVM on Linux). libkrun is
not involved. `core/build.rs` builds it from the `library/solo5` submodule and
embeds it; `commands/solo5.rs` supervises it as a child process.

What that split costs, and what pays for it:

- **A fresh clone needs `git submodule update --init library/solo5`.** Without
  it the build only warns, and `bsdkrun solo5` reports the missing tender —
  but a *stale* `core/src/solo5-bin/solo5-hvt` from an earlier build is still
  embedded, so the failure can be silence rather than an error.
- **Solo5 is pinned twice, deliberately.** The submodule is what `cargo build`
  compiles; the `solo5` flake input (`flake = false`, so solo5 needs no flake
  of its own) is what nix builds, because a flake does not fetch submodules and
  `?submodules=1` on every command is not a thing to ask of users. Bump both:
  `git submodule update --remote library/solo5 && nix flake update solo5`.
  `e2e-solo5` fails if they drift. Under nix the tender is a separate
  derivation copied into `core/src/solo5-bin` by `preBuild`, exactly as
  `pack` is — build.rs then finds no `library/solo5` in crane's cleaned source
  and leaves it alone instead of clobbering it.
- **Sign in `postFixup`, not at link time, under nix.** darwin's fixupPhase
  re-signs everything in `$out` ad-hoc and silently drops entitlements, so a
  tender signed by its own makefile installs fine and dies at `hv_vm_create`.
  The `bsdkrun` package already had this comment; the tender needed it too.
- **The tender must stay signed on macOS.** Hypervisor.framework refuses a VM
  to a process without `com.apple.security.hypervisor`, and an entitlement only
  counts inside a signature. It is signed at link time and re-signed on
  extraction. An entitled `bsdkrun` can only exec a validly signed child —
  the same trap `pack` documents, one severity worse: unsigned is SIGKILL
  before `main()`, un-entitled fails later at `hv_vm_create` and reads like a
  hypervisor bug.
- **Kill the tender with us.** `tty::track_child` hands its pid to the signal
  handler; without it Ctrl-C reaps bsdkrun and leaves a VM holding its ports
  that nothing knows the id of.
- **Devices come from the binary.** Every unikernel declares its net/block
  devices in an `MFT1` ELF note, and the tender will not boot with one
  unattached. `solo5.rs` parses it, so no device flags are needed. Note the
  four bytes of padding between the ELF descriptor and `struct mft`
  (`MFT1_NOTE_ALIGN`) — reading the descriptor directly yields a manifest
  shifted by one `uint32_t`, which parses into plausible nonsense.
- **Pin the guest MAC.** The tender generates a random one per boot; gvproxy
  then leases `.3`, `.4`, … and `--port`, which forwards to the fixed guest
  address, points at nobody. `--net-mac:NAME=` fixes it.
- **DHCP, not flags.** A MirageOS `generic_stackv4v6` reads a `--dhcp` runtime
  key defaulting to *false*, so it comes up on mirage's built-in 10.0.0.2 and
  answers nothing. Pin it at configure time (`~dhcp_key:(Key.pure true)`)
  rather than passing `--ipv4=`/`--ipv4-gateway=` on every boot.

## Verifying

A guest that boots is not a guest that works. **Curl it and read the body.** Two
traps met here: a stale VM still holding the port answered for a guest that never
started, and port 8080 on this machine belongs to an unrelated service — use a
free host port (`--port 18080:8080`) and kill leftover VMs first.

## `--strace` has a trap

A guest that dies under `--strace` with a data abort may be reporting the
*tracer's* bug, not the application's. Symbolicate before believing it:

```sh
llvm-addr2line -f -C -e .unikraft/build/<name>_fc-<arch>.dbg 0x<ELR_EL1>
```

`.NET` traces end in `memchr` (`lib/nolibc/string.c`) called from `vsnprintf`
(`lib/nolibc/stdio.c`) — Unikraft formatting a `%s` syscall argument from a
pointer it cannot read. The trace stops at the same line every time, which
looks exactly like the application failing at a fixed point and is not.

## Style

- Markdown tables are padded so the pipes line up.
- Comments explain *why*, especially where the reason is a guest constraint that
  the code cannot show.
- Commit messages state what is verified and what is not, and on which
  architecture.
