# unikraft-clojure

A [Clojure](https://clojure.org/) HTTP server on OpenJDK 21, running as a
Unikraft unikernel. Ported from [`unikraft-cloud/examples`'
`httpserver-java21`](https://github.com/unikraft-cloud/examples/tree/main/httpserver-java21)
to build for **arm64**, boot under bsdkrun, and run Clojure on top of the JVM
rather than a bare `SimpleHttpServer.java`.

```sh
./build.sh                    # host arch; or: ./build.sh x86_64
bsdkrun unikraft . --port 3000:3000 \
  --cmdline "elfloader -- /opt/jre/bin/java -XX:+UseSerialGC -XX:ActiveProcessorCount=1 -XX:-UseContainerSupport -XX:-UsePerfData -XX:TieredStopAtLevel=1 -Xmx256m -jar /usr/src/server.jar"
```

## Status

**arm64 works end to end**, verified on macOS/Hypervisor.framework: the
unikernel boots, DHCPs an address, the JVM starts, Clojure's runtime
initialises, and the server answers over a forwarded port about **2 seconds**
after `bsdkrun` is invoked — in the **default 512 MiB**.

```console
$ curl http://127.0.0.1:3000/
Hello from Clojure on Unikraft!
$ curl http://127.0.0.1:3000/info
{"runtime":"clojure","clojure":"1.12.5","java":"21.0.11","vm":"OpenJDK 64-Bit Server VM"}
```

**x86_64 builds**, and is booted by
`.github/workflows/e2e-unikraft-examples.yml`. That job is `strict: false`
until it passes once — no x86_64 host was available here, so CI is the test.

The loader is [app-elfloader-rs](https://github.com/tsirysndr/app-elfloader), a
Rust rewrite of upstream `app-elfloader`; the Kraftfile pulls it like any other
library. Set `ELFLOADER_RS=/path/to/checkout` to build a working copy instead.

## What the JVM needed

Two of them cost a boot each, and both are set in the Kraftfile rather than the
image.

**`$ORIGIN` does not resolve under the ELF loader.** `java` links against
`libjli.so` with an rpath of `$ORIGIN/../lib`, expanded from the path the
*loader* believes the executable has. Under app-elfloader it comes out empty,
and the guest dies before a single JVM instruction runs:

```
java: error while loading shared libraries: libjli.so: cannot open shared object file
```

Naming the directories in `LD_LIBRARY_PATH` sidesteps `$ORIGIN` entirely.

**`lib/server` has to come first in that path** — which looks like a cosmetic
ordering choice and is not. The launcher's `RequiresSetenv()`
([`java_md.c`](https://github.com/openjdk/jdk21u/blob/master/src/java.base/unix/native/libjli/java_md.c))
returns early only if `LD_LIBRARY_PATH` *begins with* the directory holding
`libjvm.so`. Otherwise it concludes the variable needs fixing and fixes it by
re-`exec`ing itself — `execve(execname, ...)`, where `execname` came from
`readlink("/proc/self/exe")`. There is no procfs here, so `execname` is `NULL`
and the guest panics:

```
CRIT: [libposix_process] <execve.c @ 102>  Assertion failure: pathname
```

Writing the path in the order the launcher would have written it means it never
tries. (The JVM still finds its home without procfs: JDK 21's `GetJREPath`
falls back to `dladdr()` on a JLI symbol.)

## The JVM flags are not optional either

A unikernel is one address space with one vCPU, no procfs and no sysfs, so
every default HotSpot would normally derive from the machine it is running on
has to be stated:

| flag                         | why                                                                                                                 |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `-XX:+UseSerialGC`           | G1 sizes its region tables and starts worker and refinement threads from the CPU count; serial is one thread.          |
| `-XX:ActiveProcessorCount=1` | the syscall shim's `sysconf(_SC_NPROCESSORS_ONLN)` is not an answer worth trusting.                                   |
| `-XX:-UseContainerSupport`   | stops the JVM reading `/proc/self/cgroup` and friends to size the heap. There is no procfs.                           |
| `-XX:-UsePerfData`           | stops it `mmap`ing an `hsperfdata` file for a `jstat` that will never attach.                                         |
| `-XX:TieredStopAtLevel=1`    | C1 only. C2 buys peak throughput after tens of thousands of iterations, and costs compiler threads and startup time.   |
| `-Xmx256m`                   | the default maximum heap is a fraction of physical memory — here, of the whole guest.                                 |

## Differences from upstream

**No `runtime: base-compat:latest`.** Upstream pulls a prebuilt kernel; it is
published for x86_64 only, and `base-compat` has no Kraftfile in
`unikraft/catalog` to copy. This Kraftfile builds the runtime from source
instead — `library/base`'s configuration plus the arm64 fixes described in
`../../library/unikraft-base/README.md`, exactly as in `../unikraft-expressjs`.
HotSpot needed nothing beyond it besides the environment above.

**A `jlink`ed runtime instead of a copied JDK.** Upstream copies
`/usr/lib/jvm/java-21-openjdk-amd64/` wholesale — about 180 MiB of JRE, most of
it modules a two-route HTTP server never loads. `jlink` builds a runtime from
the four modules this application actually resolves, and the whole root
filesystem comes to **37 MiB**. That matters more here than on Unikraft Cloud:
the rootfs is embedded in the kernel image *and* unpacked into a RAM filesystem
at boot, so it is resident twice. It is the reason this example runs in 512 MiB
where `../unikraft-expressjs` needs 2048.

**The Dockerfile resolves its libraries instead of listing them.** Upstream
hardcodes `/lib/x86_64-linux-gnu/libc.so.6` and the rest; on arm64 both the
directory and the dynamic loader's filename differ, so no substitution fixes
that in place. Asking `ldd` keeps it correct on both architectures.

**The jar is built once, for no architecture.** Class files are bytecode, so
the build stage runs on `$BUILDPLATFORM` and only the runtime stage is pulled
for `$TARGETPLATFORM`. Cross-building the application under emulation would buy
nothing.

## AOT matters more than usual

`build.clj` compiles the `server` namespace ahead of time with
`:direct-linking true`. Without it the JVM loads `server.clj` as source and
compiles it — and everything it requires — on every boot, which is most of what
people mean when they say Clojure starts slowly. In a unikernel, where the
point is that the guest is serving in about the time `docker run` would take,
paying that on every start would swamp everything else.

## `jlink` module sets are decided by your dependencies

The module list is `java.base`, `jdk.httpserver`, `java.logging` — and
`java.sql`, which is there for no database. `clojure.data.json` extends its
writer protocol to `java.sql.Time`, `Date` and `Timestamp`, and resolves those
classes when the namespace loads, so leaving the module out produces
`ClassNotFoundException: java.sql.Date` at startup rather than a missing
feature. That is the usual shape of a `jlink` mistake, and the reason to run
the root filesystem under Docker before building a kernel around it:

```sh
docker buildx build --platform linux/arm64 --provenance=false --load -t clojure-rootfs:arm64 .
docker run --rm -p 3000:3000 clojure-rootfs:arm64 /opt/jre/bin/java -jar /usr/src/server.jar
```

## Editing the Kraftfile's kconfig

`kraft` will not overwrite a symbol that is already in a generated `.config`,
so changing one of the `CONFIG_*` values here has no effect on a rebuild until
the stale file is gone:

```sh
rm -f .config* && SKIP_FETCH=1 ./build.sh
```

Without the `rm` the kernel builds cleanly and boots with the *old* value,
which looks exactly like the change not working.

## `--cmdline` is required

bsdkrun does not read the Kraftfile's `cmd` for a locally-built kernel, so the
program to run has to be given explicitly. The format is

```
<argv0> -- <application argv>
```

Everything before `--` is parsed as kernel library parameters and **the first
word is skipped** (Unikraft treats it as the program name), so the leading
placeholder is not optional — dropping it silently feeds your first parameter
to the parser as `argv[0]`.
