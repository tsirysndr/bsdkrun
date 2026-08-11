// Package crystal builds Crystal projects.
//
// Crystal compiles to a native binary, and on Alpine it links statically —
// which suits a unikernel exactly: no ELF interpreter, no shared libraries,
// nothing to resolve into the rootfs. The guest is one file.
package crystal

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

// amd64TextAddr keeps a statically linked binary clear of the Unikraft fc
// kernel, which occupies the 0x400000 an ET_EXEC is linked at by default.
// See providers/golang, where the same address is applied for the same
// reason, and examples/unikraft-go/README.md for the loader trace.
const amd64TextAddr = "0x40000000"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "crystal" }

func (p *Provider) Detect(dir string) (bool, error) {
	_, err := os.Stat(filepath.Join(dir, "shard.yml"))
	if err == nil {
		return true, nil
	}
	if !os.IsNotExist(err) {
		return false, err
	}
	return false, nil
}

func (p *Provider) StartCommandHelp() string {
	return `Crystal builds a static binary from shard.yml's first target (or src/<name>.cr).`
}

func (p *Provider) Plan(dir string, arch plan.Arch) (*plan.Plan, error) {
	name, main := target(dir)

	linkFlags := ""
	if arch == plan.ArchAmd64 {
		// -Ttext-segment moves the whole load address, not just .text; a
		// static binary has no interpreter to relocate it, so this is the
		// only chance to place it.
		linkFlags = fmt.Sprintf(` --link-flags "-Wl,-Ttext-segment=%s"`, amd64TextAddr)
	}

	return &plan.Plan{
		Name:     "crystal",
		Provider: p.Name(),
		// Alpine, because --static needs musl: a fully static glibc binary
		// is not something glibc supports for anything using NSS, and
		// Crystal's socket code does.
		BuildImage: "crystallang/crystal:latest-alpine",
		Script: fmt.Sprintf(`set -eu
# --without-development rather than --production: they mean the same thing
# for a build, but --production insists on a shard.lock and a project that
# has never been installed does not have one.
if [ -f shard.yml ]; then
    shards install --without-development --skip-postinstall
fi

crystal build --release --static --no-debug%s -o /tmp/%s %q

mkdir -p /out/rootfs/usr/bin /out/rootfs/tmp
cp /tmp/%s /out/rootfs/usr/bin/%s

# A static binary has no libraries to resolve, but say so out loud: if this
# ever prints an interpreter, the assumption behind this provider is wrong
# and the guest will fail to start with nothing in the log to explain it.
if ldd /out/rootfs/usr/bin/%s 2>/dev/null | grep -q '=>'; then
    echo "warning: the binary is dynamically linked; --static did not take" >&2
fi
chmod 1777 /out/rootfs/tmp
`, linkFlags, name, main, name, name, name),
		Cmd: []string{"/usr/bin/" + name},
	}, nil
}

// target picks the binary name and its entry source. shard.yml's `targets:`
// block is authoritative; failing that, the conventional src/<shard>.cr, and
// failing that whatever single .cr file sits in src/.
func target(dir string) (name, main string) {
	name = filepath.Base(dir)
	if n, m := shardTarget(filepath.Join(dir, "shard.yml")); n != "" {
		if m == "" {
			m = "src/" + n + ".cr"
		}
		return n, m
	}
	if n := shardName(filepath.Join(dir, "shard.yml")); n != "" {
		name = n
	}
	if _, err := os.Stat(filepath.Join(dir, "src", name+".cr")); err == nil {
		return name, "src/" + name + ".cr"
	}
	if matches, _ := filepath.Glob(filepath.Join(dir, "src", "*.cr")); len(matches) == 1 {
		return name, filepath.ToSlash(filepath.Join("src", filepath.Base(matches[0])))
	}
	return name, "src/" + name + ".cr"
}

// shardTarget reads the first entry of shard.yml's targets: block, which
// looks like:
//
//	targets:
//	  server:
//	    main: src/server.cr
//
// This is a deliberately small reader rather than a YAML dependency: the
// two facts wanted here are two lines at a known indentation, and a shard.yml
// that needs more than that is one the fallbacks below already handle.
func shardTarget(path string) (name, main string) {
	f, err := os.Open(path)
	if err != nil {
		return "", ""
	}
	defer f.Close()

	inTargets := false
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, "targets:") {
			inTargets = true
			continue
		}
		if !inTargets {
			continue
		}
		// A line at column zero ends the block.
		if line != "" && !strings.HasPrefix(line, " ") {
			break
		}
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		if key, value, ok := strings.Cut(trimmed, ":"); ok {
			if name == "" && strings.TrimSpace(value) == "" {
				name = strings.TrimSpace(key)
				continue
			}
			if name != "" && strings.TrimSpace(key) == "main" {
				return name, strings.TrimSpace(value)
			}
		}
	}
	return name, ""
}

// shardName reads the top-level `name:` key.
func shardName(path string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		key, value, ok := strings.Cut(scanner.Text(), ":")
		if ok && key == "name" {
			return strings.TrimSpace(value)
		}
	}
	return ""
}
