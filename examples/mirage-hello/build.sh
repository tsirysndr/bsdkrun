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
