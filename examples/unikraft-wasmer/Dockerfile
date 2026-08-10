# wasmer 7.2.1 needs rustc >= 1.93; pinned newer than ../unikraft-actix's
# 1.75.0-bookworm for that reason.
FROM rust:1.97.1-bookworm AS build

WORKDIR /src

# The guest: a tiny WASI module, built first so it exists on disk (as
# guest/target/wasm32-wasip1/release/guest.wasm) before the host crate
# compiles — app/src/main.rs embeds it with include_bytes!() at that
# relative path, so the layout under /src must mirror this repo's
# app/ + guest/ layout exactly.
RUN rustup target add wasm32-wasip1
COPY guest/Cargo.toml guest/Cargo.lock ./guest/
COPY guest/src ./guest/src
RUN cd guest && cargo build --release --target wasm32-wasip1 --locked

# The host: a plain HTTP server that compiles and runs the guest with wasmer
# on every request. See app/Cargo.toml for why singlepass rather than
# wasmer's default cranelift compiler.
COPY app/Cargo.toml app/Cargo.lock ./app/
COPY app/src ./app/src
RUN cd app && cargo build --release --locked

# Assemble the rootfs by *resolving* the binary's dependencies rather than
# listing them. Upstream conventions elsewhere in this repo (see
# ../unikraft-actix/Dockerfile) hardcode `/lib/x86_64-linux-gnu/...`; on
# arm64 that's a different directory *and* a different filename. ldd reports
# whatever this arch actually needs.
RUN set -eu; \
    mkdir -p /rootfs/tmp; \
    cp app/target/release/server /rootfs/server; \
    ldd app/target/release/server \
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
