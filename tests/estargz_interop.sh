#!/usr/bin/env bash
#
# Verify that `--compression estargz` produces an archive containerd can read.
#
# Our own tests can only prove the writer agrees with itself. eStargz exists to
# be consumed by stargz-snapshotter, so the test that matters opens the archive
# with *that* library: parse the 51-byte footer, follow it to the TOC, then seek
# to each entry's recorded offset and check the bytes there are the file's — not
# its tar header, which is the mistake the format invites.
#
# Needs Go and network access (it fetches the estargz module). Exit 0 on
# success, 1 on failure, 2 on missing prerequisites. Overridable via:
#   BSDKRUN_BIN  (default target/debug/bsdkrun)
#   IMAGE        (default "alpine")
#   TIMEOUT      (seconds per boot, default 180)
set -uo pipefail

BIN="${BSDKRUN_BIN:-target/debug/bsdkrun}"
IMAGE="${IMAGE:-alpine}"
TIMEOUT="${TIMEOUT:-180}"
NAME="e2e-estargz-$$"
KEY="e2e-estargz-$$"

if [ ! -x "$BIN" ]; then
  echo "interop: missing bsdkrun binary: $BIN (run 'make build' first)" >&2
  exit 2
fi
command -v go >/dev/null 2>&1 || { echo "interop: go is not installed" >&2; exit 2; }

ID=""
WORK="$(mktemp -d -t bsdkrun-estargz.XXXXXX)"

cleanup() {
  "$BIN" cache rm "$KEY" >/dev/null 2>&1
  [ -n "$ID" ] && "$BIN" rm -f "$ID" >/dev/null 2>&1
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

fail() { echo "interop: FAIL — $*" >&2; exit 1; }

wait_for_agent() {
  local deadline=$(( SECONDS + TIMEOUT ))
  while [ "$SECONDS" -lt "$deadline" ]; do
    "$BIN" exec "$ID" /bin/true >/dev/null 2>&1 && return 0
    sleep 2
  done
  fail "guest agent not reachable within ${TIMEOUT}s"
}

# The disk backend, so the archive is a file we can hand to Go. An S3 store
# would work too but would only add a download to the middle of the test.
unset BSDKRUN_CACHE_BACKEND

echo "interop: booting $IMAGE"
ID="$("$BIN" linux -d --name "$NAME" "$IMAGE" 2>/dev/null </dev/null | tail -n 1)"
[ -n "$ID" ] || fail "bsdkrun did not return a machine id"
wait_for_agent

"$BIN" exec "$ID" sh -c '
  set -e
  mkdir -p /work/tree/nested/deep
  printf "hello from the cache\n" > /work/tree/main.txt
  printf "deep\n" > /work/tree/nested/deep/notes.txt
  head -c 4096 /dev/urandom > /work/tree/data.bin
' >/dev/null 2>&1 || fail "could not create the source tree in the guest"

"$BIN" cache save "$ID:/work/tree" --key "$KEY" --compression estargz >/dev/null 2>&1 \
  || fail "cache save --compression estargz failed"

# Find the archive the save just wrote. `cache ls --json` names the key; the
# file is the only .tar.estargz in the store whose name derives from it.
ARCHIVE="$(find "${HOME}/.cache/bsdkrun/caches" -name '*.tar.estargz' -newermt '-5 minutes' 2>/dev/null | head -n 1)"
[ -n "$ARCHIVE" ] || fail "could not find the saved estargz archive"
echo "interop: checking $ARCHIVE"

cat > "$WORK/main.go" <<'GO'
package main

import (
	"fmt"
	"io"
	"os"

	"github.com/containerd/stargz-snapshotter/estargz"
)

// The three files the shell half wrote, with the sizes it wrote them at.
var want = map[string]string{
	"main.txt":                "hello from the cache\n",
	"nested/deep/notes.txt":   "deep\n",
}

func main() {
	f, err := os.Open(os.Args[1])
	if err != nil {
		fmt.Println("FAIL open:", err)
		os.Exit(1)
	}
	defer f.Close()
	fi, err := f.Stat()
	if err != nil {
		fmt.Println("FAIL stat:", err)
		os.Exit(1)
	}

	// Parses the footer at len-51, follows it to the TOC, and validates it.
	r, err := estargz.Open(io.NewSectionReader(f, 0, fi.Size()))
	if err != nil {
		fmt.Println("FAIL estargz.Open:", err)
		os.Exit(1)
	}
	fmt.Println("ok   estargz.Open — footer and TOC parsed")

	for name, body := range want {
		e, ok := r.Lookup(name)
		if !ok {
			fmt.Println("FAIL Lookup:", name)
			os.Exit(1)
		}
		sr, err := r.OpenFile(name)
		if err != nil {
			fmt.Println("FAIL OpenFile:", name, err)
			os.Exit(1)
		}
		buf := make([]byte, e.Size)
		if _, err := sr.ReadAt(buf, 0); err != nil && err != io.EOF {
			fmt.Println("FAIL ReadAt:", name, err)
			os.Exit(1)
		}
		if string(buf) != body {
			fmt.Printf("FAIL %s: read %q at offset %d, want %q\n", name, buf, e.Offset, body)
			os.Exit(1)
		}
		fmt.Printf("ok   %-24s %d bytes at offset %d\n", name, e.Size, e.Offset)
	}

	// A binary file too: the seek arithmetic is what breaks, and it breaks the
	// same way for text — just less visibly.
	if e, ok := r.Lookup("data.bin"); !ok || e.Size != 4096 {
		fmt.Println("FAIL data.bin missing or the wrong size in the TOC")
		os.Exit(1)
	}
	fmt.Println("ok   data.bin present at 4096 bytes")

	// The landmark the spec requires; without it a verifying reader rejects
	// the archive outright.
	if _, ok := r.Lookup(".no.prefetch.landmark"); !ok {
		fmt.Println("FAIL the required landmark entry is missing")
		os.Exit(1)
	}
	fmt.Println("ok   .no.prefetch.landmark present")
}
GO

cat > "$WORK/go.mod" <<'GO'
module estargzinterop

go 1.23
GO

( cd "$WORK" && go mod tidy >/dev/null 2>&1 && go run . "$ARCHIVE" ) || fail "containerd's reader rejected the archive"

# ...and it must still be an ordinary tar.gz to anything that has never heard of
# eStargz. That property is the reason the format is safe to make the default
# one day, and it is easy to lose.
tar -tzf "$ARCHIVE" >/dev/null 2>&1 || fail "the archive is not readable as a plain tar.gz"
echo "interop: plain 'tar -tzf' reads it too"

echo "interop: PASS"
