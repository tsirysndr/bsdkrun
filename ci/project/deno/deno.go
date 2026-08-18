// Package deno is the deno provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package deno

import (
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "deno" }

func (Provider) Detect(dir string) (string, bool) {
	for _, m := range []string{"deno.json", "deno.jsonc"} {
		if probe.Exists(dir, m) {
			return m, true
		}
	}
	return "", false
}

func hasTask(dir, name string) bool {
	for _, f := range []string{"deno.json", "deno.jsonc"} {
		if probe.FileContains(filepath.Join(dir, f), "\""+name+"\":") {
			return true
		}
	}
	return false
}

func (Provider) Job(dir string) platforms.Job {
	var steps []platforms.Step
	switch {
	case hasTask(dir, "test"):
		steps = append(steps, step("deno task test", "deno task test"))
	case probe.HasFile(dir, probe.Infix("_test.", ".test.")) || probe.Exists(dir, "tests"):
		steps = append(steps, step("deno test", "deno test -A"))
	default:
		steps = append(steps, step("deno check", "deno check ."))
	}
	return job("denoland/deno:alpine", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
