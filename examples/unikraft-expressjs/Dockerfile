FROM node:24-alpine AS build

WORKDIR /usr/src

COPY package.json ./
RUN npm install --omit=dev

COPY app/index.js ./server.js

# Assemble the rootfs by *resolving* node's dependencies rather than listing
# them. Upstream's Dockerfile hardcodes `/lib/ld-musl-x86_64.so.1`, which is
# the one thing in it that cannot survive a change of architecture: the musl
# loader is `ld-musl-aarch64.so.1` on arm64. Asking ldd keeps this correct on
# both, and it also picks up any library a future node build starts needing.
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/usr/src /rootfs/tmp; \
    cp /usr/local/bin/node /rootfs/usr/bin/node; \
    ldd /usr/local/bin/node \
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
