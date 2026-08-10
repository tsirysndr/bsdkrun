#!/bin/sh
# Build the redis unikernel.
#
# Two halves, because they target different architectures independently:
#
#   1. The root filesystem, built with `docker buildx --platform` for the
#      *target* arch. Letting kraft drive the Dockerfile builds it for the
#      *host* arch instead, which silently produces an image whose binaries
#      cannot run in the guest. This one also *runs* target-arch binaries
#      (`ldd`, at least) during the build, so building for the non-host architecture
#      needs binfmt/qemu -- Docker Desktop ships it; on a bare Linux host run
#      `docker run --privileged --rm tonistiigi/binfmt --install all` first.
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

APP=redis
HERE=$(cd "$(dirname "$0")" && pwd)
PATCHES="$HERE/../../library/unikraft-base/patches"
# The ELF loader is app-elfloader-rs, which the Kraftfile pulls from
# github.com/tsirysndr/app-elfloader like any other library.
#
# Set ELFLOADER_RS=/path/to/app-elfloader-rs to build a local checkout instead:
# the tree is then installed over whatever kraft fetched, the same way the
# Unikraft sources are patched. That is the only way to use a working copy,
# because a Kraftfile `source:` cannot name a path outside the build context
# and the kraft half of this script runs in a container on macOS.
ELFLOADER_RS="${ELFLOADER_RS:-}"
cd "$HERE"

if [ -n "$ELFLOADER_RS" ] && [ ! -f "$ELFLOADER_RS/Makefile.uk" ]; then
	echo "ELFLOADER_RS=$ELFLOADER_RS has no Makefile.uk" >&2
	exit 1
fi

ARCH="${1:-$(uname -m)}"
case "$ARCH" in
	aarch64 | arm64) ARCH=arm64; DOCKER_PLATFORM=linux/arm64 ;;
	x86_64 | amd64) ARCH=x86_64; DOCKER_PLATFORM=linux/amd64 ;;
	*)
		echo "unsupported arch: $ARCH (want arm64 or x86_64)" >&2
		exit 1
		;;
esac

rust_target() {
	case "$1" in
		arm64) echo aarch64-unknown-none-softfloat ;;
		x86_64) echo x86_64-unknown-none ;;
	esac
}

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

# Break every hard link in the root filesystem.
#
# Unikraft's ramfs has no link() -- `ramfs_link` is literally
# `vfscore_vop_eperm` -- and a ramfs is what the root filesystem is unpacked
# into at boot, so one hard link in the archive stops the boot dead:
#
#   ERR:  [libukcpio] Failed to create new hard link
#         /./usr/local/share/postgresql/timezone/Africa/Accra
#         (from /./usr/local/share/postgresql/timezone/Africa/Abidjan).
#   CRIT: [libvfscore] Failed to extract cpio archive to /: -11
#
# PostgreSQL's bundled timezone database is built almost entirely out of them --
# every zone that shares a definition with another -- so this is not an edge
# case, it is a couple of hundred files.
#
# It has to happen *here* rather than in the Dockerfile: BuildKit deduplicates
# identical files into hard links as it writes each COPY, so a tree that leaves
# the build stage with none arrives in the image with 245. Copying each one over
# itself costs about 500 KiB, and deduplication is the only thing hard links buy
# in an image that is never written to.
echo "==> [$ARCH] breaking hard links (Unikraft's ramfs has no link())"
find "$ROOTFS" -type f -links +1 -exec sh -c \
	'cp -p "$1" "$1.unlinked" && mv -f "$1.unlinked" "$1"' _ {} \;

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

# SKIP_FETCH=1 reuses an already-fetched and patched .unikraft tree, which is
# most of the wall clock on a rebuild.
if [ "${SKIP_FETCH:-0}" = 1 ] && [ -d .unikraft/unikraft ]; then
	FETCH_STEPS=""
else
	# kraft merges into an existing directory, so a tree left over from the C
	# app-elfloader keeps its elf_load.c and friends after the source changes.
	# Wipe it in that case only -- unconditionally would re-download the
	# loader on every build.
	FETCH_STEPS="kraft pkg update
[ -f .unikraft/apps/elfloader/elf_load.c ] && rm -rf .unikraft/apps/elfloader || true
kraft fetch -K .Kraftfile.$ARCH
\"\$PATCHES_DIR/apply.sh\" .unikraft/unikraft"
fi

KRAFT_STEPS=$(cat <<EOF
$FETCH_STEPS
# Optional: install a local app-elfloader-rs checkout over what kraft fetched.
if [ -n "\${ELFLOADER_RS_DIR:-}" ]; then
	rm -rf .unikraft/libs/app-elfloader
	mkdir -p .unikraft/libs/app-elfloader
	tar -C "\$ELFLOADER_RS_DIR" --exclude=./.git --exclude=./rust/target -cf - . \
		| tar -C .unikraft/libs/app-elfloader -xf -
fi
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
	command -v cargo >/dev/null 2>&1 || {
		echo "the Rust loader needs cargo; install it from https://rustup.rs" >&2
		exit 1
	}
	rustup target add "$(rust_target "$ARCH")" >/dev/null
	mkdir -p "$HERE/.rust-cache/target"
	APPELFLOADER_RUST_CACHE="$HERE/.rust-cache/target" \
	PATCHES_DIR="$PATCHES" ELFLOADER_RS_DIR="$ELFLOADER_RS" \
		KRAFTKIT_NO_PROMPT=1 sh -eux -c "$KRAFT_STEPS"
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
	# The Rust toolchain and its downloads are cached in a host directory so
	# that a rebuild does not re-download ~200 MB of rustup every time.
	# rustup + cargo registry, and the loader's cargo target directory. The
	# last one is what stops the Rust archive being rebuilt from scratch on
	# every kernel build; see APPELFLOADER_RUST_CACHE in the loader's
	# Makefile.uk.
	mkdir -p "$HERE/.rust-cache/rustup" "$HERE/.rust-cache/cargo" \
		"$HERE/.rust-cache/target"

	RS_MOUNT=""; RS_ENV=""
	if [ -n "$ELFLOADER_RS" ]; then
		RS_MOUNT="-v $ELFLOADER_RS:/elfloader-rs:ro"
		RS_ENV="-e ELFLOADER_RS_DIR=/elfloader-rs"
	fi

	# shellcheck disable=SC2086  # RS_MOUNT/RS_ENV are deliberately word-split
	exec docker run --rm -e KRAFTKIT_NO_PROMPT=1 -e PATCHES_DIR=/patches \
		$RS_MOUNT $RS_ENV \
		-e RUSTUP_HOME=/rust/rustup -e CARGO_HOME=/rust/cargo \
		-e APPELFLOADER_RUST_CACHE=/rust/target \
		-v "$HERE":/w -v "$PATCHES":/patches:ro \
		-v "$HERE/.rust-cache":/rust \
		-w /w debian:bookworm sh -eux -c "
		apt-get update -qq
		apt-get install -y -qq --no-install-recommends \
			build-essential libncurses-dev libyaml-dev flex bison git wget \
			unzip uuid-runtime python3 curl ca-certificates bc file patch \
			$CROSS >/dev/null
		curl -sSfLo /tmp/kraft.deb \
			https://github.com/unikraft/kraftkit/releases/download/v$KRAFT_VER/kraftkit_${KRAFT_VER}_linux_${HOST_DEB_ARCH}.deb
		dpkg -i /tmp/kraft.deb

		# rustup rather than Debian's rustc: bookworm ships 1.63, which is too
		# old for the bare-metal targets used here.
		export PATH=\"\$CARGO_HOME/bin:\$PATH\"
		command -v cargo >/dev/null 2>&1 || \
			curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
				| sh -s -- -y --no-modify-path --profile minimal \
				  --default-toolchain stable
		rustup target add $(rust_target "$ARCH")

		$KRAFT_STEPS
	"
fi
