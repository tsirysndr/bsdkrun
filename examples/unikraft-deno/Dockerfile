# Debian rather than denoland/deno:alpine on purpose. That "alpine" image does
# not ship a musl Deno: it carries a glibc shim under /usr/local/lib/glibc and
# runs the ordinary glibc binary through it. Its own `ldd` reports a broken
# relocation there (`_dl_find_object: symbol not found`), so it is a worse
# starting point than plain glibc, not a better one.
FROM denoland/deno:latest AS build

WORKDIR /usr/src
COPY app/server.js ./server.js

# Resolve Deno's libraries instead of listing them: the paths differ between
# architectures (`aarch64-linux-gnu` vs `x86_64-linux-gnu`) so no hardcoded
# list survives a change of target.
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/usr/src /rootfs/tmp; \
    cp "$(command -v deno)" /rootfs/usr/bin/deno; \
    ldd "$(command -v deno)" \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    cp /usr/src/server.js /rootfs/usr/src/server.js

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by the
# ELF loader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile), so this image
# deliberately ships no /etc.
FROM scratch

COPY --from=build /rootfs/ /
