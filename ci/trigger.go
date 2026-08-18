package main

// Trigger synthesis: a spindle receives its trigger metadata from a knot
// event; a local run has to reconstruct the same shape from the git checkout
// it is standing in. Downstream nothing knows the difference — the matching
// (`when:`), the env vars and the clone step all consume the lexicon structs,
// which is the compatibility this tool exists to keep.

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"tangled.org/core/api/tangled"
	"tangled.org/core/workflow"
)

// git runs one git command in dir and returns its trimmed stdout.
func git(dir string, args ...string) (string, error) {
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("git %s: %w", strings.Join(args, " "), err)
	}
	return strings.TrimSpace(string(out)), nil
}

// repoInfo is what a local checkout can tell us about itself.
type repoInfo struct {
	Root          string
	Name          string
	Sha           string
	Branch        string // empty on a detached HEAD
	DefaultBranch string
}

func inspectRepo(dir string) (*repoInfo, error) {
	root, err := git(dir, "rev-parse", "--show-toplevel")
	if err != nil {
		return nil, fmt.Errorf("%s is not inside a git repository", dir)
	}
	sha, err := git(root, "rev-parse", "HEAD")
	if err != nil {
		return nil, fmt.Errorf("the repository has no commits — CI runs a commit, not a working tree")
	}
	branch, _ := git(root, "rev-parse", "--abbrev-ref", "HEAD")
	if branch == "HEAD" {
		branch = "" // detached
	}
	// origin/HEAD when it is set; otherwise the current branch is the best
	// guess a purely local repository can offer.
	def := ""
	if ref, err := git(root, "symbolic-ref", "refs/remotes/origin/HEAD"); err == nil {
		def = strings.TrimPrefix(ref, "refs/remotes/origin/")
	}
	if def == "" {
		def = branch
	}
	if def == "" {
		def = "main"
	}
	return &repoInfo{
		Root:          root,
		Name:          filepath.Base(root),
		Sha:           sha,
		Branch:        branch,
		DefaultBranch: def,
	}, nil
}

// localTrigger builds the trigger metadata for a local run.
//
// The identity fields (knot, DID) describe a repository's place in the
// tangled network, which a local checkout does not have — they are filled
// with recognizable placeholders rather than left empty, so a workflow that
// prints its TANGLED_* env shows *why* a value is what it is.
func localTrigger(kind string, repo *repoInfo, targetBranch string, inputs map[string]string) (*tangled.Pipeline_TriggerMetadata, error) {
	repoName := repo.Name
	repoDid := "did:local:" + repo.Name
	tr := &tangled.Pipeline_TriggerMetadata{
		Kind: kind,
		Repo: &tangled.Pipeline_TriggerRepo{
			Knot:          "localhost",
			Did:           "did:local:" + userName(),
			Repo:          &repoName,
			RepoDid:       &repoDid,
			DefaultBranch: repo.DefaultBranch,
		},
	}

	branch := repo.Branch
	if branch == "" {
		branch = repo.DefaultBranch
	}
	ref := "refs/heads/" + branch

	switch workflow.TriggerKind(kind) {
	case workflow.TriggerKindManual:
		var pairs []*tangled.Pipeline_Pair
		for k, v := range inputs {
			pairs = append(pairs, &tangled.Pipeline_Pair{Key: k, Value: v})
		}
		tr.Manual = &tangled.Pipeline_ManualTriggerData{
			Sha:    repo.Sha,
			Ref:    &ref,
			Inputs: pairs,
		}
	case workflow.TriggerKindPush:
		// The previous commit stands in for the pre-push state; a
		// single-commit repository pushes from the zero SHA, as a real
		// branch creation does.
		old, err := git(repo.Root, "rev-parse", "HEAD~1")
		if err != nil {
			old = strings.Repeat("0", 40)
		}
		tr.Push = &tangled.Pipeline_PushTriggerData{
			Ref:    ref,
			NewSha: repo.Sha,
			OldSha: old,
		}
	case workflow.TriggerKindPullRequest:
		if targetBranch == "" {
			targetBranch = repo.DefaultBranch
		}
		tr.PullRequest = &tangled.Pipeline_PullRequestTriggerData{
			SourceBranch: branch,
			TargetBranch: targetBranch,
			SourceSha:    repo.Sha,
		}
	default:
		return nil, fmt.Errorf("unknown event %q (push | pull_request | manual)", kind)
	}
	return tr, nil
}

// changedFiles feeds the `paths:` constraint. Local approximation of what a
// knot computes from the ref update: the files the triggering commits touch.
func changedFiles(repo *repoInfo, tr *tangled.Pipeline_TriggerMetadata) []string {
	var rangeSpec string
	switch {
	case tr.Push != nil && !strings.HasPrefix(tr.Push.OldSha, "0000"):
		rangeSpec = tr.Push.OldSha + ".." + tr.Push.NewSha
	case tr.PullRequest != nil:
		base, err := git(repo.Root, "merge-base", tr.PullRequest.TargetBranch, "HEAD")
		if err != nil {
			return nil
		}
		rangeSpec = base + "..HEAD"
	default:
		return nil
	}
	out, err := git(repo.Root, "diff", "--name-only", rangeSpec)
	if err != nil || out == "" {
		return nil
	}
	return strings.Split(out, "\n")
}

// pipelineEnv is spindle's PipelineEnvVars, ported rather than imported:
// the original lives in spindle/models, whose package also drags in the
// Docker client and the secrets store — a heavy price for a map of strings.
// The variables and their derivations match spindle commit-for-commit; a
// workflow that reads TANGLED_* under spindle reads the same values here.
func pipelineEnv(tr *tangled.Pipeline_TriggerMetadata, pipelineID string) map[string]string {
	env := map[string]string{
		"CI":                    "true",
		"TANGLED_PIPELINE_ID":   pipelineID,
		"TANGLED_PIPELINE_KIND": tr.Kind,
	}
	if tr.Repo != nil {
		env["TANGLED_REPO_KNOT"] = tr.Repo.Knot
		env["TANGLED_REPO_DID"] = tr.Repo.Did
		if tr.Repo.Repo != nil {
			env["TANGLED_REPO_NAME"] = *tr.Repo.Repo
		}
		if tr.Repo.RepoDid != nil {
			env["TANGLED_REPO_REPO_DID"] = *tr.Repo.RepoDid
			env["TANGLED_PIPELINE_SOURCE"] = *tr.Repo.RepoDid
		}
		env["TANGLED_REPO_DEFAULT_BRANCH"] = tr.Repo.DefaultBranch
	}

	setRef := func(ref string) {
		env["TANGLED_REF"] = ref
		name, kind := refNameType(ref)
		env["TANGLED_REF_NAME"] = name
		env["TANGLED_REF_TYPE"] = kind
	}

	switch workflow.TriggerKind(tr.Kind) {
	case workflow.TriggerKindPush:
		if tr.Push != nil {
			setRef(tr.Push.Ref)
			env["TANGLED_SHA"] = tr.Push.NewSha
			env["TANGLED_COMMIT_SHA"] = tr.Push.NewSha
		}
	case workflow.TriggerKindPullRequest:
		if pr := tr.PullRequest; pr != nil {
			setRef("refs/heads/" + pr.SourceBranch)
			env["TANGLED_SHA"] = pr.SourceSha
			env["TANGLED_COMMIT_SHA"] = pr.SourceSha
			env["TANGLED_PIPELINE_SOURCE_BRANCH"] = pr.SourceBranch
			env["TANGLED_PIPELINE_TARGET_BRANCH"] = pr.TargetBranch
			env["TANGLED_PR_SOURCE_BRANCH"] = pr.SourceBranch
			env["TANGLED_PR_TARGET_BRANCH"] = pr.TargetBranch
			env["TANGLED_PR_SOURCE_SHA"] = pr.SourceSha
		}
	case workflow.TriggerKindManual:
		if m := tr.Manual; m != nil {
			env["TANGLED_SHA"] = m.Sha
			env["TANGLED_COMMIT_SHA"] = m.Sha
			if m.Ref != nil && *m.Ref != "" {
				setRef(*m.Ref)
			}
			for _, pair := range m.Inputs {
				env["TANGLED_INPUT_"+strings.ToUpper(pair.Key)] = pair.Value
			}
		}
	}
	return env
}

// refNameType splits a full ref into its short name and "branch" or "tag" —
// the two cases spindle distinguishes (via go-git's plumbing, which this
// matches for those two).
func refNameType(ref string) (string, string) {
	if name, ok := strings.CutPrefix(ref, "refs/tags/"); ok {
		return name, "tag"
	}
	if name, ok := strings.CutPrefix(ref, "refs/heads/"); ok {
		return name, "branch"
	}
	return ref, "branch"
}

func userName() string {
	if u := os.Getenv("USER"); u != "" {
		return u
	}
	return "local"
}
