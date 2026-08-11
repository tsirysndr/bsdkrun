// Package swift builds Swift Package Manager projects.
package swift

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

const defaultVersion = "5.10"

type Provider struct{}

func New() *Provider { return &Provider{} }

func (p *Provider) Name() string { return "swift" }

func (p *Provider) Detect(dir string) (bool, error) {
	for _, marker := range []string{"Package.swift", ".swift-version"} {
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
	return `Swift builds the package's executable target. .swift-version or Package.swift's swift-tools-version picks the toolchain.`
}

func (p *Provider) Plan(dir string, _ plan.Arch) (*plan.Plan, error) {
	version := Version(dir)
	name := executableName(dir)

	return &plan.Plan{
		Name:     "swift",
		Provider: p.Name(),
		// jammy rather than the default (Amazon Linux) tag: the runtime
		// libraries this copies out come from the same glibc the guest
		// resolves against.
		BuildImage: "swift:" + version + "-jammy",
		Script: fmt.Sprintf(`set -eu
%sswift build -c release --static-swift-stdlib

binary=$(swift build -c release --show-bin-path)/%s
if [ ! -f "$binary" ]; then
    binary=$(ls -S $(swift build -c release --show-bin-path)/* 2>/dev/null \
             | while read -r f; do [ -x "$f" ] && [ ! -d "$f" ] && echo "$f"; done | head -1)
fi
if [ -z "$binary" ] || [ ! -f "$binary" ]; then
    echo "no executable produced -- does Package.swift declare an executableTarget?" >&2
    exit 1
fi

mkdir -p /out/rootfs/usr/bin /out/rootfs/tmp
cp "$binary" /out/rootfs/usr/bin/%s

# --static-swift-stdlib links the Swift runtime in, but not libc and not
# the ICU that Foundation reaches for, so what remains still has to be
# resolved.
ldd_into_rootfs /out/rootfs/usr/bin/%s
chmod 1777 /out/rootfs/tmp
`, plan.LddIntoRootfs, name, name, name),
		Cmd: []string{"/usr/bin/" + name},
	}, nil
}

// Version resolves the toolchain: an explicit .swift-version wins, then the
// swift-tools-version declared at the top of Package.swift, then a default.
//
// swift-tools-version is a statement about the manifest format rather than
// the compiler, so it is a weaker signal — but a package declaring 5.9 is
// not going to build with a 5.5 toolchain, and it is the only version a
// package is obliged to carry.
func Version(dir string) string {
	if v := readSwiftVersion(filepath.Join(dir, ".swift-version")); v != "" {
		return v
	}
	if v := readToolsVersion(filepath.Join(dir, "Package.swift")); v != "" {
		return v
	}
	return defaultVersion
}

func readSwiftVersion(path string) string {
	body, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(body))
}

// readToolsVersion reads the `// swift-tools-version:5.9` comment, which
// must be the first line of a Package.swift. Both `:` and `: ` spellings
// appear in the wild.
func readToolsVersion(path string) string {
	f, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if !strings.HasPrefix(line, "//") {
			continue
		}
		_, value, ok := strings.Cut(line, "swift-tools-version")
		if !ok {
			continue
		}
		value = strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(value), ":"))
		if value != "" {
			return value
		}
	}
	return ""
}

// executableName is the product the guest runs. Package.swift's first
// .executable product names it; failing that, the directory does.
func executableName(dir string) string {
	if name := readExecutableProduct(filepath.Join(dir, "Package.swift")); name != "" {
		return name
	}
	return filepath.Base(dir)
}

// readExecutableProduct finds the executable's name in a Package.swift,
// from either a `.executable(name:` product or a `.executableTarget(name:`.
// A package that declares no products: section at all — the common shape for
// a single-executable package — has only the latter.
//
// Anything more thorough would mean running swift itself, which is what the
// build does anyway: this only has to be right often enough to name the
// binary, and the build falls back to the largest executable it finds.
func readExecutableProduct(path string) string {
	body, err := os.ReadFile(path)
	if err != nil {
		return ""
	}
	after, ok := "", false
	for _, decl := range []string{".executable(", ".executableTarget("} {
		if _, after, ok = strings.Cut(string(body), decl); ok {
			break
		}
	}
	if !ok {
		return ""
	}
	_, after, ok = strings.Cut(after, `name:`)
	if !ok {
		return ""
	}
	_, after, ok = strings.Cut(after, `"`)
	if !ok {
		return ""
	}
	name, _, ok := strings.Cut(after, `"`)
	if !ok {
		return ""
	}
	return name
}
