//go:build spindle

package main

// The input half of a spindle: how work arrives without anyone calling the
// API.
//
// Two independent streams feed it, and they answer different questions:
//
//   - The **jetstream firehose** answers "which repositories are mine?".
//     A `sh.tangled.repo` record naming this server in its `spindle` field is
//     an assignment; the repo is recorded, its owner gains the repo policies,
//     and its knot becomes a source to listen to. `sh.tangled.spindle.member`
//     records add members, but only when the record names this instance and
//     the DID that wrote it may invite.
//
//   - The **knot event stream** answers "did something happen?". Each known
//     knot is consumed over a WebSocket, and a `sh.tangled.git.refUpdate`
//     compiles the repo's workflows at the new SHA and runs the ones whose
//     `when:` matches a push. This is what makes a plain `git push` start a
//     pipeline, which is the whole point of a CI server.
//
// Both are spindle's own consumers (tangled.org/core/jetstream and
// /eventconsumer) with spindle's own semantics re-wired onto them, including
// the checks that matter for safety: an event is only honoured from the knot
// the repo actually lives on, and `skip-ci` push options are obeyed.

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
	"time"

	"net/http"
	"net/url"

	"github.com/bluesky-social/indigo/atproto/syntax"
	indigoxrpc "github.com/bluesky-social/indigo/xrpc"
	jsmodels "github.com/bluesky-social/jetstream/pkg/models"

	"tangled.org/core/api/tangled"
	avmodels "tangled.org/core/appview/models"
	"tangled.org/core/eventconsumer"
	"tangled.org/core/eventconsumer/cursor"
	"tangled.org/core/eventstream"
	"tangled.org/core/jetstream"
	knotdb "tangled.org/core/knotserver/db"
	kgit "tangled.org/core/knotserver/git"
	"tangled.org/core/spindle/db"
	spindlegit "tangled.org/core/spindle/git"
	"tangled.org/core/spindle/models"
	"tangled.org/core/workflow"
)

// startIngestion brings up both streams. It is best-effort by design: a
// server that cannot reach the firehose must still serve its API and run what
// it is told to run over XRPC, so failures here are logged, not fatal.
func (s *spindleServer) startIngestion(ctx context.Context) {
	if err := s.startKnotConsumer(ctx); err != nil {
		s.l.Error("knot event consumer not started — pushes will not trigger pipelines", "err", err)
	}
	if err := s.startJetstream(ctx); err != nil {
		s.l.Error("jetstream not started — new repo assignments will not be seen", "err", err)
	}
}

// startKnotConsumer listens to every knot this server already knows about.
// Knots learned later are added as their repos arrive.
func (s *spindleServer) startKnotConsumer(ctx context.Context) error {
	store, err := cursor.NewSQLiteStore(s.cfg.Server.DBPath)
	if err != nil {
		return fmt.Errorf("opening the cursor store: %w", err)
	}

	cfg := eventconsumer.NewConsumerConfig()
	cfg.Logger = s.l.With("component", "knots")
	cfg.ProcessFunc = s.processKnotEvent
	cfg.CursorStore = store
	// A knot that is down should be retried patiently in production and
	// impatiently in development, exactly as spindle does it.
	if s.cfg.Server.Dev {
		cfg.RetryInterval = 5 * time.Second
		cfg.MaxRetryInterval = 10 * time.Second
	} else {
		cfg.RetryInterval = time.Minute
		cfg.MaxRetryInterval = 10 * time.Minute
	}

	knots, err := s.db.Knots()
	if err != nil {
		return fmt.Errorf("listing knots: %w", err)
	}
	for _, knot := range knots {
		src := eventconsumer.NewKnotSource(knot)
		eventconsumer.MigrateLegacyCursor(store, src)
		cfg.Sources[src] = struct{}{}
		s.l.Info("listening to knot", "knot", knot)
	}

	s.knots = eventconsumer.NewConsumer(*cfg)
	go s.knots.Start(ctx)
	return nil
}

// processKnotEvent is the knot-side dispatch: pushes start pipelines, and
// collaborator changes update the ACL.
func (s *spindleServer) processKnotEvent(ctx context.Context, src eventconsumer.Source, msg eventstream.Event) error {
	switch msg.Nsid {
	case tangled.GitRefUpdateNSID:
		return s.onRefUpdate(ctx, src, msg)
	case knotdb.RepoCollaboratorUpdateNSID:
		return s.onKnotCollaborator(ctx, src, msg)
	}
	return nil
}

// onRefUpdate is the push path: verify the event really came from the repo's
// own knot, honour skip-ci, then compile and run.
func (s *spindleServer) onRefUpdate(ctx context.Context, src eventconsumer.Source, msg eventstream.Event) error {
	var event tangled.GitRefUpdate
	if err := json.Unmarshal(msg.EventJson, &event); err != nil {
		return fmt.Errorf("malformed refUpdate: %w", err)
	}
	l := s.l.With("repo", event.Repo, "ref", event.Ref, "newSha", event.NewSha)

	repoDid := syntax.DID(event.Repo)
	repo, err := s.db.GetRepoByDid(repoDid)
	if err != nil {
		return fmt.Errorf("unknown repo %s: %w", repoDid, err)
	}
	// A knot may only speak for the repos it hosts: without this check any
	// knot this server listens to could start a pipeline for any repo.
	if src.Host != repo.Knot {
		return fmt.Errorf("event source %s is not repo's knot %s", src.Host, repo.Knot)
	}
	if kgit.HasSkipCIPushOption(event.PushOptions) {
		l.Info("push asked for CI to be skipped")
		return nil
	}

	trigger := tangled.Pipeline_TriggerMetadata{
		Kind: string(workflow.TriggerKindPush),
		Push: &tangled.Pipeline_PushTriggerData{
			Ref:    event.Ref,
			OldSha: event.OldSha,
			NewSha: event.NewSha,
		},
		Repo: s.triggerRepo(ctx, repo),
	}

	pipelineId, err := s.runPipeline(ctx, repoDid, trigger, event.NewSha, nil)
	if err != nil {
		return err
	}
	if pipelineId.Rkey == "" {
		l.Info("no workflow matches a push trigger")
		return nil
	}
	l.Info("pipeline triggered", "pipeline", pipelineId.AtUri())
	return nil
}

// onKnotCollaborator applies a knot's own collaborator changes to the ACL, so
// a collaborator added on the knot can trigger pipelines here.
func (s *spindleServer) onKnotCollaborator(ctx context.Context, src eventconsumer.Source, msg eventstream.Event) error {
	var event knotdb.RepoCollaboratorUpdate
	if err := json.Unmarshal(msg.EventJson, &event); err != nil {
		return fmt.Errorf("malformed collaborator update: %w", err)
	}
	subject, err := syntax.ParseDID(event.Subject)
	if err != nil {
		s.l.Info("collaborator update has a malformed subject", "subject", event.Subject)
		return nil
	}
	repoDid, err := syntax.ParseDID(event.Repo)
	if err != nil {
		s.l.Info("collaborator update has a malformed repo", "repo", event.Repo)
		return nil
	}
	repo, err := s.db.GetRepoByDid(repoDid)
	if errors.Is(err, sql.ErrNoRows) {
		return nil // not a repo of ours
	}
	if err != nil {
		return fmt.Errorf("looking up repo %s: %w", repoDid, err)
	}
	// Same rule as pushes: a knot speaks only for the repos it hosts.
	if src.Host != repo.Knot {
		s.l.Warn("dropping collaborator update from a non-owning knot",
			"src", src.Host, "repoKnot", repo.Knot)
		return nil
	}

	switch event.Op {
	case knotdb.AclOpAdd:
		if err := s.enf.AddCollaborator(subject.String(), rbacDomain, repoDid.String()); err != nil {
			return fmt.Errorf("adding collaborator policy: %w", err)
		}
		if err := s.db.AddKnotCollaborator(repoDid, subject); err != nil {
			return fmt.Errorf("recording collaborator: %w", err)
		}
		s.l.Info("collaborator added", "repo", repoDid, "subject", subject)
	case knotdb.AclOpRemove:
		if err := s.enf.RemoveCollaborator(subject.String(), rbacDomain, repoDid.String()); err != nil {
			return fmt.Errorf("removing collaborator policy: %w", err)
		}
		if err := s.db.DeleteRepoCollaboratorBySubjectRepo(subject, repoDid); err != nil {
			return fmt.Errorf("deleting collaborator: %w", err)
		}
		s.l.Info("collaborator removed", "repo", repoDid, "subject", subject)
	default:
		return fmt.Errorf("unknown collaborator op %q", event.Op)
	}
	return nil
}

// triggerRepo assembles the metadata a pipeline record carries. The default
// branch is asked of the knot, because only the knot knows it; a knot that
// will not answer costs a field, not the run.
func (s *spindleServer) triggerRepo(ctx context.Context, repo *db.Repo) *tangled.Pipeline_TriggerRepo {
	scheme := "https"
	if s.cfg.Server.Dev {
		scheme = "http"
	}
	defaultBranch := ""
	client := &indigoxrpc.Client{Host: fmt.Sprintf("%s://%s", scheme, repo.Knot)}
	if out, err := tangled.RepoGetDefaultBranch(ctx, client, repo.RepoDid.String()); err == nil {
		defaultBranch = out.Name
	}
	rkey := string(repo.Rkey)
	repoDid := repo.RepoDid.String()
	return &tangled.Pipeline_TriggerRepo{
		Did:           repo.Owner.String(),
		Knot:          repo.Knot,
		Repo:          &rkey,
		RepoDid:       &repoDid,
		DefaultBranch: defaultBranch,
	}
}

// startJetstream consumes the AT Protocol firehose for the records that
// decide what this server is responsible for.
func (s *spindleServer) startJetstream(ctx context.Context) error {
	collections := []string{
		tangled.SpindleMemberNSID,
		tangled.RepoNSID,
		tangled.RepoCollaboratorNSID,
		tangled.RepoPullNSID,
	}
	jc, err := jetstream.NewJetstreamClient(
		s.cfg.Server.JetstreamEndpoint, "bsdkrun-spindle", collections, nil,
		s.l.With("component", "jetstream"), s.db, true, true)
	if err != nil {
		return fmt.Errorf("creating the jetstream client: %w", err)
	}
	// The firehose is filtered by DID: without this it would carry the whole
	// network. Everyone already known, plus the owner, is who we listen for.
	jc.AddDid(s.cfg.Server.Owner)
	// Pull requests are opened by people this server has never heard of, so
	// that collection is exempt from the DID filter — upstream does the same.
	jc.ExemptCollection(tangled.RepoPullNSID)
	if dids, err := s.db.GetAllDids(); err == nil {
		for _, did := range dids {
			jc.AddDid(did)
		}
	}
	if repos, err := s.db.AllRepos(); err == nil {
		for _, r := range repos {
			if r.Owner != "" {
				jc.AddDid(r.Owner.String())
			}
		}
	}
	s.jc = jc

	go func() {
		if err := jc.StartJetstream(ctx, s.processFirehose); err != nil {
			s.l.Error("jetstream stopped", "err", err)
		}
	}()
	return nil
}

func (s *spindleServer) processFirehose(ctx context.Context, e *jsmodels.Event) error {
	if e.Kind != jsmodels.EventKindCommit || e.Commit == nil {
		return nil
	}
	switch e.Commit.Collection {
	case tangled.RepoNSID:
		return s.onRepoRecord(ctx, e)
	case tangled.SpindleMemberNSID:
		return s.onMemberRecord(ctx, e)
	case tangled.RepoPullNSID:
		return s.onPullRecord(ctx, e)
	}
	return nil
}

// onRepoRecord is how a repository becomes ours: its record names this
// server's hostname in `spindle`. A record that names someone else is not an
// error — most of the firehose is other people's repositories.
func (s *spindleServer) onRepoRecord(ctx context.Context, e *jsmodels.Event) error {
	if e.Commit.Operation == jsmodels.CommitOperationDelete {
		return nil
	}
	var record tangled.Repo
	if err := json.Unmarshal(e.Commit.Record, &record); err != nil {
		return fmt.Errorf("malformed sh.tangled.repo: %w", err)
	}
	if record.Spindle == nil || !strings.EqualFold(*record.Spindle, s.cfg.Server.Hostname) {
		return nil
	}
	if record.RepoDid == nil || *record.RepoDid == "" {
		return fmt.Errorf("repo record %s names this spindle but carries no repoDid", e.Commit.RKey)
	}

	owner := syntax.DID(e.Did)
	repoDid := syntax.DID(*record.RepoDid)
	repo := db.Repo{
		Knot:      record.Knot,
		Owner:     owner,
		Rkey:      syntax.RecordKey(e.Commit.RKey),
		RepoDid:   repoDid,
		CreatedAt: time.Now().Format(time.RFC3339),
	}
	if err := s.db.AddRepo(repo); err != nil {
		return fmt.Errorf("recording repo %s: %w", repoDid, err)
	}
	if err := s.enf.AddRepo(owner.String(), rbacDomain, repoDid.String()); err != nil {
		return fmt.Errorf("granting repo policies: %w", err)
	}
	// Now that a repo lives on this knot, listen to it — otherwise its
	// pushes would never reach us.
	if s.knots != nil {
		s.knots.AddSource(ctx, eventconsumer.NewKnotSource(record.Knot))
	}
	if s.jc != nil {
		s.jc.AddDid(owner.String())
	}
	s.l.Info("repo assigned to this spindle", "repo", repoDid, "knot", record.Knot, "owner", owner)
	return nil
}

// onMemberRecord adds a spindle member. Two checks make this safe: the record
// must name this instance, and only a DID allowed to invite may write one —
// otherwise anyone could grant themselves membership by publishing a record.
func (s *spindleServer) onMemberRecord(ctx context.Context, e *jsmodels.Event) error {
	if e.Commit.Operation == jsmodels.CommitOperationDelete {
		return nil
	}
	var record tangled.SpindleMember
	if err := json.Unmarshal(e.Commit.Record, &record); err != nil {
		return fmt.Errorf("malformed sh.tangled.spindle.member: %w", err)
	}
	if !strings.EqualFold(record.Instance, s.cfg.Server.Hostname) {
		return nil
	}
	ok, err := s.enf.IsSpindleInviteAllowed(e.Did, rbacDomain)
	if err != nil {
		return fmt.Errorf("checking invite permission: %w", err)
	}
	if !ok {
		return fmt.Errorf("%s may not add members to this spindle", e.Did)
	}
	if err := s.enf.AddSpindleMember(rbacDomain, record.Subject); err != nil {
		return fmt.Errorf("adding member: %w", err)
	}
	if err := db.AddSpindleMember(s.db, db.SpindleMember{
		Did:     syntax.DID(e.Did),
		Rkey:    e.Commit.RKey,
		Subject: syntax.DID(record.Subject),
	}); err != nil {
		return fmt.Errorf("recording member: %w", err)
	}
	if s.jc != nil {
		s.jc.AddDid(record.Subject)
	}
	s.l.Info("spindle member added", "subject", record.Subject, "by", e.Did)
	return nil
}

// onPullRecord runs a pull request. Only branch-based pull requests against
// a repo of ours qualify: a patch-based one has no branch to check out, and a
// fork-based one would mean fetching from a repository this server has not
// been asked to trust.
func (s *spindleServer) onPullRecord(ctx context.Context, e *jsmodels.Event) error {
	if e.Commit.Operation == jsmodels.CommitOperationDelete {
		return nil
	}
	var record tangled.RepoPull
	if err := json.Unmarshal(e.Commit.Record, &record); err != nil {
		return fmt.Errorf("malformed sh.tangled.repo.pull: %w", err)
	}
	if record.Target == nil {
		return nil // legacy record, no target repo
	}
	if record.Source == nil || record.Source.Repo != nil {
		return nil // patch-based or fork-based
	}

	targetDid, err := syntax.ParseDID(record.Target.Repo)
	if err != nil {
		return nil
	}
	repo, err := s.db.GetRepoByDid(targetDid)
	if errors.Is(err, sql.ErrNoRows) {
		return nil // someone else's repo
	}
	if err != nil {
		return fmt.Errorf("looking up target repo %s: %w", targetDid, err)
	}

	// The SHA to build is the head of the pull's newest round, which lives in
	// a blob on the author's PDS rather than in the record.
	sourceSha, err := s.latestPullSha(ctx, e.Did, e.Commit.RKey, &record)
	if err != nil {
		return fmt.Errorf("resolving the pull's latest submission: %w", err)
	}

	pullUri := fmt.Sprintf("at://%s/%s/%s", e.Did, tangled.RepoPullNSID, e.Commit.RKey)
	trigger := tangled.Pipeline_TriggerMetadata{
		Kind: string(workflow.TriggerKindPullRequest),
		PullRequest: &tangled.Pipeline_PullRequestTriggerData{
			SourceBranch: record.Source.Branch,
			SourceSha:    sourceSha,
			TargetBranch: record.Target.Branch,
			Pull:         &pullUri,
		},
		Repo: s.triggerRepo(ctx, repo),
	}

	pipelineId, err := s.runPipeline(ctx, targetDid, trigger, sourceSha, nil)
	if err != nil {
		return err
	}
	if pipelineId.Rkey == "" {
		s.l.Info("no workflow matches a pull_request trigger", "pull", pullUri)
		return nil
	}
	s.l.Info("pipeline triggered by pull request", "pipeline", pipelineId.AtUri(), "pull", pullUri)
	return nil
}

// latestPullSha fetches the newest round's submission blob from the author's
// PDS and reads the revision it was cut from.
func (s *spindleServer) latestPullSha(ctx context.Context, did, rkey string, record *tangled.RepoPull) (string, error) {
	if len(record.Rounds) == 0 {
		return "", errors.New("pull record has no rounds")
	}
	ident, err := s.res.ResolveIdent(ctx, did)
	if err != nil {
		return "", fmt.Errorf("resolving %s: %w", did, err)
	}
	roundNumber := len(record.Rounds) - 1
	round := record.Rounds[roundNumber]
	if round == nil || round.PatchBlob == nil {
		return "", errors.New("pull round carries no patch blob")
	}

	blobURL, err := url.Parse(ident.PDSEndpoint() + "/xrpc/com.atproto.sync.getBlob")
	if err != nil {
		return "", err
	}
	q := blobURL.Query()
	q.Set("cid", round.PatchBlob.Ref.String())
	q.Set("did", did)
	blobURL.RawQuery = q.Encode()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, blobURL.String(), nil)
	if err != nil {
		return "", err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("fetching the submission blob: %w", err)
	}
	defer resp.Body.Close()

	submission, err := avmodels.PullSubmissionFromRecord(did, rkey, roundNumber, round, resp.Body)
	if err != nil {
		return "", fmt.Errorf("parsing the submission: %w", err)
	}
	if submission.SourceRev == "" {
		return "", errors.New("submission names no source revision")
	}
	return submission.SourceRev, nil
}

// syncRepo fetches a repo at a revision into the server's repo directory.
// Both the push path and the manual path go through it, so a pipeline always
// compiles from a checkout that actually contains the revision.
func (s *spindleServer) syncRepo(ctx context.Context, repo *tangled.Pipeline_TriggerRepo, rev string) (string, error) {
	cloneURL := models.BuildRepoURL(repo)
	if s.cfg.Server.Dev {
		cloneURL = strings.Replace(cloneURL, "https://", "http://", 1)
	}
	dir := s.repoPath(repo)
	if err := spindlegit.SparseSyncGitRepo(ctx, cloneURL, dir, rev); err != nil {
		return "", fmt.Errorf("syncing %s: %w", cloneURL, err)
	}
	return dir, nil
}
