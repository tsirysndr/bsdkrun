// Package php is the php provider: detect the project, generate its
// CI workflow. See ci/project's package comment for the rules every
// provider follows (specific markers first, tests before build, no
// vacuous steps).
package php

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project/internal/probe"
)

// Provider implements project.Provider.
type Provider struct{}

func (Provider) Name() string { return "php" }

func (Provider) Detect(dir string) (string, bool) {
	if probe.Exists(dir, "composer.json") {
		return "composer.json", true
	}
	return "", false
}

func hasComposerScript(dir, name string) bool {
	data := filepath.Join(dir, "composer.json")
	if !probe.FileContains(data, "\"scripts\"") {
		return false
	}
	return probe.FileContains(data, "\""+name+"\"")
}

// phpImage picks the official php image for the project: the newest PHP
// line the major frameworks are known to run on, raised to what
// composer.json's `require.php` asks for when that is higher.
//
// Not composer:2 any more. That image tracks the newest PHP the day it is
// built — 8.6 since 2026-11-19 — and frameworks lag PHP: Laravel's session
// handler trips an 8.6 deprecation that its error handler turns into an
// exception, so every request is a 500 and the project's own tests fail
// through no fault of theirs. The 8.6 build also dropped mbstring.
func phpImage(dir string) string {
	minor := defaultPHPMinor
	if m, ok := requiredPHPMinor(dir); ok && m > minor {
		minor = m
	}
	return fmt.Sprintf("php:8.%d-cli-alpine", minor)
}

// defaultPHPMinor is the PHP 8.x line used when the project does not demand
// a newer one. Bump it when the frameworks have caught up with a release.
const defaultPHPMinor = 4

// requiredPHPMinor reads the 8.x minor out of composer.json's `require.php`
// constraint (`^8.5`, `>=8.5.1`, `~8.5.0`); other majors and missing
// constraints report false.
func requiredPHPMinor(dir string) (int, bool) {
	data, err := os.ReadFile(filepath.Join(dir, "composer.json"))
	if err != nil {
		return 0, false
	}
	var c struct {
		Require map[string]string `json:"require"`
	}
	if json.Unmarshal(data, &c) != nil {
		return 0, false
	}
	m := php8Minor.FindStringSubmatch(c.Require["php"])
	if m == nil {
		return 0, false
	}
	n, err := strconv.Atoi(m[1])
	return n, err == nil
}

var php8Minor = regexp.MustCompile(`\b8\.(\d+)`)

// setupScript readies the php image for composer: composer itself (the
// official php images ship none), unzip (composer's archive path, which the
// php image lacks along with the zip extension) and mbstring (PHPUnit
// refuses to start without it; present in the 8.4/8.5 images, built in on
// demand with the image's own docker-php-ext-install otherwise).
const setupScript = `set -e
command -v composer >/dev/null 2>&1 || {
  php -r "copy('https://getcomposer.org/installer', '/tmp/composer-setup.php');"
  php /tmp/composer-setup.php --install-dir=/usr/local/bin --filename=composer --quiet
  rm -f /tmp/composer-setup.php
}
command -v unzip >/dev/null 2>&1 || { command -v apk >/dev/null 2>&1 && apk add --no-cache unzip >/dev/null 2>&1 || true; }
php -m | grep -qi '^mbstring$' || {
  command -v apk >/dev/null 2>&1 && apk add --no-cache oniguruma-dev >/dev/null 2>&1
  docker-php-ext-install -j"$(nproc)" mbstring >/dev/null 2>&1
}
php -m | grep -qi '^mbstring$' || echo "warning: mbstring is still missing; PHPUnit will not run"
php --version | head -n 1
composer --version`

func (Provider) Job(dir string) platforms.Job {
	// The guest runs as root, and composer refuses to load plugins as root
	// unless told it is fine (composer:2 set this itself; the php image does
	// not). Per step, since each step is its own shell.
	const asRoot = "export COMPOSER_ALLOW_SUPERUSER=1; "
	steps := []platforms.Step{
		step("php setup", setupScript),
		step("composer install", asRoot+"composer install --no-interaction --no-progress"),
	}
	switch {
	case hasComposerScript(dir, "test"):
		steps = append(steps, step("composer test", asRoot+"composer test"))
	case probe.Exists(dir, "phpunit.xml") || probe.Exists(dir, "phpunit.xml.dist"):
		steps = append(steps, step("phpunit", "vendor/bin/phpunit"))
	}
	return job(phpImage(dir), steps...)
}

func job(image string, steps ...platforms.Step) platforms.Job {
	return platforms.Job{Name: "build", Image: image, Steps: steps}
}

func step(name, command string) platforms.Step {
	return platforms.Step{Name: name, Command: command}
}
