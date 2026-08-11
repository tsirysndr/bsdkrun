// Package config reads railpack.json, the file a project uses to override
// what pack would otherwise infer.
//
// The schema is railpack's (core/config), so a project already carrying a
// railpack.json needs no second config file. Only the parts that mean
// something for a unikernel are honoured; the rest are recognised so they
// can be reported as unsupported rather than silently ignored — a build
// that quietly drops half your config is worse than one that says so.
package config

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
)

// FileName is the config file pack looks for, matching railpack's.
const FileName = "railpack.json"

// Config is the subset of railpack's schema pack understands, plus the
// fields it recognises only to warn about.
type Config struct {
	// Provider forces a provider instead of detecting one.
	Provider *string `json:"provider,omitempty"`

	// Packages pins tool versions, e.g. {"node": "22"}. Takes precedence
	// over mise, being the more specific statement.
	Packages map[string]string `json:"packages,omitempty"`

	// BuildAptPackages are installed in the build image before the build
	// runs.
	BuildAptPackages []string `json:"buildAptPackages,omitempty"`

	// Deploy carries the start command.
	Deploy *Deploy `json:"deploy,omitempty"`

	// Exclude are extra build-context exclusions, on top of .dockerignore.
	Exclude []string `json:"exclude,omitempty"`

	// Recognised but unsupported: pack builds with a single script per
	// provider, so railpack's multi-step graph, its caches and its secrets
	// have nowhere to go. Kept as raw JSON purely so Unsupported() can name
	// them.
	Steps   map[string]Step            `json:"steps,omitempty"`
	Caches  map[string]json.RawMessage `json:"caches,omitempty"`
	Secrets []string                   `json:"secrets,omitempty"`
}

// Step is one entry of railpack's build graph. pack builds with a single
// script per provider, so the graph itself has nowhere to go — but a step's
// `variables` are ordinary build-time environment, and those do.
type Step struct {
	Variables map[string]string `json:"variables,omitempty"`
	Secrets   []string          `json:"secrets,omitempty"`
}

// BuildVariables merges the variables from every step. pack has one build
// step, so which step declared a variable is not a distinction it can
// preserve; merging keeps them all rather than dropping the ones that
// happen to be in a step named something pack does not recognise.
func (c *Config) BuildVariables() map[string]string {
	if c == nil {
		return nil
	}
	out := map[string]string{}
	for _, step := range c.Steps {
		for k, v := range step.Variables {
			out[k] = v
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// SecretNames merges the top-level secrets with any a step declares.
func (c *Config) SecretNames() []string {
	if c == nil {
		return nil
	}
	seen := map[string]bool{}
	var out []string
	add := func(names []string) {
		for _, n := range names {
			if n != "" && !seen[n] {
				seen[n] = true
				out = append(out, n)
			}
		}
	}
	add(c.Secrets)
	for _, step := range c.Steps {
		add(step.Secrets)
	}
	return out
}

// Deploy is railpack's deploy block.
type Deploy struct {
	StartCommand string   `json:"startCommand,omitempty"`
	AptPackages  []string `json:"aptPackages,omitempty"`

	// Variables are the environment the application runs with. In a
	// unikernel there is no shell to export them, so they are compiled
	// into the image as kconfig entries.
	Variables map[string]string `json:"variables,omitempty"`
}

// Unsupported names the fields that were set but will not be honoured, so a
// caller can say so out loud.
func (c *Config) Unsupported() []string {
	if c == nil {
		return nil
	}
	var out []string
	// steps: only `variables` and `secrets` are honoured; the build graph
	// itself is not, so say so when a step carries anything else.
	var steps []string
	for name, step := range c.Steps {
		if len(step.Variables) == 0 && len(step.Secrets) == 0 {
			steps = append(steps, "steps."+name)
		}
	}
	sort.Strings(steps)
	out = append(out, steps...)
	if len(c.Caches) > 0 {
		out = append(out, "caches")
	}
	if c.Deploy != nil && len(c.Deploy.AptPackages) > 0 {
		// There is no runtime image to install into: the rootfs is the
		// unikernel, built once.
		out = append(out, "deploy.aptPackages")
	}
	return out
}

// Read parses dir/railpack.json. Returns nil (no error) when absent, which
// is the normal case. A malformed file *is* an error — silently ignoring a
// config the user wrote is worse than failing.
func Read(dir string) (*Config, error) {
	path := filepath.Join(dir, FileName)
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	var c Config
	if err := json.Unmarshal(data, &c); err != nil {
		return nil, fmt.Errorf("parsing %s: %w", path, err)
	}
	return &c, nil
}
