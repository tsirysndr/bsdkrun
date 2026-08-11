// Package php builds PHP projects. Ported from examples/unikraft-php.
package php

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/entry"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
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
	main := entry.FindOr(dir,
		[]string{"server.php", "index.php", "app.php", "main.php"}, "server.php")

	version := defaultVersion
	if v, ok := versions.Read(dir).Version("php"); ok {
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
		// llb.Image pulls in the image's filesystem but not its config, so
		// the ENV the php image sets is absent unless restated here.
		// PHP_INI_DIR is the one that bites: docker-php-ext-enable writes
		// "$PHP_INI_DIR/conf.d/...", which without it becomes /conf.d and
		// fails with "Directory nonexistent" — taking the whole build down
		// on a line that looks unrelated.
		Env: map[string]string{
			"PHP_INI_DIR": "/usr/local/etc/php",
			"PATH":        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
		},
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
# A project php.ini has to land where PHP reads it, not beside the sources.
# It is usually load-bearing: examples/unikraft-php's is what enables the
# sockets extension the server needs.
[ -f php.ini ] && cp php.ini /out/rootfs/usr/local/etc/php/php.ini || true
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs),
		Cmd: []string{"/usr/local/bin/php", "/usr/src/" + main},
	}, nil
}
