//go:build spindle

package main

// `bsdkrun ci serve --spindle` — a spindle you can swap in.
//
// The compatibility claim here is structural rather than best-effort: the
// routes, the request and response shapes, the service-auth verification, the
// ACL, the SQLite schema, the event stream and the on-disk log format are all
// spindle's own code, imported from tangled.org/core. What bsdkrun replaces is
// the one seam spindle leaves open — models.Engine — so pipelines run in
// libkrun microVMs instead of qemu ones.
//
// What is deliberately not imported is the `spindle` package itself: it wires
// its own microvm engine, which needs qemu and Linux cgroups and does not
// build on darwin. Everything it does that lives above the engine is re-wired
// here, against the same packages it uses.
//
// Configuration is spindle's, read from the same environment variables, so an
// existing deployment's env file works unchanged. See ci/README.md.

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/bluesky-social/indigo/atproto/syntax"
	"github.com/hashicorp/go-version"
	"tangled.org/core/api/tangled"
	"tangled.org/core/eventstream"
	"tangled.org/core/idresolver"
	"tangled.org/core/notifier"
	"tangled.org/core/rbac"
	"tangled.org/core/spindle/config"
	"tangled.org/core/spindle/db"
	spindleengine "tangled.org/core/spindle/engine"
	spindlegit "tangled.org/core/spindle/git"
	"tangled.org/core/spindle/models"
	"tangled.org/core/spindle/queue"
	"tangled.org/core/spindle/secrets"
	"tangled.org/core/eventconsumer"
	"tangled.org/core/jetstream"
	spindlexrpc "tangled.org/core/spindle/xrpc"
	"tangled.org/core/tid"
	"tangled.org/core/workflow"
	"tangled.org/core/xrpc/serviceauth"
)

// rbacDomain is spindle's constant: every policy in the ACL is scoped to it.
const rbacDomain = rbac.ThisServer

const spindleBuilt = true

// startSpindle is the seam serve.go calls when spindle mode is asked for.
func startSpindle(sc *serveConfig) (spindleHandle, error) {
	l := slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelInfo}))
	return newSpindleServer(context.Background(), l, sc.Cpus, sc.Mem)
}

// serveConfig is what `ci serve` hands the spindle half.
type serveConfig struct {
	Cpus int
	Mem  int
}

// spindleHandle is what it gets back: routes to mount, plus what to print.
type spindleHandle interface {
	Register(mux *http.ServeMux)
	ListenAddr() string
	Banner() []string
}

func (s *spindleServer) ListenAddr() string { return s.cfg.Server.ListenAddr }

// Banner is the startup summary: what an operator needs to confirm the swap
// took — above all the DID, which is the audience every service-auth token
// must be minted for.
func (s *spindleServer) Banner() []string {
	return []string{
		fmt.Sprintf("  did          %s", s.cfg.Server.Did()),
		fmt.Sprintf("  owner        %s", s.cfg.Server.Owner),
		fmt.Sprintf("  database     %s", s.cfg.Server.DBPath),
		fmt.Sprintf("  logs         %s", s.cfg.Server.LogDir),
		fmt.Sprintf("  repos        %s", s.cfg.Server.RepoDir),
		fmt.Sprintf("  secrets      %s", s.cfg.Server.Secrets.Provider),
		fmt.Sprintf("  engines      %s (every one runs on libkrun)", strings.Join(engineNames(), ", ")),
		"  xrpc         /xrpc/sh.tangled.{owner,ci.*,repo.*Secret*}",
		"  events       /events        logs  /logs/{knot}/{rkey}/{name}",
		fmt.Sprintf("  ingest       jetstream %s", s.cfg.Server.JetstreamEndpoint),
		fmt.Sprintf("  knots        %s", s.knotSummary()),
	}
}

// gitFloor is the git version spindle requires: its sparse checkouts (and so
// this server's) need the newer `git clone --revision` behaviour.
var gitFloor = version.Must(version.NewVersion("2.49.0"))

type spindleServer struct {
	l     *slog.Logger
	cfg   *config.Config
	db    *db.DB
	enf   *rbac.Enforcer
	vault secrets.Manager
	n     *notifier.Notifier
	res   *idresolver.Resolver
	eng   models.Engine
	jq    *queue.Queue

	// The two input streams (see spindle_ingest.go). Nil until started, and
	// nil forever on a server that could not reach them — the API still works.
	knots *eventconsumer.Consumer
	jc    *jetstream.JetstreamClient
}

var _ spindlexrpc.PipelineTrigger = (*spindleServer)(nil)

// newSpindleServer brings up everything spindle's Run() does except its
// engines and its firehose consumers.
func newSpindleServer(ctx context.Context, l *slog.Logger, cpus, mem int) (*spindleServer, error) {
	cfg, err := config.Load(ctx)
	if err != nil {
		return nil, fmt.Errorf("loading config: %w", err)
	}

	// Spindle refuses to start without a modern git, because its sparse
	// checkouts need one; the same fetch happens here, so the same floor
	// applies — and saying so beats a confusing clone failure later.
	if v, err := spindlegit.Version(); err != nil {
		return nil, fmt.Errorf("checking git version: %w", err)
	} else if v.Core().LessThan(gitFloor) {
		return nil, fmt.Errorf("git %s is too old: spindle-compatible serving needs at least %s", v, gitFloor)
	}

	d, err := db.Make(ctx, cfg.Server.DBPath)
	if err != nil {
		return nil, fmt.Errorf("opening %s: %w", cfg.Server.DBPath, err)
	}
	enf, err := rbac.NewEnforcer(cfg.Server.DBPath)
	if err != nil {
		return nil, fmt.Errorf("opening the ACL in %s: %w", cfg.Server.DBPath, err)
	}
	enf.E.EnableAutoSave(true)
	if err := enf.AddSpindle(rbacDomain); err != nil {
		return nil, fmt.Errorf("seeding the ACL: %w", err)
	}
	if err := configureOwner(enf, cfg.Server.Owner); err != nil {
		return nil, err
	}

	var vault secrets.Manager
	switch cfg.Server.Secrets.Provider {
	case "", "sqlite":
		vault, err = secrets.NewSQLiteManager(cfg.Server.DBPath)
	case "openbao":
		if cfg.Server.Secrets.OpenBao.ProxyAddr == "" {
			return nil, errors.New("SPINDLE_SERVER_SECRETS_OPENBAO_PROXY_ADDR is required for the openbao provider")
		}
		vault, err = secrets.NewOpenBaoManager(cfg.Server.Secrets.OpenBao.ProxyAddr, l,
			secrets.WithMountPath(cfg.Server.Secrets.OpenBao.Mount))
	default:
		return nil, fmt.Errorf("unknown secrets provider: %s", cfg.Server.Secrets.Provider)
	}
	if err != nil {
		return nil, fmt.Errorf("opening the secret store: %w", err)
	}

	if err := os.MkdirAll(cfg.Server.LogDir, 0o755); err != nil {
		return nil, fmt.Errorf("creating log dir %s: %w", cfg.Server.LogDir, err)
	}

	n := notifier.New()
	timeout := 5 * time.Minute
	if t, err := time.ParseDuration(cfg.NixeryPipelines.WorkflowTimeout); err == nil && t > 0 {
		timeout = t
	}

	s := &spindleServer{
		l:     l,
		cfg:   cfg,
		db:    d,
		enf:   enf,
		vault: vault,
		n:     &n,
		res:   idresolver.DefaultResolver(cfg.Server.PlcUrl),
		eng:   newSpindleEngine(l, cpus, mem, timeout),
		jq:    queue.NewQueue(cfg.Server.QueueSize, cfg.Server.MaxJobCount),
	}
	s.jq.Start()

	// Both input streams come up here: without them the server answers its
	// API but never learns of a push, which is the difference between a
	// runner you call and a CI server.
	s.startIngestion(ctx)
	return s, nil
}

// configureOwner mirrors spindle's own rule: exactly one owner, replaced when
// the configured DID changes, and more than one already in the database is a
// refusal rather than a guess.
func configureOwner(enf *rbac.Enforcer, owner string) error {
	existing, err := enf.GetUserByRole("server:owner", rbacDomain)
	if err != nil {
		return fmt.Errorf("reading the ACL owner: %w", err)
	}
	if len(existing) > 1 {
		return fmt.Errorf("more than one owner in the ACL (%s) — delete the database and start over",
			strings.Join(existing, ", "))
	}
	if len(existing) == 1 && existing[0] == owner {
		return nil
	}
	return enf.AddSpindleOwner(rbacDomain, owner)
}

// Register installs spindle's route table onto mux. The XRPC half is
// spindle's own router, so those handlers — auth, validation, error envelopes
// and all — are the upstream ones rather than a reimplementation.
func (s *spindleServer) Register(mux *http.ServeMux) {
	x := spindlexrpc.Xrpc{
		Logger:      s.l.With("component", "xrpc"),
		Db:          s.db,
		Enforcer:    s.enf,
		Engines:     enginesFor(s.eng),
		Config:      s.cfg,
		Resolver:    s.res,
		Vault:       s.vault,
		Notifier:    s.n,
		ServiceAuth: serviceauth.NewServiceAuth(s.l, s.res.Directory(), s.cfg.Server.Did().String()),
		Trigger:     s,
	}

	mux.Handle("/xrpc/", http.StripPrefix("/xrpc", x.Router()))
	mux.HandleFunc("/events", s.events)
	mux.HandleFunc("/logs/{knot}/{rkey}/{name}", s.logs)
	mux.HandleFunc("/", s.motd)
}

func (s *spindleServer) motd(w http.ResponseWriter, r *http.Request) {
	fmt.Fprintf(w, "bsdkrun ci serve — a spindle-compatible runner on libkrun microVMs\n")
	fmt.Fprintf(w, "did: %s\n", s.cfg.Server.Did())
	fmt.Fprintf(w, "engines: %s\n", strings.Join(engineNames(), ", "))
}

// events is spindle's /events: a WebSocket replaying every pipeline and
// status event after `cursor` (unix nanoseconds), then following live.
func (s *spindleServer) events(w http.ResponseWriter, r *http.Request) {
	err := eventstream.Stream(w, r, eventstream.StreamConfig{
		Backend:  s.db,
		Notifier: s.n,
		Logger:   s.l.With("component", "events"),
	})
	if err != nil && !errors.Is(err, eventstream.ErrDrainCap) {
		s.l.Error("event stream ended", "err", err)
	}
}

// TriggerManual is the xrpc.PipelineTrigger seam: sh.tangled.ci.triggerPipeline
// has already resolved and authorised the repo, so this compiles the workflows
// at `sha`, records the pipeline (which is what makes it visible to
// queryPipelines, getPipeline and subscribePipelineLogs) and enqueues it.
func (s *spindleServer) TriggerManual(
	ctx context.Context,
	repoDid syntax.DID,
	sha, ref string,
	workflows []string,
	sourceRepo syntax.DID,
	pull spindlexrpc.PullContext,
	inputs []*tangled.Pipeline_Pair,
) (syntax.ATURI, error) {
	repo, err := s.db.GetRepoByDid(repoDid)
	if err != nil {
		return "", fmt.Errorf("looking up repo %s: %w", repoDid, err)
	}

	trigger := tangled.Pipeline_TriggerMetadata{
		Kind: string(workflow.TriggerKindManual),
		Repo: &tangled.Pipeline_TriggerRepo{
			Knot:    repo.Knot,
			Did:     repo.Owner.String(),
			Repo:    strPtr(string(repo.Rkey)),
			RepoDid: strPtr(repo.RepoDid.String()),
		},
		Manual: &tangled.Pipeline_ManualTriggerData{Sha: sha, Ref: strPtr(ref), Inputs: inputs},
	}
	if pull.IsPullRequest {
		trigger.Kind = string(workflow.TriggerKindPullRequest)
		trigger.Manual = nil
		trigger.PullRequest = &tangled.Pipeline_PullRequestTriggerData{
			SourceSha:    sha,
			SourceBranch: pull.SourceBranch,
			TargetBranch: pull.TargetBranch,
			Pull:         strPtr(string(pull.Pull)),
		}
	}
	if sourceRepo != "" {
		trigger.SourceRepo = strPtr(sourceRepo.String())
	}

	pipelineId, err := s.runPipeline(ctx, repoDid, trigger, sha, workflows)
	if err != nil {
		return "", err
	}
	if pipelineId.Rkey == "" {
		return "", spindlexrpc.ErrNoMatchingWorkflows
	}
	return pipelineId.AtUri(), nil
}

// runPipeline compiles the repo's workflows at `rev` and starts the ones the
// trigger selects. Compilation is tangled's own compiler, so `when:` matching
// and engine resolution behave exactly as they do on spindle.
func (s *spindleServer) runPipeline(
	ctx context.Context,
	repoDid syntax.DID,
	trigger tangled.Pipeline_TriggerMetadata,
	rev string,
	only []string,
) (models.PipelineId, error) {
	raw, err := s.loadPipeline(ctx, trigger.Repo, rev)
	if err != nil {
		return models.PipelineId{}, fmt.Errorf("loading pipeline: %w", err)
	}
	if len(raw) == 0 {
		return models.PipelineId{}, nil
	}

	compiler := workflow.Compiler{Trigger: trigger}
	tpl := compiler.Compile(compiler.Parse(raw))
	for _, d := range compiler.Diagnostics.Errors {
		s.l.Error("workflow compile error", "err", d.String())
	}
	for _, d := range compiler.Diagnostics.Warnings {
		s.l.Warn("workflow compile warning", "warn", d.String())
	}
	if len(only) > 0 {
		tpl.Workflows = filterWorkflowsByName(tpl.Workflows, only)
	}
	if len(tpl.Workflows) == 0 {
		return models.PipelineId{}, nil
	}

	pipelineId := models.PipelineId{Knot: trigger.Repo.Knot, Rkey: tid.TID()}
	if err := s.db.CreatePipelineEvent(pipelineId.Rkey, tpl, s.n); err != nil {
		return models.PipelineId{}, fmt.Errorf("recording the pipeline: %w", err)
	}
	if err := s.processPipeline(repoDid, tpl, pipelineId); err != nil {
		return pipelineId, err
	}
	return pipelineId, nil
}

// loadPipeline fetches the repo at `rev` and reads .tangled/workflows out of
// it. The clone is sparse and shallow — the workflow directory is all that is
// needed to decide what to run.
func (s *spindleServer) loadPipeline(ctx context.Context, repo *tangled.Pipeline_TriggerRepo, rev string) (workflow.RawPipeline, error) {
	if repo == nil {
		return nil, errors.New("trigger has no repo")
	}
	if err := os.MkdirAll(s.cfg.Server.RepoDir, 0o755); err != nil {
		return nil, err
	}
	dir, err := s.syncRepo(ctx, repo, rev)
	if err != nil {
		return nil, err
	}

	entries, err := os.ReadDir(filepath.Join(dir, workflow.WorkflowDir))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil // a repo with no workflows is not an error
	} else if err != nil {
		return nil, err
	}
	var raw workflow.RawPipeline
	for _, e := range entries {
		if e.IsDir() {
			continue
		}
		contents, err := os.ReadFile(filepath.Join(dir, workflow.WorkflowDir, e.Name()))
		if err != nil {
			return nil, fmt.Errorf("reading %s: %w", e.Name(), err)
		}
		raw = append(raw, workflow.RawWorkflow{Name: e.Name(), Contents: contents})
	}
	return raw, nil
}

// processPipeline enqueues the compiled workflows and emits the pending
// statuses, in spindle's order: enqueue first, announce after, so a full queue
// never leaves a pipeline advertised as pending forever.
func (s *spindleServer) processPipeline(repoDid syntax.DID, tpl tangled.Pipeline, pipelineId models.PipelineId) error {
	trusted := true
	if tm := tpl.TriggerMetadata; tm != nil && tm.SourceRepo != nil &&
		*tm.SourceRepo != "" && *tm.SourceRepo != repoDid.String() {
		trusted = false
	}

	pipeline := &models.Pipeline{
		RepoDid:       repoDid,
		TrustedSource: trusted,
		Workflows:     map[models.Engine][]models.Workflow{},
	}
	env := models.PipelineEnvVars(tpl.TriggerMetadata, pipelineId)

	for _, twf := range tpl.Workflows {
		if twf == nil {
			continue
		}
		wid := models.WorkflowId{PipelineId: pipelineId, Name: twf.Name}
		mwf, err := s.eng.InitWorkflow(*twf, tpl)
		if err != nil {
			s.l.Error("initialising workflow", "workflow", twf.Name, "err", err)
			if serr := s.db.StatusFailed(wid, err.Error(), -1, s.n); serr != nil {
				s.l.Error("recording failure", "err", serr)
			}
			continue
		}
		if mwf.Environment == nil {
			mwf.Environment = map[string]string{}
		}
		for k, v := range env {
			mwf.Environment[k] = v
		}
		pipeline.Workflows[s.eng] = append(pipeline.Workflows[s.eng], *mwf)
	}
	if len(pipeline.Workflows) == 0 {
		return nil
	}

	ok := s.jq.Enqueue(repoDid, queue.Job{
		Run: func() error {
			spindleengine.StartWorkflows(s.l, s.vault, s.cfg, s.db, s.n, context.Background(), pipeline, pipelineId)
			return nil
		},
		OnFail: func(jobErr error) {
			s.l.Error("pipeline job failed", "pipeline", pipelineId.Rkey, "err", jobErr)
		},
	})
	if !ok {
		return errors.New("failed to enqueue pipeline: queue is full")
	}

	for _, wfs := range pipeline.Workflows {
		for _, wf := range wfs {
			wid := models.WorkflowId{PipelineId: pipelineId, Name: wf.Name}
			if err := s.db.StatusPending(wid, s.n); err != nil {
				return fmt.Errorf("db.StatusPending: %w", err)
			}
		}
	}
	return nil
}

// repoPath is where a repo is checked out: one directory per repo, named for
// the knot and rkey so two repos of the same name on different knots cannot
// collide.
func (s *spindleServer) repoPath(repo *tangled.Pipeline_TriggerRepo) string {
	slug := repo.Knot
	if repo.Repo != nil {
		slug += "-" + *repo.Repo
	}
	return filepath.Join(s.cfg.Server.RepoDir,
		strings.NewReplacer("/", "-", ":", "-").Replace(slug))
}

// knotSummary reports which knots are being listened to, because "no knots"
// is the quiet reason a push never starts anything.
func (s *spindleServer) knotSummary() string {
	knots, err := s.db.Knots()
	if err != nil || len(knots) == 0 {
		return "none yet — a repo assigned to this spindle brings its knot with it"
	}
	return strings.Join(knots, ", ")
}

func filterWorkflowsByName(workflows []*tangled.Pipeline_Workflow, only []string) []*tangled.Pipeline_Workflow {
	allowed := make(map[string]struct{}, len(only))
	for _, n := range only {
		allowed[n] = struct{}{}
	}
	var out []*tangled.Pipeline_Workflow
	for _, w := range workflows {
		if w == nil {
			continue
		}
		if _, ok := allowed[w.Name]; ok {
			out = append(out, w)
		}
	}
	return out
}

func strPtr(s string) *string { return &s }
