#!/usr/bin/env bash
#
# Build hello.so for an OSv guest, then compose it into a bootable image.
#
# The compile runs inside a Debian container: an OSv application is a *Linux*
# shared object, and macOS has no Linux toolchain. On Linux the container is
# still used, so both hosts produce the same binary.
#
# Usage: ./build.sh [arch]      arch: arm64 (default on Apple Silicon) | x86_64
set -euo pipefail

cd "$(dirname "$0")"

# A guest runs the host's architecture under hardware virtualization, so the
# default target is the host's own arch.
case "${1:-$(uname -m)}" in
    arm64|aarch64) ARCH=arm64;  TRIPLE=aarch64-linux-gnu ;;
    x86_64|amd64)  ARCH=x86_64; TRIPLE=x86_64-linux-gnu  ;;
    *) echo "unsupported arch: ${1:-$(uname -m)}" >&2; exit 1 ;;
esac

echo ">> building hello.so for $ARCH"
docker run --rm --platform "linux/$([ "$ARCH" = arm64 ] && echo arm64 || echo amd64)" \
    -v "$PWD:/src" -w /src debian:bookworm-slim \
    sh -euc '
        apt-get update -qq
        apt-get install -y -qq gcc >/dev/null
        # -shared: OSv dlopen()s the application and calls its main(), so the
        #   app must be a shared object, not an executable.
        # -fPIC:   OSv relocates it to wherever it lands in the single address
        #   space it shares with the kernel.
        gcc -O2 -shared -fPIC -o hello.so hello.c
    '

file hello.so

# capstan composes the OSv loader and the application filesystem into one image.
# --fs rofs builds a read-only filesystem entirely on the host; the zfs variant
# would need to boot a builder VM first.
if ! command -v capstan >/dev/null; then
    echo "capstan not found — build it from https://github.com/cloudius-systems/capstan" >&2
    exit 1
fi

echo ">> composing the OSv image"
capstan package compose --fs rofs --run "/hello.so" -p osv-hello

echo
echo "Done. Boot it with:"
echo "    capstan run -e /hello.so osv-hello"
