package bsdkrun

import (
	"encoding/json"
	"io"
	"regexp"
	"strconv"
	"strings"
)

var (
	machineIDRe = regexp.MustCompile(`^[0-9a-f]{6,}$`)
	sshPortRe   = regexp.MustCompile(`ssh -p (\d+)`)
)

// Sandbox is a handle to a running (or stopped) bsdkrun microVM. Create one
// with the fluent constructors (Linux, FreeBSD, ...), reconnect with
// GetSandbox, or enumerate with ListSandboxes.
type Sandbox struct {
	// ID is the machine's Docker-style short id.
	ID string
	// SSHPort is the host port forwarded to the guest's SSH, if the boot
	// banner reported one (0 otherwise).
	SSHPort int
}

// GetSandbox reconnects to an existing machine by id (a unique prefix is
// enough). It returns a *SandboxNotFoundError when nothing matches.
func GetSandbox(id string) (*Sandbox, error) {
	rows, err := ListSandboxes(true)
	if err != nil {
		return nil, err
	}
	for _, info := range rows {
		if info.ID == id || strings.HasPrefix(info.ID, id) || info.Name == id {
			return &Sandbox{ID: info.ID}, nil
		}
	}
	return nil, &SandboxNotFoundError{ID: id}
}

// ListSandboxes lists machines. all=true includes exited ones (default:
// running only).
func ListSandboxes(all bool) ([]SandboxInfo, error) {
	args := []string{"ps", "--json"}
	if all {
		args = append(args, "--all")
	}
	res, err := RunChecked(args, "bsdkrun ps", nil)
	if err != nil {
		return nil, err
	}
	return parseSandboxRows(res.Stdout)
}

func parseSandboxRows(stdout string) ([]SandboxInfo, error) {
	raw := strings.TrimSpace(stdout)
	if raw == "" {
		raw = "[]"
	}
	var rows []SandboxInfo
	if err := json.Unmarshal([]byte(raw), &rows); err != nil {
		return nil, err
	}
	for i := range rows {
		rows[i].normalize()
	}
	return rows, nil
}

// -- commands ---------------------------------------------------------------

// ExecBuilder configures a command to run in the guest through its exec
// agent. Build one with Sandbox.Command, chain options, then Run:
//
//	sbx.Command("node").Args("-e", "print(1)").Env("X", "hi").Cwd("/app").Run()
type ExecBuilder struct {
	sbx      *Sandbox
	argv     []string
	env      []string
	cwd      string
	stdin    []byte
	tty      bool
	logLevel int
	check    bool
	stdout   io.Writer
	stderr   io.Writer
}

// Command starts building a guest command: a program name plus (optional)
// initial arguments.
func (s *Sandbox) Command(name string, args ...string) *ExecBuilder {
	return &ExecBuilder{sbx: s, argv: append([]string{name}, args...)}
}

// Args appends arguments to the command.
func (b *ExecBuilder) Args(args ...string) *ExecBuilder {
	b.argv = append(b.argv, args...)
	return b
}

// Env sets a per-command environment variable (-e KEY=VALUE). Repeatable.
func (b *ExecBuilder) Env(key, value string) *ExecBuilder {
	b.env = append(b.env, key+"="+value)
	return b
}

// Cwd runs the command in a working directory.
func (b *ExecBuilder) Cwd(dir string) *ExecBuilder {
	b.cwd = dir
	return b
}

// Stdin pipes data to the command's stdin.
func (b *ExecBuilder) Stdin(data string) *ExecBuilder {
	b.stdin = []byte(data)
	return b
}

// StdinBytes pipes raw bytes to the command's stdin.
func (b *ExecBuilder) StdinBytes(data []byte) *ExecBuilder {
	b.stdin = data
	return b
}

// TTY allocates a PTY for the command.
func (b *ExecBuilder) TTY() *ExecBuilder {
	b.tty = true
	return b
}

// Stdout streams stdout to w while retaining it in the returned Result.
func (b *ExecBuilder) Stdout(w io.Writer) *ExecBuilder { b.stdout = w; return b }

// Stderr streams stderr to w while retaining it in the returned Result.
func (b *ExecBuilder) Stderr(w io.Writer) *ExecBuilder { b.stderr = w; return b }

// LogLevel sets bsdkrun's global --log-level for this invocation.
func (b *ExecBuilder) LogLevel(level int) *ExecBuilder {
	b.logLevel = level
	return b
}

// Check makes Run return a *CommandFailedError when the command exits
// non-zero (Python's throw_on_error). Without it, inspect Result.Err().
func (b *ExecBuilder) Check() *ExecBuilder {
	b.check = true
	return b
}

// Run executes the command and captures its result. The Result is returned
// even alongside a Check-triggered error, so output stays inspectable.
func (b *ExecBuilder) Run() (*Result, error) {
	argv := b.argv
	if b.cwd != "" {
		// Emulate a working directory: cd, drop it, then exec the real argv.
		argv = append([]string{"/bin/sh", "-c", `cd "$1" && shift && exec "$@"`, "sh", b.cwd}, argv...)
	}

	cliArgs := []string{"exec"}
	if b.tty {
		cliArgs = append(cliArgs, "-t")
	}
	for _, pair := range b.env {
		cliArgs = append(cliArgs, "-e", pair)
	}
	cliArgs = append(cliArgs, b.sbx.ID)
	cliArgs = append(cliArgs, argv...)

	res, err := Run(cliArgs, &RunOpts{Stdin: b.stdin, LogLevel: b.logLevel, Stdout: b.stdout, Stderr: b.stderr})
	if err != nil {
		return nil, err
	}
	result := &Result{
		Stdout:   res.Stdout,
		Stderr:   res.Stderr,
		ExitCode: res.ExitCode,
		Command:  "exec " + strings.Join(argv, " "),
	}
	if b.check {
		if err := result.Err(); err != nil {
			return result, err
		}
	}
	return result, nil
}

// Exec is the shorthand for running an argv directly:
//
//	sbx.Exec("uname", "-a")
func (s *Sandbox) Exec(argv ...string) (*Result, error) {
	if len(argv) == 0 {
		return s.Command("").Run()
	}
	return s.Command(argv[0], argv[1:]...).Run()
}

// Logs reads the machine's console log.
func (s *Sandbox) Logs() (string, error) {
	return s.logs(false)
}

// BootLogs reads bsdkrun's boot log for the machine.
func (s *Sandbox) BootLogs() (string, error) {
	return s.logs(true)
}

func (s *Sandbox) logs(boot bool) (string, error) {
	args := []string{"logs"}
	if boot {
		args = append(args, "--boot")
	}
	args = append(args, s.ID)
	res, err := Run(args, nil)
	if err != nil {
		return "", err
	}
	return res.Stdout, nil
}

// Shell attaches an interactive shell to the machine (inherits the
// terminal). It blocks until the shell exits and returns its exit code.
func (s *Sandbox) Shell() (int, error) {
	return Spawn([]string{"shell", s.ID}, nil)
}

// -- inspection -------------------------------------------------------------

// Status fetches this machine's current status row, or nil if it's gone.
func (s *Sandbox) Status() (*SandboxInfo, error) {
	rows, err := ListSandboxes(true)
	if err != nil {
		return nil, err
	}
	for i := range rows {
		if rows[i].ID == s.ID {
			return &rows[i], nil
		}
	}
	return nil, nil
}

// IsRunning reports whether the machine is currently running.
func (s *Sandbox) IsRunning() (bool, error) {
	info, err := s.Status()
	if err != nil {
		return false, err
	}
	return info != nil && info.Running, nil
}

// -- lifecycle --------------------------------------------------------------

// Stop stops the machine (BSD: clean power-off; Linux: SIGTERM).
func (s *Sandbox) Stop() error {
	_, err := RunChecked([]string{"stop", s.ID}, "bsdkrun stop", nil)
	return err
}

// Start restarts a stopped machine in place — same id, disk/rootfs, network.
func (s *Sandbox) Start() error {
	_, err := RunChecked([]string{"start", s.ID}, "bsdkrun start", nil)
	return err
}

// Remove removes the machine and its state. force stops it first if running.
func (s *Sandbox) Remove(force bool) error {
	args := []string{"rm"}
	if force {
		args = append(args, "--force")
	}
	args = append(args, s.ID)
	_, err := RunChecked(args, "bsdkrun rm", nil)
	return err
}

// UpdateBuilder changes the recorded vCPU / RAM; changes apply on the next
// Start.
type UpdateBuilder struct {
	sbx  *Sandbox
	cpus int
	mem  int
}

// Update starts an update: chain Cpus/Mem, then Apply.
func (s *Sandbox) Update() *UpdateBuilder {
	return &UpdateBuilder{sbx: s}
}

// Cpus sets the vCPU count.
func (b *UpdateBuilder) Cpus(n int) *UpdateBuilder {
	b.cpus = n
	return b
}

// Mem sets the RAM in MiB.
func (b *UpdateBuilder) Mem(mib int) *UpdateBuilder {
	b.mem = mib
	return b
}

// Apply records the change.
func (b *UpdateBuilder) Apply() error {
	args := []string{"update", b.sbx.ID}
	if b.cpus != 0 {
		args = append(args, "--cpus", strconv.Itoa(b.cpus))
	}
	if b.mem != 0 {
		args = append(args, "--mem", strconv.Itoa(b.mem))
	}
	_, err := RunChecked(args, "bsdkrun update", nil)
	return err
}

// ConnectNetwork joins or switches this machine to a global network
// (applies on the next Start).
func (s *Sandbox) ConnectNetwork(network string) error {
	return Networks.Connect(s.ID, network)
}

// DisconnectNetwork detaches this machine from its network. Applies on the
// next start.
func (s *Sandbox) DisconnectNetwork() error {
	return Networks.Disconnect(s.ID)
}

// -- in-guest agent helpers -------------------------------------------------

func (s *Sandbox) agent(family string, action []string, env map[string]string) (*Result, error) {
	args := append([]string{family, s.ID}, action...)
	res, err := Run(args, &RunOpts{Env: env})
	if err != nil {
		return nil, err
	}
	result := &Result{
		Stdout:   res.Stdout,
		Stderr:   res.Stderr,
		ExitCode: res.ExitCode,
		Command:  family + " " + strings.Join(action, " "),
	}
	if err := result.Err(); err != nil {
		return result, err
	}
	return result, nil
}

// SSHSetupBuilder installs SSH keys in the guest (`ssh setup`, via the
// agent). With no Key, the CLI installs your local ~/.ssh/*.pub keys.
type SSHSetupBuilder struct {
	sbx  *Sandbox
	user string
	keys []string
}

// SSHSetup starts an ssh setup: chain User/Key, then Run.
func (s *Sandbox) SSHSetup() *SSHSetupBuilder {
	return &SSHSetupBuilder{sbx: s}
}

// User targets a guest user.
func (b *SSHSetupBuilder) User(user string) *SSHSetupBuilder {
	b.user = user
	return b
}

// Key adds a key: a literal "ssh-..." string or a local .pub path.
// Repeatable.
func (b *SSHSetupBuilder) Key(key string) *SSHSetupBuilder {
	b.keys = append(b.keys, key)
	return b
}

// Run installs the keys.
func (b *SSHSetupBuilder) Run() (*Result, error) {
	action := []string{"setup"}
	if b.user != "" {
		action = append(action, "--user", b.user)
	}
	for _, key := range b.keys {
		action = append(action, "--key", key)
	}
	return b.sbx.agent("ssh", action, nil)
}

// TailscaleBuilder puts a guest on your tailnet (`tailscale setup`, via the
// agent).
type TailscaleBuilder struct {
	sbx      *Sandbox
	authkey  string
	hostname string
	args     []string
}

// TailscaleUp starts a tailscale setup: chain AuthKey/Hostname/Args, then
// Run.
func (s *Sandbox) TailscaleUp() *TailscaleBuilder {
	return &TailscaleBuilder{sbx: s}
}

// AuthKey is forwarded as the TS_AUTHKEY env var (kept off the arg list).
func (b *TailscaleBuilder) AuthKey(key string) *TailscaleBuilder {
	b.authkey = key
	return b
}

// Hostname sets the machine name on the tailnet.
func (b *TailscaleBuilder) Hostname(name string) *TailscaleBuilder {
	b.hostname = name
	return b
}

// Args appends extra `tailscale setup` arguments.
func (b *TailscaleBuilder) Args(args ...string) *TailscaleBuilder {
	b.args = append(b.args, args...)
	return b
}

// Run performs the setup.
func (b *TailscaleBuilder) Run() (*Result, error) {
	action := []string{"setup"}
	if b.hostname != "" {
		action = append(action, "--hostname", b.hostname)
	}
	action = append(action, b.args...)
	var env map[string]string
	if b.authkey != "" {
		env = map[string]string{"TS_AUTHKEY": b.authkey}
	}
	return b.sbx.agent("tailscale", action, env)
}
