package platforms

// Buildkite: .buildkite/pipeline.yml (or .yaml). The pipeline's command
// steps become one job's steps — Buildkite steps target agents rather than
// images, so the default image runs them, and `wait` markers dissolve into
// the runner's already-serial order. `block`/`input` (a human gate) and
// `trigger` (another pipeline) become visible skips; a step's `plugins:`
// are noted as skipped while its commands still run. Top-level and per-step
// `env:` apply; agent `queue`/`os` hints naming windows or macos skip the
// step's job the usual way.

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

func detectBuildkite(root string) bool {
	return fileExists(filepath.Join(root, ".buildkite/pipeline.yml")) ||
		fileExists(filepath.Join(root, ".buildkite/pipeline.yaml"))
}

type bkStep struct {
	Label    string            `yaml:"label"`
	Name     string            `yaml:"name"`
	Command  yaml.Node         `yaml:"command"`
	Commands yaml.Node         `yaml:"commands"`
	Env      map[string]string `yaml:"env"`
	Plugins  yaml.Node         `yaml:"plugins"`
	Block    string            `yaml:"block"`
	Input    string            `yaml:"input"`
	Trigger  string            `yaml:"trigger"`
	Wait     yaml.Node         `yaml:"wait"`
	Agents   map[string]string `yaml:"agents"`
}

func loadBuildkite(root string, repo Repo) ([]Job, error) {
	path := filepath.Join(root, ".buildkite/pipeline.yml")
	if !fileExists(path) {
		path = filepath.Join(root, ".buildkite/pipeline.yaml")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var doc struct {
		Env   map[string]string `yaml:"env"`
		Steps []yaml.Node       `yaml:"steps"`
	}
	if err := yaml.Unmarshal(data, &doc); err != nil {
		return nil, fmt.Errorf(".buildkite/pipeline.yml: %w", err)
	}

	env := map[string]string{
		"BUILDKITE":        "true",
		"BUILDKITE_COMMIT": repo.Sha,
		"BUILDKITE_BRANCH": repo.branch(),
	}
	for k, v := range doc.Env {
		env[k] = v
	}
	job := Job{Name: "pipeline", Env: env}

	for i, n := range doc.Steps {
		// The scalar spellings: "wait" (a barrier) or a bare command string.
		var plain string
		if err := n.Decode(&plain); err == nil {
			if plain == "wait" {
				continue // execution is already serial
			}
			job.Steps = append(job.Steps, Step{Name: firstLineOf(plain), Command: plain})
			continue
		}
		var s bkStep
		if err := n.Decode(&s); err != nil {
			continue
		}
		if !s.Wait.IsZero() {
			continue
		}
		name := s.Label
		if name == "" {
			name = s.Name
		}
		switch {
		case s.Block != "" || s.Input != "":
			what := s.Block + s.Input
			job.Steps = append(job.Steps, Step{
				Name:    what + " (block, skipped)",
				Command: fmt.Sprintf(`echo "skipped block step %q — human gates do not translate locally"`, what),
			})
			continue
		case s.Trigger != "":
			job.Steps = append(job.Steps, Step{
				Name:    "trigger " + s.Trigger + " (skipped)",
				Command: fmt.Sprintf(`echo "skipped trigger of pipeline %q — cross-pipeline triggers do not translate locally"`, s.Trigger),
			})
			continue
		}

		for _, hint := range s.Agents {
			if reason := linuxOnly(hint); reason != "" {
				job.SkipReason = reason
			}
		}

		lines := yamlStrings(s.Command)
		lines = append(lines, yamlStrings(s.Commands)...)
		if len(lines) == 0 {
			continue
		}
		if name == "" {
			name = fmt.Sprintf("step %d", i+1)
		}
		cmd := strings.Join(lines, "\n")
		env := s.Env
		// Plugins run for real: cloned at their ref, configured through
		// BUILDKITE_PLUGIN_* env, their hooks wrapped around the command
		// (see bkplugins.go).
		if plugins := bkParsePlugins(s.Plugins); len(plugins) > 0 {
			wrapped, pluginEnv := bkPluginLifecycle(plugins, cmd)
			cmd = wrapped
			merged := map[string]string{}
			for k, v := range pluginEnv {
				merged[k] = v
			}
			for k, v := range s.Env {
				merged[k] = v
			}
			env = merged
			names := make([]string, len(plugins))
			for pi, pl := range plugins {
				names[pi] = pl.Name
			}
			name = name + " [plugins: " + strings.Join(names, ", ") + "]"
		}
		job.Steps = append(job.Steps, Step{Name: name, Command: cmd, Env: env})
	}
	if len(job.Steps) == 0 {
		return nil, nil
	}
	return []Job{job}, nil
}
