package platforms

// CircleCI orbs, expanded at plan time. An orb is YAML — commands, jobs,
// executors, all parameterized with << parameters.x >> tokens — published
// to CircleCI's registry and fetchable unauthenticated for public orbs via
// the graphql-unstable endpoint (the same source `circleci orb source`
// prints). The registry resolves partial versions itself: @5 means the
// newest 5.x, no version means @volatile.
//
// Expansion is textual and honest: run steps are substituted and emitted,
// nested command references recurse, when/unless conditions are evaluated
// where they are scalar/equal/and/or/not over already-substituted values,
// and cache/workspace/artifact steps become visible no-ops — a local run
// has no cross-run cache to restore. What cannot be expanded (an orb that
// fails to fetch, a reference into an orb we do not have) stays a visible
// skip, never a silent one.

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"sort"
	"strings"
	"time"

	"gopkg.in/yaml.v3"
)

// OrbFetchFunc fetches an orb's YAML source by ref ("circleci/node@5.2.0").
// Package-level and swappable so tests inject fixtures.
var OrbFetchFunc func(ref string) (string, error) = fetchOrbSource

func fetchOrbSource(ref string) (string, error) {
	body, _ := json.Marshal(map[string]interface{}{
		"query":     `query($ref: String!){orbVersion(orbVersionRef: $ref){source}}`,
		"variables": map[string]string{"ref": ref},
	})
	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Post("https://circleci.com/graphql-unstable", "application/json", bytes.NewReader(body))
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return "", err
	}
	var out struct {
		Data struct {
			OrbVersion *struct {
				Source string `json:"source"`
			} `json:"orbVersion"`
		} `json:"data"`
	}
	if err := json.Unmarshal(data, &out); err != nil {
		return "", fmt.Errorf("orb registry answered non-JSON (%d bytes)", len(data))
	}
	if out.Data.OrbVersion == nil || out.Data.OrbVersion.Source == "" {
		return "", fmt.Errorf("orb %s not found in the registry", ref)
	}
	return out.Data.OrbVersion.Source, nil
}

type ccOrbParam struct {
	Type    string    `yaml:"type"`
	Default yaml.Node `yaml:"default"`
}

type ccOrbCommand struct {
	Parameters map[string]ccOrbParam `yaml:"parameters"`
	Steps      []yaml.Node           `yaml:"steps"`
}

type ccOrbExecutor struct {
	Parameters map[string]ccOrbParam `yaml:"parameters"`
	Docker     []struct {
		Image string `yaml:"image"`
	} `yaml:"docker"`
	Machine yaml.Node `yaml:"machine"`
	Macos   yaml.Node `yaml:"macos"`
}

type ccOrbJob struct {
	Parameters map[string]ccOrbParam `yaml:"parameters"`
	Executor   yaml.Node             `yaml:"executor"`
	Docker     []struct {
		Image string `yaml:"image"`
	} `yaml:"docker"`
	Macos       yaml.Node         `yaml:"macos"`
	Environment map[string]string `yaml:"environment"`
	Steps       []yaml.Node       `yaml:"steps"`
}

type ccOrb struct {
	Commands  map[string]ccOrbCommand  `yaml:"commands"`
	Jobs      map[string]ccOrbJob      `yaml:"jobs"`
	Executors map[string]ccOrbExecutor `yaml:"executors"`
}

// ccOrbSet is every orb the config imports, keyed by its local alias, plus
// per-alias fetch failures so a broken import degrades to visible skips.
type ccOrbSet struct {
	orbs   map[string]*ccOrb
	broken map[string]string // alias -> why
}

// ccLoadOrbs parses the config's `orbs:` section: string values are
// registry refs to fetch, map values are inline orb definitions.
func ccLoadOrbs(section map[string]yaml.Node) ccOrbSet {
	set := ccOrbSet{orbs: map[string]*ccOrb{}, broken: map[string]string{}}
	for alias, n := range section {
		var ref string
		if err := n.Decode(&ref); err == nil {
			if !strings.Contains(ref, "@") {
				ref += "@volatile"
			}
			src, err := OrbFetchFunc(ref)
			if err != nil {
				set.broken[alias] = err.Error()
				continue
			}
			var orb ccOrb
			if err := yaml.Unmarshal([]byte(src), &orb); err != nil {
				set.broken[alias] = fmt.Sprintf("orb %s: unparseable source: %v", ref, err)
				continue
			}
			set.orbs[alias] = &orb
			continue
		}
		var orb ccOrb
		if err := n.Decode(&orb); err != nil {
			set.broken[alias] = fmt.Sprintf("inline orb: %v", err)
			continue
		}
		set.orbs[alias] = &orb
	}
	return set
}

var ccParamToken = regexp.MustCompile(`<<\s*parameters\.([A-Za-z0-9_-]+)\s*>>`)

func ccSubst(s string, params map[string]string) string {
	return ccParamToken.ReplaceAllStringFunc(s, func(m string) string {
		name := ccParamToken.FindStringSubmatch(m)[1]
		if v, ok := params[name]; ok {
			return v
		}
		return m
	})
}

// ccScalarString renders a YAML scalar the way CircleCI interpolates it.
func ccScalarString(n yaml.Node) string {
	var v interface{}
	if err := n.Decode(&v); err != nil || v == nil {
		return ""
	}
	switch val := v.(type) {
	case bool:
		return fmt.Sprintf("%t", val)
	case string:
		return val
	default:
		return fmt.Sprintf("%v", val)
	}
}

// ccParamValues resolves a declaration's defaults overlaid with the caller's
// arguments, both as substitution strings and — for steps-type parameters —
// as raw step nodes to splice.
func ccParamValues(decl map[string]ccOrbParam, args map[string]yaml.Node, outer map[string]string) (map[string]string, map[string][]yaml.Node) {
	strs := map[string]string{}
	steps := map[string][]yaml.Node{}
	for name, p := range decl {
		if p.Type == "steps" {
			if !p.Default.IsZero() {
				_ = p.Default.Decode(&[]yaml.Node{}) // shape check only
				var def []yaml.Node
				if p.Default.Decode(&def) == nil {
					steps[name] = def
				}
			}
			continue
		}
		if !p.Default.IsZero() {
			strs[name] = ccScalarString(p.Default)
		} else {
			strs[name] = ""
		}
	}
	for name, n := range args {
		var list []yaml.Node
		if decl[name].Type == "steps" {
			if n.Decode(&list) == nil {
				steps[name] = list
			}
			continue
		}
		// An argument may itself reference the caller's parameters.
		strs[name] = ccSubst(ccScalarString(n), outer)
	}
	return strs, steps
}

// ccTruthy follows CircleCI's logic rules: false, null, 0 and the empty
// string are false, everything else true.
func ccTruthy(s string) bool {
	switch strings.TrimSpace(s) {
	case "", "false", "null", "0":
		return false
	}
	return true
}

// ccCondition evaluates a when/unless condition after substitution.
// Supported: scalars, equal, and, or, not. Anything richer reports itself
// unsupported so the caller can skip visibly instead of guessing.
func ccCondition(n yaml.Node, params map[string]string) (bool, error) {
	var scalar string
	if n.Kind == yaml.ScalarNode {
		if err := n.Decode(&scalar); err == nil {
			return ccTruthy(ccSubst(scalar, params)), nil
		}
	}
	var m map[string]yaml.Node
	if err := n.Decode(&m); err != nil {
		return false, fmt.Errorf("condition is neither scalar nor map")
	}
	if eq, ok := m["equal"]; ok {
		var items []yaml.Node
		if err := eq.Decode(&items); err != nil || len(items) == 0 {
			return false, fmt.Errorf("equal wants a list")
		}
		first := ccSubst(ccScalarString(items[0]), params)
		for _, it := range items[1:] {
			if ccSubst(ccScalarString(it), params) != first {
				return false, nil
			}
		}
		return true, nil
	}
	if andN, ok := m["and"]; ok {
		var items []yaml.Node
		if err := andN.Decode(&items); err != nil {
			return false, fmt.Errorf("and wants a list")
		}
		for _, it := range items {
			ok, err := ccCondition(it, params)
			if err != nil || !ok {
				return false, err
			}
		}
		return true, nil
	}
	if orN, ok := m["or"]; ok {
		var items []yaml.Node
		if err := orN.Decode(&items); err != nil {
			return false, fmt.Errorf("or wants a list")
		}
		for _, it := range items {
			ok, err := ccCondition(it, params)
			if err != nil {
				return false, err
			}
			if ok {
				return true, nil
			}
		}
		return false, nil
	}
	if notN, ok := m["not"]; ok {
		ok, err := ccCondition(notN, params)
		return !ok, err
	}
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return false, fmt.Errorf("unsupported condition %v", keys)
}

const ccMaxOrbDepth = 12 // orbs nest commands in commands; runaway means a cycle

// ccExpandSteps turns a list of orb (or config) step nodes into concrete
// Steps under a parameter scope. orbName scopes bare command references.
func ccExpandSteps(set ccOrbSet, orbName string, nodes []yaml.Node, params map[string]string, stepParams map[string][]yaml.Node, depth int) []Step {
	if depth > ccMaxOrbDepth {
		return []Step{{
			Name:    "orb expansion too deep (skipped)",
			Command: `echo "skipped: orb command nesting exceeded the depth limit — likely a reference cycle"`,
		}}
	}
	var out []Step
	for _, n := range nodes {
		out = append(out, ccExpandStep(set, orbName, n, params, stepParams, depth)...)
	}
	return out
}

func ccExpandStep(set ccOrbSet, orbName string, n yaml.Node, params map[string]string, stepParams map[string][]yaml.Node, depth int) []Step {
	// A plain-string step: checkout, a steps-parameter splice, or a
	// no-argument command reference.
	var plain string
	if n.Kind == yaml.ScalarNode && n.Decode(&plain) == nil {
		if m := ccParamToken.FindStringSubmatch(strings.TrimSpace(plain)); m != nil {
			return ccExpandSteps(set, orbName, stepParams[m[1]], params, nil, depth+1)
		}
		if steps, ok := ccResolveCommandRef(set, orbName, plain, nil, params, depth); ok {
			return steps
		}
		return []Step{ccStep(0, n)}
	}

	var m map[string]yaml.Node
	if err := n.Decode(&m); err != nil || len(m) != 1 {
		return []Step{ccStep(0, n)}
	}
	var key string
	var val yaml.Node
	for k, v := range m {
		key, val = k, v
	}

	switch key {
	case "run":
		var cmd string
		if val.Decode(&cmd) == nil {
			cmd = ccSubst(cmd, params)
			return []Step{{Name: firstLineOf(cmd), Command: cmd}}
		}
		var run struct {
			Name       string            `yaml:"name"`
			Command    string            `yaml:"command"`
			Env        map[string]string `yaml:"environment"`
			WorkingDir string            `yaml:"working_directory"`
		}
		if val.Decode(&run) != nil {
			return []Step{ccStep(0, n)}
		}
		cmd = ccSubst(run.Command, params)
		if wd := ccSubst(run.WorkingDir, params); wd != "" && wd != "." {
			cmd = fmt.Sprintf("cd %q\n%s", wd, cmd)
		}
		env := map[string]string{}
		for k, v := range run.Env {
			env[k] = ccSubst(v, params)
		}
		name := ccSubst(run.Name, params)
		if name == "" {
			name = firstLineOf(cmd)
		}
		return []Step{{Name: name, Command: cmd, Env: env}}
	case "when", "unless":
		var w struct {
			Condition yaml.Node   `yaml:"condition"`
			Steps     []yaml.Node `yaml:"steps"`
		}
		if val.Decode(&w) != nil {
			return []Step{ccStep(0, n)}
		}
		ok, err := ccCondition(w.Condition, params)
		if err != nil {
			return []Step{{
				Name:    key + " (skipped)",
				Command: fmt.Sprintf(`echo "skipped conditional steps — %s"`, err),
			}}
		}
		if key == "unless" {
			ok = !ok
		}
		if !ok {
			return nil // the condition ruled the steps out; that is not a skip
		}
		return ccExpandSteps(set, orbName, w.Steps, params, stepParams, depth+1)
	case "steps":
		// `steps: << parameters.x >>` — the map spelling of a splice.
		var ref string
		if val.Decode(&ref) == nil {
			if m := ccParamToken.FindStringSubmatch(strings.TrimSpace(ref)); m != nil {
				return ccExpandSteps(set, orbName, stepParams[m[1]], params, nil, depth+1)
			}
		}
		var list []yaml.Node
		if val.Decode(&list) == nil {
			return ccExpandSteps(set, orbName, list, params, stepParams, depth+1)
		}
		return []Step{ccStep(0, n)}
	case "restore_cache", "save_cache", "persist_to_workspace", "attach_workspace",
		"store_artifacts", "store_test_results", "setup_remote_docker", "add_ssh_keys":
		return []Step{{
			Name:    key + " (no-op locally)",
			Command: "true",
		}}
	}

	// A command reference with arguments — same-orb bare name or
	// alias/command into another imported orb.
	var args map[string]yaml.Node
	if val.Decode(&args) != nil {
		args = nil
	}
	if steps, ok := ccResolveCommandRef(set, orbName, key, args, params, depth); ok {
		return steps
	}
	return []Step{ccStep(0, n)}
}

// ccResolveCommandRef expands `name` as an orb command if it is one:
// "cmd" inside the current orb, or "alias/cmd" through the import set.
func ccResolveCommandRef(set ccOrbSet, orbName, name string, args map[string]yaml.Node, outer map[string]string, depth int) ([]Step, bool) {
	targetOrb, cmdName := orbName, name
	if alias, rest, ok := strings.Cut(name, "/"); ok {
		targetOrb, cmdName = alias, rest
	}
	if why, ok := set.broken[targetOrb]; ok {
		return []Step{{
			Name:    name + " (orb unavailable, skipped)",
			Command: fmt.Sprintf(`echo "skipped %s — %s"`, name, strings.ReplaceAll(why, `"`, `'`)),
		}}, true
	}
	orb := set.orbs[targetOrb]
	if orb == nil {
		return nil, false
	}
	cmd, ok := orb.Commands[cmdName]
	if !ok {
		return nil, false
	}
	params, stepParams := ccParamValues(cmd.Parameters, args, outer)
	steps := ccExpandSteps(set, targetOrb, cmd.Steps, params, stepParams, depth+1)
	for i := range steps {
		steps[i].Name = name + ": " + steps[i].Name
	}
	return steps, true
}

// ccExpandOrbJob expands a workflow reference to an orb job ("alias/job")
// into a full Job: executor resolved to an image, steps expanded.
func ccExpandOrbJob(set ccOrbSet, jobName, refName string, args map[string]yaml.Node) (Job, bool) {
	alias, jname, ok := strings.Cut(refName, "/")
	if !ok {
		return Job{}, false
	}
	if why, broken := set.broken[alias]; broken {
		return Job{Name: jobName, Steps: []Step{{
			Name:    refName + " (orb unavailable, skipped)",
			Command: fmt.Sprintf(`echo "skipped job %s — %s"`, refName, strings.ReplaceAll(why, `"`, `'`)),
		}}}, true
	}
	orb := set.orbs[alias]
	if orb == nil {
		return Job{}, false
	}
	oj, ok := orb.Jobs[jname]
	if !ok {
		return Job{}, false
	}

	params, stepParams := ccParamValues(oj.Parameters, args, nil)
	job := Job{Name: jobName, Env: map[string]string{}}
	for k, v := range oj.Environment {
		job.Env[k] = ccSubst(v, params)
	}

	if !oj.Macos.IsZero() {
		job.SkipReason = "macos orb job — a Linux microVM cannot run it"
	}
	if len(oj.Docker) > 0 {
		job.Image = ccSubst(oj.Docker[0].Image, params)
	} else if !oj.Executor.IsZero() {
		job.Image, job.SkipReason = ccResolveExecutor(orb, oj.Executor, params)
	}

	job.Steps = ccExpandSteps(set, alias, oj.Steps, params, stepParams, 0)
	return job, true
}

// ccResolveExecutor turns an orb job's executor reference into an image
// (or a skip reason for macos executors).
func ccResolveExecutor(orb *ccOrb, ref yaml.Node, outer map[string]string) (image, skipReason string) {
	execName := ""
	execArgs := map[string]yaml.Node{}
	if err := ref.Decode(&execName); err != nil {
		var m map[string]yaml.Node
		if err := ref.Decode(&m); err != nil {
			return "", ""
		}
		if n, ok := m["name"]; ok {
			_ = n.Decode(&execName)
			delete(m, "name")
			execArgs = m
		}
	}
	ex, ok := orb.Executors[execName]
	if !ok {
		return "", ""
	}
	params, _ := ccParamValues(ex.Parameters, execArgs, outer)
	if !ex.Macos.IsZero() {
		return "", "macos executor — a Linux microVM cannot run it"
	}
	if len(ex.Docker) > 0 {
		return ccSubst(ex.Docker[0].Image, params), ""
	}
	return "", ""
}
