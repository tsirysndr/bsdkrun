// Package node is the nodejs provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package node

import (
	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "nodejs" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "package.json") {
		return "package.json", true
	}
	return "", false
}

func (Provider) Job(dir string) platforms.Job {
	install := "npm install"
	switch {
	case probe.Exists(dir, "pnpm-lock.yaml"):
		install = "corepack enable && pnpm install --frozen-lockfile"
	case probe.Exists(dir, "yarn.lock"):
		install = "corepack enable && yarn install --frozen-lockfile"
	case probe.Exists(dir, "package-lock.json"):
		install = "npm ci"
	}
	steps := []platforms.Step{step("install dependencies", install)}
	if probe.HasPackageScript(dir, "test") ||
		probe.HasFile(dir, probe.Infix(".test.", ".spec.", "_test.")) {
		steps = append(steps, step("test", "npm test"))
	}
	if probe.HasPackageScript(dir, "build") {
		steps = append(steps, step("build", "npm run build"))
	}
	return job("node:24-alpine", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
