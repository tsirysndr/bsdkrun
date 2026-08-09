# The root filesystem for the Clojure unikernel: one uberjar and a Java
# runtime containing only the modules that jar resolves. Nothing else -- no
# shell, no JDK tools, no /etc.
#
# Architecture is handled in two stages, and neither of them hardcodes a path
# (upstream's Java example lists `/lib/x86_64-linux-gnu/...` by hand, which
# cannot be made to work on arm64: the directory *and* the dynamic loader's
# filename both differ):
#
#   1. The build stage runs on $BUILDPLATFORM. Everything it produces is JVM
#      bytecode, which is architecture-independent, so building it under
#      emulation for the target would be pure waste.
#   2. The runtime stage runs on $TARGETPLATFORM and is the *only* source of
#      binaries. It resolves the shared libraries with `ldd` rather than naming
#      them, the same way the other Unikraft examples here do.
ARG JDK_IMAGE=eclipse-temurin:21-jdk

FROM --platform=$BUILDPLATFORM clojure:temurin-21-tools-deps-bookworm-slim AS build

WORKDIR /src
# The tools.deps caches default to $HOME, which is fine, but pinning them keeps
# the layer cacheable regardless of the image's user.
ENV CLJ_CONFIG=/tmp/clj GITLIBS=/tmp/gitlibs

# Dependencies first, in their own layer: editing the server should not
# re-resolve Maven. `-P` prepares (downloads) without running anything.
COPY deps.edn build.clj ./
RUN clojure -P && clojure -P -T:build

COPY src ./src
RUN clojure -T:build uber

FROM --platform=$TARGETPLATFORM ${JDK_IMAGE} AS runtime

# A jlink'ed runtime rather than a stock JRE. The image is resident twice in
# the guest -- once embedded in the kernel, once unpacked into the RAM
# filesystem -- so every megabyte is paid for twice, and a full JRE 21 is about
# 180 MiB against the 37 MiB this produces:
#
#   java.base       the language, collections, sockets; all Clojure itself needs
#   jdk.httpserver  com.sun.net.httpserver, the server
#   java.logging    java.util.logging, which jdk.httpserver logs through
#   java.sql        *not* for a database: clojure.data.json extends its writer
#                   protocol to java.sql.Time/Date/Timestamp, and resolves those
#                   classes when the namespace loads. Leaving it out costs a
#                   ClassNotFoundException at startup, not a missing feature --
#                   which is the general shape of a jlink mistake, and the
#                   reason to test the image before building a kernel with it.
#
# `--compress=zip-6` compresses the jimage; the cost is paid on class load,
# which is cheaper here than carrying the bytes twice through cpio.
RUN jlink \
      --add-modules java.base,jdk.httpserver,java.logging,java.sql \
      --strip-debug --no-man-pages --no-header-files --compress=zip-6 \
      --output /rootfs/opt/jre

COPY --from=build /src/target/server.jar /rootfs/usr/src/server.jar

# java.io.tmpdir has to exist even though nothing writes to it: the JVM stats
# it during startup.
RUN set -eu; \
    mkdir -p /rootfs/tmp; \
    \
    # Resolve what the runtime links against -- glibc, libz, libstdc++,
    # libgcc_s and the dynamic loader, whose path is architecture-specific.
    # Ask every executable and every .so in the jlink output, so a module set
    # that grows later does not need this list revisited. Dependencies that
    # live inside /rootfs are already there (skip them), and unresolved ones
    # print no path at all (skipped by the -f test).
    find /rootfs/opt/jre -type f \( -perm -u+x -o -name '*.so' \) -print \
      | while read -r f; do ldd "$f" 2>/dev/null || true; done \
      | grep -oE '/[^ ()]+' \
      | grep -v '^/rootfs' \
      | sort -u \
      | while read -r lib; do \
            [ -f "$lib" ] || continue; \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by the
# ELF loader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile), so this image
# deliberately ships no /etc.
FROM scratch

COPY --from=runtime /rootfs/ /
