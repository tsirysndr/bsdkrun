// Package node builds Node.js projects. Ported from
// examples/unikraft-expressjs.
package node

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/entry"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultVersion = "24"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "node" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "package.json"))
	if err == nil {
		return true, nil
	}
	if os.IsNotExist(err) {
		return false, nil
	}
	return false, err
}

func (p *Provider) StartCommandHelp() string {
	return `Node runs package.json's "main" (default server.js). A Procfile "web:" line overrides it.`
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	entry := mainEntry(dir)

	version := defaultVersion
	if v, ok := versions.Read(dir).Major("node"); ok {
		version = v
	}

	// Alpine (musl) rather than Debian: node's own image ships a real musl
	// build, and the smaller rootfs matters because it is resident twice at
	// boot — embedded in the kernel image *and* unpacked into ramfs.
	return &plan.Plan{
		Name:       "node",
		Provider:   p.Name(),
		BuildImage: "node:" + version + "-alpine",
		Script: fmt.Sprintf(`set -eu
%smkdir -p /out/rootfs/usr/bin /out/rootfs/usr/src /out/rootfs/tmp
cp "$(command -v node)" /out/rootfs/usr/bin/node
ldd_into_rootfs "$(command -v node)"
if [ -f package.json ]; then
    npm install --omit=dev --no-audit --no-fund
    [ -d node_modules ] && cp -a node_modules /out/rootfs/usr/src/node_modules || true
fi
cp -a . /out/rootfs/usr/src/ 2>/dev/null || true
rm -rf /out/rootfs/usr/src/node_modules/.cache
`, plan.LddIntoRootfs),
		// The runtime needs FP/SIMD and signal delivery: OpenSSL probes for
		// CPU extensions by *executing* an instruction the CPU may not
		// implement and expecting SIGILL back, so without
		// LIBPOSIX_PROCESS_SIGNAL that probe is a fatal trap rather than a
		// recoverable one.
		Kconfig: map[string]string{
			"CONFIG_LIBPOSIX_PROCESS_SIGNAL": "'y'",
			"CONFIG_LIBPOSIX_PROCESS_BRK":    "'y'",
		},
		Cmd: []string{"/usr/bin/node", "/usr/src/" + entry},
	}, nil
}

// mainEntry prefers package.json's "main", then looks for a conventional
// entrypoint in the root, app/ and src/. package.json often omits "main"
// entirely (examples/unikraft-expressjs's does), so the search is what
// actually finds the program most of the time.
func mainEntry(dir string) string {
	data, err := os.ReadFile(filepath.Join(dir, "package.json"))
	if err == nil {
		var pkg struct {
			Main string `json:"main"`
		}
		if json.Unmarshal(data, &pkg) == nil && pkg.Main != "" {
			return pkg.Main
		}
	}
	return entry.FindOr(dir,
		[]string{"server.js", "index.js", "app.js", "main.js"}, "server.js")
}
