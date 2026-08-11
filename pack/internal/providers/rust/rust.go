// Package rust builds Rust projects. Ported from examples/unikraft-actix.
package rust

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultVersion = "1.75"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "rust" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "Cargo.toml"))
	if err == nil {
		return true, nil
	}
	if os.IsNotExist(err) {
		return false, nil
	}
	return false, err
}

func (p *Provider) StartCommandHelp() string {
	return "Rust runs the release binary named after Cargo.toml's [package] name."
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	name, err := packageName(filepath.Join(dir, "Cargo.toml"))
	if err != nil {
		return nil, err
	}

	version := defaultVersion
	if v, ok := versions.Read(dir).Version("rust"); ok {
		version = v
	}

	// cargo links against glibc, so unlike Go this rootfs is not just the
	// binary: ldd resolves what the *target* architecture actually needs.
	return &plan.Plan{
		Name:       name,
		Provider:   p.Name(),
		BuildImage: "rust:" + version + "-bookworm",
		Env: map[string]string{
			"PATH":        "/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
			"CARGO_HOME":  "/usr/local/cargo",
			"RUSTUP_HOME": "/usr/local/rustup",
		},
		Script: fmt.Sprintf(`set -eu
%scargo build --release
mkdir -p /out/rootfs/tmp
cp target/release/%s /out/rootfs/%s
ldd_into_rootfs target/release/%s
`, plan.LddIntoRootfs, name, name, name),
		Cmd: []string{"/" + name},
	}, nil
}

var nameRe = regexp.MustCompile(`(?m)^\s*name\s*=\s*"([^"]+)"`)

// packageName reads `[package] name = "..."`, scoped to that section so a
// renamed dependency further down the file cannot be mistaken for it.
func packageName(cargoToml string) (string, error) {
	data, err := os.ReadFile(cargoToml)
	if err != nil {
		return "", fmt.Errorf("reading %s: %w", cargoToml, err)
	}
	var section strings.Builder
	inPackage := false
	for _, line := range strings.Split(string(data), "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "[") {
			inPackage = trimmed == "[package]"
			continue
		}
		if inPackage {
			section.WriteString(line)
			section.WriteByte('\n')
		}
	}
	m := nameRe.FindStringSubmatch(section.String())
	if m == nil {
		return "", fmt.Errorf("%s: no [package] name", cargoToml)
	}
	return m[1], nil
}
