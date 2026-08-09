#!/usr/bin/env bash
#
# Package Ruby + Sinatra into a bootable OSv image.
#
# Debian's arm64 ruby is already a PIE, so nothing is recompiled — this
# collects the interpreter, its stdlib, the gems, and every library the native
# extensions need, plus a small shim for two symbols OSv's libc lacks.
#
# Usage: ./build.sh [arch]      arch: arm64 (default on Apple Silicon) | x86_64
set -euo pipefail

cd "$(dirname "$0")"

case "${1:-$(uname -m)}" in
    arm64|aarch64) ARCH=arm64;  PLATFORM=linux/arm64; TRIPLE=aarch64-linux-gnu ;;
    x86_64|amd64)  ARCH=x86_64; PLATFORM=linux/amd64; TRIPLE=x86_64-linux-gnu  ;;
    *) echo "unsupported arch: ${1:-$(uname -m)}" >&2; exit 1 ;;
esac

rm -rf root && mkdir -p root/usr/lib

echo ">> collecting ruby + sinatra for $ARCH"
docker run --rm --platform "$PLATFORM" \
    -v "$PWD/root:/out" -v "$PWD:/src:ro" -e TRIPLE="$TRIPLE" \
    debian:bookworm-slim sh -euc '
    apt-get update -qq
    apt-get install -y -qq ruby ruby-dev build-essential patchelf >/dev/null
    gem install --no-document sinatra webrick rackup >/dev/null 2>&1

    mkdir -p /out/usr/lib "/out/usr/lib/$TRIPLE"
    cp "$(readlink -f /usr/bin/ruby)" /out/ruby

    # The glibc set is implemented by OSv itself and exported from its kernel;
    # shipping Debian copies would shadow it. Everything else has to be in the
    # image, because a unikernel has no package manager and no second process.
    collect() {
        ldd "$1" 2>/dev/null | awk "{print \$3}" | grep -E "^/" | while read -r lib; do
            case "$(basename "$lib")" in
                libc.so.6|libm.so.6|libpthread.so.0|libdl.so.2|librt.so.1|ld-linux*) continue ;;
            esac
            cp -n "$lib" /out/usr/lib/ 2>/dev/null || true
        done
    }

    collect "$(readlink -f /usr/bin/ruby)"
    # Each native extension carries its own dependencies, which the ruby binary
    # does not name — openssl.so needs libcrypto, and without it Sinatra dies
    # loading it with "failed looking up symbol i2d_TS_TST_INFO".
    find "/usr/lib/$TRIPLE/ruby" -name "*.so" | while read -r so; do collect "$so"; done

    # Stdlib (pure Ruby) and the arch-specific half (rbconfig.rb + extensions).
    cp -r /usr/lib/ruby "/out/usr/lib/ruby"
    cp -r "/usr/lib/$TRIPLE/ruby" "/out/usr/lib/$TRIPLE/ruby"

    GEMDIR=$(ruby -e "puts Gem.dir")
    mkdir -p "/out$(dirname "$GEMDIR")"
    cp -r "$GEMDIR" "/out$GEMDIR"

    # Resolve the two symbols OSv lacks, by making their importers depend on
    # a shim that defines them. See osv-shim.c for why stubbing is sound.
    gcc -shared -fPIC -o /out/usr/lib/libosvshim.so /src/osv-shim.c
    patchelf --add-needed libosvshim.so /out/usr/lib/libgmp.so.10
    patchelf --add-needed libosvshim.so /out/usr/lib/libruby-*.so.*
'

cp app.rb root/app.rb

if ! file root/ruby | grep -q "pie executable"; then
    echo "ruby is not a PIE — OSv cannot load it" >&2
    exit 1
fi

if ! command -v capstan >/dev/null; then
    echo "capstan not found — build it from https://github.com/cloudius-systems/capstan" >&2
    exit 1
fi

echo ">> composing the OSv image (a few thousand stdlib files; this is slow)"
( cd root && capstan package init --name osv-sinatra --title "Sinatra on OSv" \
      --author bsdkrun --version 1.0 >/dev/null
  capstan package compose --fs rofs --run "/ruby /app.rb" -p osv-sinatra )

echo
echo "Done. Boot it with:"
echo "    bsdkrun osv <image>.raw --cmdline '/ruby /app.rb' --port 4567:4567"
echo "    curl 127.0.0.1:4567/"
