// Package haskell builds Stack projects.
package haskell

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

// resolver is the Stackage snapshot used when the project names none. Any
// project worth building carries its own stack.yaml; this exists so that a
// package.yaml on its own still builds.
const resolver = "lts-22.28"

// amd64TextAddr keeps the binary clear of the Unikraft fc kernel. GHC emits
// a position-dependent executable on x86_64, linked at the 0x400000 the
// kernel occupies — see providers/golang, which moves Go binaries for the
// same reason, and examples/unikraft-go/README.md for the loader trace.
const amd64TextAddr = "0x40000000"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "haskell" }

// Detect requires both a package.yaml and Haskell source. package.yaml is
// not exclusively Stack's — it is an ordinary name for an ordinary config
// file — so on its own it would claim projects that have nothing to do with
// Haskell.
func (p *Provider) Detect(dir string) (bool, error) {
	if _, err := os.Stat(filepath.Join(dir, "package.yaml")); err != nil {
		if !os.IsNotExist(err) {
			return false, err
		}
		return false, nil
	}
	return hasSource(dir), nil
}

// hasSource looks for a .hs file in the root and in the conventional source
// directories, rather than walking the whole tree: a deep walk of a project
// with a .stack-work/ in it is slow, and finds Haskell belonging to
// dependencies rather than to this project.
func hasSource(dir string) bool {
	for _, where := range []string{".", "src", "app", "lib", "exe"} {
		matches, err := filepath.Glob(filepath.Join(dir, where, "*.hs"))
		if err == nil && len(matches) > 0 {
			return true
		}
	}
	return false
}

func (p *Provider) StartCommandHelp() string {
	return `Haskell builds with Stack; the executable is package.yaml's first "executables:" entry.`
}

func (p *Provider) Plan(dir string, arch plan.Arch) (*plan.Plan, error) {
	name := executableName(dir)

	ghcOptions := ""
	if arch == plan.ArchAmd64 {
		ghcOptions = fmt.Sprintf(` --ghc-options "-optl-Wl,-Ttext-segment=%s"`, amd64TextAddr)
	}

	// --system-ghc --no-install-ghc: the haskell image already ships a GHC,
	// and without these Stack ignores it and downloads its own — several
	// hundred megabytes, to arrive at the same compiler.
	//
	// A stack.yaml is written only if the project has none. Overwriting one
	// would discard the project's own resolver and extra-deps, which is the
	// whole of its build configuration.
	return &plan.Plan{
		Name:       "haskell",
		Provider:   p.Name(),
		BuildImage: "haskell:9.6",
		Env: map[string]string{
			"STACK_ROOT": "/tmp/stack",
		},
		// compiler-check: newer-minor, because a Stackage snapshot names an
		// exact GHC and the image ships whatever its tag last built with —
		// lts-22.28 wants 9.6.6 and haskell:9.6 carries 9.6.7, which without
		// this is a hard "No compiler found" rather than a warning.
		//
		// No apt-get here: the image already carries the certificates Stack
		// needs to reach Hackage. That matters more than it looks — 9.4 is
		// Debian buster, whose repositories are archived, so `apt-get
		// update` there fails outright and took the build with it.
		Script: fmt.Sprintf(`set -eu
%sif [ ! -f stack.yaml ]; then
    printf 'resolver: %s\ncompiler-check: newer-minor\n' > stack.yaml
fi

stack build --system-ghc --no-install-ghc%s --copy-bins --local-bin-path /tmp/bin

binary=/tmp/bin/%s
if [ ! -f "$binary" ]; then
    binary=$(ls -S /tmp/bin/* 2>/dev/null | head -1)
fi
if [ -z "$binary" ] || [ ! -f "$binary" ]; then
    echo "stack produced no executable -- does package.yaml declare one under executables:?" >&2
    exit 1
fi

mkdir -p /out/rootfs/usr/bin /out/rootfs/tmp
cp "$binary" /out/rootfs/usr/bin/%s

# GHC links against libgmp, libffi and libc dynamically, so the binary is
# not self-contained the way a Go or Zig one is.
ldd_into_rootfs /out/rootfs/usr/bin/%s
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs, resolver, ghcOptions, name, name, name),
		Cmd: []string{"/usr/bin/" + name},
	}, nil
}

// executableName is package.yaml's first executables: entry, which is what
// Stack installs into the bin path. Failing that, the package's own name.
func executableName(dir string) string {
	path := filepath.Join(dir, "package.yaml")
	if name := firstExecutable(path); name != "" {
		return name
	}
	if name := packageName(path); name != "" {
		return name
	}
	return filepath.Base(dir)
}

// firstExecutable reads the first key under `executables:`:
//
//	executables:
//	  server:
//	    main: Main.hs
//
// A small reader rather than a YAML dependency: the one fact wanted here is
// a key at a known indentation, and the fallbacks cover anything else.
func firstExecutable(path string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()

	inBlock := false
	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := scanner.Text()
		if strings.HasPrefix(line, "executables:") {
			inBlock = true
			continue
		}
		if !inBlock {
			continue
		}
		// A line at column zero ends the block.
		if line != "" && !strings.HasPrefix(line, " ") {
			return ""
		}
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		if key, _, ok := strings.Cut(trimmed, ":"); ok {
			return strings.TrimSpace(key)
		}
	}
	return ""
}

// packageName reads the top-level `name:` key.
func packageName(path string) string {
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
