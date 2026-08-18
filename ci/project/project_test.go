package project

import (
	"os"
	"path/filepath"
	"testing"
)

func write(t *testing.T, root, rel, content string) {
	t.Helper()
	path := filepath.Join(root, rel)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func stepNames(p *Project) []string {
	var out []string
	for _, s := range p.Jobs[0].Steps {
		out = append(out, s.Name)
	}
	return out
}

func TestGoTestsRunBeforeBuild(t *testing.T) {
	root := t.TempDir()
	write(t, root, "go.mod", "module x\n")
	write(t, root, "pkg/x_test.go", "package pkg\n")
	p := Detect(root)
	if p == nil || p.Language != "go" || p.Marker != "go.mod" {
		t.Fatalf("detect: %+v", p)
	}
	names := stepNames(p)
	if len(names) != 2 || names[0] != "go test" || names[1] != "go build" {
		t.Fatalf("tests must precede build: %v", names)
	}
	if p.Jobs[0].Env["CGO_ENABLED"] != "0" {
		t.Fatalf("CGO must be off on alpine: %v", p.Jobs[0].Env)
	}
}

func TestGoWithoutTestsSkipsTestStep(t *testing.T) {
	root := t.TempDir()
	write(t, root, "go.mod", "module x\n")
	p := Detect(root)
	names := stepNames(p)
	if len(names) != 1 || names[0] != "go build" {
		t.Fatalf("no vacuous test step: %v", names)
	}
}

func TestBunBeatsNode(t *testing.T) {
	root := t.TempDir()
	write(t, root, "package.json", `{"scripts":{"test":"bun test"}}`)
	write(t, root, "bun.lock", "")
	p := Detect(root)
	if p == nil || p.Language != "bun" {
		t.Fatalf("bun lockfile must beat package.json: %+v", p)
	}
}

func TestPhpBeatsNode(t *testing.T) {
	root := t.TempDir()
	write(t, root, "composer.json", `{"require":{}}`)
	write(t, root, "package.json", `{}`)
	p := Detect(root)
	if p == nil || p.Language != "php" {
		t.Fatalf("composer.json must beat package.json: %+v", p)
	}
}

func TestNodePackageManagersAndTestOrder(t *testing.T) {
	root := t.TempDir()
	write(t, root, "package.json", `{"scripts":{"test":"vitest","build":"tsc"}}`)
	write(t, root, "pnpm-lock.yaml", "")
	p := Detect(root)
	if p.Language != "nodejs" {
		t.Fatalf("detect: %+v", p)
	}
	names := stepNames(p)
	if len(names) != 3 || names[0] != "install dependencies" ||
		names[1] != "test" || names[2] != "build" {
		t.Fatalf("install, test, then build: %v", names)
	}
	if p.Jobs[0].Steps[0].Command != "corepack enable && pnpm install --frozen-lockfile" {
		t.Fatalf("pnpm lockfile not honored: %q", p.Jobs[0].Steps[0].Command)
	}
	if p.Jobs[0].Image != "node:24-alpine" {
		t.Fatalf("image: %q", p.Jobs[0].Image)
	}
}

func TestRustTestDetection(t *testing.T) {
	root := t.TempDir()
	write(t, root, "Cargo.toml", "[package]\nname='x'\n")
	write(t, root, "src/lib.rs", "#[cfg(test)]\nmod tests {}\n")
	p := Detect(root)
	names := stepNames(p)
	if names[0] != "cargo test" || names[1] != "cargo build" {
		t.Fatalf("tests before build: %v", names)
	}
}

func TestGleam(t *testing.T) {
	root := t.TempDir()
	write(t, root, "gleam.toml", `name = "x"`)
	write(t, root, "test/x_test.gleam", "")
	p := Detect(root)
	if p == nil || p.Language != "gleam" {
		t.Fatalf("detect: %+v", p)
	}
	names := stepNames(p)
	if len(names) != 3 || names[1] != "gleam test" || names[2] != "gleam build" {
		t.Fatalf("deps, test, build: %v", names)
	}
}

func TestZig(t *testing.T) {
	root := t.TempDir()
	write(t, root, "build.zig", `const test_step = b.step("test", "Run tests");`)
	p := Detect(root)
	if p == nil || p.Language != "zig" {
		t.Fatalf("detect: %+v", p)
	}
	names := stepNames(p)
	if len(names) != 3 || names[1] != "zig build test" || names[2] != "zig build" {
		t.Fatalf("install, test, build: %v", names)
	}
}

func TestPythonPytestOnlyWithTests(t *testing.T) {
	root := t.TempDir()
	write(t, root, "requirements.txt", "requests\n")
	write(t, root, "tests/test_x.py", "def test_x(): pass\n")
	p := Detect(root)
	names := stepNames(p)
	if len(names) != 2 || names[1] != "test" {
		t.Fatalf("pytest step expected: %v", names)
	}

	bare := t.TempDir()
	write(t, bare, "requirements.txt", "requests\n")
	p = Detect(bare)
	names = stepNames(p)
	if names[len(names)-1] != "compile check" {
		t.Fatalf("no pytest without tests: %v", names)
	}
}

func TestNothingDetected(t *testing.T) {
	if p := Detect(t.TempDir()); p != nil {
		t.Fatalf("empty dir must detect nothing: %+v", p)
	}
}

func TestClojure(t *testing.T) {
	root := t.TempDir()
	write(t, root, "deps.edn", `{:aliases {:test {:extra-paths ["test"]}}}`)
	p := Detect(root)
	if p == nil || p.Language != "clojure" {
		t.Fatalf("detect: %+v", p)
	}
	names := stepNames(p)
	if len(names) != 2 || names[1] != "clojure test" {
		t.Fatalf("steps: %v", names)
	}

	lein := t.TempDir()
	write(t, lein, "project.clj", `(defproject x "0.1.0")`)
	write(t, lein, "test/core_test.clj", "")
	p = Detect(lein)
	if p.Jobs[0].Image != "clojure:temurin-21-lein" {
		t.Fatalf("lein image: %q", p.Jobs[0].Image)
	}
	if names := stepNames(p); names[1] != "lein test" {
		t.Fatalf("lein steps: %v", names)
	}
}

func TestDotnet(t *testing.T) {
	root := t.TempDir()
	write(t, root, "src/App/App.csproj", "<Project/>")
	write(t, root, "src/App.Tests/App.Tests.csproj", "<Project/>")
	p := Detect(root)
	if p == nil || p.Language != "dotnet" {
		t.Fatalf("detect: %+v", p)
	}
	names := stepNames(p)
	if len(names) != 3 || names[1] != "dotnet test" || names[2] != "dotnet build" {
		t.Fatalf("restore, test, build: %v", names)
	}
}
