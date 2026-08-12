package bsdkrun

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strconv"
)

// RawResult is the buffered result of a bsdkrun invocation.
type RawResult struct {
	Stdout   string
	Stderr   string
	ExitCode int
}

// RunOpts tunes a low-level Run/RunChecked/Spawn invocation.
type RunOpts struct {
	// Env is merged onto the current process environment.
	Env map[string]string
	// Stdin, if non-nil, is piped to the child.
	Stdin []byte
	// LogLevel sets bsdkrun's global --log-level (0 — quiet — by default,
	// so the SDK's captured output stays clean; raise it for boot
	// diagnostics).
	LogLevel int
}

// withGlobals prepends the global --log-level flag to every invocation.
func withGlobals(args []string, opts *RunOpts) []string {
	level := 0
	if opts != nil {
		level = opts.LogLevel
	}
	return append([]string{"--log-level", strconv.Itoa(level)}, args...)
}

func childEnv(opts *RunOpts) []string {
	env := os.Environ()
	if opts != nil {
		for key, value := range opts.Env {
			env = append(env, key+"="+value)
		}
	}
	return env
}

// Run runs `bsdkrun <args>` to completion, buffering stdout/stderr. A
// non-zero exit is reported in the RawResult, not as an error; the error is
// only non-nil when the binary could not be found or started at all.
func Run(args []string, opts *RunOpts) (RawResult, error) {
	binary, err := ResolveBinary()
	if err != nil {
		return RawResult{}, err
	}

	cmd := exec.Command(binary, withGlobals(args, opts)...)
	cmd.Env = childEnv(opts)
	if opts != nil && opts.Stdin != nil {
		cmd.Stdin = bytes.NewReader(opts.Stdin)
	}
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	runErr := cmd.Run()
	result := RawResult{Stdout: stdout.String(), Stderr: stderr.String()}
	if runErr != nil {
		var exitErr *exec.ExitError
		if errors.As(runErr, &exitErr) {
			result.ExitCode = exitErr.ExitCode()
			return result, nil
		}
		return result, fmt.Errorf("failed to run %s: %w", binary, runErr)
	}
	return result, nil
}

// RunChecked is like Run, but returns a *CommandFailedError on a non-zero
// exit. label names the invocation in the error message.
func RunChecked(args []string, label string, opts *RunOpts) (RawResult, error) {
	result, err := Run(args, opts)
	if err != nil {
		return result, err
	}
	if result.ExitCode != 0 {
		return result, &CommandFailedError{
			ExitCode: result.ExitCode,
			Stdout:   result.Stdout,
			Stderr:   result.Stderr,
			Command:  label,
		}
	}
	return result, nil
}

// Spawn runs `bsdkrun <args>` inheriting the parent's stdio (interactive).
// It blocks until the child exits and returns its exit code. Used by
// Sandbox.Shell.
func Spawn(args []string, opts *RunOpts) (int, error) {
	binary, err := ResolveBinary()
	if err != nil {
		return 0, err
	}
	cmd := exec.Command(binary, withGlobals(args, opts)...)
	cmd.Env = childEnv(opts)
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	runErr := cmd.Run()
	if runErr != nil {
		var exitErr *exec.ExitError
		if errors.As(runErr, &exitErr) {
			return exitErr.ExitCode(), nil
		}
		return 0, fmt.Errorf("failed to run %s: %w", binary, runErr)
	}
	return 0, nil
}
