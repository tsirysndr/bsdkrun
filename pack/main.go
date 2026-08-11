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
	"github.com/tsirysndr/bsdkrun/pack/internal/kraft"
	"github.com/tsirysndr/bsdkrun/pack/internal/kraftfile"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/procfile"
	"github.com/tsirysndr/bsdkrun/pack/internal/providers"
	"github.com/tsirysndr/bsdkrun/pack/internal/report"
	"github.com/tsirysndr/bsdkrun/pack/internal/tui"
)

// StartCmdEnv overrides the start command for a build, beating both the
// provider's inference and any Procfile. Split on whitespace — a command
// needing shell quoting belongs in a Procfile, not here.
const StartCmdEnv = "BSDKRUN_START_CMD"

func main() {
	fs := flag.NewFlagSet("bsdkrun-pack", flag.ContinueOnError)
	target := fs.String("target", "", "guest architecture to build for: arm64 or x86_64 (default: this host's)")
	plainOutput := fs.Bool("plain", false, "plain sequential output instead of the animated TUI")
	strace := fs.Bool("strace", false, "trace every guest syscall to the console (very noisy; for a guest that boots but doesn't behave)")
	doClean := fs.Bool("clean", false, "remove the build artifacts pack generated in this project, then exit")
	doPrune := fs.Bool("prune", false, "with --clean, also remove the shared buildkitd container, its build cache, and the kraft builder image")
	planOnly := fs.Bool("plan", false, "resolve the plan and print it as JSON, without building")
	loaderDebug := fs.Bool("loader-debug", false, "trace the ELF loader placing the binary, before it runs (says where a guest that dies before its first syscall got to)")
	fs.Usage = func() {
		fmt.Fprintln(os.Stderr, "usage: bsdkrun pack [path] [--target arm64|x86_64] [--plan] [--clean [--prune]] [--plain] [--strace] [--loader-debug]")
		fmt.Fprintln(os.Stderr, "\nPackage a project as a bootable Unikraft unikernel.")
		fmt.Fprintln(os.Stderr, "\nArguments:")
		fmt.Fprintln(os.Stderr, "  path   project directory to pack (default \".\")")
		fmt.Fprintln(os.Stderr, "\nEnvironment:")
		fmt.Fprintf(os.Stderr, "  %s   override the start command (beats %s and a Procfile)\n",
			StartCmdEnv, config.FileName)
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
			return runPipeline(r, path, *target, *strace, *loaderDebug)
		})
	} else {
		p := report.NewPlain()
		var final string
		final, err = runPipeline(p, path, *target, *strace, *loaderDebug)
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
func runPipeline(r report.Reporter, path, targetFlag string, strace, loaderDebug bool) (string, error) {
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
	if err := buildkit.Build(ctx, addr, absPath, p, platform, rootfsDir, onStatus); err != nil {
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
	return final, nil
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
