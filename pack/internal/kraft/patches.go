// Package kraft drives `kraft`'s fetch/patch/build pipeline against a
// generated Kraftfile + rootfs, producing a bootable unikernel image at
// .unikraft/build/<name>_fc-<arch> — the same result `bsdkrun unikraft .`
// already knows how to boot.
//
// It is a Go port of every examples/*/build.sh's kraft half — confirmed
// byte-identical across all 20 examples — not a reimplementation: same
// fetch/patch/build steps, same per-arch Kraftfile stripping, same
// macOS-container / Linux-native split, because that script is already
// proven across every example in this repo.
package kraft

import (
	"embed"
	"io/fs"
	"os"
	"path/filepath"
)

// patchesFS is `pack`'s own copy of library/unikraft-base/patches — vendored
// rather than read from a sibling directory, because `pack` runs against
// arbitrary projects outside this repo and has to be self-contained.
//
//go:embed patches
var patchesFS embed.FS

// materializePatches extracts the embedded patches to a fresh temp
// directory (so apply.sh has real paths to work with, both run natively and
// bind-mounted into a container) and returns it. The caller removes it.
func materializePatches() (string, error) {
	dir, err := os.MkdirTemp("", "bsdkrun-pack-patches-")
	if err != nil {
		return "", err
	}
	err = fs.WalkDir(patchesFS, "patches", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel("patches", path)
		if err != nil {
			return err
		}
		target := filepath.Join(dir, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		data, err := patchesFS.ReadFile(path)
		if err != nil {
			return err
		}
		mode := os.FileMode(0o644)
		if filepath.Ext(path) == ".sh" {
			mode = 0o755
		}
		return os.WriteFile(target, data, mode)
	})
	if err != nil {
		os.RemoveAll(dir)
		return "", err
	}
	return dir, nil
}
