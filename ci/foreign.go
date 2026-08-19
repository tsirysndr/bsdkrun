package main

// The bridge between the platforms package (which knows CI formats and
// nothing about VMs) and the runner (which knows VMs and nothing about CI
// formats): a foreign job becomes a Plan with the same system steps every
// custom-image workflow gets — prepare, ensure git, clone by SHA — and the
// platform identity env (GITHUB_SHA, CI_PROJECT_DIR, …) layered under the
// job's own environment.

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"tangled.org/core/workflow"
)

// platformRepo maps our repo info into what translators and Env need.
func platformRepo(repo *repoInfo) platforms.Repo {
	return platforms.Repo{
		Sha:           repo.Sha,
		Branch:        repo.Branch,
		DefaultBranch: repo.DefaultBranch,
		Name:          repo.Name,
		Workspace:     workspaceDir,
		Token:         ghToken,
	}
}

// ghToken is the operator's GITHUB_TOKEN secret, set by cmdRun before any
// plan is built — what real actions authenticate with.
var ghToken string

// foreignPlans loads the platform's jobs and turns the runnable ones into
// plans. Skipped jobs (non-Linux) and name filtering are announced on
// stderr, where they stay visible in every output mode.
func foreignPlans(p *platforms.Platform, repo *repoInfo, names []string) ([]*Plan, error) {
	jobs, err := p.Load(repo.WorkflowRoot(), platformRepo(repo))
	if err != nil {
		return nil, err
	}
	if len(jobs) == 0 {
		return nil, fmt.Errorf("the %s config in %s has no runnable jobs", p.Name, repo.WorkflowRoot())
	}

	var plans []*Plan
	for _, job := range jobs {
		if len(names) > 0 && !nameMatches(job.Name, names) {
			continue
		}
		if job.SkipReason != "" {
			fmt.Fprintf(os.Stderr, "skipping %s: %s\n", job.Name, job.SkipReason)
			continue
		}
		plans = append(plans, foreignPlan(p.Name, job, repo))
	}
	if len(plans) == 0 {
		return nil, fmt.Errorf("no runnable %s job matches", p.Name)
	}
	return plans, nil
}

func foreignPlan(platform string, job platforms.Job, repo *repoInfo) *Plan {
	env := map[string]string{"CI": "true"}
	for k, v := range platforms.Env(platform, platformRepo(repo)) {
		env[k] = v
	}
	for k, v := range job.Env {
		env[k] = v
	}
	env["HOME"] = homeDir

	image := job.Image
	if image == "" {
		image = platforms.DefaultImage
	}

	steps := []Step{prepareStep(), ensureGitStep(), localCloneStep(workflow.CloneOpts{}, repo.Sha)}
	for _, fs := range job.Steps {
		steps = append(steps, Step{Name: fs.Name, Command: fs.Command, Env: fs.Env})
	}

	workdir := ""
	if repo.Subdir != "" {
		workdir = filepath.Join(workspaceDir, repo.Subdir)
	}
	return &Plan{
		Name:     job.Name,
		Platform: platform,
		Image:    image,
		Env:      env,
		Steps:    steps,
		Workdir:  workdir,
	}
}
