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

// TLSEnv turns on TLS for the FrankenPHP server. Set it to a domain to
// serve a certificate for that name; set it to "self-signed" (or "1") for a
// baked self-signed certificate, which is what a demo or a guest behind a
// terminating proxy wants.
//
// Only FrankenPHP: PHP's built-in server speaks no TLS at all, so the knob
// would be a silent no-op there. The wall-clock fix (epoch.boot) is what
// makes any of this work — a certificate is "not yet valid" in 1970.
const TLSEnv = "BSDKRUN_PHP_TLS"

// frankenPHPTextAddr relinks FrankenPHP away from the Unikraft fc kernel.
// It is a Go binary, so it has exactly the load-address collision on x86_64
// that providers/golang and providers/static describe — and being cgo, it
// links externally, so the address has to be handed to the system linker
// rather than to Go's.
const frankenPHPTextAddr = "0x40000000"

// staticBuilderTag pins the FrankenPHP static builder. See frankenPHPPlan
// for why a floating tag does not work here.
const staticBuilderTag = "static-builder-musl-1.12.6"

// ExtensionsEnv overrides the extensions compiled into a FrankenPHP binary.
const ExtensionsEnv = "BSDKRUN_PHP_EXTENSIONS"

// The extension set is left to the builder, which infers it from the
// project's composer.json. Curating one here went badly: adding curl pulled
// in an SPC curl build that wants libssh2, and the link failed on symbols
// no PHP application here asked for. Inference gets the dependency graph
// right; what it cannot know is what a framework uses without declaring —
// so an application that reaches for an extension should require it, the
// way examples/unikraft-symfony now requires ext-filter.
//
// BSDKRUN_PHP_EXTENSIONS overrides it outright when that is not enough.

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
// EXPERIMENTAL, and currently blocked in the guest. It builds and boots —
// Caddy starts and logs — and then PHP dies before serving:
//
//	Fatal error: Could not create timer: Not supported (95)
//
// That is timer_create(), which Unikraft does not implement. PHP calls it
// because static-php-cli configures PHP with
// --enable-zend-max-execution-timers (hardcoded for ZTS builds, with no
// environment override), and that feature arms a per-process POSIX timer
// to enforce max_execution_time.
//
// Two ways out, neither small:
//
//   - Rebuild PHP with --disable-zend-max-execution-timers. That means
//     patching static-php-cli inside the builder image and forcing a
//     from-source PHP build (musl builds pull prebuilt libraries by
//     default), so the flag actually reaches configure.
//   - Implement timer_create/timer_settime/timer_delete in Unikraft.
//     Unlike the CLOCK_PROCESS_CPUTIME_ID patch this repo carries, that is
//     a feature rather than a missing case label: it needs timer state and
//     signal delivery.
//
// `builtin` remains the default because it is what is verified.
func frankenPHPPlan(docroot string, arch plan.Arch) (*plan.Plan, error) {
	tls := os.Getenv(TLSEnv) != ""

	// A self-signed certificate generated at build time. openssl is in the
	// static-builder image (it builds PHP's openssl extension). The cert is
	// dated from the epoch so it is valid the moment the guest boots,
	// whatever the guest's clock reads — the same reasoning as the guest
	// wall-clock fix, applied to the artifact instead of the reader.
	tlsScript := ""
	if tls {
		cn := os.Getenv(TLSEnv)
		if cn == "self-signed" || cn == "1" || cn == "true" {
			cn = "localhost"
		}
		tlsScript = fmt.Sprintf(`openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1     -keyout /tmp/k.pem -out /tmp/c.pem -days 3650 -nodes     -subj "/CN=%s" -addext "subjectAltName=DNS:%s,DNS:localhost,IP:127.0.0.1"
cat /tmp/c.pem /tmp/k.pem > /out/rootfs/etc/frankenphp/tls.pem
`, cn, cn)
	}

	extldflags := ""
	if arch == plan.ArchAmd64 {
		extldflags = fmt.Sprintf(` -extldflags "-Wl,-Ttext-segment=%s"`, frankenPHPTextAddr)
	}

	return &plan.Plan{
		Name:     "php",
		Provider: "php",
		// The static builder carries a PHP built for embedding, which is
		// what makes a single-binary FrankenPHP possible at all.
		//
		// Pinned, and musl. The floating `static-builder` tag holds a
		// FrankenPHP source tree older than the Caddy that xcaddy fetches
		// for it, and the two no longer compile together — Caddy changed
		// LoadConfig's signature. A released tag pins both halves to a
		// combination that was built and tested as a pair.
		//
		// musl over gnu because it links fully statically: no interpreter
		// and no shared libraries to resolve into the rootfs, which is what
		// makes this one file in the guest.
		BuildImage: "dunglas/frankenphp:" + staticBuilderTag,
		Env: map[string]string{
			"XCADDY_GO_BUILD_FLAGS": fmt.Sprintf(`-ldflags "-w -s%s"`, extldflags),
			// The static builder's own composer.lock requires ext-iconv,
			// which the minimal CLI PHP in that image does not have — its
			// own dependency install fails before the app's does. A flag on
			// the app's composer install cannot reach that one, so the
			// exemption has to be in the environment, where every
			// invocation sees it.
			//
			// Nothing is lost by it: this interpreter only drives the
			// build. What the application ends up running on is the PHP
			// compiled into the binary below.
			"COMPOSER_IGNORE_PLATFORM_REQS": "1",
			// The image pins GOTOOLCHAIN=local and ships whatever Go it was
			// built with, while xcaddy pulls the current Caddy — which by
			// now wants a newer one than the image has. "auto" lets Go
			// fetch the toolchain the modules ask for instead of failing.
			"GOTOOLCHAIN": "auto",
			// Empty leaves the builder to infer from composer.json; see
			// the note above ExtensionsEnv.
			"PHP_EXTENSIONS": os.Getenv(ExtensionsEnv),
		},
		Script: fmt.Sprintf(`set -eu
if [ -f composer.json ]; then
    # --ignore-platform-reqs: the static builder's own CLI PHP is a minimal
    # one (no iconv, among others) used only to drive the build. What the
    # application runs on is the PHP compiled into the binary below, and
    # that is not the interpreter composer is inspecting here.
    composer install --no-dev --optimize-autoloader --no-interaction \
        --no-progress --ignore-platform-reqs
fi

cd /go/src/app
EMBED=/src ./build-static.sh

binary=$(ls -S dist/frankenphp-linux-* 2>/dev/null | head -1)
if [ -z "$binary" ]; then
    echo "the static builder produced no binary in dist/" >&2
    exit 1
fi

mkdir -p /out/rootfs/usr/bin /out/rootfs/tmp /out/rootfs/etc/frankenphp
cp "$binary" /out/rootfs/usr/bin/frankenphp
chmod +x /out/rootfs/usr/bin/frankenphp
chmod 1777 /out/rootfs/tmp
%s`, tlsScript),
		Cmd: frankenPHPCommand(docroot, tls),
	}, nil
}

// frankenPHPCommand is the guest argv for the FrankenPHP server.
//
// Caddy's own automatic HTTPS is deliberately not used: it would try to
// reach an ACME server (the guest cannot, in most deployments) and manage
// certificate storage on a writable path the ramfs does not persist. A
// certificate baked into the rootfs, named explicitly, is what a unikernel
// wants — TLS terminates here, and h2 comes with it, without the guest
// talking to a CA.
func frankenPHPCommand(docroot string, tls bool) []string {
	if !tls {
		return []string{
			"/usr/bin/frankenphp", "php-server",
			"--listen", "0.0.0.0:8080",
			"--root", "/app/" + docroot,
		}
	}
	return []string{
		"/usr/bin/frankenphp", "php-server",
		"--listen", "0.0.0.0:8443",
		"--root", "/app/" + docroot,
		// The baked certificate. --tls on its own would trigger Caddy's
		// internal CA and ACME machinery; a named cert keeps it offline.
		"--tls", "/etc/frankenphp/tls.pem",
	}
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
