// Package mise reads the runtime versions a project pins, so a provider can
// build against the version the project asks for rather than whatever the
// provider defaults to.
//
// Both formats mise itself understands are supported:
//
//	mise.toml / .mise.toml   [tools]  node = "22"
//	.tool-versions           node 22.1.0        (asdf's format, which mise reads)
//
// Parsed by hand rather than with a TOML library: the [tools] table is
// flat key = "value" pairs, and pack has 7 dependencies total — a whole TOML
// parser to read one table is a poor trade for a binary embedded in every
// `bsdkrun`.
package mise

import (
	"os"
	"path/filepath"
	"strings"
)

// Tools maps a tool name ("node") to the version the project pins ("22").
type Tools map[string]string

// Version returns the pinned version for tool, and whether one was pinned.
// A leading "v" is trimmed, so `node = "v22"` and `node = "22"` agree.
func (t Tools) Version(tool string) (string, bool) {
	v, ok := t[tool]
	if !ok || v == "" {
		return "", false
	}
	return strings.TrimPrefix(v, "v"), true
}

// Major returns just the major component of a pinned version — "22" from
// "22.1.0" — which is what image tags are usually keyed on.
func (t Tools) Major(tool string) (string, bool) {
	v, ok := t.Version(tool)
	if !ok {
		return "", false
	}
	if i := strings.IndexByte(v, '.'); i > 0 {
		return v[:i], true
	}
	return v, true
}

// Read collects the tool versions pinned in dir. A missing file is not an
// error — most projects pin nothing.
func Read(dir string) Tools {
	tools := Tools{}
	// .tool-versions first so an explicit mise.toml wins over it.
	readToolVersions(filepath.Join(dir, ".tool-versions"), tools)
	for _, name := range []string{".mise.toml", "mise.toml"} {
		readMiseToml(filepath.Join(dir, name), tools)
	}
	return tools
}

// readToolVersions parses asdf's format: one `<tool> <version>` per line,
// `#` comments. A tool may list several fallback versions; the first wins,
// as it does in asdf and mise.
func readToolVersions(path string, into Tools) {
	data, err := os.ReadFile(path)
	if err != nil {
		return
	}
	for _, line := range strings.Split(string(data), "\n") {
		if i := strings.IndexByte(line, '#'); i >= 0 {
			line = line[:i]
		}
		fields := strings.Fields(line)
		if len(fields) >= 2 {
			into[fields[0]] = fields[1]
		}
	}
}

// readMiseToml parses the `[tools]` table only. Other tables (env, tasks)
// carry nothing that changes how a project is built into a unikernel.
func readMiseToml(path string, into Tools) {
	data, err := os.ReadFile(path)
	if err != nil {
		return
	}
	inTools := false
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if i := strings.IndexByte(line, '#'); i >= 0 {
			line = strings.TrimSpace(line[:i])
		}
		if strings.HasPrefix(line, "[") {
			inTools = line == "[tools]"
			continue
		}
		if !inTools {
			continue
		}
		key, val, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		key = strings.TrimSpace(key)
		val = strings.Trim(strings.TrimSpace(val), `"'`)
		// `node = { version = "22" }` — take the version field rather than
		// storing the whole inline table.
		if strings.HasPrefix(val, "{") {
			if _, v, ok := strings.Cut(val, "version"); ok {
				v = strings.TrimLeft(v, " =")
				if end := strings.IndexAny(v, `,}`); end >= 0 {
					v = v[:end]
				}
				val = strings.Trim(strings.TrimSpace(v), `"'`)
			} else {
				continue
			}
		}
		if key != "" && val != "" {
			into[key] = val
		}
	}
}
