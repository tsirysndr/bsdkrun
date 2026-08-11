// Package plan turns a detect.Detection into a concrete build: which image
// to build in, and the shell script that builds the project and assembles
// its rootfs at /out/rootfs inside that image.
//
// Each provider's script is a direct port of this repo's existing, proven
// Dockerfiles (examples/unikraft-actix, examples/unikraft-expressjs) — same
// commands, same `ldd`-resolution loop for dynamically linked binaries —
// just run as one LLB step instead of parsed from Dockerfile text.
package plan

import (
	"fmt"

	"github.com/tsirysndr/bsdkrun/pack/internal/detect"
)

// Plan is everything internal/buildkit needs to build a project's rootfs,
// and (from Phase 2 on) internal/kraftfile needs to generate its Kraftfile.
type Plan struct {
	// Name is the app name: the Kraftfile's `name:`, and the binary's path
	// inside the rootfs (`/<Name>`).
	Name string

	Provider detect.Provider

	// BuildImage is the build stage's base image, e.g. "golang:1.22-bookworm".
	// Always a Linux image — the rootfs runs under app-elfloader's Linux
	// syscall shim regardless of the host running `pack`.
	BuildImage string

	// Env is set on the build stage before Script runs. Needed because
	// `llb.Image` only pulls in the image's *filesystem*, not its config
	// (ENV/WORKDIR/etc — that's Dockerfile-frontend behavior, not something
	// building the LLB graph directly gets for free) — so PATH defaults to a
	// bare Linux one that doesn't include e.g. golang's /usr/local/go/bin.
	// Values are this repo's knowledge of what each pinned BuildImage tag's
	// own Dockerfile sets, same category of fact as knowing where `cargo
	// build --release` writes its output.
	Env map[string]string

	// Script is run as `sh -c <Script>` in BuildImage, with the project
	// source copied in at /src (cwd). It must leave the finished rootfs at
	// /out/rootfs — internal/buildkit copies that directory, and only that
	// directory, into the final scratch image.
	Script string

	// Cmd is the Kraftfile `cmd:` — the guest's argv.
	Cmd []string

	// KconfigExtra is appended to the Kraftfile's shared base kconfig block —
	// e.g. a future Node provider would set CONFIG_LIBPOSIX_PROCESS_SIGNAL
	// for OpenSSL's arm64 SIGILL probe here (see
	// examples/unikraft-expressjs/Kraftfile). Empty for Go and Rust: neither
	// needs anything past the base block (matches
	// examples/unikraft-actix/Kraftfile, which adds nothing but `cmd:`).
	KconfigExtra map[string]string
}

// Arch is the guest architecture the plan targets, in Docker/OCI spelling
// ("amd64"/"arm64"). A plain string rather than buildkit.Platform because
// internal/buildkit imports this package, not the other way round.
type Arch string

const (
	ArchAmd64 Arch = "amd64"
	ArchArm64 Arch = "arm64"
)

// Build dispatches to the provider-specific plan builder.
func Build(d *detect.Detection, arch Arch) (*Plan, error) {
	switch d.Provider {
	case detect.Go:
		return goPlan(d.Dir, arch)
	case detect.Rust:
		return rustPlan(d.Dir)
	default:
		return nil, fmt.Errorf("no plan builder for provider %q", d.Provider)
	}
}
