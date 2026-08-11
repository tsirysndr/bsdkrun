package providers

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

func write(t *testing.T, dir, name, body string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

// Detection order is load-bearing: a Deno or Bun project may also carry a
// package.json, but a Node project never carries deno.json or bun.lockb —
// so the specific markers have to win.
func TestDetectionOrder(t *testing.T) {
	cases := []struct {
		name  string
		files map[string]string
		want  string
	}{
		{"go", map[string]string{"go.mod": "module x/widget\n"}, "go"},
		{"rust", map[string]string{"Cargo.toml": "[package]\nname = \"w\"\n"}, "rust"},
		{"node", map[string]string{"package.json": `{"main":"app.js"}`}, "node"},
		{"deno beats node", map[string]string{
			"deno.json": "{}", "package.json": "{}"}, "deno"},
		{"bun beats node", map[string]string{
			"bun.lockb": "", "package.json": "{}"}, "bun"},
		{"elixir", map[string]string{"mix.exs": `app: :myapp, version: "1.2.3"`}, "elixir"},
		{"gleam", map[string]string{"gleam.toml": `name = "wisp_demo"`}, "gleam"},
		{"php", map[string]string{"composer.json": "{}"}, "php"},
		{"ruby", map[string]string{"Gemfile": "source 'x'"}, "ruby"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			for n, b := range c.files {
				write(t, dir, n, b)
			}
			p, err := Find(dir)
			if err != nil {
				t.Fatal(err)
			}
			if p.Name() != c.want {
				t.Errorf("detected %q, want %q", p.Name(), c.want)
			}
		})
	}
}

func TestFindNothing(t *testing.T) {
	if _, err := Find(t.TempDir()); err == nil {
		t.Fatal("empty dir should not match a provider")
	}
}

// Bun's example only boots because of these two: JSC's concurrent GC is
// disabled, and the loader stack is 16x the default. Losing either is a
// silent hang, so pin them.
func TestBunCarriesItsQuirks(t *testing.T) {
	dir := t.TempDir()
	write(t, dir, "bun.lockb", "")
	write(t, dir, "server.js", "")

	p, err := Get("bun").Plan(dir, plan.ArchArm64)
	if err != nil {
		t.Fatal(err)
	}
	if p.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP4"] != `"BUN_JSC_useConcurrentGC=0"` {
		t.Errorf("missing the JSC concurrent-GC opt-out: %v", p.Kconfig)
	}
	if p.ElfloaderKconfig["CONFIG_APPELFLOADER_STACK_NBPAGES"] != "2048" {
		t.Errorf("missing the bigger loader stack: %v", p.ElfloaderKconfig)
	}
	// Without /proc/self/exe Bun panics before running any JavaScript.
	if !strings.Contains(p.Script, "/out/rootfs/proc/self/exe") {
		t.Errorf("missing the /proc/self/exe symlink:\n%s", p.Script)
	}
}

// mise pins the runtime version the project asks for rather than the
// provider's default.
func TestMisePinsVersion(t *testing.T) {
	dir := t.TempDir()
	write(t, dir, "package.json", "{}")
	write(t, dir, ".tool-versions", "node 22.1.0\n")

	p, err := Get("node").Plan(dir, plan.ArchArm64)
	if err != nil {
		t.Fatal(err)
	}
	if p.BuildImage != "node:22-alpine" {
		t.Errorf("BuildImage = %q, want node:22-alpine", p.BuildImage)
	}
}

func TestGoRelocatesAmd64Only(t *testing.T) {
	dir := t.TempDir()
	write(t, dir, "go.mod", "module example.com/acme/widget\n")

	amd64, err := Get("go").Plan(dir, plan.ArchAmd64)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(amd64.Script, "-T 0x40000000") {
		t.Errorf("amd64 must be linked above the kernel:\n%s", amd64.Script)
	}
	arm64, err := Get("go").Plan(dir, plan.ArchArm64)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(arm64.Script, "-T") {
		t.Errorf("arm64 must keep its proven layout:\n%s", arm64.Script)
	}
	if amd64.Name != "widget" {
		t.Errorf("Name = %q, want widget", amd64.Name)
	}
}

// The BEAM providers boot beam.smp directly — there is no shell in the guest
// to run a release's start script — so erlexec's environment has to be baked
// into the kernel config, and the release version ends up inside the boot
// argv. Both are silent failures if wrong.
func TestBeamProviders(t *testing.T) {
	t.Run("elixir reads app and version from mix.exs", func(t *testing.T) {
		dir := t.TempDir()
		write(t, dir, "mix.exs", "def project do\n[app: :myapp, version: \"1.2.3\"]\nend")
		p, err := Get("elixir").Plan(dir, plan.ArchArm64)
		if err != nil {
			t.Fatal(err)
		}
		if p.Name != "myapp" {
			t.Errorf("Name = %q, want myapp", p.Name)
		}
		joined := strings.Join(p.Cmd, " ")
		if !strings.Contains(joined, "/srv/releases/1.2.3/start") {
			t.Errorf("boot argv should name the release version:\n%s", joined)
		}
		for _, k := range []string{"ROOTDIR", "BINDIR", "EMU", "PROGNAME"} {
			if !strings.Contains(fmt.Sprint(p.Kconfig), k) {
				t.Errorf("missing erlexec env %s: %v", k, p.Kconfig)
			}
		}
	})

	t.Run("gleam calls its own entrypoint module", func(t *testing.T) {
		dir := t.TempDir()
		write(t, dir, "gleam.toml", `name = "wisp_demo"`)
		p, err := Get("gleam").Plan(dir, plan.ArchArm64)
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(strings.Join(p.Cmd, " "), "wisp_demo@@main:run(wisp_demo)") {
			t.Errorf("wrong entrypoint: %v", p.Cmd)
		}
		if p.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP8"] != `"ERL_LIBS=/srv"` {
			t.Errorf("ERL_LIBS replaces gleam's 21-argument -pa expansion: %v", p.Kconfig)
		}
	})
}

// Ruby's stdlib ships native extensions, so resolving only the interpreter's
// own libraries leaves the first `require` of an extension failing in the
// guest.
func TestRubyWalksStdlibExtensions(t *testing.T) {
	dir := t.TempDir()
	write(t, dir, "Gemfile", "source 'https://rubygems.org'")
	p, err := Get("ruby").Plan(dir, plan.ArchArm64)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(p.Script, "find /usr/local/lib/ruby -name '*.so'") {
		t.Errorf("stdlib .so files must be ldd-walked too:\n%s", p.Script)
	}
}
