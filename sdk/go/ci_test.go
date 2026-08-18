package bsdkrun

import (
	"strings"
	"testing"
)

// The YAML this builder emits is consumed by tangled's own workflow parser
// (inside `bsdkrun ci`), so these tests pin the emitted shape — a change here
// is a change to what spindle would receive.

func TestCIWorkflowYAML(t *testing.T) {
	got := Workflow("test").
		OnPush("main").
		OnPullRequest("main", "develop").
		Deps("go", "gcc").
		DepsFrom("github:nix-community/fenix/abc123", "stable.defaultToolchain").
		Env("CGO_ENABLED", "1").
		CloneDepth(100).
		Step("vet", "go vet ./...").
		StepEnv("test", "go test ./...", map[string]string{"GOFLAGS": "-count=1"}).
		YAML()

	want := `when:
  - event: ["push"]
    branch: "main"
  - event: ["pull_request"]
    branch: ["main", "develop"]

engine: nixery

dependencies:
  "github:nix-community/fenix/abc123":
    - "stable.defaultToolchain"
  "nixpkgs":
    - "go"
    - "gcc"

environment:
  CGO_ENABLED: "1"

clone:
  depth: 100

steps:
  - name: "vet"
    command: |
      go vet ./...
  - name: "test"
    command: |
      go test ./...
    environment:
      GOFLAGS: "-count=1"
`
	if got != want {
		t.Errorf("YAML mismatch:\n--- got ---\n%s\n--- want ---\n%s", got, want)
	}
}

func TestCIWorkflowCommandFallsBackToJSONWhenBlockUnsafe(t *testing.T) {
	// Trailing spaces do not survive a literal block scalar; the emitter must
	// notice and switch representation rather than silently altering the
	// command.
	got := Workflow("edge").Step("tricky", "echo 'a'  \necho b").YAML()
	if !strings.Contains(got, `command: "echo 'a'  \necho b"`) {
		t.Errorf("expected a JSON-string command for block-unsafe content, got:\n%s", got)
	}
}

func TestCIWorkflowFileName(t *testing.T) {
	if got := Workflow("build").FileName(); got != "build.yml" {
		t.Errorf("FileName() = %q", got)
	}
	if got := Workflow("build.yaml").FileName(); got != "build.yaml" {
		t.Errorf("FileName() = %q", got)
	}
}
