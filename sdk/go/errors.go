package bsdkrun

import (
	"fmt"
	"strings"
)

// BinaryNotFoundError reports that the bsdkrun binary could not be located
// on the host. Searched lists every candidate path that was tried.
type BinaryNotFoundError struct {
	Searched []string
}

func (e *BinaryNotFoundError) Error() string {
	return `could not find the "bsdkrun" binary. Set BSDKRUN_BIN, add it to ` +
		"PATH, or call SetBinaryPath(). Looked in: " + strings.Join(e.Searched, ", ")
}

// CommandFailedError reports a bsdkrun invocation that exited non-zero. It
// carries the process exit code and the captured stdout/stderr.
type CommandFailedError struct {
	ExitCode int
	Stdout   string
	Stderr   string
	Command  string
}

func (e *CommandFailedError) Error() string {
	message := fmt.Sprintf("command failed (exit %d): %s", e.ExitCode, e.Command)
	if s := strings.TrimSpace(e.Stderr); s != "" {
		message += "\n" + s
	}
	return message
}

// SandboxNotFoundError reports that no machine matched the given id/prefix.
type SandboxNotFoundError struct {
	ID string
}

func (e *SandboxNotFoundError) Error() string {
	return fmt.Sprintf("no sandbox found matching id %q", e.ID)
}

// GraphQLError reports a GraphQL- or transport-level failure talking to a
// remote bsdkrund. Code carries the response's extensions.code when the
// daemon set one (e.g. "INVALID_ARGUMENT", "FAILED"); it is empty for a
// transport failure (the daemon was unreachable) or a malformed response.
type GraphQLError struct {
	Message string
	Code    string
}

func (e *GraphQLError) Error() string {
	return e.Message
}

// AuthError reports that the daemon rejected the bearer token: an HTTP 401,
// a GraphQL error with extensions.code == "UNAUTHENTICATED", or the
// WebSocket closing before connection_ack was ever received.
//
// It unwraps to a *GraphQLError with Code "UNAUTHENTICATED", so
// errors.As(err, &gqlErr) matches it too — mirroring the Python SDK, where
// AuthError subclasses GraphQLError.
type AuthError struct {
	Message string
}

func (e *AuthError) Error() string {
	if e.Message == "" {
		return "the daemon rejected this token"
	}
	return e.Message
}

func (e *AuthError) Unwrap() error {
	return &GraphQLError{Message: e.Error(), Code: "UNAUTHENTICATED"}
}
