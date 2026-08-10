# php:8.2-cli-bookworm is published for both amd64 and arm64; upstream pins
# --platform=linux/x86_64, which is exactly what this port removes.
FROM php:8.2-cli-bookworm AS build

# server.php serves over raw sockets, and the sockets extension is not
# compiled into the CLI image.
RUN docker-php-ext-install sockets

# Assemble the rootfs by *resolving* dependencies rather than listing them.
# Upstream enumerates 45 /lib/x86_64-linux-gnu/... paths, which do not exist
# on arm64; asking ldd -- of the php binary and of every extension .so, since
# extensions are dlopen()ed and ldd of the binary alone would miss their
# libraries -- keeps this correct on both.
#
# Only the extension directory is copied out of /usr/local/lib/php: the rest
# is PEAR, which nothing here uses. The extensions load from the same
# compiled-in path inside the guest.
RUN set -eu; \
    mkdir -p /rootfs/usr/local/bin /rootfs/usr/local/etc/php \
             /rootfs/usr/src /rootfs/tmp; \
    cp /usr/local/bin/php /rootfs/usr/local/bin/php; \
    ext_dir=$(php -r 'echo ini_get("extension_dir");'); \
    { ldd /usr/local/bin/php; \
      for so in "$ext_dir"/*.so; do ldd "$so"; done; } \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    mkdir -p "/rootfs$ext_dir"; \
    cp -a "$ext_dir"/. "/rootfs$ext_dir/"; \
    chmod 1777 /rootfs/tmp

# php.ini only loads the sockets extension; the compiled-in defaults do the
# rest.
COPY php.ini /rootfs/usr/local/etc/php/php.ini

# The single-process socket-loop HTTP server, verbatim from upstream.
COPY server.php /rootfs/usr/src/server.php

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile).
FROM scratch

COPY --from=build /rootfs/ /
