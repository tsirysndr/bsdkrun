# mongo:6.0 is published for both amd64 and arm64. (Note the CPU floors:
# MongoDB 5.0+ requires AVX on x86_64 and ARMv8.2-A on arm64. GitHub's
# runners and Apple Silicon both qualify; an older Xeon may not.)
FROM mongo:6.0 AS build

# A statically linked shell, for argv hygiene rather than for scripting:
# libkrun appends its own words (earlycon=..., tsi_hijack, a bare `--`) to
# the end of the kernel command line, past the `--` stop sequence, so they
# arrive in the application's argv. mongod's option parser rejects positional
# arguments it does not recognise, so it is booted as `sh /start.sh` instead:
# the junk becomes the script's positional parameters and the script exec()s
# mongod with a clean argv -- an execve(), not a fork
# (CONFIG_APPELFLOADER_MULTIPROCESS in the Kraftfile). Same pattern as
# ../unikraft-redis.
FROM busybox:stable-musl AS shell

# Assemble the rootfs by *resolving* mongod's dependencies rather than listing
# them. Upstream's Dockerfile enumerates 36 /lib/x86_64-linux-gnu/... paths,
# which do not exist on arm64; asking ldd keeps this correct on both and picks
# up anything a future mongod starts needing.
#
# /data/db ships empty: WiredTiger initialises it on first boot. Unlike the
# mysql/mariadb examples there is nothing to bake at build time -- mongod has
# no bootstrap SQL to run, and a fresh WiredTiger datadir is created by the
# same single process that then serves from it.
FROM build AS rootfs
COPY --from=shell /bin/busybox /bin/busybox.static
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/bin /rootfs/data/db /rootfs/tmp \
             /rootfs/etc; \
    cp /bin/busybox.static /rootfs/bin/busybox; \
    ln -s busybox /rootfs/bin/sh; \
    cp /usr/bin/mongod /rootfs/usr/bin/mongod; \
    ldd /usr/bin/mongod \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    cp -L /etc/localtime /rootfs/etc/localtime; \
    chmod 1777 /rootfs/tmp

COPY start.sh /rootfs/start.sh

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile).
FROM scratch

COPY --from=rootfs /rootfs/ /
