// Package cachedir locates bsdkrun's cache directory, mirroring
// core/src/fetch.rs::cache_dir() exactly (same env vars, same precedence) so
// `bsdkrun pack`'s buildkitd socket and kraft state share one cache root
// with the rest of bsdkrun instead of inventing a second one.
package cachedir

import (
	"errors"
	"os"
	"path/filepath"
)

// Dir returns $BSDKRUN_CACHE, else $XDG_CACHE_HOME/bsdkrun, else
// $HOME/.cache/bsdkrun.
func Dir() (string, error) {
	if c := os.Getenv("BSDKRUN_CACHE"); c != "" {
		return c, nil
	}
	if x := os.Getenv("XDG_CACHE_HOME"); x != "" {
		return filepath.Join(x, "bsdkrun"), nil
	}
	home := os.Getenv("HOME")
	if home == "" {
		return "", errors.New("HOME is not set")
	}
	return filepath.Join(home, ".cache", "bsdkrun"), nil
}
