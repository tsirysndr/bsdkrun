#!/bin/sh
# Package the built unikernels as OCI artifacts and push them to GHCR.
#
# Run ./build.sh all first — this script only packages what is already in
# .unikraft/build/.
#
# Each architecture is pushed under its own tag. kraft's OCI packaging writes
# one manifest per (plat, arch); a single `:latest` tag pushed twice would
# leave the second overwriting the first rather than producing a multi-arch
# index, so the arch is part of the tag and `:latest` points at whichever the
# host can actually run.
#
# Auth: needs a token with write:packages.
#   gh auth refresh -h github.com -s write:packages,read:packages
#   echo "$GH_TOKEN" | docker login ghcr.io -u <user> --password-stdin
#
# Usage: ./push.sh [arm64|x86_64|all]      (default: all)
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE"

: "${REGISTRY:=ghcr.io}"
: "${NAMESPACE:=tsirysndr}"
: "${IMAGE:=unikraft-base}"
: "${TAG:=latest}"

KRAFT_VER=0.12.15
HOST_DEB_ARCH=$(uname -m | sed -e 's/aarch64/arm64/' -e 's/x86_64/amd64/')

if [ -z "${GH_TOKEN:-}" ]; then
	echo "GH_TOKEN is not set (needs the write:packages scope)" >&2
	exit 1
fi

push_one() {
	ARCH="$1"
	KERNEL=".unikraft/build/unikraft-base_fc-$ARCH"
	REF="$REGISTRY/$NAMESPACE/$IMAGE:$TAG-$ARCH"

	[ -f "$KERNEL" ] || {
		echo "no kernel for $ARCH at $KERNEL — run ./build.sh $ARCH first" >&2
		return 1
	}

	echo "==> pushing $REF"

	# kraft is run in a container for the same reason as the build; the
	# credentials go in as an env var rather than a mounted docker config
	# so nothing is written to disk.
	docker run --rm -e KRAFTKIT_NO_PROMPT=1 -e GH_TOKEN="$GH_TOKEN" \
		-v "$HERE":/w -w /w debian:bookworm sh -eu -c "
		apt-get update -qq
		apt-get install -y -qq --no-install-recommends \
			curl ca-certificates >/dev/null
		curl -sSfLo /tmp/kraft.deb \
			https://github.com/unikraft/kraftkit/releases/download/v$KRAFT_VER/kraftkit_${KRAFT_VER}_linux_${HOST_DEB_ARCH}.deb
		dpkg -i /tmp/kraft.deb >/dev/null

		kraft login -u $NAMESPACE -t \"\$GH_TOKEN\" $REGISTRY

		kraft pkg --as oci --name $REF \
			--arch $ARCH --plat fc \
			--kernel $KERNEL \
			--rootfs .rootfs-$ARCH \
			--push --log-level info --log-type basic
	"
}

case "${1:-all}" in
	all) push_one arm64; push_one x86_64 ;;
	aarch64 | arm64) push_one arm64 ;;
	x86_64 | amd64) push_one x86_64 ;;
	*) echo "usage: $0 [arm64|x86_64|all]" >&2; exit 1 ;;
esac

cat <<EOF

Pushed. The package is PRIVATE until it is made public — that is a separate
step, not part of the push:

  gh api --method PATCH \\
    -H "Accept: application/vnd.github+json" \\
    /user/packages/container/$IMAGE/visibility \\
    -f visibility=public

(or via the package's settings page on github.com).
EOF
