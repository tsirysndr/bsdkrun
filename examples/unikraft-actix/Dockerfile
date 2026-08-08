FROM rust:1.75.0-bookworm AS build

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# Assemble the rootfs by *resolving* the binary's dependencies rather than
# listing them. Upstream's Dockerfile hardcodes `/lib/x86_64-linux-gnu/...`
# and `/lib64/ld-linux-x86-64.so.2`; on arm64 those are
# `/lib/aarch64-linux-gnu/...` and `/lib/ld-linux-aarch64.so.1` — a different
# directory *and* a different filename, so no amount of substitution fixes it
# in place. ldd reports whatever this arch actually needs.
RUN set -eu; \
    mkdir -p /rootfs/tmp; \
    cp target/release/hello /rootfs/server; \
    ldd target/release/hello \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done

# /etc/resolv.conf, /etc/hosts and /etc/hostname are generated at boot by
# app-elfloader (CONFIG_APPELFLOADER_AUTOGEN_* in the Kraftfile), so this
# image deliberately ships no /etc.
FROM scratch

COPY --from=build /rootfs/ /
