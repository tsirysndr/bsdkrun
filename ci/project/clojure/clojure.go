// Package clojure is the clojure provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package clojure

import (
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "clojure" }

func (Provider) Detect(dir string) (string, bool) {
	for _, m := range []string{"deps.edn", "project.clj"} {
		if probe.Exists(dir, m) {
			return m, true
		}
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	if probe.Exists(dir, "deps.edn") {
		steps := []platforms.Step{step("deps", "clojure -P")}
		// `clojure -X:test` needs a :test alias; running it without one is
		// an error about our guess, not about the project.
		if probe.FileContains(filepath.Join(dir, "deps.edn"), ":test") {
			steps = append(steps, step("clojure test", "clojure -X:test"))
		}
		return job("clojure:temurin-21-tools-deps", steps...)
	}
	steps := []platforms.Step{step("deps", "lein deps")}
	if probe.Exists(dir, "test") {
		steps = append(steps, step("lein test", "lein test"))
	}
	return job("clojure:temurin-21-lein", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
