// Package zig is the zig provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package zig

import (
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "zig" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "build.zig") {
		return "build.zig", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	// No official zig image exists; alpine's community repo (enabled by
	// default in the official image) carries a current zig, and installing
	// it is a visible step rather than hidden image magic.
	steps := []platforms.Step{step("install zig", "apk add --no-cache zig")}
	// `zig build test` fails when build.zig declares no test step; the
	// scaffold declares one, hand-written files may not.
	if probe.FileContains(filepath.Join(dir, "build.zig"), "step(\"test\"") ||
		probe.FileContains(filepath.Join(dir, "build.zig"), "addTest") {
		steps = append(steps, step("zig build test", "zig build test"))
	}
	steps = append(steps, step("zig build", "zig build"))
	return job("alpine:3.21", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
