// Package bun builds Bun projects. Ported from examples/unikraft-bun.
package bun

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/entry"
)

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "bun" }

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range []string{"bun.lockb", "bun.lock", "bunfig.toml"} {
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
	return `Bun runs server.js. A Procfile "web:" line overrides it.`
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	main := entry.FindOr(dir,
		[]string{"index.ts", "server.ts", "index.js", "server.js", "main.ts", "main.js"},
		"server.js")

	// Bun's Alpine image is a genuine musl build — unlike Deno's, which is
	// glibc behind a shim — so there is no reason to prefer Debian here.
	//
	// The /proc/self/exe symlink is load-bearing: Bun locates its own binary
	// through it at startup, to symlink itself as `node` so child processes
	// get Bun. Unikraft has no procfs, so the read fails and Bun panics with
	// error.FileNotFound before running a line of JavaScript. A static
	// symlink is the correct answer rather than a hack — a unikernel runs
	// exactly one program, so the target is known at build time.
	return &plan.Plan{
		Name:       "bun",
		Provider:   p.Name(),
		BuildImage: "oven/bun:alpine",
		Script: fmt.Sprintf(`set -eu
%smkdir -p /out/rootfs/usr/bin /out/rootfs/usr/src /out/rootfs/tmp
cp "$(command -v bun)" /out/rootfs/usr/bin/bun
ldd_into_rootfs "$(command -v bun)"
if [ -f package.json ]; then bun install --production || true; fi
cp -a . /out/rootfs/usr/src/ 2>/dev/null || true
mkdir -p /out/rootfs/proc/self
ln -sf /usr/bin/bun /out/rootfs/proc/self/exe
`, plan.LddIntoRootfs),
		Kconfig: map[string]string{
			// JavaScriptCore's concurrent GC needs threading behaviour the
			// guest does not provide; without this Bun hangs at startup.
			"CONFIG_LIBPOSIX_ENVIRON_ENVP4": `"BUN_JSC_useConcurrentGC=0"`,
		},
		ElfloaderKconfig: map[string]string{
			// Bun needs far more stack than the 128 pages the other
			// runtimes get by default.
			"CONFIG_APPELFLOADER_STACK_NBPAGES": "2048",
		},
		Cmd: []string{"/usr/bin/bun", "run", "/usr/src/" + main},
	}, nil
}
