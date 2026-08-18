package main

// Executing a plan: one microVM per workflow, booted through the bsdkrun Go
// SDK. The SDK shells out to the `bsdkrun` binary — the very one that
// extracted and exec'd this tool, handed back via $BSDKRUN_BIN — so a local
// run needs nothing installed beyond bsdkrun itself.
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

	bsdkrun "github.com/tsirysndr/bsdkrun/sdk/go"
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

	create := bsdkrun.Linux(plan.Image).
		Name(vmName(plan.Name)).
		Cpus(opts.Cpus).
		Mem(opts.Mem).
		// The guest runs steps, not a service: idle init is all it needs.
		Command("sleep", "infinity")
	if opts.Source != "" {
		// Read-only is the point: a CI step must not be able to write to the
		// checkout that triggered it.
		create = create.Mount(opts.Source + ":" + sourceMount + ":ro")
	}

	sbx, err := create.Create()
	if err != nil {
		return nil, fmt.Errorf("booting the %s VM (image %s): %w", plan.Name, plan.Image, err)
	}
	defer func() {
		if err != nil && opts.Keep {
			logf(opts, "keeping VM %s for inspection — `bsdkrun shell %s`, `bsdkrun rm -f %s`\n",
				sbx.ID, sbx.ID, sbx.ID)
			return
		}
		if rmErr := sbx.Remove(true); rmErr != nil {
			logf(opts, "warning: could not remove VM %s: %v\n", sbx.ID, rmErr)
		}
	}()

	// The workspace and home exist before any step runs; every step then
	// starts from the workspace, like spindle's container workdir.
	if _, err := sbx.Command("mkdir").Args("-p", workspaceDir, homeDir).Check().Run(); err != nil {
		return nil, fmt.Errorf("preparing %s: %w", workspaceDir, err)
	}

	for i, step := range plan.Steps {
		start := time.Now()
		emitControl(opts, i, step, "start")

		// `bash -lc` rather than sh: bash is in every image this plans
		// (spindle appends it to the dependency set), and workflow commands
		// are written against it. `cd` per step because exec sessions do not
		// share a cwd.
		script := "cd " + workspaceDir + " && {\n" + step.Command + "\n}"
		cmd := sbx.Command("bash").Args("-lc", script)
		for k, v := range plan.Env {
			cmd = cmd.Env(k, v)
		}
		for k, v := range step.Env {
			cmd = cmd.Env(k, v)
		}
		if !opts.JSON {
			cmd = cmd.Stdout(prefixed(opts.Out)).Stderr(prefixed(opts.Out))
		}
		res, runErr := cmd.Run()

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
			emitData(opts, i, res.Stdout, "stdout")
			emitData(opts, i, res.Stderr, "stderr")
		}
		emitControl(opts, i, step, "end")

		if runErr != nil {
			return results, fmt.Errorf("step %q could not run: %w", step.Name, runErr)
		}
		if code != 0 {
			return results, fmt.Errorf("step %q failed with exit code %d", step.Name, code)
		}
		logf(opts, "  ✓ %s (%s)\n", step.Name, results[len(results)-1].Duration)
	}
	return results, nil
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
