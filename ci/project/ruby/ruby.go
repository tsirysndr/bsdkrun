// Package ruby is the ruby provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package ruby

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "ruby" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "Gemfile") {
		return "Gemfile", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	steps := []platforms.Step{step("bundle install", "bundle install")}
	switch {
	case probe.Exists(dir, "spec"):
		steps = append(steps, step("rspec", "bundle exec rspec"))
	case probe.Exists(dir, "Rakefile") && probe.Exists(dir, "test"):
		steps = append(steps, step("rake test", "bundle exec rake test"))
	}
	return job("ruby:3.3-slim", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
