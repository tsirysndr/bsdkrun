# Bun's Alpine image is a genuine musl build -- unlike Deno's, which is glibc
# behind a shim -- so there is no reason to prefer Debian here.
FROM oven/bun:alpine AS build

WORKDIR /usr/src
COPY app/server.js ./server.js

# Resolve Bun's libraries rather than listing them; see the Deno example for
# why. Bun pulls in libstdc++ and libgcc_s on top of musl.
RUN set -eu; \
    mkdir -p /rootfs/usr/bin /rootfs/usr/src /rootfs/tmp; \
    cp "$(command -v bun)" /rootfs/usr/bin/bun; \
    ldd "$(command -v bun)" \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do \
            mkdir -p "/rootfs$(dirname "$lib")"; \
            cp -L "$lib" "/rootfs$lib"; \
        done; \
    cp /usr/src/server.js /rootfs/usr/src/server.js; \
    mkdir -p /rootfs/proc/self; \
    ln -s /usr/bin/bun /rootfs/proc/self/exe

# Why the /proc/self/exe symlink: Bun locates its own binary through it at
# startup, to symlink itself as `node` so child processes get Bun (the
# `createFakeTemporaryNodeExecutable` step). Unikraft has no procfs, so the
# read fails and Bun panics with `error.FileNotFound` before it runs a line of
# JavaScript.
#
# A static symlink is not a hack here so much as the correct answer: a
# unikernel runs exactly one program, so the target of /proc/self/exe is known
# when the image is built. It would be wrong on a multi-process system and is
# exactly right on this one. Unikraft's cpio extractor creates symlinks and
# ramfs stores them, so it survives into the guest.

# /etc is generated at boot by the ELF loader; see the Deno example.
FROM scratch

COPY --from=build /rootfs/ /
