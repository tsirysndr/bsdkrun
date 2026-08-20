// bsdkrun-ci runs tangled spindle workflows in bsdkrun microVMs.
//
// It is the Go half of `bsdkrun ci`, compiled by core/build.rs and embedded
// in the bsdkrun binary exactly as `pack/` is — an end user never needs Go,
// and the `bsdkrun` that extracted this binary hands itself back through
// $BSDKRUN_BIN for the SDK to drive.
//
// Go rather than Rust for one decisive reason: the workflow schema and its
// `when:` matching are imported from tangled.org/core itself, so a file
// spindle accepts is byte-for-byte a file this accepts. A Rust reimplementation
// would be a second opinion about someone else's format, wrong within a month.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"golang.org/x/term"
	"tangled.org/core/workflow"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
	"github.com/tsirysndr/bsdkrun/ci/project"
	"github.com/tsirysndr/bsdkrun/ci/project/deploy"
)

func jsonMarshal(v any) ([]byte, error) { return json.Marshal(v) }

func main() {
	// Go's flag package stops at the first positional, so `run test.yml
	// --event push` would silently read no flags at all — the same trap
	// pack/main.go documents. Permute flags to the front, positionals to
	// the back, before parsing.
	args := os.Args[1:]
	cmd := "run"
	if len(args) > 0 && !strings.HasPrefix(args[0], "-") {
		// `bsdkrun ci <dir>` reads as "run the workflows there", not as a
		// subcommand — a path is never a verb, and making people type `run`
		// before it helps nobody.
		if st, err := os.Stat(args[0]); err == nil && st.IsDir() && !isCommand(args[0]) {
			// keep args as-is; cmdRun treats the directory positional below
		} else {
			cmd, args = args[0], args[1:]
		}
	} else if len(args) > 0 && (args[0] == "--help" || args[0] == "-h") {
		// A bare `--help` is a help request, not run's flag — without this it
		// falls into run's FlagSet and prints the flag dump instead.
		cmd = "help"
	}

	var err error
	switch cmd {
	case "run":
		err = cmdRun(permute(args))
	case "ls":
		err = cmdLs(permute(args))
	case "serve":
		err = cmdServe(permute(args))
	case "help", "--help", "-h":
		usage()
	default:
		fmt.Fprintf(os.Stderr, "unknown command %q\n\n", cmd)
		usage()
		os.Exit(2)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "Error:", err)
		os.Exit(1)
	}
}

func isCommand(s string) bool {
	switch s {
	case "run", "ls", "serve", "help":
		return true
	}
	return false
}

func usage() {
	fmt.Print(`bsdkrun ci — run tangled spindle workflows in microVMs

Usage:
  bsdkrun ci [path] [names...]  run workflows (default: every one that matches;
                                a directory positional is the workspace)
  bsdkrun ci run [names...]     the same, spelled out
  bsdkrun ci ls                 list workflows and whether they match
  bsdkrun ci serve              accept sh.tangled.pipeline records over HTTP

Run flags:
  --event push|pull_request|manual   trigger to simulate (default manual)
  --branch <name>                    pull_request target branch
  --input k=v                        manual-trigger input (repeatable)
  -f, --file <workflow.yml>          run an explicit file (repeatable; skips
                                     discovery and 'when' matching)
  -w, --workspace <dir>              repository to run against (default .)
  --cpus N, --mem MIB                VM size (default 2 cpus, 2048 MiB)
  --keep                             keep the VM after a failure
  --json                             spindle log-line JSON on stdout
  --plain                            plain line output even on a terminal
  --sh, --shell, --ssh               when the workflow ends (pass or fail),
                                     keep its microVM and open a shell in it
  --sh=JOB                           only for the job named JOB (prefix or
                                     substring is enough)
  --dagger-call FUNCTION             for a dagger module: the function to call
                                     (default: ci, test, build or all)
                                     (the default when stdout is not a tty)
  --dry-run                          a generated deploy step announces its
                                     command instead of running it
  --detect                           ignore CI configs: detect the project
                                     (go, rust, nodejs, bun, deno, python,
                                     ruby, php, elixir, gleam, zig, clojure,
                                     dotnet, crystal, haskell)
                                     and generate + run a workflow for it.
                                     This also happens automatically when a
                                     repository has no CI config at all
  --platform <name>                  run a foreign CI config locally:
                                     github (also forgejo/gitea), gitlab,
                                     woodpecker, drone, circleci, buildkite,
                                     semaphore, jenkins, azure, codebuild,
                                     tekton, travis — detected
                                     automatically when the repository has no
                                     .tangled/workflows; linux jobs only
  --secret KEY=VALUE | KEY           inject a secret env var into every step
                                     (bare KEY reads the host environment);
                                     values are masked as *** in all output
  --secrets-file <path>              dotenv file of secrets (repeatable);
                                     .tangled/secrets.env is loaded
                                     automatically when present — gitignore it
  --nixery <host>                    a self-hosted nixery instead of nixery.dev
  --otlp <url>                       export OpenTelemetry spans (or set
                                     OTEL_EXPORTER_OTLP_ENDPOINT); one span
                                     per step, sent live as each ends

A manual run executes the repository's HEAD commit — never the dirty working
tree. Commit first; CI that quietly tested uncommitted changes would pass
locally and fail everywhere else.
`)
}

// permute moves every flag (and its value) ahead of the positionals.
func permute(args []string) []string {
	flagsWithValue := map[string]bool{
		"--event": true, "--branch": true, "--input": true, "--workspace": true,
		"-w": true, "--cpus": true, "--mem": true, "--bind": true,
		"-f": true, "--file": true, "--nixery": true, "--otlp": true,
		"--secret": true, "--secrets-file": true, "--platform": true,
	}
	var flags, rest []string
	for i := 0; i < len(args); i++ {
		a := args[i]
		if strings.HasPrefix(a, "-") {
			flags = append(flags, a)
			if flagsWithValue[a] && !strings.Contains(a, "=") && i+1 < len(args) {
				i++
				flags = append(flags, args[i])
			}
		} else {
			rest = append(rest, a)
		}
	}
	return append(flags, rest...)
}

// repeatable is a flag.Value collecting every occurrence.
type repeatable []string

func (r *repeatable) String() string     { return strings.Join(*r, ",") }
func (r *repeatable) Set(v string) error { *r = append(*r, v); return nil }

func cmdRun(args []string) error {
	fs := flag.NewFlagSet("run", flag.ExitOnError)
	event := fs.String("event", "manual", "")
	branch := fs.String("branch", "", "")
	workspace := fs.String("workspace", ".", "")
	fs.StringVar(workspace, "w", ".", "")
	cpus := fs.Int("cpus", 2, "")
	mem := fs.Int("mem", 2048, "")
	keep := fs.Bool("keep", false, "")
	jsonOut := fs.Bool("json", false, "")
	plain := fs.Bool("plain", false, "")
	platformFlag := fs.String("platform", "auto", "")
	detect := fs.Bool("detect", false, "")
	dryRun := fs.Bool("dry-run", false, "")
	nixery := fs.String("nixery", "", "")
	otlp := fs.String("otlp", "", "")
	// Three spellings of one flag: --sh is what it is, --shell is what people
	// type, --ssh is what fingers type after years of remote debugging. None
	// of them is worth a wrong-flag error.
	// Which dagger function to call, for a repository whose CI *is* a dagger
	// module. Empty picks a conventional one in the guest, where the engine
	// that can answer "what does this module expose?" is running.
	daggerCall := fs.String("dagger-call", "", "")
	shellFlag := &shellTarget{}
	fs.Var(shellFlag, "sh", "")
	fs.Var(shellFlag, "shell", "")
	fs.Var(shellFlag, "ssh", "")
	var inputs, files, secretFlags, secretFiles repeatable
	fs.Var(&inputs, "input", "")
	fs.Var(&files, "file", "")
	fs.Var(&files, "f", "")
	fs.Var(&secretFlags, "secret", "")
	fs.Var(&secretFiles, "secrets-file", "")
	if err := fs.Parse(args); err != nil {
		return err
	}
	names := fs.Args()
	platforms.DaggerCall = *daggerCall
	nixeryOverride = *nixery
	otlpOverride = *otlp

	// A positional that is an existing directory is the workspace, the rest
	// are workflow names: `bsdkrun ci examples/foo test` needs no -w.
	kept := names[:0]
	for _, n := range names {
		if st, err := os.Stat(n); err == nil && st.IsDir() {
			*workspace = n
			continue
		}
		kept = append(kept, n)
	}
	names = kept

	repo, err := inspectRepo(*workspace)
	if err != nil {
		return err
	}
	inputMap := map[string]string{}
	for _, kv := range inputs {
		k, v, ok := strings.Cut(kv, "=")
		if !ok {
			return fmt.Errorf("--input wants k=v, got %q", kv)
		}
		inputMap[k] = v
	}
	tr, err := localTrigger(*event, repo, *branch, inputMap)
	if err != nil {
		return err
	}
	pipelineID := fmt.Sprintf("at://did:local/%s/local-%s", "sh.tangled.pipeline", repo.Sha[:12])

	// Which world is this? Native tangled workflows win unless --platform
	// points elsewhere; without them, the well-known files of the supported
	// platforms are probed in registry order.
	native := dirHasTangled(repo.WorkflowRoot())
	if (*platformFlag != "auto" && *platformFlag != "tangled") || *detect {
		native = false
	}
	if !native && len(files) == 0 {
		var plat *platforms.Platform
		if !*detect {
			var derr error
			plat, derr = platforms.Detect(repo.WorkflowRoot(), *platformFlag)
			if derr != nil {
				return derr
			}
		}
		// Nothing recognizable (or --detect): read the project itself and
		// generate a workflow on the fly — pack's detect step, for CI.
		if plat == nil && (*detect || *platformFlag == "auto") {
			if proj := project.Detect(repo.WorkflowRoot()); proj != nil {
				secrets, err := collectSecrets(repo.WorkflowRoot(), secretFlags, secretFiles)
				if err != nil {
					return err
				}
				// The secrets say where this ships: a deploy step joins the
				// generated workflow (and only the generated one — a
				// committed config already says what it deploys).
				secretNames := make([]string, 0, len(secrets))
				for k := range secrets {
					secretNames = append(secretNames, k)
				}
				target, alsoDetected := deploy.Detect(secretNames)
				if target != nil {
					last := len(proj.Jobs) - 1
					proj.Jobs[last].Steps = append(proj.Jobs[last].Steps,
						target.Step(*dryRun))
				}
				opts := runOpts{
					Cpus:    *cpus,
					Mem:     *mem,
					Keep:    *keep,
					Shell:   *shellFlag,
					Source:  repo.Root,
					JSON:    *jsonOut,
					Out:     os.Stdout,
					Secrets: secrets,
					Masker:  newMasker(secrets),
				}
				var plans []*Plan
				for _, j := range proj.Jobs {
					if len(names) > 0 && !nameMatches(j.Name, names) {
						continue
					}
					plans = append(plans, foreignPlan(proj.Language, j, repo))
				}
				if len(plans) == 0 {
					return fmt.Errorf("no generated job matches")
				}
				// Say exactly what was inferred before anything boots: the
				// project, the marker that gave it away, and every step —
				// a generated workflow the operator has not read must be
				// shown, not sprung.
				fmt.Fprintf(os.Stderr, "detected %s project (%s) — no CI configuration found, workflow generated:\n",
					proj.Language, proj.Marker)
				for _, plan := range plans {
					fmt.Fprintf(os.Stderr, "  %s · image %s\n", plan.Name, plan.Image)
					n := 0
					for _, st := range plan.Steps {
						if st.System {
							continue
						}
						n++
						fmt.Fprintf(os.Stderr, "    %d. %s\n", n, st.Name)
					}
				}
				if target != nil {
					mode := ""
					if *dryRun {
						mode = " [dry-run]"
					}
					fmt.Fprintf(os.Stderr, "deploy: %s (%s detected)%s\n",
						target.Platform, target.Secret, mode)
					if len(alsoDetected) > 0 {
						fmt.Fprintf(os.Stderr, "  (tokens for %s also present — first match wins)\n",
							joinNames(alsoDetected))
					}
				}
				return runPlans(plans, opts, *jsonOut, *plain)
			}
			if *detect {
				return fmt.Errorf("nothing detectable here — known project types: %s",
					joinNames(project.Languages()))
			}
		}
		if plat != nil {
			secrets, err := collectSecrets(repo.WorkflowRoot(), secretFlags, secretFiles)
			if err != nil {
				return err
			}
			ghToken = secrets["GITHUB_TOKEN"]
			opts := runOpts{
				Cpus:    *cpus,
				Mem:     *mem,
				Keep:    *keep,
				Shell:   *shellFlag,
				Source:  repo.Root,
				JSON:    *jsonOut,
				Out:     os.Stdout,
				Secrets: secrets,
				Masker:  newMasker(secrets),
			}
			plans, err := foreignPlans(plat, repo, names)
			if err != nil {
				return err
			}
			fmt.Fprintf(os.Stderr, "platform: %s (%d job(s))\n", plat.Name, len(plans))
			return runPlans(plans, opts, *jsonOut, *plain)
		}
		if *platformFlag != "auto" && *platformFlag != "tangled" {
			return fmt.Errorf("--platform %s: no config found in %s", *platformFlag, repo.WorkflowRoot())
		}
	}

	// Explicit files bypass discovery *and* matching: naming a file is the
	// selection, the same way spindle's manual dispatch skips constraints.
	var selected []workflow.Workflow
	if len(files) > 0 {
		for _, f := range files {
			contents, err := os.ReadFile(f)
			if err != nil {
				return err
			}
			wf, err := workflow.FromFile(filepath.Base(f), contents)
			if err != nil {
				return fmt.Errorf("%s: %w", f, err)
			}
			selected = append(selected, wf)
		}
	} else {
		wfs, err := loadWorkflows(repo.WorkflowRoot())
		if err != nil {
			return err
		}
		changed := changedFiles(repo, tr)
		for _, wf := range wfs {
			if len(names) > 0 && !nameMatches(wf.Name, names) {
				continue
			}
			// Naming a workflow on the command line is a manual selection;
			// only an unnamed sweep asks the `when:` constraints.
			if len(names) == 0 {
				ok, err := wf.Match(*tr, changed)
				if err != nil {
					return fmt.Errorf("%s: %w", wf.Name, err)
				}
				if !ok {
					continue
				}
			}
			selected = append(selected, wf)
		}
	}
	if len(selected) == 0 {
		fmt.Println("No workflow matches this trigger. `bsdkrun ci ls` shows why.")
		return nil
	}

	secrets, err := collectSecrets(repo.WorkflowRoot(), secretFlags, secretFiles)
	if err != nil {
		return err
	}

	opts := runOpts{
		Cpus:    *cpus,
		Mem:     *mem,
		Keep:    *keep,
		Shell:   *shellFlag,
		Source:  repo.Root,
		JSON:    *jsonOut,
		Out:     os.Stdout,
		Secrets: secrets,
		Masker:  newMasker(secrets),
	}

	plans := make([]*Plan, 0, len(selected))
	for _, wf := range selected {
		plan, err := buildPlan(wf, tr, pipelineID, repo.Subdir)
		if err != nil {
			return err
		}
		plans = append(plans, plan)
	}
	return runPlans(plans, opts, *jsonOut, *plain)
}

// runPlans executes plans through the renderer the output mode calls for.
// printRunTotal closes a run with what it cost: how many workflows ran, how
// many failed, and the wall time for all of it. Written to stderr so it
// survives a piped stdout, and skipped in --json, where the stream is the
// output and a prose line would corrupt it.
func printRunTotal(opts runOpts, total, failed int, d time.Duration) {
	if opts.JSON || total == 0 {
		return
	}
	word := "workflows"
	if total == 1 {
		word = "workflow"
	}
	switch {
	case failed > 0:
		fmt.Fprintf(os.Stderr, "%d %s in %s — %d failed\n", total, word, fmtDur(d), failed)
	default:
		fmt.Fprintf(os.Stderr, "%d %s in %s\n", total, word, fmtDur(d))
	}
}

// An interactive terminal gets the live TUI; --json, --plain and anything
// piped get lines. The TUI consumes the same LogLine stream --json prints,
// so the two views can never tell a different story.
func runPlans(plans []*Plan, opts runOpts, jsonOut, plain bool) error {
	// --sh ends by handing the terminal to a shell, and the TUI owns the
	// terminal until it exits: two things cannot draw on it at once, so the
	// shell wins and the run prints lines.
	if opts.Shell.on {
		plain = true
	}
	// One clock for the whole run, started before the first VM boots: what a
	// reader wants to compare against their hosted CI is wall time, image
	// pulls and all, not the sum of the step timings.
	started := time.Now()
	if !jsonOut && !plain && term.IsTerminal(int(os.Stdout.Fd())) {
		failed, err := runPlansTUI(plans, opts)
		platforms.CleanupTempDisks()
		printRunTotal(opts, len(plans), failed, time.Since(started))
		return err
	}
	failed := 0
	shellMatched := false
	for i, plan := range plans {
		// The shell tells you what exiting it means, so it has to know what
		// is still queued behind this workflow.
		opts.MoreJobs = len(plans) - i - 1
		if opts.Shell.matches(plan.Name) {
			shellMatched = true
		}
		// A shelled workflow that failed does not stop the rest: the point of
		// jumping into one job is to inspect it while the run carries on.
		if _, err := runPlan(plan, opts); err != nil {
			failed++
			fmt.Fprintf(os.Stderr, "✗ %s: %v\n", plan.Name, err)
			continue
		}
		logf(opts, "✓ %s passed\n\n", plan.Name)
	}
	// A job name that matched nothing would otherwise be a silent no-op — the
	// run would look normal and the shell simply never appear.
	if opts.Shell.on && opts.Shell.job != "" && !shellMatched {
		names := make([]string, 0, len(plans))
		for _, p := range plans {
			names = append(names, p.Name)
		}
		fmt.Fprintf(os.Stderr, "--sh=%s matched no job in this run; it has: %s\n",
			opts.Shell.job, strings.Join(names, ", "))
	}
	platforms.CleanupTempDisks()
	printRunTotal(opts, len(plans), failed, time.Since(started))
	if failed > 0 {
		return fmt.Errorf("%d of %d workflow(s) failed", failed, len(plans))
	}
	return nil
}

// dirHasTangled reports whether root carries native workflows.
func joinNames(names []string) string {
	out := ""
	for i, n := range names {
		if i > 0 {
			out += ", "
		}
		out += n
	}
	return out
}

func dirHasTangled(root string) bool {
	entries, err := os.ReadDir(filepath.Join(root, workflow.WorkflowDir))
	return err == nil && len(entries) > 0
}

func nameMatches(name string, wanted []string) bool {
	base := strings.TrimSuffix(strings.TrimSuffix(name, ".yml"), ".yaml")
	for _, w := range wanted {
		if w == name || strings.TrimSuffix(strings.TrimSuffix(w, ".yml"), ".yaml") == base {
			return true
		}
	}
	return false
}

func cmdLs(args []string) error {
	fs := flag.NewFlagSet("ls", flag.ExitOnError)
	event := fs.String("event", "manual", "")
	branch := fs.String("branch", "", "")
	workspace := fs.String("workspace", ".", "")
	fs.StringVar(workspace, "w", ".", "")
	jsonOut := fs.Bool("json", false, "")
	if err := fs.Parse(args); err != nil {
		return err
	}

	repo, err := inspectRepo(*workspace)
	if err != nil {
		return err
	}
	tr, err := localTrigger(*event, repo, *branch, nil)
	if err != nil {
		return err
	}
	// No native workflows? List what the detected foreign platform would run.
	if !dirHasTangled(repo.WorkflowRoot()) {
		plat, derr := platforms.Detect(repo.WorkflowRoot(), "auto")
		if derr == nil && plat == nil {
			if proj := project.Detect(repo.WorkflowRoot()); proj != nil {
				if *jsonOut {
					type drow struct {
						Name     string `json:"name"`
						Engine   string `json:"engine"`
						Matches  bool   `json:"matches"`
						Platform string `json:"platform"`
					}
					var rows []drow
					for _, j := range proj.Jobs {
						rows = append(rows, drow{Name: j.Name, Engine: "generated",
							Matches: true, Platform: proj.Language})
					}
					b, _ := json.Marshal(rows)
					fmt.Println(string(b))
					return nil
				}
				fmt.Printf("detected %s project (%s) — workflow would be generated:\n",
					proj.Language, proj.Marker)
				for _, j := range proj.Jobs {
					for _, st := range j.Steps {
						fmt.Printf("  %s\n", st.Name)
					}
				}
				return nil
			}
		}
		if derr == nil && plat != nil {
			jobs, jerr := plat.Load(repo.WorkflowRoot(), platformRepo(repo))
			if jerr != nil {
				return jerr
			}
			// Same keys as the tangled listing, so every consumer (the
			// desktop/web CI screens included) reads one shape; `platform`
			// is what the badge shows.
			type frow struct {
				Name     string `json:"name"`
				Engine   string `json:"engine"`
				Matches  bool   `json:"matches"`
				Platform string `json:"platform"`
				Skip     string `json:"skip,omitempty"`
			}
			var rows []frow
			for _, j := range jobs {
				rows = append(rows, frow{
					Name: j.Name, Engine: plat.Name, Platform: plat.Name,
					Matches: j.SkipReason == "", Skip: j.SkipReason,
				})
			}
			if *jsonOut {
				b, _ := json.Marshal(rows)
				fmt.Println(string(b))
				return nil
			}
			fmt.Printf("%-28s  %-10s  %s\n", "JOB", "PLATFORM", "RUNNABLE")
			for _, r := range rows {
				state := "yes"
				if !r.Matches {
					state = "no — " + r.Skip
				}
				fmt.Printf("%-28s  %-10s  %s\n", r.Name, r.Platform, state)
			}
			return nil
		}
	}

	wfs, err := loadWorkflows(repo.WorkflowRoot())
	if err != nil {
		return err
	}
	changed := changedFiles(repo, tr)

	type row struct {
		Name     string `json:"name"`
		Engine   string `json:"engine"`
		Matches  bool   `json:"matches"`
		Platform string `json:"platform"`
	}
	var rows []row
	for _, wf := range wfs {
		ok, _ := wf.Match(*tr, changed)
		rows = append(rows, row{Name: wf.Name, Engine: wf.Engine, Matches: ok, Platform: "tangled"})
	}

	if *jsonOut {
		b, _ := json.Marshal(rows)
		fmt.Println(string(b))
		return nil
	}
	fmt.Printf("%-28s  %-10s  %s\n", "WORKFLOW", "ENGINE", "MATCHES --event "+*event)
	for _, r := range rows {
		match := "no"
		if r.Matches {
			match = "yes"
		}
		fmt.Printf("%-28s  %-10s  %s\n", r.Name, r.Engine, match)
	}
	return nil
}
