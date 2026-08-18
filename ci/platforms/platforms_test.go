package platforms

import (
	"os"
	"path/filepath"
	"testing"
)

func write(t *testing.T, root, rel, content string) {
	t.Helper()
	path := filepath.Join(root, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

var testRepo = Repo{Sha: "abc123", Branch: "main", DefaultBranch: "main", Name: "proj", Workspace: "/tangled/workspace"}

func TestGithubTranslation(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".github/workflows/ci.yml", `
name: CI
env: {TOP: "1"}
jobs:
  build:
    runs-on: ubuntu-latest
    container: node:22
    env: {JOB: "2"}
    steps:
      - uses: actions/checkout@v4
      - name: install
        run: npm ci
      - uses: actions/setup-node@v4
      - run: |
          npm test
        working-directory: web
  windows:
    runs-on: windows-latest
    steps: [{run: "echo hi"}]
  after:
    needs: [build, windows]
    runs-on: ubuntu-latest
    steps: [{run: "echo done"}]
`)
	if !detectGithub(root) {
		t.Fatal("github not detected")
	}
	jobs, err := loadGithub(root, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 3 {
		t.Fatalf("want 3 jobs, got %d", len(jobs))
	}
	// needs-order: build and windows before after.
	if jobs[2].Name != "after" {
		t.Fatalf("needs order broken: %v", []string{jobs[0].Name, jobs[1].Name, jobs[2].Name})
	}
	b := jobs[0]
	if b.Image != "node:22" {
		t.Fatalf("container image lost: %q", b.Image)
	}
	if b.Env["TOP"] != "1" || b.Env["JOB"] != "2" {
		t.Fatalf("env merge wrong: %v", b.Env)
	}
	// checkout no-ops, setup-node is a visible skip, run steps translate.
	if len(b.Steps) != 4 {
		t.Fatalf("want 4 steps, got %d: %+v", len(b.Steps), b.Steps)
	}
	if b.Steps[0].Command != "true" {
		t.Fatalf("checkout should be a no-op: %+v", b.Steps[0])
	}
	if b.Steps[1].Command != "npm ci" {
		t.Fatalf("run step lost: %+v", b.Steps[1])
	}
	if want := `echo "skipped uses: actions/setup-node@v4 — actions are not supported locally"`; b.Steps[2].Command != want {
		t.Fatalf("action skip not visible: %+v", b.Steps[2])
	}
	for _, j := range jobs {
		if j.Name == "windows" && j.SkipReason == "" {
			t.Fatal("windows job not skipped")
		}
	}
}

func TestGitlabTranslation(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".gitlab-ci.yml", `
stages: [lint, test]
variables: {TOP: "1"}
image: alpine:3.20
test-job:
  stage: test
  variables: {JOB: "2"}
  script:
    - go test ./...
lint-job:
  stage: lint
  image: golangci/golangci-lint
  before_script:
    - echo before
  script: golangci-lint run
.mixin:
  script: echo hidden
`)
	if !detectGitlab(root) {
		t.Fatal("gitlab not detected")
	}
	jobs, err := loadGitlab(root, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 2 {
		t.Fatalf("want 2 jobs (hidden excluded), got %d", len(jobs))
	}
	if jobs[0].Name != "lint-job" || jobs[1].Name != "test-job" {
		t.Fatalf("stage order broken: %s, %s", jobs[0].Name, jobs[1].Name)
	}
	if jobs[0].Image != "golangci/golangci-lint" {
		t.Fatalf("job image lost: %q", jobs[0].Image)
	}
	if jobs[1].Image != "alpine:3.20" {
		t.Fatalf("default image lost: %q", jobs[1].Image)
	}
	if jobs[1].Env["TOP"] != "1" || jobs[1].Env["JOB"] != "2" {
		t.Fatalf("variables merge wrong: %v", jobs[1].Env)
	}
	if len(jobs[0].Steps) != 2 || jobs[0].Steps[0].Name != "before_script" {
		t.Fatalf("before_script lost: %+v", jobs[0].Steps)
	}
}

func TestWoodpeckerAndDrone(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".woodpecker/build.yml", `
steps:
  - name: test
    image: golang:1.22
    commands:
      - go vet ./...
      - go test ./...
    environment:
      FOO: bar
  - name: publish
    image: plugins/docker
    settings: {repo: x}
`)
	if !detectWoodpecker(root) {
		t.Fatal("woodpecker not detected")
	}
	jobs, err := loadWoodpecker(root, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 1 {
		t.Fatalf("want 1 pipeline, got %d", len(jobs))
	}
	j := jobs[0]
	if j.Image != "golang:1.22" {
		t.Fatalf("image: %q", j.Image)
	}
	// image-divergence notice + test step + plugin skip
	if len(j.Steps) != 3 {
		t.Fatalf("steps: %+v", j.Steps)
	}
	if j.Steps[1].Env["FOO"] != "bar" {
		t.Fatalf("environment lost: %+v", j.Steps[1])
	}

	droneRoot := t.TempDir()
	write(t, droneRoot, ".drone.yml", `
kind: pipeline
name: default
platform: {os: linux}
steps:
  - name: test
    image: alpine
    commands: [echo one]
---
kind: pipeline
name: win
platform: {os: windows}
steps:
  - name: x
    image: alpine
    commands: [echo two]
`)
	djobs, err := loadDrone(droneRoot, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if len(djobs) != 2 {
		t.Fatalf("want both documents, got %d", len(djobs))
	}
	if djobs[1].SkipReason == "" {
		t.Fatal("windows pipeline not skipped")
	}
}

func TestCircleci(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".circleci/config.yml", `
version: 2.1
jobs:
  build:
    docker:
      - image: cimg/go:1.22
    steps:
      - checkout
      - run: go build ./...
      - run:
          name: tests
          command: go test ./...
  release:
    docker: [{image: cimg/base:stable}]
    steps: [{run: echo release}]
workflows:
  main:
    jobs:
      - build
      - release:
          requires: [build]
`)
	if !detectCircleci(root) {
		t.Fatal("circleci not detected")
	}
	jobs, err := loadCircleci(root, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 2 || jobs[0].Name != "build" || jobs[1].Name != "release" {
		t.Fatalf("requires order broken: %+v", jobs)
	}
	if jobs[0].Image != "cimg/go:1.22" {
		t.Fatalf("docker image lost: %q", jobs[0].Image)
	}
	steps := jobs[0].Steps
	if len(steps) != 3 || steps[0].Command != "true" {
		t.Fatalf("checkout not covered: %+v", steps)
	}
	if steps[2].Name != "tests" || steps[2].Command != "go test ./..." {
		t.Fatalf("named run lost: %+v", steps[2])
	}
}

func TestDetectPriorityAndForce(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".gitlab-ci.yml", "job:\n  script: [echo hi]\n")
	write(t, root, ".drone.yml", "kind: pipeline\nname: d\nsteps: [{name: s, image: a, commands: [echo]}]\n")

	p, err := Detect(root, "auto")
	if err != nil || p == nil || p.Name != "gitlab" {
		t.Fatalf("priority broken: %+v, %v", p, err)
	}
	p, err = Detect(root, "drone")
	if err != nil || p.Name != "drone" {
		t.Fatalf("force broken: %+v, %v", p, err)
	}
	if _, err := Detect(root, "jenkins"); err == nil {
		t.Fatal("unknown platform accepted")
	}
}

func TestTravis(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".travis.yml", `
language: go
os: [linux, osx]
env:
  global:
    - FOO=bar
install: go mod download
script:
  - go vet ./...
  - go test ./...
`)
	if !detectTravis(root) {
		t.Fatal("travis not detected")
	}
	jobs, err := loadTravis(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	j := jobs[0]
	if j.SkipReason != "" {
		t.Fatalf("linux present in os list, must run: %q", j.SkipReason)
	}
	if j.Env["FOO"] != "bar" || j.Env["TRAVIS"] != "true" {
		t.Fatalf("env: %v", j.Env)
	}
	if len(j.Steps) != 2 || j.Steps[0].Name != "install" || j.Steps[1].Name != "script" {
		t.Fatalf("phases: %+v", j.Steps)
	}

	macRoot := t.TempDir()
	write(t, macRoot, ".travis.yml", "os: osx\nscript: [echo hi]\n")
	mjobs, err := loadTravis(macRoot, testRepo)
	if err != nil || len(mjobs) != 1 || mjobs[0].SkipReason == "" {
		t.Fatalf("osx-only travis must be skipped: %+v, %v", mjobs, err)
	}
}

func TestBuildkite(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".buildkite/pipeline.yml", `
env: {TOP: "1"}
steps:
  - label: tests
    command: go test ./...
    env: {STEP: "2"}
  - wait
  - block: "deploy?"
  - label: lint
    commands:
      - go vet ./...
    plugins:
      - docker#v5: {image: golang}
`)
	if !detectBuildkite(root) {
		t.Fatal("buildkite not detected")
	}
	jobs, err := loadBuildkite(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	j := jobs[0]
	if j.Env["TOP"] != "1" || j.Env["BUILDKITE"] != "true" {
		t.Fatalf("env: %v", j.Env)
	}
	// tests, block skip, lint — wait dissolves.
	if len(j.Steps) != 3 {
		t.Fatalf("steps: %+v", j.Steps)
	}
	if j.Steps[0].Env["STEP"] != "2" {
		t.Fatalf("step env lost: %+v", j.Steps[0])
	}
	if j.Steps[2].Command[:4] != "echo" {
		t.Fatalf("plugin note missing: %+v", j.Steps[2])
	}
}

func TestSemaphore(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".semaphore/semaphore.yml", `
version: v1.0
name: pipeline
agent:
  machine: {type: e1-standard-2, os_image: ubuntu2004}
global_job_config:
  env_vars:
    - {name: GLOBAL, value: g}
  prologue:
    commands: [checkout]
blocks:
  - name: Test
    dependencies: [Build]
    task:
      env_vars:
        - {name: BLOCK, value: b}
      jobs:
        - name: unit
          commands: [go test ./...]
          env_vars:
            - {name: JOB, value: j}
  - name: Build
    dependencies: []
    task:
      agent:
        containers: [{name: main, image: golang:1.22}]
      jobs:
        - name: build
          commands: [go build ./...]
`)
	if !detectSemaphore(root) {
		t.Fatal("semaphore not detected")
	}
	jobs, err := loadSemaphore(root, testRepo)
	if err != nil || len(jobs) != 2 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	// dependency order: Build before Test.
	if jobs[0].Name != "Build/build" || jobs[1].Name != "Test/unit" {
		t.Fatalf("block order: %s, %s", jobs[0].Name, jobs[1].Name)
	}
	if jobs[0].Image != "golang:1.22" {
		t.Fatalf("container image lost: %q", jobs[0].Image)
	}
	u := jobs[1]
	if u.Env["GLOBAL"] != "g" || u.Env["BLOCK"] != "b" || u.Env["JOB"] != "j" {
		t.Fatalf("env layering: %v", u.Env)
	}
	// checkout dissolves; the command survives.
	if len(u.Steps) != 1 || u.Steps[0].Command != "go test ./..." {
		t.Fatalf("steps: %+v", u.Steps)
	}

	macRoot := t.TempDir()
	write(t, macRoot, ".semaphore/semaphore.yml", `
agent:
  machine: {type: a1-standard-4, os_image: macos-xcode15}
blocks:
  - name: b
    task:
      jobs: [{name: j, commands: [echo hi]}]
`)
	mjobs, err := loadSemaphore(macRoot, testRepo)
	if err != nil || len(mjobs) != 1 || mjobs[0].SkipReason == "" {
		t.Fatalf("macos pipeline must be skipped: %+v, %v", mjobs, err)
	}
}
