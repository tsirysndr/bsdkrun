// Package clean removes what `bsdkrun pack` leaves behind: a project's
// generated build artifacts, and the shared Docker resources every project
// on this host reuses.
package clean

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	bkclient "github.com/moby/buildkit/client"

	"github.com/tsirysndr/bsdkrun/pack/internal/kraftfile"
)

// Removed describes what a clean did, so the caller can report it rather
// than deleting things silently.
type Removed struct {
	Paths []string
	Notes []string
}

// Project removes the artifacts pack generated in dir: the fetched-and-
// patched Unikraft tree, the exported rootfs, the loader's cargo cache, and
// the generated Kraftfile and kconfig.
//
// The Kraftfile is only removed when it carries pack's generated marker. A
// hand-written one — which every unported examples/unikraft-* still has —
// is left alone and reported, because deleting a file someone wrote by hand
// is not something a cache clean should ever do.
func Project(dir string) (*Removed, error) {
	out := &Removed{}

	fixed := []string{".unikraft", ".rust-cache"}
	for _, name := range fixed {
		p := filepath.Join(dir, name)
		if _, err := os.Stat(p); err == nil {
			if err := os.RemoveAll(p); err != nil {
				return out, err
			}
			out.Paths = append(out.Paths, name)
		}
	}

	// Globs: .rootfs-<arch>/, .config.<name>_fc-<arch>[.old], .Kraftfile.<arch>
	for _, pattern := range []string{".rootfs-*", ".config.*", ".Kraftfile.*"} {
		matches, err := filepath.Glob(filepath.Join(dir, pattern))
		if err != nil {
			return out, err
		}
		for _, m := range matches {
			if err := os.RemoveAll(m); err != nil {
				return out, err
			}
			out.Paths = append(out.Paths, filepath.Base(m))
		}
	}

	kf := filepath.Join(dir, "Kraftfile")
	switch data, err := os.ReadFile(kf); {
	case err != nil:
		// No Kraftfile is the normal case for a pack-only project that has
		// not been built yet.
	case strings.Contains(string(data), kraftfile.GeneratedMarker):
		if err := os.Remove(kf); err != nil {
			return out, err
		}
		out.Paths = append(out.Paths, "Kraftfile")
	default:
		out.Notes = append(out.Notes,
			"kept Kraftfile: hand-written (no pack marker), not ours to delete")
	}

	return out, nil
}

// Shared removes the Docker resources pack reuses across projects: the
// long-lived buildkitd container (and with it BuildKit's own build cache)
// and the cached kraft builder image.
//
// Separate from Project because these are expensive to rebuild — a
// buildkitd cache miss re-pulls every base image, and the builder image is
// a full apt+rustup install — so removing them should be asked for
// explicitly rather than bundled into a project clean.
func Shared(ctx context.Context) (*Removed, error) {
	out := &Removed{}
	if _, err := exec.LookPath("docker"); err != nil {
		out.Notes = append(out.Notes, "docker not on PATH; nothing shared to remove")
		return out, nil
	}

	// Prune BuildKit's cache through its own API first. Removing the
	// container below would discard it anyway (the cache lives in the
	// container's filesystem, not a volume), but pruning reports how much
	// was actually freed, and it is the graceful path — it works while
	// buildkitd is running and does not depend on the container being
	// disposable.
	if freed, err := pruneBuildKit(ctx); err == nil && freed > 0 {
		out.Paths = append(out.Paths,
			fmt.Sprintf("BuildKit build cache (%s)", humanBytes(freed)))
	}

	if err := exec.CommandContext(ctx, "docker", "rm", "-f",
		"bsdkrun-pack-buildkitd").Run(); err == nil {
		out.Paths = append(out.Paths, "container bsdkrun-pack-buildkitd")
	}

	// The builder image is tagged per kraft version and host arch, so remove
	// every tag rather than guessing which ones this host has built.
	ids, err := exec.CommandContext(ctx, "docker", "images",
		"--filter", "reference=bsdkrun-pack-builder", "--format", "{{.Repository}}:{{.Tag}}").Output()
	if err == nil {
		for _, tag := range strings.Fields(string(ids)) {
			if exec.CommandContext(ctx, "docker", "rmi", "-f", tag).Run() == nil {
				out.Paths = append(out.Paths, "image "+tag)
			}
		}
	}
	if len(out.Paths) == 0 {
		out.Notes = append(out.Notes, "nothing shared to remove")
	}
	return out, nil
}

// pruneBuildKit clears buildkitd's build cache, returning the bytes freed.
// A daemon that is not running is not an error: there is then no cache.
func pruneBuildKit(ctx context.Context) (int64, error) {
	port, err := exec.CommandContext(ctx, "docker", "port",
		"bsdkrun-pack-buildkitd", "1234/tcp").Output()
	if err != nil {
		return 0, err
	}
	line := strings.TrimSpace(strings.Split(string(port), "\n")[0])
	if line == "" {
		return 0, fmt.Errorf("no published port")
	}
	c, err := bkclient.New(ctx, "tcp://"+line)
	if err != nil {
		return 0, err
	}
	defer c.Close()

	ch := make(chan bkclient.UsageInfo)
	var freed int64
	done := make(chan struct{})
	go func() {
		defer close(done)
		for u := range ch {
			freed += u.Size
		}
	}()
	err = c.Prune(ctx, ch, bkclient.PruneAll)
	close(ch)
	<-done
	return freed, err
}

func humanBytes(n int64) string {
	const unit = 1024
	if n < unit {
		return fmt.Sprintf("%d B", n)
	}
	div, exp := int64(unit), 0
	for m := n / unit; m >= unit; m /= unit {
		div *= unit
		exp++
	}
	return fmt.Sprintf("%.1f %cB", float64(n)/float64(div), "KMGT"[exp])
}

// Report renders what was removed.
func (r *Removed) Report(prefix string) string {
	var b strings.Builder
	for _, p := range r.Paths {
		fmt.Fprintf(&b, "%sremoved %s\n", prefix, p)
	}
	for _, n := range r.Notes {
		fmt.Fprintf(&b, "%s%s\n", prefix, n)
	}
	if b.Len() == 0 {
		fmt.Fprintf(&b, "%snothing to remove\n", prefix)
	}
	return b.String()
}
