package bsdkrun

import (
	"errors"
	"fmt"
	"strings"
	"testing"
)

func TestCommandFailedMessageIncludesStderr(t *testing.T) {
	err := &CommandFailedError{ExitCode: 2, Stderr: "boom\n", Command: "bsdkrun stop"}
	msg := err.Error()
	if !strings.Contains(msg, "command failed (exit 2): bsdkrun stop") || !strings.Contains(msg, "boom") {
		t.Fatalf("message: %q", msg)
	}
	// A blank stderr adds nothing.
	quiet := &CommandFailedError{ExitCode: 1, Stderr: "  \n", Command: "x"}
	if strings.Contains(quiet.Error(), "\n") {
		t.Fatalf("message: %q", quiet.Error())
	}
}

func TestAuthErrorIsAGraphQLError(t *testing.T) {
	// Mirrors Python, where AuthError subclasses GraphQLError: an AuthError
	// must satisfy errors.As for *GraphQLError with code UNAUTHENTICATED.
	var authErr *AuthError
	var gqlErr *GraphQLError

	err := fmt.Errorf("request failed: %w", &AuthError{})
	if !errors.As(err, &authErr) {
		t.Fatal("errors.As(*AuthError) failed")
	}
	if !errors.As(err, &gqlErr) {
		t.Fatal("errors.As(*GraphQLError) failed for an AuthError")
	}
	if gqlErr.Code != "UNAUTHENTICATED" {
		t.Fatalf("code: %q", gqlErr.Code)
	}
}

func TestErrorTypesAreAsable(t *testing.T) {
	var binErr *BinaryNotFoundError
	if !errors.As(fmt.Errorf("wrap: %w", &BinaryNotFoundError{Searched: []string{"a"}}), &binErr) {
		t.Fatal("BinaryNotFoundError")
	}
	if !strings.Contains(binErr.Error(), "BSDKRUN_BIN") {
		t.Fatalf("message: %q", binErr.Error())
	}

	var nfErr *SandboxNotFoundError
	if !errors.As(fmt.Errorf("wrap: %w", &SandboxNotFoundError{ID: "abc"}), &nfErr) {
		t.Fatal("SandboxNotFoundError")
	}
	if !strings.Contains(nfErr.Error(), `"abc"`) {
		t.Fatalf("message: %q", nfErr.Error())
	}

	var gqlErr *GraphQLError
	if !errors.As(fmt.Errorf("wrap: %w", &GraphQLError{Message: "nope", Code: "FAILED"}), &gqlErr) {
		t.Fatal("GraphQLError")
	}
	if gqlErr.Code != "FAILED" {
		t.Fatalf("code: %q", gqlErr.Code)
	}
}
