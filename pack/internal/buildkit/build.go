package buildkit

import (
	"context"
	"fmt"

	bkclient "github.com/moby/buildkit/client"
	"github.com/moby/buildkit/client/llb"
	specs "github.com/opencontainers/image-spec/specs-go/v1"
	"github.com/tonistiigi/fsutil"

	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
)

// Platform is the guest CPU architecture to build for — always a Linux
// image, since the rootfs runs under app-elfloader's Linux syscall shim
// regardless of what OS `pack` itself runs on.
type Platform string

const (
	PlatformArm64 Platform = "arm64"
	PlatformAmd64 Platform = "amd64"
)

func (p Platform) ociPlatform() specs.Platform {
	return specs.Platform{OS: "linux", Architecture: string(p)}
}

// HostPlatform maps a Go GOARCH to the Platform `pack` defaults to building
// for when none is given explicitly (`fc/arm64` and `fc/x86_64` are the only
// two Unikraft targets that matter here — see the Kraftfiles under
// examples/).
func HostPlatform(goarch string) (Platform, error) {
	switch goarch {
	case "arm64":
		return PlatformArm64, nil
	case "amd64":
		return PlatformAmd64, nil
	default:
		return "", fmt.Errorf("unsupported host architecture %q", goarch)
	}
}

// Build constructs the LLB graph for p — copy the local source into
// p.BuildImage, run p.Script, then export only what it left at /out/rootfs —
// and solves it, exporting the result directly to outDir (replacing the
// `docker buildx build` + `docker create` + `docker export` dance
// build.sh uses today with one BuildKit local-exporter call).
//
// onStatus (nil-safe) is called for every SolveStatus BuildKit emits as the
// build progresses — vertex started/completed, log lines, byte counters —
// so a caller (internal/tui) can render it live instead of waiting for
// Build to return.
func Build(ctx context.Context, addr, srcDir string, p *plan.Plan, platform Platform, outDir string, onStatus func(*bkclient.SolveStatus)) error {
	c, err := bkclient.New(ctx, addr)
	if err != nil {
		return fmt.Errorf("connecting to buildkitd at %s: %w", addr, err)
	}
	defer c.Close()

	ociPlatform := platform.ociPlatform()

	base := llb.Image(p.BuildImage, llb.Platform(ociPlatform))
	for k, v := range p.Env {
		base = base.AddEnv(k, v)
	}
	src := llb.Local("context", llb.WithCustomName("load "+srcDir))

	build := base.
		File(llb.Copy(src, "/", "/src", &llb.CopyInfo{CreateDestPath: true})).
		Dir("/src").
		Run(
			llb.Args([]string{"sh", "-c", p.Script}),
			llb.WithCustomName(fmt.Sprintf("build %s (%s)", p.Name, p.Provider)),
		).
		Root()

	// CopyDirContentsOnly: without it, Copy nests the source directory itself
	// under the destination (/out/rootfs/hello ends up at /rootfs/hello, not
	// /hello) — see llb.CopyInfo's doc for why.
	final := llb.Scratch().
		File(llb.Copy(build, "/out/rootfs", "/", &llb.CopyInfo{
			CreateDestPath:      true,
			CopyDirContentsOnly: true,
		}))

	def, err := final.Marshal(ctx, llb.Platform(ociPlatform))
	if err != nil {
		return fmt.Errorf("marshaling LLB graph: %w", err)
	}

	contextFS, err := fsutil.NewFS(srcDir)
	if err != nil {
		return fmt.Errorf("reading %s: %w", srcDir, err)
	}

	// Solve sends to statusChan and closes it when done; a nil onStatus still
	// needs the channel drained, or Solve would block writing to it.
	statusCh := make(chan *bkclient.SolveStatus)
	statusDone := make(chan struct{})
	go func() {
		defer close(statusDone)
		for s := range statusCh {
			if onStatus != nil {
				onStatus(s)
			}
		}
	}()

	_, err = c.Solve(ctx, def, bkclient.SolveOpt{
		LocalMounts: map[string]fsutil.FS{"context": contextFS},
		Exports: []bkclient.ExportEntry{
			{Type: bkclient.ExporterLocal, OutputDir: outDir},
		},
	}, statusCh)
	<-statusDone
	if err != nil {
		return fmt.Errorf("buildkit solve: %w", err)
	}
	return nil
}
