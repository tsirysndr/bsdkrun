package main

// `bsdkrun ci serve` — the server half: a runner that accepts spindle's own
// pipeline records over HTTP and executes each workflow in a microVM.
//
// This is deliberately the *runner* seam, not the whole spindle. Spindle is
// also a jetstream consumer, an XRPC server, a secrets store and an AT-proto
// record publisher; all of that stays with spindle (or tack). What this
// serves is the piece bsdkrun is uniquely placed to provide — executing a
// `sh.tangled.pipeline` in real VMs — behind an interface small enough to
// point either of them at, or curl by hand:
//
//	curl -X POST localhost:8517/pipelines -d @pipeline.json
//	curl localhost:8517/pipelines/<id>/logs        # spindle LogLine JSON
//
// Unlike a local run there is no checkout to mount, so the clone step fetches
// from the knot URL in the trigger metadata, exactly as spindle's engines do.

import (
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"strings"
	"sync"
	"time"

	"tangled.org/core/api/tangled"
	"tangled.org/core/workflow"
)

type pipelineRun struct {
	ID       string                  `json:"id"`
	Status   string                  `json:"status"` // pending|running|failed|success
	Error    string                  `json:"error,omitempty"`
	Started  time.Time               `json:"started"`
	Finished *time.Time              `json:"finished,omitempty"`
	Results  map[string][]stepResult `json:"results,omitempty"`

	mu   sync.Mutex
	logs []byte
}

func (r *pipelineRun) Write(b []byte) (int, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.logs = append(r.logs, b...)
	return len(b), nil
}

type server struct {
	mu   sync.Mutex
	runs map[string]*pipelineRun
	next int
	opts runOpts
}

func cmdServe(args []string) error {
	fs := flag.NewFlagSet("serve", flag.ExitOnError)
	bind := fs.String("bind", "127.0.0.1:8517", "")
	cpus := fs.Int("cpus", 2, "")
	mem := fs.Int("mem", 2048, "")
	if err := fs.Parse(args); err != nil {
		return err
	}

	s := &server{
		runs: map[string]*pipelineRun{},
		opts: runOpts{Cpus: *cpus, Mem: *mem, JSON: true},
	}
	mux := http.NewServeMux()
	mux.HandleFunc("POST /pipelines", s.postPipeline)
	mux.HandleFunc("GET /pipelines/{id}", s.getPipeline)
	mux.HandleFunc("GET /pipelines/{id}/logs", s.getLogs)
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) {
		fmt.Fprintln(w, "ok")
	})

	fmt.Printf("bsdkrun ci serve on http://%s\n", *bind)
	fmt.Println("POST a sh.tangled.pipeline record to /pipelines to run it.")
	return http.ListenAndServe(*bind, mux)
}

func (s *server) postPipeline(w http.ResponseWriter, r *http.Request) {
	var tpl tangled.Pipeline
	if err := json.NewDecoder(r.Body).Decode(&tpl); err != nil {
		http.Error(w, "not a sh.tangled.pipeline record: "+err.Error(), http.StatusBadRequest)
		return
	}
	if tpl.TriggerMetadata == nil || len(tpl.Workflows) == 0 {
		http.Error(w, "a pipeline needs triggerMetadata and workflows", http.StatusBadRequest)
		return
	}

	s.mu.Lock()
	s.next++
	id := fmt.Sprintf("run-%d", s.next)
	run := &pipelineRun{ID: id, Status: "pending", Started: time.Now(), Results: map[string][]stepResult{}}
	s.runs[id] = run
	s.mu.Unlock()

	go s.execute(run, &tpl)

	w.WriteHeader(http.StatusAccepted)
	json.NewEncoder(w).Encode(map[string]string{"id": id})
}

func (s *server) execute(run *pipelineRun, tpl *tangled.Pipeline) {
	run.Status = "running"
	opts := s.opts
	opts.Out = run
	// No mounted source on a server: the clone fetches over the network.
	opts.Source = ""

	var firstErr error
	for _, twf := range tpl.Workflows {
		if twf == nil {
			continue
		}
		wf, err := workflow.FromFile(twf.Name, []byte(twf.Raw))
		if err == nil {
			var plan *Plan
			plan, err = buildPlan(wf, tpl.TriggerMetadata, "at://"+run.ID, "")
			if err == nil {
				// Swap the local clone for the remote one — the only step
				// that differs between a local and a served run.
				plan.Steps[1] = remoteCloneStep(twf, *tpl.TriggerMetadata)
				var results []stepResult
				results, err = runPlan(plan, opts)
				run.Results[twf.Name] = results
			}
		}
		if err != nil && firstErr == nil {
			firstErr = fmt.Errorf("%s: %w", twf.Name, err)
		}
	}

	now := time.Now()
	run.Finished = &now
	if firstErr != nil {
		run.Status = "failed"
		run.Error = firstErr.Error()
		return
	}
	run.Status = "success"
}

// remoteCloneStep is spindle's BuildCloneStep, against the knot URL the
// trigger names: init, remote add, fetch-by-sha at the requested depth,
// checkout FETCH_HEAD.
func remoteCloneStep(twf *tangled.Pipeline_Workflow, tr tangled.Pipeline_TriggerMetadata) Step {
	if twf.Clone != nil && twf.Clone.Skip {
		return Step{Name: "Clone repository into workspace (skipped)", System: true, Command: "true"}
	}
	sha := ""
	switch {
	case tr.Push != nil:
		sha = tr.Push.NewSha
	case tr.PullRequest != nil:
		sha = tr.PullRequest.SourceSha
	case tr.Manual != nil:
		sha = tr.Manual.Sha
	}
	url := ""
	if repo := tr.Repo; repo != nil && repo.RepoDid != nil {
		host := repo.Knot
		scheme := "https"
		// The `http://` knot spelling is how spindle marks a dev knot.
		if h, ok := strings.CutPrefix(host, "http://"); ok {
			host, scheme = h, "http"
		}
		url = fmt.Sprintf("%s://%s/%s", scheme, host, *repo.RepoDid)
	}
	depth := int64(1)
	fetch := []string{fmt.Sprintf("git fetch --depth=%d", depth)}
	if c := twf.Clone; c != nil {
		if c.Depth > 0 {
			fetch[0] = fmt.Sprintf("git fetch --depth=%d", c.Depth)
		}
		if c.Submodules {
			fetch = append(fetch, "--recurse-submodules=yes")
		}
		if c.Tags {
			fetch = append(fetch, "--tags")
		}
	}
	fetch = append(fetch, "origin", sha)
	return Step{
		Name:   "Clone repository into workspace",
		System: true,
		Command: strings.Join([]string{
			"git init -q",
			"git remote add origin " + url,
			strings.Join(fetch, " "),
			"git checkout -q FETCH_HEAD",
		}, "\n"),
	}
}

func (s *server) get(id string) *pipelineRun {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.runs[id]
}

func (s *server) getPipeline(w http.ResponseWriter, r *http.Request) {
	run := s.get(r.PathValue("id"))
	if run == nil {
		http.NotFound(w, r)
		return
	}
	json.NewEncoder(w).Encode(run)
}

func (s *server) getLogs(w http.ResponseWriter, r *http.Request) {
	run := s.get(r.PathValue("id"))
	if run == nil {
		http.NotFound(w, r)
		return
	}
	run.mu.Lock()
	logs := append([]byte(nil), run.logs...)
	run.mu.Unlock()
	w.Header().Set("Content-Type", "application/jsonl")
	w.Write(logs)
}
