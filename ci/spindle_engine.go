//go:build spindle

package main

// The bsdkrun engine, as spindle sees it.
//
// Spindle puts execution behind one interface — models.Engine — and calls it
// per workflow: InitWorkflow compiles the record into steps, SetupWorkflow
// brings a machine up, RunStep runs step N inside it, DestroyWorkflow tears it
// down. Everything else spindle does (the XRPC surface, the ACL, the event
// stream, the log format) is engine-agnostic, which is what makes a drop-in
// possible: `bsdkrun ci serve` runs spindle's own handlers over this engine,
// so a workflow that ran on spindle's VMs runs unchanged on libkrun ones.
//
// Deliberately NOT imported: `tangled.org/core/spindle` itself, which pulls in
// its own microvm engine and with it qemu and Linux cgroups — that package
// does not build on darwin at all. Importing only the handler packages keeps
// this cross-platform and keeps libkrun the single VM mechanism.

import (
	"context"
	"fmt"
	"log/slog"
	"strings"
	"sync"
	"time"

	"tangled.org/core/api/tangled"
	"tangled.org/core/spindle/models"
	"tangled.org/core/spindle/secrets"
	"tangled.org/core/workflow"
)

var _ models.Engine = (*spindleEngine)(nil)

// engineStep adapts one plan step to spindle's Step interface. The kind is
// not cosmetic: spindle's log format distinguishes steps it injected (clone,
// prepare) from the ones the user wrote.
type engineStep struct{ step Step }

func (s engineStep) Name() string    { return s.step.Name }
func (s engineStep) Command() string { return s.step.Command }
func (s engineStep) Kind() models.StepKind {
	if s.step.System {
		return models.StepKindSystem
	}
	return models.StepKindUser
}

// engineWorkflow is what hangs off models.Workflow.Data: the resolved plan,
// the machine running it, and the shell that machine turned out to have.
type engineWorkflow struct {
	plan  *Plan
	vm    *vm
	shell string
}

type spindleEngine struct {
	l       *slog.Logger
	cpus    int
	mem     int
	timeout time.Duration

	mu  sync.Mutex
	vms map[string]*vm // by WorkflowId.String(), so cancel can reach them
}

func newSpindleEngine(l *slog.Logger, cpus, mem int, timeout time.Duration) *spindleEngine {
	return &spindleEngine{
		l:       l.With("engine", "bsdkrun"),
		cpus:    cpus,
		mem:     mem,
		timeout: timeout,
		vms:     map[string]*vm{},
	}
}

// InitWorkflow compiles a pipeline record's workflow into steps. The parsing
// is tangled's own (workflow.FromFile) and the planning is the one `ci run`
// uses, so a served workflow and a local one resolve identically.
func (e *spindleEngine) InitWorkflow(twf tangled.Pipeline_Workflow, tpl tangled.Pipeline) (*models.Workflow, error) {
	if tpl.TriggerMetadata == nil {
		return nil, fmt.Errorf("workflow %q: pipeline has no trigger metadata", twf.Name)
	}
	wf, err := workflow.FromFile(twf.Name, []byte(twf.Raw))
	if err != nil {
		return nil, fmt.Errorf("parsing workflow %q: %w", twf.Name, err)
	}
	plan, err := buildPlan(wf, tpl.TriggerMetadata, "", "")
	if err != nil {
		return nil, err
	}
	// A served run has no mounted checkout: the clone fetches from the knot,
	// the way spindle's own engines do.
	for i, st := range plan.Steps {
		if strings.HasPrefix(st.Name, "Clone repository into workspace") {
			plan.Steps[i] = remoteCloneStep(&twf, *tpl.TriggerMetadata)
			break
		}
	}

	mwf := &models.Workflow{
		Name:        twf.Name,
		Environment: plan.Env,
		Data:        &engineWorkflow{plan: plan, shell: "bash"},
	}
	for _, st := range plan.Steps {
		mwf.Steps = append(mwf.Steps, engineStep{step: st})
	}
	return mwf, nil
}

// SetupWorkflow boots the machine and brings it to the state every step
// assumes: a trustworthy clock, a known shell, and an existing workspace.
// Spindle logs this as step -1, framed by control lines like any other step.
func (e *spindleEngine) SetupWorkflow(ctx context.Context, wid models.WorkflowId, wf *models.Workflow, wfLogger models.WorkflowLogger) error {
	ew, ok := wf.Data.(*engineWorkflow)
	if !ok {
		return fmt.Errorf("workflow %s was not initialised by this engine", wid)
	}

	const setupIdx = -1
	setup := engineStep{step: Step{Name: "Boot VM", System: true, Command: "bsdkrun linux " + ew.plan.Image}}
	wfLogger.ControlWriter(setupIdx, setup, models.StepStatusStart).Write(nil)
	defer wfLogger.ControlWriter(setupIdx, setup, models.StepStatusEnd).Write(nil)

	out := wfLogger.DataWriter(setupIdx, "stdout")
	errW := wfLogger.DataWriter(setupIdx, "stderr")

	mem := e.mem
	if ew.plan.MinMemMiB > mem {
		mem = ew.plan.MinMemMiB
		fmt.Fprintf(out, "memory raised to %d MiB — the workflow declares it needs it\n", mem)
	}

	machine, err := createVM(ew.plan.Image, vmName(wid.String()), e.cpus, mem,
		ew.plan.ExtraMounts, []string{"sleep", "infinity"}, out, errW)
	if err != nil && len(ew.plan.NixpkgsDeps) > 0 && ew.plan.Image != fallbackImage {
		// nixery could not build the image in time; same environment by the
		// other road, announced rather than silently substituted.
		fmt.Fprintf(errW, "image pull failed (%v) — falling back to %s + nix profile add\n", err, fallbackImage)
		ew.plan.ToFallback()
		machine, err = createVM(ew.plan.Image, vmName(wid.String())+"-fb", e.cpus, mem,
			ew.plan.ExtraMounts, []string{"sleep", "infinity"}, out, errW)
	}
	if err != nil {
		return fmt.Errorf("booting the %s VM (image %s): %w", wf.Name, ew.plan.Image, err)
	}
	ew.vm = machine
	e.mu.Lock()
	e.vms[wid.String()] = machine
	e.mu.Unlock()

	if err := machine.waitReady(60 * time.Second); err != nil {
		return fmt.Errorf("guest agent for %s: %w", wf.Name, err)
	}

	// The guest wall clock has booted at the epoch and drifted days from the
	// host; TLS and package-index validation both break on it. Best-effort.
	_, _ = machine.exec([]string{"sh", "-c",
		fmt.Sprintf("date -u -s @%d >/dev/null 2>&1 || true", time.Now().Unix())}, nil, nil, nil)

	// Workflow commands are written against bash, but a workflow may name an
	// image without it; decide once per VM, as the local runner does.
	if res, err := machine.exec([]string{"sh", "-c", "command -v bash >/dev/null 2>&1"}, nil, nil, nil); err != nil || res.ExitCode != 0 {
		ew.shell = "sh"
		fmt.Fprintln(out, "no bash in this image — steps run under sh")
	}

	// Workspace and HOME before any step. HOME on a tmpfs: on Linux hosts
	// the virtio-fs rootfs shows the real host uid and tools refuse a HOME
	// "owned by someone else".
	res, err := machine.exec([]string{"sh", "-c",
		"mkdir -p " + workspaceDir + " " + homeDir +
			" && { mount -t tmpfs -o mode=0755 tmpfs " + homeDir + " 2>/dev/null || true; }" +
			" && chown 0:0 /tangled " + workspaceDir + " " + homeDir + " 2>/dev/null || true"},
		nil, nil, errW)
	if err != nil {
		return fmt.Errorf("preparing %s: %w", workspaceDir, err)
	}
	if res.ExitCode != 0 {
		return fmt.Errorf("preparing %s: %s", workspaceDir, strings.TrimSpace(res.Stderr))
	}

	fmt.Fprintf(out, "%s ready on libkrun (%s)\n", ew.plan.Image, machine.ID)
	return nil
}

func (e *spindleEngine) WorkflowTimeout() time.Duration { return e.timeout }

// DestroyWorkflow is called for every registered engine when a pipeline is
// cancelled, so an id this engine never saw is normal, not an error.
func (e *spindleEngine) DestroyWorkflow(ctx context.Context, wid models.WorkflowId) error {
	e.mu.Lock()
	machine := e.vms[wid.String()]
	delete(e.vms, wid.String())
	e.mu.Unlock()
	if machine == nil {
		return nil
	}
	return machine.remove()
}

// RunStep runs one step in the workflow's machine. Secrets arrive per step;
// they go in as environment, never argv, and the logger masks their values on
// the way to disk.
func (e *spindleEngine) RunStep(ctx context.Context, wid models.WorkflowId, w *models.Workflow, idx int, secretList []secrets.UnlockedSecret, wfLogger models.WorkflowLogger) error {
	ew, ok := w.Data.(*engineWorkflow)
	if !ok || ew.vm == nil {
		return fmt.Errorf("workflow %s has no machine: SetupWorkflow did not run", wid)
	}
	if idx < 0 || idx >= len(ew.plan.Steps) {
		return fmt.Errorf("step %d out of range for workflow %s", idx, wid)
	}
	step := ew.plan.Steps[idx]

	// User steps start from the plan's workdir (a monorepo subdirectory when
	// the workflows live in one); system steps run at the workspace root,
	// which exists before the clone does.
	wd := workspaceDir
	if !step.System && ew.plan.Workdir != "" {
		wd = ew.plan.Workdir
	}
	script := "cd " + wd + " && {\n" + step.Command + "\n}"

	env := map[string]string{}
	for k, v := range ew.plan.Env {
		env[k] = v
	}
	// Secrets are run-time input, so they beat the committed workflow's
	// environment; a step's own env stays the most specific thing.
	for _, s := range secretList {
		env[s.Key] = s.Value
	}
	for k, v := range step.Env {
		env[k] = v
	}

	res, err := ew.vm.exec([]string{ew.shell, "-c", script}, env,
		wfLogger.DataWriter(idx, "stdout"), wfLogger.DataWriter(idx, "stderr"))
	if err != nil {
		return fmt.Errorf("step %q could not run: %w", step.Name, err)
	}
	if res.ExitCode != 0 {
		return fmt.Errorf("step %q failed with exit code %d", step.Name, res.ExitCode)
	}
	return nil
}

// engineNames are the engine keys this server answers to. Every workflow in
// the wild names `nixery` or `microvm`; after a swap they must keep running,
// so all of them resolve to this engine rather than failing as "unknown
// engine". `bsdkrun` is the honest name for what actually runs them.
func engineNames() []string { return []string{"bsdkrun", "nixery", "microvm", "dummy"} }

// enginesFor builds the map spindle's dispatch looks names up in.
func enginesFor(e models.Engine) map[string]models.Engine {
	m := map[string]models.Engine{}
	for _, name := range engineNames() {
		m[name] = e
	}
	return m
}
