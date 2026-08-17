package bsdkrun

import (
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"strings"
	"sync"
)

// A client for a remote bsdkrund daemon's GraphQL API.
//
// Sandbox talks to a *local* bsdkrun binary by shelling out to it. Client
// is the network sibling: it drives the exact same operations against a
// daemon over HTTP (queries/mutations) and a hand-rolled
// graphql-transport-ws socket (subscriptions — exec output, live shells,
// log follow), so a program can target either a machine with the CLI
// installed or a remote host running bsdkrund with the same calls.
//
// The GraphQL documents below are deliberately minimal string literals
// rather than a generated client: this SDK has no code-generation step, and
// the schema is small and stable enough (daemon/src/graphql.rs) that
// hand-typed queries stay easy to keep in sync.

// ---------------------------------------------------------------------------
// GraphQL documents
// ---------------------------------------------------------------------------

const (
	machineFields = "id name image kind command status running exitCode pid detached " +
		"cpus mem volume stateDir createdAt finishedAt network netIp origin " +
		"ports { bind host guest }"
	snapshotFields = "id name machineId machineName kind image path parent description " +
		"cpus mem size createdAt ports { bind host guest }"
	cmdResultFields = "exitCode stdout stderr"
	sessionFields   = "id machineId finished truncated"
)

var (
	listQuery = "query($all: Boolean!) { machines(all: $all) { " + machineFields + " } }"
	getQuery  = "query($id: String!) { machine(id: $id) { " + machineFields + " } }"
	logsQuery = "query($id: String!, $boot: Boolean!) { machineLogs(id: $id, boot: $boot) }"

	stopMutation   = "mutation($id: String!) { stopMachine(id: $id) { " + cmdResultFields + " } }"
	startMutation  = "mutation($id: String!) { startMachine(id: $id) { " + cmdResultFields + " } }"
	removeMutation = "mutation($ids: [String!]!, $force: Boolean!) { " +
		"removeMachines(ids: $ids, force: $force) { " + cmdResultFields + " } }"
	updateMutation = "mutation($id: String!, $cpus: Int, $mem: Int) { " +
		"updateMachine(id: $id, cpus: $cpus, mem: $mem) { " + cmdResultFields + " } }"
	commitMutation = "mutation($id: String!, $name: String!, $description: String!) { " +
		"commitMachine(id: $id, name: $name, description: $description) { " + cmdResultFields + " } }"

	aiAgentFields   = "id label flavor description installed running"
	aiSessionFields = "id name agent running workspace createdAt"

	aiAgentsQuery       = "{ aiAgents { " + aiAgentFields + " } }"
	aiSessionsQuery     = "{ aiSessions { " + aiSessionFields + " } }"
	aiShellCommandQuery = "query($agent: String!, $machineId: String!) { " +
		"aiShellCommand(agent: $agent, machineId: $machineId) }"
	aiStartMutation = "mutation($input: AiStartInput!) { aiStart(input: $input) }"
	aiStopMutation  = "mutation($agent: String!) { aiStop(agent: $agent) { " +
		cmdResultFields + " } }"
	aiRemoveMutation = "mutation($agent: String!, $keepHome: Boolean!) { " +
		"aiRemove(agent: $agent, keepHome: $keepHome) { " + cmdResultFields + " } }"

	dockerStatusFields = "running machineId machineRunning socket socketReady apiPort " +
		"version containers images mounts disk diskSize"
	dockerContainerFields = "id name image command state status ports created"

	dockerStatusQuery     = "{ dockerStatus { " + dockerStatusFields + " } }"
	dockerContainersQuery = "query($all: Boolean!) { dockerContainers(all: $all) { " +
		dockerContainerFields + " } }"
	dockerLogsQuery = "query($id: String!, $tail: Int!) { " +
		"dockerContainerLogs(id: $id, tail: $tail) }"
	dockerStartMutation = "mutation($input: DockerStartInput!) { dockerStart(input: $input) { " +
		dockerStatusFields + " } }"
	dockerStopMutation      = "mutation { dockerStop { " + cmdResultFields + " } }"
	dockerContainerMutation = "mutation($action: String!, $ids: [String!]!) { " +
		"dockerContainer(action: $action, ids: $ids) { " + cmdResultFields + " } }"

	snapshotsQuery = "query($machine: String) { snapshots(machine: $machine) { " +
		snapshotFields + " } }"
	snapshotMutation = "mutation($id: String!, $name: String, $description: String!) { " +
		"snapshotMachine(id: $id, name: $name, description: $description) { " +
		snapshotFields + " } }"
	removeSnapshotsMutation = "mutation($names: [String!]!) { " +
		"removeSnapshots(names: $names) { " + cmdResultFields + " } }"
	restoreMutation = "mutation($id: String!, $snapshot: String!, $force: Boolean!, " +
		"$backup: Boolean!) { restoreMachine(id: $id, snapshot: $snapshot, force: $force, " +
		"backup: $backup) { " + cmdResultFields + " } }"
	rollbackMutation = "mutation($id: String!, $force: Boolean!, $backup: Boolean!) { " +
		"rollbackMachine(id: $id, force: $force, backup: $backup) { " + cmdResultFields + " } }"
	branchMutation = "mutation($input: BranchInput!) { branchSnapshot(input: $input) }"

	runLinuxMutation    = "mutation($input: RunLinuxInput!) { runLinux(input: $input) }"
	runBsdMutation      = "mutation($input: RunBsdInput!) { runBsd(input: $input) }"
	runNanosMutation    = "mutation($input: RunNanosInput!) { runNanos(input: $input) }"
	runUnikraftMutation = "mutation($input: RunUnikraftInput!) { runUnikraft(input: $input) }"
	runSolo5Mutation    = "mutation($input: RunSolo5Input!) { runSolo5(input: $input) }"
	runOsvMutation      = "mutation($input: RunOsvInput!) { runOsv(input: $input) }"
	runFlavorMutation   = "mutation($input: RunFlavorInput!) { runFlavor(input: $input) }"

	machineLogsSubscription = "subscription($id: String!, $follow: Boolean!, $boot: Boolean!) { " +
		"machineLogs(id: $id, follow: $follow, boot: $boot) { dataBase64 exitCode } }"

	openShellMutation = "mutation($machineId: String!, $command: [String!]!, $env: [String!]!, " +
		"$rows: Int!, $cols: Int!) { " +
		"openShell(machineId: $machineId, command: $command, env: $env, " +
		"rows: $rows, cols: $cols) { " + sessionFields + " } }"
	shellOutputSubscription = "subscription($sessionId: String!) { " +
		"shellOutput(sessionId: $sessionId) { dataBase64 exitCode } }"
	sendInputMutation = "mutation($sessionId: String!, $dataBase64: String!) { " +
		"sendShellInput(sessionId: $sessionId, dataBase64: $dataBase64) }"
	resizeMutation = "mutation($sessionId: String!, $rows: Int!, $cols: Int!) { " +
		"resizeShell(sessionId: $sessionId, rows: $rows, cols: $cols) }"
	closeMutation = "mutation($sessionId: String!) { closeShell(sessionId: $sessionId) }"
)

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

// Client drives a remote bsdkrund's GraphQL API. Queries and mutations go
// over HTTP; subscriptions (used internally by Exec, Shell and FollowLogs)
// share one lazily opened graphql-transport-ws socket per Client, torn down
// once the last subscription ends.
type Client struct {
	URL   string
	Token string

	wsMu sync.Mutex
	ws   *wsTransport
}

// NewClient builds a client from a URL and a bearer token. Both are
// required: a URL configured without a token is a configuration error,
// never a silent fall-back to an unauthenticated request.
func NewClient(rawURL, token string) (*Client, error) {
	if strings.TrimSpace(rawURL) == "" {
		return nil, errors.New("a daemon URL is required; nothing to connect to")
	}
	if strings.TrimSpace(token) == "" {
		return nil, errors.New("a bearer token is required alongside the daemon URL")
	}
	return &Client{URL: normalizeURL(rawURL), Token: token}, nil
}

// ClientFromEnv builds a client from BSDKRUN_URL/BSDKRUN_TOKEN. It fails if
// BSDKRUN_URL is unset (nothing to connect to), or if it is set but
// BSDKRUN_TOKEN is not.
func ClientFromEnv() (*Client, error) {
	rawURL := strings.TrimSpace(os.Getenv(EnvURL))
	if rawURL == "" {
		return nil, fmt.Errorf("%s is not set; nothing to connect to", EnvURL)
	}
	token := strings.TrimSpace(os.Getenv(EnvToken))
	if token == "" {
		return nil, fmt.Errorf("%s is set but %s is not", EnvURL, EnvToken)
	}
	return NewClient(rawURL, token)
}

// -- transport (escape hatch) -----------------------------------------------

// Request runs a raw query or mutation and returns its data — the escape
// hatch for anything without a typed method yet.
func (c *Client) Request(query string, variables map[string]any) (map[string]any, error) {
	return httpRequest(c.URL, c.Token, query, variables)
}

// SubscriptionHandlers carries the callbacks for a raw Subscribe. OnNext is
// required; OnError and OnComplete may be nil.
type SubscriptionHandlers struct {
	OnNext     func(data any)
	OnError    func(err error)
	OnComplete func()
}

// Subscribe starts a raw subscription. It returns an unsubscribe function.
func (c *Client) Subscribe(query string, variables map[string]any, h SubscriptionHandlers) (func(), error) {
	transport := c.wsTransport()
	onNext := h.OnNext
	if onNext == nil {
		onNext = func(any) {}
	}
	subID, err := transport.subscribe(query, variables, onNext, h.OnError, h.OnComplete)
	if err != nil {
		return nil, err
	}
	return func() { transport.unsubscribe(subID) }, nil
}

func (c *Client) wsTransport() *wsTransport {
	c.wsMu.Lock()
	defer c.wsMu.Unlock()
	if c.ws == nil {
		c.ws = newWSTransport(wsEndpoint(c.URL), c.Token)
	}
	return c.ws
}

// -- lifecycle / listing ----------------------------------------------------

// List lists machines. all=true includes exited ones.
func (c *Client) List(all bool) ([]SandboxInfo, error) {
	data, err := c.Request(listQuery, map[string]any{"all": all})
	if err != nil {
		return nil, err
	}
	rows, _ := data["machines"].([]any)
	out := make([]SandboxInfo, 0, len(rows))
	for _, row := range rows {
		out = append(out, sandboxInfoFromGraphQL(asMap(row)))
	}
	return out, nil
}

// Get fetches one machine by id (a unique prefix) or name, or nil when
// nothing matches.
func (c *Client) Get(id string) (*SandboxInfo, error) {
	data, err := c.Request(getQuery, map[string]any{"id": id})
	if err != nil {
		return nil, err
	}
	m := asMap(data["machine"])
	if m == nil {
		return nil, nil
	}
	info := sandboxInfoFromGraphQL(m)
	return &info, nil
}

func (c *Client) commandResult(query string, variables map[string]any, field string) (*CommandResult, error) {
	data, err := c.Request(query, variables)
	if err != nil {
		return nil, err
	}
	return commandResultFromGraphQL(asMap(data[field])), nil
}

// Stop stops a machine.
func (c *Client) Stop(id string) (*CommandResult, error) {
	return c.commandResult(stopMutation, map[string]any{"id": id}, "stopMachine")
}

// Start restarts a stopped machine in place.
func (c *Client) Start(id string) (*CommandResult, error) {
	return c.commandResult(startMutation, map[string]any{"id": id}, "startMachine")
}

// Remove removes machines and their state. force stops them first if
// running.
func (c *Client) Remove(ids []string, force bool) (*CommandResult, error) {
	return c.commandResult(removeMutation, map[string]any{"ids": ids, "force": force}, "removeMachines")
}

// Update changes a machine's recorded vCPU / RAM (0 leaves a value
// untouched). Applies on its next start.
func (c *Client) Update(id string, cpus, mem int) (*CommandResult, error) {
	variables := map[string]any{"id": id, "cpus": nil, "mem": nil}
	if cpus != 0 {
		variables["cpus"] = cpus
	}
	if mem != 0 {
		variables["mem"] = mem
	}
	return c.commandResult(updateMutation, variables, "updateMachine")
}

// Commit snapshots a machine into a named flavor.
func (c *Client) Commit(id, name, description string) (*CommandResult, error) {
	variables := map[string]any{"id": id, "name": name, "description": description}
	return c.commandResult(commitMutation, variables, "commitMachine")
}

// -- ai agents ----------------------------------------------------------------
//
// A sandbox is a machine, so its terminal is the ordinary Shell with the argv
// AiShellCommand returns.

// AiAgents lists the coding agents and whether each one's image is built.
func (c *Client) AiAgents() ([]AiAgent, error) {
	data, err := c.Request(aiAgentsQuery, nil)
	if err != nil {
		return nil, err
	}
	rows, _ := data["aiAgents"].([]any)
	out := make([]AiAgent, 0, len(rows))
	for _, row := range rows {
		out = append(out, aiAgentFromGraphQL(asMap(row)))
	}
	return out, nil
}

// AiSessions lists agent sandboxes, newest first.
func (c *Client) AiSessions() ([]AiSession, error) {
	data, err := c.Request(aiSessionsQuery, nil)
	if err != nil {
		return nil, err
	}
	rows, _ := data["aiSessions"].([]any)
	out := make([]AiSession, 0, len(rows))
	for _, row := range rows {
		out = append(out, aiSessionFromGraphQL(asMap(row)))
	}
	return out, nil
}

// AiStartOpts tunes AiStart. The zero value reuses the agent's running
// sandbox and shares nothing.
type AiStartOpts struct {
	Cpus int
	Mem  int
	// Workspace is a directory **on the engine's host** to share, at the same
	// path. A remote daemon cannot see your own filesystem.
	Workspace string
	// New boots a second sandbox against the same saved login.
	New bool
}

// AiStart starts (or reuses) a sandbox and returns its machine id.
func (c *Client) AiStart(agent string, opts *AiStartOpts) (string, error) {
	if opts == nil {
		opts = &AiStartOpts{}
	}
	input := map[string]any{
		"agent":     agent,
		"cpus":      nil,
		"mem":       nil,
		"workspace": nil,
		"new":       opts.New,
	}
	if opts.Cpus != 0 {
		input["cpus"] = opts.Cpus
	}
	if opts.Mem != 0 {
		input["mem"] = opts.Mem
	}
	if opts.Workspace != "" {
		input["workspace"] = opts.Workspace
	}
	data, err := c.Request(aiStartMutation, map[string]any{"input": input})
	if err != nil {
		return "", err
	}
	return asString(data["aiStart"]), nil
}

// AiShellCommand returns the argv that starts the agent's TUI — pass it to
// Shell.
func (c *Client) AiShellCommand(agent, machineID string) ([]string, error) {
	data, err := c.Request(aiShellCommandQuery,
		map[string]any{"agent": agent, "machineId": machineID})
	if err != nil {
		return nil, err
	}
	raw, _ := data["aiShellCommand"].([]any)
	out := make([]string, 0, len(raw))
	for _, v := range raw {
		out = append(out, asString(v))
	}
	return out, nil
}

// AiStop stops an agent's sandboxes. Its saved login survives.
func (c *Client) AiStop(agent string) (*CommandResult, error) {
	return c.commandResult(aiStopMutation, map[string]any{"agent": agent}, "aiStop")
}

// AiRemove removes an agent's sandboxes, and unless keepHome its login too.
func (c *Client) AiRemove(agent string, keepHome bool) (*CommandResult, error) {
	variables := map[string]any{"agent": agent, "keepHome": keepHome}
	return c.commandResult(aiRemoveMutation, variables, "aiRemove")
}

// -- docker -------------------------------------------------------------------
//
// bsdkrun runs one `docker:dind` microVM and serves its API on a host unix
// socket, so these drive the same engine the host's `docker` CLI does.

// DockerStatus reports whether the Docker engine is up, and where its socket
// is.
func (c *Client) DockerStatus() (*DockerStatus, error) {
	data, err := c.Request(dockerStatusQuery, nil)
	if err != nil {
		return nil, err
	}
	s := dockerStatusFromGraphQL(asMap(data["dockerStatus"]))
	return &s, nil
}

// DockerContainers lists containers. all=false lists only running ones.
func (c *Client) DockerContainers(all bool) ([]DockerContainer, error) {
	data, err := c.Request(dockerContainersQuery, map[string]any{"all": all})
	if err != nil {
		return nil, err
	}
	rows, _ := data["dockerContainers"].([]any)
	out := make([]DockerContainer, 0, len(rows))
	for _, row := range rows {
		out = append(out, dockerContainerFromGraphQL(asMap(row)))
	}
	return out, nil
}

// DockerStartOpts tunes DockerStart. The zero value is what
// `bsdkrun docker start` with no flags does.
type DockerStartOpts struct {
	Cpus int
	Mem  int
	// Mounts are host directories to share, each "PATH" or "HOST:GUEST".
	Mounts []string
	// NoHome opts out of sharing $HOME (shared by default).
	NoHome bool
	// PublishBind is where published ports bind on the host: "mirror"
	// (default) or a fixed address.
	PublishBind string
	// DiskSize gives the image store a dedicated disk, e.g. "60G".
	DiskSize string
}

// DockerStart starts (or resumes) the engine, returning its status once the
// daemon inside answers.
//
// Idempotent: the VM has a fixed name, so this resumes the existing one rather
// than creating a second.
func (c *Client) DockerStart(opts *DockerStartOpts) (*DockerStatus, error) {
	if opts == nil {
		opts = &DockerStartOpts{}
	}
	input := map[string]any{
		"cpus":        nil,
		"mem":         nil,
		"mounts":      opts.Mounts,
		"noHome":      opts.NoHome,
		"publishBind": nil,
		"diskSize":    nil,
	}
	if opts.Cpus != 0 {
		input["cpus"] = opts.Cpus
	}
	if opts.Mem != 0 {
		input["mem"] = opts.Mem
	}
	if opts.PublishBind != "" {
		input["publishBind"] = opts.PublishBind
	}
	if opts.DiskSize != "" {
		input["diskSize"] = opts.DiskSize
	}
	if opts.Mounts == nil {
		input["mounts"] = []string{}
	}
	data, err := c.Request(dockerStartMutation, map[string]any{"input": input})
	if err != nil {
		return nil, err
	}
	s := dockerStatusFromGraphQL(asMap(data["dockerStart"]))
	return &s, nil
}

// DockerStop stops the engine. Images and containers stay on its disk.
func (c *Client) DockerStop() (*CommandResult, error) {
	return c.commandResult(dockerStopMutation, nil, "dockerStop")
}

// DockerContainer acts on containers: start / stop / restart / kill / pause /
// unpause / rm.
func (c *Client) DockerContainer(action string, ids []string) (*CommandResult, error) {
	variables := map[string]any{"action": action, "ids": ids}
	return c.commandResult(dockerContainerMutation, variables, "dockerContainer")
}

// DockerLogs returns one container's logs (stdout+stderr, most recent tail
// lines).
func (c *Client) DockerLogs(id string, tail int) (string, error) {
	if tail <= 0 {
		tail = 200
	}
	data, err := c.Request(dockerLogsQuery, map[string]any{"id": id, "tail": tail})
	if err != nil {
		return "", err
	}
	return asString(data["dockerContainerLogs"]), nil
}

// -- snapshots --------------------------------------------------------------
//
// A snapshot is a copy-on-write clone of a machine's disk state: instant to
// take, free until the two sides diverge. Branch boots a new machine from
// one; Restore/Rollback put one back.

// Snapshots lists snapshots, newest first. A non-empty machine narrows the
// list to that machine's.
func (c *Client) Snapshots(machine string) ([]SnapshotInfo, error) {
	variables := map[string]any{"machine": nil}
	if machine != "" {
		variables["machine"] = machine
	}
	data, err := c.Request(snapshotsQuery, variables)
	if err != nil {
		return nil, err
	}
	rows, _ := data["snapshots"].([]any)
	out := make([]SnapshotInfo, 0, len(rows))
	for _, row := range rows {
		out = append(out, snapshotInfoFromGraphQL(asMap(row)))
	}
	return out, nil
}

// Snapshot captures a machine's disk state. An empty name is filled in by the
// engine as "<machine>-<n>".
//
// A BSD guest is powered off first — a mounted UFS cannot be cloned
// consistently — so the machine is left stopped; Start brings it back.
func (c *Client) Snapshot(id, name, description string) (*SnapshotInfo, error) {
	variables := map[string]any{"id": id, "name": nil, "description": description}
	if name != "" {
		variables["name"] = name
	}
	data, err := c.Request(snapshotMutation, variables)
	if err != nil {
		return nil, err
	}
	info := snapshotInfoFromGraphQL(asMap(data["snapshotMachine"]))
	return &info, nil
}

// RemoveSnapshots deletes snapshots and their data. Machines already branched
// from them are unaffected.
func (c *Client) RemoveSnapshots(names []string) (*CommandResult, error) {
	return c.commandResult(removeSnapshotsMutation, map[string]any{"names": names}, "removeSnapshots")
}

// Restore puts a machine's disk state back to one of its snapshots.
//
// force stops the machine first (it holds the very files being replaced);
// backup snapshots the state being overwritten, which is a CoW clone and
// therefore free. The machine is left stopped.
func (c *Client) Restore(id, snapshot string, force, backup bool) (*CommandResult, error) {
	variables := map[string]any{"id": id, "snapshot": snapshot, "force": force, "backup": backup}
	return c.commandResult(restoreMutation, variables, "restoreMachine")
}

// Rollback restores a machine to its most recent snapshot.
func (c *Client) Rollback(id string, force, backup bool) (*CommandResult, error) {
	variables := map[string]any{"id": id, "force": force, "backup": backup}
	return c.commandResult(rollbackMutation, variables, "rollbackMachine")
}

// BranchOpts tunes Branch. The zero value inherits everything the snapshot
// recorded and lets the engine name the machine.
type BranchOpts struct {
	// Name for the new machine; generated when empty.
	Name string
	// Cpus / Mem default to what the snapshot recorded when 0.
	Cpus int
	Mem  int
	// Ports are host↔guest forwards, each "[BIND:]HOST:GUEST". Empty inherits
	// the snapshot's, remapping any host port that is already taken.
	Ports []string
	// NoPorts forwards nothing, ignoring what the snapshot recorded.
	NoPorts bool
}

// Branch boots a NEW machine from a snapshot — or from a machine, which is
// snapshotted first — and returns the new machine's id.
//
// The state is cloned, never booted in place, so the source is untouched and
// one snapshot can be branched any number of times.
func (c *Client) Branch(snapshot string, opts *BranchOpts) (string, error) {
	if opts == nil {
		opts = &BranchOpts{}
	}
	input := map[string]any{
		"snapshot": snapshot,
		"name":     nil,
		"cpus":     nil,
		"mem":      nil,
		"ports":    opts.Ports,
		"noPorts":  opts.NoPorts,
	}
	if opts.Name != "" {
		input["name"] = opts.Name
	}
	if opts.Cpus != 0 {
		input["cpus"] = opts.Cpus
	}
	if opts.Mem != 0 {
		input["mem"] = opts.Mem
	}
	if opts.Ports == nil {
		input["ports"] = []string{}
	}
	data, err := c.Request(branchMutation, map[string]any{"input": input})
	if err != nil {
		return "", err
	}
	return asString(data["branchSnapshot"]), nil
}

// Logs is a one-shot read of a machine's console log (or bsdkrun's boot log
// with boot=true).
func (c *Client) Logs(id string, boot bool) (string, error) {
	data, err := c.Request(logsQuery, map[string]any{"id": id, "boot": boot})
	if err != nil {
		return "", err
	}
	return asString(data["machineLogs"]), nil
}

// FollowLogsOpts tunes FollowLogs. The zero value follows the console log
// live.
type FollowLogsOpts struct {
	// NoFollow reads what's there and completes instead of tailing.
	NoFollow bool
	// Boot streams bsdkrun's boot log instead of the console log.
	Boot       bool
	OnError    func(err error)
	OnComplete func()
}

// FollowLogs streams a machine's console log live. It returns an
// unsubscribe function.
func (c *Client) FollowLogs(id string, onData func([]byte), opts *FollowLogsOpts) (func(), error) {
	if opts == nil {
		opts = &FollowLogsOpts{}
	}
	transport := c.wsTransport()

	onNext := func(data any) {
		payload := asMap(asMap(data)["machineLogs"])
		if payload == nil {
			return
		}
		if b64 := asString(payload["dataBase64"]); b64 != "" {
			if raw, err := base64.StdEncoding.DecodeString(b64); err == nil {
				onData(raw)
			}
		}
		// exitCode marks the stream's end; graphql-transport-ws follows it
		// with its own "complete" message, which fires OnComplete.
	}

	subID, err := transport.subscribe(
		machineLogsSubscription,
		map[string]any{"id": id, "follow": !opts.NoFollow, "boot": opts.Boot},
		onNext,
		opts.OnError,
		opts.OnComplete,
	)
	if err != nil {
		return nil, err
	}
	return func() { transport.unsubscribe(subID) }, nil
}

// -- exec / interactive shell -----------------------------------------------

// Exec runs a command to completion via the machine's shell agent. env
// entries are "KEY=VALUE" strings.
//
// Sequenced exactly as daemon/README.md describes: openShell (with command
// set, so the session runs it instead of a login shell), THEN subscribe to
// shellOutput (output is buffered from the moment the session opened, so
// nothing is lost even though the subscribe necessarily happens after the
// mutation), collecting bytes until an event carries a non-null exit code,
// THEN closeShell — called unconditionally, including on error, since it is
// idempotent and a session must never be left dangling.
func (c *Client) Exec(id string, command []string, env ...string) (*ExecResult, error) {
	transport := c.wsTransport()
	data, err := c.Request(openShellMutation, map[string]any{
		"machineId": id,
		"command":   stringsOrEmpty(command),
		"env":       stringsOrEmpty(env),
		"rows":      24,
		"cols":      80,
	})
	if err != nil {
		return nil, err
	}
	session := shellSessionInfoFromGraphQL(asMap(data["openShell"]))

	var chunksMu sync.Mutex
	var chunks [][]byte
	type doneMsg struct {
		code int
		err  error
	}
	// The reader goroutine delivers shellOutput events via callbacks; this
	// channel is how the calling goroutine blocks until the one it cares
	// about (an exit code, or a terminal error) arrives, keeping Exec a
	// synchronous call. Buffered + non-blocking sends: after the exit code,
	// a late complete/error must never wedge the reader.
	done := make(chan doneMsg, 2)
	push := func(m doneMsg) {
		select {
		case done <- m:
		default:
		}
	}

	onNext := func(data any) {
		p := asMap(asMap(data)["shellOutput"])
		if p == nil {
			return
		}
		if b64 := asString(p["dataBase64"]); b64 != "" {
			if raw, err := base64.StdEncoding.DecodeString(b64); err == nil {
				chunksMu.Lock()
				chunks = append(chunks, raw)
				chunksMu.Unlock()
			}
		}
		if code, ok := asInt(p["exitCode"]); ok {
			push(doneMsg{code: int(code)})
		}
	}
	onError := func(err error) {
		push(doneMsg{err: err})
	}
	onComplete := func() {
		// The subscription ended without ever delivering an exit code
		// (e.g. the daemon tore the session down) — surface that instead
		// of blocking forever.
		push(doneMsg{err: &GraphQLError{Message: "shell session ended before an exit code arrived"}})
	}

	closeSession := func() {
		// closeShell is idempotent; an already-gone session is not a failure.
		_, _ = c.Request(closeMutation, map[string]any{"sessionId": session.ID})
	}

	subID, err := transport.subscribe(
		shellOutputSubscription,
		map[string]any{"sessionId": session.ID},
		onNext,
		onError,
		onComplete,
	)
	if err != nil {
		closeSession()
		return nil, err
	}

	msg := <-done
	transport.unsubscribe(subID)
	closeSession()

	if msg.err != nil {
		return nil, msg.err
	}
	chunksMu.Lock()
	defer chunksMu.Unlock()
	var output []byte
	for _, chunk := range chunks {
		output = append(output, chunk...)
	}
	return &ExecResult{ExitCode: msg.code, Output: output}, nil
}

// ShellOpts tunes Client.Shell. The zero value opens a 24x80 login shell.
type ShellOpts struct {
	// Command, when set, runs instead of a login shell.
	Command []string
	// Env entries are "KEY=VALUE" strings.
	Env  []string
	Rows int
	Cols int
}

// Shell opens a live interactive session. Output/exit arrive via the
// session's OnOutput/OnExit callbacks.
func (c *Client) Shell(id string, opts *ShellOpts) (*ShellSession, error) {
	if opts == nil {
		opts = &ShellOpts{}
	}
	rows, cols := opts.Rows, opts.Cols
	if rows == 0 {
		rows = 24
	}
	if cols == 0 {
		cols = 80
	}

	transport := c.wsTransport()
	data, err := c.Request(openShellMutation, map[string]any{
		"machineId": id,
		"command":   stringsOrEmpty(opts.Command),
		"env":       stringsOrEmpty(opts.Env),
		"rows":      rows,
		"cols":      cols,
	})
	if err != nil {
		return nil, err
	}
	info := shellSessionInfoFromGraphQL(asMap(data["openShell"]))
	session := &ShellSession{ID: info.ID, client: c, transport: transport}
	if err := session.start(); err != nil {
		session.Close()
		return nil, err
	}
	return session, nil
}

// stringsOrEmpty keeps GraphQL list variables as [] rather than null — the
// daemon's [String!]! arguments reject null.
func stringsOrEmpty(values []string) []string {
	if values == nil {
		return []string{}
	}
	return values
}
