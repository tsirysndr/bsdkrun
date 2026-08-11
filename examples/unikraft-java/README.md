# Java on Unikraft

A Java HTTP service on a **jlink'd JRE**, built with Maven.

Detected by a `pom.*` (any of Maven's polyglot dialects) or a `gradlew`
wrapper. Gradle and Maven builds of Kotlin, Groovy or Scala come through here
too — at this level what matters is the build tool, not the language. (sbt has
its own provider; see `../unikraft-scala`.)

The runtime is jlinked rather than shipped whole: a full JRE would be far too
large for an image that is resident twice at boot. The module set comes from
`jdeps`, unioned with a broad fallback — `jdeps` sees only what the bytecode
references, so anything reached by reflection (a JDBC driver, a logging backend,
a crypto provider) is invisible to it, and the failure mode for guessing low is
a `ClassNotFoundException` in a guest that cannot be fixed without a rebuild.

The JVM flags are load-bearing, not tuning. The guest is a single CPU with no
cgroup to read limits from and no perf-data file to write; left to itself the
JVM sizes a GC thread pool from a CPU count it cannot determine and dies before
`main()`.

There is no `Dockerfile`, no `Kraftfile` and no `build.sh` — `bsdkrun pack`
detects the project and generates all three internally.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "java -- /opt/jre/bin/java -XX:+UseSerialGC -XX:ActiveProcessorCount=1 -XX:-UseContainerSupport -XX:-UsePerfData -XX:TieredStopAtLevel=1 -Xmx256m -jar /usr/src/server.jar"
```

`pack` prints that command when it finishes.

## Try it

```sh
curl http://<vm-ip>:8080/
```

## Publish it

```sh
bsdkrun pack . --push ghcr.io/you/java:v1
bsdkrun unikraft ghcr.io/you/java:v1
```

The second command needs no copy of this directory: the kernel is pulled on
first use and cached, and the argv comes from the image.
