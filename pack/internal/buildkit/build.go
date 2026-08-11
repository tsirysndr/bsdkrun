package buildkit

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"strings"

	"github.com/distribution/reference"
	bkclient "github.com/moby/buildkit/client"
	"github.com/moby/buildkit/client/llb"
	"github.com/moby/buildkit/client/llb/sourceresolver"
	gwclient "github.com/moby/buildkit/frontend/gateway/client"
	"github.com/moby/buildkit/session"
	"github.com/moby/buildkit/session/secrets/secretsprovider"
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
func Build(ctx context.Context, addr, srcDir string, p *plan.Plan, platform Platform, outDir string, excludes []string, cacheDir string, onStatus func(*bkclient.SolveStatus)) error {
	c, err := bkclient.New(ctx, addr)
	if err != nil {
		return fmt.Errorf("connecting to buildkitd at %s: %w", addr, err)
	}
	defer c.Close()

	ociPlatform := platform.ociPlatform()
	// Excludes matter here in a way they would not for a normal build: pack
	// writes .unikraft/ and .rootfs-<arch>/ into the project, so without
	// them every rebuild uploads the previous build back as context.
	src := llb.Local("context",
		llb.ExcludePatterns(excludes),
		llb.WithCustomName("load "+srcDir))

	contextFS, err := fsutil.NewFS(srcDir)
	if err != nil {
		return fmt.Errorf("reading %s: %w", srcDir, err)
	}
	// Filter the mount as well as the LLB op: ExcludePatterns alone keeps
	// the files out of the *build*, but the client would still walk and
	// transfer them.
	contextFS, err = fsutil.NewFilterFS(contextFS, &fsutil.FilterOpt{
		ExcludePatterns: excludes,
	})
	if err != nil {
		return fmt.Errorf("applying excludes to %s: %w", srcDir, err)
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

	// c.Build rather than c.Solve: only a gateway client can answer
	// ResolveImageConfig, and without the image's own config the build runs
	// with none of the ENV the image sets. That is not a detail — it cost
	// four separate CI failures before this existed (PATH/GOPATH for Go,
	// CARGO_HOME for Rust, PHP_INI_DIR for PHP, JAVA_HOME for Clojure),
	// each surfacing as an unrelated-looking error deep in a build script.
	buildFn := func(ctx context.Context, gw gwclient.Client) (*gwclient.Result, error) {
		base := llb.Image(p.BuildImage, llb.Platform(ociPlatform))

		// Inherit the image's ENV, then let the provider override it: the
		// image knows where its own toolchain lives, the provider knows
		// what its build needs.
		env, err := imageEnv(ctx, gw, p.BuildImage, ociPlatform)
		if err != nil {
			return nil, err
		}
		for k, v := range env {
			base = base.AddEnv(k, v)
		}
		for k, v := range p.Env {
			base = base.AddEnv(k, v)
		}

		build := base.
			File(llb.Copy(src, "/", "/src", &llb.CopyInfo{CreateDestPath: true}))

		// Tools the base image does not ship, taken from images that do.
		for _, t := range p.Tools {
			build = build.File(llb.Copy(
				llb.Image(t.Image, llb.Platform(ociPlatform)), t.Src, t.Dst,
				&llb.CopyInfo{CreateDestPath: true},
			))
		}

		// Optional first stage, in its own image; its /out/stage lands at
		// /stage here.
		if p.BuilderImage != "" {
			builder := llb.Image(p.BuilderImage, llb.Platform(ociPlatform))
			benv, err := imageEnv(ctx, gw, p.BuilderImage, ociPlatform)
			if err != nil {
				return nil, err
			}
			for k, v := range benv {
				builder = builder.AddEnv(k, v)
			}
			for k, v := range p.Env {
				builder = builder.AddEnv(k, v)
			}
			builderOpts := []llb.RunOption{
				llb.Args([]string{"sh", "-c", p.BuilderScript}),
				llb.WithCustomName(fmt.Sprintf("build %s (%s, stage 1)", p.Name, p.Provider)),
			}
			builderOpts = append(builderOpts, secretMounts(p.Secrets)...)

			staged := builder.
				File(llb.Copy(src, "/", "/src", &llb.CopyInfo{CreateDestPath: true})).
				Dir("/src").
				Run(builderOpts...).
				Root()
			build = build.File(llb.Copy(staged, "/out/stage", "/stage", &llb.CopyInfo{
				CreateDestPath:      true,
				CopyDirContentsOnly: true,
			}))
		}

		runOpts := []llb.RunOption{
			llb.Args([]string{"sh", "-c", p.Script}),
			llb.WithCustomName(fmt.Sprintf("build %s (%s)", p.Name, p.Provider)),
		}
		runOpts = append(runOpts, secretMounts(p.Secrets)...)

		build = build.
			Dir("/src").
			Run(runOpts...).
			Root()

		// CopyDirContentsOnly: without it, Copy nests the source directory
		// itself under the destination.
		final := llb.Scratch().
			File(llb.Copy(build, "/out/rootfs", "/", &llb.CopyInfo{
				CreateDestPath:      true,
				CopyDirContentsOnly: true,
			}))

		def, err := final.Marshal(ctx, llb.Platform(ociPlatform))
		if err != nil {
			return nil, fmt.Errorf("marshaling LLB graph: %w", err)
		}
		return gw.Solve(ctx, gwclient.SolveRequest{Definition: def.ToPB()})
	}

	solveOpt := bkclient.SolveOpt{
		LocalMounts: map[string]fsutil.FS{"context": contextFS},
		// Non-nil deliberately. buildkit's solve() does
		// `maps.Copy(maps.Clone(opt.FrontendAttrs), cacheOpt.frontendAttrs)`,
		// and maps.Clone(nil) is nil — so as soon as a cache import supplies
		// attributes, it panics writing into a nil map. Only reachable with
		// CacheImports set, which is why it surfaced the moment the build
		// cache was wired up in CI and never locally.
		FrontendAttrs: map[string]string{},
		Exports: []bkclient.ExportEntry{
			{Type: bkclient.ExporterLocal, OutputDir: outDir},
		},
		Session: secretSession(p.Secrets),
	}
	// A local cache directory, when asked for, makes the build survive the
	// daemon: buildkitd's own cache lives inside its container and is lost
	// whenever that container is (every CI job, for instance). Importing is
	// conditional on the directory existing, since BuildKit treats a missing
	// import source as an error rather than a cold cache.
	if cacheDir != "" {
		if _, err := os.Stat(cacheDir); err == nil {
			solveOpt.CacheImports = []bkclient.CacheOptionsEntry{
				{Type: "local", Attrs: map[string]string{"src": cacheDir}},
			}
		}
		if err := os.MkdirAll(cacheDir, 0o755); err != nil {
			return fmt.Errorf("creating build cache dir %s: %w", cacheDir, err)
		}
		solveOpt.CacheExports = []bkclient.CacheOptionsEntry{
			{Type: "local", Attrs: map[string]string{"dest": cacheDir, "mode": "max"}},
		}
	}

	_, err = c.Build(ctx, solveOpt, "bsdkrun-pack", buildFn, statusCh)
	<-statusDone
	if err != nil {
		return fmt.Errorf("buildkit solve: %w", err)
	}
	return nil
}

// secretSession supplies the values for the secrets the build mounts.
//
// Each is read from the environment variable of the same name, which is how
// the same secret reaches a local run and a CI job without a second
// mechanism: `export NPM_TOKEN=...` locally, a repository secret exported
// in the workflow. A name with nothing behind it is left out rather than
// passed as empty, so the mount is simply absent and the build script's own
// check for it does the right thing.
func secretSession(names []string) []session.Attachable {
	values := map[string][]byte{}
	for _, name := range names {
		if v, ok := os.LookupEnv(name); ok {
			values[name] = []byte(v)
		}
	}
	if len(values) == 0 {
		return nil
	}
	return []session.Attachable{secretsprovider.FromMap(values)}
}

// secretMounts mounts each named secret at /run/secrets/<name>.
//
// A secret mount is not a layer: the file exists only while the command
// runs, so a token used to fetch a private dependency does not end up
// readable in the image afterwards. That matters more here than in an
// ordinary container build, because the result is a unikernel that gets
// pushed to a registry whole.
func secretMounts(names []string) []llb.RunOption {
	opts := make([]llb.RunOption, 0, len(names))
	for _, name := range names {
		opts = append(opts, llb.AddSecret("/run/secrets/"+name,
			llb.SecretID(name), llb.SecretOptional))
	}
	return opts
}

// imageEnv reads the ENV a base image declares in its own config.
//
// llb.Image pulls in an image's filesystem but not its config, so anything
// the image sets — PATH entries for its toolchain, JAVA_HOME, PHP_INI_DIR —
// is simply absent unless asked for. Asking the daemon means a provider no
// longer has to restate what its image already says.
func imageEnv(ctx context.Context, gw gwclient.Client, ref string, platform specs.Platform) (map[string]string, error) {
	// Normalize first: BuildKit wants a fully-qualified reference, and a
	// short one like "clojure:temurin-21-..." parses as host:port ("invalid
	// port after host") rather than as repository:tag.
	named, err := reference.ParseNormalizedNamed(ref)
	if err != nil {
		return nil, fmt.Errorf("parsing image reference %s: %w", ref, err)
	}
	full := reference.TagNameOnly(named).String()

	_, _, cfgBytes, err := gw.ResolveImageConfig(ctx, full, sourceresolver.Opt{
		Platform: &platform,
	})
	if err != nil {
		return nil, fmt.Errorf("resolving config for %s: %w", ref, err)
	}
	var img struct {
		Config struct {
			Env []string `json:"Env"`
		} `json:"config"`
	}
	if err := json.Unmarshal(cfgBytes, &img); err != nil {
		return nil, fmt.Errorf("parsing image config for %s: %w", ref, err)
	}
	env := make(map[string]string, len(img.Config.Env))
	for _, kv := range img.Config.Env {
		if k, v, ok := strings.Cut(kv, "="); ok {
			env[k] = v
		}
	}
	return env, nil
}
