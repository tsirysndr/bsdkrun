# golang:1.21-bookworm is published for both amd64 and arm64; upstream pins
# --platform=linux/x86_64, which is exactly what this port removes.
FROM golang:1.21-bookworm AS build

ARG CADDY_VERSION=2.7.6
# Set automatically by `docker buildx build --platform` (see build.sh); no
# --build-arg needed.
ARG TARGETARCH

WORKDIR /caddy

# Upstream cross-compiles with cgo (-linkmode external -extldflags
# static-pie), which needs a C toolchain for the *target* platform. A fully
# static build (CGO_ENABLED=0) needs none: no C toolchain to cross-compile,
# and the result has no ELF interpreter or dynamic libraries to resolve with
# ldd, unlike ../unikraft-php and ../unikraft-redis.
#
# -T on amd64 is not tuning, it is what makes this boot at all there. `go
# build` emits a non-PIE ET_EXEC whose load addresses are fixed, starting at
# 0x400000 (4 MiB) — and the Unikraft fc kernel links at ~1 MiB and grows
# past 4 MiB with the rootfs embedded, so the loader maps caddy over the
# running kernel and the guest dies mute while mapping the writable segment.
# 1 GiB clears the kernel while staying below the heap base (16 GiB) and
# ukvmem's application range (64 GiB). arm64 links at 0x10000 with the kernel
# 2 GiB away and needs no such move. See ../unikraft-go/README.md for the
# loader trace that shows the failure.
RUN --mount=type=cache,target=/root/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    set -xe; \
    apt-get update; \
    apt-get install -y --no-install-recommends git; \
    git clone --depth=1 --branch v${CADDY_VERSION} https://github.com/caddyserver/caddy.git /caddy; \
    LDFLAGS="-s -w"; \
    if [ "${TARGETARCH}" = amd64 ]; then LDFLAGS="$LDFLAGS -T 0x40000000"; fi; \
    CGO_ENABLED=0 GOOS=linux GOARCH=${TARGETARCH} go build \
      -tags netgo \
      -ldflags "$LDFLAGS" \
      -o /caddy/caddy cmd/caddy/main.go

RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/etc/caddy /rootfs/var/www; \
    cp /caddy/caddy /rootfs/usr/bin/caddy

COPY Caddyfile /rootfs/etc/caddy/Caddyfile
COPY index.html /rootfs/var/www/index.html

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile). Upstream
# ships a static /etc/hosts; that is not needed here.
FROM scratch

COPY --from=build /rootfs/ /
