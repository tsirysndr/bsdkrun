package main

// From workflow files to something a microVM can execute.
//
// The parsing of `when:`, `clone:` and `engine:` is tangled's own `workflow`
// package — imported, not reimplemented, so a file spindle accepts is a file
// this accepts, glob semantics and constraint defaults included. What that
// package deliberately leaves opaque (`Raw`) is the engine-specific half:
// steps, dependencies and environment. Those are parsed here with the same
// shapes spindle's engines use.

import (
	"fmt"
	"os"
	"path"
	"path/filepath"
	"runtime"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
	"tangled.org/core/api/tangled"
	"tangled.org/core/workflow"
)

// depMap tolerates both spellings of `dependencies:` in the wild.
//
// The microvm engine takes a plain list (implicitly nixpkgs); the nixery
// engine takes a map of registry → packages. Files of both shapes exist in
// tangled's own repositories, so a runner claiming compatibility has to read
// both.
type depMap map[string][]string

func (d *depMap) UnmarshalYAML(node *yaml.Node) error {
	switch node.Kind {
	case yaml.SequenceNode:
		var list []string
		if err := node.Decode(&list); err != nil {
			return err
		}
		*d = depMap{"nixpkgs": list}
	case yaml.MappingNode:
		var m map[string][]string
		if err := node.Decode(&m); err != nil {
			return err
		}
		*d = m
	default:
		return fmt.Errorf("dependencies must be a list or a registry map")
	}
	return nil
}

// spec is the engine-facing half of a workflow file — the part `workflow.
// Workflow` carries as `Raw`. Field shapes match spindle's nixery engine.
type spec struct {
	Image        string `yaml:"image"`
	Dependencies depMap `yaml:"dependencies"`
	Environment  map[string]string
	Steps        []struct {
		Name        string            `yaml:"name"`
		Command     string            `yaml:"command"`
		Environment map[string]string `yaml:"environment"`
	} `yaml:"steps"`
}

// Step is one command to run in the workflow's VM.
type Step struct {
	Name    string
	Command string
	Env     map[string]string
	// System steps are injected by the runner (clone, nix setup); user steps
	// come from the file. The distinction is part of spindle's log format.
	System bool
}

// Plan is one workflow, resolved to the point of being executable.
type Plan struct {
	Name  string
	Image string
	Env   map[string]string
	Steps []Step
	Clone workflow.CloneOpts
}

// loadWorkflows reads every workflow under `.tangled/workflows`, exactly
// where spindle looks.
func loadWorkflows(root string) ([]workflow.Workflow, error) {
	dir := filepath.Join(root, workflow.WorkflowDir)
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("no %s in %s — nothing to run", workflow.WorkflowDir, root)
	}
	var wfs []workflow.Workflow
	for _, e := range entries {
		name := e.Name()
		if e.IsDir() || (!strings.HasSuffix(name, ".yml") && !strings.HasSuffix(name, ".yaml")) {
			continue
		}
		contents, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			return nil, err
		}
		wf, err := workflow.FromFile(name, contents)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", name, err)
		}
		wfs = append(wfs, wf)
	}
	sort.Slice(wfs, func(i, j int) bool { return wfs[i].Name < wfs[j].Name })
	return wfs, nil
}

// nixeryHost is where dependency images come from. Overridable because a
// self-hosted nixery is the difference between CI that works offline-ish
// (a local cache) and CI that hits a public instance on every cold pull.
func nixeryHost() string {
	if h := os.Getenv("BSDKRUN_CI_NIXERY"); h != "" {
		return h
	}
	return "nixery.dev"
}

// workflowImage maps a dependency set to a nixery image reference, matching
// spindle's nixery engine: nixpkgs deps as path segments, then the default
// toolchain every workflow gets (bash, git, coreutils, nix), with the arm64
// prefix on arm hosts.
//
// The same mapping serves `engine: microvm`. Spindle boots those on a NixOS
// image and installs the (plain-list) dependencies inside; a nixery image
// with the same packages reaches the same userland with none of the NixOS
// machinery — and bsdkrun's whole job is making an OCI image bootable.
func workflowImage(deps depMap) string {
	segments := append([]string{}, deps["nixpkgs"]...)
	// spindle's defaults, plus util-linux: spindle's containers get /proc,
	// /dev and their bind mounts from the container runtime, but a microVM's
	// init does that mounting itself and needs a mount(8) to do it with —
	// coreutils does not carry one, and without it the guest boots with no
	// /proc and no virtio-fs shares, which surfaces as a git clone failing
	// against an empty directory.
	segments = append(segments, "bash", "git", "coreutils", "util-linux", "nix")
	if runtime.GOARCH == "arm64" {
		segments = append([]string{"arm64"}, segments...)
	}
	return path.Join(nixeryHost(), path.Join(segments...))
}

// prepareStep makes the guest writable where the workflow machinery needs it.
//
// A container runtime gives spindle's steps a writable /etc for free; here
// the rootfs is a nix-built image over virtio-fs, whose directories —
// `/`, `/etc`, `/nix` — are mode 0555, and passthrough enforces exactly
// that. The guest owns them, so a chmod is its right; targeted rather than
// recursive because dirtying metadata for every store path in a 52-layer
// image costs real time for nothing.
func prepareStep() Step {
	return Step{
		Name:   "Prepare VM",
		System: true,
		Command: `chmod u+w / 2>/dev/null || true
for d in /etc /nix /root /usr /var /tmp; do [ -d "$d" ] && chmod u+w "$d" 2>/dev/null; done
mkdir -p /tmp && chmod 1777 /tmp 2>/dev/null || true
[ -d /nix ] && { chmod u+w /nix 2>/dev/null; mkdir -p /nix/var 2>/dev/null; } || true
true`,
	}
}

// nixConfStep is spindle's, verbatim: flakes on, no build users, no sandbox
// (the isolation is the VM), and the homeless-shelter cleanup hook.
func nixConfStep() Step {
	return Step{
		Name:   "Configure Nix",
		System: true,
		Command: `mkdir -p /etc/nix
echo 'extra-experimental-features = nix-command flakes' >> /etc/nix/nix.conf
echo 'build-users-group = ' >> /etc/nix/nix.conf
echo 'sandbox = false' >> /etc/nix/nix.conf
printf '#!/bin/sh\nrm -rf /homeless-shelter\n' > /etc/nix/post-build-hook.sh
chmod +x /etc/nix/post-build-hook.sh
echo 'post-build-hook = /etc/nix/post-build-hook.sh' >> /etc/nix/nix.conf`,
	}
}

// customDepsStep installs non-nixpkgs dependencies (flake registries like
// `github:owner/repo/rev`) with `nix profile add`, as spindle does. nixpkgs
// deps never reach it — they are already in the image.
func customDepsStep(deps depMap) *Step {
	var pkgs []string
	// Deterministic order; map iteration is not.
	regs := make([]string, 0, len(deps))
	for reg := range deps {
		if reg != "nixpkgs" {
			regs = append(regs, reg)
		}
	}
	sort.Strings(regs)
	for _, reg := range regs {
		if len(deps[reg]) == 0 {
			pkgs = append(pkgs, reg)
		}
		for _, p := range deps[reg] {
			pkgs = append(pkgs, fmt.Sprintf("'%s#%s'", reg, p))
		}
	}
	if len(pkgs) == 0 {
		return nil
	}
	return &Step{
		Name:   "Install custom dependencies",
		System: true,
		Command: "nix --extra-experimental-features nix-command --extra-experimental-features flakes " +
			"profile add " + strings.Join(pkgs, " "),
		Env: map[string]string{
			"NIX_NO_COLOR":               "1",
			"NIX_SHOW_DOWNLOAD_PROGRESS": "0",
		},
	}
}

// The paths inside the guest, matching spindle's container layout so a
// workflow that hardcodes them ports over unchanged.
const (
	workspaceDir = "/tangled/workspace"
	homeDir      = "/tangled/home"
	// Where a local run mounts the checkout, read-only. The clone step reads
	// from here instead of a knot URL — the commit is right there, and a
	// local run must work with no network and no knot.
	sourceMount = "/tangled/source"
)

// localCloneStep reproduces spindle's clone step against the mounted
// checkout: same fetch-by-sha shape, so depth and submodule options mean the
// same thing, and the workspace holds the *commit*, not the working tree.
// Uncommitted changes deliberately do not run — CI that quietly tested a
// dirty tree would pass locally and fail everywhere else.
func localCloneStep(opts workflow.CloneOpts, sha string) Step {
	if opts.Skip {
		return Step{
			Name:    "Clone repository into workspace (skipped)",
			System:  true,
			Command: "true",
		}
	}
	depth := opts.Depth
	if depth == 0 {
		depth = 1
	}
	fetch := []string{fmt.Sprintf("git fetch --depth=%d", depth)}
	if opts.IncludeSubmodules != nil && *opts.IncludeSubmodules {
		fetch = append(fetch, "--recurse-submodules=yes")
	}
	if opts.Tags != nil && *opts.Tags {
		fetch = append(fetch, "--tags")
	}
	fetch = append(fetch, "origin", sha)
	return Step{
		Name:   "Clone repository into workspace",
		System: true,
		Command: strings.Join([]string{
			"git init -q",
			"git remote add origin file://" + sourceMount,
			strings.Join(fetch, " "),
			"git checkout -q FETCH_HEAD",
		}, "\n"),
	}
}

// buildPlan resolves one matched workflow into an executable plan.
func buildPlan(wf workflow.Workflow, tr *tangled.Pipeline_TriggerMetadata, pipelineID string) (*Plan, error) {
	var s spec
	if err := yaml.Unmarshal([]byte(wf.Raw), &s); err != nil {
		return nil, fmt.Errorf("%s: %w", wf.Name, err)
	}
	if len(s.Steps) == 0 {
		return nil, fmt.Errorf("%s has no steps", wf.Name)
	}

	env := pipelineEnv(tr, pipelineID)
	for k, v := range s.Environment {
		env[k] = v
	}
	env["HOME"] = homeDir

	sha := env["TANGLED_COMMIT_SHA"]
	steps := []Step{prepareStep(), nixConfStep(), localCloneStep(wf.CloneOpts, sha)}
	if dep := customDepsStep(s.Dependencies); dep != nil {
		steps = append(steps, *dep)
	}
	for _, us := range s.Steps {
		name := us.Name
		if name == "" {
			name = firstLine(us.Command)
		}
		steps = append(steps, Step{Name: name, Command: us.Command, Env: us.Environment})
	}

	return &Plan{
		Name:  wf.Name,
		Image: workflowImage(s.Dependencies),
		Env:   env,
		Steps: steps,
		Clone: wf.CloneOpts,
	}, nil
}

func firstLine(s string) string {
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	if len(s) > 60 {
		s = s[:60] + "…"
	}
	return s
}
