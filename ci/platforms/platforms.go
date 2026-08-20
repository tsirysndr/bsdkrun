// Package platforms translates foreign CI configurations — GitHub Actions,
// GitLab CI, Woodpecker, Drone, CircleCI, Buildkite, Semaphore, Jenkins
// (declarative), Azure Pipelines, AWS CodeBuild, Tekton, Travis —
// into a
// platform-neutral job list the runner turns into microVM plans.
//
// A platform's *jobs* are what mean something on a laptop: their image,
// their environment, their script steps, in execution order. Triggers,
// caches, artifact stores and the plugin/action ecosystems belong to the
// platforms themselves; a step that cannot be translated into a shell
// command becomes a visible skip in the timeline rather than a silent
// omission.
//
// Linux only, deliberately. A job that asks for windows or macos is dropped
// with its reason recorded — a Linux microVM cannot become another OS, and
// "passing" a macos job on Linux would be a lie with a green checkmark.
//
// The package is self-contained on purpose: it knows nothing about VMs or
// the runner. In: a directory and what git says about it. Out: jobs.
package platforms

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// DefaultImage boots when a job names no image. ubuntu:24.04 is what the
// hosted runners of most of these platforms mean by "linux" anyway.
const DefaultImage = "ubuntu:24.04"

// Step is one translated command.
type Step struct {
	Name    string
	Command string
	Env     map[string]string
}

// Job is one platform job; the runner gives each its own VM.
type Job struct {
	// Name selects the job on the command line and titles the run.
	Name string
	// Image is an OCI reference; empty means the runner's default.
	Image string
	Env   map[string]string
	Steps []Step
	// SkipReason, when set, excludes the job and says why.
	SkipReason string
	// MinMemMiB raises the VM's memory when the default would starve the
	// job (a real Jenkins needs heap, not hope).
	MinMemMiB int
	// ExtraMounts are host:guest:ro mounts the VM needs — plugin image
	// rootfs, pulled host-side at plan time.
	ExtraMounts []string
	// Disks are `hostImage:/guest/path` block devices. A job needs one when
	// a directory has to behave like a real filesystem rather than the
	// virtio-fs rootfs — overlayfs, for one, will not stack on virtio-fs.
	Disks []string
}

// Repo is what the translators may know about the checkout.
type Repo struct {
	Sha           string
	Branch        string
	DefaultBranch string
	Name          string
	// Workspace is the in-guest path the clone lands at; every platform's
	// workspace variable must point there or scripts cd into nothing.
	Workspace string
	// Token is the operator's GITHUB_TOKEN secret, when injected — what
	// `${{ github.token }}` resolves to for real actions.
	Token string
}

func (r Repo) branch() string {
	if r.Branch != "" {
		return r.Branch
	}
	return r.DefaultBranch
}

// Platform is one supported translator.
type Platform struct {
	Name string
	// Detect reports whether root carries this platform's config.
	Detect func(root string) bool
	// Load translates the config into jobs, in execution order.
	Load func(root string, repo Repo) ([]Job, error)
}

// Registry lists the platforms in detection priority order. The
// GitHub-compatible directories (forgejo, gitea) ride the github translator.
func Registry() []Platform {
	return []Platform{
		{Name: "github", Detect: detectGithub, Load: loadGithub},
		{Name: "gitlab", Detect: detectGitlab, Load: loadGitlab},
		{Name: "woodpecker", Detect: detectWoodpecker, Load: loadWoodpecker},
		{Name: "drone", Detect: detectDrone, Load: loadDrone},
		{Name: "circleci", Detect: detectCircleci, Load: loadCircleci},
		{Name: "buildkite", Detect: detectBuildkite, Load: loadBuildkite},
		{Name: "semaphore", Detect: detectSemaphore, Load: loadSemaphore},
		{Name: "jenkins", Detect: detectJenkins, Load: loadJenkins},
		{Name: "azure", Detect: detectAzure, Load: loadAzure},
		{Name: "codebuild", Detect: detectCodebuild, Load: loadCodebuild},
		{Name: "tekton", Detect: detectTekton, Load: loadTekton},
		{Name: "travis", Detect: detectTravis, Load: loadTravis},
		// Last on purpose: a repository with a dagger module usually also has
		// a CI config that *calls* dagger, and running that config reproduces
		// what its CI does. `--platform dagger` picks it deliberately.
		{Name: "dagger", Detect: detectDagger, Load: loadDagger},
	}
}

// Detect finds the platform for root: the forced name when given, else the
// first whose config exists. A nil, nil return means nothing matched.
func Detect(root, forced string) (*Platform, error) {
	defs := Registry()
	if forced != "" && forced != "auto" {
		for i := range defs {
			if defs[i].Name == forced {
				return &defs[i], nil
			}
		}
		names := make([]string, 0, len(defs))
		for _, d := range defs {
			names = append(names, d.Name)
		}
		sort.Strings(names)
		return nil, fmt.Errorf("unknown platform %q (known: tangled, %s)",
			forced, joinComma(names))
	}
	for i := range defs {
		if defs[i].Detect(root) {
			return &defs[i], nil
		}
	}
	return nil, nil
}

// Env is the identity a platform's scripts expect to find in their
// environment, workspace paths included.
func Env(platform string, repo Repo) map[string]string {
	switch platform {
	case "github":
		return map[string]string{
			"GITHUB_ACTIONS":    "true",
			"GITHUB_SHA":        repo.Sha,
			"GITHUB_REF":        "refs/heads/" + repo.branch(),
			"GITHUB_REF_NAME":   repo.branch(),
			"GITHUB_REPOSITORY": repo.Name,
			"GITHUB_WORKSPACE":  repo.Workspace,
		}
	case "gitlab":
		return map[string]string{
			"GITLAB_CI":          "true",
			"CI_COMMIT_SHA":      repo.Sha,
			"CI_COMMIT_REF_NAME": repo.branch(),
			"CI_PROJECT_NAME":    repo.Name,
			"CI_PROJECT_DIR":     repo.Workspace,
		}
	case "woodpecker":
		return map[string]string{
			"CI":               "woodpecker",
			"CI_COMMIT_SHA":    repo.Sha,
			"CI_COMMIT_BRANCH": repo.branch(),
			"CI_REPO_NAME":     repo.Name,
			"CI_WORKSPACE":     repo.Workspace,
		}
	case "drone":
		return map[string]string{
			"DRONE":            "true",
			"DRONE_COMMIT_SHA": repo.Sha,
			"DRONE_BRANCH":     repo.branch(),
			"DRONE_REPO_NAME":  repo.Name,
			"DRONE_WORKSPACE":  repo.Workspace,
		}
	case "circleci":
		return map[string]string{
			"CIRCLECI":                 "true",
			"CIRCLE_SHA1":              repo.Sha,
			"CIRCLE_BRANCH":            repo.branch(),
			"CIRCLE_PROJECT_REPONAME":  repo.Name,
			"CIRCLE_WORKING_DIRECTORY": repo.Workspace,
		}
	case "semaphore":
		return map[string]string{
			"SEMAPHORE":              "true",
			"SEMAPHORE_GIT_SHA":      repo.Sha,
			"SEMAPHORE_GIT_BRANCH":   repo.branch(),
			"SEMAPHORE_PROJECT_NAME": repo.Name,
			"SEMAPHORE_GIT_DIR":      repo.Workspace,
		}
	case "azure":
		return map[string]string{
			"TF_BUILD":               "True",
			"BUILD_SOURCEVERSION":    repo.Sha,
			"BUILD_SOURCEBRANCHNAME": repo.branch(),
			"BUILD_REPOSITORY_NAME":  repo.Name,
			"BUILD_SOURCESDIRECTORY": repo.Workspace,
		}
	case "codebuild":
		return map[string]string{
			"CODEBUILD_CI":                      "true",
			"CODEBUILD_BUILD_ID":                repo.Name + ":local",
			"CODEBUILD_RESOLVED_SOURCE_VERSION": repo.Sha,
			"CODEBUILD_SOURCE_VERSION":          repo.Sha,
			"CODEBUILD_SRC_DIR":                 repo.Workspace,
		}
	case "jenkins":
		return map[string]string{
			"JENKINS_URL":  "local",
			"BUILD_NUMBER": "1",
			"GIT_COMMIT":   repo.Sha,
			"BRANCH_NAME":  repo.branch(),
			"JOB_NAME":     repo.Name,
			"WORKSPACE":    repo.Workspace,
		}
	}
	return nil
}

func joinComma(names []string) string {
	out := ""
	for i, n := range names {
		if i > 0 {
			out += ", "
		}
		out += n
	}
	return out
}

func fileExists(path string) bool {
	st, err := os.Stat(path)
	return err == nil && !st.IsDir()
}

// yamlFiles lists a directory's YAML files, sorted for stable job order.
func yamlFiles(dir string) []string {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var out []string
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		if ext := filepath.Ext(e.Name()); ext == ".yml" || ext == ".yaml" {
			out = append(out, filepath.Join(dir, e.Name()))
		}
	}
	sort.Strings(out)
	return out
}

// linuxOnly returns the skip reason for an OS request, or "" when it is
// runnable here. Matching is permissive on purpose: "ubuntu-latest",
// "ubuntu-22.04", "linux", "self-hosted" all pass; anything naming windows
// or macos does not.
func linuxOnly(what string) string {
	l := strings.ToLower(what)
	switch {
	case strings.Contains(l, "windows"):
		return "windows job — a Linux microVM cannot run it"
	case strings.Contains(l, "macos"), strings.Contains(l, "osx"):
		return "macos job — a Linux microVM cannot run it"
	}
	return ""
}
