// Command bsdkrun-pack is the Go half of `bsdkrun pack`: detect a project's
// language, build a plan, build its rootfs with BuildKit, and generate a
// Kraftfile so `kraft build` (and then `bsdkrun unikraft .`) can turn it into
// a bootable unikernel.
//
// It is compiled at `cargo build --release` time (see core/build.rs) and
// embedded into the `bsdkrun` binary; `bsdkrun pack` extracts and execs this
// binary, forwarding args and inheriting stdio. It is also a normal
// standalone Go binary for local development: `go run ./pack <path>`.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/mattn/go-isatty"
	bkclient "github.com/moby/buildkit/client"

	"github.com/tsirysndr/bsdkrun/pack/internal/buildkit"
	"github.com/tsirysndr/bsdkrun/pack/internal/cachedir"
	"github.com/tsirysndr/bsdkrun/pack/internal/clean"
	"github.com/tsirysndr/bsdkrun/pack/internal/config"
	"github.com/tsirysndr/bsdkrun/pack/internal/ignore"
	"github.com/tsirysndr/bsdkrun/pack/internal/kraft"
	"github.com/tsirysndr/bsdkrun/pack/internal/kraftfile"
	"github.com/tsirysndr/bsdkrun/pack/internal/oci"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/procfile"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers"
	"github.com/tsirysndr/bsdkrun/pack/internal/report"
	"github.com/tsirysndr/bsdkrun/pack/internal/tools"
	"github.com/tsirysndr/bsdkrun/pack/internal/tui"
)

// StartCmdEnv overrides the start command for a build, beating both the
// provider's inference and any Procfile. Split on whitespace — a command
// needing shell quoting belongs in a Procfile, not here.
const StartCmdEnv = "BSDKRUN_START_CMD"

// BuildCacheEnv points at a directory BuildKit imports from and exports to,
// so a build survives the daemon that ran it. Unset means the daemon's own
// cache only, which is right for a workstation (where buildkitd is
// long-lived) and useless in CI (where it is not).
const BuildCacheEnv = "BSDKRUN_PACK_BUILD_CACHE"

func main() {
	// `pull` is a subcommand rather than a flag: its argument is a registry
	// reference, and everything else this command takes is a path.
	if len(os.Args) > 1 && os.Args[1] == "pull" {
		if err := runPull(os.Args[2:]); err != nil {
			fmt.Fprintf(os.Stderr, "bsdkrun pack: %v\n", err)
			os.Exit(1)
		}
		return
	}

	fs := flag.NewFlagSet("bsdkrun-pack", flag.ContinueOnError)
	target := fs.String("target", "", "guest architecture to build for: arm64 or x86_64 (default: this host's)")
	plainOutput := fs.Bool("plain", false, "plain sequential output instead of the animated TUI")
	strace := fs.Bool("strace", false, "trace every guest syscall to the console (very noisy; for a guest that boots but doesn't behave)")
	doClean := fs.Bool("clean", false, "remove the build artifacts pack generated in this project, then exit")
	doPrune := fs.Bool("prune", false, "with --clean, also remove the shared buildkitd container, its build cache, and the kraft builder image")
	planOnly := fs.Bool("plan", false, "resolve the plan and print it as JSON, without building")
	pushRef := fs.String("push", "", "after building, push the unikernel to an OCI registry (e.g. ghcr.io/you/app:v1)")
	loaderDebug := fs.Bool("loader-debug", false, "trace the ELF loader placing the binary, before it runs (says where a guest that dies before its first syscall got to)")
	fs.Usage = func() {
		fmt.Fprintln(os.Stderr, "usage: bsdkrun pack [path] [--target arm64|x86_64] [--push REF] [--plan] [--clean [--prune]] [--plain] [--strace] [--loader-debug]")
		fmt.Fprintln(os.Stderr, "       bsdkrun pack pull REF [dir]")
		fmt.Fprintln(os.Stderr, "\nPackage a project as a bootable Unikraft unikernel.")
		fmt.Fprintln(os.Stderr, "\nArguments:")
		fmt.Fprintln(os.Stderr, "  path   project directory to pack (default \".\")")
		fmt.Fprintln(os.Stderr, "\nRegistry:")
		fmt.Fprintln(os.Stderr, "  --push REF          push the built unikernel; a reference with no registry means Docker Hub")
		fmt.Fprintln(os.Stderr, "  pull REF [dir]      fetch one into the local cache (`bsdkrun unikraft REF` does this for you)")
		fmt.Fprintln(os.Stderr, "\nEnvironment:")
		fmt.Fprintf(os.Stderr, "  %s   override the start command (beats %s and a Procfile)\n",
			StartCmdEnv, config.FileName)
		fmt.Fprintf(os.Stderr, "  %s  directory for a BuildKit cache that outlives the daemon\n", BuildCacheEnv)
		fmt.Fprintf(os.Stderr, "  %s / %s\n", oci.UsernameEnv, oci.PasswordEnv)
		fmt.Fprintf(os.Stderr, "  %s      registry credentials, overriding ~/.docker/config.json\n", oci.TokenEnv)
		fmt.Fprintf(os.Stderr, "  %s   allow plain HTTP to a registry without TLS (localhost needs no opt-in)\n", oci.InsecureEnv)
		fmt.Fprintln(os.Stderr, "\nConfig:")
		fmt.Fprintf(os.Stderr, "  %s   provider, packages, buildAptPackages, deploy.startCommand\n", config.FileName)
	}
	if err := fs.Parse(permuteArgs(fs, os.Args[1:])); err != nil {
		if err == flag.ErrHelp {
			os.Exit(0)
		}
		os.Exit(2)
	}

	path := "."
	if fs.NArg() > 0 {
		path = fs.Arg(0)
	}

	// The TUI needs a real terminal to draw into; piped/redirected output
	// (a log file, CI, `| tee build.log`) falls back to the plain printer —
	// every test run in this repo's own development used exactly that path.
	useTUI := !*plainOutput && isatty.IsTerminal(os.Stdout.Fd())

	// --clean short-circuits too: nothing is built, things are removed.
	if *doClean {
		if err := runClean(path, *doPrune); err != nil {
			fmt.Fprintf(os.Stderr, "bsdkrun pack: %v\n", err)
			os.Exit(1)
		}
		return
	}

	// --plan short-circuits everything below: no Docker, no BuildKit, no
	// kraft. Useful on its own to see what pack decided, and it is how CI
	// learns the kernel name and boot argv a provider produces without
	// running a full build to find out.
	if *planOnly {
		if err := printPlan(path, *target); err != nil {
			fmt.Fprintf(os.Stderr, "bsdkrun pack: %v\n", err)
			os.Exit(1)
		}
		return
	}

	var err error
	if useTUI {
		err = tui.Run(func(r report.Reporter) (string, error) {
			return runPipeline(r, path, *target, *pushRef, *strace, *loaderDebug)
		})
	} else {
		p := report.NewPlain()
		var final string
		final, err = runPipeline(p, path, *target, *pushRef, *strace, *loaderDebug)
		fmt.Printf("\ntotal %s\n", report.FormatDuration(p.Elapsed()))
		if err == nil {
			fmt.Println(final)
		}
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "bsdkrun pack: %v\n", err)
		os.Exit(1)
	}
}

// permuteArgs moves flags ahead of positional arguments.
//
// Go's flag package stops parsing at the first non-flag argument, so
// `bsdkrun pack . --strace` would silently ignore --strace — the exact
// ordering this command's own usage line shows, and the natural one to
// type. (It cost a full CI round trip and two wrong conclusions before
// being spotted: both debug flags read as `false` while appearing to be
// set.) Everything after a bare `--` is left alone, as a positional.
func permuteArgs(fs *flag.FlagSet, args []string) []string {
	var flags, positional []string
	for i := 0; i < len(args); i++ {
		a := args[i]
		if a == "--" {
			positional = append(positional, args[i+1:]...)
			break
		}
		if len(a) < 2 || a[0] != '-' {
			positional = append(positional, a)
			continue
		}
		flags = append(flags, a)
		name := strings.TrimLeft(a, "-")
		if strings.ContainsRune(name, '=') {
			continue // --flag=value carries its own value
		}
		// A non-boolean flag takes the next argument as its value, so that
		// argument has to travel with it rather than becoming positional.
		if f := fs.Lookup(name); f != nil {
			if b, ok := f.Value.(interface{ IsBoolFlag() bool }); !ok || !b.IsBoolFlag() {
				if i+1 < len(args) {
					i++
					flags = append(flags, args[i])
				}
			}
		}
	}
	if len(positional) == 0 {
		return flags
	}
	// The `--` is re-emitted, not just consumed: without it the flag package
	// would parse a positional that happens to start with `-` as a flag,
	// which is the very thing `--` exists to prevent.
	return append(append(flags, "--"), positional...)
}

// applyStartCommand resolves what the guest will run, lowest precedence
// first: what the provider inferred, then the project's Procfile, then
// railpack.json, then an explicit env override — the env var last because
// it is what someone typed deliberately at the point of building.
//
// A unikernel runs exactly one process, so a Procfile's other entries are
// named as dropped rather than ignored silently. r may be nil (--plan),
// in which case nothing is logged.
func applyStartCommand(p *plan.Plan, dir string, cfg *config.Config, r report.Reporter) {
	log := func(msg string) {
		if r != nil {
			r.Log(report.PhasePlan, msg)
		}
	}
	if pf := procfile.Read(dir); pf != nil {
		if cmd, ok := pf.Web(); ok {
			p.Cmd = strings.Fields(cmd)
			log("Procfile: " + cmd)
		}
		if ignored := pf.Ignored(); len(ignored) > 0 {
			log("Procfile: ignoring " + strings.Join(ignored, ", ") +
				" (a unikernel runs one process)")
		}
	}
	if cfg != nil && cfg.Deploy != nil && strings.TrimSpace(cfg.Deploy.StartCommand) != "" {
		p.Cmd = strings.Fields(cfg.Deploy.StartCommand)
		log(config.FileName + ": " + cfg.Deploy.StartCommand)
	}
	if cmd := strings.TrimSpace(os.Getenv(StartCmdEnv)); cmd != "" {
		p.Cmd = strings.Fields(cmd)
		log(StartCmdEnv + ": " + cmd)
	}
}

// bootCmdline is what `bsdkrun unikraft --cmdline` needs.
//
// bsdkrun does not read the Kraftfile's `cmd:` for a locally-built kernel,
// so the program has to be named explicitly, as "<placeholder> -- <argv>":
// everything before `--` is parsed as kernel library parameters and the
// first word is always skipped (Unikraft treats it as the program name), so
// the leading placeholder is required even though it is discarded.
func bootCmdline(p *plan.Plan) string {
	return p.Name + " -- " + strings.Join(p.Cmd, " ")
}

// runClean removes pack's build artifacts. Project-local by default;
// --prune additionally removes the Docker resources shared across every
// project on this host, which are expensive to rebuild and so are not
// bundled into an ordinary clean.
func runClean(path string, prune bool) error {
	absPath, err := filepath.Abs(path)
	if err != nil {
		return err
	}
	fmt.Printf("cleaning %s\n", displayPath(absPath))
	removed, err := clean.Project(absPath)
	if err != nil {
		return err
	}
	fmt.Print(removed.Report("  "))

	if prune {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Minute)
		defer cancel()
		fmt.Println("pruning shared caches")
		shared, err := clean.Shared(ctx)
		if err != nil {
			return err
		}
		fmt.Print(shared.Report("  "))
	}
	return nil
}

// printPlan resolves a plan and prints it, without building anything.
func printPlan(path, targetFlag string) error {
	absPath, err := filepath.Abs(path)
	if err != nil {
		return err
	}
	platform, err := resolvePlatform(targetFlag)
	if err != nil {
		return err
	}
	kraftArch, err := kraft.FromDockerArch(string(platform))
	if err != nil {
		return err
	}

	cfg, err := config.Read(absPath)
	if err != nil {
		return err
	}
	var prov providers.Provider
	if cfg != nil && cfg.Provider != nil && *cfg.Provider != "" {
		if prov = providers.Get(*cfg.Provider); prov == nil {
			return fmt.Errorf("%s names provider %q, which does not exist",
				config.FileName, *cfg.Provider)
		}
	} else if prov, err = providers.Find(absPath); err != nil {
		return err
	}

	p, err := prov.Plan(absPath, plan.Arch(platform))
	if err != nil {
		return err
	}
	applyStartCommand(p, absPath, cfg, nil)

	out := struct {
		Provider   string   `json:"provider"`
		Name       string   `json:"name"`
		BuildImage string   `json:"buildImage"`
		Arch       string   `json:"arch"`
		Kernel     string   `json:"kernel"`
		Cmd        []string `json:"cmd"`
		Cmdline    string   `json:"cmdline"`
	}{
		Provider:   prov.Name(),
		Name:       p.Name,
		BuildImage: p.BuildImage,
		Arch:       string(kraftArch),
		Kernel:     fmt.Sprintf("%s_fc-%s", p.Name, kraftArch),
		Cmd:        p.Cmd,
		Cmdline:    bootCmdline(p),
	}
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	return enc.Encode(out)
}

// runPipeline is the pack pipeline itself: detect -> plan -> build rootfs ->
// generate Kraftfile -> fetch/patch Unikraft -> kraft build. It only talks
// to r — main decides separately whether r renders as the plain printer or
// the TUI, so this function has no idea which.
func runPipeline(r report.Reporter, path, targetFlag, pushRef string, strace, loaderDebug bool) (string, error) {
	absPath, err := filepath.Abs(path)
	if err != nil {
		return "", err
	}
	if _, err := os.Stat(absPath); err != nil {
		return "", err
	}

	platform, err := resolvePlatform(targetFlag)
	if err != nil {
		return "", err
	}
	kraftArch, err := kraft.FromDockerArch(string(platform))
	if err != nil {
		return "", err
	}

	// railpack.json, if present, is the project overriding what pack would
	// infer. A malformed one is an error rather than a shrug: silently
	// ignoring config someone wrote is worse than refusing to build.
	cfg, err := config.Read(absPath)
	if err != nil {
		r.PhaseError(report.PhaseDetect, err)
		return "", err
	}

	r.PhaseStart(report.PhaseDetect)
	var prov providers.Provider
	if cfg != nil && cfg.Provider != nil && *cfg.Provider != "" {
		if prov = providers.Get(*cfg.Provider); prov == nil {
			err := fmt.Errorf("%s names provider %q, which does not exist",
				config.FileName, *cfg.Provider)
			r.PhaseError(report.PhaseDetect, err)
			return "", err
		}
	} else if prov, err = providers.Find(absPath); err != nil {
		r.PhaseError(report.PhaseDetect, err)
		return "", err
	}
	detail := "provider: " + prov.Name()
	if cfg != nil && cfg.Provider != nil {
		detail += " (from " + config.FileName + ")"
	}
	r.PhaseDone(report.PhaseDetect, detail)
	// Name what will not be honoured, rather than dropping it quietly.
	if unsupported := cfg.Unsupported(); len(unsupported) > 0 {
		r.Log(report.PhaseDetect, config.FileName+": ignoring "+
			strings.Join(unsupported, ", ")+" (pack builds one script per provider)")
	}

	r.PhaseStart(report.PhasePlan)
	p, err := prov.Plan(absPath, plan.Arch(platform))
	if err != nil {
		r.PhaseError(report.PhasePlan, err)
		return "", err
	}
	applyStartCommand(p, absPath, cfg, r)

	// Secrets, guest environment and build-time variables from
	// railpack.json.
	if cfg != nil {
		if names := cfg.SecretNames(); len(names) > 0 {
			p.Secrets = names
			r.Log(report.PhasePlan, config.FileName+": secrets "+strings.Join(names, " "))
		}
		if cfg.Deploy != nil && len(cfg.Deploy.Variables) > 0 {
			p.GuestEnv = cfg.Deploy.Variables
			r.Log(report.PhasePlan, fmt.Sprintf("%s: %d guest variable(s)",
				config.FileName, len(cfg.Deploy.Variables)))
		}
		// Build-time variables become environment for the build image, on
		// top of what the image itself declares and what the provider set.
		if vars := cfg.BuildVariables(); len(vars) > 0 {
			if p.Env == nil {
				p.Env = map[string]string{}
			}
			for k, v := range vars {
				p.Env[k] = v
			}
			r.Log(report.PhasePlan, fmt.Sprintf("%s: %d build variable(s)",
				config.FileName, len(vars)))
		}
	}
	// Extra tools the project asked for, installed with mise and put on
	// PATH before anything else runs — a build tool is no use after the
	// build. What the provider already resolved for itself is skipped.
	if extra := tools.Extra(absPath, p.Provider); len(extra) > 0 {
		p.Script = tools.Script(extra) + p.Script
		if p.BuilderScript != "" {
			p.BuilderScript = tools.Script(extra) + p.BuilderScript
		}
		r.Log(report.PhasePlan, "mise: "+strings.Join(extra, " "))
	}
	// Build-time packages, prepended so they are present before the
	// provider's script runs. apt-get or apk depending on the base image —
	// providers pick both Debian and Alpine bases.
	if cfg != nil && len(cfg.BuildAptPackages) > 0 {
		pkgs := strings.Join(cfg.BuildAptPackages, " ")
		p.Script = fmt.Sprintf(`if command -v apt-get >/dev/null 2>&1; then
    apt-get update -qq && apt-get install -y -qq --no-install-recommends %s
else
    apk add --no-cache %s
fi
`, pkgs, pkgs) + p.Script
		r.Log(report.PhasePlan, config.FileName+": build packages "+pkgs)
	}
	r.PhaseDone(report.PhasePlan, fmt.Sprintf("name: %s, build image: %s", p.Name, p.BuildImage))

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Minute)
	defer cancel()

	r.PhaseStart(report.PhaseRootfs)
	cache, err := cachedir.Dir()
	if err != nil {
		r.PhaseError(report.PhaseRootfs, err)
		return "", err
	}
	addr, err := buildkit.Bootstrap(ctx, cache)
	if err != nil {
		err = fmt.Errorf("bootstrapping buildkitd: %w", err)
		r.PhaseError(report.PhaseRootfs, err)
		return "", err
	}
	rootfsRelDir := fmt.Sprintf(".rootfs-%s", kraftArch)
	rootfsDir := filepath.Join(absPath, rootfsRelDir)
	onStatus := func(s *bkclient.SolveStatus) { r.BuildKitStatus(report.PhaseRootfs, s) }
	var extraExcludes []string
	if cfg != nil {
		extraExcludes = cfg.Exclude
	}
	excludes := ignore.Read(absPath, extraExcludes)
	// A persistent BuildKit cache, when pointed at one. buildkitd's own
	// cache lives inside its container, so CI — which gets a fresh one every
	// job — otherwise rebuilds from scratch every time.
	buildCache := os.Getenv(BuildCacheEnv)
	if err := buildkit.Build(ctx, addr, absPath, p, platform, rootfsDir, excludes, buildCache, onStatus); err != nil {
		err = fmt.Errorf("building rootfs: %w", err)
		r.PhaseError(report.PhaseRootfs, err)
		return "", err
	}
	r.PhaseDone(report.PhaseRootfs, displayPath(rootfsDir))

	r.PhaseStart(report.PhaseKraftfile)
	if err := kraftfile.Write(absPath, p, kraftfile.Options{Strace: strace, LoaderDebug: loaderDebug}); err != nil {
		err = fmt.Errorf("generating Kraftfile: %w", err)
		r.PhaseError(report.PhaseKraftfile, err)
		return "", err
	}
	r.PhaseDone(report.PhaseKraftfile, displayPath(filepath.Join(absPath, "Kraftfile")))

	// kraft.Build covers both remaining pack-level phases in one call (fetch
	// + patch, then kraft build) — its onPhase callback tells us which of
	// the two is active so PhaseStart/PhaseDone land on the right one.
	r.PhaseStart(report.PhaseFetch)
	kraftCurrent := report.PhaseFetch
	onPhase := func(phase string) {
		r.PhaseDone(kraftCurrent, "")
		switch phase {
		case "build":
			kraftCurrent = report.PhaseKraftBuild
		default:
			kraftCurrent = report.PhaseFetch
		}
		r.PhaseStart(kraftCurrent)
	}
	onLine := func(line string) { r.Log(kraftCurrent, line) }
	if err := kraft.Build(ctx, absPath, kraftArch, p.Name, rootfsRelDir, onPhase, onLine); err != nil {
		err = fmt.Errorf("kraft build: %w", err)
		r.PhaseError(kraftCurrent, err)
		return "", err
	}
	kernelPath := fmt.Sprintf("%s/.unikraft/build/%s_fc-%s", absPath, p.Name, kraftArch)
	r.PhaseDone(report.PhaseKraftBuild, displayPath(kernelPath))

	cmdline := bootCmdline(p)
	final := fmt.Sprintf("built: %s\nboot it: bsdkrun unikraft %s --cmdline %q",
		displayPath(kernelPath), displayPath(absPath), cmdline)

	if pushRef != "" {
		r.PhaseStart(report.PhasePush)
		digest, err := oci.Push(pushRef, kernelPath, oci.Metadata{
			Name:     p.Name,
			Provider: p.Provider,
			Arch:     string(kraftArch),
			Cmdline:  cmdline,
			Kernel:   filepath.Base(kernelPath),
		}, func(line string) { r.Log(report.PhasePush, line) })
		if err != nil {
			r.PhaseError(report.PhasePush, err)
			return "", err
		}
		r.PhaseDone(report.PhasePush, digest)
		// The pushed reference is the useful one to echo: it is what
		// someone else runs, and it needs no copy of this directory.
		final = fmt.Sprintf("pushed: %s\nboot it: bsdkrun unikraft %s", digest, pushRef)
	}
	return final, nil
}

// runPull fetches a unikernel from a registry into dir (or the shared cache
// when no directory is given) and prints where it landed. This is what the
// Rust side calls when `bsdkrun unikraft` is handed a reference rather than
// a path.
func runPull(args []string) error {
	if len(args) == 0 {
		return fmt.Errorf("usage: bsdkrun pack pull REF [dir]")
	}
	ref := args[0]

	dest := ""
	if len(args) > 1 {
		dest = args[1]
	} else {
		var err error
		if dest, err = CachePath(ref); err != nil {
			return err
		}
	}

	// Already cached: say so and stop. A boot asks for this on every run,
	// and a unikernel reference is immutable in practice — re-fetching it
	// would put a registry round trip in front of every start.
	meta, err := cached(dest)
	if err != nil {
		return err
	}
	if meta == nil {
		if meta, err = oci.Pull(ref, dest, func(line string) {
			fmt.Fprintln(os.Stderr, line)
		}); err != nil {
			return err
		}
	}

	// Printed to stdout, as the caller's machine-readable answer: the
	// kernel to boot and the argv to boot it with, which the kernel itself
	// does not record.
	out, err := json.Marshal(struct {
		Kernel  string `json:"kernel"`
		Cmdline string `json:"cmdline"`
		Name    string `json:"name"`
		Arch    string `json:"arch"`
	}{
		Kernel:  filepath.Join(dest, oci.KernelFileName),
		Cmdline: meta.Cmdline,
		Name:    meta.Name,
		Arch:    meta.Arch,
	})
	if err != nil {
		return err
	}
	fmt.Println(string(out))
	return nil
}

// cached reads a previously pulled unikernel's metadata, or returns nil if
// this reference has not been pulled. Both files have to be present: a
// kernel without metadata is missing the argv needed to boot it.
func cached(dir string) (*oci.Metadata, error) {
	if _, err := os.Stat(filepath.Join(dir, oci.KernelFileName)); err != nil {
		return nil, nil
	}
	body, err := os.ReadFile(filepath.Join(dir, "metadata.json"))
	if err != nil {
		return nil, nil
	}
	var meta oci.Metadata
	if err := json.Unmarshal(body, &meta); err != nil {
		return nil, nil
	}
	return &meta, nil
}

// CachePath is where a pulled unikernel lives. Keyed by the reference with
// the characters a path cannot hold replaced, so two tags of the same image
// do not collide and neither needs a registry round trip to locate.
func CachePath(ref string) (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	safe := strings.NewReplacer("/", "_", ":", "_", "@", "_").Replace(ref)
	return filepath.Join(home, ".cache", "bsdkrun", "unikernels", safe), nil
}

// displayPath renders p relative to the current working directory when
// that's actually shorter — `bsdkrun pack .` should echo back `.` and
// `Kraftfile`, not a 60-character absolute path repeated at every step. Every
// internal operation still uses the absolute path (buildkit.Build,
// kraft.Build, and friends never see this); it exists purely for what gets
// printed.
//
// Picking whichever form is shorter (rather than always preferring relative)
// is what keeps this correct when `path` points well outside the current
// directory: a naive `filepath.Rel` there produces a "../../../../.." chain
// longer and less readable than the absolute path it was trying to shorten.
func displayPath(p string) string {
	cwd, err := os.Getwd()
	if err != nil {
		return p
	}
	rel, err := filepath.Rel(cwd, p)
	if err != nil || len(rel) >= len(p) {
		return p
	}
	return rel
}

func resolvePlatform(targetFlag string) (buildkit.Platform, error) {
	switch targetFlag {
	case "":
		return buildkit.HostPlatform(runtime.GOARCH)
	case "arm64":
		return buildkit.PlatformArm64, nil
	case "x86_64", "amd64":
		return buildkit.PlatformAmd64, nil
	default:
		return "", fmt.Errorf("unsupported --target %q (want arm64 or x86_64)", targetFlag)
	}
}
