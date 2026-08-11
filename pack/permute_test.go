package main

import (
	"flag"
	"testing"
)

// newTestFlags mirrors main's real flag set: one string flag (takes a
// value) and two booleans (do not).
func newTestFlags() (*flag.FlagSet, *string, *bool, *bool) {
	fs := flag.NewFlagSet("t", flag.ContinueOnError)
	target := fs.String("target", "", "")
	strace := fs.Bool("strace", false, "")
	loaderDebug := fs.Bool("loader-debug", false, "")
	return fs, target, strace, loaderDebug
}

func TestPermuteArgs(t *testing.T) {
	cases := []struct {
		name       string
		args       []string
		wantPath   string
		wantTarget string
		wantStrace bool
		wantLoader bool
	}{
		{
			// The regression this exists for: flags after the path used to
			// be swallowed as positionals and silently ignored.
			name: "flags after path", args: []string{".", "--plain", "--strace", "--loader-debug"},
			wantPath: ".", wantStrace: true, wantLoader: true,
		},
		{
			name: "flags before path", args: []string{"--strace", "."},
			wantPath: ".", wantStrace: true,
		},
		{
			// --target takes a value, so that value must travel with it
			// rather than being mistaken for the path.
			name: "valued flag after path", args: []string{"proj", "--target", "x86_64"},
			wantPath: "proj", wantTarget: "x86_64",
		},
		{
			name: "valued flag equals form", args: []string{"proj", "--target=arm64", "--strace"},
			wantPath: "proj", wantTarget: "arm64", wantStrace: true,
		},
		{
			name: "no args at all", args: []string{},
			wantPath: "",
		},
		{
			// After a bare --, nothing is a flag any more.
			name: "double dash guards positionals", args: []string{"--strace", "--", "--target"},
			wantPath: "--target", wantStrace: true,
		},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			fs, target, strace, loaderDebug := newTestFlags()
			// --plain isn't declared in newTestFlags; declare it so parsing
			// of the realistic arg list doesn't fail on an unknown flag.
			fs.Bool("plain", false, "")
			if err := fs.Parse(permuteArgs(fs, c.args)); err != nil {
				t.Fatalf("parse: %v", err)
			}
			path := ""
			if fs.NArg() > 0 {
				path = fs.Arg(0)
			}
			if path != c.wantPath {
				t.Errorf("path = %q, want %q", path, c.wantPath)
			}
			if *target != c.wantTarget {
				t.Errorf("target = %q, want %q", *target, c.wantTarget)
			}
			if *strace != c.wantStrace {
				t.Errorf("strace = %v, want %v", *strace, c.wantStrace)
			}
			if *loaderDebug != c.wantLoader {
				t.Errorf("loaderDebug = %v, want %v", *loaderDebug, c.wantLoader)
			}
		})
	}
}
