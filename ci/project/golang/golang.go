// Package golang is the go provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package golang

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "go" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "go.mod") {
		return "go.mod", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	var steps []platforms.Step
	if probe.HasFile(dir, probe.Suffix("_test.go")) {
		steps = append(steps, step("go test", "go test ./..."))
	}
	steps = append(steps, step("go build", "go build ./..."))
	j := job("golang:1.23-alpine", steps...)
	// Pure-Go by default: alpine has no gcc, and most modules need none.
	j.Env = map[string]string{"CGO_ENABLED": "0"}
	return j
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
