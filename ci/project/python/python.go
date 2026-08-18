// Package python is the python provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package python

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "python" }

func (Provider) Detect(dir string) (string, bool) {
	for _, m := range []string{"pyproject.toml", "requirements.txt", "setup.py"} {
		if probe.Exists(dir, m) {
			return m, true
		}
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	install := "pip install -r requirements.txt"
	if probe.Exists(dir, "pyproject.toml") || (probe.Exists(dir, "setup.py") && !probe.Exists(dir, "requirements.txt")) {
		install = "pip install ."
	}
	steps := []platforms.Step{step("install dependencies", install)}
	if probe.Exists(dir, "tests") || probe.HasFile(dir, probe.Infix("test_")) {
		steps = append(steps, step("test", "pip install pytest && python -m pytest"))
	} else {
		steps = append(steps, step("compile check", "python -m compileall -q ."))
	}
	return job("python:3.12-slim", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
