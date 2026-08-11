package ignore

import (
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// The point of the defaults: pack writes .unikraft/ (a full Unikraft source
// tree) and .rootfs-<arch>/ into the project, so a rebuild would upload its
// own previous output as build context unless these are excluded — and a
// project should not have to know pack's output layout to avoid that.
func TestDefaultsExcludePacksOwnOutput(t *testing.T) {
	got := Read(t.TempDir(), nil)
	for _, want := range []string{".unikraft", ".rootfs-*", ".rust-cache", ".config.*"} {
		if !slices.Contains(got, want) {
			t.Errorf("defaults must exclude %q, got %v", want, got)
		}
	}
}

func TestReadsDockerignoreAndConfig(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, FileName), []byte(
		"# a comment\n\n/build.sh\ntmp/\n!tmp/keep\n"), 0o644)

	got := Read(dir, []string{"extra/", " "})

	for _, want := range []string{"build.sh", "tmp/", "!tmp/keep", "extra/"} {
		if !slices.Contains(got, want) {
			t.Errorf("expected %q in %v", want, got)
		}
	}
	// Comments and blank lines are not patterns.
	for _, unwanted := range []string{"# a comment", "", " "} {
		if slices.Contains(got, unwanted) {
			t.Errorf("%q should not be a pattern: %v", unwanted, got)
		}
	}
}

// Docker treats a leading / as relative to the context root; fsutil's
// matcher wants it without, so it has to be trimmed or the pattern silently
// matches nothing.
func TestLeadingSlashTrimmed(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, FileName), []byte("/Dockerfile\n"), 0o644)
	if got := Read(dir, nil); !slices.Contains(got, "Dockerfile") {
		t.Errorf("leading slash should be trimmed, got %v", got)
	}
}
