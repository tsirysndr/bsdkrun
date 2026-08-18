// Package rust is the rust provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package rust

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "rust" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "Cargo.toml") {
		return "Cargo.toml", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	var steps []platforms.Step
	if probe.Exists(dir, "tests") || probe.SourceMentions(dir, "src", ".rs", "#[test]", "#[cfg(test)]") {
		steps = append(steps, step("cargo test", "cargo test"))
	}
	steps = append(steps, step("cargo build", "cargo build --all-targets"))
	return job("rust:1-slim", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
