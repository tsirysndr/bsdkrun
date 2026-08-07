#!/bin/sh
# Build the unikernel for bsdkrun.
#
# The Unikraft build tree needs GNU make/sed and a Linux toolchain, so on macOS
# this runs kraft inside a Debian container (a host build dies with "gsed:
# command not found", then "Target architecture () is currently not supported").
# On Linux, just run `kraft build --plat fc --arch <arch>` directly.
#
# Usage: ./build.sh [arm64|x86_64]     (default: the host arch)
set -eu

ARCH="${1:-$(uname -m)}"
case "$ARCH" in
	aarch64 | arm64) ARCH=arm64 ;;
	x86_64 | amd64) ARCH=x86_64 ;;
	*)
		echo "unsupported arch: $ARCH (want arm64 or x86_64)" >&2
		exit 1
		;;
esac

# kraft's own release .deb — get.kraftkit.sh wants /dev/tty and gpg, neither of
# which a plain `docker run` has.
KRAFT_VER=0.12.15
HOST_DEB_ARCH=$(uname -m | sed -e 's/aarch64/arm64/' -e 's/x86_64/amd64/')

# Cross-compiling (e.g. x86_64 on an arm64 Mac) needs the matching toolchain;
# Unikraft invokes it as <triple>-gcc.
CROSS=""
[ "$ARCH" = x86_64 ] && [ "$HOST_DEB_ARCH" != amd64 ] && CROSS="gcc-x86-64-linux-gnu binutils-x86-64-linux-gnu"
[ "$ARCH" = arm64 ] && [ "$HOST_DEB_ARCH" != arm64 ] && CROSS="gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu"

exec docker run --rm -e KRAFTKIT_NO_PROMPT=1 -v "$PWD":/w -w /w debian:bookworm sh -eux -c "
	apt-get update -qq
	apt-get install -y -qq --no-install-recommends \
		build-essential libncurses-dev libyaml-dev flex bison git wget unzip \
		uuid-runtime python3 curl ca-certificates bc file $CROSS >/dev/null
	curl -sSfLo /tmp/kraft.deb \
		https://github.com/unikraft/kraftkit/releases/download/v$KRAFT_VER/kraftkit_${KRAFT_VER}_linux_${HOST_DEB_ARCH}.deb
	dpkg -i /tmp/kraft.deb
	kraft build --arch $ARCH --plat fc --no-cache --log-level info --log-type basic
	ls -l .unikraft/build/helloworld_fc-$ARCH
"
