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

	"tangled.org/core/workflow"
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
		cmd, args = args[0], args[1:]
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

func usage() {
	fmt.Print(`bsdkrun ci — run tangled spindle workflows in microVMs

Usage:
  bsdkrun ci run [names...]     run workflows (default: every one that matches)
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
		"-f": true, "--file": true,
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
	var inputs, files repeatable
	fs.Var(&inputs, "input", "")
	fs.Var(&files, "file", "")
	fs.Var(&files, "f", "")
	if err := fs.Parse(args); err != nil {
		return err
	}
	names := fs.Args()

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
		wfs, err := loadWorkflows(repo.Root)
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

	opts := runOpts{
		Cpus:   *cpus,
		Mem:    *mem,
		Keep:   *keep,
		Source: repo.Root,
		JSON:   *jsonOut,
		Out:    os.Stdout,
	}

	failed := 0
	for _, wf := range selected {
		plan, err := buildPlan(wf, tr, pipelineID)
		if err != nil {
			return err
		}
		if _, err := runPlan(plan, opts); err != nil {
			failed++
			fmt.Fprintf(os.Stderr, "✗ %s: %v\n", wf.Name, err)
			continue
		}
		logf(opts, "✓ %s passed\n\n", wf.Name)
	}
	if failed > 0 {
		return fmt.Errorf("%d of %d workflow(s) failed", failed, len(selected))
	}
	return nil
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
	wfs, err := loadWorkflows(repo.Root)
	if err != nil {
		return err
	}
	changed := changedFiles(repo, tr)

	type row struct {
		Name    string `json:"name"`
		Engine  string `json:"engine"`
		Matches bool   `json:"matches"`
	}
	var rows []row
	for _, wf := range wfs {
		ok, _ := wf.Match(*tr, changed)
		rows = append(rows, row{Name: wf.Name, Engine: wf.Engine, Matches: ok})
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
