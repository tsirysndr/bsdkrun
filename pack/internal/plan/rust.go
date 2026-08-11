package plan

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/detect"
)

// rustPlan builds a Rust project with `cargo build --release`, then resolves
// its shared-library dependencies with `ldd` and copies them alongside the
// binary — the same approach examples/unikraft-actix/Dockerfile uses and
// explains: `ldd` reports whatever the *target* arch's binary actually
// needs (glibc's path and the dynamic loader's name both differ between
// x86_64 and arm64), so resolving instead of hardcoding a lib list is what
// keeps this correct across architectures.
func rustPlan(dir string) (*Plan, error) {
	name, err := cargoPackageName(filepath.Join(dir, "Cargo.toml"))
	if err != nil {
		return nil, err
	}
	return &Plan{
		Name:       name,
		Provider:   detect.Rust,
		BuildImage: "rust:1.75-bookworm",
		Env: map[string]string{
			"PATH":        "/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
			"CARGO_HOME":  "/usr/local/cargo",
			"RUSTUP_HOME": "/usr/local/rustup",
		},
		Script: fmt.Sprintf(`set -eu
cargo build --release
mkdir -p /out/rootfs/tmp
cp target/release/%s /out/rootfs/%s
ldd target/release/%s \
  | grep -oE '/[^ ()]+' \
  | sort -u \
  | while read -r lib; do
        mkdir -p "/out/rootfs$(dirname "$lib")"
        cp -L "$lib" "/out/rootfs$lib"
    done
`, name, name, name),
		Cmd: []string{"/" + name},
	}, nil
}

var cargoNameRe = regexp.MustCompile(`(?m)^\s*name\s*=\s*"([^"]+)"`)

// cargoPackageName reads `[package] name = "..."` from Cargo.toml. Scoped to
// the [package] section specifically, so a dependency further down the file
// that happens to also declare a `name` key (a renamed dependency) can't be
// mistaken for the project's own name.
func cargoPackageName(cargoToml string) (string, error) {
	data, err := os.ReadFile(cargoToml)
	if err != nil {
		return "", fmt.Errorf("reading %s: %w", cargoToml, err)
	}

	var packageSection strings.Builder
	inPackage := false
	for _, line := range strings.Split(string(data), "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "[") {
			inPackage = trimmed == "[package]"
			continue
		}
		if inPackage {
			packageSection.WriteString(line)
			packageSection.WriteByte('\n')
		}
	}

	m := cargoNameRe.FindStringSubmatch(packageSection.String())
	if m == nil {
		return "", fmt.Errorf("%s: no [package] name", cargoToml)
	}
	return m[1], nil
}
