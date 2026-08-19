package main

import "testing"

// --sh must be usable as a bare flag and as one naming a job, and a named job
// must not quietly match the wrong one.
func TestShellTargetMatching(t *testing.T) {
	var bare shellTarget
	if err := bare.Set("true"); err != nil { // what Go's flag package passes for `--sh`
		t.Fatal(err)
	}
	if !bare.on || bare.job != "" {
		t.Fatalf("bare --sh: %+v", bare)
	}
	if !bare.matches("anything") {
		t.Fatal("bare --sh must take any job")
	}

	var named shellTarget
	if err := named.Set("build"); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"build", "Build", "build.yml", "docker-build"} {
		if !named.matches(name) {
			t.Fatalf("--sh=build should match %q", name)
		}
	}
	for _, name := range []string{"test", "lint", "deploy"} {
		if named.matches(name) {
			t.Fatalf("--sh=build must not match %q", name)
		}
	}

	var off shellTarget
	if off.matches("build") {
		t.Fatal("a run without --sh must never open a shell")
	}
}
