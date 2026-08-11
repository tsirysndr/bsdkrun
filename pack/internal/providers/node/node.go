// Package node builds Node.js projects. Ported from
// examples/unikraft-expressjs.
package node

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/mise"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
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
	if v, ok := mise.Read(dir).Major("node"); ok {
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

// mainEntry reads package.json's "main", falling back to server.js — the
// same default npm itself documents.
func mainEntry(dir string) string {
	data, err := os.ReadFile(filepath.Join(dir, "package.json"))
	if err != nil {
		return "server.js"
	}
	var pkg struct {
		Main string `json:"main"`
	}
	if err := json.Unmarshal(data, &pkg); err != nil || pkg.Main == "" {
		return "server.js"
	}
	return pkg.Main
}
