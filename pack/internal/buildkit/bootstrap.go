// Package buildkit drives BuildKit directly (its Go client + LLB API, no
// generated Dockerfile) to turn a project into a rootfs directory, and
// manages the buildkitd instance it talks to.
package buildkit

import (
	"context"
	"fmt"
	"net"
	"os/exec"
	"strings"
	"time"

	bkclient "github.com/moby/buildkit/client"
)

// containerName is fixed and reused across invocations (and across
// projects): bootstrapping a buildkitd container is expensive enough
// (~seconds, an image pull on first use) that every `bsdkrun pack` run
// should share one rather than starting a fresh one.
const containerName = "bsdkrun-pack-buildkitd"

// buildkitPort is the port buildkitd listens on *inside* the container.
// Published to an ephemeral host port (looked up after start) rather than a
// fixed one, so a second unrelated process already on some hardcoded port
// can't collide with it.
const buildkitPort = "1234"

// buildkitImage is unpinned-by-tag-only deliberately: BuildKit's client/server
// protocol is what has to match, and the Go client here (moby/buildkit
// v0.17.3) negotiates it, not the calling code — a `:latest` daemon is safe
// to run against an older client.
const buildkitImage = "moby/buildkit:latest"

// Bootstrap ensures a buildkitd container is running with its control port
// published to loopback, and returns the address to dial (tcp://127.0.0.1:port).
// Reuses an existing container (starting it if stopped) rather than always
// creating a new one.
//
// This is the same technique Earthly/Dagger use to get a BuildKit endpoint
// from nothing but a working `docker`: it sidesteps depending on buildx's
// internal driver/container naming, and only needs `--privileged` (BuildKit
// runs its own nested containers/namespaces for build steps).
//
// TCP rather than a bind-mounted unix socket: buildkitd `chmod`s its socket
// on startup, and that fails with "invalid argument" when the socket file
// lives on a host bind mount through Docker Desktop's virtiofs on macOS.
// Keeping the socket inside the container's own filesystem and publishing a
// loopback-only TCP port instead sidesteps that — and is identical on Linux.
func Bootstrap(ctx context.Context, cacheDir string) (string, error) {
	_ = cacheDir // no longer needed for a bind mount, kept for API stability

	if _, err := exec.LookPath("docker"); err != nil {
		return "", fmt.Errorf("bsdkrun pack needs docker on PATH to run BuildKit: %w", err)
	}

	running, err := containerRunning(ctx)
	if err != nil {
		return "", err
	}
	if !running {
		exists, err := containerExists(ctx)
		if err != nil {
			return "", err
		}
		if exists {
			if out, err := dockerOutput(ctx, "start", containerName); err != nil {
				return "", fmt.Errorf("starting %s: %w\n%s", containerName, err, out)
			}
		} else {
			if out, err := dockerOutput(ctx, "run", "-d",
				"--name", containerName,
				"--privileged",
				"-p", "127.0.0.1::"+buildkitPort+"/tcp",
				buildkitImage,
				"--addr", "tcp://0.0.0.0:"+buildkitPort,
			); err != nil {
				return "", fmt.Errorf("starting buildkitd container: %w\n%s", err, out)
			}
		}
	}

	hostPort, err := publishedPort(ctx)
	if err != nil {
		return "", fmt.Errorf("finding buildkitd's published port: %w", err)
	}
	addr := "127.0.0.1:" + hostPort

	if err := waitForConnect(addr, 30*time.Second); err != nil {
		return "", fmt.Errorf(
			"buildkitd container %q never accepted a connection on %s: %w (check `docker logs %s`)",
			containerName, addr, err, containerName,
		)
	}

	// Accepting a TCP connection is not the same as being ready to serve:
	// Docker's port forwarder accepts on the host side as soon as the
	// container exists, well before buildkitd is listening inside it. Going
	// straight to Solve() at that point fails with a bare gRPC
	// "Unavailable" / "error reading server preface: EOF" that looks like a
	// build error rather than a startup race. Ask the daemon something real
	// until it answers.
	endpoint := "tcp://" + addr
	if err := waitForReady(ctx, endpoint, 60*time.Second); err != nil {
		return "", fmt.Errorf(
			"buildkitd container %q accepted a connection on %s but never served gRPC: %w "+
				"(check `docker logs %s`)",
			containerName, addr, err, containerName,
		)
	}

	return endpoint, nil
}

// waitForReady polls buildkitd with a real RPC (ListWorkers) until it
// answers. gRPC dials lazily, so constructing a client proves nothing —
// only a round trip does.
func waitForReady(ctx context.Context, endpoint string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	var lastErr error
	for {
		c, err := bkclient.New(ctx, endpoint)
		if err == nil {
			_, err = c.ListWorkers(ctx)
			c.Close()
			if err == nil {
				return nil
			}
		}
		lastErr = err
		if time.Now().After(deadline) {
			return fmt.Errorf("timed out after %s: %w", timeout, lastErr)
		}
		time.Sleep(500 * time.Millisecond)
	}
}

func containerRunning(ctx context.Context) (bool, error) {
	out, err := dockerOutput(ctx, "inspect", "-f", "{{.State.Running}}", containerName)
	if err != nil {
		// `docker inspect` on a container that doesn't exist exits non-zero;
		// that's "not running", not an error worth surfacing.
		return false, nil
	}
	return strings.TrimSpace(out) == "true", nil
}

func containerExists(ctx context.Context) (bool, error) {
	_, err := dockerOutput(ctx, "inspect", containerName)
	return err == nil, nil
}

// publishedPort looks up the host port Docker mapped to buildkitPort. Looked
// up fresh on every call rather than cached: an ephemeral port assignment
// only lasts as long as the container that got it, and this container may
// have been (re)started by a previous `bsdkrun pack` run under a different
// mapping.
func publishedPort(ctx context.Context) (string, error) {
	out, err := dockerOutput(ctx, "port", containerName, buildkitPort+"/tcp")
	if err != nil {
		return "", fmt.Errorf("%w\n%s", err, out)
	}
	// e.g. "127.0.0.1:54321" (possibly with a trailing "[::1]:54321" line too).
	line := strings.TrimSpace(strings.Split(out, "\n")[0])
	_, port, err := net.SplitHostPort(line)
	if err != nil {
		return "", fmt.Errorf("unexpected `docker port` output %q: %w", out, err)
	}
	return port, nil
}

func dockerOutput(ctx context.Context, args ...string) (string, error) {
	cmd := exec.CommandContext(ctx, "docker", args...)
	out, err := cmd.CombinedOutput()
	return string(out), err
}

func waitForConnect(addr string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for {
		conn, err := net.DialTimeout("tcp", addr, time.Second)
		if err == nil {
			conn.Close()
			return nil
		}
		if time.Now().After(deadline) {
			return fmt.Errorf("timed out after %s: %w", timeout, err)
		}
		time.Sleep(200 * time.Millisecond)
	}
}
