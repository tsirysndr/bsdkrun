// Package gleam is the gleam provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package gleam

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "gleam" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "gleam.toml") {
		return "gleam.toml", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	steps := []platforms.Step{step("deps", "gleam deps download")}
	// `gleam new` scaffolds test/ with a test main; only ask when it exists.
	if probe.Exists(dir, "test") {
		steps = append(steps, step("gleam test", "gleam test"))
	}
	steps = append(steps, step("gleam build", "gleam build"))
	// ghcr tags are version-pinned (no floating latest); pinned tags stay
	// pullable forever, they just age.
	return job("ghcr.io/gleam-lang/gleam:v1.18.0-erlang-alpine", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
