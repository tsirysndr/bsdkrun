// Package static serves a directory of files — a built SPA, a generated
// site, a single index.html — with Caddy.
//
// This is the one provider whose "build" produces no application code. What
// it needs from the build stage is a web server, and Caddy is the right one
// here for a reason that is specific to unikernels rather than to taste: it
// is a single static Go binary with no ELF interpreter and no shared
// libraries, so nothing has to be ldd-resolved into the rootfs. The whole
// guest is one file plus the site.
//
// The server is compiled from source rather than copied out of the official
// caddy image, which would otherwise be cheaper. A released Go binary for
// linux/amd64 is a non-PIE ET_EXEC linked at 0x400000, and the Unikraft fc
// kernel occupies that address — so a prebuilt caddy is mapped over the
// running kernel and the guest dies mute. Relinking it elsewhere is only
// possible at build time. See providers/golang for the same problem and
// examples/unikraft-go/README.md for the loader trace.
package static

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

// caddyVersion matches examples/unikraft-caddy, which is where this
// provider's build recipe was proven.
const caddyVersion = "2.7.6"

// amd64TextAddr keeps the server clear of the fc kernel; see the package
// comment and providers/golang, which links application binaries the same
// way for the same reason.
const amd64TextAddr = "0x40000000"

// RootEnv names the directory to serve, for a project whose layout matches
// none of the conventions below. Railpack spells this RAILPACK_STATIC_FILE_ROOT.
const RootEnv = "BSDKRUN_STATIC_FILE_ROOT"

// docroot is where the site lands in the guest.
const docroot = "/var/www"

// port is Caddy's own default. Every other provider here serves on 8080,
// and a static site is no different to whoever is curling it.
const port = "8080"

// candidates are the conventional output directories, most specific first.
// dist/ and build/ come before public/ because a project with both is
// nearly always a bundler's output plus its unprocessed static assets, and
// it is the output that should be served.
var candidates = []string{"dist", "build", "out", "_site", "public"}

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "static" }

func (p *Provider) Detect(dir string) (bool, error) {
	if os.Getenv(RootEnv) != "" {
		return true, nil
	}
	for _, marker := range []string{"Staticfile", "index.html"} {
		_, err := os.Stat(filepath.Join(dir, marker))
		if err == nil {
			return true, nil
		}
		if !os.IsNotExist(err) {
			return false, err
		}
	}
	// A directory only counts if it holds something to serve: an empty
	// public/ left over in a repo is not a static site, and treating it as
	// one would shadow whatever the project actually is.
	for _, name := range candidates {
		if hasFiles(filepath.Join(dir, name)) {
			return true, nil
		}
	}
	return false, nil
}

func (p *Provider) StartCommandHelp() string {
	return fmt.Sprintf(`Static sites are served by Caddy on :%s. Set %s to choose the directory.`, port, RootEnv)
}

func (p *Provider) Plan(dir string, arch plan.Arch) (*plan.Plan, error) {
	root := siteRoot(dir)

	ldflags := "-s -w"
	if arch == plan.ArchAmd64 {
		ldflags += " -T " + amd64TextAddr
	}

	return &plan.Plan{
		Name:       "static",
		Provider:   p.Name(),
		BuildImage: "golang:1.21-bookworm",
		Env: map[string]string{
			"CGO_ENABLED": "0",
			"GOOS":        "linux",
		},
		Script: fmt.Sprintf(`set -eu
apt-get update -qq
apt-get install -y -qq --no-install-recommends git ca-certificates >/dev/null

git clone --depth=1 --branch v%s https://github.com/caddyserver/caddy.git /caddy
cd /caddy
go build -tags netgo -ldflags %q -o /caddy/caddy cmd/caddy/main.go
cd /src

mkdir -p /out/rootfs/usr/bin /out/rootfs/etc/caddy /out/rootfs%s /out/rootfs/tmp
cp /caddy/caddy /out/rootfs/usr/bin/caddy

# The site itself. A trailing /. copies the directory's contents rather than
# the directory, so the document root is what was asked for and not a level
# above it.
cp -a %q/. /out/rootfs%s/

cat > /out/rootfs/etc/caddy/Caddyfile <<'CADDYFILE'
:%s
root * %s
encode gzip
templates
file_server

# A client-side router serves every unknown path from index.html; without
# this, a reload of /about is a 404 from the file server. Harmless for a
# plain site, where index.html is what would be served anyway.
try_files {path} {path}/ /index.html
CADDYFILE

chmod 1777 /out/rootfs/tmp
`, caddyVersion, ldflags, docroot, root, docroot, port, docroot),
		Cmd: []string{"/usr/bin/caddy", "run", "--config", "/etc/caddy/Caddyfile"},
	}, nil
}

// siteRoot picks the directory to serve, relative to the project. The
// environment wins, then an explicit Staticfile, then convention, and
// failing all of those the project root — which is the right answer for the
// index.html-and-nothing-else case that made this provider match.
func siteRoot(dir string) string {
	if root := os.Getenv(RootEnv); root != "" {
		return strings.TrimPrefix(root, "./")
	}
	if root := staticfileRoot(filepath.Join(dir, "Staticfile")); root != "" {
		return root
	}
	for _, name := range candidates {
		if hasFiles(filepath.Join(dir, name)) {
			return name
		}
	}
	return "."
}

// staticfileRoot reads the `root: dist` line out of a Cloud Foundry-style
// Staticfile. The format is one `key: value` per line; only root is
// meaningful here.
func staticfileRoot(path string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		key, value, ok := strings.Cut(scanner.Text(), ":")
		if !ok || strings.TrimSpace(key) != "root" {
			continue
		}
		if root := strings.TrimSpace(value); root != "" {
			return strings.TrimSuffix(strings.TrimPrefix(root, "./"), "/")
		}
	}
	return ""
}

// hasFiles reports whether path is a directory with anything in it.
func hasFiles(path string) bool {
	entries, err := os.ReadDir(path)
	return err == nil && len(entries) > 0
}
