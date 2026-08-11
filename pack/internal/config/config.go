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

	// Recognised but unsupported: pack builds with a single script per
	// provider, so railpack's multi-step graph, its caches and its secrets
	// have nowhere to go. Kept as raw JSON purely so Unsupported() can name
	// them.
	Steps   map[string]json.RawMessage `json:"steps,omitempty"`
	Caches  map[string]json.RawMessage `json:"caches,omitempty"`
	Secrets []string                   `json:"secrets,omitempty"`
}

// Deploy is railpack's deploy block.
type Deploy struct {
	StartCommand string   `json:"startCommand,omitempty"`
	AptPackages  []string `json:"aptPackages,omitempty"`
}

// Unsupported names the fields that were set but will not be honoured, so a
// caller can say so out loud.
func (c *Config) Unsupported() []string {
	if c == nil {
		return nil
	}
	var out []string
	if len(c.Steps) > 0 {
		out = append(out, "steps")
	}
	if len(c.Caches) > 0 {
		out = append(out, "caches")
	}
	if len(c.Secrets) > 0 {
		out = append(out, "secrets")
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
