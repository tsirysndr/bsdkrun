package main

import (
	"os"
	"path/filepath"
	"testing"
)

// chdir switches the test process's cwd to dir and restores the original on
// cleanup. displayPath reads os.Getwd() directly (it has to match the real
// `bsdkrun pack` invocation), so this is what makes the test deterministic
// rather than dependent on wherever `go test` happens to run from.
func chdir(t *testing.T, dir string) {
	t.Helper()
	orig, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Chdir(dir); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chdir(orig) })
}

func TestDisplayPath(t *testing.T) {
	chdir(t, t.TempDir())
	// Re-read via os.Getwd() rather than trusting t.TempDir()'s string
	// directly: on macOS /var is a symlink to /private/var, and Getwd
	// resolves it while t.TempDir()'s raw string doesn't — displayPath uses
	// Getwd internally, so the test has to build its expected paths from the
	// same resolved form or the two disagree on a symlink they'd never
	// actually disagree on in real usage (main.go's absPath comes from
	// filepath.Abs, which calls this same Getwd).
	cwd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}

	t.Run("same dir", func(t *testing.T) {
		if got := displayPath(cwd); got != "." {
			t.Errorf("displayPath(cwd) = %q, want \".\"", got)
		}
	})

	t.Run("child dir", func(t *testing.T) {
		p := filepath.Join(cwd, ".rootfs-arm64")
		if got := displayPath(p); got != ".rootfs-arm64" {
			t.Errorf("displayPath(%q) = %q, want \".rootfs-arm64\"", p, got)
		}
	})

	t.Run("nested child", func(t *testing.T) {
		p := filepath.Join(cwd, ".unikraft", "build", "hello_fc-arm64")
		want := filepath.Join(".unikraft", "build", "hello_fc-arm64")
		if got := displayPath(p); got != want {
			t.Errorf("displayPath(%q) = %q, want %q", p, got, want)
		}
	})

	t.Run("one level up, relative still wins", func(t *testing.T) {
		p := filepath.Join(filepath.Dir(cwd), "other-project")
		want := filepath.Join("..", "other-project")
		if got := displayPath(p); got != want {
			t.Errorf("displayPath(%q) = %q, want %q", p, got, want)
		}
	})

	t.Run("far away, absolute is shorter than the dotdot chain", func(t *testing.T) {
		// A deeply nested cwd (t.TempDir() gives one) and a target that
		// shares almost none of it: the relative form has to climb nearly
		// back to "/" and then back down, which is longer than just naming
		// the absolute path.
		p := filepath.Join(string(filepath.Separator), "x")
		if got := displayPath(p); got != p {
			t.Errorf("displayPath(%q) = %q, want the absolute path unchanged (%q)", p, got, p)
		}
	})
}
