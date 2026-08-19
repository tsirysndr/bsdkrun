package platforms

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/ci/platforms/actions"
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
	// Offline and deterministic: setup-node resolves against a fixture, so
	// the assertion is about the runner, not the network.
	oldFetch := actions.FetchFunc
	actions.FetchFunc = func(ref actions.Ref) ([]byte, error) {
		if ref.Slug() == "actions/setup-node" {
			return []byte("name: Setup Node\nruns: {using: node20, main: dist/index.js}\n"), nil
		}
		return nil, fmt.Errorf("no fixture for %s", ref.Slug())
	}
	defer func() { actions.FetchFunc = oldFetch }()

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
	// node provision first (setup-node is a JS action), then checkout
	// no-op, npm ci, the real setup-node execution, npm test — all wrapped
	// in the Actions env protocol.
	if len(b.Steps) != 5 {
		t.Fatalf("want 5 steps, got %d: %+v", len(b.Steps), b.Steps)
	}
	if b.Steps[0].Name != "Provision actions runtime (node)" {
		t.Fatalf("node provisioning must come first: %+v", b.Steps[0])
	}
	if !strings.Contains(b.Steps[1].Command, "true") {
		t.Fatalf("checkout should be a no-op: %+v", b.Steps[1])
	}
	if !strings.Contains(b.Steps[2].Command, "npm ci") ||
		!strings.Contains(b.Steps[2].Command, "GITHUB_ENV=") {
		t.Fatalf("run step must carry the env protocol: %+v", b.Steps[2])
	}
	if b.Steps[3].Name != "Setup Node" ||
		!strings.Contains(b.Steps[3].Command, "dist/index.js") {
		t.Fatalf("setup-node must run for real: %+v", b.Steps[3])
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
	// test step + plugin skip; no divergence notice — a plugin step's image
	// is the chroot target, not a VM-image candidate
	if len(j.Steps) != 2 {
		t.Fatalf("steps: %+v", j.Steps)
	}
	if j.Steps[0].Env["FOO"] != "bar" {
		t.Fatalf("environment lost: %+v", j.Steps[0])
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
	if _, err := Detect(root, "bamboo"); err == nil {
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
	if !strings.Contains(j.Steps[2].Command, "docker-buildkite-plugin") ||
		!strings.Contains(j.Steps[2].Command, "hooks/environment") {
		t.Fatalf("plugin lifecycle missing: %+v", j.Steps[2])
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

func TestJenkinsDeclarative(t *testing.T) {
	root := t.TempDir()
	write(t, root, "Jenkinsfile", `
// a comment
pipeline {
    agent {
        docker { image 'golang:1.22' }
    }
    environment {
        FOO = 'bar'
        DYN = credentials('secret-id')
    }
    stages {
        stage('Build') {
            steps {
                checkout scm
                sh 'go build ./...'
                sh(script: 'go vet ./...')
            }
        }
        stage('Test') {
            environment {
                STAGE = 'test'
            }
            steps {
                echo 'running tests'
                sh """
                go test ./...
                """
            }
        }
    }
    post {
        always { echo 'done' }
    }
}
`)
	if !detectJenkins(root) {
		t.Fatal("jenkins not detected")
	}
	jobs, err := loadJenkins(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	j := jobs[0]
	if j.Image != "golang:1.22" {
		t.Fatalf("docker agent image lost: %q", j.Image)
	}
	if j.Env["FOO"] != "bar" {
		t.Fatalf("literal env lost: %v", j.Env)
	}
	if _, has := j.Env["DYN"]; has {
		t.Fatalf("credentials() must be dropped, not mistranslated: %v", j.Env)
	}
	if len(j.Steps) != 2 || j.Steps[0].Name != "Build" || j.Steps[1].Name != "Test" {
		t.Fatalf("stages: %+v", j.Steps)
	}
	if j.Steps[0].Command != "go build ./...\ngo vet ./..." {
		t.Fatalf("sh forms: %q", j.Steps[0].Command)
	}
	if j.Steps[1].Env["STAGE"] != "test" {
		t.Fatalf("stage env lost: %+v", j.Steps[1])
	}

}

func TestJenkinsScriptedGetsRealJenkins(t *testing.T) {
	root := t.TempDir()
	write(t, root, "Jenkinsfile", `
node {
    stage('Build') {
        sh 'make'
    }
}
`)
	jobs, err := loadJenkins(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("scripted must run under real Jenkins: %+v, %v", jobs, err)
	}
	j := jobs[0]
	if j.Image != "eclipse-temurin:17-jdk" {
		t.Fatalf("runner image: %q", j.Image)
	}
	if len(j.Steps) != 4 {
		t.Fatalf("provision, plugins, run expected: %+v", j.Steps)
	}
	if !strings.Contains(j.Steps[0].Command, "scripted pipelines are Groovy programs") {
		t.Fatalf("the reason must be announced: %q", j.Steps[0].Command)
	}
	if !strings.Contains(j.Steps[3].Command, "jenkinsfile-runner") ||
		!strings.Contains(j.Steps[3].Command, "-f Jenkinsfile") {
		t.Fatalf("run step: %q", j.Steps[3].Command)
	}
}

func TestJenkinsPluginStepsGetRealJenkins(t *testing.T) {
	root := t.TempDir()
	write(t, root, "Jenkinsfile", `
pipeline {
    agent any
    stages {
        stage('Test') {
            steps {
                sh 'make test'
                junit 'report.xml'
            }
        }
    }
}
`)
	write(t, root, "plugins.txt", "junit\n")
	jobs, err := loadJenkins(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("plugin steps must run under real Jenkins: %+v, %v", jobs, err)
	}
	j := jobs[0]
	if j.Image != "eclipse-temurin:17-jdk" {
		t.Fatalf("runner image: %q", j.Image)
	}
	if !strings.Contains(j.Steps[0].Command, "junit") {
		t.Fatalf("the plugin step must be named in the reason: %q", j.Steps[0].Command)
	}
	if !strings.Contains(j.Steps[2].Command, "cat plugins.txt") {
		t.Fatalf("the repo's plugins.txt must be honored: %q", j.Steps[2].Command)
	}
}

func TestAzure(t *testing.T) {
	root := t.TempDir()
	write(t, root, "azure-pipelines.yml", `
pool: {vmImage: ubuntu-latest}
variables: {TOP: "1"}
jobs:
  - job: deploy
    dependsOn: [build]
    steps:
      - script: echo deploy
  - job: build
    container: node:22
    variables:
      - name: JOB
        value: "2"
    steps:
      - checkout: self
      - script: npm ci
        displayName: install
      - bash: npm test
      - pwsh: Write-Host hi
      - task: PublishBuildArtifacts@1
  - job: winjob
    pool: {vmImage: windows-latest}
    steps: [{script: echo hi}]
`)
	if !detectAzure(root) {
		t.Fatal("azure not detected")
	}
	jobs, err := loadAzure(root, testRepo)
	if err != nil || len(jobs) != 3 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	// build must precede deploy; the independent winjob may sit between.
	pos := map[string]int{}
	for i, j := range jobs {
		pos[j.Name] = i
	}
	if pos["build"] > pos["deploy"] {
		t.Fatalf("dependsOn order: %v", pos)
	}
	b := jobs[0]
	if b.Image != "node:22" {
		t.Fatalf("container: %q", b.Image)
	}
	if b.Env["TOP"] != "1" || b.Env["JOB"] != "2" {
		t.Fatalf("variables: %v", b.Env)
	}
	// checkout no-op, script, bash, pwsh skip, task skip
	if len(b.Steps) != 5 || b.Steps[1].Name != "install" {
		t.Fatalf("steps: %+v", b.Steps)
	}
	if !strings.Contains(b.Steps[3].Command, "PowerShell") {
		t.Fatalf("pwsh skip not visible: %+v", b.Steps[3])
	}
	for _, j := range jobs {
		if j.Name == "winjob" && j.SkipReason == "" {
			t.Fatal("windows pool not skipped")
		}
	}
}

func TestCodebuild(t *testing.T) {
	root := t.TempDir()
	write(t, root, "buildspec.yml", `
version: 0.2
env:
  variables: {FOO: bar}
  secrets-manager: {DB_PASS: "prod/db:password"}
phases:
  install:
    runtime-versions: {nodejs: 20}
    commands: [npm ci]
  build:
    commands:
      - npm test
`)
	if !detectCodebuild(root) {
		t.Fatal("codebuild not detected")
	}
	jobs, err := loadCodebuild(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	j := jobs[0]
	if j.Env["FOO"] != "bar" {
		t.Fatalf("env: %v", j.Env)
	}
	// dropped-secrets note, install, build — fixed phase order.
	if len(j.Steps) != 3 || j.Steps[1].Name != "install" || j.Steps[2].Name != "build" {
		t.Fatalf("phases: %+v", j.Steps)
	}
	if !strings.Contains(j.Steps[0].Command, "DB_PASS") {
		t.Fatalf("dropped aws-managed env not announced: %+v", j.Steps[0])
	}
}

func TestTekton(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".tekton/pipeline.yaml", `
apiVersion: tekton.dev/v1
kind: Pipeline
metadata: {name: ci}
spec:
  tasks:
    - name: test
      runAfter: [build]
      taskRef: {name: test-task}
      params:
        - {name: flags, value: "-v"}
    - name: build
      taskSpec:
        steps:
          - name: compile
            image: golang:1.22
            script: go build ./...
---
apiVersion: tekton.dev/v1
kind: Task
metadata: {name: test-task}
spec:
  params:
    - {name: flags, default: ""}
    - {name: pkg, default: "./..."}
  steps:
    - name: run
      image: golang:1.22
      script: go test $(params.flags) $(params.pkg)
`)
	if !detectTekton(root) {
		t.Fatal("tekton not detected")
	}
	jobs, err := loadTekton(root, testRepo)
	if err != nil || len(jobs) != 2 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	if jobs[0].Name != "build" || jobs[1].Name != "test" {
		t.Fatalf("runAfter order: %s, %s", jobs[0].Name, jobs[1].Name)
	}
	if jobs[1].Steps[0].Command != "go test -v ./..." {
		t.Fatalf("param substitution: %q", jobs[1].Steps[0].Command)
	}
	if jobs[0].Image != "golang:1.22" {
		t.Fatalf("image: %q", jobs[0].Image)
	}
}

func TestBuildkitePluginsRunForReal(t *testing.T) {
	root := t.TempDir()
	write(t, root, ".buildkite/pipeline.yml", `
steps:
  - label: lint
    command: make lint
    plugins:
      - shellcheck#v1.4.0:
          files:
            - scripts/*.sh
            - hooks/**
      - myorg/custom#v2.0.0:
          debug: true
          region: eu-1
`)
	jobs, err := loadBuildkite(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	s := jobs[0].Steps[0]
	if !strings.Contains(s.Name, "plugins: shellcheck#v1.4.0, myorg/custom#v2.0.0") &&
		!strings.Contains(s.Name, "plugins:") {
		t.Fatalf("plugins must be announced in the name: %q", s.Name)
	}
	// Config flattened per Buildkite's scheme: arrays indexed, keys upper.
	if s.Env["BUILDKITE_PLUGIN_SHELLCHECK_FILES_0"] != "scripts/*.sh" ||
		s.Env["BUILDKITE_PLUGIN_SHELLCHECK_FILES_1"] != "hooks/**" {
		t.Fatalf("array config: %v", s.Env)
	}
	if s.Env["BUILDKITE_PLUGIN_CUSTOM_DEBUG"] != "true" ||
		s.Env["BUILDKITE_PLUGIN_CUSTOM_REGION"] != "eu-1" {
		t.Fatalf("map config: %v", s.Env)
	}
	// The lifecycle: clone from the conventional repos, source environment,
	// pre-command, the command, post-command with the exit code preserved.
	for _, needle := range []string{
		"github.com/buildkite-plugins/shellcheck-buildkite-plugin",
		"github.com/myorg/custom-buildkite-plugin",
		"hooks/environment",
		`export -p > "$__bk_envcap"`,
		"hooks/pre-command",
		"make lint",
		"BUILDKITE_COMMAND_EXIT_STATUS=$__bk_rc",
		"hooks/post-command",
		"exit $__bk_rc",
	} {
		if !strings.Contains(s.Command, needle) {
			t.Fatalf("lifecycle lacks %q:\n%s", needle, s.Command)
		}
	}
}

func TestDronePluginsExecute(t *testing.T) {
	oldPull := PullImageFunc
	PullImageFunc = func(ref string) (PulledImage, error) {
		if ref != "plugins/download" {
			return PulledImage{}, fmt.Errorf("no fixture for %s", ref)
		}
		return PulledImage{
			Rootfs:     "/host/cache/oci/sha-x/rootfs",
			Entrypoint: []string{"/bin/drone-download"},
			Env:        []string{"PATH=/bin", "GODEBUG=netdns=go"},
		}, nil
	}
	defer func() { PullImageFunc = oldPull }()

	root := t.TempDir()
	write(t, root, ".drone.yml", `
kind: pipeline
name: default
steps:
  - name: fetch
    image: plugins/download
    settings:
      source: https://example.com/file.txt
      md5: abc
      tags:
        - one
        - two
  - name: use
    image: alpine:3.20
    commands: [test -f file.txt]
`)
	jobs, err := loadDrone(root, testRepo)
	if err != nil || len(jobs) != 1 {
		t.Fatalf("jobs: %+v, %v", jobs, err)
	}
	j := jobs[0]
	var plugin *Step
	for i := range j.Steps {
		if strings.Contains(j.Steps[i].Name, "plugin: plugins/download") {
			plugin = &j.Steps[i]
		}
	}
	if plugin == nil {
		t.Fatalf("plugin step missing: %+v", j.Steps)
	}
	// Drone's settings flattening: upper keys, lists comma-joined.
	if plugin.Env["PLUGIN_SOURCE"] != "https://example.com/file.txt" ||
		plugin.Env["PLUGIN_TAGS"] != "one,two" {
		t.Fatalf("settings env: %v", plugin.Env)
	}
	// The image's own env rides along, and DRONE identity is set.
	if plugin.Env["GODEBUG"] != "netdns=go" || plugin.Env["DRONE_WORKSPACE"] != "/drone/src" {
		t.Fatalf("image/identity env: %v", plugin.Env)
	}
	// The execution shape: overlay, workspace bind, cwd trick, chroot.
	for _, needle := range []string{
		"mount -t overlay", "mount --bind /tangled/workspace",
		`cd "$W/root/drone/src"`, `chroot "$W/root" '/bin/drone-download'`,
	} {
		if !strings.Contains(plugin.Command, needle) {
			t.Fatalf("execution lacks %q:\n%s", needle, plugin.Command)
		}
	}
	// And the VM must mount the pulled rootfs read-only.
	if len(j.ExtraMounts) != 1 || !strings.HasPrefix(j.ExtraMounts[0], "/host/cache/oci/sha-x/rootfs:") ||
		!strings.HasSuffix(j.ExtraMounts[0], ":ro") {
		t.Fatalf("mounts: %v", j.ExtraMounts)
	}

	// The VM boots the command-step's image; a scratch plugin image must
	// never be a boot candidate, and its divergence is not announced.
	if j.Image != "alpine:3.20" {
		t.Fatalf("plugin image leaked into VM image: %q", j.Image)
	}
	for _, s := range j.Steps {
		if strings.Contains(s.Name, "per-step images") {
			t.Fatalf("plugin step should not trigger a divergence notice: %+v", j.Steps)
		}
	}

	// Woodpecker rides the same path but speaks CI_* alongside PLUGIN_*.
	wpRoot := t.TempDir()
	write(t, wpRoot, ".woodpecker.yml", `
steps:
  - name: fetch
    image: plugins/download
    settings: {source: https://example.com/file.txt}
  - name: use
    image: alpine:3.20
    commands: [test -f file.txt]
`)
	wpJobs, err := loadWoodpecker(wpRoot, testRepo)
	if err != nil || len(wpJobs) != 1 {
		t.Fatalf("woodpecker jobs: %+v, %v", wpJobs, err)
	}
	var wpPlugin *Step
	for i := range wpJobs[0].Steps {
		if strings.Contains(wpJobs[0].Steps[i].Name, "plugin: plugins/download") {
			wpPlugin = &wpJobs[0].Steps[i]
		}
	}
	if wpPlugin == nil {
		t.Fatalf("woodpecker plugin step missing: %+v", wpJobs[0].Steps)
	}
	if wpPlugin.Env["CI"] != "woodpecker" || wpPlugin.Env["CI_WORKSPACE"] != "/drone/src" ||
		wpPlugin.Env["CI_COMMIT_SHA"] != testRepo.Sha || wpPlugin.Env["DRONE"] != "true" {
		t.Fatalf("woodpecker plugin env: %v", wpPlugin.Env)
	}

	// A pull failure stays a visible skip, never a silent one.
	PullImageFunc = func(ref string) (PulledImage, error) {
		return PulledImage{}, fmt.Errorf("registry unreachable")
	}
	jobs, _ = loadDrone(root, testRepo)
	found := false
	for _, s := range jobs[0].Steps {
		if strings.Contains(s.Name, "skipped") && strings.Contains(s.Command, "registry unreachable") {
			found = true
		}
	}
	if !found {
		t.Fatalf("pull failure must be a visible skip: %+v", jobs[0].Steps)
	}
}

func TestCircleciOrbsExpand(t *testing.T) {
	oldFetch := OrbFetchFunc
	OrbFetchFunc = func(ref string) (string, error) {
		if ref != "circleci/node@5" {
			return "", fmt.Errorf("no fixture for %s", ref)
		}
		return `
commands:
  install-packages:
    parameters:
      pkg-manager:
        type: enum
        default: npm
      with-cache:
        type: boolean
        default: true
    steps:
      - when:
          condition: << parameters.with-cache >>
          steps:
            - restore_cache: {key: deps}
      - when:
          condition:
            equal: [yarn, << parameters.pkg-manager >>]
          steps:
            - run: yarn install --frozen-lockfile
      - unless:
          condition:
            equal: [yarn, << parameters.pkg-manager >>]
          steps:
            - run: npm ci
executors:
  default:
    parameters:
      tag:
        type: string
        default: lts
    docker:
      - image: cimg/node:<< parameters.tag >>
jobs:
  test:
    parameters:
      version:
        type: string
        default: lts
    executor:
      name: default
      tag: << parameters.version >>
    steps:
      - checkout
      - install-packages
      - run: npm test
`, nil
	}
	defer func() { OrbFetchFunc = oldFetch }()

	root := t.TempDir()
	write(t, root, ".circleci/config.yml", `
version: 2.1
orbs:
  node: circleci/node@5
  gone: acme/missing@1.2.3
jobs:
  build:
    docker:
      - image: cimg/base:stable
    steps:
      - checkout
      - node/install-packages:
          pkg-manager: yarn
      - gone/deploy: {target: prod}
      - run: echo built
workflows:
  main:
    jobs:
      - build
      - node/test:
          version: "20.11"
          requires: [build]
`)
	jobs, err := loadCircleci(root, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if len(jobs) != 2 {
		t.Fatalf("want 2 jobs, got %d: %+v", len(jobs), jobs)
	}

	build := jobs[0]
	if build.Name != "build" {
		t.Fatalf("order: %+v", []string{jobs[0].Name, jobs[1].Name})
	}
	joined := ""
	for _, s := range build.Steps {
		joined += s.Name + "\n" + s.Command + "\n"
	}
	// The yarn branch was chosen, the npm branch dropped, the cache step
	// became a visible no-op, and the broken orb a visible skip.
	if !strings.Contains(joined, "yarn install --frozen-lockfile") {
		t.Fatalf("yarn branch missing:\n%s", joined)
	}
	if strings.Contains(joined, "npm ci") {
		t.Fatalf("npm branch should be ruled out:\n%s", joined)
	}
	if !strings.Contains(joined, "restore_cache (no-op locally)") {
		t.Fatalf("cache no-op missing:\n%s", joined)
	}
	if !strings.Contains(joined, "no fixture for acme/missing@1.2.3") {
		t.Fatalf("broken orb must skip visibly:\n%s", joined)
	}

	test := jobs[1]
	if test.Name != "node/test" {
		t.Fatalf("orb job name: %q", test.Name)
	}
	// Executor resolved through the orb with the workflow argument.
	if test.Image != "cimg/node:20.11" {
		t.Fatalf("executor image: %q", test.Image)
	}
	joined = ""
	for _, s := range test.Steps {
		joined += s.Name + "\n" + s.Command + "\n"
	}
	if !strings.Contains(joined, "npm test") || !strings.Contains(joined, "npm ci") {
		t.Fatalf("orb job steps:\n%s", joined)
	}
}
