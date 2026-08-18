// Package elixir is the elixir provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package elixir

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "elixir" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "mix.exs") {
		return "mix.exs", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	return job("elixir:1.17",
		step("deps", "mix local.hex --force && mix local.rebar --force && mix deps.get"),
		step("mix test", "mix test"),
	)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
