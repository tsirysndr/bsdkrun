// Package gleam builds Gleam projects. Ported from examples/unikraft-gleam.
package gleam

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/beam"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultVersion = "1.18.0"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "gleam" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "gleam.toml"))
	if err == nil {
		return true, nil
	}
	if os.IsNotExist(err) {
		return false, nil
	}
	return false, err
}

func (p *Provider) StartCommandHelp() string {
	return "Gleam boots its erlang-shipment through beam.smp, calling <name>@@main:run(<name>)."
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	name := projectName(filepath.Join(dir, "gleam.toml"))

	version := defaultVersion
	if v, ok := versions.Read(dir).Version("gleam"); ok {
		version = v
	}

	script := fmt.Sprintf(`set -eu
%sgleam deps download
gleam export erlang-shipment
mkdir -p /out/rootfs/srv
cp -a build/erlang-shipment/. /out/rootfs/srv/
%s`, plan.LddIntoRootfs, beam.ExtractERTS(beam.GleamApps))

	// ERL_LIBS replaces the `-pa "$BASE"/*/ebin` that Gleam's generated
	// entrypoint.sh expands to — 21 arguments there, one env var here.
	pl := beam.Plan(&plan.Plan{
		Name:       name,
		Provider:   p.Name(),
		BuildImage: "ghcr.io/gleam-lang/gleam:v" + version + "-erlang",
		Script:     script,
		Cmd: append(beam.Argv(),
			"-noshell", "-eval", name+"@@main:run("+name+")", "--"),
	})
	pl.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP8"] = `"ERL_LIBS=/srv"`
	pl.Kconfig["CONFIG_LIBPOSIX_ENVIRON_ENVP9"] = `"TMPDIR=/tmp"`
	return pl, nil
}

var nameRe = regexp.MustCompile(`(?m)^\s*name\s*=\s*"([^"]+)"`)

func projectName(gleamToml string) string {
	data, err := os.ReadFile(gleamToml)
	if err != nil {
		return "app"
	}
	if m := nameRe.FindSubmatch(data); m != nil {
		return string(m[1])
	}
	return "app"
}
