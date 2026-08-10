# debian:bookworm is published for both amd64 and arm64. There is no
# upstream unikraft-cloud/examples port of Apache to follow (unlike
# ../unikraft-php and ../unikraft-redis); this is Debian's `apache2` package
# trimmed down from scratch to a single foreground process serving one
# static index.html.
FROM debian:bookworm AS build

RUN apt-get update -qq && \
    apt-get install -y -qq --no-install-recommends apache2 && \
    rm -rf /var/lib/apt/lists/*

# A shell, but not for an entrypoint script's process tree -- for argv
# hygiene. libkrun appends its own words (earlycon=..., tsi_hijack, a bare
# `--`) to the end of the kernel command line, past the `--` stop sequence,
# so they arrive in the application's argv; see
# ../../library/unikraft-base/README.md. apache2's argv parser is
# getopt-based and dies on the first unrecognized word ("Usage: apache2
# [-D name] ..."). Booting `sh /start.sh` instead makes the junk the script's
# positional parameters, which nothing reads, and the script `exec`s apache2
# with a clean argv -- an execve(), not a fork, which Unikraft supports
# (CONFIG_APPELFLOADER_MULTIPROCESS in the Kraftfile).
#
# The musl variant of the official busybox image is statically linked, so it
# needs no loader or libc of its own in either architecture's rootfs.
FROM busybox:stable-musl AS shell

# Assemble the rootfs by *resolving* apache2's dependencies rather than
# listing them, exactly like ../unikraft-php and ../unikraft-redis do.
# apache2 dlopen()s its modules (LoadModule), so ldd of the binary alone
# would miss their libraries -- resolve each of the four modules the trimmed
# config actually loads (mpm_event, authz_core, mime, dir) too. In practice
# all four only pull in libc; libpcre2/libaprutil/libapr/libexpat/libcrypt/
# libuuid come from the main binary.
FROM build AS rootfs
COPY --from=shell /bin/busybox /bin/busybox.static
RUN set -eu; \
    mkdir -p /rootfs/usr/sbin /rootfs/usr/lib/apache2/modules \
             /rootfs/etc/apache2 /rootfs/var/www/html \
             /rootfs/var/run/apache2 /rootfs/bin /rootfs/tmp; \
    cp /bin/busybox.static /rootfs/bin/busybox; \
    ln -s busybox /rootfs/bin/sh; \
    cp /usr/sbin/apache2 /rootfs/usr/sbin/apache2; \
    for mod in mod_mpm_event.so mod_authz_core.so mod_mime.so mod_dir.so; do \
        cp "/usr/lib/apache2/modules/$mod" "/rootfs/usr/lib/apache2/modules/$mod"; \
    done; \
    { ldd /usr/sbin/apache2; \
      for mod in mod_mpm_event.so mod_authz_core.so mod_mime.so mod_dir.so; do \
          ldd "/usr/lib/apache2/modules/$mod"; \
      done; } \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    chmod 1777 /rootfs/tmp

COPY apache2.conf /rootfs/etc/apache2/apache2.conf
COPY mime.types /rootfs/etc/apache2/mime.types
COPY index.html /rootfs/var/www/html/index.html
COPY start.sh /rootfs/start.sh

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile).
FROM scratch

COPY --from=rootfs /rootfs/ /
