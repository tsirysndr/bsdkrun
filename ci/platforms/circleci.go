package platforms

// CircleCI: .circleci/config.yml. Jobs with a docker executor translate
// (first container's image is the environment, CircleCI's own rule); the
// machine executor gets the default image; macos executors are skipped.
// Steps: `checkout` is covered by the clone, `run` in both its string and
// map forms translates. Orbs are fetched from the registry at plan time
// and expanded — commands, jobs and executors, with parameter substitution
// (see ccorbs.go); what cannot expand becomes a visible skip.
// Workflow order is honored via requires; without a workflows section every
// job runs in name order.

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"gopkg.in/yaml.v3"
)

func detectCircleci(root string) bool {
	return fileExists(filepath.Join(root, ".circleci/config.yml")) ||
		fileExists(filepath.Join(root, ".circleci/config.yaml"))
}

type ccJob struct {
	Docker []struct {
		Image string `yaml:"image"`
	} `yaml:"docker"`
	Machine     yaml.Node         `yaml:"machine"`
	Macos       yaml.Node         `yaml:"macos"`
	Environment map[string]string `yaml:"environment"`
	Steps       []yaml.Node       `yaml:"steps"`
	WorkingDir  string            `yaml:"working_directory"`
}

type ccConfig struct {
	Orbs      map[string]yaml.Node `yaml:"orbs"`
	Jobs      map[string]ccJob     `yaml:"jobs"`
	Workflows map[string]yaml.Node `yaml:"workflows"`
}

func loadCircleci(root string, repo Repo) ([]Job, error) {
	path := filepath.Join(root, ".circleci/config.yml")
	if !fileExists(path) {
		path = filepath.Join(root, ".circleci/config.yaml")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var cfg ccConfig
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		return nil, fmt.Errorf(".circleci/config.yml: %w", err)
	}

	orbSet := ccLoadOrbs(cfg.Orbs)

	var out []Job
	for _, ref := range ccJobOrder(cfg) {
		j, ok := cfg.Jobs[ref.name]
		if !ok {
			// A workflow may reference an orb job directly: expand it.
			if job, ok := ccExpandOrbJob(orbSet, ref.display(), ref.name, ref.args); ok {
				out = append(out, job)
			}
			continue
		}
		job := Job{Name: ref.display(), Env: j.Environment}
		if !j.Macos.IsZero() {
			job.SkipReason = "macos job — a Linux microVM cannot run it"
		}
		if len(j.Docker) > 0 {
			job.Image = j.Docker[0].Image
		}
		job.Steps = ccExpandSteps(orbSet, "", j.Steps, nil, nil, 0)
		out = append(out, job)
	}
	return out, nil
}

// ccWfJob is one workflow job entry: the job (or orb job) it references,
// its arguments, and the ordering constraint.
type ccWfJob struct {
	name     string
	alias    string // workflow-level `name:` override
	args     map[string]yaml.Node
	requires []string
}

func (r ccWfJob) display() string {
	if r.alias != "" {
		return r.alias
	}
	return r.name
}

// ccJobOrder resolves workflow ordering (requires) when a workflows section
// exists; otherwise name order.
func ccJobOrder(cfg ccConfig) []ccWfJob {
	var entries []ccWfJob
	for _, wfNode := range cfg.Workflows {
		var wf struct {
			Jobs []yaml.Node `yaml:"jobs"`
		}
		if err := wfNode.Decode(&wf); err != nil {
			continue
		}
		for _, jn := range wf.Jobs {
			var plain string
			if err := jn.Decode(&plain); err == nil {
				entries = append(entries, ccWfJob{name: plain})
				continue
			}
			var m map[string]map[string]yaml.Node
			if err := jn.Decode(&m); err == nil {
				for name, spec := range m {
					e := ccWfJob{name: name, args: map[string]yaml.Node{}}
					for k, v := range spec {
						switch k {
						case "requires":
							_ = v.Decode(&e.requires)
						case "name":
							_ = v.Decode(&e.alias)
						case "context", "filters", "matrix", "type":
							// orchestration keys, not job arguments
						default:
							e.args[k] = v
						}
					}
					entries = append(entries, e)
				}
			}
		}
	}
	if len(entries) == 0 {
		names := make([]string, 0, len(cfg.Jobs))
		for n := range cfg.Jobs {
			names = append(names, n)
		}
		sort.Strings(names)
		out := make([]ccWfJob, len(names))
		for i, n := range names {
			out[i] = ccWfJob{name: n}
		}
		return out
	}
	// requires-order, same scheme as the github translator's needs. A
	// `requires:` names the workflow-level display name.
	sort.SliceStable(entries, func(a, b int) bool { return entries[a].display() < entries[b].display() })
	done := map[string]bool{}
	var order []ccWfJob
	for len(order) < len(entries) {
		progressed := false
		for _, e := range entries {
			if done[e.display()] {
				continue
			}
			ok := true
			for _, r := range e.requires {
				if !done[r] {
					ok = false
					break
				}
			}
			if ok {
				order = append(order, e)
				done[e.display()] = true
				progressed = true
			}
		}
		if !progressed {
			for _, e := range entries {
				if !done[e.display()] {
					order = append(order, e)
					done[e.display()] = true
				}
			}
		}
	}
	return order
}

func ccStep(i int, n yaml.Node) Step {
	var plain string
	if err := n.Decode(&plain); err == nil {
		switch plain {
		case "checkout":
			return Step{Name: "checkout (covered by the clone)", Command: "true"}
		default:
			return Step{
				Name:    plain + " (skipped)",
				Command: fmt.Sprintf(`echo "skipped step %q — not supported locally"`, plain),
			}
		}
	}
	var m map[string]yaml.Node
	if err := n.Decode(&m); err == nil {
		if rn, ok := m["run"]; ok {
			var cmd string
			if err := rn.Decode(&cmd); err == nil {
				return Step{Name: firstLineOf(cmd), Command: cmd}
			}
			var run struct {
				Name        string            `yaml:"name"`
				Command     string            `yaml:"command"`
				Environment map[string]string `yaml:"environment"`
			}
			if err := rn.Decode(&run); err == nil {
				name := run.Name
				if name == "" {
					name = firstLineOf(run.Command)
				}
				return Step{Name: name, Command: run.Command, Env: run.Environment}
			}
		}
		for key := range m {
			return Step{
				Name:    key + " (skipped)",
				Command: fmt.Sprintf(`echo "skipped step %q — not supported locally"`, key),
			}
		}
	}
	return Step{Name: fmt.Sprintf("step %d (empty)", i+1), Command: "true"}
}
