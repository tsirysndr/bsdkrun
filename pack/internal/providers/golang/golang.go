// Package golang builds Go projects. Ported from examples/unikraft-go.
package golang

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/mise"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

// amd64TextAddr is where a packed Go binary's text is linked on amd64: 1
// GiB, comfortably above the kernel image (which starts at ~1 MiB and grows
// with the embedded rootfs) and below both the Unikraft heap base
// (0x400000000, 16 GiB) and ukvmem's application range (0x1000000000,
// 64 GiB), so it collides with neither.
const amd64TextAddr = "0x40000000"

const defaultVersion = "1.22"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "go" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "go.mod"))
	if err == nil {
		return true, nil
	}
	if os.IsNotExist(err) {
		return false, nil
	}
	return false, err
}

func (p *Provider) StartCommandHelp() string {
	return "Go builds the package in the project root; the binary is named after go.mod's module path."
}

func (p *Provider) Plan(dir string, arch plan.Arch) (*plan.Plan, error) {
	name, err := moduleName(filepath.Join(dir, "go.mod"))
	if err != nil {
		return nil, err
	}

	version := defaultVersion
	if v, ok := mise.Read(dir).Major("go"); ok {
		version = v
	}

	// `go build` emits a non-PIE ET_EXEC, so its load addresses are fixed
	// rather than relocatable. On amd64 they start at 0x400000 (4 MiB) —
	// and the Unikraft `fc` kernel links at ~1 MiB, growing past 4 MiB once
	// the rootfs is embedded into the image. The loader maps the two
	// read-only segments over the running kernel and then dies mapping the
	// writable one (whose BSS is zero-filled), with no console output,
	// because what it is overwriting is the kernel itself. arm64 is
	// unaffected: it links at 0x10000 with the kernel 2 GiB away, which is
	// why Go has always worked there and never here.
	//
	// -T moves the whole image above that, keeping the binary static and
	// interpreter-free. (-buildmode=pie would also relocate it, but adds a
	// PT_INTERP requiring /lib64/ld-linux-x86-64.so.2 in the rootfs, which
	// defeats CGO_ENABLED=0.) Only amd64 is moved, so the proven arm64
	// layout is left exactly as it is.
	ldflags := "-s -w"
	if arch == plan.ArchAmd64 {
		ldflags += " -T " + amd64TextAddr
	}

	return &plan.Plan{
		Name:       name,
		Provider:   p.Name(),
		BuildImage: "golang:" + version + "-bookworm",
		Env: map[string]string{
			"PATH":   "/go/bin:/usr/local/go/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
			"GOPATH": "/go",
		},
		Script: fmt.Sprintf(`set -eu
mkdir -p /out/rootfs
CGO_ENABLED=0 go build -trimpath -ldflags=%q -o /out/rootfs/%s .
`, ldflags, name),
		Cmd: []string{"/" + name},
	}, nil
}

// moduleName reads the last path element of go.mod's `module` directive,
// e.g. "module github.com/acme/widget" -> "widget" — the same name `go
// build` would give the binary with no `-o`, so it doubles as the rootfs
// binary's filename.
func moduleName(goMod string) (string, error) {
	data, err := os.ReadFile(goMod)
	if err != nil {
		return "", fmt.Errorf("reading %s: %w", goMod, err)
	}
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if after, ok := strings.CutPrefix(line, "module "); ok {
			parts := strings.Split(strings.TrimSpace(after), "/")
			name := parts[len(parts)-1]
			if name == "" {
				return "", fmt.Errorf("%s: empty module path", goMod)
			}
			return name, nil
		}
	}
	return "", fmt.Errorf("%s has no `module` directive", goMod)
}
