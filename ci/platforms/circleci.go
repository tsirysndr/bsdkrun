package platforms

// CircleCI: .circleci/config.yml. Jobs with a docker executor translate
// (first container's image is the environment, CircleCI's own rule); the
// machine executor gets the default image; macos executors are skipped.
// Steps: `checkout` is covered by the clone, `run` in both its string and
// map forms translates, orbs and everything else becomes a visible skip.
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

	var out []Job
	for _, name := range ccJobOrder(cfg) {
		j, ok := cfg.Jobs[name]
		if !ok {
			continue // a workflow can reference an orb job we cannot host
		}
		job := Job{Name: name, Env: j.Environment}
		if !j.Macos.IsZero() {
			job.SkipReason = "macos job — a Linux microVM cannot run it"
		}
		if len(j.Docker) > 0 {
			job.Image = j.Docker[0].Image
		}
		for i, sn := range j.Steps {
			job.Steps = append(job.Steps, ccStep(i, sn))
		}
		out = append(out, job)
	}
	return out, nil
}

// ccJobOrder resolves workflow ordering (requires) when a workflows section
// exists; otherwise name order.
func ccJobOrder(cfg ccConfig) []string {
	type entry struct {
		name     string
		requires []string
	}
	var entries []entry
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
				entries = append(entries, entry{name: plain})
				continue
			}
			var m map[string]struct {
				Requires []string `yaml:"requires"`
			}
			if err := jn.Decode(&m); err == nil {
				for name, spec := range m {
					entries = append(entries, entry{name: name, requires: spec.Requires})
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
		return names
	}
	// requires-order, same scheme as the github translator's needs.
	sort.SliceStable(entries, func(a, b int) bool { return entries[a].name < entries[b].name })
	done := map[string]bool{}
	var order []string
	for len(order) < len(entries) {
		progressed := false
		for _, e := range entries {
			if done[e.name] {
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
				order = append(order, e.name)
				done[e.name] = true
				progressed = true
			}
		}
		if !progressed {
			for _, e := range entries {
				if !done[e.name] {
					order = append(order, e.name)
					done[e.name] = true
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
