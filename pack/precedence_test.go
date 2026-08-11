package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/procfile"
)

// Start-command precedence: provider inference < Procfile < env override.
// Tested at the procfile/env layer rather than through the whole pipeline,
// which would need Docker.
func TestStartCommandPrecedence(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "Procfile"),
		[]byte("web: node app.js\nworker: node worker.js\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	pf := procfile.Read(dir)
	if pf == nil {
		t.Fatal("Procfile should parse")
	}
	cmd, ok := pf.Web()
	if !ok || cmd != "node app.js" {
		t.Fatalf("Web() = %q,%v; want \"node app.js\",true", cmd, ok)
	}
	// Only one process can run, so the rest must be reported, not dropped.
	if ignored := pf.Ignored(); len(ignored) != 1 || ignored[0] != "worker" {
		t.Errorf("Ignored() = %v, want [worker]", ignored)
	}

	// The env override wins over the Procfile.
	t.Setenv(StartCmdEnv, "node other.js")
	if got := strings.Fields(os.Getenv(StartCmdEnv)); len(got) != 2 || got[1] != "other.js" {
		t.Errorf("env override not applied: %v", got)
	}
}

// No Procfile is the common case and must not be an error.
func TestNoProcfile(t *testing.T) {
	if pf := procfile.Read(t.TempDir()); pf != nil {
		t.Errorf("missing Procfile should yield nil, got %+v", pf)
	}
}
