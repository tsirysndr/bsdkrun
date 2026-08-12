package bsdkrun

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
)

// PortForward is a host->guest TCP port forward, e.g. 2222 -> 22. Bind is
// the host interface the forward is bound to ("127.0.0.1" by default, or
// "0.0.0.0" for a LAN-reachable forward).
type PortForward struct {
	Host  int    `json:"host"`
	Guest int    `json:"guest"`
	Bind  string `json:"bind"`
}

// String renders the forward as the CLI's "HOST:GUEST" shape.
func (p PortForward) String() string {
	return fmt.Sprintf("%d:%d", p.Host, p.Guest)
}

// Result is the captured result of running a command in a guest, returned
// by Sandbox.Exec and ExecBuilder.Run.
type Result struct {
	Stdout   string
	Stderr   string
	ExitCode int
	Command  string
}

// Ok reports whether the command succeeded (exit 0).
func (r *Result) Ok() bool {
	return r.ExitCode == 0
}

// Text is Stdout with trailing newlines trimmed — the common case.
func (r *Result) Text() string {
	return strings.TrimRight(r.Stdout, "\n")
}

// JSON parses Stdout as JSON into v.
func (r *Result) JSON(v any) error {
	return json.Unmarshal([]byte(r.Stdout), v)
}

// Lines returns the non-empty Stdout lines.
func (r *Result) Lines() []string {
	var out []string
	for _, line := range strings.Split(r.Stdout, "\n") {
		if line != "" {
			out = append(out, line)
		}
	}
	return out
}

// Err returns a *CommandFailedError if the command exited non-zero, nil
// otherwise — the Go rendering of Python's throw_if_failed().
func (r *Result) Err() error {
	if r.ExitCode != 0 {
		return &CommandFailedError{
			ExitCode: r.ExitCode,
			Stdout:   r.Stdout,
			Stderr:   r.Stderr,
			Command:  r.Command,
		}
	}
	return nil
}

// SandboxInfo is a machine as reported by `bsdkrun ps --json` (or the
// daemon's GraphQL Machine type). Nullable string fields (Name, Volume,
// Network, NetIP) arrive as "" when absent; nullable numbers are pointers.
type SandboxInfo struct {
	ID         string        `json:"id"`
	Name       string        `json:"name"`
	Image      string        `json:"image"`
	Kind       string        `json:"kind"`
	Command    string        `json:"command"`
	Status     string        `json:"-"`
	Running    bool          `json:"running"`
	ExitCode   *int          `json:"exit_code"`
	PID        *int          `json:"pid"`
	Detached   bool          `json:"detached"`
	Cpus       int           `json:"cpus"`
	Mem        int           `json:"mem"`
	Volume     string        `json:"volume"`
	StateDir   string        `json:"state_dir"`
	Network    string        `json:"network"`
	NetIP      string        `json:"net_ip"`
	Ports      []PortForward `json:"ports"`
	CreatedAt  int64         `json:"created_at"`
	FinishedAt *int64        `json:"finished_at"`
}

// normalize fills the fields the CLI's JSON rows leave implicit.
func (s *SandboxInfo) normalize() {
	if s.Status == "" {
		if s.Running {
			s.Status = "running"
		} else {
			s.Status = "exited"
		}
	}
	for i := range s.Ports {
		if s.Ports[i].Bind == "" {
			s.Ports[i].Bind = "127.0.0.1"
		}
	}
}

// ImageInfo is an image as reported by `bsdkrun images --json`.
type ImageInfo struct {
	ID        string `json:"id"`
	Reference string `json:"reference"`
	Digest    string `json:"digest"`
	Size      int64  `json:"size"`
	Rootfs    string `json:"rootfs"`
	CreatedAt int64  `json:"created_at"`
}

// VolumeInfo is a volume as reported by `bsdkrun volume ls --json`.
type VolumeInfo struct {
	Name      string `json:"name"`
	Guest     string `json:"guest"`
	Base      string `json:"base"`
	Path      string `json:"path"`
	Size      string `json:"size"`
	CreatedAt *int64 `json:"created_at"`
	Tracked   bool   `json:"tracked"`
}

// NetworkInfo is a global network as reported by `bsdkrun network ls --json`.
type NetworkInfo struct {
	Name      string `json:"name"`
	Subnet    string `json:"subnet"`
	Gateway   string `json:"gateway"`
	Members   int    `json:"members"`
	Running   int    `json:"running"`
	Up        bool   `json:"up"`
	CreatedAt *int64 `json:"created_at"`
}

// CommandResult is the outcome of a remote lifecycle mutation
// (stop/start/remove/update/commit), mirroring the GraphQL CommandResult
// type. A non-zero ExitCode is reported rather than turned into an error:
// for some underlying commands (`ssh status`, `tailscale status`) it is a
// legitimate state to display, not a transport failure.
type CommandResult struct {
	ExitCode int
	Stdout   string
	Stderr   string
}

// ShellSessionInfo is a shell session as reported by openShell /
// shellSessions.
type ShellSessionInfo struct {
	ID        string
	MachineID string
	Finished  bool
	Truncated bool
}

// ExecResult is the captured result of Client.Exec. Unlike Result (the
// local CLI's captured stdout/stderr as text), a remote exec's output is a
// single interleaved byte stream — the shell agent's shellOutput
// subscription does not distinguish stdout from stderr — so this carries
// raw bytes instead.
type ExecResult struct {
	ExitCode int
	Output   []byte
}

// Text returns Output as a string.
func (r *ExecResult) Text() string {
	return string(r.Output)
}

// ---------------------------------------------------------------------------
// GraphQL response coercion
//
// The schema is camelCase, and createdAt/finishedAt arrive as
// decimal-string unix timestamps (the daemon passes the CLI's own text
// through unchanged) rather than numbers — so responses are decoded into
// map[string]any and coerced field by field, same as the Python SDK.
// ---------------------------------------------------------------------------

func asMap(v any) map[string]any {
	m, _ := v.(map[string]any)
	return m
}

func asString(v any) string {
	s, _ := v.(string)
	return s
}

func asBool(v any) bool {
	b, _ := v.(bool)
	return b
}

// asInt coerces a JSON value (float64, string, json.Number, or int) to an
// int64, reporting whether it carried a number at all.
func asInt(v any) (int64, bool) {
	switch n := v.(type) {
	case float64:
		return int64(n), true
	case int:
		return int64(n), true
	case int64:
		return n, true
	case json.Number:
		i, err := n.Int64()
		return i, err == nil
	case string:
		i, err := strconv.ParseInt(strings.TrimSpace(n), 10, 64)
		return i, err == nil
	}
	return 0, false
}

func asIntPtr(v any) *int {
	if n, ok := asInt(v); ok {
		i := int(n)
		return &i
	}
	return nil
}

func asInt64Ptr(v any) *int64 {
	if n, ok := asInt(v); ok {
		return &n
	}
	return nil
}

func sandboxInfoFromGraphQL(m map[string]any) SandboxInfo {
	info := SandboxInfo{
		ID:       asString(m["id"]),
		Name:     asString(m["name"]),
		Image:    asString(m["image"]),
		Kind:     asString(m["kind"]),
		Command:  asString(m["command"]),
		Status:   asString(m["status"]),
		Running:  asBool(m["running"]),
		ExitCode: asIntPtr(m["exitCode"]),
		PID:      asIntPtr(m["pid"]),
		Detached: asBool(m["detached"]),
		Volume:   asString(m["volume"]),
		StateDir: asString(m["stateDir"]),
		Network:  asString(m["network"]),
		NetIP:    asString(m["netIp"]),
	}
	if n, ok := asInt(m["cpus"]); ok {
		info.Cpus = int(n)
	}
	if n, ok := asInt(m["mem"]); ok {
		info.Mem = int(n)
	}
	if n, ok := asInt(m["createdAt"]); ok {
		info.CreatedAt = n
	}
	info.FinishedAt = asInt64Ptr(m["finishedAt"])
	if ports, ok := m["ports"].([]any); ok {
		for _, p := range ports {
			pm := asMap(p)
			port := PortForward{Bind: asString(pm["bind"])}
			if n, ok := asInt(pm["host"]); ok {
				port.Host = int(n)
			}
			if n, ok := asInt(pm["guest"]); ok {
				port.Guest = int(n)
			}
			info.Ports = append(info.Ports, port)
		}
	}
	info.normalize()
	return info
}

func commandResultFromGraphQL(m map[string]any) *CommandResult {
	result := &CommandResult{
		Stdout: asString(m["stdout"]),
		Stderr: asString(m["stderr"]),
	}
	if n, ok := asInt(m["exitCode"]); ok {
		result.ExitCode = int(n)
	}
	return result
}

func shellSessionInfoFromGraphQL(m map[string]any) ShellSessionInfo {
	return ShellSessionInfo{
		ID:        asString(m["id"]),
		MachineID: asString(m["machineId"]),
		Finished:  asBool(m["finished"]),
		Truncated: asBool(m["truncated"]),
	}
}
