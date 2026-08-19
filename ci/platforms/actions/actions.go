// Package actions is a real GitHub Actions runner for `uses:` steps — not a
// curated allowlist. Any JavaScript or composite action runs: at plan time
// the action's action.yml is fetched (host-side, cached) and parsed to
// learn what the action *is*; at run time the guest clones the action at
// its ref and executes it under the genuine Actions protocol — `INPUT_*`
// variables from `with:` (dashes preserved, exactly as the real runner
// sets them), GITHUB_ENV / GITHUB_PATH / GITHUB_OUTPUT command files whose
// effects persist into every later step, RUNNER_* identity, and a node
// runtime provisioned once per VM for JavaScript actions.
//
// The honest limits, stated rather than papered over: container actions
// need a Docker daemon a microVM does not run — visible skip; `pre`/`post`
// hooks (cache save, problem matchers) are not executed; `if:` expressions
// inside composite actions are not evaluated (their steps run). Expression
// interpolation covers the lookups that carry real workflows —
// `${{ inputs.* }}`, `${{ github.* }}`, `${{ env.* }}`,
// `${{ steps.<id>.outputs.* }}` — the last resolved in the guest at run
// time, where the outputs actually live.
package actions

import (
	"fmt"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"
)

// Step is one translated step, decoupled from the platforms package so the
// github translator can depend on this package without a cycle.
type Step struct {
	Name    string
	Command string
	Env     map[string]string
}

// Repo is what expression interpolation may know about the checkout.
type Repo struct {
	Sha       string
	Branch    string
	Name      string
	Workspace string
}

// Metadata is the parsed action.yml — the parts execution needs.
type Metadata struct {
	Name   string `yaml:"name"`
	Inputs map[string]struct {
		Default  string `yaml:"default"`
		Required bool   `yaml:"required"`
	} `yaml:"inputs"`
	Runs struct {
		Using string `yaml:"using"`
		Main  string `yaml:"main"`
		Steps []struct {
			Name  string            `yaml:"name"`
			Run   string            `yaml:"run"`
			Shell string            `yaml:"shell"`
			Uses  string            `yaml:"uses"`
			With  map[string]string `yaml:"with"`
			Env   map[string]string `yaml:"env"`
		} `yaml:"steps"`
	} `yaml:"runs"`
}

// Ref is a parsed `uses:` reference.
type Ref struct {
	Owner string
	Repo  string
	// Path inside the repository, for owner/repo/subdir actions.
	Path string
	Ref  string
}

func (r Ref) Slug() string {
	s := r.Owner + "/" + r.Repo
	if r.Path != "" {
		s += "/" + r.Path
	}
	return s
}

// CloneDir is where the guest checks the action out.
func (r Ref) CloneDir() string {
	return "/tangled/.gha/actions/" + r.Owner + "-" + r.Repo + "-" + sanitizeRef(r.Ref)
}

func sanitizeRef(s string) string {
	return strings.Map(func(c rune) rune {
		switch {
		case c >= 'a' && c <= 'z', c >= 'A' && c <= 'Z', c >= '0' && c <= '9', c == '.', c == '-', c == '_':
			return c
		}
		return '-'
	}, s)
}

// ParseRef splits `owner/repo[/path]@ref`. Local (./) and docker:// forms
// return ok=false — the caller renders their own skip.
func ParseRef(uses string) (Ref, bool) {
	if strings.HasPrefix(uses, "./") || strings.HasPrefix(uses, "docker://") {
		return Ref{}, false
	}
	spec, ref, ok := strings.Cut(uses, "@")
	if !ok || ref == "" {
		return Ref{}, false
	}
	parts := strings.SplitN(spec, "/", 3)
	if len(parts) < 2 || parts[0] == "" || parts[1] == "" {
		return Ref{}, false
	}
	out := Ref{Owner: parts[0], Repo: parts[1], Ref: ref}
	if len(parts) == 3 {
		out.Path = parts[2]
	}
	return out, true
}

// Translate turns one `uses:` step into runnable steps. `stepID` is the
// workflow step's id (for outputs); `env` is the step-level env. The
// returned needsNode reports whether a JavaScript action is among them —
// the caller provisions the runtime once per job.
func Translate(uses, stepID string, with, env map[string]string, repo Repo) (steps []Step, needsNode bool, err error) {
	return translate(uses, stepID, with, env, repo, 0)
}

func translate(uses, stepID string, with, env map[string]string, repo Repo, depth int) ([]Step, bool, error) {
	if depth > 3 {
		return nil, false, fmt.Errorf("composite actions nested deeper than 3 (%s)", uses)
	}
	if strings.HasPrefix(uses, "docker://") {
		return nil, false, fmt.Errorf("container action %s — a microVM runs no Docker daemon", uses)
	}
	ref, ok := ParseRef(uses)
	if !ok {
		return nil, false, fmt.Errorf("unsupported uses reference %q", uses)
	}
	// checkout is genuinely covered by the runner's own clone; running the
	// real action would fight it over the same directory.
	if ref.Owner == "actions" && ref.Repo == "checkout" {
		return []Step{{Name: uses + " (covered by the clone)", Command: "true"}}, false, nil
	}

	meta, err := Fetch(ref)
	if err != nil {
		return nil, false, fmt.Errorf("fetching %s's action.yml: %w", ref.Slug(), err)
	}

	inputs := resolveInputs(meta, with, repo)

	switch {
	case strings.HasPrefix(meta.Runs.Using, "node"):
		return []Step{jsStep(uses, ref, meta, stepID, inputs, env)}, true, nil
	case meta.Runs.Using == "composite":
		return compositeSteps(uses, ref, meta, stepID, inputs, env, repo, depth)
	case meta.Runs.Using == "docker":
		return nil, false, fmt.Errorf("%s is a container action — a microVM runs no Docker daemon", ref.Slug())
	default:
		return nil, false, fmt.Errorf("%s uses %q, which this runner cannot host", ref.Slug(), meta.Runs.Using)
	}
}

// resolveInputs merges action defaults under the workflow's `with:`, and
// interpolates the github context both may reference.
func resolveInputs(meta *Metadata, with map[string]string, repo Repo) map[string]string {
	out := map[string]string{}
	for name, in := range meta.Inputs {
		if in.Default != "" {
			out[name] = Interpolate(in.Default, nil, repo)
		}
	}
	for k, v := range with {
		out[k] = Interpolate(v, nil, repo)
	}
	return out
}

// jsStep runs a JavaScript action: clone at ref, `node <main>` under the
// Actions protocol. The env-file plumbing itself lives in protocol.go and
// wraps every step of the job, this one included.
func jsStep(uses string, ref Ref, meta *Metadata, stepID string, inputs, env map[string]string) Step {
	stepEnv := map[string]string{}
	for k, v := range env {
		stepEnv[k] = v
	}
	for name, value := range inputs {
		// The real runner preserves dashes in INPUT_ names; execve env
		// accepts them even though shell `export` would not.
		stepEnv["INPUT_"+strings.ToUpper(strings.ReplaceAll(name, " ", "_"))] = value
	}
	name := meta.Name
	if name == "" {
		name = uses
	}
	return Step{
		Name: name,
		Command: cloneSnippet(ref) + "\n" +
			"export GITHUB_ACTION=" + shellQuote(stepID) + "\n" +
			"node " + shellQuote(ref.CloneDir()+"/"+actionMain(ref, meta)) + "\n",
		Env: stepEnv,
	}
}

func actionMain(ref Ref, meta *Metadata) string {
	main := meta.Runs.Main
	if ref.Path != "" {
		main = ref.Path + "/" + main
	}
	return main
}

// compositeSteps expands a composite action into its own steps, run: and
// nested uses: alike, with `${{ inputs.* }}` interpolated.
func compositeSteps(uses string, ref Ref, meta *Metadata, stepID string, inputs, env map[string]string, repo Repo, depth int) ([]Step, bool, error) {
	var out []Step
	needsNode := false
	// The composite's own files may be referenced via github.action_path.
	prelude := cloneSnippet(ref) + "\nexport GITHUB_ACTION_PATH=" + shellQuote(actionDir(ref)) + "\n"
	first := true
	for i, cs := range meta.Runs.Steps {
		stepEnv := map[string]string{}
		for k, v := range env {
			stepEnv[k] = v
		}
		for k, v := range cs.Env {
			stepEnv[k] = Interpolate(v, inputs, repo)
		}
		switch {
		case cs.Uses != "":
			nested, nn, err := translate(Interpolate(cs.Uses, inputs, repo),
				fmt.Sprintf("%s-%d", stepID, i), interpolateMap(cs.With, inputs, repo), stepEnv, repo, depth+1)
			if err != nil {
				out = append(out, Step{
					Name:    cs.Uses + " (skipped)",
					Command: fmt.Sprintf(`echo "skipped nested action: %s"`, err),
				})
				continue
			}
			out = append(out, nested...)
			needsNode = needsNode || nn
		case cs.Run != "":
			name := cs.Name
			if name == "" {
				name = fmt.Sprintf("%s step %d", uses, i+1)
			}
			cmd := Interpolate(cs.Run, inputs, repo)
			cmd = strings.ReplaceAll(cmd, "$GITHUB_ACTION_PATH", actionDir(ref))
			cmd = strings.ReplaceAll(cmd, "${GITHUB_ACTION_PATH}", actionDir(ref))
			body := cmd
			if first {
				body = prelude + cmd
				first = false
			} else {
				body = "export GITHUB_ACTION_PATH=" + shellQuote(actionDir(ref)) + "\n" + cmd
			}
			out = append(out, Step{Name: name, Command: body, Env: stepEnv})
		}
	}
	if len(out) == 0 {
		return nil, false, fmt.Errorf("%s is a composite action with no runnable steps", ref.Slug())
	}
	return out, needsNode, nil
}

func actionDir(ref Ref) string {
	dir := ref.CloneDir()
	if ref.Path != "" {
		dir += "/" + ref.Path
	}
	return dir
}

// cloneSnippet fetches the action's code in the guest, once per VM.
func cloneSnippet(ref Ref) string {
	dir := ref.CloneDir()
	url := "https://github.com/" + ref.Owner + "/" + ref.Repo
	return fmt.Sprintf(
		`[ -d %s ] || git clone --quiet --depth 1 --branch %s %s %s 2>/dev/null || { git clone --quiet %s %s && git -C %s checkout --quiet %s; }`,
		shellQuote(dir), shellQuote(ref.Ref), shellQuote(url), shellQuote(dir),
		shellQuote(url), shellQuote(dir), shellQuote(dir), shellQuote(ref.Ref))
}

// Interpolate substitutes the expression lookups real workflows lean on.
// `steps.<id>.outputs.<name>` resolves in the guest at run time — that is
// where the output files live — via command substitution.
func Interpolate(s string, inputs map[string]string, repo Repo) string {
	for {
		start := strings.Index(s, "${{")
		if start < 0 {
			return s
		}
		end := strings.Index(s[start:], "}}")
		if end < 0 {
			return s
		}
		expr := strings.TrimSpace(s[start+3 : start+end])
		s = s[:start] + evalExpr(expr, inputs, repo) + s[start+end+2:]
	}
}

func interpolateMap(m, inputs map[string]string, repo Repo) map[string]string {
	if m == nil {
		return nil
	}
	out := map[string]string{}
	for k, v := range m {
		out[k] = Interpolate(v, inputs, repo)
	}
	return out
}

func evalExpr(expr string, inputs map[string]string, repo Repo) string {
	switch {
	case strings.HasPrefix(expr, "inputs."):
		return inputs[strings.TrimPrefix(expr, "inputs.")]
	case strings.HasPrefix(expr, "env."):
		return "${" + strings.TrimPrefix(expr, "env.") + "}"
	case strings.HasPrefix(expr, "steps.") && strings.Contains(expr, ".outputs."):
		rest := strings.TrimPrefix(expr, "steps.")
		id, out, _ := strings.Cut(rest, ".outputs.")
		return fmt.Sprintf(`$(cat /tangled/.gha/outputs/%s/%s 2>/dev/null)`, sanitizeRef(id), sanitizeRef(out))
	case expr == "github.workspace":
		return repo.Workspace
	case expr == "github.sha":
		return repo.Sha
	case expr == "github.ref_name":
		return repo.Branch
	case expr == "github.ref":
		return "refs/heads/" + repo.Branch
	case expr == "github.repository":
		return repo.Name
	case expr == "github.token", expr == "secrets.GITHUB_TOKEN":
		return "${GITHUB_TOKEN}"
	case expr == "runner.os":
		return "Linux"
	case expr == "runner.temp":
		return "/tmp"
	case expr == "runner.tool_cache":
		return "/tangled/.gha/toolcache"
	case strings.HasPrefix(expr, "github.action_path"):
		return "${GITHUB_ACTION_PATH}"
	}
	// Anything else (functions, operators) is beyond the subset; keep it
	// visible in the command rather than silently emptying it.
	return "${{ " + expr + " }}"
}

func shellQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'"
}

// SortedKeys is a test helper for deterministic assertions.
func SortedKeys(m map[string]string) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func parseMetadata(data []byte) (*Metadata, error) {
	var m Metadata
	if err := yaml.Unmarshal(data, &m); err != nil {
		return nil, err
	}
	if m.Runs.Using == "" {
		return nil, fmt.Errorf("action.yml has no runs.using")
	}
	return &m, nil
}
