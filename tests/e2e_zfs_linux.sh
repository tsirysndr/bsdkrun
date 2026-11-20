#!/usr/bin/env bash
# End-to-end check of `bsdkrun zfs` against a Linux guest. The default
# kernel (vmlinux-builder 7.3.5 and later) has OpenZFS built in; the guest
# script installs `zpool` from the image's package manager when it is
# missing, which this test relies on (nothing is installed by hand).
#
# Alpine's edge: its userland matches the OpenZFS the kernel carries. The
# rootfs sits on a named volume so the install survives the restarts below.
# The pool is imported by the generated init, before the entrypoint, so the
# command sees it mounted.
#
# Usage: tests/e2e_zfs_linux.sh [path/to/bsdkrun]
# Another kernel: E2E_KERNEL=/path/to/vmlinux or E2E_KERNEL_VERSION=<release>.
set -euo pipefail

B=${1:-${BSDKRUN:-target/release/bsdkrun}}
export RUST_LOG=${RUST_LOG:-warn}
IMAGE=${E2E_IMAGE:-alpine:edge}
VOL=e2ezfsl$$
ROOT=e2ezfsroot$$
MACHINE=zfs-e2e-linux-$$
KERNEL_ARGS=()
if [ -n "${E2E_KERNEL:-}" ]; then
  KERNEL_ARGS=(--kernel "$E2E_KERNEL")
elif [ -n "${E2E_KERNEL_VERSION:-}" ]; then
  KERNEL_ARGS=(--kernel-version "$E2E_KERNEL_VERSION")
fi

cleanup() {
  "$B" stop "$MACHINE" >/dev/null 2>&1 || true
  "$B" rm -f "$MACHINE" >/dev/null 2>&1 || true
  "$B" zfs destroy --force "$VOL" >/dev/null 2>&1 || true
  "$B" volume rm -f "$ROOT" >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_agent() {
  for _ in $(seq 1 120); do "$B" exec "$1" true >/dev/null 2>&1 && return 0; sleep 2; done
  echo "agent for $1 did not come up" >&2
  return 1
}
expect() { if ! grep -q -- "$1"; then echo "expected $1" >&2; exit 1; fi; }

echo "== linux ($IMAGE) =="
"$B" zfs create "$VOL" --size 1G

# A stock image with no ZFS userland, on a volume so the restart below keeps
# what the init installs.
"$B" linux -d -v "$ROOT" --name "$MACHINE" "${KERNEL_ARGS[@]}" "$IMAGE" >/dev/null
wait_agent "$MACHINE"
"$B" exec "$MACHINE" sh -c 'uname -r; command -v zpool || echo "no zpool yet"' | expect "no zpool yet"
"$B" stop "$MACHINE" >/dev/null

# Attach while stopped; `start` boots the same kernel (recorded at the first
# boot) and the init installs zpool and imports the pool before anything
# else runs.
"$B" zfs attach "$VOL" "$MACHINE" | expect "takes effect"
"$B" start "$MACHINE" >/dev/null
wait_agent "$MACHINE"
"$B" exec "$MACHINE" sh -c 'zpool version' | expect "zfs-"
"$B" exec "$MACHINE" sh -c "zpool list -H -o name,health $VOL; echo hello > /mnt/$VOL/f" | expect "ONLINE"

# Branch live (the holder runs) and mount the branch live: real CoW, the
# write on the branch leaves main alone.
"$B" zfs branch "$VOL" feat | expect "$VOL@feat"
"$B" zfs attach "$VOL@feat" "$MACHINE" --mount /srv | expect "mounted at /srv"
"$B" exec "$MACHINE" sh -c "echo more >> /srv/f; echo main=\$(cat /mnt/$VOL/f); echo feat=\$(cat /srv/f | tr '\n' ' ')" > "/tmp/zfs-e2e-linux-$$.out"
expect "main=hello" < "/tmp/zfs-e2e-linux-$$.out"
expect "feat=hello more" < "/tmp/zfs-e2e-linux-$$.out"
"$B" exec "$MACHINE" sh -c "zfs list -H -o name,origin $VOL/feat" | expect "$VOL/main@feat"

# Persistence across a restart, through the init's import.
"$B" stop "$MACHINE" >/dev/null
"$B" start "$MACHINE" >/dev/null
wait_agent "$MACHINE"
"$B" exec "$MACHINE" sh -c "cat /mnt/$VOL/f" | expect "hello"
"$B" stop "$MACHINE" >/dev/null

# A pool made by Linux (portable feature set) must import in the FreeBSD helper.
"$B" zfs branch "$VOL" feat2 --from feat | expect "$VOL@feat2"
"$B" zfs destroy "$VOL@feat2" | expect "$VOL@feat2"
echo "== linux ok =="
echo "e2e zfs linux: ok"
