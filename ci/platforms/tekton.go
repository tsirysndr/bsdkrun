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
	"regexp"
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
	Name       string   `yaml:"name"`
	Image      string   `yaml:"image"`
	Script     string   `yaml:"script"`
	Command    []string `yaml:"command"`
	Args       []string `yaml:"args"`
	WorkingDir string   `yaml:"workingDir"`
	Env        []struct {
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
				Name     string    `yaml:"name"`
				Resolver string    `yaml:"resolver"`
				Params   []tkParam `yaml:"params"`
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
	runParams := newTkValues()
	for _, m := range manifests {
		switch m.Kind {
		case "Task":
			tasks[m.Metadata.Name] = m
		case "Pipeline":
			pipelines = append(pipelines, m)
		case "PipelineRun":
			for _, p := range m.Spec.Params {
				runParams.set(p.Name, p.Value)
			}
		}
	}

	var out []Job
	used := map[string]bool{}
	catalog := map[string]tkTaskSpec{}
	for _, pl := range pipelines {
		for _, pt := range tkTaskOrder(pl) {
			params := runParams.clone()
			for _, p := range pt.params {
				params.set(p.Name, p.Value)
			}
			var spec tkTaskSpec
			name := pt.name
			if pt.ref != "" {
				if t, ok := tasks[pt.ref]; ok {
					used[pt.ref] = true
					spec = tkTaskSpec{Params: t.Spec.Params, Steps: t.Spec.Steps}
				} else {
					// The checkout does not carry this task: resolve it from
					// the tektoncd catalog, the way the hub resolver would.
					catSpec, err := tkCatalogTask(catalog, pt.ref, pt.refVersion)
					if err != nil {
						out = append(out, Job{Name: name, Steps: []Step{{
							Name:    pt.ref + " (unresolved, skipped)",
							Command: fmt.Sprintf(`echo "skipped task %s — %s"`, pt.ref, strings.ReplaceAll(err.Error(), `"`, `'`)),
						}}})
						continue
					}
					spec = catSpec
				}
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
	name       string
	ref        string
	refVersion string
	spec       *tkTaskSpec
	params     []tkParam
}

// tkCatalogTask fetches and parses a catalog task once per load.
func tkCatalogTask(cache map[string]tkTaskSpec, name, version string) (tkTaskSpec, error) {
	key := name + "@" + version
	if spec, ok := cache[key]; ok {
		return spec, nil
	}
	src, err := TektonCatalogFunc(name, version)
	if err != nil {
		return tkTaskSpec{}, err
	}
	var m tkManifest
	if err := yaml.Unmarshal([]byte(src), &m); err != nil {
		return tkTaskSpec{}, fmt.Errorf("catalog task %s: unparseable manifest: %v", name, err)
	}
	if m.Kind != "Task" || len(m.Spec.Steps) == 0 {
		return tkTaskSpec{}, fmt.Errorf("catalog task %s: manifest is not a Task with steps", name)
	}
	spec := tkTaskSpec{Params: m.Spec.Params, Steps: m.Spec.Steps}
	cache[key] = spec
	return spec, nil
}

func tkTaskOrder(pl tkManifest) []tkOrderedTask {
	var entries []tkOrderedTask
	after := map[string][]string{}
	for _, t := range pl.Spec.Tasks {
		e := tkOrderedTask{
			name: t.Name, ref: t.TaskRef.Name, spec: t.TaskSpec, params: t.Params,
		}
		// The hub resolver spells the reference as params on the ref.
		if t.TaskRef.Resolver == "hub" {
			for _, p := range t.TaskRef.Params {
				v, _ := tkScalar(p.Value)
				switch p.Name {
				case "name":
					e.ref = v
				case "version":
					e.refVersion = v
				}
			}
		}
		entries = append(entries, e)
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

func tkTaskJob(name string, spec tkTaskSpec, params tkValues, repo Repo) (Job, bool) {
	// Declared defaults fill whatever the run and pipeline left unset.
	resolved := newTkValues()
	for _, p := range spec.Params {
		resolved.set(p.Name, p.Default)
	}
	for k, v := range params.scalar {
		resolved.scalar[k] = v
		delete(resolved.array, k)
	}
	for k, v := range params.array {
		resolved.array[k] = v
		delete(resolved.scalar, k)
	}
	subst := func(s string) string {
		for k, v := range resolved.scalar {
			s = strings.ReplaceAll(s, "$(params."+k+")", v)
			s = strings.ReplaceAll(s, "$(params['"+k+"'])", v)
		}
		// The shared workspace is this runner's workspace.
		s = strings.ReplaceAll(s, "$(workspaces.source.path)", repo.Workspace)
		s = strings.ReplaceAll(s, "$(workspaces.output.path)", repo.Workspace)
		return s
	}

	job := Job{Name: name}
	// The VM boots the first step's image; later steps that declare a
	// different one chroot into their own image's pulled rootfs (the
	// drone-plugin machinery), so Tekton's per-step containers hold.
	for _, st := range spec.Steps {
		if img := subst(st.Image); img != "" {
			job.Image = img
			break
		}
	}
	mounted := map[string]string{} // image -> guest dir, one mount per image
	for i, st := range spec.Steps {
		sname := st.Name
		if sname == "" {
			sname = fmt.Sprintf("step %d", i+1)
		}
		env := map[string]string{}
		for _, e := range st.Env {
			env[e.Name] = subst(e.Value)
		}

		var script string
		var argv []string
		switch {
		case st.Script != "":
			script = subst(st.Script)
		case len(st.Command) > 0:
			for _, a := range append(append([]string{}, st.Command...), st.Args...) {
				argv = append(argv, tkSubstArg(a, subst, resolved)...)
			}
		default:
			job.Steps = append(job.Steps, Step{
				Name:    sname + " (empty)",
				Command: `echo "step has neither script nor command"`,
			})
			continue
		}

		img := subst(st.Image)
		if img != "" && img != job.Image {
			step, mount, err := tkChrootStep(sname, img, script, argv, env, subst(st.WorkingDir), repo, mounted)
			if err != nil {
				job.Steps = append(job.Steps, Step{
					Name:    sname + " (image unavailable, skipped)",
					Command: fmt.Sprintf(`echo "skipped step %s — %s"`, sname, strings.ReplaceAll(err.Error(), `"`, `'`)),
				})
				continue
			}
			job.Steps = append(job.Steps, step)
			if mount != "" {
				job.ExtraMounts = append(job.ExtraMounts, mount)
			}
			continue
		}

		if len(env) == 0 {
			env = nil
		}
		cmd := script
		if cmd == "" {
			cmd = shellJoin(argv)
		}
		if wd := subst(st.WorkingDir); wd != "" && wd != "." {
			cmd = fmt.Sprintf("cd %q\n%s", wd, cmd)
		}
		job.Steps = append(job.Steps, Step{Name: sname, Command: cmd, Env: env})
	}
	return job, len(job.Steps) > 0
}

// tkChrootStep runs one step in its own image via the shared chroot
// machinery: rootfs pulled host-side, workspace bound at the same path it
// has outside so substituted $(workspaces.*) values stay valid.
func tkChrootStep(name, image, script string, argv []string, env map[string]string, workdir string, repo Repo, mounted map[string]string) (Step, string, error) {
	mount := ""
	guestImg, ok := mounted[image]
	if !ok {
		pulled, err := PullImageFunc(image)
		if err != nil {
			return Step{}, "", fmt.Errorf("pulling image %s: %w", image, err)
		}
		guestImg = dronePluginMountDir(sanitizeBk(image))
		mounted[image] = guestImg
		mount = pulled.Rootfs + ":" + guestImg + ":ro"
		for _, kv := range pulled.Env {
			if k, v, hasEq := strings.Cut(kv, "="); hasEq {
				if _, set := env[k]; !set {
					env[k] = v
				}
			}
		}
		if script == "" && len(argv) == 0 {
			argv = append(append([]string{}, pulled.Entrypoint...), pulled.Cmd...)
		}
	}
	if workdir == "" {
		workdir = repo.Workspace
	}
	env["HOME"] = "/root"
	return Step{
		Name:    name + " [image: " + image + "]",
		Command: chrootExecScript(guestImg, repo.Workspace, workdir, argv, script),
		Env:     env,
	}, mount, nil
}

// tkValues is a param scope: Tekton params are either strings or arrays,
// and an array is referenced as $(params.x[*]) — one argv element that
// expands into many, not a string that can be substituted in place.
type tkValues struct {
	scalar map[string]string
	array  map[string][]string
}

func newTkValues() tkValues {
	return tkValues{scalar: map[string]string{}, array: map[string][]string{}}
}

func (v tkValues) clone() tkValues {
	out := newTkValues()
	for k, s := range v.scalar {
		out.scalar[k] = s
	}
	for k, a := range v.array {
		out.array[k] = a
	}
	return out
}

// set records a param value under whichever shape it has.
func (v tkValues) set(name string, n yaml.Node) {
	if s, ok := tkScalar(n); ok {
		v.scalar[name] = s
		return
	}
	var list []string
	if err := n.Decode(&list); err == nil {
		v.array[name] = list
	}
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

// tkSubstArg expands one argv element. An element that is exactly an array
// reference becomes that array's elements; everything else is substituted
// as a string. An unset array reference expands to nothing, which is what
// Tekton does with an empty default.
func tkSubstArg(a string, subst func(string) string, vals tkValues) []string {
	for k, list := range vals.array {
		if strings.TrimSpace(a) == "$(params."+k+"[*])" ||
			strings.TrimSpace(a) == "$(params['"+k+"'][*])" {
			out := make([]string, len(list))
			for i, item := range list {
				out[i] = subst(item)
			}
			return out
		}
	}
	if arrayRef.MatchString(strings.TrimSpace(a)) {
		return nil // a declared-but-unset array reference
	}
	return []string{subst(a)}
}

var arrayRef = regexp.MustCompile(`^\$\(params(\.[A-Za-z0-9_-]+|\['[^']+'\])\[\*\]\)$`)

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
