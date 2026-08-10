# nginx:1.25.3-bookworm is published for both amd64 and arm64; upstream pins
# neither the base image nor a platform, but does hardcode
# /lib/x86_64-linux-gnu/... paths below -- exactly what this port removes.
FROM nginx:1.25.3-bookworm AS build

# A shell, but not for an entrypoint script's process tree -- for argv
# hygiene. libkrun appends its own words (earlycon=..., tsi_hijack, a bare
# `--`) to the end of the kernel command line, past the `--` stop sequence, so
# they arrive in the application's argv; see
# ../../library/unikraft-base/README.md. nginx's own argv parser
# (ngx_get_options) rejects the first word that does not start with `-`:
# `nginx: invalid option: "earlycon=pl011,mmio32,0x0a001000"`. Booting
# `sh /start.sh` instead makes the junk the script's positional parameters,
# which nothing reads, and the script `exec`s nginx with a clean argv -- an
# execve(), not a fork, which Unikraft supports
# (CONFIG_APPELFLOADER_MULTIPROCESS in the Kraftfile).
#
# The musl variant of the official busybox image is statically linked, so it
# needs no loader or libc of its own in either architecture's rootfs.
FROM busybox:stable-musl AS shell

# Assemble the rootfs by *resolving* nginx's dependencies rather than listing
# them. Upstream's Dockerfile hardcodes /lib/x86_64-linux-gnu/... and
# /lib64/ld-linux-x86-64.so.2, which is exactly the part that cannot survive a
# change of architecture; asking ldd keeps this correct on both. nginx.conf
# below loads no dynamic modules (no `load_module` directive), so nothing in
# /usr/lib/nginx/modules needs to ship.
FROM build AS rootfs
COPY --from=shell /bin/busybox /bin/busybox.static
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/bin /rootfs/etc/nginx \
             /rootfs/var/log/nginx /rootfs/var/cache/nginx /rootfs/var/run \
             /rootfs/wwwroot /rootfs/tmp; \
    cp /bin/busybox.static /rootfs/bin/busybox; \
    ln -s busybox /rootfs/bin/sh; \
    cp /usr/sbin/nginx /rootfs/usr/bin/nginx; \
    ldd /usr/sbin/nginx \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    cp /etc/nginx/mime.types /rootfs/etc/nginx/mime.types; \
    touch /rootfs/var/log/nginx/access.log /rootfs/var/log/nginx/error.log; \
    chmod 1777 /rootfs/tmp

# nginx.conf listens on 8080 (not upstream's stock 80/443 vhosts, dropped
# entirely) and sets `user root root;`, which is why /etc/passwd and
# /etc/group below carry a trimmed root entry -- nginx's core module resolves
# that name with getpwnam()/getgrnam() while parsing the `user` directive,
# unconditionally, regardless of master_process/daemon settings, and exits
# with "getpwnam(\"root\") failed" if the lookup misses.
COPY rootfs/etc/nginx/nginx.conf /rootfs/etc/nginx/nginx.conf
COPY rootfs/etc/passwd /rootfs/etc/passwd
COPY rootfs/etc/group /rootfs/etc/group
COPY rootfs/wwwroot/ /rootfs/wwwroot/
COPY start.sh /rootfs/start.sh

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile), so unlike
# upstream's rootfs this one ships none of them.
FROM scratch

COPY --from=rootfs /rootfs/ /
