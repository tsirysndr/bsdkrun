#!/usr/bin/env bash
#
# End-to-end test for `bsdkrun linux --attach-disk`: boot a detached Alpine
# guest with a raw disk attached as virtio-blk and assert that
#
#   1. the disk shows up in the guest as /dev/vda,
#   2. guest writes to the device reach the host image (flushed on `stop`),
#   3. `start` re-attaches the recorded disk, with the data intact,
#   4. automatic ext4 formatting preserves image size and restart persistence.
#
# The marker is written straight to the block device (no mkfs), so the test
# needs no package installation. The formatted-disk stage uses docker:dind
# and exercises installation of e2fsprogs when its image does not include it.
#
# Exit 0 on success, 1 on failure, 2 on missing prerequisites. Overridable via
# environment:
#   BSDKRUN_BIN  (default target/debug/bsdkrun — build + sign with `make build`)
#   IMAGE        (default "alpine", raw-device stage)
#   FORMATTED_IMAGE (default "docker:dind", ext4-formatting stage)
#   TIMEOUT      (seconds per boot, default 180)
set -uo pipefail

BIN="${BSDKRUN_BIN:-target/debug/bsdkrun}"
IMAGE="${IMAGE:-alpine}"
FORMATTED_IMAGE="${FORMATTED_IMAGE:-docker:dind}"
TIMEOUT="${TIMEOUT:-180}"
NAME="e2e-linux-disk-$$"
MARKER="bsdkrun-e2e-disk-marker-$$"
# Marker location on the raw device: block 16 of 4 KiB (64 KiB in), clear of
# anything that could be mistaken for a partition table.
BS=4096
BLOCK=16

if [ ! -x "$BIN" ]; then
  echo "e2e: missing bsdkrun binary: $BIN (run 'make build' first)" >&2
  exit 2
fi

DISK="$(mktemp -t bsdkrun-e2e-disk.XXXXXX)"
ID=""
VOLUME=""

cleanup() {
  [ -n "$ID" ] && "$BIN" rm -f "$ID" >/dev/null 2>&1
  [ -n "$VOLUME" ] && "$BIN" volume rm "$VOLUME" >/dev/null 2>&1
  rm -f "$DISK"
}
trap cleanup EXIT INT TERM

fail() {
  echo "e2e: FAIL — $*" >&2
  [ -n "$ID" ] && "$BIN" logs "$ID" 2>/dev/null | tail -20 >&2
  exit 1
}

# Poll the guest agent until a trivial exec succeeds.
wait_for_agent() {
  local deadline=$(( SECONDS + TIMEOUT ))
  while [ "$SECONDS" -lt "$deadline" ]; do
    if "$BIN" exec "$ID" /bin/true >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  fail "guest agent not reachable within ${TIMEOUT}s"
}

# 64 MiB sparse raw image.
python3 -c 'import sys; open(sys.argv[1], "wb").truncate(64 * 1024 * 1024)' "$DISK" \
  || fail "could not create the disk image"

echo "e2e: booting $IMAGE with --attach-disk $DISK"
ID="$("$BIN" linux -d --name "$NAME" --attach-disk "$DISK" "$IMAGE" 2>/dev/null | tail -n 1)"
[ -n "$ID" ] || fail "bsdkrun did not return a machine id"
wait_for_agent

echo "e2e: checking the disk is attached as /dev/vda"
"$BIN" exec "$ID" sh -c '[ -b /dev/vda ]' \
  || fail "/dev/vda is not a block device in the guest"

echo "e2e: writing a marker to the device"
"$BIN" exec "$ID" sh -c \
  "printf '%s' '$MARKER' | dd of=/dev/vda bs=$BS seek=$BLOCK conv=notrunc 2>/dev/null && sync" \
  || fail "could not write to /dev/vda"

echo "e2e: stopping the machine"
"$BIN" stop "$ID" >/dev/null || fail "stop failed"

echo "e2e: checking the marker reached the host image"
LC_ALL=C grep -aq "$MARKER" "$DISK" \
  || fail "marker not found in the host disk image after stop"

echo "e2e: restarting the machine (the disk must be re-attached)"
"$BIN" start "$ID" >/dev/null || fail "start failed"
wait_for_agent

echo "e2e: reading the marker back through the re-attached disk"
"$BIN" exec "$ID" sh -c \
  "dd if=/dev/vda bs=$BS skip=$BLOCK count=1 2>/dev/null | grep -aq '$MARKER'" \
  || fail "marker not readable from /dev/vda after restart"

echo "e2e: PASS — raw disk attached, flushed and re-attached"
"$BIN" rm -f "$ID" >/dev/null || fail "raw-disk cleanup failed"
ID=""

# mke2fs's final optimized zeroout used to shrink the backing raw file by
# 64 KiB. The first mount still succeeded because the running virtio device
# advertised the old capacity; ext4 rejected it at the next boot. Exercise
# the actual guest formatter, not a host-preformatted fixture.
FS_BYTES=$(( 512 * 1024 * 1024 ))
python3 -c 'import sys; open(sys.argv[1], "wb").truncate(int(sys.argv[2]))' "$DISK" "$FS_BYTES" \
  || fail "could not create the blank filesystem image"
VOLUME="$NAME-ext4"
ID="$("$BIN" linux -d --name "$VOLUME" -v "$VOLUME" --entrypoint /bin/sh \
  --attach-disk "$DISK:/data" "$FORMATTED_IMAGE" 2>/dev/null | tail -n 1)"
[ -n "$ID" ] || fail "formatted-disk boot did not return an id"
wait_for_agent

check_ext4() {
  "$BIN" exec "$ID" awk '$2 == "/data" && $3 == "ext4" { found=1 } END { exit !found }' /proc/mounts \
    || fail "/data is not mounted as ext4"
}
check_size() {
  python3 -c 'import os,sys; size=os.path.getsize(sys.argv[1]); expected=int(sys.argv[2]); assert size == expected, (size,expected)' "$DISK" "$FS_BYTES" \
    || fail "formatting or restart changed the backing file size"
}

check_ext4
check_size
"$BIN" exec "$ID" sh -c "printf '%s' '$MARKER' > /data/marker && sync" \
  || fail "could not write the ext4 marker"
for cycle in 1 2; do
  echo "e2e: checking formatted-disk restart $cycle"
  "$BIN" stop "$ID" >/dev/null || fail "formatted-disk stop failed"
  check_size
  "$BIN" start "$ID" >/dev/null || fail "formatted-disk restart failed"
  wait_for_agent
  check_ext4
  check_size
  got="$("$BIN" exec "$ID" cat /data/marker)" || fail "could not read the ext4 marker"
  [ "$got" = "$MARKER" ] || fail "ext4 data did not survive restart"
done

echo "e2e: PASS — guest-formatted ext4 retains its size, mount and data across restarts"
exit 0
