package static

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

func write(t *testing.T, dir, name, body string) {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestDetect(t *testing.T) {
	cases := []struct {
		name  string
		files map[string]string
		dirs  []string
		want  bool
	}{
		{name: "index.html at root", files: map[string]string{"index.html": "<h1>"}, want: true},
		{name: "Staticfile", files: map[string]string{"Staticfile": "root: dist\n"}, want: true},
		{name: "public with files", files: map[string]string{"public/index.html": "<h1>"}, want: true},
		{name: "dist with files", files: map[string]string{"dist/app.js": "//"}, want: true},
		// An empty public/ is a leftover, not a site. Matching it would let
		// this provider shadow whatever the project actually is.
		{name: "empty public", dirs: []string{"public"}, want: false},
		{name: "nothing", want: false},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			dir := t.TempDir()
			for name, body := range tc.files {
				write(t, dir, name, body)
			}
			for _, d := range tc.dirs {
				if err := os.MkdirAll(filepath.Join(dir, d), 0o755); err != nil {
					t.Fatal(err)
				}
			}
			got, err := New().Detect(dir)
			if err != nil {
				t.Fatal(err)
			}
			if got != tc.want {
				t.Errorf("Detect() = %v, want %v", got, tc.want)
			}
		})
	}
}

func TestSiteRoot(t *testing.T) {
	t.Run("Staticfile beats convention", func(t *testing.T) {
		dir := t.TempDir()
		write(t, dir, "Staticfile", "root: ./out\n")
		write(t, dir, "public/index.html", "<h1>")
		if got := siteRoot(dir); got != "out" {
			t.Errorf("siteRoot() = %q, want %q", got, "out")
		}
	})

	t.Run("env beats Staticfile", func(t *testing.T) {
		dir := t.TempDir()
		write(t, dir, "Staticfile", "root: out\n")
		t.Setenv(RootEnv, "./www")
		if got := siteRoot(dir); got != "www" {
			t.Errorf("siteRoot() = %q, want %q", got, "www")
		}
	})

	t.Run("dist beats public", func(t *testing.T) {
		dir := t.TempDir()
		write(t, dir, "dist/app.js", "//")
		write(t, dir, "public/logo.svg", "<svg/>")
		if got := siteRoot(dir); got != "dist" {
			t.Errorf("siteRoot() = %q, want %q", got, "dist")
		}
	})

	t.Run("bare index.html serves the project root", func(t *testing.T) {
		dir := t.TempDir()
		write(t, dir, "index.html", "<h1>")
		if got := siteRoot(dir); got != "." {
			t.Errorf("siteRoot() = %q, want %q", got, ".")
		}
	})
}

// amd64 must relink the server away from the fc kernel's load address;
// arm64 must not, and shipping the flag there would be a silent change to a
// working target.
func TestAmd64Relinks(t *testing.T) {
	dir := t.TempDir()
	write(t, dir, "index.html", "<h1>")

	amd64, err := New().Plan(dir, plan.ArchAmd64)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(amd64.Script, amd64TextAddr) {
		t.Errorf("amd64 plan does not relink to %s", amd64TextAddr)
	}

	arm64, err := New().Plan(dir, plan.ArchArm64)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(arm64.Script, amd64TextAddr) {
		t.Errorf("arm64 plan should not carry -T")
	}
}
