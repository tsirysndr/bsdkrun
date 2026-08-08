#!/bin/sh
# Build the Actix unikernel.
#
# Two halves, because they target different architectures independently:
#
#   1. The root filesystem, built with `docker buildx --platform` for the
#      *target* arch. Letting kraft drive the Dockerfile builds it for the
#      *host* arch instead, which silently produces an image whose binaries
#      cannot run in the guest.
#   2. The kernel, built by kraft against a patched Unikraft tree. The patches
#      (see ../../library/unikraft-base/patches) are what make an arm64
#      elfloader build work at all, so the sources are fetched explicitly first
#      and patched before `kraft build` runs.
#
# On macOS the kraft half runs in a Debian container: the Unikraft build tree
# needs GNU make/sed and a Linux toolchain. On Linux it runs directly.
#
# Usage: ./build.sh [arm64|x86_64]     (default: the host arch)
set -eu

APP=actix
HERE=$(cd "$(dirname "$0")" && pwd)
PATCHES="$HERE/../../library/unikraft-base/patches"
cd "$HERE"

ARCH="${1:-$(uname -m)}"
case "$ARCH" in
	aarch64 | arm64) ARCH=arm64; DOCKER_PLATFORM=linux/arm64 ;;
	x86_64 | amd64) ARCH=x86_64; DOCKER_PLATFORM=linux/amd64 ;;
	*)
		echo "unsupported arch: $ARCH (want arm64 or x86_64)" >&2
		exit 1
		;;
esac

KRAFT_VER=0.12.15
HOST_DEB_ARCH=$(uname -m | sed -e 's/aarch64/arm64/' -e 's/x86_64/amd64/')

echo "==> [$ARCH] building rootfs with buildx ($DOCKER_PLATFORM)"

# --provenance=false: with it, --load stores an index rather than a plain
# image, and `docker create` cannot instantiate that.
docker buildx build \
	--platform "$DOCKER_PLATFORM" \
	--provenance=false \
	--load \
	-t "$APP-rootfs:$ARCH" \
	.

ROOTFS="$HERE/.rootfs-$ARCH"
rm -rf "$ROOTFS"
mkdir -p "$ROOTFS"
CID=$(docker create --platform "$DOCKER_PLATFORM" "$APP-rootfs:$ARCH" /bin/true)
docker export "$CID" | tar -x -C "$ROOTFS"
docker rm -f "$CID" >/dev/null

echo "==> [$ARCH] building unikernel"

# The kraft half, as one script so it can run either directly or in a container.
# `kraft pkg update` is explicit: `kraft build` does it implicitly but
# `kraft fetch` does not, and fails with "could not find: core/unikraft:stable"
# on a clean machine. `kraft build` is NOT given --no-cache: that re-fetches the
# sources and throws the patches away.
# `kraft fetch` ignores --arch, --plat and --target: it always configures the
# *first* target in the Kraftfile. With both arches listed that is fc/arm64, so
# an x86_64 build would try to cross-compile and die for want of
# aarch64-linux-gnu-gcc. Hand kraft a Kraftfile with only the wanted target
# instead. (kraft fetch is also deprecated, but a separate fetch is what gives
# us a tree to patch before the build.)
OTHER=$([ "$ARCH" = arm64 ] && echo x86_64 || echo arm64)
sed "/^- fc\/$OTHER\$/d" Kraftfile > ".Kraftfile.$ARCH"
grep -q "^- fc/$ARCH\$" ".Kraftfile.$ARCH" || {
	echo "Kraftfile has no fc/$ARCH target" >&2
	exit 1
}

KRAFT_STEPS=$(cat <<EOF
kraft pkg update
kraft fetch -K .Kraftfile.$ARCH
"\$PATCHES_DIR/apply.sh" .unikraft/unikraft
kraft build -K .Kraftfile.$ARCH --rootfs .rootfs-$ARCH \
	--log-level info --log-type basic
ls -l .unikraft/build/${APP}_fc-$ARCH
EOF
)

if [ "$(uname -s)" = Linux ]; then
	command -v kraft >/dev/null 2>&1 || {
		echo "kraft not found; install it from https://get.kraftkit.sh" >&2
		exit 1
	}
	PATCHES_DIR="$PATCHES" KRAFTKIT_NO_PROMPT=1 sh -eux -c "$KRAFT_STEPS"
else
	# Cross-compiling needs the matching toolchain; Unikraft invokes it as
	# <triple>-gcc.
	CROSS=""
	[ "$ARCH" = x86_64 ] && [ "$HOST_DEB_ARCH" != amd64 ] && CROSS="gcc-x86-64-linux-gnu binutils-x86-64-linux-gnu"
	[ "$ARCH" = arm64 ] && [ "$HOST_DEB_ARCH" != arm64 ] && CROSS="gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu"

	# KRAFTKIT_NO_PROMPT: without it kraft opens an interactive prompt, fails
	# to create a reader with no TTY, and hangs at "updating index" forever.
	# The patches directory is mounted separately because it lives outside
	# this example's directory.
	exec docker run --rm -e KRAFTKIT_NO_PROMPT=1 -e PATCHES_DIR=/patches \
		-v "$HERE":/w -v "$PATCHES":/patches:ro -w /w debian:bookworm sh -eux -c "
		apt-get update -qq
		apt-get install -y -qq --no-install-recommends \
			build-essential libncurses-dev libyaml-dev flex bison git wget \
			unzip uuid-runtime python3 curl ca-certificates bc file patch \
			$CROSS >/dev/null
		curl -sSfLo /tmp/kraft.deb \
			https://github.com/unikraft/kraftkit/releases/download/v$KRAFT_VER/kraftkit_${KRAFT_VER}_linux_${HOST_DEB_ARCH}.deb
		dpkg -i /tmp/kraft.deb
		$KRAFT_STEPS
	"
fi
