package kraft

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
)

// kraftVersion pins the kraftkit release the macOS container path installs —
// same pin build.sh uses, since Kraftfile-spec compatibility matters more
// than always tracking latest.
const kraftVersion = "0.12.15"

// Build runs kraft's fetch + patch + build pipeline against dir (which must
// already have a generated Kraftfile — see internal/kraftfile) and
// rootfsRelDir (a directory already built by internal/buildkit, named
// relative to dir — e.g. ".rootfs-arm64"), producing
// dir/.unikraft/build/<name>_fc-<arch>.
//
// onPhase (nil-safe) is called with "fetch" then "build" as the pipeline
// crosses those sub-phase boundaries (skipping straight to "build" when an
// already-fetched tree is reused); onLine (nil-safe) receives every other
// line of output. Both let a caller (internal/tui) render this live instead
// of only seeing the result once Build returns.
func Build(ctx context.Context, dir string, arch Arch, name, rootfsRelDir string, onPhase func(string), onLine func(string)) error {
	kraftfileRel, err := stripKraftfile(dir, arch)
	if err != nil {
		return err
	}

	// Reuse an already-fetched-and-patched tree when there is one: it's most
	// of the wall clock on a rebuild (mirrors build.sh's SKIP_FETCH=1, made
	// the default here — a dev-facing tool that re-fetched Unikraft's source
	// tree on every run would be unusable for iteration).
	skipFetch := unikraftTreeExists(dir)
	steps := kraftSteps(kraftfileRel, rootfsRelDir, name, arch, skipFetch)
	w := &lineWriter{onPhase: onPhase, onLine: onLine}

	if runtime.GOOS == "linux" {
		return buildLinux(ctx, dir, arch, steps, w)
	}
	return buildContainer(ctx, dir, steps, w)
}

func unikraftTreeExists(dir string) bool {
	_, err := os.Stat(filepath.Join(dir, ".unikraft", "unikraft", "Makefile.uk"))
	return err == nil
}

// stripKraftfile writes .Kraftfile.<arch> containing only the wanted
// `targets:` entry and returns its (dir-relative) name. `kraft fetch`
// ignores --arch/--plat/--target and always configures the Kraftfile's
// *first* target, so a Kraftfile listing both fc/arm64 and fc/x86_64 would
// make an x86_64 fetch try to configure arm64 and cross-compile for the
// wrong arch.
func stripKraftfile(dir string, arch Arch) (string, error) {
	src := filepath.Join(dir, "Kraftfile")
	data, err := os.ReadFile(src)
	if err != nil {
		return "", fmt.Errorf("reading %s: %w", src, err)
	}

	other := "- fc/" + string(arch.other())
	want := "- fc/" + string(arch)
	found := false
	var kept []string
	for _, line := range strings.Split(string(data), "\n") {
		trimmed := strings.TrimRight(line, "\r")
		if trimmed == other {
			continue
		}
		if trimmed == want {
			found = true
		}
		kept = append(kept, line)
	}
	if !found {
		return "", fmt.Errorf("%s has no %s target", src, want)
	}

	name := ".Kraftfile." + string(arch)
	if err := os.WriteFile(filepath.Join(dir, name), []byte(strings.Join(kept, "\n")), 0o644); err != nil {
		return "", err
	}
	return name, nil
}

// kraftSteps is the fetch/patch/build shell script, shared verbatim between
// the native (Linux) and containerized (macOS) paths — same commands
// build.sh's $KRAFT_STEPS runs, minus the ELFLOADER_RS local-checkout
// override (a build.sh dev convenience, not something `pack` needs).
func kraftSteps(kraftfileRel, rootfsRelDir, name string, arch Arch, skipFetch bool) string {
	// Emits a line lineWriter recognizes as a phase transition rather than
	// build output — see phaseMarker's doc. Safe as a plain double-quoted
	// echo: phase is always one of the fixed words below, and the marker's
	// control bytes need no shell escaping to come through literally.
	echoPhase := func(b *strings.Builder, phase string) {
		fmt.Fprintf(b, "echo \"%s%s\"\n", phaseMarker, phase)
	}

	var b strings.Builder
	echoPhase(&b, "fetch")
	if !skipFetch {
		// kraft merges into an existing directory, so a tree left over from
		// the C app-elfloader keeps its elf_load.c after the source
		// changes; wipe it only in that case, not unconditionally (that
		// would re-download the loader on every build).
		b.WriteString("kraft pkg update\n")
		b.WriteString("[ -f .unikraft/apps/elfloader/elf_load.c ] && rm -rf .unikraft/apps/elfloader || true\n")
		fmt.Fprintf(&b, "kraft fetch -K %s\n", kraftfileRel)
		b.WriteString("\"$PATCHES_DIR/apply.sh\" .unikraft/unikraft\n")
	} else {
		// apply.sh is idempotent (it checks before patching), so re-running
		// it against a cached tree is cheap and guards against a patches
		// update since the tree was last fetched.
		b.WriteString("\"$PATCHES_DIR/apply.sh\" .unikraft/unikraft\n")
	}
	echoPhase(&b, "build")
	fmt.Fprintf(&b, "kraft build -K %s --rootfs %s --log-level info --log-type basic\n", kraftfileRel, rootfsRelDir)
	fmt.Fprintf(&b, "ls -l .unikraft/build/%s_fc-%s\n", name, arch)
	return b.String()
}

// buildLinux runs kraftSteps directly: build.sh does the same, since a
// Linux host already has everything (kraft, cargo, a Linux toolchain) the
// build needs.
func buildLinux(ctx context.Context, dir string, arch Arch, steps string, w *lineWriter) error {
	if _, err := exec.LookPath("kraft"); err != nil {
		return fmt.Errorf("kraft not found on PATH; install it from https://get.kraftkit.sh")
	}
	if _, err := exec.LookPath("cargo"); err != nil {
		return fmt.Errorf("the Rust loader needs cargo on PATH; install it from https://rustup.rs")
	}
	if out, err := exec.CommandContext(ctx, "rustup", "target", "add", arch.rustTarget()).CombinedOutput(); err != nil {
		return fmt.Errorf("rustup target add %s: %w\n%s", arch.rustTarget(), err, out)
	}

	patchesDir, err := materializePatches()
	if err != nil {
		return err
	}
	defer os.RemoveAll(patchesDir)

	rustCache := filepath.Join(dir, ".rust-cache", "target")
	if err := os.MkdirAll(rustCache, 0o755); err != nil {
		return err
	}

	cmd := exec.CommandContext(ctx, "sh", "-eux", "-c", steps)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(),
		"APPELFLOADER_RUST_CACHE="+rustCache,
		"PATCHES_DIR="+patchesDir,
		"KRAFTKIT_NO_PROMPT=1",
	)
	cmd.Stdout = w
	cmd.Stderr = w
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("kraft build: %w", err)
	}
	return nil
}

// buildContainer runs the same steps inside a container: the Unikraft build
// tree needs GNU make/sed and a Linux toolchain that macOS doesn't have. The
// container's toolchain (packages, kraftkit .deb, rustup) is baked into a
// cached image by ensureBuilderImage rather than reinstalled by this
// function on every run — see image.go's doc comment for why.
//
// Unlike buildLinux this takes no Arch: the image carries the cross
// toolchains for both arches, and kraftSteps already names the target.
func buildContainer(ctx context.Context, dir string, steps string, w *lineWriter) error {
	if _, err := exec.LookPath("docker"); err != nil {
		return fmt.Errorf("bsdkrun pack needs docker on PATH to build the unikernel on macOS: %w", err)
	}

	patchesDir, err := materializePatches()
	if err != nil {
		return err
	}
	defer os.RemoveAll(patchesDir)

	// Only the loader's cargo build cache and its crates.io registry cache
	// need to persist per-project/host — the rustup toolchain itself now
	// lives in the builder image, not here.
	rustCacheDir := filepath.Join(dir, ".rust-cache")
	for _, sub := range []string{"target", "cargo-registry"} {
		if err := os.MkdirAll(filepath.Join(rustCacheDir, sub), 0o755); err != nil {
			return err
		}
	}

	hostDebArch, err := hostDebArch()
	if err != nil {
		return err
	}

	image, err := ensureBuilderImage(ctx, hostDebArch, w)
	if err != nil {
		return err
	}

	args := []string{
		"run", "--rm",
		"-e", "KRAFTKIT_NO_PROMPT=1",
		"-e", "PATCHES_DIR=/patches",
		"-e", "APPELFLOADER_RUST_CACHE=/rust/target",
		"-v", dir + ":/w",
		"-v", patchesDir + ":/patches:ro",
		"-v", filepath.Join(rustCacheDir, "target") + ":/rust/target",
		"-v", filepath.Join(rustCacheDir, "cargo-registry") + ":/root/.cargo/registry",
		"-w", "/w",
		image,
		"sh", "-eux", "-c", steps,
	}
	cmd := exec.CommandContext(ctx, "docker", args...)
	cmd.Stdout = w
	cmd.Stderr = w
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("kraft build (container): %w", err)
	}
	return nil
}

func hostDebArch() (string, error) {
	switch runtime.GOARCH {
	case "arm64":
		return "arm64", nil
	case "amd64":
		return "amd64", nil
	default:
		return "", fmt.Errorf("unsupported host arch %q", runtime.GOARCH)
	}
}
