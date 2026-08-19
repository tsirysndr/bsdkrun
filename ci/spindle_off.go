//go:build !spindle

package main

// The spindle-compatible server is a build-tag feature, and this is the half
// that exists when it was not built in.
//
// Why it is optional at all: spindle's storage layer is SQLite through
// mattn/go-sqlite3, which needs cgo. The `ci` binary embedded in bsdkrun is
// built with CGO_ENABLED=0 on purpose — a pure-Go binary runs on any host,
// musl included, and `bsdkrun ci run` should not acquire a libc dependency for
// a server feature most users never start. Building with `-tags spindle`
// (and cgo on) turns it on.
//
// The failure mode this avoids is the one worth naming: a flag that is
// accepted and does nothing. `--spindle` on a build without it says so, and
// says how to get one.

import (
	"errors"
	"net/http"
)

// serveConfig is what `ci serve` hands the spindle half.
type serveConfig struct {
	Cpus int
	Mem  int
}

// spindleHandle is what it gets back: routes to mount, plus what to print.
type spindleHandle interface {
	Register(mux *http.ServeMux)
	ListenAddr() string
	Banner() []string
}

const spindleBuilt = false

var errNoSpindle = errors.New(
	"this bsdkrun was built without spindle support\n" +
		"  rebuild with:  BSDKRUN_CI_SPINDLE=1 cargo build --release\n" +
		"  (it needs cgo — spindle's SQLite storage is mattn/go-sqlite3)\n" +
		"  see ci/README.md § Self-hosting a spindle")

// startSpindle is the seam serve.go calls; without the tag it only explains.
func startSpindle(_ *serveConfig) (spindleHandle, error) { return nil, errNoSpindle }
