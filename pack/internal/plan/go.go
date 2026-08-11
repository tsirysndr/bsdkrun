package plan

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/tsirysndr/bsdkrun/pack/internal/detect"
)

// goPlan builds a Go project with CGO_ENABLED=0, which produces a fully
// static binary — unlike the Rust/actix plan, there is no `ldd` step at all,
// since there is nothing dynamic left to resolve.
func goPlan(dir string) (*Plan, error) {
	name, err := goModuleName(filepath.Join(dir, "go.mod"))
	if err != nil {
		return nil, err
	}
	return &Plan{
		Name:       name,
		Provider:   detect.Go,
		BuildImage: "golang:1.22-bookworm",
		Env: map[string]string{
			"PATH":   "/go/bin:/usr/local/go/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
			"GOPATH": "/go",
		},
		Script: fmt.Sprintf(`set -eu
mkdir -p /out/rootfs
CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /out/rootfs/%s .
`, name),
		Cmd: []string{"/" + name},
	}, nil
}

// goModuleName reads the last path element of go.mod's `module` directive,
// e.g. "module github.com/acme/widget" -> "widget" — the same name `go
// build` would give the binary with no `-o`, so it doubles as the rootfs
// binary's filename.
func goModuleName(goMod string) (string, error) {
	data, err := os.ReadFile(goMod)
	if err != nil {
		return "", fmt.Errorf("reading %s: %w", goMod, err)
	}
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if after, ok := strings.CutPrefix(line, "module "); ok {
			modPath := strings.TrimSpace(after)
			parts := strings.Split(modPath, "/")
			name := parts[len(parts)-1]
			if name == "" {
				return "", fmt.Errorf("%s: empty module path", goMod)
			}
			return name, nil
		}
	}
	return "", fmt.Errorf("%s has no `module` directive", goMod)
}
