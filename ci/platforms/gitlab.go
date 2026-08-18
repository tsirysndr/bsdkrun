package platforms

// GitLab CI: one file, jobs keyed at the top level, ordered by stages.
//
// What translates: `stages:` order (GitLab's defaults when absent), each
// job's image (or the top-level default image, or the runner default),
// top-level and per-job `variables:`, `before_script` + `script` +
// `after_script` as steps. Hidden jobs (`.name`) and the non-job keys
// (stages, variables, default, workflow, include) are excluded; `include:`
// is not followed — a local run translates the file in front of it.

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"

	"gopkg.in/yaml.v3"
)

func detectGitlab(root string) bool {
	return fileExists(filepath.Join(root, ".gitlab-ci.yml"))
}

// glDefaultStages is GitLab's built-in stage order.
var glDefaultStages = []string{".pre", "build", "test", "deploy", ".post"}

type glJob struct {
	Stage        string            `yaml:"stage"`
	Image        yaml.Node         `yaml:"image"`
	Variables    map[string]string `yaml:"variables"`
	BeforeScript yaml.Node         `yaml:"before_script"`
	Script       yaml.Node         `yaml:"script"`
	AfterScript  yaml.Node         `yaml:"after_script"`
	Tags         []string          `yaml:"tags"`
}

func loadGitlab(root string, repo Repo) ([]Job, error) {
	data, err := os.ReadFile(filepath.Join(root, ".gitlab-ci.yml"))
	if err != nil {
		return nil, err
	}

	// Two passes over one document: the typed keys, then the job map.
	var meta struct {
		Stages    []string          `yaml:"stages"`
		Variables map[string]string `yaml:"variables"`
		Image     yaml.Node         `yaml:"image"`
		Default   struct {
			Image        yaml.Node `yaml:"image"`
			BeforeScript yaml.Node `yaml:"before_script"`
		} `yaml:"default"`
	}
	if err := yaml.Unmarshal(data, &meta); err != nil {
		return nil, fmt.Errorf(".gitlab-ci.yml: %w", err)
	}
	var raw map[string]yaml.Node
	if err := yaml.Unmarshal(data, &raw); err != nil {
		return nil, fmt.Errorf(".gitlab-ci.yml: %w", err)
	}

	stages := meta.Stages
	if len(stages) == 0 {
		stages = glDefaultStages
	}
	stageIdx := map[string]int{}
	for i, s := range stages {
		stageIdx[s] = i
	}

	reserved := map[string]bool{
		"stages": true, "variables": true, "default": true, "workflow": true,
		"include": true, "image": true, "services": true, "cache": true,
		"before_script": true, "after_script": true, "pages": false,
	}

	type namedJob struct {
		name string
		job  glJob
	}
	var named []namedJob
	for key, node := range raw {
		if reserved[key] || len(key) == 0 || key[0] == '.' {
			continue
		}
		var j glJob
		if err := node.Decode(&j); err != nil {
			continue // a non-job-shaped key (e.g. a scalar) is not a job
		}
		if yamlStrings(j.Script) == nil && j.Script.IsZero() {
			continue // jobs have scripts; anything else is config
		}
		named = append(named, namedJob{name: key, job: j})
	}
	// Stage order first, name order within a stage — deterministic where
	// GitLab would parallelize.
	sort.SliceStable(named, func(a, b int) bool {
		sa, sb := named[a].job.Stage, named[b].job.Stage
		if sa == "" {
			sa = "test"
		}
		if sb == "" {
			sb = "test"
		}
		ia, ib := stageIdx[sa], stageIdx[sb]
		if ia != ib {
			return ia < ib
		}
		return named[a].name < named[b].name
	})

	defaultImage := glImage(meta.Image)
	if defaultImage == "" {
		defaultImage = glImage(meta.Default.Image)
	}

	var out []Job
	for _, nj := range named {
		j := nj.job
		job := Job{Name: nj.name, Image: glImage(j.Image)}
		if job.Image == "" {
			job.Image = defaultImage
		}
		for _, t := range j.Tags {
			if reason := linuxOnly(t); reason != "" {
				job.SkipReason = reason
			}
		}
		env := map[string]string{}
		for k, v := range meta.Variables {
			env[k] = v
		}
		for k, v := range j.Variables {
			env[k] = v
		}
		job.Env = env

		before := j.BeforeScript
		if before.IsZero() {
			before = meta.Default.BeforeScript
		}
		if lines := scriptOf(before); lines != "" {
			job.Steps = append(job.Steps, Step{Name: "before_script", Command: lines})
		}
		if lines := scriptOf(j.Script); lines != "" {
			job.Steps = append(job.Steps, Step{Name: "script", Command: lines})
		}
		if lines := scriptOf(j.AfterScript); lines != "" {
			job.Steps = append(job.Steps, Step{Name: "after_script", Command: lines})
		}
		out = append(out, job)
	}
	return out, nil
}

// glImage handles `image: ref` and `image: {name: ref}`.
func glImage(n yaml.Node) string {
	if n.IsZero() {
		return ""
	}
	var s string
	if err := n.Decode(&s); err == nil {
		return s
	}
	var m struct {
		Name string `yaml:"name"`
	}
	if err := n.Decode(&m); err == nil {
		return m.Name
	}
	return ""
}

// scriptOf joins a scalar-or-list script into one command block.
func scriptOf(n yaml.Node) string {
	lines := yamlStrings(n)
	out := ""
	for i, l := range lines {
		if i > 0 {
			out += "\n"
		}
		out += l
	}
	return out
}
