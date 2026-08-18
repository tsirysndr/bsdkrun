package platforms

// Travis CI: .travis.yml. The build is phase-shaped rather than job-shaped —
// before_install, install, before_script, script, after_success — so the
// whole file becomes one job whose steps are the declared phases, in
// Travis's own phase order. `env: global:` variables apply (matrix env rows
// are not expanded, same policy as GitHub's matrix); an `os:` list that
// includes linux runs once for linux, one that does not skips the job.
// Language-specific toolchain bootstrap belongs to Travis images; here the
// default image runs exactly what the file says and nothing more.

import (
	"os"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

func detectTravis(root string) bool {
	return fileExists(filepath.Join(root, ".travis.yml"))
}

// travisPhases in execution order. after_failure/after_script are omitted:
// the runner stops at the first failing step, which is where a local
// iteration loop wants to stop.
var travisPhases = []string{
	"before_install", "install", "before_script", "script", "after_success",
}

func loadTravis(root string, repo Repo) ([]Job, error) {
	data, err := os.ReadFile(filepath.Join(root, ".travis.yml"))
	if err != nil {
		return nil, err
	}
	var doc map[string]yaml.Node
	if err := yaml.Unmarshal(data, &doc); err != nil {
		return nil, err
	}

	job := Job{Name: "travis"}

	if osNode, ok := doc["os"]; ok {
		linux := false
		var reasons []string
		for _, o := range yamlStrings(osNode) {
			if reason := linuxOnly(o); reason == "" && strings.Contains(strings.ToLower(o), "linux") {
				linux = true
			} else if reason != "" {
				reasons = append(reasons, reason)
			}
		}
		if !linux && len(reasons) > 0 {
			job.SkipReason = reasons[0]
		}
	}

	env := map[string]string{"TRAVIS": "true", "TRAVIS_COMMIT": repo.Sha, "TRAVIS_BRANCH": repo.branch()}
	if envNode, ok := doc["env"]; ok {
		// env can be a list (matrix rows — not expanded) or a map with
		// global/jobs keys; only global applies.
		var m struct {
			Global yaml.Node `yaml:"global"`
		}
		if err := envNode.Decode(&m); err == nil {
			for _, kv := range yamlStrings(m.Global) {
				if k, v, ok := strings.Cut(kv, "="); ok {
					env[k] = v
				}
			}
		}
	}
	job.Env = env

	for _, phase := range travisPhases {
		node, ok := doc[phase]
		if !ok {
			continue
		}
		if lines := scriptOf(node); lines != "" {
			job.Steps = append(job.Steps, Step{Name: phase, Command: lines})
		}
	}
	if len(job.Steps) == 0 {
		return nil, nil
	}
	return []Job{job}, nil
}
