package platforms

// Drone and Woodpecker share a shape — Woodpecker began as a Drone fork —
// so one translator core serves both: named steps with an image, commands
// and environment. Differences that matter here: the file locations
// (.drone.yml vs .woodpecker/*.yml or .woodpecker.yml), Woodpecker's legacy
// `pipeline:` key for what is now `steps:`, and Drone's multi-document files
// (one pipeline per YAML document).
//
// The workspace model is the platforms' own: steps share one workspace but
// may name different images. One VM runs one image, so a pipeline whose
// steps disagree runs on the *first* step's image, and the timeline says so
// — shared state was judged more load-bearing than per-step userlands for a
// local run. Plugin steps (settings: and no commands) become visible skips.

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

func detectDrone(root string) bool {
	return fileExists(filepath.Join(root, ".drone.yml"))
}

func detectWoodpecker(root string) bool {
	if fileExists(filepath.Join(root, ".woodpecker.yml")) ||
		fileExists(filepath.Join(root, ".woodpecker.yaml")) {
		return true
	}
	return len(yamlFiles(filepath.Join(root, ".woodpecker"))) > 0
}

type dwStep struct {
	Name        string    `yaml:"name"`
	Image       string    `yaml:"image"`
	Commands    []string  `yaml:"commands"`
	Environment yaml.Node `yaml:"environment"`
	Settings    yaml.Node `yaml:"settings"`
	When        yaml.Node `yaml:"when"` // parsed only to be ignored
}

type dwPipeline struct {
	Kind     string `yaml:"kind"` // drone: "pipeline"
	Type     string `yaml:"type"`
	Name     string `yaml:"name"`
	Platform struct {
		OS string `yaml:"os"`
	} `yaml:"platform"`
	Steps []dwStep `yaml:"steps"`
	// Woodpecker's legacy spelling of steps.
	Pipeline yaml.Node `yaml:"pipeline"`
}

func loadDrone(root string, repo Repo) ([]Job, error) {
	return dwLoadFiles([]string{filepath.Join(root, ".drone.yml")}, "drone")
}

func loadWoodpecker(root string, repo Repo) ([]Job, error) {
	var files []string
	for _, f := range []string{".woodpecker.yml", ".woodpecker.yaml"} {
		if fileExists(filepath.Join(root, f)) {
			files = append(files, filepath.Join(root, f))
		}
	}
	files = append(files, yamlFiles(filepath.Join(root, ".woodpecker"))...)
	return dwLoadFiles(files, "woodpecker")
}

func dwLoadFiles(files []string, flavor string) ([]Job, error) {
	var out []Job
	for _, f := range files {
		data, err := os.ReadFile(f)
		if err != nil {
			return nil, err
		}
		// Drone files hold several pipelines as YAML documents.
		dec := yaml.NewDecoder(strings.NewReader(string(data)))
		docIdx := 0
		for {
			var p dwPipeline
			err := dec.Decode(&p)
			if err != nil {
				break // io.EOF or a trailing empty document
			}
			job, ok := dwPipelineJob(f, docIdx, p)
			if ok {
				out = append(out, job)
			}
			docIdx++
		}
	}
	return out, nil
}

func dwPipelineJob(file string, docIdx int, p dwPipeline) (Job, bool) {
	steps := p.Steps
	if len(steps) == 0 && !p.Pipeline.IsZero() {
		// Woodpecker's legacy `pipeline:` map keeps declaration order in the
		// node's content; a plain map decode would lose it.
		var m map[string]dwStep
		if err := p.Pipeline.Decode(&m); err == nil {
			for i := 0; i < len(p.Pipeline.Content)-1; i += 2 {
				name := p.Pipeline.Content[i].Value
				s := m[name]
				s.Name = name
				steps = append(steps, s)
			}
		}
	}
	if len(steps) == 0 {
		return Job{}, false
	}

	name := p.Name
	if name == "" {
		name = strings.TrimSuffix(strings.TrimSuffix(filepath.Base(file), ".yml"), ".yaml")
		if docIdx > 0 {
			name = fmt.Sprintf("%s-%d", name, docIdx+1)
		}
	}
	job := Job{Name: name}
	if p.Platform.OS != "" {
		if reason := linuxOnly(p.Platform.OS); reason != "" {
			job.SkipReason = reason
		}
	}

	// One VM, one image: the first step's. Divergence is announced, not
	// hidden.
	var divergent []string
	for _, s := range steps {
		if s.Image != "" && job.Image == "" {
			job.Image = s.Image
		} else if s.Image != "" && s.Image != job.Image {
			divergent = append(divergent, fmt.Sprintf("%s (%s)", s.Name, s.Image))
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

	for i, s := range steps {
		name := s.Name
		if name == "" {
			name = fmt.Sprintf("step %d", i+1)
		}
		if len(s.Commands) == 0 {
			if !s.Settings.IsZero() {
				job.Steps = append(job.Steps, Step{
					Name:    name + " (plugin, skipped)",
					Command: fmt.Sprintf(`echo "skipped plugin step %s — plugins are not supported locally"`, name),
				})
			}
			continue
		}
		job.Steps = append(job.Steps, Step{
			Name:    name,
			Command: strings.Join(s.Commands, "\n"),
			Env:     dwEnv(s.Environment),
		})
	}
	return job, true
}

// dwEnv accepts both spellings: the map form and Drone's `- K=V` list form.
func dwEnv(n yaml.Node) map[string]string {
	if n.IsZero() {
		return nil
	}
	m := map[string]string{}
	if err := n.Decode(&m); err == nil {
		return m
	}
	var list []string
	if err := n.Decode(&list); err == nil {
		for _, kv := range list {
			if k, v, ok := strings.Cut(kv, "="); ok {
				m[k] = v
			}
		}
		return m
	}
	return nil
}
