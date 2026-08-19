// Package haskell is the haskell provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package haskell

import (
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "haskell" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "stack.yaml") {
		return "stack.yaml", true
	}
	if m := probe.GlobFirst(dir, "*.cabal"); m != "" {
		return m, true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	if probe.Exists(dir, "stack.yaml") {
		// --system-ghc: the image ships GHC; letting stack download its own
		// would multiply the run by gigabytes.
		return job("haskell:9.6-slim",
			step("stack build", "stack build --system-ghc --allow-different-user --no-terminal --test"))
	}
	steps := []platforms.Step{step("cabal update", "cabal update")}
	if cabalHasTests(dir) {
		steps = append(steps, step("cabal test", "cabal test all"))
	}
	steps = append(steps, step("cabal build", "cabal build all"))
	return job("haskell:9.6-slim", steps...)
}

func cabalHasTests(dir string) bool {
	names, _ := filepath.Glob(filepath.Join(dir, "*.cabal"))
	for _, n := range names {
		if probe.FileContains(n, "test-suite") {
			return true
		}
	}
	return false
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
