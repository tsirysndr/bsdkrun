// Package versions answers "which version of this runtime does the project
// want", from every source that can say so.
//
// Two sources, least specific first: mise (.tool-versions, mise.toml) and
// railpack.json's `packages`. The config file wins, being the more
// deliberate statement of the two.
package versions

import (
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/config"
	"github.com/tsirysndr/bsdkrun/pack/internal/mise"
)

// Tools maps a tool name to the pinned version.
type Tools map[string]string

// Read merges every version source for dir.
func Read(dir string) Tools {
	t := Tools{}
	for k, v := range mise.Read(dir) {
		t[k] = v
	}
	if c, err := config.Read(dir); err == nil && c != nil {
		for k, v := range c.Packages {
			t[k] = v
		}
	}
	return t
}

// Version returns the pinned version for tool, without a leading "v".
func (t Tools) Version(tool string) (string, bool) {
	v, ok := t[tool]
	if !ok || v == "" {
		return "", false
	}
	return strings.TrimPrefix(v, "v"), true
}

// Major returns just the major component — "22" from "22.1.0" — which is
// what image tags are usually keyed on.
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
