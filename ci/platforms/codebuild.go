package platforms

// AWS CodePipeline — by way of CodeBuild, and that routing is the honest
// part. A CodePipeline definition (the pipeline.json behind the console,
// CloudFormation or CDK) is pure orchestration: its actions *reference*
// CodeBuild projects, Lambda functions and deploy providers, and contain no
// commands at all. The thing a laptop can truthfully run is the CodeBuild
// project's buildspec.yml — so that is what translates, one job whose steps
// are the buildspec phases in their fixed order: install, pre_build, build,
// post_build.
//
// `env.variables` apply; `parameter-store` and `secrets-manager` entries
// are dropped rather than faked (their values live in AWS — inject them
// with --secret / the secrets editor if a step needs them, and they will be
// masked like any other secret). `runtime-versions` and `artifacts` are
// CodeBuild-image machinery and are noted, not simulated.

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

func detectCodebuild(root string) bool {
	return fileExists(filepath.Join(root, "buildspec.yml")) ||
		fileExists(filepath.Join(root, "buildspec.yaml"))
}

type cbPhase struct {
	Commands        []string  `yaml:"commands"`
	RuntimeVersions yaml.Node `yaml:"runtime-versions"`
}

type cbSpec struct {
	Env struct {
		Variables      map[string]string `yaml:"variables"`
		ParameterStore map[string]string `yaml:"parameter-store"`
		SecretsManager map[string]string `yaml:"secrets-manager"`
	} `yaml:"env"`
	Phases map[string]cbPhase `yaml:"phases"`
}

// cbPhaseOrder is CodeBuild's own, fixed. finally-blocks and artifacts are
// not phases and do not appear.
var cbPhaseOrder = []string{"install", "pre_build", "build", "post_build"}

func loadCodebuild(root string, repo Repo) ([]Job, error) {
	path := filepath.Join(root, "buildspec.yml")
	if !fileExists(path) {
		path = filepath.Join(root, "buildspec.yaml")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var spec cbSpec
	if err := yaml.Unmarshal(data, &spec); err != nil {
		return nil, fmt.Errorf("buildspec.yml: %w", err)
	}

	job := Job{Name: "buildspec", Env: map[string]string{}}
	for k, v := range spec.Env.Variables {
		job.Env[k] = v
	}
	var dropped []string
	for k := range spec.Env.ParameterStore {
		dropped = append(dropped, k)
	}
	for k := range spec.Env.SecretsManager {
		dropped = append(dropped, k)
	}
	if len(dropped) > 0 {
		job.Steps = append(job.Steps, Step{
			Name: "aws-managed env (not resolved)",
			Command: fmt.Sprintf(
				`echo "parameter-store/secrets-manager values live in AWS and were not resolved: %s — inject them with --secret if needed"`,
				strings.Join(dropped, ", ")),
		})
	}

	for _, phase := range cbPhaseOrder {
		p, ok := spec.Phases[phase]
		if !ok || len(p.Commands) == 0 {
			continue
		}
		job.Steps = append(job.Steps, Step{
			Name:    phase,
			Command: strings.Join(p.Commands, "\n"),
		})
	}
	if len(job.Steps) == 0 {
		return nil, fmt.Errorf("buildspec.yml has no phase commands")
	}
	return []Job{job}, nil
}
