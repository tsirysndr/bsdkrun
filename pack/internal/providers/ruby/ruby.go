// Package ruby builds Ruby projects. Ported from examples/unikraft-ruby.
package ruby

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers/entry"
	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

const defaultVersion = "3.2"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "ruby" }

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range []string{"Gemfile", "config.ru", ".ruby-version"} {
		_, err := os.Stat(filepath.Join(dir, marker))
		if err == nil {
			return true, nil
		}
		if !os.IsNotExist(err) {
			return false, err
		}
	}
	// A single-file Ruby service need not carry any of those — this repo's
	// own examples/unikraft-ruby is just server.rb — so a conventional
	// entrypoint counts as a marker in its own right.
	_, ok := entry.Find(dir, scripts)
	return ok, nil
}

var scripts = []string{"server.rb", "app.rb", "main.rb", "config.ru"}

func (p *Provider) StartCommandHelp() string {
	return `Ruby runs server.rb (or config.ru's app). A Procfile "web:" line overrides it.`
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	main := entry.FindOr(dir, scripts, "server.rb")

	version := defaultVersion
	if v, ok := versions.Read(dir).Version("ruby"); ok {
		version = v
	}

	// The stdlib ships native extensions, so resolving only the `ruby`
	// binary's own libraries is not enough — every .so under
	// /usr/local/lib/ruby has to be walked too, or the first `require` of an
	// extension fails in the guest.
	return &plan.Plan{
		Name:       "ruby",
		Provider:   p.Name(),
		BuildImage: "ruby:" + version + "-bookworm",
		Script: fmt.Sprintf(`set -eu
%smkdir -p /out/rootfs/usr/bin /out/rootfs/src /out/rootfs/tmp /out/rootfs/usr/local/lib
cp /usr/local/bin/ruby /out/rootfs/usr/bin/ruby
cp -a /usr/local/lib/ruby /out/rootfs/usr/local/lib/ruby
{ ldd /usr/local/bin/ruby
  find /usr/local/lib/ruby -name '*.so' -exec ldd {} \; 2>/dev/null || true; } \
  | grep -oE '/[^ ()]+' \
  | sort -u \
  | while read -r lib; do
        mkdir -p "/out/rootfs$(dirname "$lib")"
        cp -L "$lib" "/out/rootfs$lib"
    done
if [ -f Gemfile ]; then
    bundle config set --local path /out/rootfs/src/vendor/bundle || true
    bundle install || true
fi
cp -a . /out/rootfs/src/ 2>/dev/null || true
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs),
		Cmd: []string{"/usr/bin/ruby", "/src/" + main},
	}, nil
}
