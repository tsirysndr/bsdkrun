// Package providers turns a project directory into a build plan, one
// provider per language/runtime — the same shape railpack uses
// (core/providers), adapted to unikernels: a provider also decides the
// Kraftfile knobs its runtime needs to boot, not just how to build.
//
// Each provider is a port of the corresponding hand-written example under
// examples/unikraft-*, whose Dockerfile and Kraftfile encode a lot of
// hard-won detail (which base image actually works, which libraries to
// resolve, which kconfig symbol keeps the runtime from aborting). Those
// details belong here now; the examples remain as the reference that proved
// them.
package providers

import (
	"fmt"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/bun"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/deno"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/golang"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/node"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/rust"
)

// Provider builds one language's plan. Mirrors railpack's Provider
// interface, minus the parts that only make sense for its multi-step
// BuildKit graph.
type Provider interface {
	// Name identifies the provider, e.g. "go".
	Name() string

	// Detect reports whether this provider claims dir.
	Detect(dir string) (bool, error)

	// Plan builds the plan for dir, targeting arch.
	Plan(dir string, arch plan.Arch) (*plan.Plan, error)

	// StartCommandHelp says how to override the start command, shown when
	// detection succeeds but the entrypoint is ambiguous.
	StartCommandHelp() string
}

// All returns every provider in detection order.
//
// Order matters: the first provider whose Detect returns true wins. The
// runtime-specific ones come before the ones with broader marker files —
// Deno and Bun before Node, because a Deno or Bun project may also carry a
// package.json, but a Node project never carries deno.json or bun.lockb.
func All() []Provider {
	return []Provider{
		golang.New(),
		rust.New(),
		deno.New(),
		bun.New(),
		node.New(),
	}
}

// Get returns the provider with the given name, or nil. Case-insensitive,
// so `--provider Node` works.
func Get(name string) Provider {
	for _, p := range All() {
		if strings.EqualFold(p.Name(), name) {
			return p
		}
	}
	return nil
}

// Find returns the first provider that claims dir.
func Find(dir string) (Provider, error) {
	for _, p := range All() {
		ok, err := p.Detect(dir)
		if err != nil {
			return nil, fmt.Errorf("%s provider: %w", p.Name(), err)
		}
		if ok {
			return p, nil
		}
	}
	return nil, fmt.Errorf(
		"no supported project found in %s (looked for: %s)", dir, markers())
}

func markers() string {
	names := make([]string, 0, len(All()))
	for _, p := range All() {
		names = append(names, p.Name())
	}
	return strings.Join(names, ", ")
}
