// Package project detects what a repository *is* — a Go module, a bun app,
// a mix project — and generates a runnable CI workflow for it on the fly:
// railpack's move, applied to CI. It only speaks when asked: the runner
// consults it when a directory has no recognizable CI configuration at all,
// or when --detect forces it.
//
// One provider per language, one subpackage per provider — the same shape
// pack's providers (and railpack's core/providers) use. Detection order is
// specific-before-broad exactly as there: bun's lockfile before
// package.json, composer.json before package.json (a Laravel app carries
// both and is a PHP project). Each provider names an official image and
// emits install/test/build steps — tests run before the build, and a step
// that would fail vacuously (a test runner with no tests) is only generated
// when its subject exists: a green run must mean something.
package project

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/bun"
	"github.com/tsirysndr/bsdkrun/ci/project/clojure"
	"github.com/tsirysndr/bsdkrun/ci/project/crystal"
	"github.com/tsirysndr/bsdkrun/ci/project/deno"
	"github.com/tsirysndr/bsdkrun/ci/project/dotnet"
	"github.com/tsirysndr/bsdkrun/ci/project/elixir"
	"github.com/tsirysndr/bsdkrun/ci/project/gleam"
	"github.com/tsirysndr/bsdkrun/ci/project/golang"
	"github.com/tsirysndr/bsdkrun/ci/project/haskell"
	"github.com/tsirysndr/bsdkrun/ci/project/node"
	"github.com/tsirysndr/bsdkrun/ci/project/php"
	"github.com/tsirysndr/bsdkrun/ci/project/python"
	"github.com/tsirysndr/bsdkrun/ci/project/ruby"
	"github.com/tsirysndr/bsdkrun/ci/project/rust"
	"github.com/tsirysndr/bsdkrun/ci/project/zig"
)

// Provider detects one language and generates its workflow. Mirrors pack's
// Provider interface, minus the parts that only make sense for unikernel
// builds.
type Provider interface {
	// Name identifies the provider, e.g. "go".
	Name() string
	// Detect reports whether this provider claims dir, and which marker
	// file gave it away.
	Detect(dir string) (marker string, ok bool)
	// Job generates the CI job for dir.
	Job(dir string) platforms.Job
}

// Project is a detection result: the language, the marker that gave it
// away, and the generated job.
type Project struct {
	Language string
	Marker   string
	Jobs     []platforms.Job
}

// All returns every provider in detection order. The first whose Detect
// returns true wins.
func All() []Provider {
	return []Provider{
		bun.Provider{},
		deno.Provider{},
		php.Provider{},
		node.Provider{},
		golang.Provider{},
		rust.Provider{},
		gleam.Provider{},
		elixir.Provider{},
		zig.Provider{},
		ruby.Provider{},
		python.Provider{},
		clojure.Provider{},
		crystal.Provider{},
		haskell.Provider{},
		dotnet.Provider{},
	}
}

// Detect returns the first matching provider's generated workflow, or nil
// when the directory resembles nothing we know.
func Detect(root string) *Project {
	for _, p := range All() {
		if marker, ok := p.Detect(root); ok {
			return &Project{
				Language: p.Name(),
				Marker:   marker,
				Jobs:     []platforms.Job{p.Job(root)},
			}
		}
	}
	return nil
}

// Languages lists what Detect can recognize, for help text and errors.
func Languages() []string {
	defs := All()
	out := make([]string, len(defs))
	for i, p := range defs {
		out[i] = p.Name()
	}
	return out
}
