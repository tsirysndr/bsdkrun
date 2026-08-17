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
	// Origin is the snapshot this machine was branched from, if any.
	Origin string `json:"origin"`
}

// SnapshotInfo is a machine snapshot: one machine's disk state, captured
// under a name.
//
// A copy-on-write clone rather than a memory image — the files the guest
// wrote, not what it was executing. Client.Branch boots a new machine from
// one; Client.Restore puts one back over the machine it came from.
type SnapshotInfo struct {
	ID   string `json:"id"`
	Name string `json:"name"`
	// MachineID is the machine it was taken from; MachineName is what that
	// machine was called at the time (a copy — a snapshot outlives it).
	MachineID   string `json:"machine_id"`
	MachineName string `json:"machine_name"`
	// Kind is the guest OS: linux / freebsd / netbsd / unikraft.
	Kind  string `json:"kind"`
	Image string `json:"image"`
	Path  string `json:"path"`
	// Parent is the snapshot the source machine was itself branched from.
	Parent      string        `json:"parent"`
	Description string        `json:"description"`
	Cpus        int           `json:"cpus"`
	Mem         int           `json:"mem"`
	Ports       []PortForward `json:"ports"`
	// Size is human-readable when measured; taking a CoW clone costs nothing.
	Size      string `json:"size"`
	CreatedAt int64  `json:"created_at"`
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

// AiAgent is a coding agent bsdkrun can sandbox.
//
// Each runs in a disposable microVM with a persistent login, a shared skills
// store, and only the folder you choose to share.
type AiAgent struct {
	ID    string `json:"id"`
	Label string `json:"label"`
	// Flavor is the catalog flavor that installs it.
	Flavor      string `json:"flavor"`
	Description string `json:"description"`
	// Installed means its flavor is provisioned, so a sandbox boots in a
	// second; false means the first launch installs a toolchain (minutes).
	Installed bool  `json:"installed"`
	Running   int64 `json:"running"`
}

// AiSession is one agent sandbox. It is a machine, so Logs/Stop work on ID.
type AiSession struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Agent   string `json:"agent"`
	Running bool   `json:"running"`
	// Workspace is the directory shared into it, on the engine's host.
	Workspace string `json:"workspace"`
	CreatedAt int64  `json:"created_at"`
}

func aiAgentFromGraphQL(m map[string]any) AiAgent {
	a := AiAgent{
		ID:          asString(m["id"]),
		Label:       asString(m["label"]),
		Flavor:      asString(m["flavor"]),
		Description: asString(m["description"]),
		Installed:   asBool(m["installed"]),
	}
	if n, ok := asInt(m["running"]); ok {
		a.Running = n
	}
	return a
}

func aiSessionFromGraphQL(m map[string]any) AiSession {
	s := AiSession{
		ID:        asString(m["id"]),
		Name:      asString(m["name"]),
		Agent:     asString(m["agent"]),
		Running:   asBool(m["running"]),
		Workspace: asString(m["workspace"]),
	}
	if n, ok := asInt(m["createdAt"]); ok {
		s.CreatedAt = n
	}
	return s
}

// DockerStatus is the Docker engine VM: whether it is up, and how to reach
// it.
//
// bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
// socket, so the host's own `docker` CLI drives the same engine.
type DockerStatus struct {
	Running        bool   `json:"running"`
	MachineID      string `json:"machine_id"`
	MachineRunning bool   `json:"machine_running"`
	// Socket is the unix socket the `docker` CLI talks to.
	Socket      string `json:"socket"`
	SocketReady bool   `json:"socket_ready"`
	APIPort     int    `json:"api_port"`
	Version     string `json:"version"`
	Containers  int64  `json:"containers"`
	Images      int64  `json:"images"`
	// Mounts are host directories shared into the VM, each "HOST:GUEST".
	Mounts []string `json:"mounts"`
	// Disk is the dedicated image store, when the VM has one; DiskSize is its
	// size in bytes — sparse, so the cap rather than the usage.
	Disk     string `json:"disk"`
	DiskSize int64  `json:"disk_size"`
}

// DockerContainer is a container in the engine — a trimmed `docker ps` row.
type DockerContainer struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Image   string `json:"image"`
	Command string `json:"command"`
	// State is "running" | "exited" | "created" | "paused" | ...
	State string `json:"state"`
	// Status is Docker's human status, e.g. "Up 3 minutes".
	Status string `json:"status"`
	// Ports are published forwards, each "HOST:GUEST/proto".
	Ports []string `json:"ports"`
	// Created is unix epoch seconds.
	Created int64 `json:"created"`
}

// IsRunning reports whether the container is up.
func (c DockerContainer) IsRunning() bool { return c.State == "running" }

func dockerStatusFromGraphQL(m map[string]any) DockerStatus {
	s := DockerStatus{
		Running:        asBool(m["running"]),
		MachineID:      asString(m["machineId"]),
		MachineRunning: asBool(m["machineRunning"]),
		Socket:         asString(m["socket"]),
		SocketReady:    asBool(m["socketReady"]),
		Version:        asString(m["version"]),
		Disk:           asString(m["disk"]),
	}
	if n, ok := asInt(m["apiPort"]); ok {
		s.APIPort = int(n)
	}
	if n, ok := asInt(m["containers"]); ok {
		s.Containers = n
	}
	if n, ok := asInt(m["images"]); ok {
		s.Images = n
	}
	if n, ok := asInt(m["diskSize"]); ok {
		s.DiskSize = n
	}
	if mounts, ok := m["mounts"].([]any); ok {
		for _, v := range mounts {
			s.Mounts = append(s.Mounts, asString(v))
		}
	}
	return s
}

func dockerContainerFromGraphQL(m map[string]any) DockerContainer {
	c := DockerContainer{
		ID:      asString(m["id"]),
		Name:    asString(m["name"]),
		Image:   asString(m["image"]),
		Command: asString(m["command"]),
		State:   asString(m["state"]),
		Status:  asString(m["status"]),
	}
	if n, ok := asInt(m["created"]); ok {
		c.Created = n
	}
	if ports, ok := m["ports"].([]any); ok {
		for _, v := range ports {
			c.Ports = append(c.Ports, asString(v))
		}
	}
	return c
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
		Origin:   asString(m["origin"]),
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

func snapshotInfoFromGraphQL(m map[string]any) SnapshotInfo {
	info := SnapshotInfo{
		ID:          asString(m["id"]),
		Name:        asString(m["name"]),
		MachineID:   asString(m["machineId"]),
		MachineName: asString(m["machineName"]),
		Kind:        asString(m["kind"]),
		Image:       asString(m["image"]),
		Path:        asString(m["path"]),
		Parent:      asString(m["parent"]),
		Description: asString(m["description"]),
		Size:        asString(m["size"]),
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
			if port.Bind == "" {
				port.Bind = "127.0.0.1"
			}
			info.Ports = append(info.Ports, port)
		}
	}
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
