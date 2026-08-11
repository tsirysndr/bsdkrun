// Package deno builds Deno projects. Ported from examples/unikraft-deno.
package deno

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "deno" }

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range []string{"deno.json", "deno.jsonc", "deno.lock"} {
		_, err := os.Stat(filepath.Join(dir, marker))
		if err == nil {
			return true, nil
		}
		if !os.IsNotExist(err) {
			return false, err
		}
	}
	return false, nil
}

func (p *Provider) StartCommandHelp() string {
	return `Deno runs server.js with --allow-net. A Procfile "web:" line overrides it.`
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	entry := "server.js"
	for _, c := range []string{"main.ts", "server.ts", "main.js", "server.js"} {
		if _, err := os.Stat(filepath.Join(dir, c)); err == nil {
			entry = c
			break
		}
	}

	// denoland/deno's Debian image, not its "alpine" one: that tag does not
	// ship a musl Deno, it runs the glibc binary through a glibc shim whose
	// own ldd reports a broken relocation — a worse starting point, not a
	// better one.
	return &plan.Plan{
		Name:       "deno",
		Provider:   p.Name(),
		BuildImage: "denoland/deno:latest",
		Script: fmt.Sprintf(`set -eu
%smkdir -p /out/rootfs/usr/bin /out/rootfs/usr/src /out/rootfs/tmp
cp "$(command -v deno)" /out/rootfs/usr/bin/deno
ldd_into_rootfs "$(command -v deno)"
cp -a . /out/rootfs/usr/src/ 2>/dev/null || true
`, plan.LddIntoRootfs),
		// --quiet is required, not cosmetic: Deno's progress bar otherwise
		// redraws in a tight loop and starves the thread doing the real
		// work.
		Cmd: []string{"/usr/bin/deno", "run", "--quiet", "--allow-net", "/usr/src/" + entry},
	}, nil
}
