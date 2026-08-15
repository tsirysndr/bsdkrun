#!/usr/bin/env bash
#
# End-to-end test for `bsdkrun cache`: save a guest directory under a key and
# restore it into a different path, byte for byte, in every archive format —
# against both backends.
#
# The disk backend needs nothing. The S3 backend runs against a real MinIO,
# because that is the half that unit tests cannot reach: SigV4 either signs a
# request a server accepts or it does not, and the failure is a flat 403 with
# no hint as to which of the five derivation steps drifted. It also pins the
# behaviour that broke on first contact with a real bucket — a 404 from the
# "is this key already cached?" probe is an *answer*, not an error.
#
# MinIO is optional: without S3_ENDPOINT the S3 half is skipped and the disk
# half still runs, so the script is useful on a laptop.
#
# Exit 0 on success, 1 on failure, 2 on missing prerequisites. Overridable via
# environment:
#   BSDKRUN_BIN  (default target/debug/bsdkrun — build + sign with `make build`)
#   IMAGE        (default "alpine")
#   TIMEOUT      (seconds per boot, default 180)
#   S3_ENDPOINT  (e.g. http://127.0.0.1:19000; unset skips the S3 half)
#   S3_BUCKET    (default bsdkrun-cache)
set -uo pipefail

BIN="${BSDKRUN_BIN:-target/debug/bsdkrun}"
IMAGE="${IMAGE:-alpine}"
TIMEOUT="${TIMEOUT:-180}"
NAME="e2e-cache-$$"
S3_BUCKET="${S3_BUCKET:-bsdkrun-cache}"

if [ ! -x "$BIN" ]; then
  echo "e2e: missing bsdkrun binary: $BIN (run 'make build' first)" >&2
  exit 2
fi

ID=""
# Every key this run creates, so a failure part-way still cleans the store.
KEYS=()

cleanup() {
  for key in "${KEYS[@]:-}"; do
    [ -n "$key" ] && "$BIN" cache rm "$key" >/dev/null 2>&1
  done
  [ -n "$ID" ] && "$BIN" rm -f "$ID" >/dev/null 2>&1
}
trap cleanup EXIT INT TERM

fail() {
  echo "e2e: FAIL — $*" >&2
  [ -n "$ID" ] && "$BIN" logs "$ID" 2>/dev/null | tail -20 >&2
  exit 1
}

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

# The manifest of a directory in the guest: every file's path and sha256. This
# is what "restored correctly" means — a tree that merely has the right *names*
# has silently lost content, and a tree with an extra file (estargz's TOC and
# landmark are real tar members) has silently gained one.
manifest() {
  "$BIN" exec "$ID" sh -c "cd $1 && find . -type f | sort | xargs sha256sum" 2>/dev/null
}

echo "e2e: booting $IMAGE"
ID="$("$BIN" linux -d --name "$NAME" "$IMAGE" 2>/dev/null </dev/null | tail -n 1)"
[ -n "$ID" ] || fail "bsdkrun did not return a machine id"
wait_for_agent

# A tree with a nested directory, a text file and 256 KiB of incompressible
# data — so a format that quietly truncates or reorders shows up.
"$BIN" exec "$ID" sh -c '
  set -e
  mkdir -p /work/deps/nested/deep
  echo lib-a > /work/deps/a.txt
  echo lib-b > /work/deps/nested/deep/b.txt
  head -c 262144 /dev/urandom > /work/deps/blob.bin
' >/dev/null 2>&1 || fail "could not create the source tree in the guest"

ORIGINAL="$(manifest /work/deps)"
[ -n "$ORIGINAL" ] || fail "the source tree came back empty"

# ---------------------------------------------------------------------------
# One backend, every format.
# ---------------------------------------------------------------------------
run_backend() {
  local label="$1"
  echo
  echo "e2e: === $label backend ==="

  local fmt key dest restored
  for fmt in gzip zstd estargz none; do
    key="e2e-$$-$fmt"
    dest="/restored-$fmt"
    KEYS+=("$key")

    "$BIN" cache save "$ID:/work/deps" --key "$key" --compression "$fmt" >/dev/null 2>&1 \
      || fail "$label: cache save failed for $fmt"

    "$BIN" exec "$ID" rm -rf "$dest" >/dev/null 2>&1
    "$BIN" cache restore "$ID:$dest" --key "$key" >/dev/null 2>&1 \
      || fail "$label: cache restore failed for $fmt"

    restored="$(manifest "$dest")"
    if [ "$restored" != "$ORIGINAL" ]; then
      echo "--- expected ---" >&2; echo "$ORIGINAL" >&2
      echo "--- restored ---" >&2; echo "$restored" >&2
      fail "$label: $fmt did not round-trip"
    fi
    echo "e2e: $label/$fmt round-tripped"
  done

  # `ls` has to show what we just saved.
  "$BIN" cache ls | grep -q "e2e-$$-gzip" \
    || fail "$label: cache ls does not list the entry it just saved"

  # A miss is an ordinary answer, not a failure — a first CI run depends on it.
  if ! "$BIN" cache restore "$ID:/miss" --key "definitely-absent-$$" >/dev/null 2>&1; then
    fail "$label: a cache miss exited non-zero"
  fi

  # Restore-keys: an exact miss falls back to a prefix.
  "$BIN" exec "$ID" rm -rf /fallback >/dev/null 2>&1
  "$BIN" cache restore "$ID:/fallback" --key "e2e-$$-nope" --restore-keys "e2e-$$-" >/dev/null 2>&1 \
    || fail "$label: restore-keys fallback failed"
  [ -n "$(manifest /fallback)" ] || fail "$label: restore-keys fallback restored nothing"
  echo "e2e: $label restore-keys fallback works"

  # Saving over an existing key needs --force. This is the path that issues the
  # "does this key exist?" probe, whose 404 must not read as an error.
  if "$BIN" cache save "$ID:/work/deps" --key "e2e-$$-gzip" >/dev/null 2>&1; then
    fail "$label: saving over an existing key succeeded without --force"
  fi
  "$BIN" cache save "$ID:/work/deps" --key "e2e-$$-gzip" --force >/dev/null 2>&1 \
    || fail "$label: --force did not replace the entry"
  echo "e2e: $label duplicate-key handling works"

  # Removal really removes — and a removed key must also stop resolving, which
  # is what proves the archive went with the metadata rather than being orphaned
  # in the store where no listing would ever show it again.
  "$BIN" cache rm "e2e-$$-gzip" >/dev/null 2>&1 || fail "$label: cache rm failed"
  if "$BIN" cache ls | grep -q "e2e-$$-gzip"; then
    fail "$label: cache rm left the entry listed"
  fi
  "$BIN" exec "$ID" rm -rf /gone >/dev/null 2>&1
  "$BIN" cache restore "$ID:/gone" --key "e2e-$$-gzip" >/dev/null 2>&1 \
    || fail "$label: restoring a removed key errored instead of missing"
  if [ -n "$(manifest /gone)" ]; then
    fail "$label: a removed key still restored content"
  fi
  echo "e2e: $label removal works"

  # Clear the rest of this run's keys, so the next backend starts clean and the
  # store is left as we found it.
  for fmt in zstd estargz none; do
    "$BIN" cache rm "e2e-$$-$fmt" >/dev/null 2>&1
  done
  if "$BIN" cache ls | grep -q "e2e-$$-"; then
    fail "$label: entries from this run survived removal"
  fi
  echo "e2e: $label store is clean"
}

# ---------------------------------------------------------------------------
# Disk backend (the default).
# ---------------------------------------------------------------------------
unset BSDKRUN_CACHE_BACKEND
run_backend "disk"

# ---------------------------------------------------------------------------
# S3 backend, against MinIO when one is reachable.
# ---------------------------------------------------------------------------
if [ -z "${S3_ENDPOINT:-}" ]; then
  echo
  echo "e2e: S3_ENDPOINT unset — skipping the S3 half"
else
  export BSDKRUN_CACHE_BACKEND=s3
  export BSDKRUN_CACHE_S3_ENDPOINT="$S3_ENDPOINT"
  export BSDKRUN_CACHE_S3_BUCKET="$S3_BUCKET"
  export BSDKRUN_CACHE_S3_REGION="${AWS_REGION:-us-east-1}"
  export BSDKRUN_CACHE_S3_PREFIX="e2e"
  : "${AWS_ACCESS_KEY_ID:?e2e: S3_ENDPOINT is set but AWS_ACCESS_KEY_ID is not}"
  : "${AWS_SECRET_ACCESS_KEY:?e2e: S3_ENDPOINT is set but AWS_SECRET_ACCESS_KEY is not}"

  run_backend "s3"
fi

echo
echo "e2e: PASS"
