package main

// Executing a plan: one microVM per workflow, driven through the inlined
// bsdkrun driver (driver.go — see there for why the Go SDK was removed). The
// driver shells out to the `bsdkrun` binary that extracted and exec'd this
// tool, handed back via $BSDKRUN_BIN, so a local run needs nothing installed
// beyond bsdkrun itself.
//
// One VM per *workflow*, not per step, matching spindle: steps share a
// filesystem (the clone lands once, build output persists between steps) and
// the workflow's failure tears down everything it did.

import (
	"fmt"
	"io"
	"os"
	"strings"
	"time"
)

// runOpts is what the CLI layer decides; the runner just obeys.
type runOpts struct {
	Cpus int
	Mem  int
	// Keep the VM around after a failure, for post-mortem `bsdkrun shell`.
	Keep bool
	// Source directory mounted read-only for the clone step. Empty in serve
	// mode, where the clone fetches from a knot URL instead.
	Source string
	// Structured spindle-style log lines on stdout instead of human output.
	JSON bool
	Out  io.Writer
}

// stepResult is what one step came to, for the run report.
type stepResult struct {
	Name     string        `json:"name"`
	System   bool          `json:"system"`
	ExitCode int           `json:"exit_code"`
	Duration time.Duration `json:"duration"`
}

// runPlan boots the VM, runs every step in order, and returns the failing
// step's error (nil when all pass). The VM is destroyed on success and on
// failure unless opts.Keep — a CI runner that leaks a VM per run would eat
// the host in a day.
func runPlan(plan *Plan, opts runOpts) (results []stepResult, err error) {
	logf(opts, "workflow %s\n  image %s\n", plan.Name, plan.Image)

	// One trace per run, one span per step, exported live when a collector
	// is configured (--otlp / $OTEL_EXPORTER_OTLP_ENDPOINT). A nil trace is
	// tracing off; every call below tolerates it.
	trace := NewTrace(plan.Name, opts.Source)
	defer func() { trace.Finish(err) }()

	// Read-only mount is the point: a CI step must not be able to write to
	// the checkout that triggered it. The guest runs steps, not a service, so
	// idle init is all it needs.
	var mounts []string
	if opts.Source != "" {
		mounts = append(mounts, opts.Source+":"+sourceMount+":ro")
	}
	name := vmName(plan.Name)

	// The boot is a step like any other, and a *streamed* one: an image pull
	// can take minutes (nixery builds large dependency sets server-side and
	// may even 504 while it does — bsdkrun retries, and the retry notice
	// belongs on screen, not in a void). Step id 0; user steps start at 1.
	bootStep := Step{Name: "Boot VM", System: true, Command: "bsdkrun linux " + plan.Image}
	bootStart := time.Now()
	emitControl(opts, 0, bootStep, "start")
	bootSpan := trace.StartSpan("Boot VM", map[string]string{"image": plan.Image})
	var bootOut, bootErr io.Writer
	if opts.JSON {
		bootOut = &lineEmitter{out: opts.Out, stepID: 0, stream: "stdout"}
		bootErr = &lineEmitter{out: opts.Out, stepID: 0, stream: "stderr"}
	} else {
		bootOut = prefixed(opts.Out)
		bootErr = prefixed(opts.Out)
	}
	sbx, err := createVM(plan.Image, name, opts.Cpus, opts.Mem, mounts,
		[]string{"sleep", "infinity"}, bootOut, bootErr)
	if err != nil && len(plan.NixpkgsDeps) > 0 && plan.Image != fallbackImage {
		// nixery could not produce the image (it builds server-side, and a
		// big closure outlives its gateway timeout however patiently the
		// pull retries). Same environment, different road: the pinned nix
		// base image plus `nix profile add` — announced, because a fallback
		// nobody sees looks like magic or a bug depending on the day.
		note := fmt.Sprintf(
			"image pull failed (%v) — falling back to %s + nix profile add",
			err, fallbackImage,
		)
		if opts.JSON {
			emitData(opts, 0, note, "stderr")
		} else {
			logf(opts, "  %s\n", note)
		}
		plan.ToFallback()
		sbx, err = createVM(plan.Image, name+"-fb", opts.Cpus, opts.Mem, mounts,
			[]string{"sleep", "infinity"}, bootOut, bootErr)
	}
	bootRes := stepResult{
		Name:     bootStep.Name,
		System:   true,
		Duration: time.Since(bootStart).Round(time.Millisecond),
	}
	bootSpan.End(err)
	if err != nil {
		bootRes.ExitCode = 1
		results = append(results, bootRes)
		emitControl(opts, 0, bootStep, "end")
		return results, fmt.Errorf("booting the %s VM (image %s): %w", plan.Name, plan.Image, err)
	}
	// The boot step is not done until the guest can actually run something.
	if err := sbx.waitReady(60 * time.Second); err != nil {
		bootRes.ExitCode = 1
		results = append(results, bootRes)
		emitControl(opts, 0, bootStep, "end")
		return results, fmt.Errorf("booting the %s VM: %w", plan.Name, err)
	}
	results = append(results, bootRes)
	emitControl(opts, 0, bootStep, "end")
	logf(opts, "  ✓ %s (%s)\n", bootStep.Name, bootRes.Duration)
	defer func() {
		if err != nil && opts.Keep {
			logf(opts, "keeping VM %s for inspection — `bsdkrun shell %s`, `bsdkrun rm -f %s`\n",
				sbx.ID, sbx.ID, sbx.ID)
			return
		}
		if rmErr := sbx.remove(); rmErr != nil {
			logf(opts, "warning: could not remove VM %s: %v\n", sbx.ID, rmErr)
		}
	}()

	// The workspace and home exist before any step runs; every step then
	// starts from the workspace, like spindle's container workdir.
	if res, err := sbx.exec([]string{"mkdir", "-p", workspaceDir, homeDir}, nil, nil, nil); err != nil {
		return nil, fmt.Errorf("preparing %s: %w", workspaceDir, err)
	} else if res.ExitCode != 0 {
		return nil, fmt.Errorf("preparing %s: %s", workspaceDir, strings.TrimSpace(res.Stderr))
	}

	for i, step := range plan.Steps {
		idx := i + 1
		start := time.Now()
		emitControl(opts, idx, step, "start")
		span := trace.StartSpan(step.Name, map[string]string{
			"step.system": fmt.Sprintf("%t", step.System),
		})

		// `bash -lc` rather than sh: bash is in every image this plans
		// (spindle appends it to the dependency set), and workflow commands
		// are written against it. `cd` per step because exec sessions do not
		// share a cwd.
		script := "cd " + workspaceDir + " && {\n" + step.Command + "\n}"
		env := map[string]string{}
		for k, v := range plan.Env {
			env[k] = v
		}
		for k, v := range step.Env {
			env[k] = v
		}
		var sOut, sErr io.Writer
		if !opts.JSON {
			sOut = prefixed(opts.Out)
			sErr = prefixed(opts.Out)
		}
		res, runErr := sbx.exec([]string{"bash", "-lc", script}, env, sOut, sErr)

		code := 0
		if res != nil {
			code = res.ExitCode
		}
		results = append(results, stepResult{
			Name:     step.Name,
			System:   step.System,
			ExitCode: code,
			Duration: time.Since(start).Round(time.Millisecond),
		})
		if opts.JSON && res != nil {
			emitData(opts, idx, res.Stdout, "stdout")
			emitData(opts, idx, res.Stderr, "stderr")
		}
		emitControl(opts, idx, step, "end")

		if runErr != nil {
			span.End(runErr)
			return results, fmt.Errorf("step %q could not run: %w", step.Name, runErr)
		}
		if code != 0 {
			stepErr := fmt.Errorf("step %q failed with exit code %d", step.Name, code)
			span.End(stepErr)
			return results, stepErr
		}
		span.End(nil)
		logf(opts, "  ✓ %s (%s)\n", step.Name, results[len(results)-1].Duration)
	}
	return results, nil
}

// lineEmitter turns a raw output stream into per-line spindle data records —
// what lets a boot's progress render in the same step UI as everything else.
type lineEmitter struct {
	out     io.Writer
	stepID  int
	stream  string
	partial string
}

func (l *lineEmitter) Write(b []byte) (int, error) {
	l.partial += string(b)
	for {
		i := strings.IndexByte(l.partial, '\n')
		if i < 0 {
			break
		}
		line := strings.TrimRight(l.partial[:i], "\r")
		l.partial = l.partial[i+1:]
		if line == "" {
			continue
		}
		emitJSON(l.out, map[string]any{
			"kind":    "data",
			"content": line,
			"time":    time.Now().Format(time.RFC3339Nano),
			"step_id": l.stepID,
			"stream":  l.stream,
		})
	}
	return len(b), nil
}

// vmName gives CI VMs a recognizable, prunable prefix.
func vmName(workflow string) string {
	clean := strings.Map(func(r rune) rune {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9', r == '-':
			return r
		case r >= 'A' && r <= 'Z':
			return r + ('a' - 'A')
		default:
			return '-'
		}
	}, strings.TrimSuffix(strings.TrimSuffix(workflow, ".yml"), ".yaml"))
	return fmt.Sprintf("bsdkrun-ci-%s-%d", clean, time.Now().Unix()%100000)
}

// prefixed indents guest output so it reads as belonging to a step.
func prefixed(w io.Writer) io.Writer {
	return &prefixWriter{w: w, prefix: []byte("    ")}
}

type prefixWriter struct {
	w       io.Writer
	prefix  []byte
	midline bool
}

func (p *prefixWriter) Write(b []byte) (int, error) {
	total := len(b)
	for len(b) > 0 {
		if !p.midline {
			if _, err := p.w.Write(p.prefix); err != nil {
				return total - len(b), err
			}
			p.midline = true
		}
		i := strings.IndexByte(string(b), '\n')
		if i < 0 {
			_, err := p.w.Write(b)
			return total, err
		}
		if _, err := p.w.Write(b[:i+1]); err != nil {
			return total - len(b), err
		}
		b = b[i+1:]
		p.midline = false
	}
	return total, nil
}

func logf(opts runOpts, format string, args ...any) {
	if opts.JSON {
		return
	}
	fmt.Fprintf(opts.Out, format, args...)
}

// The two JSON emitters speak spindle's log-line schema (models.LogLine), so
// anything that consumes a spindle log stream can consume `--json` output.

func emitControl(opts runOpts, idx int, step Step, status string) {
	if !opts.JSON {
		if status == "start" {
			fmt.Fprintf(opts.Out, "  ▶ %s\n", step.Name)
		}
		return
	}
	kind := 1 // user
	if step.System {
		kind = 0
	}
	emitJSON(opts.Out, map[string]any{
		"kind":         "control",
		"content":      step.Name,
		"time":         time.Now().Format(time.RFC3339Nano),
		"step_id":      idx,
		"step_status":  status,
		"step_kind":    kind,
		"step_command": step.Command,
	})
}

func emitData(opts runOpts, idx int, content, stream string) {
	if content == "" {
		return
	}
	for _, line := range strings.Split(strings.TrimRight(content, "\n"), "\n") {
		emitJSON(opts.Out, map[string]any{
			"kind":    "data",
			"content": line,
			"time":    time.Now().Format(time.RFC3339Nano),
			"step_id": idx,
			"stream":  stream,
		})
	}
}

func emitJSON(w io.Writer, v map[string]any) {
	b, err := jsonMarshal(v)
	if err != nil {
		fmt.Fprintln(os.Stderr, "log line lost:", err)
		return
	}
	w.Write(append(b, '\n'))
}
