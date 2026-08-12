package bsdkrun

import (
	"errors"
	"reflect"
	"testing"
)

const psRow = `[
  {
    "id": "abc123def456",
    "name": "web",
    "image": "alpine",
    "kind": "linux",
    "command": "sleep 300",
    "running": true,
    "exit_code": null,
    "pid": 42,
    "detached": true,
    "cpus": 2,
    "mem": 512,
    "volume": null,
    "state_dir": "/state",
    "network": "devnet",
    "net_ip": "10.88.0.2",
    "ports": [{"bind": "127.0.0.1", "host": 2222, "guest": 22}],
    "created_at": 1700000000,
    "finished_at": null
  }
]`

func TestParseSandboxRows(t *testing.T) {
	rows, err := parseSandboxRows(psRow)
	if err != nil {
		t.Fatal(err)
	}
	if len(rows) != 1 {
		t.Fatalf("got %d rows", len(rows))
	}
	info := rows[0]
	if info.ID != "abc123def456" || info.Name != "web" || info.Kind != "linux" {
		t.Fatalf("bad identity fields: %+v", info)
	}
	if info.Status != "running" || !info.Running {
		t.Fatalf("status not derived: %+v", info)
	}
	if info.PID == nil || *info.PID != 42 {
		t.Fatalf("pid: %+v", info.PID)
	}
	if info.ExitCode != nil || info.FinishedAt != nil {
		t.Fatalf("nullables should be nil: %+v", info)
	}
	if info.CreatedAt != 1700000000 {
		t.Fatalf("created_at: %d", info.CreatedAt)
	}
	want := []PortForward{{Host: 2222, Guest: 22, Bind: "127.0.0.1"}}
	if !reflect.DeepEqual(info.Ports, want) {
		t.Fatalf("ports: %+v", info.Ports)
	}
}

func TestParseSandboxRowsEmpty(t *testing.T) {
	rows, err := parseSandboxRows("")
	if err != nil || len(rows) != 0 {
		t.Fatalf("rows=%v err=%v", rows, err)
	}
}

func TestSandboxInfoFromGraphQL(t *testing.T) {
	// The schema is camelCase; createdAt/finishedAt arrive as
	// decimal-string unix timestamps.
	info := sandboxInfoFromGraphQL(map[string]any{
		"id":         "abc123",
		"name":       nil,
		"image":      "alpine",
		"kind":       "linux",
		"command":    "sleep 1",
		"status":     "running",
		"running":    true,
		"exitCode":   nil,
		"pid":        float64(42),
		"detached":   true,
		"cpus":       float64(2),
		"mem":        float64(512),
		"volume":     nil,
		"stateDir":   "/s",
		"createdAt":  "1700000000",
		"finishedAt": nil,
		"network":    nil,
		"netIp":      nil,
		"ports":      []any{map[string]any{"bind": "127.0.0.1", "host": float64(2222), "guest": float64(22)}},
	})
	if info.ID != "abc123" || info.CreatedAt != 1700000000 {
		t.Fatalf("%+v", info)
	}
	if info.PID == nil || *info.PID != 42 {
		t.Fatalf("pid: %+v", info.PID)
	}
	if info.Status != "running" {
		t.Fatalf("status: %q", info.Status)
	}
	if len(info.Ports) != 1 || info.Ports[0].Host != 2222 || info.Ports[0].Guest != 22 {
		t.Fatalf("ports: %+v", info.Ports)
	}
}

func TestSandboxInfoFromGraphQLDerivesStatus(t *testing.T) {
	info := sandboxInfoFromGraphQL(map[string]any{"id": "x", "running": false})
	if info.Status != "exited" {
		t.Fatalf("status: %q", info.Status)
	}
}

func TestCommandResultFromGraphQL(t *testing.T) {
	result := commandResultFromGraphQL(map[string]any{
		"exitCode": float64(3),
		"stdout":   "out",
		"stderr":   "err",
	})
	if result.ExitCode != 3 || result.Stdout != "out" || result.Stderr != "err" {
		t.Fatalf("%+v", result)
	}
}

func TestShellSessionInfoFromGraphQL(t *testing.T) {
	info := shellSessionInfoFromGraphQL(map[string]any{
		"id":        "sess-1",
		"machineId": "abc",
		"finished":  false,
		"truncated": true,
	})
	if info.ID != "sess-1" || info.MachineID != "abc" || info.Finished || !info.Truncated {
		t.Fatalf("%+v", info)
	}
}

func TestResultHelpers(t *testing.T) {
	res := &Result{Stdout: "a\nb\n\n", Stderr: "", ExitCode: 0, Command: "exec x"}
	if !res.Ok() {
		t.Fatal("Ok")
	}
	if res.Text() != "a\nb" {
		t.Fatalf("Text: %q", res.Text())
	}
	if !reflect.DeepEqual(res.Lines(), []string{"a", "b"}) {
		t.Fatalf("Lines: %v", res.Lines())
	}
	if err := res.Err(); err != nil {
		t.Fatalf("Err: %v", err)
	}

	jsonRes := &Result{Stdout: `{"x": 1}`}
	var parsed map[string]int
	if err := jsonRes.JSON(&parsed); err != nil || parsed["x"] != 1 {
		t.Fatalf("JSON: %v %v", parsed, err)
	}

	failed := &Result{Stdout: "o", Stderr: "e", ExitCode: 3, Command: "exec y"}
	if failed.Ok() {
		t.Fatal("Ok on failure")
	}
	var cmdErr *CommandFailedError
	if err := failed.Err(); !errors.As(err, &cmdErr) {
		t.Fatalf("Err type: %v", err)
	} else if cmdErr.ExitCode != 3 || cmdErr.Stdout != "o" || cmdErr.Stderr != "e" || cmdErr.Command != "exec y" {
		t.Fatalf("%+v", cmdErr)
	}
}

func TestPortForwardString(t *testing.T) {
	if got := (PortForward{Host: 2222, Guest: 22}).String(); got != "2222:22" {
		t.Fatalf("String: %q", got)
	}
}

func TestAsInt(t *testing.T) {
	cases := []struct {
		in   any
		want int64
		ok   bool
	}{
		{float64(7), 7, true},
		{"1700000000", 1700000000, true},
		{" 42 ", 42, true},
		{nil, 0, false},
		{"x", 0, false},
		{true, 0, false},
	}
	for _, c := range cases {
		got, ok := asInt(c.in)
		if got != c.want || ok != c.ok {
			t.Errorf("asInt(%v) = %d,%v; want %d,%v", c.in, got, ok, c.want, c.ok)
		}
	}
}
