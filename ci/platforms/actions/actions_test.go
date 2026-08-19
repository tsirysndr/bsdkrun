package actions

import (
	"fmt"
	"strings"
	"testing"
)

var testRepo = Repo{Sha: "abc123", Branch: "main", Name: "proj", Workspace: "/tangled/workspace"}

// fake wires a metadata fixture per slug into the fetcher.
func fake(t *testing.T, fixtures map[string]string) {
	t.Helper()
	old := FetchFunc
	FetchFunc = func(ref Ref) ([]byte, error) {
		if y, ok := fixtures[ref.Slug()]; ok {
			return []byte(y), nil
		}
		return nil, fmt.Errorf("no fixture for %s", ref.Slug())
	}
	t.Cleanup(func() { FetchFunc = old })
}

func TestParseRef(t *testing.T) {
	r, ok := ParseRef("oven-sh/setup-bun@v2")
	if !ok || r.Owner != "oven-sh" || r.Repo != "setup-bun" || r.Ref != "v2" || r.Path != "" {
		t.Fatalf("ref: %+v", r)
	}
	r, ok = ParseRef("github/codeql-action/init@v3")
	if !ok || r.Path != "init" {
		t.Fatalf("subdir ref: %+v", r)
	}
	if _, ok := ParseRef("./local-action"); ok {
		t.Fatal("local actions are not fetchable")
	}
	if _, ok := ParseRef("no-ref/anywhere"); ok {
		t.Fatal("a ref requires @")
	}
}

func TestJavaScriptAction(t *testing.T) {
	fake(t, map[string]string{
		"oven-sh/setup-bun": `
name: Setup Bun
inputs:
  bun-version:
    default: latest
runs:
  using: node20
  main: dist/setup/index.js
`,
	})
	steps, needsNode, err := Translate("oven-sh/setup-bun@v2", "s1",
		map[string]string{"bun-version": "1.2.0"}, nil, testRepo)
	if err != nil || len(steps) != 1 || !needsNode {
		t.Fatalf("steps=%+v needsNode=%v err=%v", steps, needsNode, err)
	}
	s := steps[0]
	if s.Name != "Setup Bun" {
		t.Fatalf("name: %q", s.Name)
	}
	if s.Env["INPUT_BUN-VERSION"] != "1.2.0" {
		t.Fatalf("with must override the default, dashes preserved: %v", s.Env)
	}
	if !strings.Contains(s.Command, "git clone") ||
		!strings.Contains(s.Command, "dist/setup/index.js") ||
		!strings.Contains(s.Command, "node ") {
		t.Fatalf("command: %q", s.Command)
	}
}

func TestInputDefaultsApply(t *testing.T) {
	fake(t, map[string]string{
		"a/b": `
runs: {using: node20, main: index.js}
inputs:
  version: {default: stable}
`,
	})
	steps, _, err := Translate("a/b@v1", "s1", nil, nil, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if steps[0].Env["INPUT_VERSION"] != "stable" {
		t.Fatalf("default input lost: %v", steps[0].Env)
	}
}

func TestCompositeAction(t *testing.T) {
	fake(t, map[string]string{
		"me/composite": `
name: Compo
inputs:
  greeting: {default: hi}
runs:
  using: composite
  steps:
    - name: greet
      run: echo "${{ inputs.greeting }} from ${{ github.repository }}"
      shell: bash
    - uses: nested/js@v1
`,
		"nested/js": `
runs: {using: node16, main: main.js}
`,
	})
	steps, needsNode, err := Translate("me/composite@v1", "s2",
		map[string]string{"greeting": "hello"}, nil, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if len(steps) != 2 {
		t.Fatalf("steps: %+v", steps)
	}
	if !strings.Contains(steps[0].Command, `echo "hello from proj"`) {
		t.Fatalf("input/context interpolation: %q", steps[0].Command)
	}
	if !needsNode {
		t.Fatal("the nested js action needs node")
	}
	if !strings.Contains(steps[1].Command, "main.js") {
		t.Fatalf("nested action lost: %q", steps[1].Command)
	}
}

func TestRefusalsNameTheirReasons(t *testing.T) {
	fake(t, map[string]string{
		"con/tainer": "runs: {using: docker, image: Dockerfile}\n",
	})
	if _, _, err := Translate("con/tainer@v1", "s", nil, nil, testRepo); err == nil ||
		!strings.Contains(err.Error(), "Docker daemon") {
		t.Fatalf("container refusal: %v", err)
	}
	if _, _, err := Translate("docker://alpine:3.20", "s", nil, nil, testRepo); err == nil {
		t.Fatal("docker:// must be refused")
	}
	if _, _, err := Translate("missing/action@v1", "s", nil, nil, testRepo); err == nil ||
		!strings.Contains(err.Error(), "action.yml") {
		t.Fatalf("fetch failure must say so: %v", err)
	}
}

func TestCheckoutIsCovered(t *testing.T) {
	steps, needsNode, err := Translate("actions/checkout@v4", "s", nil, nil, testRepo)
	if err != nil || needsNode || len(steps) != 1 || steps[0].Command != "true" {
		t.Fatalf("checkout: %+v, %v", steps, err)
	}
}

func TestInterpolate(t *testing.T) {
	got := Interpolate("sha=${{ github.sha }} out=${{ steps.build.outputs.digest }} env=${{ env.HOME }}",
		nil, testRepo)
	want := `sha=abc123 out=$(cat /tangled/.gha/outputs/build/digest 2>/dev/null) env=${HOME}`
	if got != want {
		t.Fatalf("interpolate:\n got %q\nwant %q", got, want)
	}
	// The unknown stays visible, never silently emptied.
	if got := Interpolate("${{ hashFiles('**/lockfile') }}", nil, testRepo); !strings.Contains(got, "hashFiles") {
		t.Fatalf("unknown expression must stay visible: %q", got)
	}
}

func TestWrapStepCarriesTheProtocol(t *testing.T) {
	w := WrapStep("step-1", "echo hi")
	for _, needle := range []string{
		"GITHUB_ENV=", "GITHUB_PATH=", "GITHUB_OUTPUT=",
		"RUNNER_OS=Linux", "/tangled/.gha/env.sh", "echo hi", "exit $__bsdkrun_rc",
	} {
		if !strings.Contains(w, needle) {
			t.Fatalf("wrapped step lacks %q", needle)
		}
	}
}

func TestExpressionDefaultsResolveEmptyInValues(t *testing.T) {
	// setup-bun's real token default: an operator expression the subset
	// cannot evaluate. In value position it must vanish — the visible
	// marker became a bearer token and a 401 once.
	fake(t, map[string]string{
		"a/tok": `
runs: {using: node20, main: index.js}
inputs:
  token:
    default: ${{ github.server_url == 'https://github.com' && github.token || '' }}
  version: {default: latest}
`,
	})
	steps, _, err := Translate("a/tok@v1", "s", nil, nil, testRepo)
	if err != nil {
		t.Fatal(err)
	}
	if _, present := steps[0].Env["INPUT_TOKEN"]; present {
		t.Fatalf("unevaluable default must be unset, got %v", steps[0].Env)
	}
	if steps[0].Env["INPUT_VERSION"] != "latest" {
		t.Fatalf("plain defaults must survive: %v", steps[0].Env)
	}
}
