package platforms

// Semaphore CI: .semaphore/semaphore.yml. Blocks order themselves through
// `dependencies`; within a block, a task holds jobs that Semaphore runs on
// parallel machines — here each (block, job) pair becomes one VM, serial,
// in block-dependency order. `global_job_config` and task `prologue`
// commands are prepended to every job (that is what a prologue is for);
// `epilogue` is omitted — it runs even on failure upstream, and this runner
// stops at the first failing step. The `checkout` built-in is covered by
// the clone. An agent with containers supplies the image; an os_image
// naming macOS skips the pipeline. Promotions point at other pipelines and
// are not followed.

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

func detectSemaphore(root string) bool {
	return fileExists(filepath.Join(root, ".semaphore/semaphore.yml")) ||
		fileExists(filepath.Join(root, ".semaphore/semaphore.yaml"))
}

// smEnvVar is Semaphore's env spelling: a list of name/value pairs.
type smEnvVar struct {
	Name  string `yaml:"name"`
	Value string `yaml:"value"`
}

type smAgent struct {
	Machine struct {
		Type    string `yaml:"type"`
		OsImage string `yaml:"os_image"`
	} `yaml:"machine"`
	Containers []struct {
		Name  string `yaml:"name"`
		Image string `yaml:"image"`
	} `yaml:"containers"`
}

type smJob struct {
	Name     string     `yaml:"name"`
	Commands []string   `yaml:"commands"`
	EnvVars  []smEnvVar `yaml:"env_vars"`
}

type smBlock struct {
	Name         string   `yaml:"name"`
	Dependencies []string `yaml:"dependencies"`
	Task         struct {
		Agent    smAgent    `yaml:"agent"`
		EnvVars  []smEnvVar `yaml:"env_vars"`
		Prologue struct {
			Commands []string `yaml:"commands"`
		} `yaml:"prologue"`
		Jobs []smJob `yaml:"jobs"`
	} `yaml:"task"`
}

type smPipeline struct {
	Name            string    `yaml:"name"`
	Agent           smAgent   `yaml:"agent"`
	Blocks          []smBlock `yaml:"blocks"`
	GlobalJobConfig struct {
		EnvVars  []smEnvVar `yaml:"env_vars"`
		Prologue struct {
			Commands []string `yaml:"commands"`
		} `yaml:"prologue"`
	} `yaml:"global_job_config"`
}

func loadSemaphore(root string, repo Repo) ([]Job, error) {
	path := filepath.Join(root, ".semaphore/semaphore.yml")
	if !fileExists(path) {
		path = filepath.Join(root, ".semaphore/semaphore.yaml")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var p smPipeline
	if err := yaml.Unmarshal(data, &p); err != nil {
		return nil, fmt.Errorf(".semaphore/semaphore.yml: %w", err)
	}

	pipelineSkip := smOsSkip(p.Agent)
	pipelineImage := smImage(p.Agent)

	var out []Job
	for _, block := range smBlockOrder(p.Blocks) {
		agent := block.Task.Agent
		image := smImage(agent)
		if image == "" {
			image = pipelineImage
		}
		skip := pipelineSkip
		if s := smOsSkip(agent); s != "" {
			skip = s
		}
		for _, j := range block.Task.Jobs {
			name := j.Name
			if name == "" {
				name = "job"
			}
			if block.Name != "" && len(p.Blocks) > 1 {
				name = block.Name + "/" + name
			}
			job := Job{Name: name, Image: image, SkipReason: skip}

			env := map[string]string{}
			for _, kv := range p.GlobalJobConfig.EnvVars {
				env[kv.Name] = kv.Value
			}
			for _, kv := range block.Task.EnvVars {
				env[kv.Name] = kv.Value
			}
			for _, kv := range j.EnvVars {
				env[kv.Name] = kv.Value
			}
			job.Env = env

			var commands []string
			commands = append(commands, p.GlobalJobConfig.Prologue.Commands...)
			commands = append(commands, block.Task.Prologue.Commands...)
			commands = append(commands, j.Commands...)
			var kept []string
			for _, c := range commands {
				// `checkout` is Semaphore's clone-and-cd built-in; the clone
				// already happened and every step starts in the workspace.
				if strings.TrimSpace(c) == "checkout" {
					continue
				}
				kept = append(kept, c)
			}
			if len(kept) == 0 {
				continue
			}
			job.Steps = append(job.Steps, Step{
				Name:    "commands",
				Command: strings.Join(kept, "\n"),
			})
			out = append(out, job)
		}
	}
	return out, nil
}

// smBlockOrder resolves `dependencies` the same way github's needs are:
// repeatedly take satisfied blocks; a cycle degrades to declaration order.
func smBlockOrder(blocks []smBlock) []smBlock {
	byName := map[string]int{}
	for i, b := range blocks {
		byName[b.Name] = i
	}
	done := map[string]bool{}
	var order []smBlock
	for len(order) < len(blocks) {
		progressed := false
		for _, b := range blocks {
			if done[b.Name] {
				continue
			}
			ok := true
			for _, dep := range b.Dependencies {
				if _, known := byName[dep]; known && !done[dep] {
					ok = false
					break
				}
			}
			if ok {
				order = append(order, b)
				done[b.Name] = true
				progressed = true
			}
		}
		if !progressed {
			rest := make([]smBlock, 0, len(blocks)-len(order))
			for _, b := range blocks {
				if !done[b.Name] {
					rest = append(rest, b)
					done[b.Name] = true
				}
			}
			sort.SliceStable(rest, func(a, b int) bool { return rest[a].Name < rest[b].Name })
			order = append(order, rest...)
		}
	}
	return order
}

func smImage(a smAgent) string {
	if len(a.Containers) > 0 {
		return a.Containers[0].Image
	}
	return ""
}

func smOsSkip(a smAgent) string {
	if a.Machine.OsImage == "" {
		return ""
	}
	return linuxOnly(a.Machine.OsImage)
}
