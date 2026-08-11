package config

import (
	"os"
	"path/filepath"
	"testing"
)

func writeCfg(t *testing.T, body string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, FileName), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

func TestReadAbsentIsNotAnError(t *testing.T) {
	c, err := Read(t.TempDir())
	if err != nil || c != nil {
		t.Fatalf("absent config should be (nil, nil), got (%v, %v)", c, err)
	}
}

// Silently ignoring a config someone wrote is worse than refusing to build.
func TestMalformedIsAnError(t *testing.T) {
	if _, err := Read(writeCfg(t, "{ not json")); err == nil {
		t.Fatal("malformed railpack.json should be an error")
	}
}

func TestReadHonouredFields(t *testing.T) {
	dir := writeCfg(t, `{
	  "provider": "node",
	  "packages": {"node": "22"},
	  "buildAptPackages": ["git"],
	  "deploy": {"startCommand": "node other.js"}
	}`)
	c, err := Read(dir)
	if err != nil {
		t.Fatal(err)
	}
	if c.Provider == nil || *c.Provider != "node" {
		t.Errorf("provider = %v, want node", c.Provider)
	}
	if c.Packages["node"] != "22" {
		t.Errorf("packages = %v", c.Packages)
	}
	if len(c.BuildAptPackages) != 1 || c.BuildAptPackages[0] != "git" {
		t.Errorf("buildAptPackages = %v", c.BuildAptPackages)
	}
	if c.Deploy.StartCommand != "node other.js" {
		t.Errorf("startCommand = %q", c.Deploy.StartCommand)
	}
	if u := c.Unsupported(); len(u) != 0 {
		t.Errorf("nothing should be unsupported here, got %v", u)
	}
}

// pack builds one script per provider, so railpack's multi-step graph has
// nowhere to go — it must be reported, not dropped.
func TestUnsupportedFieldsAreNamed(t *testing.T) {
	dir := writeCfg(t, `{
	  "steps": {"build": {}},
	  "caches": {"npm": {}},
	  "secrets": ["TOKEN"],
	  "deploy": {"aptPackages": ["curl"]}
	}`)
	c, err := Read(dir)
	if err != nil {
		t.Fatal(err)
	}
	got := map[string]bool{}
	for _, u := range c.Unsupported() {
		got[u] = true
	}
	for _, want := range []string{"steps", "caches", "secrets", "deploy.aptPackages"} {
		if !got[want] {
			t.Errorf("Unsupported() should name %q, got %v", want, c.Unsupported())
		}
	}
}
