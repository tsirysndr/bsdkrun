// Package crystal is the crystal provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package crystal

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "crystal" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "shard.yml") {
		return "shard.yml", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	steps := []platforms.Step{step("shards install", "shards install")}
	if probe.Exists(dir, "spec") {
		steps = append(steps, step("crystal spec", "crystal spec"))
	}
	steps = append(steps, step("shards build", "shards build"))
	return job("crystallang/crystal:latest", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
