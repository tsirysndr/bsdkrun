#!/usr/bin/env bash
#
# Package Debian's redis-server into a bootable OSv image.
#
# Nothing is compiled here: Debian's arm64 redis-server is already a PIE, which
# is what OSv's loader needs, so the stock binary runs unmodified. All this does
# is collect it and the libraries OSv does not provide itself.
#
# Usage: ./build.sh [arch]      arch: arm64 (default on Apple Silicon) | x86_64
set -euo pipefail

cd "$(dirname "$0")"

case "${1:-$(uname -m)}" in
    arm64|aarch64) ARCH=arm64;  PLATFORM=linux/arm64 ;;
    x86_64|amd64)  ARCH=x86_64; PLATFORM=linux/amd64 ;;
    *) echo "unsupported arch: ${1:-$(uname -m)}" >&2; exit 1 ;;
esac

rm -rf root && mkdir -p root/usr/lib

echo ">> collecting redis-server for $ARCH"
docker run --rm --platform "$PLATFORM" -v "$PWD/root:/out" debian:bookworm-slim sh -euc '
    apt-get update -qq
    apt-get install -y -qq redis-server >/dev/null
    cp /usr/bin/redis-server /out/redis-server

    # OSv implements the glibc set itself — its kernel exports the symbols, and
    # shipping Debian copies would shadow them. Everything else redis links
    # against has to be in the image, because there is no package manager and
    # no second process to fetch it.
    ldd /usr/bin/redis-server | awk "{print \$3}" | grep -E "^/" | while read -r lib; do
        case "$(basename "$lib")" in
            libc.so.6|libm.so.6|libpthread.so.0|libdl.so.2|librt.so.1|ld-linux*) continue ;;
        esac
        cp "$lib" /out/usr/lib/
    done
'

cp redis.conf root/redis.conf

# ELF type is the thing that decides whether this can work at all: OSv runs the
# application in the single address space it shares with the kernel, so a
# position-dependent binary (ET_EXEC) has nowhere to be loaded.
if ! file root/redis-server | grep -q "pie executable"; then
    echo "redis-server is not a PIE — OSv cannot load it" >&2
    exit 1
fi

if ! command -v capstan >/dev/null; then
    echo "capstan not found — build it from https://github.com/cloudius-systems/capstan" >&2
    exit 1
fi

echo ">> composing the OSv image"
( cd root && capstan package init --name osv-redis --title "Redis on OSv" \
      --author bsdkrun --version 1.0 >/dev/null
  capstan package compose --fs rofs --run "/redis-server /redis.conf" -p osv-redis )

echo
echo "Done. Boot it with:"
echo "    bsdkrun osv <image>.raw --cmdline '/redis-server /redis.conf' --port 6379:6379"
