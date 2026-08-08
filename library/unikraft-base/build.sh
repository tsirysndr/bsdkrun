#!/bin/sh
# Build the `unikraft-base` runtime — Unikraft's app-elfloader stack, which
# runs an unmodified Linux binary as a unikernel.
#
# Upstream publishes this for x86_64 only, so arm64 has to be built from
# source. Three things are needed on top of a stock checkout, all applied by
# patches/apply.sh; see that script and the Kraftfile for the reasoning.
#
# The root filesystem is built with `docker buildx` for the *target*
# architecture (a separate axis from the kernel's), then handed to kraft as a
# prebuilt directory. Letting kraft drive the Dockerfile itself would build it
# for the host's architecture, which silently produces an image whose binaries
# cannot run in the guest.
#
# Usage: ./build.sh [arm64|x86_64|all]     (default: the host arch)
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE"

KRAFT_VER=0.12.15
HOST_DEB_ARCH=$(uname -m | sed -e 's/aarch64/arm64/' -e 's/x86_64/amd64/')

build_one() {
	ARCH="$1"
	case "$ARCH" in
		arm64) DOCKER_PLATFORM=linux/arm64 ;;
		x86_64) DOCKER_PLATFORM=linux/amd64 ;;
		*) echo "unsupported arch: $ARCH" >&2; return 1 ;;
	esac

	echo "==> [$ARCH] building rootfs with buildx ($DOCKER_PLATFORM)"

	# buildx emulates the target arch via binfmt, so this works on either
	# host. --load puts the result in the local image store so it can be
	# exported; --provenance=false keeps the store entry a plain image
	# rather than an index, which `docker create` cannot instantiate.
	docker buildx build \
		--platform "$DOCKER_PLATFORM" \
		--provenance=false \
		--load \
		-t "unikraft-base-rootfs:$ARCH" \
		.

	# Flatten the image to a directory. `docker export` on a created (never
	# started) container gives the merged filesystem without the layer
	# metadata, which is exactly what kraft's --rootfs wants.
	ROOTFS="$HERE/.rootfs-$ARCH"
	rm -rf "$ROOTFS"
	mkdir -p "$ROOTFS"
	CID=$(docker create --platform "$DOCKER_PLATFORM" "unikraft-base-rootfs:$ARCH" /fallback)
	docker export "$CID" | tar -x -C "$ROOTFS"
	docker rm -f "$CID" >/dev/null

	echo "==> [$ARCH] building unikernel"

	# Cross-compiling needs the matching toolchain; Unikraft invokes it as
	# <triple>-gcc.
	CROSS=""
	[ "$ARCH" = x86_64 ] && [ "$HOST_DEB_ARCH" != amd64 ] && CROSS="gcc-x86-64-linux-gnu binutils-x86-64-linux-gnu"
	[ "$ARCH" = arm64 ] && [ "$HOST_DEB_ARCH" != arm64 ] && CROSS="gcc-aarch64-linux-gnu binutils-aarch64-linux-gnu"

	# KRAFTKIT_NO_PROMPT: without it kraft opens an interactive prompt,
	# fails to create a reader with no TTY, and hangs at "updating index".
	docker run --rm -e KRAFTKIT_NO_PROMPT=1 \
		-v "$HERE":/w -w /w debian:bookworm sh -eux -c "
		apt-get update -qq
		apt-get install -y -qq --no-install-recommends \
			build-essential libncurses-dev libyaml-dev flex bison git \
			wget unzip uuid-runtime python3 curl ca-certificates bc \
			file patch $CROSS >/dev/null
		curl -sSfLo /tmp/kraft.deb \
			https://github.com/unikraft/kraftkit/releases/download/v$KRAFT_VER/kraftkit_${KRAFT_VER}_linux_${HOST_DEB_ARCH}.deb
		dpkg -i /tmp/kraft.deb

		# Fetch first, so there is a tree to patch. The index refresh is
		# explicit: \`kraft build\` does it implicitly but \`kraft fetch\`
		# does not, and fails with 'could not find: core/unikraft:stable'
		# on a clean machine.
		kraft pkg update
		kraft fetch --arch $ARCH --plat fc

		./patches/apply.sh .unikraft/unikraft

		# NOT --no-cache: that re-fetches the sources and throws the
		# patches away.
		kraft build --arch $ARCH --plat fc \
			--rootfs .rootfs-$ARCH \
			--log-level info --log-type basic
		ls -l .unikraft/build/unikraft-base_fc-$ARCH
	"
}

case "${1:-$(uname -m)}" in
	all) build_one arm64; build_one x86_64 ;;
	aarch64 | arm64) build_one arm64 ;;
	x86_64 | amd64) build_one x86_64 ;;
	*) echo "usage: $0 [arm64|x86_64|all]" >&2; exit 1 ;;
esac
