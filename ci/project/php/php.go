// Package php is the php provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package php

import (
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "php" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "composer.json") {
		return "composer.json", true
	}
	return "", false
}

func hasComposerScript(dir, name string) bool {
	data := filepath.Join(dir, "composer.json")
	if !probe.FileContains(data, "\"scripts\"") {
		return false
	}
	return probe.FileContains(data, "\""+name+"\"")
}

func (Provider) Job(dir string) platforms.Job {
	// composer:2 is the official image that actually carries composer.
	steps := []platforms.Step{
		step("composer install", "composer install --no-interaction --no-progress"),
	}
	switch {
	case hasComposerScript(dir, "test"):
		steps = append(steps, step("composer test", "composer test"))
	case probe.Exists(dir, "phpunit.xml") || probe.Exists(dir, "phpunit.xml.dist"):
		steps = append(steps, step("phpunit", "vendor/bin/phpunit"))
	}
	return job("composer:2", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
