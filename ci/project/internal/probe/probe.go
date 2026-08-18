// Package probe holds the filesystem sniffers every provider shares:
// marker files, bounded test-file walks, cheap content checks. Bounded and
// vendor-aware on purpose — detection must stay instant on a large tree.
package probe

import (
	"os"
	"path/filepath"
	"strings"
)

// Exists reports whether root/rel exists.
func Exists(root, rel string) bool {
	_, err := os.Stat(filepath.Join(root, rel))
	return err == nil
}

// Glob reports whether the (non-recursive) pattern matches anything.
func Glob(root, pattern string) bool {
	names, _ := filepath.Glob(filepath.Join(root, pattern))
	return len(names) > 0
}

// GlobFirst returns the first (non-recursive) match's basename.
func GlobFirst(root, pattern string) string {
	names, _ := filepath.Glob(filepath.Join(root, pattern))
	if len(names) == 0 {
		return ""
	}
	return filepath.Base(names[0])
}

var skipDirs = map[string]bool{
	"node_modules": true, ".git": true, "vendor": true, "target": true,
	"_build": true, "deps": true, ".venv": true, "dist": true,
}

// HasFile walks the tree (bounded, vendor dirs skipped) for a file the
// matcher accepts — filepath.Glob has no `**`, and interesting files rarely
// sit at the root.
func HasFile(root string, match func(name string) bool) bool {
	found := false
	var walk func(dir string, depth int)
	walk = func(dir string, depth int) {
		if found || depth > 6 {
			return
		}
		entries, err := os.ReadDir(dir)
		if err != nil {
			return
		}
		for _, e := range entries {
			if found {
				return
			}
			if e.IsDir() {
				if !skipDirs[e.Name()] && !strings.HasPrefix(e.Name(), ".") {
					walk(filepath.Join(dir, e.Name()), depth+1)
				}
				continue
			}
			if match(e.Name()) {
				found = true
				return
			}
		}
	}
	walk(root, 0)
	return found
}

// Suffix matches any of the given filename suffixes.
func Suffix(suffixes ...string) func(string) bool {
	return func(name string) bool {
		for _, s := range suffixes {
			if strings.HasSuffix(name, s) {
				return true
			}
		}
		return false
	}
}

// Infix matches any of the given filename substrings.
func Infix(infixes ...string) func(string) bool {
	return func(name string) bool {
		for _, in := range infixes {
			if strings.Contains(name, in) {
				return true
			}
		}
		return false
	}
}

// FileContains reports whether the file at path contains needle.
func FileContains(path, needle string) bool {
	data, err := os.ReadFile(path)
	return err == nil && strings.Contains(string(data), needle)
}

// SourceMentions greps a source tree (one extension) for any needle,
// bounded by the same walk rules as HasFile.
func SourceMentions(root, dir, ext string, needles ...string) bool {
	found := false
	var walk func(d string, depth int)
	walk = func(d string, depth int) {
		if found || depth > 6 {
			return
		}
		entries, err := os.ReadDir(d)
		if err != nil {
			return
		}
		for _, e := range entries {
			if found {
				return
			}
			if e.IsDir() {
				walk(filepath.Join(d, e.Name()), depth+1)
				continue
			}
			if !strings.HasSuffix(e.Name(), ext) {
				continue
			}
			data, err := os.ReadFile(filepath.Join(d, e.Name()))
			if err != nil {
				continue
			}
			for _, n := range needles {
				if strings.Contains(string(data), n) {
					found = true
					return
				}
			}
		}
	}
	walk(filepath.Join(root, dir), 0)
	return found
}

// HasPackageScript reports whether package.json declares this script. A
// substring probe, deliberately: pulling a JSON parser to ask one question
// is not worth the false positives it would prevent.
func HasPackageScript(root, name string) bool {
	return FileContains(filepath.Join(root, "package.json"), "\""+name+"\":")
}
