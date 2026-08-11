// Package zig builds Zig projects.
//
// Zig cross-compiles to static musl binaries without a cross toolchain,
// which is as close to a unikernel's ideal input as a language gets: one
// file, no interpreter, no shared libraries.
package zig

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultVersion = "0.13.0"

// VersionEnv pins the compiler, overriding any .tool-versions or mise pin.
const VersionEnv = "ZIG_VERSION"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "zig" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "build.zig"))
	if err == nil {
		return true, nil
	}
	if !os.IsNotExist(err) {
		return false, err
	}
	matches, err := filepath.Glob(filepath.Join(dir, "*.zig"))
	if err != nil {
		return false, err
	}
	return len(matches) > 0, nil
}

func (p *Provider) StartCommandHelp() string {
	return fmt.Sprintf(`Zig builds with build.zig, or compiles the single .zig file it finds. %s pins the compiler.`, VersionEnv)
}

func (p *Provider) Plan(dir string, arch plan.Arch) (*plan.Plan, error) {
	version := defaultVersion
	if v, ok := versions.Read(dir).Version("zig"); ok {
		version = v
	}
	// The environment wins: it is the per-build override, where the pin
	// files are the project's own default.
	if v := os.Getenv(VersionEnv); v != "" {
		version = v
	}

	name := artifactName(dir)
	build := `zig build --release=safe -Dtarget=` + zigArch(arch) + `-linux-musl
binary=$(ls -S zig-out/bin/* 2>/dev/null | head -1)`

	// A project with no build.zig is a single file, compiled directly. The
	// same target triple applies: the default would be the build host's
	// glibc, which is dynamically linked and would need its loader in the
	// rootfs.
	if !hasBuildZig(dir) {
		main := singleSource(dir)
		build = fmt.Sprintf(`zig build-exe -target %s-linux-musl -O ReleaseSafe -femit-bin=/tmp/%s %q
binary=/tmp/%s`, zigArch(arch), name, main, name)
	}

	return &plan.Plan{
		Name:       "zig",
		Provider:   p.Name(),
		BuildImage: "debian:bookworm-slim",
		Env: map[string]string{
			"PATH": "/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
			// mise installs the compiler. Zig's own download URLs have
			// changed shape between releases (the tarball is
			// zig-linux-x86_64-<v> in some and zig-x86_64-linux-<v> in
			// others), and mise is what knows which is which.
			"MISE_DATA_DIR": "/opt/mise",
			"MISE_YES":      "1",
		},
		Script: fmt.Sprintf(`set -eu
apt-get update -qq
apt-get install -y -qq --no-install-recommends ca-certificates curl git xz-utils >/dev/null

curl -fsSL https://mise.run | sh
mise install "zig@%s"
export PATH="$(mise where "zig@%s")/bin:$PATH"

%s
if [ -z "$binary" ]; then
    echo "the build produced no binary in zig-out/bin" >&2
    exit 1
fi

mkdir -p /out/rootfs/usr/bin /out/rootfs/tmp
cp "$binary" /out/rootfs/usr/bin/%s
chmod +x /out/rootfs/usr/bin/%s
chmod 1777 /out/rootfs/tmp
`, version, version, build, name, name),
		Cmd: []string{"/usr/bin/" + name},
	}, nil
}

// zigArch maps OCI architecture names to Zig's target triples.
func zigArch(arch plan.Arch) string {
	if arch == plan.ArchArm64 {
		return "aarch64"
	}
	return "x86_64"
}

// artifactName is what the built binary is called. build.zig's
// addExecutable declares it; without one, the single source file does.
func artifactName(dir string) string {
	if name := readArtifactName(filepath.Join(dir, "build.zig")); name != "" {
		return name
	}
	if main := singleSource(dir); main != "" {
		return strings.TrimSuffix(main, ".zig")
	}
	return filepath.Base(dir)
}

// readArtifactName finds `.name = "server"` in a build.zig. Only the first
// is read: a build declaring several executables needs more than this
// provider offers, and the largest artifact is used as a fallback anyway.
func readArtifactName(path string) string {
	body, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	_, after, ok := strings.Cut(string(body), ".name = ")
	if !ok {
		return ""
	}
	_, after, ok = strings.Cut(after, `"`)
	if !ok {
		return ""
	}
	name, _, ok := strings.Cut(after, `"`)
	if !ok {
		return ""
	}
	return name
}

func hasBuildZig(dir string) bool {
	_, err := os.Stat(filepath.Join(dir, "build.zig"))
	return err == nil
}

// singleSource picks the entry file for a project with no build.zig:
// main.zig by convention, or the only .zig file present.
func singleSource(dir string) string {
	if _, err := os.Stat(filepath.Join(dir, "main.zig")); err == nil {
		return "main.zig"
	}
	if matches, _ := filepath.Glob(filepath.Join(dir, "*.zig")); len(matches) == 1 {
		return filepath.Base(matches[0])
	}
	return "main.zig"
}
