// Package detect identifies what kind of project lives in a directory, the
// same first step railpack takes: everything downstream (the build plan, the
// Kraftfile kconfig deltas) is keyed off this.
package detect

import (
	"fmt"
	"os"
	"path/filepath"
)

// Provider is a language/runtime pack knows how to build. Only Go and Rust
// exist yet — see the project plan for why (static/near-static binaries, no
// interpreter runtime, cheapest way to prove the pipeline end to end).
// Node/Bun/Deno/Ruby/Elixir are future work, added the same way: a new
// case here plus a new plan builder.
type Provider string

const (
	Go   Provider = "go"
	Rust Provider = "rust"
)

// Detection is what Detect found: which provider matched, and the project
// root it matched in (== dir, kept alongside the provider so callers don't
// have to thread both separately).
type Detection struct {
	Provider Provider
	Dir      string
}

// Detect inspects dir for a recognized project and reports which provider
// claims it. Order matters only in that it's deterministic — a project with
// both a go.mod and a Cargo.toml (unlikely) picks Go.
func Detect(dir string) (*Detection, error) {
	if exists(filepath.Join(dir, "go.mod")) {
		return &Detection{Provider: Go, Dir: dir}, nil
	}
	if exists(filepath.Join(dir, "Cargo.toml")) {
		return &Detection{Provider: Rust, Dir: dir}, nil
	}
	return nil, fmt.Errorf(
		"no supported project found in %s (looked for go.mod, Cargo.toml)", dir,
	)
}

func exists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}
