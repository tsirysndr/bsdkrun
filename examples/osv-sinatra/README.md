# osv-sinatra

**Sinatra 4.2.1 on Ruby 3.1.2, served from an OSv unikernel** — booting in ~7 ms
on macOS/Apple Silicon and answering real HTTP from the host.

```
== Sinatra (v4.2.1) has taken the stage on 4567 for development with backup from WEBrick

$ curl 127.0.0.1:4567/
hello from Sinatra on OSv
$ curl 127.0.0.1:4567/info
ruby 3.1.2 on aarch64-linux-gnu
```

`app.rb` is an ordinary Sinatra app — nothing in it knows it is running on a
unikernel.

```sh
./build.sh
qemu-img convert -O raw ~/.capstan/repository/osv-sinatra/osv-sinatra.qemu app.raw
bsdkrun osv app.raw --cmdline '/ruby /app.rb' --port 4567:4567
```

## What it takes to get a scripting runtime onto OSv

Ruby is the interesting case: unlike Node, Deno and Bun it *does* run, but only
after four distinct problems are solved. Each failed loudly and specifically,
which is what made them tractable.

**1. The binary has to be relocatable.** OSv runs the application in the single
address space it shares with the kernel, so it loads a shared object or a PIE.
Debian builds PIE by default, so its `ruby` works as shipped — while Node's and
Bun's official arm64 binaries are `ET_EXEC` and cannot be loaded at all.

**2. Missing libc symbols, resolved with a shim.** OSv's libc is broadly
glibc-compatible, but not complete:

```
/usr/lib/libgmp.so.10: failed looking up symbol obstack_vprintf
/usr/lib/libruby-3.1.so.3.1: failed looking up symbol timer_create
```

Neither is Ruby's own code — `libgmp` wants a GNU obstack helper it only uses
for `gmp_obstack_printf`, and Ruby wants POSIX timers. `osv-shim.c` defines both
and `build.sh` adds it to the importers' `DT_NEEDED` with `patchelf`, so OSv's
loader resolves them there. Stubbing is sound in both cases: the obstack helpers
are never called, and Ruby *already* has a fallback for platforms without POSIX
timers — you see it take that path at boot:

```
warning: timer_create failed: Not supported, signals racy
```

**3. Every library has to be in the image, including the ones nothing names.**
There is no package manager in a unikernel and no second process to run one.
Walking `ldd` on the `ruby` binary is not enough, because each native extension
carries its own dependencies — miss `libcrypto` and Sinatra dies loading Ruby's
OpenSSL extension with `failed looking up symbol i2d_TS_TST_INFO`. `build.sh`
walks every `*.so` under the extension directory as well.

**4. Debian splits the stdlib in two.** The pure-Ruby half lives in
`/usr/lib/ruby`, the arch-specific half — `rbconfig.rb` and the compiled
extensions — in `/usr/lib/<triple>/ruby`. Ship only the first and Ruby starts
and then fails at `cannot load such file -- rbconfig`.

## WEBrick, not puma

WEBrick is pure Ruby. Puma and thin ship native extensions, each of which would
need its own libraries collected into the image — the problem in point 3 again,
for no benefit here.

## Things you will see

**`timer_create failed: Not supported, signals racy`** — point 2 above. Ruby is
telling you it took the fallback path.

**`unhandled InterruptID irq=0x20`** — libkrun gives its legacy devices the
first SPIs (serial 32, RTC 33, GPIO 34; virtio starts at 35), so IRQ 32 is the
PL011 serial that OSv prints through but never registers a handler for.

**A slow `capstan package compose`.** The image carries a few thousand stdlib
files and capstan's ROFS writer is not fast; expect a couple of minutes.
