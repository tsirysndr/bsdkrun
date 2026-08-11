# unikraft-elixir

An Elixir HTTP server -- [Plug](https://hexdocs.pm/plug/) on
[Cowboy](https://ninenines.eu/docs/en/cowboy/), running on the BEAM -- as a
Unikraft unikernel. Ported from [`unikraft-cloud/examples`'
`httpserver-elixir1.16`](https://github.com/unikraft-cloud/examples/tree/main/httpserver-elixir1.16)
to build for **both architectures** and boot under bsdkrun.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --mem 1024 --port 3000:3000 --cmdline \
  "elfloader -- /erl/erts/bin/beam.smp -fnu -- -root /erl -bindir /erl/erts/bin \
   -progname erl -- -home /root -- -noshell -mode embedded \
   -config /srv/releases/0.1.0/sys -boot /srv/releases/0.1.0/start \
   -boot_var RELEASE_LIB /srv/lib -- -extra"
```

## Status

**arm64 works**, verified on macOS/Hypervisor.framework:

```console
$ curl localhost:3000/
Hello from Elixir on Unikraft!
$ curl localhost:3000/info
{"runtime":"elixir","otp":"26","schedulers":1,"elixir":"1.16.2","erts":"14.2.5"}
$ curl -o /dev/null -w '%{http_code}\n' localhost:3000/nope
404
```

x86_64 has never been run; `.github/workflows/e2e-unikraft-examples.yml` builds
and boots it, non-strict, and that job is the test.

Getting there needed one fix in Unikraft, applied by
[`../../library/unikraft-base/patches/apply.sh`](../../library/unikraft-base/patches/apply.sh):

| #  | bug                                                                                                                                                                                                                                                                                                    |
|----|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 17 | **A leading zero-length `iovec` entry failed the whole `writev()` with `EIO`.** POSIX says an empty entry contributes nothing and Linux skips it; lwip walks the vectors unfiltered and `tcp_write()` rejects the `NULL` pointer with `ERR_ARG` *before* its own `len == 0` shortcut. `err_to_errno()` maps that to `EIO`. |

The shape that trips it is structural to Erlang, not incidental: `tcp_sendv()`
in `erts/emulator/drivers/common/inet_drv.c` reserves `ev->iov[0]` for the
packet-length header and fills it in only `if (h_len > 0)`, then passes the
whole vector to `writev()`. An HTTP server's socket is `{packet, raw}`, so
`h_len` is 0 and `iov[0]` stays `{NULL, 0}`.

The symptom is not obviously a write failure. The boot completes, the listener
comes up, the connection is accepted and the request is read — and then the
response never appears:

```
recvfrom(fd, "GET / HTTP/1.1\r\nHost: 12"..., 1460, ...) = 78
writev(...)                                               = Input/output error (-5)
close(fd)                                                 = OK
```

`curl` reports `Empty reply from server`. That trace is why
`CONFIG_LIBSYSCALL_SHIM_STRACE` is worth the noise: nothing else in the guest
says a word about it.

## Architecture

Upstream's Dockerfile names its shared libraries by hand:

```dockerfile
COPY --from=runtime /lib/x86_64-linux-gnu/libc.so.6 \
                    ...
COPY --from=runtime /lib64/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
```

None of that can be patched into an arm64 image: the directory is
`aarch64-linux-gnu`, and the dynamic loader is `/lib/ld-linux-aarch64.so.1` --
a different path *and* a different filename. This example asks `ldd` instead,
over the emulator, the helper programs and the NIFs, so the answer is whatever
the target architecture actually needs. That is the same approach the ExpressJS
and Actix examples here take.

Elixir also lets the build be split, which the other examples cannot do:

| stage     | platform         | produces                                         |
|-----------|------------------|--------------------------------------------------|
| `build`   | `$BUILDPLATFORM` | the mix release -- `.beam` bytecode, no binaries |
| `runtime` | `$TARGETPLATFORM`| ERTS, the OTP applications, the shared libraries |

Erlang bytecode is architecture-independent, so compiling it under emulation
would be pure waste; `include_erts: false` in `mix.exs` is what guarantees no
host-arch binary can ride along in the release. Building the x86_64 image on an
Apple Silicon machine therefore only emulates a handful of `cp` and `ldd`
invocations.

## No shell, so no `wrapper.sh`

Upstream starts the server through four shell scripts:

```
wrapper.sh -> _build/dev/rel/server/bin/server start -> releases/0.1.0/elixir -> erl -> erlexec -> beam.smp
```

A unikernel runs exactly one program and has no shell, so that chain has to be
collapsed into the single `execve` at the end of it. The way to find out what
that `execve` looks like is to ask, rather than to reconstruct it from
`erlexec`'s source -- replace the emulator with a script that prints its own
arguments:

```sh
B=$(echo /usr/local/lib/erlang/erts-*/bin)
mv "$B/beam.smp" "$B/beam.smp.real"
printf '#!/bin/sh\nfor a; do echo "$a"; done\n' > "$B/beam.smp"
chmod +x "$B/beam.smp"
_build/prod/rel/server/bin/server start
```

The `cmd:` in the Kraftfile is that vector, with three deliberate changes:

* **`-sname server` and `-setcookie ...` are dropped.** Distribution makes the
  VM start `epmd` as a *child process*, and there is no `fork()` here (see
  below).
* **`-s elixir start_cli` and `--no-halt` are dropped.** `start_cli` implements
  the `elixir` command's semantics, including "run every plain argument as a
  script". The release's applications are started by `start.boot` regardless,
  and dropping it is also what makes the trailing junk on the command line
  harmless -- see the next section.
* **`-fnu` is added**, which is `+fnu` as `erlexec` rewrites it. The image
  ships no locale data, so `setlocale()` fails, the VM settles on latin1 and
  warns on every boot that Elixir may malfunction.

Four environment variables come with it. `erlexec` exports `ROOTDIR`,
`BINDIR`, `EMU` and `PROGNAME` before exec'ing the emulator, and `BINDIR` is
not optional -- without it beam.smp prints

```
Environment variable BINDIR is not set
```

and exits before it opens a single file. A unikernel has no shell to export
them from, so they are compiled in through `CONFIG_LIBPOSIX_ENVIRON_ENVP4..7`
in the Kraftfile.

## `--cmdline` is required, and it must end in `-extra`

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel, so the
program to run has to be given explicitly. The format is

```
<argv0> -- <application argv>
```

and everything before `--` is parsed as kernel library parameters, with the
first word skipped -- see the [ExpressJS
README](../unikraft-expressjs/README.md) for why the placeholder is not
optional.

What is specific to this example is the *end* of the command line. libkrun
appends its own hints (`earlycon=`, `virtio_mmio.device=`) after the string you
pass, and they arrive as extra `argv` entries. The BEAM's own argument parser
would fold them into whichever flag came last, so the vector ends with `-extra`
and nothing after it: everything from there on lands in
`init:get_plain_arguments/0`, which nothing in this release reads.

Leaving `-s elixir start_cli` in place instead produces the giveaway:

```
No file named earlycon=pl011,mmio32,0x0a001000
```

-- Elixir's CLI faithfully trying to run libkrun's console hint as a script.

## What ships in the image

`mix release` writes down which OTP applications the boot script resolves under
`$ROOT`, so the Dockerfile reads them out of `start.script` rather than
guessing:

```
$ROOT/lib/{asn1,compiler,crypto,kernel,public_key,sasl,ssl,stdlib}/ebin
```

Everything else in `/usr/local/lib/erlang/lib` -- `wx`, `megaco`, `dialyzer`,
`common_test` and the rest of the 140 MiB -- is left behind, along with every
`src`, `doc`, `examples`, `man` and `include` directory. From `erts-*/bin` only
`beam.smp`, `erl_child_setup` and `inet_gethost` are kept; nothing else there
is reachable from a release that neither compiles code nor uses distribution.
The release's own applications (elixir, plug, cowboy, jason, ...) come from
`$RELEASE_LIB`, which is the release directory at `/srv`.

The result is a 68 MiB root filesystem, against about 250 MiB for a naive copy.
That matters more than usual: the filesystem is embedded in the kernel image
*and* unpacked into a RAM filesystem at boot, so it is resident twice before
the BEAM allocates anything.

## `fork()` is not available

Unikraft has a single address space, so `clone()` without `CLONE_VM` returns
`-ENOTSUP`:

```
ERR: [libposix_process] <clone.c @ 216> CLONE_VM not set: Multiple address spaces are not supported
```

ERTS calls `fork()` once at startup, in `forker_start()`
(`erts/emulator/sys/unix/sys_drivers.c`), to hand off process spawning to a
helper called `erl_child_setup`. Any Erlang *port* that runs an external
program -- `os:cmd/1`, `open_port({spawn, ...})`, `epmd`, `inet_gethost` for
DNS -- goes through it.

This one line above is what that costs here, and it is worth knowing what it
does *not* cost: ERTS never checks the return value of that `fork()`, so the
failure does not stop the boot. A release that serves HTTP and shells out to
nothing never asks the forker to do anything.
