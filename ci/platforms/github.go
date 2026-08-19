package platforms

// GitHub Actions — and its compatible cousins: Forgejo and Gitea Actions
// read the same format from their own directories, so they ride this
// translator too.
//
// What translates: jobs in `needs:` order, each job's container image (or
// the default), workflow/job/step env, and every `run:` step. What does not:
// `uses:` actions. actions/checkout is genuinely covered by the runner's
// clone; every other action becomes a step that says it was skipped —
// visible in the timeline, never silent. Matrix jobs run once, without
// matrix context, and say so.

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"

	"github.com/tsirysndr/bsdkrun/ci/platforms/actions"
)

var githubDirs = []string{
	".github/workflows",
	".forgejo/workflows",
	".gitea/workflows",
}

func detectGithub(root string) bool {
	for _, d := range githubDirs {
		if len(yamlFiles(filepath.Join(root, d))) > 0 {
			return true
		}
	}
	return false
}

type ghWorkflow struct {
	Name string            `yaml:"name"`
	Env  map[string]string `yaml:"env"`
	Jobs map[string]ghJob  `yaml:"jobs"`
}

type ghJob struct {
	Name      string            `yaml:"name"`
	RunsOn    yaml.Node         `yaml:"runs-on"`
	Container yaml.Node         `yaml:"container"`
	Needs     yaml.Node         `yaml:"needs"`
	Env       map[string]string `yaml:"env"`
	Steps     []ghStep          `yaml:"steps"`
	Strategy  struct {
		Matrix yaml.Node `yaml:"matrix"`
	} `yaml:"strategy"`
}

type ghStep struct {
	ID    string            `yaml:"id"`
	Name  string            `yaml:"name"`
	Run   string            `yaml:"run"`
	Uses  string            `yaml:"uses"`
	Shell string            `yaml:"shell"`
	With  map[string]string `yaml:"with"`
	Env   map[string]string `yaml:"env"`
	Wdir  string            `yaml:"working-directory"`
}

func loadGithub(root string, repo Repo) ([]Job, error) {
	var files []string
	for _, d := range githubDirs {
		files = append(files, yamlFiles(filepath.Join(root, d))...)
	}
	var jobs []Job
	for _, f := range files {
		data, err := os.ReadFile(f)
		if err != nil {
			return nil, err
		}
		var wf ghWorkflow
		if err := yaml.Unmarshal(data, &wf); err != nil {
			return nil, fmt.Errorf("%s: %w", filepath.Base(f), err)
		}
		jobs = append(jobs, ghWorkflowJobs(filepath.Base(f), wf, repo)...)
	}
	return jobs, nil
}

func ghWorkflowJobs(file string, wf ghWorkflow, repo Repo) []Job {
	// needs-order: repeatedly take jobs whose needs are satisfied; ties in
	// declaration-independent map order break alphabetically for stability.
	ids := make([]string, 0, len(wf.Jobs))
	for id := range wf.Jobs {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	done := map[string]bool{}
	var order []string
	for len(order) < len(ids) {
		progressed := false
		for _, id := range ids {
			if done[id] {
				continue
			}
			ok := true
			for _, need := range yamlStrings(wf.Jobs[id].Needs) {
				if !done[need] {
					ok = false
					break
				}
			}
			if ok {
				order = append(order, id)
				done[id] = true
				progressed = true
			}
		}
		if !progressed { // a needs cycle; run the rest alphabetically
			for _, id := range ids {
				if !done[id] {
					order = append(order, id)
					done[id] = true
				}
			}
		}
	}

	var out []Job
	for _, id := range order {
		j := wf.Jobs[id]
		name := id
		if len(wf.Jobs) == 1 {
			// A one-job workflow reads better under the file's name.
			name = strings.TrimSuffix(strings.TrimSuffix(file, ".yml"), ".yaml")
		}
		job := Job{Name: name}

		for _, ro := range yamlStrings(j.RunsOn) {
			if reason := linuxOnly(ro); reason != "" {
				job.SkipReason = reason
			}
		}
		job.Image = ghContainerImage(j.Container)

		env := map[string]string{}
		for k, v := range wf.Env {
			env[k] = v
		}
		for k, v := range j.Env {
			env[k] = v
		}
		job.Env = env

		if !j.Strategy.Matrix.IsZero() {
			job.Steps = append(job.Steps, Step{
				Name:    "matrix (not expanded)",
				Command: `echo "matrix strategies are not expanded locally — running the job once, without matrix context"`,
			})
		}
		needsNode := false
		var translated []Step
		for i, s := range j.Steps {
			steps, nn := ghStepToSteps(i, s, repo)
			translated = append(translated, steps...)
			needsNode = needsNode || nn
		}
		// JavaScript actions need a node runtime the image may not carry;
		// provision it once, before anything runs.
		if needsNode {
			p := actions.NodeProvisionStep()
			job.Steps = append(job.Steps, Step{Name: p.Name, Command: p.Command})
		}
		// Every step runs under the Actions env protocol — a run: step must
		// see what a setup action exported, exactly as on the real runner.
		for i := range translated {
			translated[i].Command = actions.WrapStep(ghStepID(id, i), translated[i].Command)
		}
		job.Steps = append(job.Steps, translated...)
		out = append(out, job)
	}
	return out
}

func ghStepID(jobID string, i int) string {
	return fmt.Sprintf("%s-%d", jobID, i)
}

// ghStepToSteps translates one workflow step. A `uses:` goes through the
// actions runner: any JavaScript or composite action executes for real;
// what genuinely cannot run here (container actions, unreachable
// metadata) becomes a visible skip naming the reason.
func ghStepToSteps(i int, s ghStep, repo Repo) ([]Step, bool) {
	name := s.Name
	switch {
	case s.Uses != "":
		if name == "" {
			name = s.Uses
		}
		stepID := s.ID
		if stepID == "" {
			stepID = fmt.Sprintf("step-%d", i)
		}
		aRepo := actions.Repo{
			Sha: repo.Sha, Branch: repo.branch(), Name: repo.Name,
			Workspace: repo.Workspace, Token: repo.Token,
		}
		with := map[string]string{}
		for k, v := range s.With {
			with[k] = v
		}
		translated, needsNode, err := actions.Translate(s.Uses, stepID, with, s.Env, aRepo)
		if err != nil {
			return []Step{{
				Name:    name + " (skipped)",
				Command: fmt.Sprintf(`echo "skipped uses: %s — %s"`, s.Uses, strings.ReplaceAll(err.Error(), `"`, `'`)),
			}}, false
		}
		out := make([]Step, len(translated))
		for j, t := range translated {
			out[j] = Step{Name: t.Name, Command: t.Command, Env: t.Env}
		}
		return out, needsNode
	case s.Run != "":
		if name == "" {
			name = firstLineOf(s.Run)
		}
		cmd := s.Run
		if s.Wdir != "" {
			cmd = "cd " + s.Wdir + " && {\n" + cmd + "\n}"
		}
		return []Step{{Name: name, Command: cmd, Env: s.Env}}, false
	default:
		if name == "" {
			name = fmt.Sprintf("step %d", i+1)
		}
		return []Step{{Name: name + " (empty)", Command: "true"}}, false
	}
}

// ghContainerImage handles both `container: image-ref` and
// `container: {image: ref}`.
func ghContainerImage(n yaml.Node) string {
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

// yamlStrings reads a scalar-or-list node as strings.
func yamlStrings(n yaml.Node) []string {
	if n.IsZero() {
		return nil
	}
	var one string
	if err := n.Decode(&one); err == nil {
		return []string{one}
	}
	var many []string
	if err := n.Decode(&many); err == nil {
		return many
	}
	return nil
}

func firstLineOf(s string) string {
	s = strings.TrimSpace(s)
	if i := strings.IndexByte(s, '\n'); i >= 0 {
		s = s[:i]
	}
	if len(s) > 60 {
		s = s[:59] + "…"
	}
	return s
}
