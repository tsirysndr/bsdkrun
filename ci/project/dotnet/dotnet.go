// Package dotnet is the dotnet provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package dotnet

import (
	"strings"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "dotnet" }

func (Provider) Detect(dir string) (string, bool) {
	found := ""
	probe.HasFile(dir, func(name string) bool {
		if found == "" && (strings.HasSuffix(name, ".sln") ||
			strings.HasSuffix(name, ".csproj") || strings.HasSuffix(name, ".fsproj")) {
			found = name
		}
		return found != ""
	})
	return found, found != ""
}

func (Provider) Job(dir string) platforms.Job {
	steps := []platforms.Step{step("restore", "dotnet restore")}
	// Projects named *Test* are the convention every dotnet template uses.
	if probe.HasFile(dir, func(name string) bool {
		return (strings.HasSuffix(name, ".csproj") || strings.HasSuffix(name, ".fsproj")) &&
			strings.Contains(strings.ToLower(name), "test")
	}) {
		steps = append(steps, step("dotnet test", "dotnet test --no-restore"))
	}
	steps = append(steps, step("dotnet build", "dotnet build --no-restore -c Release"))
	return job("mcr.microsoft.com/dotnet/sdk:8.0", steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
