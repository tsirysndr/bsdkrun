# Scala on Unikraft

A Scala 3 HTTP service on a **jlink'd JRE**, built with sbt.

Detected by `build.sbt`. Gradle and Maven builds of Scala go through the Java
provider instead — what decides the build is the tool, not the language.

The project needs **sbt-assembly** (see `project/plugins.sbt`). `sbt package`
alone emits a jar holding this project's classes and nothing else — not even the
Scala standard library — and a guest running it dies on the first Scala symbol it
touches. The provider tries `assembly` first so that missing plugin explains
itself here rather than as a `NoClassDefFoundError` in the guest.

sbt comes from a pinned release tarball rather than an image tag: the official
sbt images encode JDK, sbt and Scala versions in a single tag, so any of the
three moving breaks the reference.

There is no `Dockerfile`, no `Kraftfile` and no `build.sh` — `bsdkrun pack`
detects the project and generates all three internally.

## Build

```sh
bsdkrun pack .
```

## Run

```sh
bsdkrun unikraft . --cmdline "scala -- /opt/jre/bin/java -XX:+UseSerialGC -XX:ActiveProcessorCount=1 -XX:-UseContainerSupport -XX:-UsePerfData -XX:TieredStopAtLevel=1 -Xmx256m -jar /usr/src/server.jar"
```

`pack` prints that command when it finishes.

## Try it

```sh
curl http://<vm-ip>:8080/
```

## Publish it

```sh
bsdkrun pack . --push ghcr.io/you/scala:v1
bsdkrun unikraft ghcr.io/you/scala:v1
```

The second command needs no copy of this directory: the kernel is pulled on
first use and cached, and the argv comes from the image.
