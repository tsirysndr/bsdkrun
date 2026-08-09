# Why mysqld hangs: `IB_thread::start()` versus a cooperative scheduler

The guest wedges during InnoDB startup and never issues another system call.
This is how that was pinned down, and what it turned out to be.

## Root cause (found)

InnoDB starts a background thread and then **busy-waits for it with no yield
and no system call**:

```cpp
// storage/innobase/include/os0thread-create.h
void IB_thread::start() {
  ut_a(state() == State::NOT_STARTED);
  m_state->store(State::ALLOWED_TO_START);
  wait(State::STARTED);            // <- spins until the child publishes STARTED
}
```

which compiles to a six-instruction loop:

```
_ZN9IB_thread5startEv+32:  mov  w2, #0x2         // ALLOWED_TO_START
                     +36:  stlr w2, [x1]         // publish it
                     +48:  ldar w1, [x1]         ┐
                     +52:  cmp  w1, #0x2         │ spin while the state is
                     +56:  b.ne +96              │ still ALLOWED_TO_START
                     +60:  isb                   │ (the only relax hint)
                     +64:  ldr  x1, [x0, #16]    │
                     +68:  cbnz x1, +48          ┘
```

There is no `bl` in that loop: it never enters the kernel.

Unikraft's scheduler is non-preemptive. `schedcoop_thread_add()` appends a new
thread to the tail of the run queue and returns to the caller, and
`lib/ukschedcoop/schedcoop.c` says so itself — *"The scheduler is non-preemptive
(cooperative), and schedules according to Round Robin algorithm."* A thread is
descheduled only when it yields or blocks.

So the two halves deadlock:

* the parent spins waiting for the child to store `STARTED`,
* the child cannot run until the parent yields,
* the parent's spin contains nothing that yields.

The timer interrupt fires throughout and changes nothing, because a cooperative
scheduler does not reschedule on it. That is the whole bug. It is not specific
to arm64 — the e2e workflow reproduces it on x86_64 — and not specific to
libkrun, since QEMU reproduces it too.

## How it was found

1. **A syscall trace** narrowed it to a `clone` — the guest's last system call,
   after which the child managed one `rt_sigprocmask` and nothing further
   happened. It also showed the *first* `clone` succeeding and doing all of
   InnoDB's tablespace work, so threads were not broken in general.
2. **A `qemu/arm64` target**, because the `fc` platform image cannot drive a gdb
   stub. It reproduces the hang identically, which exonerates libkrun.
3. **`gdb/where.gdb`** — attaching to QEMU's stub halts the vCPU wherever it is,
   so no breakpoint is needed. It showed a PC of `0x156d5b0` cycling over six
   instructions. That address is below 4 GiB and not in the kernel: `mysqld` is
   a non-PIE executable, so it lives at its link address and its own symbols
   resolve directly (`info symbol` against `.rootfs-arm64/usr/sbin/mysqld`),
   giving `IB_thread::start()+64`, with `LR` in `buf_pool_init()`.
4. **`gdb/poke-started.gdb`** — the confirmation. Writing `STARTED` into the
   state word by hand, which is exactly the store the child would have made,
   released the parent immediately.

Step 4 also proved the child was runnable all along rather than lost. Once the
parent stopped spinning, the child ran at once — and found the state forged:

```
[ERROR] [MY-013183] [InnoDB] Assertion failure:
  os0thread-create.h:185: m_thread.state() == IB_thread::State::ALLOWED_TO_START
```

That assertion lives in the child's own entry path. Its firing is the proof: the
child needed nothing except for the parent to stop hogging the CPU.

## Reproducing

Build the debug target. The fetched and patched tree is reused, so this does not
refetch:

```sh
sed -e 's|^- fc/arm64$|- qemu/arm64|' -e '/^- fc\/x86_64$/d' Kraftfile \
    > .Kraftfile.qemu-arm64
# then, in the Debian container build.sh uses on macOS:
#   kraft build -K .Kraftfile.qemu-arm64 --rootfs .rootfs-arm64
```

Boot it with the stub open. `-S` is deliberately *not* used: the interesting
moment is a minute in, so it is easier to let it wedge and then attach.

```sh
qemu-system-aarch64 -machine virt -accel hvf -cpu host -m 2048 -nographic \
  -kernel .unikraft/build/mysql_qemu-arm64 \
  -append "elfloader -- /usr/sbin/mysqld --user=root" \
  -device virtio-rng-device \
  -netdev user,id=n0 -device virtio-net-device,netdev=n0 \
  -gdb tcp:0.0.0.0:1234
```

Wait for `InnoDB initialization has started`, then attach from a container —
`gdb-multiarch` is not readily available on macOS:

```sh
docker build -t gdbma -<<'EOF'
FROM debian:bookworm
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends gdb-multiarch
EOF

docker run --rm --add-host=host.docker.internal:host-gateway -v "$PWD":/w -w /w gdbma \
  gdb-multiarch -q -batch -x repro/gdb/where.gdb .unikraft/build/mysql_qemu-arm64.dbg
```

Keep gdb scripts short. Every `stepi` is a round trip to the stub, so stepping
a few hundred thousand instructions takes longer than the rest of the session.

## What would fix it

Nothing in the image or in MySQL's configuration; the spin has no knob. The fix
belongs in the guest kernel, and there are two shapes:

* **Let a newly cloned thread run before the cloner returns.** A yield at the
  end of the `clone` path is enough here, because publishing `STARTED` is the
  first thing the child does and it then blocks on an event, handing the CPU
  straight back. This is small, but it only helps spins that wait on something
  a fresh thread does immediately.
* **Preempt.** The general answer, and the one that would also cover a spin on a
  lock held by an already-running thread. Unikraft has no preemptive scheduler
  to switch to — `lib/` contains `ukschedcoop` and nothing else — so this is new
  work rather than a Kconfig change.

Either one is a change to the shared `library/unikraft-base` patch set that
every other Unikraft example here is built against, so it wants its own
before/after run across them.
