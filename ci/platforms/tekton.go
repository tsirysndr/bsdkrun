package platforms

// Tekton: Kubernetes manifests under .tekton/ — Tasks (steps with an image
// and a script or command), Pipelines (tasks in runAfter order, by taskRef
// or embedded taskSpec), and PipelineRuns (read only for param values).
//
// Each Tekton Task becomes one job: its steps run serially in one VM, on
// the first step's image when steps disagree (announced, drone-style —
// Tekton's per-step containers share a pod's workspace the way these steps
// share the VM's). `$(params.x)` is substituted from PipelineRun params,
// pipeline task params, or the param's declared default, in that order;
// a param with no value anywhere is left visibly unresolved. Workspace
// references resolve to the run's own workspace directory. Tasks that only
// a Pipeline references by name run in that pipeline's order; standalone
// Tasks run alphabetically after.

import (
	"fmt"
	"os"
	"strings"

	"gopkg.in/yaml.v3"
)

func detectTekton(root string) bool {
	return len(yamlFiles(root+"/.tekton")) > 0
}

type tkParam struct {
	Name    string    `yaml:"name"`
	Default yaml.Node `yaml:"default"`
	Value   yaml.Node `yaml:"value"`
}

type tkStep struct {
	Name    string   `yaml:"name"`
	Image   string   `yaml:"image"`
	Script  string   `yaml:"script"`
	Command []string `yaml:"command"`
	Args    []string `yaml:"args"`
	Env     []struct {
		Name  string `yaml:"name"`
		Value string `yaml:"value"`
	} `yaml:"env"`
}

type tkTaskSpec struct {
	Params []tkParam `yaml:"params"`
	Steps  []tkStep  `yaml:"steps"`
}

type tkManifest struct {
	Kind     string `yaml:"kind"`
	Metadata struct {
		Name string `yaml:"name"`
	} `yaml:"metadata"`
	Spec struct {
		// Task
		Params []tkParam `yaml:"params"`
		Steps  []tkStep  `yaml:"steps"`
		// Pipeline
		Tasks []struct {
			Name     string   `yaml:"name"`
			RunAfter []string `yaml:"runAfter"`
			TaskRef  struct {
				Name string `yaml:"name"`
			} `yaml:"taskRef"`
			TaskSpec *tkTaskSpec `yaml:"taskSpec"`
			Params   []tkParam   `yaml:"params"`
		} `yaml:"tasks"`
		// PipelineRun
		PipelineRef struct {
			Name string `yaml:"name"`
		} `yaml:"pipelineRef"`
	} `yaml:"spec"`
}

func loadTekton(root string, repo Repo) ([]Job, error) {
	var manifests []tkManifest
	for _, f := range yamlFiles(root + "/.tekton") {
		data, err := os.ReadFile(f)
		if err != nil {
			return nil, err
		}
		dec := yaml.NewDecoder(strings.NewReader(string(data)))
		for {
			var m tkManifest
			if err := dec.Decode(&m); err != nil {
				break
			}
			if m.Kind != "" {
				manifests = append(manifests, m)
			}
		}
	}

	tasks := map[string]tkManifest{}
	var pipelines []tkManifest
	runParams := map[string]string{}
	for _, m := range manifests {
		switch m.Kind {
		case "Task":
			tasks[m.Metadata.Name] = m
		case "Pipeline":
			pipelines = append(pipelines, m)
		case "PipelineRun":
			for _, p := range m.Spec.Params {
				if v, ok := tkScalar(p.Value); ok {
					runParams[p.Name] = v
				}
			}
		}
	}

	var out []Job
	used := map[string]bool{}
	for _, pl := range pipelines {
		for _, pt := range tkTaskOrder(pl) {
			params := map[string]string{}
			for k, v := range runParams {
				params[k] = v
			}
			for _, p := range pt.params {
				if v, ok := tkScalar(p.Value); ok {
					params[p.Name] = v
				}
			}
			var spec tkTaskSpec
			name := pt.name
			if pt.ref != "" {
				t, ok := tasks[pt.ref]
				if !ok {
					continue // a cluster task this checkout does not carry
				}
				used[pt.ref] = true
				spec = tkTaskSpec{Params: t.Spec.Params, Steps: t.Spec.Steps}
			} else if pt.spec != nil {
				spec = *pt.spec
			} else {
				continue
			}
			if job, ok := tkTaskJob(name, spec, params, repo); ok {
				out = append(out, job)
			}
		}
	}
	// Standalone tasks nothing referenced still run, after the pipelines.
	for _, f := range sortedKeys(tasks) {
		if used[f] {
			continue
		}
		t := tasks[f]
		if job, ok := tkTaskJob(t.Metadata.Name,
			tkTaskSpec{Params: t.Spec.Params, Steps: t.Spec.Steps}, runParams, repo); ok {
			out = append(out, job)
		}
	}
	return out, nil
}

type tkOrderedTask struct {
	name   string
	ref    string
	spec   *tkTaskSpec
	params []tkParam
}

func tkTaskOrder(pl tkManifest) []tkOrderedTask {
	var entries []tkOrderedTask
	after := map[string][]string{}
	for _, t := range pl.Spec.Tasks {
		entries = append(entries, tkOrderedTask{
			name: t.Name, ref: t.TaskRef.Name, spec: t.TaskSpec, params: t.Params,
		})
		after[t.Name] = t.RunAfter
	}
	done := map[string]bool{}
	var order []tkOrderedTask
	for len(order) < len(entries) {
		progressed := false
		for _, e := range entries {
			if done[e.name] {
				continue
			}
			ok := true
			for _, dep := range after[e.name] {
				if !done[dep] {
					ok = false
					break
				}
			}
			if ok {
				order = append(order, e)
				done[e.name] = true
				progressed = true
			}
		}
		if !progressed {
			for _, e := range entries {
				if !done[e.name] {
					order = append(order, e)
					done[e.name] = true
				}
			}
		}
	}
	return order
}

func tkTaskJob(name string, spec tkTaskSpec, params map[string]string, repo Repo) (Job, bool) {
	// Declared defaults fill whatever the run and pipeline left unset.
	resolved := map[string]string{}
	for _, p := range spec.Params {
		if v, ok := tkScalar(p.Default); ok {
			resolved[p.Name] = v
		}
	}
	for k, v := range params {
		resolved[k] = v
	}
	subst := func(s string) string {
		for k, v := range resolved {
			s = strings.ReplaceAll(s, "$(params."+k+")", v)
			s = strings.ReplaceAll(s, "$(params['"+k+"'])", v)
		}
		// The shared workspace is this runner's workspace.
		s = strings.ReplaceAll(s, "$(workspaces.source.path)", repo.Workspace)
		s = strings.ReplaceAll(s, "$(workspaces.output.path)", repo.Workspace)
		return s
	}

	job := Job{Name: name}
	var divergent []string
	for _, st := range spec.Steps {
		img := subst(st.Image)
		if img != "" && job.Image == "" {
			job.Image = img
		} else if img != "" && img != job.Image {
			divergent = append(divergent, fmt.Sprintf("%s (%s)", st.Name, img))
		}
	}
	if len(divergent) > 0 {
		job.Steps = append(job.Steps, Step{
			Name: "per-step images (not supported)",
			Command: fmt.Sprintf(
				`echo "steps declaring their own images run on %s here: %s"`,
				job.Image, strings.Join(divergent, ", ")),
		})
	}
	for i, st := range spec.Steps {
		sname := st.Name
		if sname == "" {
			sname = fmt.Sprintf("step %d", i+1)
		}
		env := map[string]string{}
		for _, e := range st.Env {
			env[e.Name] = subst(e.Value)
		}
		if len(env) == 0 {
			env = nil
		}
		switch {
		case st.Script != "":
			job.Steps = append(job.Steps, Step{Name: sname, Command: subst(st.Script), Env: env})
		case len(st.Command) > 0:
			argv := append(append([]string{}, st.Command...), st.Args...)
			for i := range argv {
				argv[i] = subst(argv[i])
			}
			job.Steps = append(job.Steps, Step{Name: sname, Command: shellJoin(argv), Env: env})
		default:
			job.Steps = append(job.Steps, Step{
				Name:    sname + " (empty)",
				Command: `echo "step has neither script nor command"`,
			})
		}
	}
	return job, len(job.Steps) > 0
}

// tkScalar reads a scalar param value; arrays and objects do not translate.
func tkScalar(n yaml.Node) (string, bool) {
	if n.IsZero() {
		return "", false
	}
	var s string
	if err := n.Decode(&s); err == nil {
		return s, true
	}
	return "", false
}

func shellJoin(argv []string) string {
	parts := make([]string, len(argv))
	for i, a := range argv {
		parts[i] = shellQuote(a)
	}
	return strings.Join(parts, " ")
}

func sortedKeys(m map[string]tkManifest) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	// alphabetical, for a stable run order
	for i := 1; i < len(keys); i++ {
		for j := i; j > 0 && keys[j] < keys[j-1]; j-- {
			keys[j], keys[j-1] = keys[j-1], keys[j]
		}
	}
	return keys
}
