//go:build spindle

package main

// These tests assert the shape of the spindle-compatible surface: the routes
// exist, and the ones that must refuse an unauthenticated caller do. They run
// against the real router — spindle's own handlers over our engine — so a
// change that quietly drops an endpoint or loosens auth fails here rather than
// in someone's swapped-out deployment.

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
)

func testServer(t *testing.T) *spindleServer {
	t.Helper()
	dir := t.TempDir()
	// Spindle's configuration is environment-only, so a test configures it the
	// same way an operator does.
	t.Setenv("SPINDLE_SERVER_HOSTNAME", "spindle.test:6555")
	t.Setenv("SPINDLE_SERVER_OWNER", "did:plc:owner")
	t.Setenv("SPINDLE_SERVER_DB_PATH", filepath.Join(dir, "spindle.db"))
	t.Setenv("SPINDLE_SERVER_LOG_DIR", filepath.Join(dir, "logs"))
	t.Setenv("SPINDLE_SERVER_REPO_DIR", filepath.Join(dir, "repos"))

	s, err := newSpindleServer(context.Background(),
		slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelError})), 1, 512)
	if err != nil {
		t.Fatalf("starting the server: %v", err)
	}
	return s
}

func TestSpindleSurface(t *testing.T) {
	s := testServer(t)
	mux := http.NewServeMux()
	s.Register(mux)
	srv := httptest.NewServer(mux)
	defer srv.Close()

	// The DID is derived from the hostname, port percent-encoded — every
	// service-auth token is minted for this audience, so getting it wrong
	// breaks every authenticated call.
	if got := s.cfg.Server.Did().String(); got != "did:web:spindle.test%3A6555" {
		t.Fatalf("service DID: %q", got)
	}

	t.Run("owner is public and returns the configured DID", func(t *testing.T) {
		resp, err := http.Get(srv.URL + "/xrpc/sh.tangled.owner")
		if err != nil {
			t.Fatal(err)
		}
		defer resp.Body.Close()
		if resp.StatusCode != 200 {
			t.Fatalf("status %d", resp.StatusCode)
		}
		var body struct {
			Owner string `json:"owner"`
		}
		if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body.Owner != "did:plc:owner" {
			t.Fatalf("owner: %q", body.Owner)
		}
	})

	t.Run("queryPipelines is public and validates its parameters", func(t *testing.T) {
		resp, err := http.Get(srv.URL + "/xrpc/sh.tangled.ci.queryPipelines")
		if err != nil {
			t.Fatal(err)
		}
		defer resp.Body.Close()
		if resp.StatusCode != 400 {
			t.Fatalf("missing repo should be 400, got %d", resp.StatusCode)
		}

		resp2, err := http.Get(srv.URL + "/xrpc/sh.tangled.ci.queryPipelines?repo=did:plc:x")
		if err != nil {
			t.Fatal(err)
		}
		defer resp2.Body.Close()
		if resp2.StatusCode != 200 {
			t.Fatalf("status %d", resp2.StatusCode)
		}
	})

	// Everything that can start work or touch secrets must refuse an
	// unauthenticated caller.
	for _, tc := range []struct {
		name, method, path string
	}{
		{"triggerPipeline", "POST", "/xrpc/sh.tangled.ci.triggerPipeline"},
		{"cancelPipeline", "POST", "/xrpc/sh.tangled.ci.cancelPipeline"},
		{"addSecret", "POST", "/xrpc/sh.tangled.repo.addSecret"},
		{"removeSecret", "POST", "/xrpc/sh.tangled.repo.removeSecret"},
		{"listSecrets", "GET", "/xrpc/sh.tangled.repo.listSecrets?repo=at://did:plc:x/sh.tangled.repo/a"},
	} {
		t.Run(tc.name+" requires service auth", func(t *testing.T) {
			req, err := http.NewRequest(tc.method, srv.URL+tc.path, nil)
			if err != nil {
				t.Fatal(err)
			}
			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			defer resp.Body.Close()
			if resp.StatusCode != http.StatusForbidden {
				t.Fatalf("unauthenticated %s should be 403, got %d", tc.name, resp.StatusCode)
			}
			var body struct {
				Error string `json:"error"`
			}
			if err := json.NewDecoder(resp.Body).Decode(&body); err != nil {
				t.Fatal(err)
			}
			if body.Error != "Auth" {
				t.Fatalf("error envelope: %q", body.Error)
			}
		})
	}

	// `/` is what a human sees when they open a spindle, so it serves
	// spindle's own greeting verbatim rather than something of our own.
	t.Run("motd is spindle's, verbatim", func(t *testing.T) {
		resp, err := http.Get(srv.URL + "/")
		if err != nil {
			t.Fatal(err)
		}
		defer resp.Body.Close()
		if resp.StatusCode != 200 {
			t.Fatalf("status %d", resp.StatusCode)
		}
		body, err := io.ReadAll(resp.Body)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Equal(body, defaultMotd) {
			t.Fatalf("/ must serve the embedded motd byte for byte, got %d bytes", len(body))
		}
		if !bytes.Contains(body, []byte("This is a spindle server")) {
			t.Fatal("the embedded motd lost its text")
		}
		if !bytes.Contains(body, []byte("****")) {
			t.Fatal("the embedded motd lost its ascii art")
		}
	})
}

// Every engine name a workflow in the wild might carry has to resolve, or the
// first pipeline after a swap fails as "unknown engine".
func TestSpindleEngineAliases(t *testing.T) {
	engines := enginesFor(newSpindleEngine(slog.Default(), 1, 512, 0))
	for _, name := range []string{"nixery", "microvm", "bsdkrun", "dummy"} {
		if engines[name] == nil {
			t.Fatalf("engine %q does not resolve", name)
		}
	}
}
