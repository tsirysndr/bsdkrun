package bsdkrun

// CI workflows defined in code instead of YAML.
//
// The builder produces exactly the file `bsdkrun ci` (and tangled's spindle)
// consumes — [CIWorkflow.YAML] is that file, [CIWorkflow.Save] commits it to
// `.tangled/workflows/`, and [CIWorkflow.Run] executes it in a microVM
// without a file ever touching the repository:
//
//	err := bsdkrun.Workflow("test").
//		OnPush("main").
//		Deps("go", "gcc").
//		Env("CGO_ENABLED", "1").
//		Step("vet", "go vet ./...").
//		Step("test", "go test ./...").
//		Run()
//
// Code is the source of truth and YAML is the wire format, in that order —
// which is why Save writes a generated-file header: a hand-edit there will be
// overwritten by the next Save.

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// CIWorkflow accumulates a workflow definition. Zero value is not useful;
// start with [Workflow].
type CIWorkflow struct {
	name     string
	engine   string
	when     []ciConstraint
	deps     map[string][]string
	env      map[string]string
	steps    []ciStep
	cloneOpt *ciClone
}

type ciConstraint struct {
	events   []string
	branches []string
}

type ciStep struct {
	name    string
	command string
	env     map[string]string
}

type ciClone struct {
	depth      int
	skip       bool
	submodules bool
}

// Workflow starts a CI workflow definition. The name becomes the workflow
// file's name (`<name>.yml`) and its identity in run output.
func Workflow(name string) *CIWorkflow {
	return &CIWorkflow{
		name:   name,
		engine: "nixery",
		deps:   map[string][]string{},
		env:    map[string]string{},
	}
}

// Engine overrides the engine (`nixery` by default; `microvm` is the other
// spindle engine — `bsdkrun ci` runs both in microVMs either way, but the
// value matters when the same file runs on a real spindle).
func (w *CIWorkflow) Engine(engine string) *CIWorkflow {
	w.engine = engine
	return w
}

// OnPush adds a push trigger for the given branches.
func (w *CIWorkflow) OnPush(branches ...string) *CIWorkflow {
	w.when = append(w.when, ciConstraint{events: []string{"push"}, branches: branches})
	return w
}

// OnPullRequest adds a pull_request trigger targeting the given branches.
func (w *CIWorkflow) OnPullRequest(branches ...string) *CIWorkflow {
	w.when = append(w.when, ciConstraint{events: []string{"pull_request"}, branches: branches})
	return w
}

// On adds a trigger with explicit events, for combinations the two helpers
// above do not cover.
func (w *CIWorkflow) On(events []string, branches ...string) *CIWorkflow {
	w.when = append(w.when, ciConstraint{events: events, branches: branches})
	return w
}

// Deps adds nixpkgs dependencies — the toolchain the steps run against.
func (w *CIWorkflow) Deps(packages ...string) *CIWorkflow {
	w.deps["nixpkgs"] = append(w.deps["nixpkgs"], packages...)
	return w
}

// DepsFrom adds dependencies from a custom registry (a flake reference such
// as "github:nix-community/fenix/<rev>").
func (w *CIWorkflow) DepsFrom(registry string, packages ...string) *CIWorkflow {
	w.deps[registry] = append(w.deps[registry], packages...)
	return w
}

// Env sets a workflow-level environment variable.
func (w *CIWorkflow) Env(key, value string) *CIWorkflow {
	w.env[key] = value
	return w
}

// Step appends a step. Steps run serially, in one VM, each from the
// workspace root.
func (w *CIWorkflow) Step(name, command string) *CIWorkflow {
	w.steps = append(w.steps, ciStep{name: name, command: command})
	return w
}

// StepEnv appends a step with step-scoped environment variables.
func (w *CIWorkflow) StepEnv(name, command string, env map[string]string) *CIWorkflow {
	w.steps = append(w.steps, ciStep{name: name, command: command, env: env})
	return w
}

// CloneDepth sets the clone depth (default 1). A workflow that walks history
// — a commit linter, a changelog check — needs more than the tip.
func (w *CIWorkflow) CloneDepth(depth int) *CIWorkflow {
	if w.cloneOpt == nil {
		w.cloneOpt = &ciClone{}
	}
	w.cloneOpt.depth = depth
	return w
}

// SkipClone skips the checkout entirely.
func (w *CIWorkflow) SkipClone() *CIWorkflow {
	if w.cloneOpt == nil {
		w.cloneOpt = &ciClone{}
	}
	w.cloneOpt.skip = true
	return w
}

// YAML renders the workflow file this definition describes.
//
// Every scalar that could bite is emitted as a JSON string — valid YAML by
// construction — and commands as literal blocks when they are safe to
// round-trip that way. No YAML library: the shape is fixed and small, and the
// SDK stays dependency-free.
func (w *CIWorkflow) YAML() string {
	var b strings.Builder

	if len(w.when) > 0 {
		b.WriteString("when:\n")
		for _, c := range w.when {
			fmt.Fprintf(&b, "  - event: [%s]\n", jsonList(c.events))
			if len(c.branches) == 1 {
				fmt.Fprintf(&b, "    branch: %s\n", jsonStr(c.branches[0]))
			} else if len(c.branches) > 1 {
				fmt.Fprintf(&b, "    branch: [%s]\n", jsonList(c.branches))
			}
		}
		b.WriteString("\n")
	}

	fmt.Fprintf(&b, "engine: %s\n", w.engine)

	if len(w.deps) > 0 {
		b.WriteString("\ndependencies:\n")
		regs := make([]string, 0, len(w.deps))
		for r := range w.deps {
			regs = append(regs, r)
		}
		sort.Strings(regs)
		for _, r := range regs {
			fmt.Fprintf(&b, "  %s:\n", jsonStr(r))
			for _, p := range w.deps[r] {
				fmt.Fprintf(&b, "    - %s\n", jsonStr(p))
			}
		}
	}

	if len(w.env) > 0 {
		b.WriteString("\nenvironment:\n")
		keys := make([]string, 0, len(w.env))
		for k := range w.env {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			fmt.Fprintf(&b, "  %s: %s\n", k, jsonStr(w.env[k]))
		}
	}

	if w.cloneOpt != nil {
		b.WriteString("\nclone:\n")
		if w.cloneOpt.skip {
			b.WriteString("  skip: true\n")
		}
		if w.cloneOpt.depth > 0 {
			fmt.Fprintf(&b, "  depth: %d\n", w.cloneOpt.depth)
		}
		if w.cloneOpt.submodules {
			b.WriteString("  submodules: true\n")
		}
	}

	b.WriteString("\nsteps:\n")
	for _, s := range w.steps {
		fmt.Fprintf(&b, "  - name: %s\n", jsonStr(s.name))
		writeCommand(&b, s.command)
		if len(s.env) > 0 {
			b.WriteString("    environment:\n")
			keys := make([]string, 0, len(s.env))
			for k := range s.env {
				keys = append(keys, k)
			}
			sort.Strings(keys)
			for _, k := range keys {
				fmt.Fprintf(&b, "      %s: %s\n", k, jsonStr(s.env[k]))
			}
		}
	}
	return b.String()
}

// writeCommand prefers a literal block (readable in a committed file), and
// falls back to a JSON string for content a block scalar cannot represent
// byte-for-byte — trailing spaces and carriage returns.
func writeCommand(b *strings.Builder, cmd string) {
	safe := !strings.Contains(cmd, "\r")
	for _, line := range strings.Split(cmd, "\n") {
		if strings.TrimRight(line, " ") != line {
			safe = false
		}
	}
	if !safe || cmd == "" {
		fmt.Fprintf(b, "    command: %s\n", jsonStr(cmd))
		return
	}
	b.WriteString("    command: |\n")
	for _, line := range strings.Split(strings.TrimRight(cmd, "\n"), "\n") {
		fmt.Fprintf(b, "      %s\n", line)
	}
}

func jsonStr(s string) string {
	b, _ := json.Marshal(s)
	return string(b)
}

func jsonList(items []string) string {
	quoted := make([]string, len(items))
	for i, s := range items {
		quoted[i] = jsonStr(s)
	}
	return strings.Join(quoted, ", ")
}

// FileName is the name Save writes: `<name>.yml`.
func (w *CIWorkflow) FileName() string {
	name := w.name
	if !strings.HasSuffix(name, ".yml") && !strings.HasSuffix(name, ".yaml") {
		name += ".yml"
	}
	return name
}

// Save writes the workflow into `<repo>/.tangled/workflows/`, where both
// spindle and `bsdkrun ci` discover it, and returns the path. The header
// marks it generated: this builder owns the file, and a hand-edit will be
// overwritten by the next Save.
func (w *CIWorkflow) Save(repo string) (string, error) {
	dir := filepath.Join(repo, ".tangled", "workflows")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	path := filepath.Join(dir, w.FileName())
	content := "# Generated by the bsdkrun SDK — edit the code that Save()d it instead.\n" + w.YAML()
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		return "", err
	}
	return path, nil
}

// Run executes the workflow in a microVM against `dir` (the current
// directory when empty), streaming output to this process's stdout/stderr.
// The YAML never touches the repository: it is written to a temp file and
// handed to `bsdkrun ci run -f`.
//
// The exit is an error for any failing step, carrying the step's message.
func (w *CIWorkflow) Run() error {
	return w.RunIn("")
}

// RunIn is [CIWorkflow.Run] against an explicit repository directory.
func (w *CIWorkflow) RunIn(dir string) error {
	tmp, err := os.MkdirTemp("", "bsdkrun-ci-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tmp)
	file := filepath.Join(tmp, w.FileName())
	if err := os.WriteFile(file, []byte(w.YAML()), 0o644); err != nil {
		return err
	}

	args := []string{"ci", "run", "-f", file}
	if dir != "" {
		args = append(args, "-w", dir)
	}
	// Spawn inherits stdio, so step output streams exactly as a terminal run
	// of `bsdkrun ci` would.
	code, err := Spawn(args, nil)
	if err != nil {
		return err
	}
	if code != 0 {
		return fmt.Errorf("workflow %s failed (exit %d)", w.name, code)
	}
	return nil
}
