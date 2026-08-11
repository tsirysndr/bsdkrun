package versions

import (
	"os"
	"path/filepath"
	"testing"
)

// railpack.json is the more deliberate statement of the two, so it wins
// over mise.
func TestConfigBeatsMise(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, ".tool-versions"), []byte("node 20.1.0\n"), 0o644)

	if v, _ := Read(dir).Major("node"); v != "20" {
		t.Errorf("mise alone should give 20, got %q", v)
	}

	os.WriteFile(filepath.Join(dir, "railpack.json"),
		[]byte(`{"packages":{"node":"22"}}`), 0o644)
	if v, _ := Read(dir).Major("node"); v != "22" {
		t.Errorf("railpack.json should win, got %q", v)
	}
}

func TestUnpinnedIsAbsent(t *testing.T) {
	if _, ok := Read(t.TempDir()).Version("node"); ok {
		t.Error("nothing pinned should report absent")
	}
}
