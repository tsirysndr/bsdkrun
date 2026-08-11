// Package elixir builds Elixir projects. Ported from
// examples/unikraft-elixir.
package elixir

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"

	"github.com/tsirysndr/bsdkrun/pack/internal/mise"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/beam"
)

const defaultVersion = "1.16.2"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "elixir" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "mix.exs"))
	if err == nil {
		return true, nil
	}
	if os.IsNotExist(err) {
		return false, nil
	}
	return false, err
}

func (p *Provider) StartCommandHelp() string {
	return "Elixir boots the mix release directly through beam.smp; there is no shell to run its start script."
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	app, version := appAndVersion(filepath.Join(dir, "mix.exs"))

	elixirVersion := defaultVersion
	if v, ok := mise.Read(dir).Version("elixir"); ok {
		elixirVersion = v
	}

	// The release lands at /srv, then beam.go carves the runtime out of the
	// image's OTP install around it.
	script := fmt.Sprintf(`set -eu
%sexport MIX_ENV=prod HEX_HOME=/tmp/hex MIX_HOME=/tmp/mix
mix local.hex --force
mix local.rebar --force
mix deps.get
mix release --overwrite
mkdir -p /out/rootfs/srv
cp -a _build/prod/rel/%s/. /out/rootfs/srv/
%s`, plan.LddIntoRootfs, app, beam.ExtractERTS)

	cmd := append(beam.Argv(),
		"-noshell", "-mode", "embedded",
		"-config", "/srv/releases/"+version+"/sys",
		"-boot", "/srv/releases/"+version+"/start",
		"-boot_var", "RELEASE_LIB", "/srv/lib",
		// Absorbs the words libkrun appends past the cmdline's `--` stop,
		// which would otherwise reach the VM as arguments.
		"--", "-extra",
	)

	return beam.Plan(&plan.Plan{
		Name:       app,
		Provider:   p.Name(),
		BuildImage: "elixir:" + elixirVersion + "-slim",
		Script:     script,
		Cmd:        cmd,
	}), nil
}

var (
	appRe = regexp.MustCompile(`app:\s*:([A-Za-z0-9_]+)`)
	verRe = regexp.MustCompile(`version:\s*"([^"]+)"`)
)

// appAndVersion reads mix.exs's `app:` and `version:`. Both end up in paths
// the boot argv names (/srv/releases/<version>/start), so they have to be
// known before the build runs, not discovered after it.
func appAndVersion(mixExs string) (string, string) {
	app, version := "server", "0.1.0"
	data, err := os.ReadFile(mixExs)
	if err != nil {
		return app, version
	}
	if m := appRe.FindSubmatch(data); m != nil {
		app = string(m[1])
	}
	if m := verRe.FindSubmatch(data); m != nil {
		version = string(m[1])
	}
	return app, version
}
