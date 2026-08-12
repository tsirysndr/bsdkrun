package bsdkrun

import (
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
)

// Resolution order (first match wins, then cached):
//
//  1. an explicit override set via SetBinaryPath,
//  2. the $BSDKRUN_BIN environment variable,
//  3. bsdkrun on $PATH,
//  4. in-repo dev builds relative to this source file:
//     <repo_root>/target/release/bsdkrun then .../target/debug/bsdkrun.

var (
	binaryMu       sync.Mutex
	binaryOverride string
	binaryResolved string
)

// SetBinaryPath forces the SDK to use a specific bsdkrun binary, bypassing
// discovery. Handy in tests or when running against a locally built debug
// binary.
func SetBinaryPath(path string) {
	binaryMu.Lock()
	defer binaryMu.Unlock()
	binaryOverride = path
	binaryResolved = ""
}

// ResetBinaryCache resets cached discovery state and any override (mainly
// for tests).
func ResetBinaryCache() {
	binaryMu.Lock()
	defer binaryMu.Unlock()
	binaryOverride = ""
	binaryResolved = ""
}

// repoRoot derives the bsdkrun checkout root from this source file's
// compile-time path (sdk/go/binary.go -> two directories up). For a consumer
// pulling the module from a proxy the path points into the module cache,
// where no target/ exists — the candidates simply never match, same as the
// Python SDK's __file__-relative probe outside a checkout.
func repoRoot() string {
	_, file, _, ok := runtime.Caller(0)
	if !ok {
		return ""
	}
	return filepath.Dir(filepath.Dir(filepath.Dir(file)))
}

// binaryCandidates lists candidate locations, in priority order. Callers
// hold binaryMu.
func binaryCandidates() []string {
	var out []string
	if binaryOverride != "" {
		out = append(out, binaryOverride)
	}
	if env := os.Getenv("BSDKRUN_BIN"); env != "" {
		out = append(out, env)
	}
	// A bsdkrun already on PATH wins over in-repo builds.
	if onPath, err := exec.LookPath("bsdkrun"); err == nil {
		out = append(out, onPath)
	}
	if root := repoRoot(); root != "" {
		out = append(out, filepath.Join(root, "target", "release", "bsdkrun"))
		out = append(out, filepath.Join(root, "target", "debug", "bsdkrun"))
	}
	return out
}

// ResolveBinary resolves (and caches) the path to the bsdkrun binary. It
// returns a *BinaryNotFoundError if none of the candidate locations exist.
func ResolveBinary() (string, error) {
	binaryMu.Lock()
	defer binaryMu.Unlock()
	if binaryResolved != "" {
		return binaryResolved, nil
	}
	searched := binaryCandidates()
	for _, candidate := range searched {
		if strings.ContainsRune(candidate, os.PathSeparator) || strings.Contains(candidate, "/") {
			if _, err := os.Stat(candidate); err == nil {
				binaryResolved = candidate
				return candidate, nil
			}
		} else if _, err := exec.LookPath(candidate); err == nil {
			binaryResolved = candidate
			return candidate, nil
		}
	}
	return "", &BinaryNotFoundError{Searched: searched}
}
