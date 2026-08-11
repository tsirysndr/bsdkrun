// Package plan holds the build plan a provider produces: what image to build
// in, the script that builds the project and assembles its rootfs, and the
// Kraftfile knobs the result needs to boot.
//
// Provider-specific logic lives in internal/providers, not here — this
// package is the data structure they all fill in, which is what keeps it
// importable from both sides without a cycle.
package plan

// Arch is the guest architecture the plan targets, in Docker/OCI spelling
// ("amd64"/"arm64"). A plain string rather than buildkit.Platform because
// internal/buildkit imports this package, not the other way round.
type Arch string

const (
	ArchAmd64 Arch = "amd64"
	ArchArm64 Arch = "arm64"
)

// Plan is everything internal/buildkit needs to build a project's rootfs and
// internal/kraftfile needs to generate its Kraftfile.
type Plan struct {
	// Name is the app name: the Kraftfile's `name:`, and what the built
	// kernel is called (`<name>_fc-<arch>`).
	Name string

	// Provider is the name of the provider that produced this plan, for
	// display only.
	Provider string

	// BuildImage is the build stage's base image, e.g.
	// "golang:1.22-bookworm". Always a Linux image — the rootfs runs under
	// app-elfloader's Linux syscall shim regardless of the host running
	// `pack`.
	BuildImage string

	// Env is set on the build stage before Script runs. Needed because
	// `llb.Image` only pulls in the image's *filesystem*, not its config
	// (ENV/WORKDIR/etc — that's Dockerfile-frontend behavior, not something
	// building the LLB graph directly gets for free).
	Env map[string]string

	// BuilderImage and BuilderScript are an optional first stage, for
	// providers whose build tool and runtime want different images. The
	// script runs in BuilderImage and leaves its artifacts in /out/stage;
	// they appear at /stage in the second stage.
	//
	// Clojure needs this: the example builds its uberjar in the official
	// clojure image (a pinned tools-deps CLI) but jlinks the JRE in a
	// pristine eclipse-temurin JDK, and the JRE jlink emits is what the
	// guest executes. Collapsing the two into one image changes both the
	// CLI producing the jar and the JDK producing the runtime.
	BuilderImage  string
	BuilderScript string

	// Script is run as `sh -c <Script>` in BuildImage, with the project
	// source copied in at /src (cwd). It must leave the finished rootfs at
	// /out/rootfs — internal/buildkit copies that directory, and only that
	// directory, into the final scratch image.
	Script string

	// Tools are files copied into the build stage from other images,
	// before the script runs. That is how a build gets a tool its base
	// image does not ship — PHP's composer, say — without fetching an
	// installer over the network mid-build: the copy is a normal BuildKit
	// input, so it is pinned by the image ref and cached like any layer.
	Tools []ToolCopy

	// Cmd is the Kraftfile `cmd:` — the guest's argv.
	Cmd []string

	// Kconfig is merged into the Kraftfile's `unikraft:` kconfig block,
	// e.g. Bun's BUN_JSC_useConcurrentGC=0 environment entry.
	Kconfig map[string]string

	// ElfloaderKconfig overrides entries in the `app-elfloader:` library's
	// kconfig block, e.g. Bun needs CONFIG_APPELFLOADER_STACK_NBPAGES at
	// 2048 rather than the default 128.
	ElfloaderKconfig map[string]string
}

// ToolCopy is one file lifted out of Image at Src and placed at Dst in the
// build stage.
type ToolCopy struct {
	Image string
	Src   string
	Dst   string
}

// LddIntoRootfs is the shell fragment that copies a dynamically linked
// binary's shared libraries into the rootfs alongside it.
//
// Every interpreted-language provider needs this and every one of this
// repo's hand-written Dockerfiles arrived at the same answer: *resolve* the
// libraries with ldd rather than listing them, because the paths differ by
// architecture (`/lib/aarch64-linux-gnu/...` vs `/lib/x86_64-linux-gnu/...`,
// and `ld-linux-aarch64.so.1` vs `ld-linux-x86-64.so.2`) — a different
// directory *and* a different filename, so no hardcoded list survives a
// change of target.
const LddIntoRootfs = `ldd_into_rootfs() {
    ldd "$1" \
      | grep -oE '/[^ ()]+' \
      | sort -u \
      | while read -r lib; do
            mkdir -p "/out/rootfs$(dirname "$lib")"
            cp -L "$lib" "/out/rootfs$lib"
        done
}
`
