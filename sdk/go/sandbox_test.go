package bsdkrun

// Sandbox tests against a fake bsdkrun: a shell script standing in for the
// binary, which records every argv it receives and replays canned output —
// no real binary and no VM needed.

import (
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

const fakePS = `[
  {"id": "abc123def456", "name": "web", "image": "alpine", "kind": "linux",
   "command": "sleep 300", "running": true, "exit_code": null, "pid": 42,
   "detached": true, "cpus": 2, "mem": 512, "volume": null,
   "state_dir": "/state", "network": "devnet", "net_ip": "10.88.0.2",
   "ports": [{"bind": "127.0.0.1", "host": 2222, "guest": 22}],
   "created_at": 1700000000, "finished_at": null}
]`

// installFakeBinary writes the stand-in script, points the SDK at it, and
// returns a reader for the argv records it accumulates (one per
// invocation).
func installFakeBinary(t *testing.T) func() [][]string {
	t.Helper()
	dir := t.TempDir()
	logPath := filepath.Join(dir, "argv.log")
	psPath := filepath.Join(dir, "ps.json")
	if err := os.WriteFile(psPath, []byte(fakePS), 0o644); err != nil {
		t.Fatal(err)
	}

	script := `#!/bin/sh
{ printf '%s\n' "$@"; printf -- '----\n'; } >> "` + logPath + `"
shift 2  # drop the global --log-level N pair
case "$1" in
  linux)
    echo "some boot noise"
    echo "abc123def456"
    echo "  connect: ssh -p 2201 root@127.0.0.1" >&2
    ;;
  ps)
    cat "` + psPath + `"
    ;;
  exec)
    printf 'EXEC_OUT\n'
    printf 'EXEC_ERR\n' >&2
    exit 7
    ;;
  logs)
    printf 'console log\n'
    ;;
  ssh)
    printf 'keys installed\n'
    ;;
  *)
    :
    ;;
esac
`
	bin := filepath.Join(dir, "bsdkrun")
	if err := os.WriteFile(bin, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	SetBinaryPath(bin)
	t.Cleanup(ResetBinaryCache)

	return func() [][]string {
		raw, err := os.ReadFile(logPath)
		if err != nil {
			return nil
		}
		var records [][]string
		var current []string
		for _, line := range strings.Split(string(raw), "\n") {
			if line == "----" {
				records = append(records, current)
				current = nil
				continue
			}
			if line != "" {
				current = append(current, line)
			}
		}
		return records
	}
}

func TestCreateParsesIDAndSSHPort(t *testing.T) {
	argv := installFakeBinary(t)

	sbx, err := Linux("alpine").
		Cpus(2).Mem(1024).
		Volume("web").
		Mount("~/project:/src").
		Port("8080:80").
		Command("sleep", "300").
		Create()
	if err != nil {
		t.Fatal(err)
	}
	if sbx.ID != "abc123def456" {
		t.Fatalf("id: %q", sbx.ID)
	}
	if sbx.SSHPort != 2201 {
		t.Fatalf("ssh port: %d", sbx.SSHPort)
	}

	records := argv()
	if len(records) != 1 {
		t.Fatalf("invocations: %d", len(records))
	}
	want := []string{
		"--log-level", "1",
		"linux", "alpine", "-d",
		"-v", "web",
		"--mount", "~/project:/src",
		"--port", "8080:80",
		"--cpus", "2",
		"--mem", "1024",
		"--", "sleep", "300",
	}
	if !reflect.DeepEqual(records[0], want) {
		t.Fatalf("argv:\n got %v\nwant %v", records[0], want)
	}
}

func TestExecBuilderArgv(t *testing.T) {
	argv := installFakeBinary(t)
	sbx := &Sandbox{ID: "abc123def456"}

	res, err := sbx.Command("node").
		Args("-e", "1").
		Env("X", "hi").
		Cwd("/app").
		TTY().
		Stdin("data").
		Run()
	if err != nil {
		t.Fatal(err)
	}
	if res.ExitCode != 7 || res.Stdout != "EXEC_OUT\n" || res.Stderr != "EXEC_ERR\n" {
		t.Fatalf("%+v", res)
	}
	var cmdErr *CommandFailedError
	if err := res.Err(); !errors.As(err, &cmdErr) {
		t.Fatalf("Err: %v", err)
	}

	want := []string{
		"--log-level", "0",
		"exec", "-t", "-e", "X=hi", "abc123def456",
		"/bin/sh", "-c", `cd "$1" && shift && exec "$@"`, "sh", "/app",
		"node", "-e", "1",
	}
	if got := argv()[0]; !reflect.DeepEqual(got, want) {
		t.Fatalf("argv:\n got %v\nwant %v", got, want)
	}
}

func TestExecCheckReturnsCommandFailed(t *testing.T) {
	installFakeBinary(t)
	sbx := &Sandbox{ID: "abc123def456"}
	res, err := sbx.Command("false").Check().Run()
	var cmdErr *CommandFailedError
	if !errors.As(err, &cmdErr) || cmdErr.ExitCode != 7 {
		t.Fatalf("err: %v", err)
	}
	if res == nil || res.Stdout != "EXEC_OUT\n" {
		t.Fatalf("result should still be inspectable: %+v", res)
	}
}

func TestExecShorthand(t *testing.T) {
	argv := installFakeBinary(t)
	sbx := &Sandbox{ID: "abc123def456"}
	if _, err := sbx.Exec("uname", "-a"); err != nil {
		t.Fatal(err)
	}
	want := []string{"--log-level", "0", "exec", "abc123def456", "uname", "-a"}
	if got := argv()[0]; !reflect.DeepEqual(got, want) {
		t.Fatalf("argv: %v", got)
	}
}

func TestListStatusAndGet(t *testing.T) {
	argv := installFakeBinary(t)

	rows, err := ListSandboxes(true)
	if err != nil || len(rows) != 1 {
		t.Fatalf("rows=%v err=%v", rows, err)
	}
	if rows[0].ID != "abc123def456" || !rows[0].Running || rows[0].Status != "running" {
		t.Fatalf("%+v", rows[0])
	}
	want := []string{"--log-level", "0", "ps", "--json", "--all"}
	if got := argv()[0]; !reflect.DeepEqual(got, want) {
		t.Fatalf("argv: %v", got)
	}

	// Prefix reconnection.
	sbx, err := GetSandbox("abc123")
	if err != nil || sbx.ID != "abc123def456" {
		t.Fatalf("sbx=%v err=%v", sbx, err)
	}

	var nfErr *SandboxNotFoundError
	if _, err := GetSandbox("zzz"); !errors.As(err, &nfErr) {
		t.Fatalf("err: %v", err)
	}

	info, err := sbx.Status()
	if err != nil || info == nil || info.Network != "devnet" {
		t.Fatalf("info=%v err=%v", info, err)
	}
	running, err := sbx.IsRunning()
	if err != nil || !running {
		t.Fatalf("running=%v err=%v", running, err)
	}
}

func TestLifecycleArgv(t *testing.T) {
	argv := installFakeBinary(t)
	sbx := &Sandbox{ID: "abc123def456"}

	if err := sbx.Stop(); err != nil {
		t.Fatal(err)
	}
	if err := sbx.Start(); err != nil {
		t.Fatal(err)
	}
	if err := sbx.Update().Cpus(4).Mem(2048).Apply(); err != nil {
		t.Fatal(err)
	}
	if err := sbx.Remove(true); err != nil {
		t.Fatal(err)
	}
	if err := sbx.ConnectNetwork("devnet"); err != nil {
		t.Fatal(err)
	}
	if err := sbx.DisconnectNetwork(); err != nil {
		t.Fatal(err)
	}

	records := argv()
	want := [][]string{
		{"--log-level", "0", "stop", "abc123def456"},
		{"--log-level", "0", "start", "abc123def456"},
		{"--log-level", "0", "update", "abc123def456", "--cpus", "4", "--mem", "2048"},
		{"--log-level", "0", "rm", "--force", "abc123def456"},
		{"--log-level", "0", "network", "connect", "abc123def456", "devnet"},
		{"--log-level", "0", "network", "disconnect", "abc123def456"},
	}
	if !reflect.DeepEqual(records, want) {
		t.Fatalf("argv:\n got %v\nwant %v", records, want)
	}
}

func TestLogs(t *testing.T) {
	argv := installFakeBinary(t)
	sbx := &Sandbox{ID: "abc123def456"}
	out, err := sbx.Logs()
	if err != nil || out != "console log\n" {
		t.Fatalf("out=%q err=%v", out, err)
	}
	if _, err := sbx.BootLogs(); err != nil {
		t.Fatal(err)
	}
	records := argv()
	want := [][]string{
		{"--log-level", "0", "logs", "abc123def456"},
		{"--log-level", "0", "logs", "--boot", "abc123def456"},
	}
	if !reflect.DeepEqual(records, want) {
		t.Fatalf("argv: %v", records)
	}
}

func TestSSHSetupArgv(t *testing.T) {
	argv := installFakeBinary(t)
	sbx := &Sandbox{ID: "abc123def456"}
	res, err := sbx.SSHSetup().User("tsiry").Key("~/.ssh/work.pub").Run()
	if err != nil {
		t.Fatal(err)
	}
	if res.Stdout != "keys installed\n" {
		t.Fatalf("%+v", res)
	}
	want := []string{
		"--log-level", "0",
		"ssh", "abc123def456", "setup", "--user", "tsiry", "--key", "~/.ssh/work.pub",
	}
	if got := argv()[0]; !reflect.DeepEqual(got, want) {
		t.Fatalf("argv: %v", got)
	}
}

func TestNetworksMembersFilters(t *testing.T) {
	installFakeBinary(t)
	members, err := Networks.Members("devnet")
	if err != nil || len(members) != 1 || members[0].ID != "abc123def456" {
		t.Fatalf("members=%v err=%v", members, err)
	}
	none, err := Networks.Members("other")
	if err != nil || len(none) != 0 {
		t.Fatalf("members=%v err=%v", none, err)
	}
}
