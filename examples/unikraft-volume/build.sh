#!/bin/sh
# Build the unikernel for bsdkrun, with the upstream fixes it needs.
#
# Unlike the helloworld example this is a three-step build — fetch, patch,
# build — because two bugs in unikraft 0.21.0 stop a virtio-fs guest working
# at all. Both are one-liners; see patches/ for what and why. `kraft build`
# alone would pull the sources and compile them in one go, leaving nowhere to
# patch, so the fetch is run explicitly first.
#
# The Unikraft build tree needs GNU make/sed and a Linux toolchain, so on
# macOS this runs inside a Debian container. On Linux you can run the same
# three commands directly.
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

# kraft's own release .deb — get.kraftkit.sh wants /dev/tty and gpg, neither
# of which a plain `docker run` has.
KRAFT_VER=0.12.15
HOST_DEB_ARCH=$(uname -m | sed -e 's/aarch64/arm64/' -e 's/x86_64/amd64/')

# Cross-compiling (e.g. x86_64 on an arm64 Mac) needs the matching toolchain;
# Unikraft invokes it as <triple>-gcc.
CROSS=""
[ "$ARCH" = x86_64 ] && [ "$HOST_DEB_ARCH" != amd64 ] && CROSS="gcc-x86-64-linux-gnu binutils-x86-64-linux-gnu"
[ "$ARCH" = arm64 ] && [ "$HOST_DEB_ARCH" != arm64 ] && CROSS="gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu"

# Everything below runs in the container (or directly, if you lift it out).
exec docker run --rm -e KRAFTKIT_NO_PROMPT=1 -v "$PWD":/w -w /w debian:bookworm sh -eux -c "
	apt-get update -qq
	apt-get install -y -qq --no-install-recommends \
		build-essential libncurses-dev libyaml-dev flex bison git wget unzip \
		uuid-runtime python3 curl ca-certificates bc file patch $CROSS >/dev/null
	curl -sSfLo /tmp/kraft.deb \
		https://github.com/unikraft/kraftkit/releases/download/v$KRAFT_VER/kraftkit_${KRAFT_VER}_linux_${HOST_DEB_ARCH}.deb
	dpkg -i /tmp/kraft.deb

	# 1. Fetch the Unikraft sources, so there is something to patch.
	#    The index refresh is explicit: \`kraft build\` does it implicitly,
	#    but \`kraft fetch\` does not and fails with 'could not find:
	#    core/unikraft:stable' on a clean machine.
	kraft pkg update
	kraft fetch --arch $ARCH --plat fc

	# 2. Apply the upstream fixes. -N makes this idempotent: an
	#    already-patched tree is skipped rather than failing the build, so
	#    rebuilds work without a clean fetch.
	for p in patches/*.patch; do
		echo \"applying \$p\"
		patch -p1 -N -r - -d .unikraft/unikraft <\"\$p\" || true
	done

	# 3. Build. NOT --no-cache: that re-fetches the sources and would throw
	#    the patches away again.
	kraft build --arch $ARCH --plat fc --log-level info --log-type basic
	ls -l .unikraft/build/volume_fc-$ARCH
"
