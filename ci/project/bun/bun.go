// Package bun is the bun provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package bun

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "bun" }

func (Provider) Detect(dir string) (string, bool) {
	for _, m := range []string{"bun.lockb", "bun.lock", "bunfig.toml"} {
		if probe.Exists(dir, m) {
			return m, true
		}
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	var steps []platforms.Step
	// A bunfig.toml-only project (zero dependencies) has nothing to
	// install, and `bun install` *fails* without a package.json.
	if probe.Exists(dir, "package.json") {
		steps = append(steps, step("bun install", "bun install"))
	}
	// `bun test` with nothing to run exits non-zero; only ask when tests
	// exist or the project declares a test script.
	switch {
	case probe.HasPackageScript(dir, "test"):
		steps = append(steps, step("bun run test", "bun run test"))
	case probe.HasFile(dir, probe.Infix(".test.", ".spec.", "_test.")) ||
		probe.Exists(dir, "test") || probe.Exists(dir, "tests"):
		steps = append(steps, step("bun test", "bun test"))
	}
	return job("oven/bun:1-alpine", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
