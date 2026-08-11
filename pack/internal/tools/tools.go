// Package tools installs the extra tools a project asks for with mise.
//
// railpack.json's `packages` (and mise's own .tool-versions / mise.toml) map
// a tool name to a version. Some of those name the project's own runtime —
// "python": "3.12" for a Python project — and the provider has already
// consumed those to pick its base image or its interpreter. What is left is
// what this installs: the build-time tools a project needs that its language
// image does not ship, like jq, protoc, or a second language's compiler.
//
// mise does the installing because it is the same thing the version pins are
// already expressed in, so a project states a tool and its version once.
package tools

import (
	"fmt"
	"sort"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/versions"
)

// consumed maps a provider to the tool names it already resolves itself.
// Installing those again would at best duplicate the provider's own work and
// at worst put a second, differently-built runtime ahead of it on PATH.
var consumed = map[string][]string{
	"bun":     {"bun"},
	"clojure": {"java", "clojure"},
	"crystal": {"crystal"},
	"deno":    {"deno"},
	"dotnet":  {"dotnet"},
	"elixir":  {"elixir", "erlang"},
	"gleam":   {"gleam", "erlang", "rebar"},
	"go":      {"go", "golang"},
	"haskell": {"ghc", "stack", "haskell"},
	"java":    {"java", "maven", "gradle"},
	"node":    {"node"},
	"php":     {"php"},
	"python":  {"python"},
	"ruby":    {"ruby"},
	"rust":    {"rust"},
	"scala":   {"java", "scala", "sbt"},
	"static":  {},
	"swift":   {"swift"},
	"zig":     {"zig"},
}

// Extra returns the tools to install for dir, given the provider that
// claimed it, as an ordered list of "tool@version" strings.
//
// Sorted, so the generated script is stable: an unordered map would change
// the build script on every run and defeat BuildKit's cache.
func Extra(dir, provider string) []string {
	skip := map[string]bool{}
	for _, name := range consumed[provider] {
		skip[name] = true
	}

	var out []string
	for name, version := range versions.Read(dir) {
		if skip[name] || name == "" || version == "" {
			continue
		}
		out = append(out, name+"@"+strings.TrimPrefix(version, "v"))
	}
	sort.Strings(out)
	return out
}

// Script is the shell that installs tools and puts them on PATH, to be
// prepended to a provider's build script.
//
// Written to work on both Debian and Alpine bases, because providers use
// both, and to be a no-op when mise is already present — the python and zig
// providers install it themselves, and installing it twice would undo the
// version they chose.
func Script(tools []string) string {
	if len(tools) == 0 {
		return ""
	}

	var b strings.Builder
	b.WriteString(`# Extra tools, from railpack.json's packages or a mise config.
export MISE_DATA_DIR="${MISE_DATA_DIR:-/opt/mise}"
export MISE_YES=1
if ! command -v mise >/dev/null 2>&1; then
    if command -v apt-get >/dev/null 2>&1; then
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends ca-certificates curl git >/dev/null
    else
        apk add --no-cache ca-certificates curl git >/dev/null
    fi
    curl -fsSL https://mise.run | sh
    export PATH="/root/.local/bin:$PATH"
fi
`)

	for _, t := range tools {
		// Each tool goes on PATH as it is installed, so one tool can be
		// used to build the next.
		b.WriteString(fmt.Sprintf("mise install %q\n", t))
		b.WriteString(fmt.Sprintf("export PATH=\"$(mise where %q)/bin:$PATH\"\n", t))
	}
	b.WriteString("\n")
	return b.String()
}
