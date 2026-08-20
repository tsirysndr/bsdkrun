package main

// The thin bsdkrun driver — create / exec / remove over the CLI, inlined.
//
// This used to be the Go SDK, pulled in through a `replace ../sdk/go`. That
// replace was a trap under nix: buildGoModule's fixed-output vendor step
// copies the replaced directory into vendor/ **at vendorHash-pin time**, and
// the build then compiles against that frozen copy — so any SDK change
// invisible to go.sum (which is every change, for a path replace) silently
// pinned the old SDK. It shipped one real breakage (`CreateStreaming
// undefined` in CI while the method sat in the same commit) before being
// removed.
//
// What the runner needs is small enough to own outright: boot a VM streaming
// its output, exec a command with env, remove the VM. Argv in, lines out —
// the flags are the CLI's public, stable surface, the same contract every
// other SDK builds on.

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"os/exec"
	"regexp"
	"strconv"
	"strings"
	"time"
)

var machineIDRe = regexp.MustCompile(`^[0-9a-f]{12}$`)

// bsdkrunBin resolves the CLI to drive: the binary that exec'd this tool
// ($BSDKRUN_BIN, set by `bsdkrun ci`), or whatever PATH offers.
func bsdkrunBin() string {
	if b := os.Getenv("BSDKRUN_BIN"); b != "" {
		return b
	}
	return "bsdkrun"
}

// vm is one booted machine.
type vm struct {
	ID string
}

// createVM boots `image` detached, streaming the boot's own output (image
// pull, extraction, boot log) to the writers while it runs. Returns once the
// machine id is known.
func createVM(
	image, name string,
	cpus, mem int,
	mounts []string,
	disks []string,
	command []string,
	stdout, stderr io.Writer,
) (*vm, error) {
	args := []string{
		"--log-level", "1", "linux", image, "-d",
		"--name", name,
		"--cpus", strconv.Itoa(cpus),
		"--mem", strconv.Itoa(mem),
	}
	for _, m := range mounts {
		args = append(args, "--mount", m)
	}
	for _, d := range disks {
		args = append(args, "--attach-disk", d)
	}
	if len(command) > 0 {
		args = append(args, "--")
		args = append(args, command...)
	}

	// Captured *and* streamed: the writers get the live view, the buffers are
	// what the machine id is parsed from afterwards.
	var outBuf, errBuf bytes.Buffer
	cmd := exec.Command(bsdkrunBin(), args...)
	cmd.Stdout = io.MultiWriter(&outBuf, stdout)
	cmd.Stderr = io.MultiWriter(&errBuf, stderr)
	if err := cmd.Run(); err != nil {
		return nil, fmt.Errorf("bsdkrun create: %w", err)
	}

	// Detached runs print the machine id on stdout; take the last id-shaped
	// line, in case boot noise precedes it.
	var id string
	for _, line := range strings.Split(outBuf.String(), "\n") {
		if s := strings.TrimSpace(line); machineIDRe.MatchString(s) {
			id = s
		}
	}
	if id == "" {
		return nil, fmt.Errorf("bsdkrun create: no machine id in output")
	}
	return &vm{ID: id}, nil
}

// waitReady blocks until the guest agent answers an exec, or the deadline
// passes. `bsdkrun linux -d` returns when the VM is *booted*, which is not
// yet a VM you can exec into — connecting in that window fails with "the
// guest agent accepted the connection but sent no output". A slow boot (an
// image pull) used to hide the race; a cached rootfs boots fast enough to
// lose it every time, which is exactly how it was found.
func (v *vm) waitReady(timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	var last string
	for time.Now().Before(deadline) {
		res, err := v.exec([]string{"true"}, nil, nil, nil)
		if err == nil && res.ExitCode == 0 {
			return nil
		}
		if err != nil {
			last = err.Error()
		} else {
			last = strings.TrimSpace(res.Stderr)
		}
		time.Sleep(300 * time.Millisecond)
	}
	return fmt.Errorf("the guest agent did not come up within %s: %s", timeout, last)
}

// execResult is one guest command's outcome.
type execResult struct {
	Stdout   string
	Stderr   string
	ExitCode int
}

// exec runs argv in the guest with `env`, capturing output and, when writers
// are given, streaming it live as well. A non-zero guest exit is a result,
// not an error — the caller decides what a failing step means.
func (v *vm) exec(
	argv []string,
	env map[string]string,
	stdout, stderr io.Writer,
) (*execResult, error) {
	args := []string{"--log-level", "0", "exec"}
	for k, val := range env {
		args = append(args, "-e", k+"="+val)
	}
	args = append(args, v.ID, "--")
	args = append(args, argv...)

	var outBuf, errBuf bytes.Buffer
	var wOut io.Writer = &outBuf
	var wErr io.Writer = &errBuf
	if stdout != nil {
		wOut = io.MultiWriter(&outBuf, stdout)
	}
	if stderr != nil {
		wErr = io.MultiWriter(&errBuf, stderr)
	}
	cmd := exec.Command(bsdkrunBin(), args...)
	cmd.Stdout = wOut
	cmd.Stderr = wErr
	err := cmd.Run()
	res := &execResult{Stdout: outBuf.String(), Stderr: errBuf.String()}
	if err != nil {
		var exitErr *exec.ExitError
		if ok := errorsAs(err, &exitErr); ok {
			res.ExitCode = exitErr.ExitCode()
			return res, nil
		}
		return res, fmt.Errorf("bsdkrun exec: %w", err)
	}
	return res, nil
}

// remove force-removes the machine and its state.
func (v *vm) remove() error {
	out, err := exec.Command(bsdkrunBin(), "--log-level", "0", "rm", "-f", v.ID).CombinedOutput()
	if err != nil {
		return fmt.Errorf("bsdkrun rm %s: %v: %s", v.ID, err, strings.TrimSpace(string(out)))
	}
	return nil
}

// errorsAs is errors.As without importing errors for one call site.
func errorsAs(err error, target *(*exec.ExitError)) bool {
	e, ok := err.(*exec.ExitError)
	if ok {
		*target = e
	}
	return ok
}
