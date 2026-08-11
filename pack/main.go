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
	"github.com/tsirysndr/bsdkrun/pack/internal/detect"
	"github.com/tsirysndr/bsdkrun/pack/internal/kraft"
	"github.com/tsirysndr/bsdkrun/pack/internal/kraftfile"
	"github.com/tsirysndr/bsdkrun/pack/internal/plan"
	"github.com/tsirysndr/bsdkrun/pack/internal/report"
	"github.com/tsirysndr/bsdkrun/pack/internal/tui"
)

func main() {
	fs := flag.NewFlagSet("bsdkrun-pack", flag.ContinueOnError)
	target := fs.String("target", "", "guest architecture to build for: arm64 or x86_64 (default: this host's)")
	plainOutput := fs.Bool("plain", false, "plain sequential output instead of the animated TUI")
	fs.Usage = func() {
		fmt.Fprintln(os.Stderr, "usage: bsdkrun pack [path] [--target arm64|x86_64] [--plain]")
		fmt.Fprintln(os.Stderr, "\nPackage a project as a bootable Unikraft unikernel.")
		fmt.Fprintln(os.Stderr, "\nArguments:")
		fmt.Fprintln(os.Stderr, "  path   project directory to pack (default \".\")")
	}
	if err := fs.Parse(os.Args[1:]); err != nil {
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

	var err error
	if useTUI {
		err = tui.Run(func(r report.Reporter) (string, error) {
			return runPipeline(r, path, *target)
		})
	} else {
		p := report.NewPlain()
		var final string
		final, err = runPipeline(p, path, *target)
		if err == nil {
			fmt.Println(final)
		}
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "bsdkrun pack: %v\n", err)
		os.Exit(1)
	}
}

// runPipeline is the pack pipeline itself: detect -> plan -> build rootfs ->
// generate Kraftfile -> fetch/patch Unikraft -> kraft build. It only talks
// to r — main decides separately whether r renders as the plain printer or
// the TUI, so this function has no idea which.
func runPipeline(r report.Reporter, path, targetFlag string) (string, error) {
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

	r.PhaseStart(report.PhaseDetect)
	d, err := detect.Detect(absPath)
	if err != nil {
		r.PhaseError(report.PhaseDetect, err)
		return "", err
	}
	r.PhaseDone(report.PhaseDetect, fmt.Sprintf("provider: %s", d.Provider))

	r.PhaseStart(report.PhasePlan)
	p, err := plan.Build(d)
	if err != nil {
		r.PhaseError(report.PhasePlan, err)
		return "", err
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
	r.PhaseDone(report.PhaseRootfs, rootfsDir)

	r.PhaseStart(report.PhaseKraftfile)
	if err := kraftfile.Write(absPath, p); err != nil {
		err = fmt.Errorf("generating Kraftfile: %w", err)
		r.PhaseError(report.PhaseKraftfile, err)
		return "", err
	}
	r.PhaseDone(report.PhaseKraftfile, filepath.Join(absPath, "Kraftfile"))

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
	r.PhaseDone(report.PhaseKraftBuild, kernelPath)

	// bsdkrun unikraft does not read the Kraftfile's `cmd:` for a
	// locally-built kernel (see examples/unikraft-expressjs/README.md,
	// "--cmdline is required") — it has to be given explicitly, as
	// "<placeholder> -- <argv>": everything before `--` is parsed as kernel
	// library parameters and the first word is always skipped (treated as
	// the program name), so a leading placeholder is mandatory even though
	// it's discarded.
	cmdline := p.Name + " -- " + strings.Join(p.Cmd, " ")
	final := fmt.Sprintf("built: %s\nboot it: bsdkrun unikraft %s --cmdline %q", kernelPath, absPath, cmdline)
	return final, nil
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
