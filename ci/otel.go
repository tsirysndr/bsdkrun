package main

// OpenTelemetry export for CI runs — hand-rolled OTLP/HTTP JSON, not the
// otel-go SDK. The SDK is a large dependency tree for what a CI runner needs:
// one trace per run, one span per step, POSTed to a collector as they end.
// OTLP's JSON encoding is stable and small enough to write by hand, and this
// stays a single file with zero new modules.
//
// Enabled by the standard environment variable — a collector at
// `$OTEL_EXPORTER_OTLP_ENDPOINT` receives every span at `/v1/traces` — or by
// `--otlp <url>`. Spans export when they end, so a collector's live view
// (Jaeger, Grafana, dagger's own cloud) fills in step by step as the run
// progresses; the workflow's root span lands last, closing the trace.

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

// otlpOverride is set by --otlp; it wins over the environment.
var otlpOverride string

func otlpEndpoint() string {
	if otlpOverride != "" {
		return otlpOverride
	}
	return os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT")
}

// Trace is one workflow run's trace: a root span plus a child per step.
type Trace struct {
	endpoint string
	traceID  string
	rootID   string
	service  string
	started  time.Time
	attrs    map[string]string
	// Everything recorded, for the engine's own SQLite — the local half of
	// the trace, kept whether or not a collector is configured.
	mu       sync.Mutex
	recorded []recordedSpan
}

// recordedSpan mirrors core's db::CiSpanRow.
type recordedSpan struct {
	TraceID  string  `json:"trace_id"`
	SpanID   string  `json:"span_id"`
	ParentID *string `json:"parent_id"`
	Name     string  `json:"name"`
	Workflow string  `json:"workflow"`
	Repo     string  `json:"repo"`
	StartNs  int64   `json:"start_ns"`
	EndNs    int64   `json:"end_ns"`
	Ok       bool    `json:"ok"`
	Error    *string `json:"error"`
}

// Span is one step (or the boot) within a trace.
type Span struct {
	trace   *Trace
	spanID  string
	name    string
	started time.Time
	attrs   map[string]string
}

// NewTrace starts a trace for one workflow run. Returns nil when no
// collector is configured — callers treat a nil *Trace as "tracing off",
// so the hot path stays free of conditionals.
func NewTrace(workflow, repo string) *Trace {
	return &Trace{
		endpoint: strings.TrimRight(otlpEndpoint(), "/"),
		traceID:  randHex(16),
		rootID:   randHex(8),
		service:  "bsdkrun-ci",
		started:  time.Now(),
		attrs: map[string]string{
			"ci.workflow": workflow,
			"ci.repo":     repo,
		},
	}
}

// StartSpan opens a child span; end it with `End`.
func (t *Trace) StartSpan(name string, attrs map[string]string) *Span {
	if t == nil {
		return nil
	}
	return &Span{
		trace:   t,
		spanID:  randHex(8),
		name:    name,
		started: time.Now(),
		attrs:   attrs,
	}
}

// End closes the span and exports it immediately — this is what makes a
// collector's view fill in live rather than all at once at the end.
func (s *Span) End(err error) {
	if s == nil {
		return
	}
	attrs := map[string]string{}
	for k, v := range s.trace.attrs {
		attrs[k] = v
	}
	for k, v := range s.attrs {
		attrs[k] = v
	}
	status := map[string]any{"code": 1} // OK
	if err != nil {
		status = map[string]any{"code": 2, "message": err.Error()}
		attrs["error"] = err.Error()
	}
	s.trace.post(s.spanID, s.trace.rootID, s.name, s.started, time.Now(), attrs, status)
}

// Finish closes the trace's root span. Call once, after the last step.
func (t *Trace) Finish(err error) {
	if t == nil {
		return
	}
	status := map[string]any{"code": 1}
	if err != nil {
		status = map[string]any{"code": 2, "message": err.Error()}
	}
	t.post(t.rootID, "", "ci.workflow "+t.attrs["ci.workflow"], t.started, time.Now(), t.attrs, status)

	// Persist the whole trace into the engine's SQLite through the hidden
	// `ci __record-trace` verb — that is what makes `bsdkrun ci traces` and
	// the daemon's trace queries work with no collector anywhere. Failure is
	// logged, never fatal: history must not fail the build it records.
	t.mu.Lock()
	batch, err := json.Marshal(t.recorded)
	t.mu.Unlock()
	if err != nil {
		return
	}
	bin := os.Getenv("BSDKRUN_BIN")
	if bin == "" {
		bin = "bsdkrun"
	}
	cmd := exec.Command(bin, "ci", "__record-trace")
	cmd.Stdin = bytes.NewReader(batch)
	if out, err := cmd.CombinedOutput(); err != nil {
		fmt.Fprintf(os.Stderr, "recording the trace: %v: %s\n", err, strings.TrimSpace(string(out)))
	}
}

func (t *Trace) post(
	spanID, parentID, name string,
	start, end time.Time,
	attrs map[string]string,
	status map[string]any,
) {
	// Local record first — it must not depend on any collector existing.
	var parent *string
	if parentID != "" {
		parent = &parentID
	}
	ok := status["code"] == 1
	var errMsg *string
	if m, has := status["message"].(string); has && m != "" {
		errMsg = &m
	}
	t.mu.Lock()
	t.recorded = append(t.recorded, recordedSpan{
		TraceID:  t.traceID,
		SpanID:   spanID,
		ParentID: parent,
		Name:     name,
		Workflow: t.attrs["ci.workflow"],
		Repo:     t.attrs["ci.repo"],
		StartNs:  start.UnixNano(),
		EndNs:    end.UnixNano(),
		Ok:       ok,
		Error:    errMsg,
	})
	t.mu.Unlock()
	if t.endpoint == "" {
		return
	}
	kv := make([]map[string]any, 0, len(attrs))
	for k, v := range attrs {
		kv = append(kv, map[string]any{
			"key":   k,
			"value": map[string]any{"stringValue": v},
		})
	}
	span := map[string]any{
		"traceId":           t.traceID,
		"spanId":            spanID,
		"name":              name,
		"kind":              1, // internal
		"startTimeUnixNano": fmt.Sprintf("%d", start.UnixNano()),
		"endTimeUnixNano":   fmt.Sprintf("%d", end.UnixNano()),
		"attributes":        kv,
		"status":            status,
	}
	if parentID != "" {
		span["parentSpanId"] = parentID
	}
	payload := map[string]any{
		"resourceSpans": []any{map[string]any{
			"resource": map[string]any{
				"attributes": []any{map[string]any{
					"key":   "service.name",
					"value": map[string]any{"stringValue": t.service},
				}},
			},
			"scopeSpans": []any{map[string]any{
				"scope": map[string]any{"name": "bsdkrun-ci"},
				"spans": []any{span},
			}},
		}},
	}
	body, err := json.Marshal(payload)
	if err != nil {
		return
	}
	// Fire-and-forget with a short timeout: a slow collector must never slow
	// the build it is observing, and a dead one must never fail it.
	go func() {
		client := &http.Client{Timeout: 5 * time.Second}
		resp, err := client.Post(
			t.endpoint+"/v1/traces",
			"application/json",
			bytes.NewReader(body),
		)
		if err == nil {
			resp.Body.Close()
		}
	}()
}

func randHex(n int) string {
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {
		// Degrade to a timestamp-derived id rather than failing tracing —
		// uniqueness suffers, the build does not.
		return fmt.Sprintf("%0*x", n*2, time.Now().UnixNano())
	}
	return hex.EncodeToString(b)
}
