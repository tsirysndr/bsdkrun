package clean

import (
	"os"
	"path/filepath"
	"testing"

	"github.com/tsirysndr/bsdkrun/pack/internal/kraftfile"
)

func touch(t *testing.T, path string, body string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestProjectRemovesGeneratedArtifacts(t *testing.T) {
	dir := t.TempDir()
	touch(t, filepath.Join(dir, ".unikraft/unikraft/Makefile.uk"), "")
	touch(t, filepath.Join(dir, ".rootfs-arm64/hello"), "")
	touch(t, filepath.Join(dir, ".rust-cache/target/x"), "")
	touch(t, filepath.Join(dir, ".config.hello_fc-arm64"), "")
	touch(t, filepath.Join(dir, ".Kraftfile.arm64"), "")
	touch(t, filepath.Join(dir, "Kraftfile"), kraftfile.GeneratedMarker+"\nspec: v0.6\n")
	// Source must survive.
	touch(t, filepath.Join(dir, "go.mod"), "module x/app\n")

	if _, err := Project(dir); err != nil {
		t.Fatal(err)
	}
	for _, gone := range []string{".unikraft", ".rootfs-arm64", ".rust-cache",
		".config.hello_fc-arm64", ".Kraftfile.arm64", "Kraftfile"} {
		if _, err := os.Stat(filepath.Join(dir, gone)); !os.IsNotExist(err) {
			t.Errorf("%s should have been removed", gone)
		}
	}
	if _, err := os.Stat(filepath.Join(dir, "go.mod")); err != nil {
		t.Error("go.mod is source, it must survive a clean")
	}
}

// The one that matters: every unported examples/unikraft-* has a
// hand-written Kraftfile. A cache clean must never delete a file someone
// wrote by hand.
func TestProjectKeepsHandWrittenKraftfile(t *testing.T) {
	dir := t.TempDir()
	touch(t, filepath.Join(dir, "Kraftfile"), "spec: v0.6\nname: mine\n")

	removed, err := Project(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "Kraftfile")); err != nil {
		t.Fatal("a hand-written Kraftfile must survive --clean")
	}
	if len(removed.Notes) == 0 {
		t.Error("keeping it should be reported, not silent")
	}
}

func TestProjectOnEmptyDir(t *testing.T) {
	r, err := Project(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if len(r.Paths) != 0 {
		t.Errorf("nothing to remove, got %v", r.Paths)
	}
}
