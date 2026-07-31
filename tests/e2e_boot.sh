#!/usr/bin/env bash
#
# End-to-end boot test: launch bsdkrun's `firmware` target under a PTY and assert
# that the guest reaches the FreeBSD loader's beastie menu, then kill the VM.
#
# Why a PTY: libkrun's serial console registers its *input* fd with kqueue, which
# rejects non-pollable fds. `script` gives the child a real tty on fd 0/1, so the
# console works exactly as it does in an interactive terminal (see the README's
# "Console" section).
#
# Success = the marker string appears in the console output before the timeout.
# Exit 0 on success, 1 on failure/timeout. Overridable via environment:
#   BSDKRUN_BIN  (default target/debug/bsdkrun)
#   FIRMWARE     (default images/KRUN_EFI.fd)
#   DISK         (default images/fbsd15.raw)
#   MARKER       (default "Welcome to FreeBSD" — the beastie menu header)
#   TIMEOUT      (seconds, default 180)
#   CPUS / MEM   (default 2 / 2048)
set -uo pipefail

BIN="${BSDKRUN_BIN:-target/debug/bsdkrun}"
FIRMWARE="${FIRMWARE:-images/KRUN_EFI.fd}"
DISK="${DISK:-images/fbsd15.raw}"
MARKER="${MARKER:-Welcome to FreeBSD}"
TIMEOUT="${TIMEOUT:-180}"
CPUS="${CPUS:-2}"
MEM="${MEM:-2048}"

for f in "$BIN" "$FIRMWARE" "$DISK"; do
  if [ ! -e "$f" ]; then
    echo "e2e: missing required file: $f" >&2
    exit 2
  fi
done

LOG="$(mktemp -t bsdkrun-e2e.XXXXXX)"
SPID=""

cleanup() {
  [ -n "$SPID" ] && kill -9 "$SPID" 2>/dev/null
  # `script` is the parent; make sure the bsdkrun child is gone too.
  pkill -9 -f "$BIN" 2>/dev/null
  rm -f "$LOG"
}
trap cleanup EXIT INT TERM

echo "e2e: booting under PTY, waiting up to ${TIMEOUT}s for: '${MARKER}'"

# BSD `script` syntax: script [-q] <logfile> <command ...>. Runs the command with
# a pseudo-terminal and records everything written to the tty into <logfile>.
script -q "$LOG" "$BIN" --log-level 0 firmware \
  --firmware "$FIRMWARE" --disk "$DISK" --cpus "$CPUS" --mem "$MEM" \
  >/dev/null 2>&1 &
SPID=$!

deadline=$(( SECONDS + TIMEOUT ))
found=0
while [ "$SECONDS" -lt "$deadline" ]; do
  if LC_ALL=C grep -qa "$MARKER" "$LOG" 2>/dev/null; then
    found=1
    break
  fi
  # If the VM process died before the marker showed, stop waiting — it crashed
  # or failed to create the VM (e.g. no nested virt / missing entitlement).
  if ! kill -0 "$SPID" 2>/dev/null; then
    echo "e2e: VM process exited before reaching the marker" >&2
    break
  fi
  sleep 2
done

echo "--- console output (ANSI stripped, last 40 lines) ---"
LC_ALL=C sed $'s/\x1b\\[[0-9;?=]*[A-Za-z]//g; s/\x1b[()][B0]//g' "$LOG" \
  | tr '\r' '\n' | grep -aE '[[:print:]]' | tail -40

if [ "$found" -eq 1 ]; then
  echo "e2e: PASS — reached '${MARKER}'"
  exit 0
fi

echo "e2e: FAIL — '${MARKER}' not seen within ${TIMEOUT}s" >&2
exit 1
