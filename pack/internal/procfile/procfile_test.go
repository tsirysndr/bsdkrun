package procfile

import (
	"os"
	"path/filepath"
	"testing"
)

func write(t *testing.T, body string) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "Procfile"), []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir
}

// The order is railpack's: web, then worker, then whatever was declared
// first. The worker step matters for a Procfile with only background
// processes — "first declared" would pick `release: migrate`, a command
// that exits immediately and leaves the guest dead.
func TestWorkerBeatsFirstDeclared(t *testing.T) {
	p := Read(write(t, "release: migrate\nworker: consume\n"))
	cmd, ok := p.Web()
	if !ok || cmd != "consume" {
		t.Errorf("Web() = %q, %v; want the worker command", cmd, ok)
	}
	if ignored := p.Ignored(); len(ignored) != 1 || ignored[0] != "release" {
		t.Errorf("Ignored() = %v, want [release]", ignored)
	}
}

func TestWebWins(t *testing.T) {
	p := Read(write(t, "worker: consume\nweb: serve\n"))
	if cmd, _ := p.Web(); cmd != "serve" {
		t.Errorf("Web() = %q, want serve", cmd)
	}
	if ignored := p.Ignored(); len(ignored) != 1 || ignored[0] != "worker" {
		t.Errorf("Ignored() = %v, want [worker]", ignored)
	}
}

// Neither web nor worker: the first declared runs, and the rest are named
// rather than silently dropped.
func TestFallsBackToFirstDeclared(t *testing.T) {
	p := Read(write(t, "clock: tick\nrelease: migrate\n"))
	if cmd, _ := p.Web(); cmd != "tick" {
		t.Errorf("Web() = %q, want tick", cmd)
	}
	if ignored := p.Ignored(); len(ignored) != 1 || ignored[0] != "release" {
		t.Errorf("Ignored() = %v, want [release]", ignored)
	}
}

func TestCommentsAndBlanksIgnored(t *testing.T) {
	p := Read(write(t, "# a comment\n\nweb: serve --port 8080\n"))
	if cmd, _ := p.Web(); cmd != "serve --port 8080" {
		t.Errorf("Web() = %q", cmd)
	}
}

// No Procfile is the common case, not an error.
func TestMissingIsNil(t *testing.T) {
	if p := Read(t.TempDir()); p != nil {
		t.Errorf("Read() = %v, want nil", p)
	}
	var nilp *Procfile
	if _, ok := nilp.Web(); ok {
		t.Error("nil Procfile should have no command")
	}
	if ignored := nilp.Ignored(); ignored != nil {
		t.Errorf("nil Procfile Ignored() = %v", ignored)
	}
}
