# osv-redis

**Redis 7.0.15, unmodified, as an OSv unikernel** — booting in ~5 ms on
macOS/Apple Silicon and answering from the host over a forwarded port.

```
$ redis-cli -p 6379 ping
PONG
$ redis-cli -p 6379 set osv works
OK
$ redis-cli -p 6379 get osv
"works"
```

Nothing is compiled: Debian's arm64 `redis-server` is already position
independent, which is all OSv's loader requires, so the stock distro binary runs
as-is. `build.sh` collects it plus the libraries OSv doesn't provide, and hands
the lot to capstan.

```sh
./build.sh
qemu-img convert -O raw ~/.capstan/repository/osv-redis/osv-redis.qemu redis.raw
bsdkrun osv redis.raw --cmdline '/redis-server /redis.conf' --port 6379:6379
```

## Why Redis, and not Node or Postgres

Redis is close to an ideal OSv guest, and the reasons it fits are exactly the
reasons most servers don't:

* **It's a PIE.** OSv runs the application in the single address space it
  shares with the kernel, so the binary must be relocatable — a shared object
  or a PIE. A position-dependent binary (`ET_EXEC`) has nowhere to go and the
  guest hangs with nothing on the console. Debian builds PIE by default, which
  is why this works; Node's official arm64 tarball is `ET_EXEC`, which is why
  it does not.
* **It's one process.** OSv has no `fork()`. Redis serves entirely from its
  main process, so the only thing lost is `BGSAVE` — hence `save ""` in
  `redis.conf`, which turns a runtime failure into a configuration choice.
  PostgreSQL and MariaDB are multi-process by design and cannot be made to fit.
* **It asks little of the kernel.** Runtimes with their own memory managers or
  JITs lean on syscalls OSv lacks — Deno dies at load with `failed looking up
  symbol mremap`.

## Every library has to be in the image

There is no package manager in a unikernel and no second process to run one, so
whatever `redis-server` links against must be in the image before boot.
`build.sh` reads that list from `ldd` and copies each one — except the glibc set
(`libc`, `libm`, `libpthread`, `libdl`, `librt`), which OSv implements itself
and exports from the kernel. Shipping Debian's copies would shadow OSv's own and
break the guest.

That comes to about 13 MB: OpenSSL, jemalloc, libstdc++, lz4/zstd/lzma, and a
few smaller ones.

## Things you will see

**`sbrk() stubbed`** at startup. Redis probes it; jemalloc uses `mmap` instead,
so nothing depends on the answer.

**`unhandled InterruptID irq=0x20`.** libkrun gives its legacy devices the first
SPIs — serial 32, RTC 33, GPIO 34, with virtio starting at 35 — so IRQ 32 is the
PL011 serial that OSv prints through but never registers a handler for.

**No `earlycon=` in the command line.** OSv passes everything after the
application path to the application as `argv`, so libkrun's `earlycon=` hint
would arrive as a stray argument — enough that `redis-server` aborts with
`FATAL CONFIG FILE ERROR ... 'earlycon=pl011,mmio32,0x0a001000'`, a directive
nobody wrote. `bsdkrun osv` sets `KRUN_NO_EARLYCON=1` to leave it out; the
console is unaffected.
