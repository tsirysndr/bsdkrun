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
# A hand-written /proc and /sys, same trick as ../unikraft-dragonflydb:
# Unikraft has no procfs or sysfs, so nothing shadows these paths and the
# ramfs serves whatever the image put there. mongod needs two different
# things from them.
#
# The CPU count, which it takes from std::thread::hardware_concurrency ->
# glibc's __get_nprocs (which reads /sys/devices/system/cpu/online and falls
# back to /proc/stat). With neither file present it detects zero cores and
# dies on its own invariant: "Invariant failure numPartitions > 0,
# src/mongo/db/query/partitioned_cache.h:103". One CPU here, matching the
# single vcpu libkrun gives the guest.
#
# And its own process statistics, from /proc/<pid>/stat and statm
# (processinfo_linux.cpp). Missing, those throw Location13538 -- which the
# FTDC thread catches and logs once a second, but a connection thread does
# not, and an uncaught C++ exception is terminate() and then abort(). That is
# the "Got signal: 6 (Aborted)" with no message of mongod's own that killed
# the server moments after it accepted its first client. The pid is 2: the
# loader's init is 1 and the start.sh shell -- whose pid execve() keeps -- is
# 2. /proc/self is a symlink to it for the paths that spell it that way.
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/bin /rootfs/data/db /rootfs/tmp \
             /rootfs/etc /rootfs/sys/devices/system/cpu /rootfs/proc; \
    printf '0\n' > /rootfs/sys/devices/system/cpu/online; \
    printf 'cpu  0 0 0 0 0 0 0 0 0 0\ncpu0 0 0 0 0 0 0 0 0 0 0\n' \
        > /rootfs/proc/stat; \
    printf 'processor\t: 0\n\n' > /rootfs/proc/cpuinfo; \
    printf 'MemTotal:       2097152 kB\nMemFree:        1048576 kB\nMemAvailable:   1048576 kB\nBuffers:              0 kB\nCached:               0 kB\nSwapTotal:            0 kB\nSwapFree:             0 kB\n' \
        > /rootfs/proc/meminfo; \
    mkdir -p /rootfs/proc/2; \
    ln -s 2 /rootfs/proc/self; \
    printf '2 (mongod) S 1 2 2 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 0 2147483648 65536 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n' \
        > /rootfs/proc/2/stat; \
    printf '524288 65536 0 1 0 0 0\n' > /rootfs/proc/2/statm; \
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
