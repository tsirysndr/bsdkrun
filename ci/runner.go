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
	// Secret values to inject into every step, and the log masker built from
	// them — every emitted line passes through mask() so a value (or its
	// base64 encoding) never survives into any consumer's view.
	Secrets map[string]string
	Masker  *strings.Replacer
}

func (o runOpts) mask(sub string) string {
	if o.Masker == nil {
		return sub
	}
	return o.Masker.Replace(sub)
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
	if plan.Platform != "" && plan.Platform != "tangled" {
		logf(opts, "workflow %s [%s]\n  image %s\n", plan.Name, plan.Platform, plan.Image)
	} else {
		logf(opts, "workflow %s\n  image %s\n", plan.Name, plan.Image)
	}

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
		bootOut = &lineEmitter{opts: opts, stepID: 0, stream: "stdout"}
		bootErr = &lineEmitter{opts: opts, stepID: 0, stream: "stderr"}
	} else {
		bootOut = prefixed(opts)
		bootErr = prefixed(opts)
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

	// Workflow commands are written against bash, and every image the
	// tangled path plans carries it — but a foreign platform's image may be
	// bash-less (alpine). GitLab's own runner falls back to sh there; so
	// does this, decided once per VM.
	shell := "bash"
	if res, err := sbx.exec([]string{"sh", "-c", "command -v bash >/dev/null 2>&1"}, nil, nil, nil); err != nil || res.ExitCode != 0 {
		shell = "sh"
		logf(opts, "  (no bash in this image — steps run under sh)\n")
	}

	// The workspace and home exist before any step runs; every step then
	// starts from the workspace, like spindle's container workdir.
	if res, err := sbx.exec([]string{"mkdir", "-p", workspaceDir, homeDir}, nil, nil, nil); err != nil {
		return nil, fmt.Errorf("preparing %s: %w", workspaceDir, err)
	} else if res.ExitCode != 0 {
		return nil, fmt.Errorf("preparing %s: %s", workspaceDir, opts.mask(strings.TrimSpace(res.Stderr)))
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
		// share a cwd. User steps start from the plan's workdir (a monorepo
		// subdirectory when the workflows live in one); system steps always
		// run at the workspace root, which exists before the clone does.
		wd := workspaceDir
		if !step.System && plan.Workdir != "" {
			wd = plan.Workdir
		}
		script := "cd " + wd + " && {\n" + step.Command + "\n}"
		env := map[string]string{}
		for k, v := range plan.Env {
			env[k] = v
		}
		// Secrets are run-time input, so they beat the committed workflow's
		// environment; a step's own env stays the most specific thing.
		for k, v := range opts.Secrets {
			env[k] = v
		}
		for k, v := range step.Env {
			env[k] = v
		}
		// Streamed in BOTH modes. JSON used to emit a step's output only
		// after it exited, which froze every consumer of the stream (desktop,
		// web, the TUI) for the length of a compile — a `nix build` looked
		// hung for minutes and then dumped everything at once.
		var sOut, sErr io.Writer
		if opts.JSON {
			sOut = &lineEmitter{opts: opts, stepID: idx, stream: "stdout"}
			sErr = &lineEmitter{opts: opts, stepID: idx, stream: "stderr"}
		} else {
			sOut = prefixed(opts)
			sErr = prefixed(opts)
		}
		res, runErr := sbx.exec([]string{shell, "-lc", script}, env, sOut, sErr)

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
	opts    runOpts
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
		emitJSON(l.opts.Out, map[string]any{
			"kind":    "data",
			"content": l.opts.mask(line),
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

// prefixed indents guest output so it reads as belonging to a step, and
// masks secrets per line. Line-buffered — the mask must see whole lines, or
// a value split across two writes would slip through.
func prefixed(opts runOpts) io.Writer {
	return &prefixWriter{opts: opts, prefix: "    "}
}

type prefixWriter struct {
	opts    runOpts
	prefix  string
	partial string
}

func (p *prefixWriter) Write(b []byte) (int, error) {
	p.partial += string(b)
	for {
		i := strings.IndexByte(p.partial, '\n')
		if i < 0 {
			break
		}
		line := strings.TrimRight(p.partial[:i], "\r")
		p.partial = p.partial[i+1:]
		fmt.Fprintf(p.opts.Out, "%s%s\n", p.prefix, p.opts.mask(line))
	}
	return len(b), nil
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
		"kind": "control",
		// Masked like everything else: a workflow that interpolates (or
		// happens to contain) a secret's value must not replay it here.
		"content":      opts.mask(step.Name),
		"time":         time.Now().Format(time.RFC3339Nano),
		"step_id":      idx,
		"step_status":  status,
		"step_kind":    kind,
		"step_command": opts.mask(step.Command),
	})
}

func emitData(opts runOpts, idx int, content, stream string) {
	if content == "" {
		return
	}
	for _, line := range strings.Split(strings.TrimRight(content, "\n"), "\n") {
		emitJSON(opts.Out, map[string]any{
			"kind":    "data",
			"content": opts.mask(line),
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
