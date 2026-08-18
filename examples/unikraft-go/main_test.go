package main

import (
	"net/http/httptest"
	"strings"
	"testing"
)

// The handlers are what the unikernel serves; test them exactly as the e2e
// workflow asserts them, without a listener.
func TestHello(t *testing.T) {
	rec := httptest.NewRecorder()
	hello(rec, httptest.NewRequest("GET", "/", nil))
	if got := rec.Body.String(); got != "Bye, World!\r\n" {
		t.Fatalf("hello body: %q", got)
	}
}

func TestHey(t *testing.T) {
	rec := httptest.NewRecorder()
	hey(rec, httptest.NewRequest("GET", "/hey", nil))
	if got := rec.Body.String(); got != "Buh bye!" {
		t.Fatalf("hey body: %q", got)
	}
}

func TestEcho(t *testing.T) {
	rec := httptest.NewRecorder()
	echo(rec, httptest.NewRequest("POST", "/echo", strings.NewReader("ping")))
	if got := rec.Body.String(); got != "ping" {
		t.Fatalf("echo body: %q", got)
	}
}
