#!/bin/sh
# Build a Nanos unikernel image from hello.c with ops (https://ops.city).
#
# The app must be a *static Linux* binary — Nanos implements the Linux
# syscall ABI. On macOS, compile it in a container; on Linux, plain
# `gcc -static` works.
#
# Usage: ./build.sh [arm64|x86_64]     (default: the host arch)
set -eu

ARCH="${1:-$(uname -m)}"
case "$ARCH" in
	aarch64 | arm64) ARCH=arm64 ;;
	x86_64 | amd64) ARCH=x86_64 ;;
	*) echo "unsupported arch: $ARCH" >&2; exit 1 ;;
esac

OPS="${OPS:-$HOME/.ops/bin/ops}"
[ -x "$OPS" ] || OPS=ops
command -v "$OPS" >/dev/null || {
	echo "ops not found — install it: curl https://ops.city/get.sh -sSfL | sh" >&2
	exit 1
}

if [ "$(uname -s)" = Darwin ]; then
	docker run --rm -v "$PWD":/w -w /w "--platform=linux/$(echo $ARCH | sed s/x86_64/amd64/)" debian:bookworm sh -c \
		'apt-get update -qq >/dev/null && apt-get install -y -qq gcc libc6-dev >/dev/null && gcc -static -O2 -o hello hello.c'
else
	gcc -static -O2 -o hello hello.c
fi

# The UEFI flag (config.json) matters on arm64: without it the image has no
# EFI System Partition at all. On x86_64 the default BIOS image is what the
# direct-kernel boot path uses, so plain build there.
if [ "$ARCH" = arm64 ]; then
	"$OPS" build hello --arch "$ARCH" -c config.json -i nanos-hello
else
	"$OPS" build hello --arch "$ARCH" -i nanos-hello
fi
echo "image: $HOME/.ops/images/nanos-hello"
