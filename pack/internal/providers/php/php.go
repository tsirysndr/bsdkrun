// Package php builds PHP projects. Ported from examples/unikraft-php.
package php

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/mise"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

const defaultVersion = "8.2"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "php" }

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range []string{"composer.json", "index.php", "server.php"} {
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
	return `PHP runs server.php (or index.php). A Procfile "web:" line overrides it.`
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	entry := "server.php"
	if _, err := os.Stat(filepath.Join(dir, "server.php")); err != nil {
		if _, err := os.Stat(filepath.Join(dir, "index.php")); err == nil {
			entry = "index.php"
		}
	}

	version := defaultVersion
	if v, ok := mise.Read(dir).Version("php"); ok {
		version = v
	}

	// `sockets` is not optional for anything that serves: PHP's stream
	// server needs it and the official image does not build it in. The
	// extension directory is queried rather than hardcoded — it carries the
	// PHP version and ABI in its path.
	return &plan.Plan{
		Name:       "php",
		Provider:   p.Name(),
		BuildImage: "php:" + version + "-cli-bookworm",
		Script: fmt.Sprintf(`set -eu
%sdocker-php-ext-install sockets >/dev/null
mkdir -p /out/rootfs/usr/local/bin /out/rootfs/usr/local/etc/php /out/rootfs/usr/src /out/rootfs/tmp
cp /usr/local/bin/php /out/rootfs/usr/local/bin/php
ext_dir=$(php -r 'echo ini_get("extension_dir");')
{ ldd /usr/local/bin/php
  for so in "$ext_dir"/*.so; do ldd "$so"; done; } \
  | grep -oE '/[^ ()]+' \
  | sort -u \
  | while read -r lib; do
        mkdir -p "/out/rootfs$(dirname "$lib")"
        cp -L "$lib" "/out/rootfs$lib"
    done
mkdir -p "/out/rootfs$ext_dir"
cp -a "$ext_dir"/. "/out/rootfs$ext_dir/"
if [ -f composer.json ] && command -v composer >/dev/null 2>&1; then
    composer install --no-dev --no-interaction || true
fi
cp -a . /out/rootfs/usr/src/ 2>/dev/null || true
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs),
		Cmd: []string{"/usr/local/bin/php", "/usr/src/" + entry},
	}, nil
}
