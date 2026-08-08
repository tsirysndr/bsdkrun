# Debian's packaged node, not the official node image, for two reasons that
# together rule out every other obvious choice on arm64:
#
#   * app-elfloader only loads **position-independent** executables. The
#     Debian-based official images (node:24-bookworm-slim and friends) ship an
#     upstream binary of ELF type EXEC, which is rejected outright:
#       "ELF executable is not position-independent!"
#   * node:24-alpine *is* PIE, but that build fails on arm64 for reasons of its
#     own. (Not a musl problem: trivial musl binaries, both dynamic and
#     static-PIE, run fine.)
#
# Debian builds with PIE by default, so `apt install nodejs` gives a binary that
# is both PIE and glibc. The cost is the distribution's version (Node 18 on
# bookworm) rather than Node 24; Express 4.18 is happy on it.
#
# NOTE: this does not make the example work on arm64 yet — node then trips a
# stack-protector check inside libc.so.6 itself. It does work on x86_64, and it
# moves arm64 off a node-specific dead end onto the one open glibc issue, which
# is tracked in ../../library/unikraft-base/README.md.
FROM debian:bookworm-slim AS build

RUN set -eu; \
    apt-get update; \
    apt-get install -y --no-install-recommends nodejs npm ca-certificates; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src

COPY package.json ./
RUN npm install --omit=dev

COPY app/index.js ./server.js

# Resolve the binary's libraries rather than listing them. Upstream's Dockerfile
# hardcodes a loader path, which is the one thing that cannot survive a change
# of architecture or libc — the loader is ld-musl-x86_64.so.1,
# ld-musl-aarch64.so.1 or ld-linux-aarch64.so.1 depending on both. Asking ldd
# keeps this correct everywhere and picks up anything a future build needs.
RUN set -eu; \
    NODE_BIN="$(command -v node)"; \
    mkdir -p /rootfs/usr/bin /rootfs/usr/src /rootfs/tmp; \
    cp "$NODE_BIN" /rootfs/usr/bin/node; \
    ldd "$NODE_BIN" \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    cp -a /usr/src/node_modules /rootfs/usr/src/node_modules; \
    cp /usr/src/server.js /rootfs/usr/src/server.js

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile), so this
# image deliberately ships no /etc.
FROM scratch

COPY --from=build /rootfs/ /
