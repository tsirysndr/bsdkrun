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

// ServerEnv chooses what serves a framework's public/ document root.
//
//	builtin     php -S, PHP's own single-process server (default)
//	frankenphp  FrankenPHP, a Caddy with PHP embedded
//
// nginx + php-fpm is deliberately not offered. php-fpm is a process
// manager: it fork()s its workers, and Unikraft has no fork() — the guest
// would need two processes and a socket between them, which is a different
// kind of thing from every other target here. FrankenPHP is the answer to
// the same problem, being one process that serves PHP with threads.
const ServerEnv = "BSDKRUN_PHP_SERVER"

// frankenPHPTextAddr relinks FrankenPHP away from the Unikraft fc kernel.
// It is a Go binary, so it has exactly the load-address collision on x86_64
// that providers/golang and providers/static describe — and being cgo, it
// links externally, so the address has to be handed to the system linker
// rather than to Go's.
const frankenPHPTextAddr = "0x40000000"

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
	return `PHP runs server.php (or index.php); a public/index.php is served by php -S on :8080. A Procfile "web:" line overrides it.`
}

func (p *Provider) Plan(dir string, arch plan.Arch) (*plan.Plan, error) {
	main := entry.FindOr(dir,
		[]string{"server.php", "index.php", "app.php", "main.php"}, "server.php")

	// A public/ document root is the framework convention — Laravel,
	// Symfony and anything built on them put their front controller there
	// and nothing else above it. Those apps expect a web SAPI: their
	// index.php reads the request from superglobals and never listens on a
	// socket, so running it as a CLI script produces one response to a
	// request that never came, and exits.
	//
	// PHP's built-in server supplies the SAPI. It is single-process, which
	// suits a guest that is a single CPU, and it is the same server
	// `php artisan serve` and `symfony serve` wrap.
	docroot := ""
	if _, err := os.Stat(filepath.Join(dir, "public", "index.php")); err == nil {
		docroot = "public"
	}

	if docroot != "" && os.Getenv(ServerEnv) == "frankenphp" {
		return frankenPHPPlan(docroot, arch)
	}

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
		Tools: []plan.ToolCopy{
			{Image: "composer:2", Src: "/usr/bin/composer", Dst: "/usr/local/bin/composer"},
		},
		Script: fmt.Sprintf(`set -eu
%sapt-get update -qq
apt-get install -y -qq --no-install-recommends ca-certificates >/dev/null
docker-php-ext-install sockets >/dev/null
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
# Dependencies, when the project declares any. Composer is copied in from
# its own image (see Tools) because the php image does not ship it -- the
# check here used to be for the composer command, which was never present,
# so a project with a composer.json silently got no vendor/ at all and its
# first require() failed in the guest.
#
# unzip and git are what composer uses to fetch packages (dist and source
# form respectively). No "|| true": a project that declares dependencies
# and cannot install them is a broken build, and should say so here rather
# than in a unikernel.
if [ -f composer.json ]; then
    apt-get install -y -qq --no-install-recommends unzip git >/dev/null
    composer install --no-dev --optimize-autoloader --no-interaction --no-progress
fi
cp -a . /out/rootfs/usr/src/ 2>/dev/null || true
# A project php.ini has to land where PHP reads it, not beside the sources.
# It is usually load-bearing: examples/unikraft-php's is what enables the
# sockets extension the server needs.
[ -f php.ini ] && cp php.ini /out/rootfs/usr/local/etc/php/php.ini || true
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs),
		Cmd: command(docroot, main),
	}, nil
}

// frankenPHPPlan serves the document root with FrankenPHP instead of PHP's
// built-in server.
//
// It is compiled rather than taken from the official image, for the reason
// in frankenPHPTextAddr: a released Go binary for linux/amd64 loads exactly
// where the fc kernel already is, and only a build can move it.
//
// EXPERIMENTAL: this path has not yet been booted. The build is a long one
// — FrankenPHP's static builder compiles PHP and its dependencies from
// source — so it is opt-in, and `builtin` remains the default because that
// is what is verified.
func frankenPHPPlan(docroot string, arch plan.Arch) (*plan.Plan, error) {
	extldflags := ""
	if arch == plan.ArchAmd64 {
		extldflags = fmt.Sprintf(` -extldflags "-Wl,-Ttext-segment=%s"`, frankenPHPTextAddr)
	}

	return &plan.Plan{
		Name:     "php",
		Provider: "php",
		// The static builder carries a PHP built for embedding, which is
		// what makes a single-binary FrankenPHP possible at all.
		BuildImage: "dunglas/frankenphp:static-builder",
		Env: map[string]string{
			"XCADDY_GO_BUILD_FLAGS": fmt.Sprintf(`-ldflags "-w -s%s"`, extldflags),
		},
		Script: fmt.Sprintf(`set -eu
if [ -f composer.json ]; then
    composer install --no-dev --optimize-autoloader --no-interaction --no-progress
fi

cd /go/src/app
EMBED=/src ./build-static.sh

binary=$(ls -S dist/frankenphp-linux-* 2>/dev/null | head -1)
if [ -z "$binary" ]; then
    echo "the static builder produced no binary in dist/" >&2
    exit 1
fi

mkdir -p /out/rootfs/usr/bin /out/rootfs/tmp
cp "$binary" /out/rootfs/usr/bin/frankenphp
chmod +x /out/rootfs/usr/bin/frankenphp
chmod 1777 /out/rootfs/tmp
`),
		Cmd: []string{
			"/usr/bin/frankenphp", "php-server",
			"--listen", "0.0.0.0:8080",
			"--root", "/app/" + docroot,
		},
	}, nil
}

// command is the guest argv: the built-in server for a framework's public/
// document root, or the script itself for a single-file service that does
// its own listening.
func command(docroot, main string) []string {
	if docroot != "" {
		return []string{
			"/usr/local/bin/php", "-S", "0.0.0.0:8080",
			"-t", "/usr/src/" + docroot,
			"/usr/src/" + docroot + "/index.php",
		}
	}
	return []string{"/usr/local/bin/php", "/usr/src/" + main}
}
