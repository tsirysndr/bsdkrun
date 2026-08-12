package bsdkrun

import (
	"os"
	"path/filepath"
	"testing"
)

func touchExecutable(t *testing.T, dir, name string) string {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.WriteFile(path, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestSetBinaryPathWins(t *testing.T) {
	t.Cleanup(ResetBinaryCache)
	override := touchExecutable(t, t.TempDir(), "bsdkrun-override")
	SetBinaryPath(override)
	got, err := ResolveBinary()
	if err != nil || got != override {
		t.Fatalf("got %q, %v", got, err)
	}
}

func TestEnvVarWinsOverPath(t *testing.T) {
	t.Cleanup(ResetBinaryCache)
	ResetBinaryCache()
	envBin := touchExecutable(t, t.TempDir(), "bsdkrun-env")
	t.Setenv("BSDKRUN_BIN", envBin)
	got, err := ResolveBinary()
	if err != nil || got != envBin {
		t.Fatalf("got %q, %v", got, err)
	}
}

func TestResolutionIsCached(t *testing.T) {
	t.Cleanup(ResetBinaryCache)
	first := touchExecutable(t, t.TempDir(), "bsdkrun-a")
	SetBinaryPath(first)
	if got, _ := ResolveBinary(); got != first {
		t.Fatalf("got %q", got)
	}
	// A later env change is invisible until the cache is reset.
	t.Setenv("BSDKRUN_BIN", touchExecutable(t, t.TempDir(), "bsdkrun-b"))
	if got, _ := ResolveBinary(); got != first {
		t.Fatalf("cache miss: got %q", got)
	}
}

func TestMissingOverrideFallsThrough(t *testing.T) {
	t.Cleanup(ResetBinaryCache)
	// A dangling override must not be returned; the env candidate that
	// exists wins instead.
	SetBinaryPath(filepath.Join(t.TempDir(), "does-not-exist"))
	envBin := touchExecutable(t, t.TempDir(), "bsdkrun-env")
	t.Setenv("BSDKRUN_BIN", envBin)
	got, err := ResolveBinary()
	if err != nil || got != envBin {
		t.Fatalf("got %q, %v", got, err)
	}
}
