// Package ignore decides what stays out of the build context.
//
// This matters more for pack than for a normal Docker build: pack writes
// its own output *into* the project (.unikraft/ is a full Unikraft source
// tree, hundreds of MB; .rootfs-<arch>/ is the exported image), so without
// excludes every rebuild would upload the previous build back to the daemon
// as context.
package ignore

import (
	"os"
	"path/filepath"
	"strings"
)

// FileName is the ignore file, matching Docker's so a project already
// carrying one needs no second file.
const FileName = ".dockerignore"

// Defaults are always excluded: pack's own generated artifacts, plus the
// version-control and dependency directories no build needs shipped. A
// project should not have to know pack's output layout to avoid uploading
// it.
var Defaults = []string{
	".unikraft",
	".rootfs-*",
	".rust-cache",
	".config.*",
	".Kraftfile.*",
	".git",
	"**/node_modules",
	"**/target",
	"**/_build",
	"**/.gradle",
}

// Read returns the patterns to exclude from dir's build context: the
// defaults, plus whatever .dockerignore lists, plus extra (railpack.json's
// `exclude`).
//
// Negations (!pattern) are passed through as written — fsutil and BuildKit
// both understand them, and re-implementing that matching here would only
// be a way to disagree with them.
func Read(dir string, extra []string) []string {
	out := append([]string{}, Defaults...)

	if data, err := os.ReadFile(filepath.Join(dir, FileName)); err == nil {
		for _, line := range strings.Split(string(data), "\n") {
			line = strings.TrimSpace(line)
			if line == "" || strings.HasPrefix(line, "#") {
				continue
			}
			// Docker treats a leading / as relative to the context root;
			// fsutil's matcher wants it without.
			out = append(out, strings.TrimPrefix(line, "/"))
		}
	}

	for _, e := range extra {
		if e = strings.TrimSpace(e); e != "" {
			out = append(out, strings.TrimPrefix(e, "/"))
		}
	}
	return out
}
