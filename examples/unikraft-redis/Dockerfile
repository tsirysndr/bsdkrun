# The Debian-based image rather than alpine, to match the glibc the other
# examples ship; redis:7.2-bookworm is published for both amd64 and arm64.
FROM redis:7.2-bookworm AS build

# A shell, but not for an entrypoint script's process tree -- for argv
# hygiene. libkrun appends its own words (earlycon=..., tsi_hijack, a bare
# `--`) to the end of the kernel command line, past the `--` stop sequence, so
# they arrive in the application's argv; see
# ../../library/unikraft-base/README.md. redis-server parses everything after
# its config-file argument as configuration directives and dies on the first
# one: "*** FATAL CONFIG FILE ERROR ... '\"earlycon=pl011,mmio32,0x0a001000\"'
# Bad directive". Booting `sh /start.sh` instead makes the junk the script's
# positional parameters, which nothing reads, and the script `exec`s
# redis-server with a clean argv -- an execve(), not a fork, which Unikraft
# supports (CONFIG_APPELFLOADER_MULTIPROCESS in the Kraftfile).
#
# The musl variant of the official busybox image is statically linked, so it
# needs no loader or libc of its own in either architecture's rootfs.
FROM busybox:stable-musl AS shell

# Assemble the rootfs by *resolving* redis-server's dependencies rather than
# listing them. Upstream's Dockerfile hardcodes /lib/x86_64-linux-gnu/... and
# /lib64/ld-linux-x86-64.so.2, which is exactly the part that cannot survive a
# change of architecture; asking ldd keeps this correct on both and picks up
# anything a future redis build starts needing.
FROM build AS rootfs
COPY --from=shell /bin/busybox /bin/busybox.static
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/bin /rootfs/etc/redis /rootfs/data \
             /rootfs/tmp; \
    cp /bin/busybox.static /rootfs/bin/busybox; \
    ln -s busybox /rootfs/bin/sh; \
    cp /usr/local/bin/redis-server /rootfs/usr/bin/redis-server; \
    ldd /usr/local/bin/redis-server \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    chmod 1777 /rootfs/tmp

# The server's configuration lives in the image rather than on the command
# line; everything in it is a unikernel constraint, see the comments there.
COPY redis.conf /rootfs/etc/redis/redis.conf
COPY start.sh /rootfs/start.sh

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile).
FROM scratch

COPY --from=rootfs /rootfs/ /
