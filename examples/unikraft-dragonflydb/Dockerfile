# The same tag upstream pins; the image is published for both amd64 and arm64
# (the binary inside is dragonfly-x86_64 or dragonfly-aarch64, behind a
# symlink that cp -L resolves).
FROM docker.dragonflydb.io/dragonflydb/dragonfly:v1.14.1 AS build

# The image drops to a non-root user for serving; assembling /rootfs needs
# root again. The guest has exactly one user anyway (the Kraftfile's
# CONFIG_LIBPOSIX_USER_* say root).
USER root

# Assemble the rootfs by *resolving* dragonfly's dependencies rather than
# listing them. Upstream's Dockerfile hardcodes /lib/x86_64-linux-gnu/...
# paths, which do not exist on arm64; asking ldd keeps this correct on both.
#
# Nothing else from upstream's rootfs is wanted: it ships bash, mount and
# their libraries solely so a wrapper script can mount cgroup2 before starting
# the server. The guest has no shell, no mount(8) and no cgroups -- the flags
# the wrapper exists to support are passed on the command line instead (see
# the Kraftfile's cmd).
# The /proc and /sys entries are plain files. Dragonfly aborts at startup if
# it cannot read /proc/self/cgroup ("Check failed: cg.has_value()") -- that is
# what upstream's wrapper mounts cgroup2 for. Unikraft has no procfs or
# sysfs, which means nothing shadows these paths either: the ramfs the rootfs
# is unpacked into serves whatever is at /proc/self/cgroup like any other
# file. So the image carries the minimal cgroup2 surface dragonfly reads --
# a v2 /proc/self/cgroup ("0::/") and unlimited memory/cpu at the
# corresponding /sys/fs/cgroup paths.
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/data /rootfs/tmp \
             /rootfs/proc/self /rootfs/sys/fs/cgroup; \
    printf '0::/\n' > /rootfs/proc/self/cgroup; \
    printf 'max\n' > /rootfs/sys/fs/cgroup/memory.max; \
    printf 'max 100000\n' > /rootfs/sys/fs/cgroup/cpu.max; \
    printf '0\n' > /rootfs/sys/fs/cgroup/cpuset.cpus; \
    printf '0\n' > /rootfs/sys/fs/cgroup/cpuset.cpus.effective; \
    cp -L /usr/local/bin/dragonfly /rootfs/usr/bin/dragonfly; \
    ldd /usr/local/bin/dragonfly \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    chmod 1777 /rootfs/tmp

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile).
FROM scratch

COPY --from=build /rootfs/ /
