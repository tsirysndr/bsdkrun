package platforms

// Azure Pipelines: azure-pipelines.yml. The file comes in three nestings —
// bare `steps:`, `jobs:` with steps, or `stages:` with jobs — and all three
// flatten here, jobs in dependsOn order within stage order. `script:` and
// `bash:` steps translate; `pwsh`/`powershell` are Windows-shaped and skip
// visibly, as does every `task:` reference (marketplace tasks are hosted
// machinery). `checkout: self` dissolves into the clone. Variables in both
// spellings (map, and name/value list) apply at every level; a pool
// vmImage naming windows or macOS skips its job; a container job's image
// is used.

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

func detectAzure(root string) bool {
	return fileExists(filepath.Join(root, "azure-pipelines.yml")) ||
		fileExists(filepath.Join(root, "azure-pipelines.yaml"))
}

type azStep struct {
	Script      string    `yaml:"script"`
	Bash        string    `yaml:"bash"`
	Pwsh        string    `yaml:"pwsh"`
	Powershell  string    `yaml:"powershell"`
	Task        string    `yaml:"task"`
	Checkout    string    `yaml:"checkout"`
	DisplayName string    `yaml:"displayName"`
	Env         yaml.Node `yaml:"env"`
	WorkingDir  string    `yaml:"workingDirectory"`
}

type azJob struct {
	Job       string    `yaml:"job"`
	DependsOn yaml.Node `yaml:"dependsOn"`
	Pool      azPool    `yaml:"pool"`
	Container yaml.Node `yaml:"container"`
	Variables yaml.Node `yaml:"variables"`
	Steps     []azStep  `yaml:"steps"`
}

type azStage struct {
	Stage     string    `yaml:"stage"`
	DependsOn yaml.Node `yaml:"dependsOn"`
	Jobs      []azJob   `yaml:"jobs"`
}

type azPool struct {
	VmImage string `yaml:"vmImage"`
	Name    string `yaml:"name"`
}

type azPipeline struct {
	Pool      azPool    `yaml:"pool"`
	Container yaml.Node `yaml:"container"`
	Variables yaml.Node `yaml:"variables"`
	Steps     []azStep  `yaml:"steps"`
	Jobs      []azJob   `yaml:"jobs"`
	Stages    []azStage `yaml:"stages"`
}

func loadAzure(root string, repo Repo) ([]Job, error) {
	path := filepath.Join(root, "azure-pipelines.yml")
	if !fileExists(path) {
		path = filepath.Join(root, "azure-pipelines.yaml")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var p azPipeline
	if err := yaml.Unmarshal(data, &p); err != nil {
		return nil, fmt.Errorf("azure-pipelines.yml: %w", err)
	}

	rootVars := azVars(p.Variables)
	rootSkip := azPoolSkip(p.Pool)
	rootImage := azImage(p.Container)

	// Bare steps are an implicit single job.
	jobs := p.Jobs
	if len(jobs) == 0 && len(p.Steps) > 0 {
		jobs = []azJob{{Job: "build", Steps: p.Steps}}
	}

	var out []Job
	emit := func(prefix string, list []azJob) {
		for _, j := range azJobOrder(list) {
			name := j.Job
			if name == "" {
				name = "job"
			}
			if prefix != "" {
				name = prefix + "/" + name
			}
			job := Job{Name: name, Image: azImage(j.Container)}
			if job.Image == "" {
				job.Image = rootImage
			}
			job.SkipReason = rootSkip
			if s := azPoolSkip(j.Pool); s != "" {
				job.SkipReason = s
			}
			env := map[string]string{}
			for k, v := range rootVars {
				env[k] = v
			}
			for k, v := range azVars(j.Variables) {
				env[k] = v
			}
			job.Env = env
			for i, st := range j.Steps {
				if step, ok := azStepToStep(i, st); ok {
					job.Steps = append(job.Steps, step)
				}
			}
			if len(job.Steps) > 0 {
				out = append(out, job)
			}
		}
	}

	if len(p.Stages) > 0 {
		for _, st := range azStageOrder(p.Stages) {
			emit(st.Stage, st.Jobs)
		}
	} else {
		emit("", jobs)
	}
	return out, nil
}

func azStepToStep(i int, s azStep) (Step, bool) {
	name := s.DisplayName
	env := azVars(s.Env)
	switch {
	case s.Checkout != "":
		return Step{Name: "checkout (covered by the clone)", Command: "true"}, true
	case s.Script != "":
		if name == "" {
			name = firstLineOf(s.Script)
		}
		return Step{Name: name, Command: azWrapWdir(s.Script, s.WorkingDir), Env: env}, true
	case s.Bash != "":
		if name == "" {
			name = firstLineOf(s.Bash)
		}
		return Step{Name: name, Command: azWrapWdir(s.Bash, s.WorkingDir), Env: env}, true
	case s.Pwsh != "" || s.Powershell != "":
		if name == "" {
			name = "powershell"
		}
		return Step{
			Name:    name + " (skipped)",
			Command: `echo "skipped PowerShell step — a Linux microVM runs sh/bash"`,
		}, true
	case s.Task != "":
		if name == "" {
			name = s.Task
		}
		return Step{
			Name:    name + " (skipped)",
			Command: fmt.Sprintf(`echo "skipped task: %s — marketplace tasks are not supported locally"`, s.Task),
		}, true
	}
	_ = i
	return Step{}, false
}

func azWrapWdir(cmd, wdir string) string {
	if wdir == "" {
		return cmd
	}
	return "cd " + wdir + " && {\n" + cmd + "\n}"
}

// azVars reads both spellings: `variables: {K: v}` and
// `variables: [{name: K, value: v}]` (group entries are dropped).
func azVars(n yaml.Node) map[string]string {
	if n.IsZero() {
		return nil
	}
	m := map[string]string{}
	if err := n.Decode(&m); err == nil {
		return m
	}
	var list []struct {
		Name  string `yaml:"name"`
		Value string `yaml:"value"`
		Group string `yaml:"group"`
	}
	if err := n.Decode(&list); err == nil {
		out := map[string]string{}
		for _, v := range list {
			if v.Name != "" {
				out[v.Name] = v.Value
			}
		}
		return out
	}
	return nil
}

// azImage reads `container: image-ref` and `container: {image: ref}`.
func azImage(n yaml.Node) string {
	if n.IsZero() {
		return ""
	}
	var s string
	if err := n.Decode(&s); err == nil {
		return s
	}
	var m struct {
		Image string `yaml:"image"`
	}
	if err := n.Decode(&m); err == nil {
		return m.Image
	}
	return ""
}

func azPoolSkip(p azPool) string {
	if p.VmImage == "" {
		return ""
	}
	return linuxOnly(p.VmImage)
}

// azJobOrder / azStageOrder: dependsOn toposort, the shared scheme.
func azJobOrder(jobs []azJob) []azJob {
	names := map[string]bool{}
	for _, j := range jobs {
		names[j.Job] = true
	}
	done := map[string]bool{}
	var order []azJob
	for len(order) < len(jobs) {
		progressed := false
		for _, j := range jobs {
			if done[j.Job] {
				continue
			}
			ok := true
			for _, dep := range yamlStrings(j.DependsOn) {
				if names[dep] && !done[dep] {
					ok = false
					break
				}
			}
			if ok {
				order = append(order, j)
				done[j.Job] = true
				progressed = true
			}
		}
		if !progressed {
			var rest []azJob
			for _, j := range jobs {
				if !done[j.Job] {
					rest = append(rest, j)
					done[j.Job] = true
				}
			}
			sort.SliceStable(rest, func(a, b int) bool { return rest[a].Job < rest[b].Job })
			order = append(order, rest...)
		}
	}
	return order
}

func azStageOrder(stages []azStage) []azStage {
	names := map[string]bool{}
	for _, s := range stages {
		names[s.Stage] = true
	}
	done := map[string]bool{}
	var order []azStage
	for len(order) < len(stages) {
		progressed := false
		for _, s := range stages {
			if done[s.Stage] {
				continue
			}
			ok := true
			for _, dep := range yamlStrings(s.DependsOn) {
				if names[dep] && !done[dep] {
					ok = false
					break
				}
			}
			if ok {
				order = append(order, s)
				done[s.Stage] = true
				progressed = true
			}
		}
		if !progressed {
			for _, s := range stages {
				if !done[s.Stage] {
					order = append(order, s)
					done[s.Stage] = true
				}
			}
		}
	}
	return order
}

var _ = strings.TrimSpace
