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
	// The nixpkgs dependency list, kept for the no-nixery fallback.
	NixpkgsDeps []string
	// Platform names where this plan came from — "tangled" or a foreign
	// platform — purely for display.
	Platform string
	// MinMemMiB raises the VM memory floor for jobs that need it.
	MinMemMiB int
	// ExtraMounts are additional host:guest:ro mounts (plugin rootfs).
	ExtraMounts []string
	// Workdir is where user steps start; empty means the workspace root.
	// Set when the workflows live in a subdirectory of the repository.
	Workdir string
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

// nixeryOverride is set by --nixery; it wins over the environment.
var nixeryOverride string

// nixeryHost is where dependency images come from. Overridable (--nixery or
// $BSDKRUN_CI_NIXERY) because a self-hosted nixery is the difference between
// CI that depends on a public instance and CI that does not.
func nixeryHost() string {
	if nixeryOverride != "" {
		return nixeryOverride
	}
	if h := os.Getenv("BSDKRUN_CI_NIXERY"); h != "" {
		return h
	}
	return "nixery.dev"
}

// fallbackImage boots when the nixery image cannot be pulled at all.
//
// nixery builds an image server-side on its first request, and a big closure
// (a rust toolchain, say) takes *minutes* — far past its gateway timeout, so
// even patient retries see 504 after 504. The same environment is reachable
// without nixery: the official nix image (pinned, multi-arch, served by a
// registry that does not build on demand) plus `nix profile add` for the
// workflow's dependencies. Slower on the first run — nix substitutes from
// cache.nixos.org — but it finishes, which is the property CI actually needs.
const fallbackImage = "docker.io/nixos/nix:2.30.3"

// fallbackDepsStep installs the nixpkgs dependencies the nixery image would
// have carried. It runs after the clone (nixos/nix already ships git and a
// mount(8), both verified) and before any user step.
func fallbackDepsStep(deps []string) *Step {
	if len(deps) == 0 {
		return nil
	}
	pkgs := make([]string, 0, len(deps))
	for _, d := range deps {
		pkgs = append(pkgs, "nixpkgs#"+d)
	}
	return &Step{
		Name:   "Install dependencies (nix fallback)",
		System: true,
		Command: "nix --extra-experimental-features 'nix-command flakes' " +
			"profile add " + strings.Join(pkgs, " "),
		Env: map[string]string{"NIX_NO_COLOR": "1"},
	}
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
# Republish the exec environment for every later step. The agent hands each
# exec session the image's ENV (PATH with /usr/local/go/bin, JAVA_HOME,
# PHP_INI_DIR, ...), but steps run *login* shells, and /etc/profile on most
# images resets PATH — clobbering the toolchains the image put there (found
# live: golang:alpine's go binary vanished from a -lc shell while a plain
# exec saw it). /proc/self/environ is the step's pre-profile environment —
# exactly what the agent provided — so capturing it into /etc/profile.d
# (which profile sources *after* its reset) restores the image's world.
# PATH merges rather than assigns; other vars only fill gaps, so env passed
# explicitly to a step stays authoritative.
if [ -r /proc/self/environ ]; then
  mkdir -p /etc/profile.d 2>/dev/null || true
  # cat|tr, not tr<file: busybox reads /proc/self/environ as empty through a
  # redirect (measured in the guest), while the pipe sees every byte.
  cat /proc/self/environ | tr '\0' '\n' | while IFS= read -r kv; do
    k=${kv%%=*}; v=${kv#*=}
    case "$v" in *'"'*) continue ;; esac
    case "$k" in
      HOME|PWD|SHLVL|TERM|HOSTNAME|_|'') continue ;;
      PATH) printf 'case ":$PATH:" in *":%s:"*) ;; *) export PATH="%s:$PATH" ;; esac\n' "$v" "$v" ;;
      *) printf 'if [ -z "${%s+x}" ]; then export %s="%s"; fi\n' "$k" "$k" "$v" ;;
    esac
  done > /etc/profile.d/bsdkrun-image-env.sh 2>/dev/null || true
fi
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
chmod u+w /etc/nix 2>/dev/null || true
# nixos/nix (the fallback image) ships /etc/nix/nix.conf read-only; appending
# through virtio-fs passthrough is denied even for guest root, because the
# host side enforces the file mode. Replace it with a writable copy first —
# mv needs only the directory to be writable.
if [ -e /etc/nix/nix.conf ] || [ -L /etc/nix/nix.conf ]; then
  cp -L /etc/nix/nix.conf /etc/nix/nix.conf.new 2>/dev/null || : > /etc/nix/nix.conf.new
  chmod u+w /etc/nix/nix.conf.new
  mv -f /etc/nix/nix.conf.new /etc/nix/nix.conf
fi
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
			// git refuses a repository owned by another uid ("dubious ownership");
			// the source mount is owned by the host user while the guest runs as
			// root, so every modern git needs it marked safe first.
			"git config --global --add safe.directory '*'",
			"git remote add origin file://" + sourceMount,
			strings.Join(fetch, " "),
			"git checkout -q FETCH_HEAD",
		}, "\n"),
	}
}

// buildPlan resolves one matched workflow into an executable plan.
func buildPlan(wf workflow.Workflow, tr *tangled.Pipeline_TriggerMetadata, pipelineID, subdir string) (*Plan, error) {
	// User steps start from the directory whose workflows these are; system
	// steps (prepare, clone) always run at the workspace root — the subdir
	// does not even exist until the clone lands.
	workdir := ""
	if subdir != "" {
		workdir = path.Join(workspaceDir, subdir)
	}
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

	// An `image:` that reads as an OCI reference ("ubuntu:24.04",
	// "ghcr.io/org/img") boots that image directly — no nixery, no nix
	// machinery. Bare words ("nixos", spindle's default) keep the nixery
	// mapping, so existing workflows change nothing.
	if img := strings.TrimSpace(s.Image); img != "" && isOCIRef(img) {
		if len(s.Dependencies) > 0 {
			return nil, fmt.Errorf(
				"%s: image: %s and dependencies: cannot combine — dependencies come "+
					"from nixery; install packages in a step instead", wf.Name, img)
		}
		steps := []Step{prepareStep(), ensureGitStep(), localCloneStep(wf.CloneOpts, sha)}
		for _, us := range s.Steps {
			name := us.Name
			if name == "" {
				name = firstLine(us.Command)
			}
			steps = append(steps, Step{Name: name, Command: us.Command, Env: us.Environment})
		}
		return &Plan{
			Name:     wf.Name,
			Platform: "tangled",
			Image:    img,
			Env:      env,
			Steps:    steps,
			Clone:    wf.CloneOpts,
			Workdir:  workdir,
		}, nil
	}

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
		Name:        wf.Name,
		Platform:    "tangled",
		Image:       workflowImage(s.Dependencies),
		Env:         env,
		Steps:       steps,
		Clone:       wf.CloneOpts,
		NixpkgsDeps: s.Dependencies["nixpkgs"],
		Workdir:     workdir,
	}, nil
}

// isOCIRef distinguishes a pullable image reference from spindle's bare base
// names: anything with a registry path or a tag is a reference.
func isOCIRef(s string) bool {
	return strings.ContainsAny(s, "/:")
}

// ensureGitStep makes the clone step viable on images that do not ship git
// (ubuntu, debian, alpine bases). A no-op when git is already present; a
// clear error when there is no known package manager to install it with.
func ensureGitStep() Step {
	return Step{
		Name:   "Ensure git",
		System: true,
		Command: `command -v git >/dev/null 2>&1 && exit 0
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get -o Acquire::Check-Valid-Until=false -o Acquire::Retries=3 update -qq && apt-get -o Acquire::Retries=3 install -y -qq --no-install-recommends git ca-certificates
elif command -v apk >/dev/null 2>&1; then
  apk add --no-cache git
elif command -v dnf >/dev/null 2>&1; then
  dnf install -y git
else
  echo "this image has no git and no known package manager to install it" >&2
  exit 1
fi`,
	}
}

// ToFallback rewrites the plan for the no-nixery path: the pinned nix base
// image, with the nixpkgs dependencies installed by a system step inserted
// after the clone (index 2: prepare, nix-conf, clone, …).
func (p *Plan) ToFallback() {
	p.Image = fallbackImage
	if dep := fallbackDepsStep(p.NixpkgsDeps); dep != nil {
		at := 3
		if at > len(p.Steps) {
			at = len(p.Steps)
		}
		steps := append([]Step{}, p.Steps[:at]...)
		steps = append(steps, *dep)
		steps = append(steps, p.Steps[at:]...)
		p.Steps = steps
	}
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
