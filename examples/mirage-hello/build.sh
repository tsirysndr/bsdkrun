#!/bin/sh
# Build the MirageOS unikernel for the Solo5 hvt target.
#
# This needs the MirageOS toolchain (opam + the `mirage` CLI), not bsdkrun —
# `bsdkrun solo5` runs the result, it does not build it. The tender bsdkrun
# embeds is the *runtime* half of Solo5; the cross-compiler that produces a
# unikernel is a separate, much larger, install.
#
#   opam install mirage
#   ./build.sh
#
# Leaves the unikernel at dist/hello.hvt.
set -eu

cd "$(dirname "$0")"

# config.ml relies on mirage >= 4.11, where the network stack defaults to DHCP
# (4.11 inverted the `dhcp` key into `no_dhcp` and flipped the default). This is
# checked rather than left to fail on its own because the two failure modes are
# very different: 4.10 and earlier reject `~dhcp_key` outright at configure
# time, which is loud, but any version that *accepts* config.ml and defaults to
# static gives you a unikernel that boots perfectly and answers nothing on
# 10.0.0.2.
MIRAGE_MIN=4.11.0
mirage_version=$(mirage --version | sed 's/^v//')
if [ "$(printf '%s\n%s\n' "$MIRAGE_MIN" "$mirage_version" | sort -V | head -1)" != "$MIRAGE_MIN" ]; then
    echo "error: this example needs mirage >= $MIRAGE_MIN, found $mirage_version" >&2
    echo "       upgrade with: opam install mirage.$MIRAGE_MIN" >&2
    exit 1
fi

# -t hvt selects the tender bsdkrun embeds. The other targets (spt, virtio,
# xen, …) produce binaries that solo5-hvt refuses to load — `bsdkrun solo5`
# reads the ABI note and says so by name rather than letting the tender fail
# with an ELF error.
mirage configure -t hvt

# Pulls this unikernel's dependency sources into duniverse/ (opam-monorepo).
# Slow the first time, cached afterwards.
make depends

make build

echo
echo "Built dist/hello.hvt — boot it with:"
echo
echo "  bsdkrun solo5 dist/hello.hvt --mem 128 --port 18080:8080"
echo
echo "then: curl http://127.0.0.1:18080/"
