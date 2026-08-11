// Package entry finds a project's entrypoint file.
//
// Providers cannot assume the entrypoint sits in the project root: putting
// sources under app/ or src/ is at least as common, and several of this
// repo's own examples do exactly that (examples/unikraft-deno keeps
// app/server.js, examples/unikraft-expressjs keeps app/index.js). A provider
// that only looked in the root would silently build an image with no
// program in it.
package entry

import (
	"os"
	"path/filepath"
)

// SearchDirs are the directories to look in, in order. "" is the project
// root itself.
var SearchDirs = []string{"", "app", "src"}

// Find returns the first candidate that exists, as a path relative to the
// project root (e.g. "app/server.js"), and whether one was found. Candidates
// are tried in order within each directory, so the caller's preference wins
// over location.
func Find(dir string, candidates []string) (string, bool) {
	for _, sub := range SearchDirs {
		for _, name := range candidates {
			rel := filepath.Join(sub, name)
			if st, err := os.Stat(filepath.Join(dir, rel)); err == nil && !st.IsDir() {
				return rel, true
			}
		}
	}
	return "", false
}

// FindOr is Find with a fallback for when nothing matches — the build will
// fail loudly in the guest rather than the plan failing here, which keeps
// the error next to the thing that is actually missing.
func FindOr(dir string, candidates []string, fallback string) string {
	if found, ok := Find(dir, candidates); ok {
		return found
	}
	return fallback
}
