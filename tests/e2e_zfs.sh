#!/usr/bin/env bash
# End-to-end check of `bsdkrun zfs` against a FreeBSD guest (and NetBSD with
# E2E_ZFS_NETBSD=1): create, attach while stopped, import on start, branch
# live, mount a branch live, detach, branch through the helper VM, destroy.
#
# Reads what the guest actually did — a `zfs list` from inside, a file written
# on one branch and absent from the other — rather than trusting exit codes.
#
# Usage: tests/e2e_zfs.sh [path/to/bsdkrun]
set -euo pipefail

B=${1:-${BSDKRUN:-target/release/bsdkrun}}
export RUST_LOG=${RUST_LOG:-warn}
VOL=e2ezfs$$
MACHINE=zfs-e2e-$$
cleanup() {
  "$B" stop "$MACHINE" >/dev/null 2>&1 || true
  "$B" rm -f "$MACHINE" >/dev/null 2>&1 || true
  "$B" zfs destroy --force "$VOL" >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_agent() {
  for _ in $(seq 1 120); do "$B" exec "$1" true >/dev/null 2>&1 && return 0; sleep 2; done
  echo "agent for $1 did not come up" >&2
  return 1
}
# The import runs through the agent after boot (BSDs), so wait for the mount
# rather than the agent.
wait_mounted() {
  for _ in $(seq 1 45); do
    "$B" exec "$1" sh -c "zfs get -H -o value mounted '$2' 2>/dev/null | grep -q yes" && return 0
    sleep 2
  done
  echo "$2 never mounted in $1" >&2
  "$B" logs --boot "$1" | tail -20 >&2 || true
  return 1
}
expect() { # expect STRING <<< OUTPUT
  if ! grep -q -- "$1"; then echo "expected $1" >&2; exit 1; fi
}

run_os() {
  local os=$1
  echo "== $os =="
  "$B" zfs create "$VOL" --size 1G
  "$B" zfs ls | expect "$VOL"

  "$B" "$os" -d --name "$MACHINE" >/dev/null
  wait_agent "$MACHINE"
  # No hot-plug: a running non-holder is refused.
  if "$B" zfs attach "$VOL" "$MACHINE" 2>/dev/null; then echo "attach to a running non-holder should fail" >&2; exit 1; fi
  "$B" stop "$MACHINE" >/dev/null
  "$B" zfs attach "$VOL" "$MACHINE" | expect "takes effect"
  "$B" start "$MACHINE" >/dev/null
  wait_agent "$MACHINE"
  wait_mounted "$MACHINE" "$VOL/main"
  "$B" exec "$MACHINE" sh -c "zpool list -H -o name,health $VOL; echo hello > /mnt/$VOL/f" | expect "ONLINE"

  # Branch live (the holder runs), mount it live, and check it is a real
  # copy-on-write fork: a write on the branch leaves main untouched.
  "$B" zfs branch "$VOL" feat | expect "$VOL@feat"
  "$B" zfs attach "$VOL@feat" "$MACHINE" --mount /srv | expect "mounted at /srv"
  "$B" exec "$MACHINE" sh -c "echo more >> /srv/f; echo main=\$(cat /mnt/$VOL/f); echo feat=\$(cat /srv/f | tr '\n' ' ')" > "/tmp/zfs-e2e-$$.out"
  expect "main=hello" < "/tmp/zfs-e2e-$$.out"
  expect "feat=hello more" < "/tmp/zfs-e2e-$$.out"
  "$B" exec "$MACHINE" sh -c "zfs list -H -o name,origin $VOL/feat" | expect "$VOL/main@feat"
  "$B" zfs ls "$VOL" | expect "feat"

  "$B" zfs detach "$VOL@feat" "$MACHINE" | expect "detached"
  "$B" exec "$MACHINE" sh -c "zfs get -H -o value mounted $VOL/feat" | expect "no"

  # Data persists on the pool across a restart.
  "$B" stop "$MACHINE" >/dev/null
  "$B" start "$MACHINE" >/dev/null
  wait_agent "$MACHINE"
  wait_mounted "$MACHINE" "$VOL/main"
  "$B" exec "$MACHINE" sh -c "cat /mnt/$VOL/f" | expect "hello"
  "$B" stop "$MACHINE" >/dev/null

  # Nothing runs: branching boots the FreeBSD helper.
  "$B" zfs branch "$VOL" feat2 --from feat | expect "$VOL@feat2"
  "$B" zfs ls "$VOL" | expect "feat2"
  "$B" zfs destroy "$VOL@feat2" | expect "$VOL@feat2"
  if "$B" zfs destroy "$VOL" 2>/dev/null; then echo "destroying an attached volume should need --force" >&2; exit 1; fi
  "$B" zfs ls --json | expect "\"attached_to\":\[{\"machine\""

  "$B" rm -f "$MACHINE" >/dev/null
  "$B" zfs destroy --force "$VOL" | expect "$VOL"
  echo "== $os ok =="
}

run_os freebsd
if [ "${E2E_ZFS_NETBSD:-0}" = 1 ]; then
  # The first attach boots the machine once to ship the kernel's zfs module.
  run_os netbsd
fi
echo "e2e zfs: ok"
