package bsdkrun

// Client tests against in-process fake servers speaking exactly what the
// real bsdkrund speaks: GraphQL-over-HTTP (httptest) and
// graphql-transport-ws over a hand-rolled WebSocket (a raw net.Listener
// driving this package's own frame codec) — no real daemon needed.

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"
)

// -- fake HTTP server -------------------------------------------------------

type gqlCall struct {
	Query     string
	Variables map[string]any
}

// fakeHTTPServer dispatches on substrings in the query text rather than
// actually parsing GraphQL, which is all a unit test needs.
type fakeHTTPServer struct {
	srv      *httptest.Server
	mu       sync.Mutex
	calls    []gqlCall
	machines []map[string]any
	// override, when set, replaces the default dispatch entirely and
	// returns (status, full response body).
	override func(query string, variables map[string]any) (int, any)
}

func newFakeHTTPServer(t *testing.T) *fakeHTTPServer {
	t.Helper()
	f := &fakeHTTPServer{}
	f.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body struct {
			Query     string         `json:"query"`
			Variables map[string]any `json:"variables"`
		}
		_ = json.NewDecoder(r.Body).Decode(&body)
		f.mu.Lock()
		f.calls = append(f.calls, gqlCall{Query: body.Query, Variables: body.Variables})
		override := f.override
		f.mu.Unlock()

		status, response := 200, any(map[string]any{"data": f.dispatch(body.Query, body.Variables)})
		if override != nil {
			status, response = override(body.Query, body.Variables)
		}
		w.Header().Set("content-type", "application/json")
		w.WriteHeader(status)
		_ = json.NewEncoder(w).Encode(response)
	}))
	t.Cleanup(f.srv.Close)
	return f
}

func (f *fakeHTTPServer) url() string { return f.srv.URL + "/graphql" }

func (f *fakeHTTPServer) recorded() []gqlCall {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]gqlCall(nil), f.calls...)
}

func (f *fakeHTTPServer) dispatch(query string, variables map[string]any) map[string]any {
	switch {
	case strings.Contains(query, "openShell"):
		return map[string]any{"openShell": map[string]any{
			"id":        "sess-1",
			"machineId": variables["machineId"],
			"finished":  false,
			"truncated": false,
		}}
	case strings.Contains(query, "closeShell"):
		return map[string]any{"closeShell": true}
	case strings.Contains(query, "sendShellInput"):
		return map[string]any{"sendShellInput": true}
	case strings.Contains(query, "resizeShell"):
		return map[string]any{"resizeShell": true}
	case strings.Contains(query, "stopMachine"):
		return map[string]any{"stopMachine": map[string]any{"exitCode": 0, "stdout": "stopped", "stderr": ""}}
	case strings.Contains(query, "runLinux"):
		return map[string]any{"runLinux": "m-linux"}
	case strings.Contains(query, "runBsd"):
		return map[string]any{"runBsd": "m-bsd"}
	case strings.Contains(query, "runFlavor"):
		return map[string]any{"runFlavor": "m-flavor"}
	case strings.Contains(query, "machine("):
		id, _ := variables["id"].(string)
		for _, m := range f.machines {
			if m["id"] == id {
				return map[string]any{"machine": m}
			}
		}
		return map[string]any{"machine": nil}
	case strings.Contains(query, "machines("):
		rows := make([]any, 0, len(f.machines))
		for _, m := range f.machines {
			rows = append(rows, m)
		}
		return map[string]any{"machines": rows}
	}
	return map[string]any{}
}

// -- fake WebSocket server --------------------------------------------------

// fakeWSServer is a minimal graphql-transport-ws server. react is invoked
// for every parsed client text frame with a send function that writes an
// unmasked server frame back, exactly the shape the real daemon would.
type fakeWSServer struct {
	ln       net.Listener
	mu       sync.Mutex
	received []map[string]any
	react    func(send func(any), msg map[string]any, conn net.Conn)
}

func newFakeWSServer(t *testing.T, react func(send func(any), msg map[string]any, conn net.Conn)) *fakeWSServer {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	f := &fakeWSServer{ln: ln, react: react}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go f.handle(conn)
		}
	}()
	return f
}

func (f *fakeWSServer) url() string {
	return "ws://" + f.ln.Addr().String() + "/graphql/ws"
}

func (f *fakeWSServer) recorded() []map[string]any {
	f.mu.Lock()
	defer f.mu.Unlock()
	return append([]map[string]any(nil), f.received...)
}

func (f *fakeWSServer) handle(conn net.Conn) {
	defer conn.Close()

	// HTTP Upgrade handshake.
	var buf []byte
	chunk := make([]byte, 4096)
	for !strings.Contains(string(buf), "\r\n\r\n") {
		n, err := conn.Read(chunk)
		if n > 0 {
			buf = append(buf, chunk[:n]...)
		}
		if err != nil {
			return
		}
	}
	head, rest, _ := strings.Cut(string(buf), "\r\n\r\n")
	key := ""
	for _, line := range strings.Split(head, "\r\n") {
		if name, value, ok := strings.Cut(line, ":"); ok &&
			strings.EqualFold(strings.TrimSpace(name), "Sec-WebSocket-Key") {
			key = strings.TrimSpace(value)
		}
	}
	_, _ = conn.Write([]byte("HTTP/1.1 101 Switching Protocols\r\n" +
		"Upgrade: websocket\r\n" +
		"Connection: Upgrade\r\n" +
		"Sec-WebSocket-Accept: " + computeAccept(key) + "\r\n" +
		"Sec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"))

	var writeMu sync.Mutex
	send := func(obj any) {
		payload, _ := json.Marshal(obj)
		writeMu.Lock()
		defer writeMu.Unlock()
		// Server->client frames must NOT be masked.
		_, _ = conn.Write(buildFrame(payload, opText, false, true))
	}

	frames := []byte(rest)
	for {
		frame, consumed := parseFrame(frames)
		if frame == nil {
			n, err := conn.Read(chunk)
			if n > 0 {
				frames = append(frames, chunk[:n]...)
			}
			if err != nil && n == 0 {
				return
			}
			continue
		}
		frames = frames[consumed:]
		switch frame.opcode {
		case opText:
			var msg map[string]any
			if err := json.Unmarshal(frame.payload, &msg); err == nil {
				f.mu.Lock()
				f.received = append(f.received, msg)
				f.mu.Unlock()
				if f.react != nil {
					f.react(send, msg, conn)
				}
			}
		case opClose:
			return
		}
	}
}

// ackAndScriptShellOutput is the standard react: auto-ack connection_init
// and, on subscribe, replay chunks followed by an exit code — exactly the
// shape Exec waits on.
func ackAndScriptShellOutput(chunks []string, exitCode int) func(func(any), map[string]any, net.Conn) {
	return func(send func(any), msg map[string]any, _ net.Conn) {
		switch msg["type"] {
		case "connection_init":
			send(map[string]any{"type": "connection_ack"})
		case "subscribe":
			id := msg["id"]
			for _, chunk := range chunks {
				send(map[string]any{"type": "next", "id": id, "payload": map[string]any{
					"data": map[string]any{"shellOutput": map[string]any{
						"dataBase64": base64.StdEncoding.EncodeToString([]byte(chunk)),
						"exitCode":   nil,
					}},
				}})
			}
			send(map[string]any{"type": "next", "id": id, "payload": map[string]any{
				"data": map[string]any{"shellOutput": map[string]any{
					"dataBase64": nil,
					"exitCode":   exitCode,
				}},
			}})
		}
	}
}

func newTestClient(t *testing.T, httpSrv *fakeHTTPServer, wsSrv *fakeWSServer) *Client {
	t.Helper()
	client, err := NewClient(httpSrv.url(), "tok")
	if err != nil {
		t.Fatal(err)
	}
	if wsSrv != nil {
		// Point the WS side at the fake WS server directly, bypassing the
		// normal URL derivation (covered by TestWSEndpoint).
		client.ws = newWSTransport(wsSrv.url(), "tok")
	}
	return client
}

func jsonNorm(t *testing.T, v any) any {
	t.Helper()
	raw, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	var out any
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatal(err)
	}
	return out
}

// -- construction -----------------------------------------------------------

func TestClientFromEnv(t *testing.T) {
	t.Setenv(EnvURL, "")
	t.Setenv(EnvToken, "")
	if _, err := ClientFromEnv(); err == nil {
		t.Fatal("no URL should fail")
	}

	t.Setenv(EnvURL, "http://localhost:50052")
	if _, err := ClientFromEnv(); err == nil {
		t.Fatal("URL without token should fail")
	}

	t.Setenv(EnvToken, "   ")
	if _, err := ClientFromEnv(); err == nil {
		t.Fatal("blank token should count as unset")
	}

	t.Setenv(EnvURL, "localhost:50052")
	t.Setenv(EnvToken, "secret")
	client, err := ClientFromEnv()
	if err != nil {
		t.Fatal(err)
	}
	if client.URL != "http://localhost:50052/graphql" || client.Token != "secret" {
		t.Fatalf("%+v", client)
	}
}

func TestNewClientRejectsMissingHalves(t *testing.T) {
	if _, err := NewClient("", "tok"); err == nil {
		t.Fatal("empty URL should fail")
	}
	if _, err := NewClient("http://host", ""); err == nil {
		t.Fatal("empty token should fail")
	}
	client, err := NewClient("host:50052", "tok")
	if err != nil || client.URL != "http://host:50052/graphql" {
		t.Fatalf("client=%v err=%v", client, err)
	}
}

// -- HTTP-backed methods ----------------------------------------------------

func graphqlMachineRow(id, name string) map[string]any {
	return map[string]any{
		"id": id, "name": name, "image": "alpine", "kind": "linux",
		"command": "sleep 1", "status": "running", "running": true,
		"exitCode": nil, "pid": 42, "detached": true, "cpus": 2, "mem": 512,
		"volume": nil, "stateDir": "/s", "createdAt": "1700000000",
		"finishedAt": nil, "network": nil, "netIp": nil,
		"ports": []any{map[string]any{"bind": "127.0.0.1", "host": 2222, "guest": 22}},
	}
}

func TestClientListAndGet(t *testing.T) {
	server := newFakeHTTPServer(t)
	server.machines = []map[string]any{graphqlMachineRow("abc123", "web")}
	client := newTestClient(t, server, nil)

	machines, err := client.List(true)
	if err != nil {
		t.Fatal(err)
	}
	if len(machines) != 1 || machines[0].ID != "abc123" || machines[0].CreatedAt != 1700000000 {
		t.Fatalf("%+v", machines)
	}
	if machines[0].PID == nil || *machines[0].PID != 42 || machines[0].Ports[0].Host != 2222 {
		t.Fatalf("%+v", machines[0])
	}

	found, err := client.Get("abc123")
	if err != nil || found == nil || found.Name != "web" {
		t.Fatalf("found=%v err=%v", found, err)
	}
	missing, err := client.Get("nope")
	if err != nil || missing != nil {
		t.Fatalf("missing=%v err=%v", missing, err)
	}
}

func TestClientStopAndLogs(t *testing.T) {
	server := newFakeHTTPServer(t)
	server.override = func(query string, _ map[string]any) (int, any) {
		if strings.Contains(query, "stopMachine") {
			return 200, map[string]any{"data": map[string]any{
				"stopMachine": map[string]any{"exitCode": 0, "stdout": "stopped", "stderr": ""},
			}}
		}
		return 200, map[string]any{"data": map[string]any{"machineLogs": "console text"}}
	}
	client := newTestClient(t, server, nil)

	result, err := client.Stop("abc123")
	if err != nil || result.ExitCode != 0 || result.Stdout != "stopped" {
		t.Fatalf("result=%v err=%v", result, err)
	}
	logs, err := client.Logs("abc123", false)
	if err != nil || logs != "console text" {
		t.Fatalf("logs=%q err=%v", logs, err)
	}
}

func TestClientAuthErrorOn401(t *testing.T) {
	server := newFakeHTTPServer(t)
	server.override = func(string, map[string]any) (int, any) {
		return 401, map[string]any{}
	}
	client := newTestClient(t, server, nil)

	var authErr *AuthError
	if _, err := client.List(false); !errors.As(err, &authErr) {
		t.Fatalf("err: %v", err)
	}
}

func TestClientGraphQLErrorCarriesCode(t *testing.T) {
	server := newFakeHTTPServer(t)
	server.override = func(string, map[string]any) (int, any) {
		return 200, map[string]any{"errors": []any{map[string]any{
			"message":    "no such machine",
			"extensions": map[string]any{"code": "INVALID_ARGUMENT"},
		}}}
	}
	client := newTestClient(t, server, nil)

	var gqlErr *GraphQLError
	_, err := client.List(false)
	if !errors.As(err, &gqlErr) || gqlErr.Code != "INVALID_ARGUMENT" || gqlErr.Message != "no such machine" {
		t.Fatalf("err: %v", err)
	}
}

func TestClientUnauthenticatedExtensionIsAuthError(t *testing.T) {
	server := newFakeHTTPServer(t)
	server.override = func(string, map[string]any) (int, any) {
		return 200, map[string]any{"errors": []any{map[string]any{
			"message":    "bad token",
			"extensions": map[string]any{"code": "UNAUTHENTICATED"},
		}}}
	}
	client := newTestClient(t, server, nil)

	var authErr *AuthError
	if _, err := client.List(false); !errors.As(err, &authErr) {
		t.Fatalf("err: %v", err)
	}
}

// -- run builders -----------------------------------------------------------

func TestRunLinuxInputShape(t *testing.T) {
	server := newFakeHTTPServer(t)
	client := newTestClient(t, server, nil)

	id, err := client.RunLinux().
		Image("alpine").
		Cpus(2).Mem(1024).
		Name("web").
		Port("8080:80").
		Env("X", "1").
		Command("sleep", "300").
		Launch()
	if err != nil || id != "m-linux" {
		t.Fatalf("id=%q err=%v", id, err)
	}

	calls := server.recorded()
	if len(calls) != 1 {
		t.Fatalf("calls: %d", len(calls))
	}
	want := jsonNorm(t, map[string]any{
		"image": "alpine",
		"cpus":  2,
		"mem":   1024,
		"net": map[string]any{
			"noNet":   false,
			"ports":   []string{"8080:80"},
			"mac":     nil,
			"network": nil,
			"name":    "web",
		},
		"volume":        nil,
		"mounts":        []string{},
		"env":           []string{"X=1"},
		"entrypoint":    nil,
		"initramfs":     false,
		"kernel":        nil,
		"kernelVersion": nil,
		"console":       nil,
		"repo":          nil,
		"command":       []string{"sleep", "300"},
	})
	if got := jsonNorm(t, calls[0].Variables["input"]); !reflect.DeepEqual(got, want) {
		t.Fatalf("input:\n got %v\nwant %v", got, want)
	}
}

func TestRunLinuxWithoutNetSendsNull(t *testing.T) {
	server := newFakeHTTPServer(t)
	client := newTestClient(t, server, nil)
	if _, err := client.RunLinux().Image("alpine").Launch(); err != nil {
		t.Fatal(err)
	}
	input := asMap(server.recorded()[0].Variables["input"])
	if net, present := input["net"]; !present || net != nil {
		t.Fatalf("net: %v (present %v)", net, present)
	}
}

func TestRunLinuxRequiresImage(t *testing.T) {
	client := newTestClient(t, newFakeHTTPServer(t), nil)
	if _, err := client.RunLinux().Cpus(2).Launch(); err == nil {
		t.Fatal("expected an error without Image()")
	}
}

func TestRunBSDValidatesOS(t *testing.T) {
	server := newFakeHTTPServer(t)
	client := newTestClient(t, server, nil)

	if _, err := client.RunBSD().Launch(); err == nil {
		t.Fatal("expected an error without OS()")
	}
	if _, err := client.RunBSD().OS("plan9").Launch(); err == nil {
		t.Fatal("expected an error for a bad OS")
	}

	id, err := client.RunBSD().OS("freebsd").Version("14.3").Mem(2048).Launch()
	if err != nil || id != "m-bsd" {
		t.Fatalf("id=%q err=%v", id, err)
	}
	input := asMap(server.recorded()[0].Variables["input"])
	if input["os"] != "FREEBSD" || input["version"] != "14.3" {
		t.Fatalf("input: %v", input)
	}
}

func TestRunFlavorPortsAreTopLevel(t *testing.T) {
	server := newFakeHTTPServer(t)
	client := newTestClient(t, server, nil)

	id, err := client.RunFlavor().Name("postgres").Port("5432:5432").Launch()
	if err != nil || id != "m-flavor" {
		t.Fatalf("id=%q err=%v", id, err)
	}
	input := asMap(server.recorded()[0].Variables["input"])
	want := jsonNorm(t, map[string]any{
		"name": "postgres", "cpus": nil, "mem": nil,
		"ports": []string{"5432:5432"}, "volume": nil, "repo": nil,
	})
	if got := jsonNorm(t, input); !reflect.DeepEqual(got, want) {
		t.Fatalf("input:\n got %v\nwant %v", got, want)
	}
}

// -- exec sequencing --------------------------------------------------------

func TestExecOpensSubscribesWaitsThenCloses(t *testing.T) {
	httpServer := newFakeHTTPServer(t)
	wsServer := newFakeWSServer(t, ackAndScriptShellOutput([]string{"hello ", "world\n"}, 7))
	client := newTestClient(t, httpServer, wsServer)

	result, err := client.Exec("machine123", []string{"echo", "hello world"})
	if err != nil {
		t.Fatal(err)
	}
	if result.ExitCode != 7 || string(result.Output) != "hello world\n" {
		t.Fatalf("%+v", result)
	}

	queries := httpServer.recorded()
	openIdx, closeIdx := -1, -1
	for i, call := range queries {
		if strings.Contains(call.Query, "openShell") {
			openIdx = i
		}
		if strings.Contains(call.Query, "closeShell") {
			closeIdx = i
		}
	}
	if openIdx == -1 || closeIdx == -1 || openIdx >= closeIdx {
		t.Fatalf("openShell/closeShell ordering: open=%d close=%d", openIdx, closeIdx)
	}

	// The open call carried the command through unmodified.
	openVars := queries[openIdx].Variables
	if !reflect.DeepEqual(jsonNorm(t, openVars["command"]), jsonNorm(t, []string{"echo", "hello world"})) {
		t.Fatalf("command: %v", openVars["command"])
	}
	if openVars["machineId"] != "machine123" {
		t.Fatalf("machineId: %v", openVars["machineId"])
	}

	var subs []map[string]any
	for _, msg := range wsServer.recorded() {
		if msg["type"] == "subscribe" {
			subs = append(subs, msg)
		}
	}
	if len(subs) != 1 {
		t.Fatalf("subscribes: %d", len(subs))
	}
	payload := asMap(subs[0]["payload"])
	if asMap(payload["variables"])["sessionId"] != "sess-1" {
		t.Fatalf("payload: %v", payload)
	}
}

// -- shell sessions ---------------------------------------------------------

func TestShellSessionBuffersEarlyOutput(t *testing.T) {
	httpServer := newFakeHTTPServer(t)
	wsServer := newFakeWSServer(t, func(send func(any), msg map[string]any, _ net.Conn) {
		switch msg["type"] {
		case "connection_init":
			send(map[string]any{"type": "connection_ack"})
		case "subscribe":
			// Output arrives immediately — before the caller has had a
			// chance to register OnOutput.
			send(map[string]any{"type": "next", "id": msg["id"], "payload": map[string]any{
				"data": map[string]any{"shellOutput": map[string]any{
					"dataBase64": base64.StdEncoding.EncodeToString([]byte("early$ ")),
					"exitCode":   nil,
				}},
			}})
		}
	})
	client := newTestClient(t, httpServer, wsServer)

	session, err := client.Shell("machine123", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer session.Close()

	// Wait for the frame to land in the buffer, proving the buffering path
	// (not just a late delivery) is what hands it to the callback.
	waitFor(t, func() bool {
		session.cbMu.Lock()
		defer session.cbMu.Unlock()
		return len(session.bufferedOutput) > 0
	})

	got := make(chan []byte, 1)
	session.OnOutput(func(data []byte) { got <- data })
	select {
	case data := <-got:
		if string(data) != "early$ " {
			t.Fatalf("data: %q", data)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("buffered output was never flushed")
	}

	// Write goes through the sendShellInput mutation over HTTP.
	if err := session.WriteString("ls\n"); err != nil {
		t.Fatal(err)
	}
	var sendCall *gqlCall
	for _, call := range httpServer.recorded() {
		if strings.Contains(call.Query, "sendShellInput") {
			sendCall = &call
		}
	}
	if sendCall == nil {
		t.Fatal("sendShellInput was never called")
	}
	raw, _ := base64.StdEncoding.DecodeString(asString(sendCall.Variables["dataBase64"]))
	if string(raw) != "ls\n" {
		t.Fatalf("input: %q", raw)
	}
	if err := session.Resize(50, 120); err != nil {
		t.Fatal(err)
	}
}

func TestShellSessionExitCallback(t *testing.T) {
	httpServer := newFakeHTTPServer(t)
	wsServer := newFakeWSServer(t, ackAndScriptShellOutput([]string{"bye"}, 0))
	client := newTestClient(t, httpServer, wsServer)

	session, err := client.Shell("machine123", &ShellOpts{Command: []string{"true"}})
	if err != nil {
		t.Fatal(err)
	}
	defer session.Close()

	exited := make(chan int, 1)
	session.OnExit(func(code int) { exited <- code })
	select {
	case code := <-exited:
		if code != 0 {
			t.Fatalf("exit code: %d", code)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("exit never fired")
	}
}

// -- raw subscriptions ------------------------------------------------------

func TestFollowLogsStreams(t *testing.T) {
	httpServer := newFakeHTTPServer(t)
	wsServer := newFakeWSServer(t, func(send func(any), msg map[string]any, _ net.Conn) {
		switch msg["type"] {
		case "connection_init":
			send(map[string]any{"type": "connection_ack"})
		case "subscribe":
			send(map[string]any{"type": "next", "id": msg["id"], "payload": map[string]any{
				"data": map[string]any{"machineLogs": map[string]any{
					"dataBase64": base64.StdEncoding.EncodeToString([]byte("boot line\n")),
					"exitCode":   nil,
				}},
			}})
			send(map[string]any{"type": "complete", "id": msg["id"]})
		}
	})
	client := newTestClient(t, httpServer, wsServer)

	data := make(chan []byte, 1)
	completed := make(chan struct{}, 1)
	unsubscribe, err := client.FollowLogs("machine123", func(chunk []byte) { data <- chunk }, &FollowLogsOpts{
		OnComplete: func() { completed <- struct{}{} },
	})
	if err != nil {
		t.Fatal(err)
	}
	defer unsubscribe()

	select {
	case chunk := <-data:
		if string(chunk) != "boot line\n" {
			t.Fatalf("chunk: %q", chunk)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("no log data arrived")
	}
	select {
	case <-completed:
	case <-time.After(2 * time.Second):
		t.Fatal("complete never fired")
	}
}

func TestSubscribeErrorMessage(t *testing.T) {
	httpServer := newFakeHTTPServer(t)
	wsServer := newFakeWSServer(t, func(send func(any), msg map[string]any, _ net.Conn) {
		switch msg["type"] {
		case "connection_init":
			send(map[string]any{"type": "connection_ack"})
		case "subscribe":
			send(map[string]any{"type": "error", "id": msg["id"], "payload": []any{
				map[string]any{"message": "boom"},
			}})
		}
	})
	client := newTestClient(t, httpServer, wsServer)

	errs := make(chan error, 1)
	_, err := client.Subscribe("subscription { x }", nil, SubscriptionHandlers{
		OnNext:  func(any) {},
		OnError: func(err error) { errs <- err },
	})
	if err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-errs:
		var gqlErr *GraphQLError
		if !errors.As(err, &gqlErr) || gqlErr.Message != "boom" {
			t.Fatalf("err: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("error never fired")
	}
}

func TestWSCloseBeforeAckIsAuthError(t *testing.T) {
	httpServer := newFakeHTTPServer(t)
	wsServer := newFakeWSServer(t, func(_ func(any), msg map[string]any, conn net.Conn) {
		// The daemon rejects connection_init (bad token) by hanging up
		// without ever sending connection_ack.
		if msg["type"] == "connection_init" {
			conn.Close()
		}
	})
	client := newTestClient(t, httpServer, wsServer)

	errs := make(chan error, 1)
	_, err := client.Subscribe("subscription { x }", nil, SubscriptionHandlers{
		OnNext:  func(any) {},
		OnError: func(err error) { errs <- err },
	})
	if err != nil {
		t.Fatal(err)
	}
	select {
	case err := <-errs:
		var authErr *AuthError
		if !errors.As(err, &authErr) {
			t.Fatalf("err: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("error never fired")
	}
}

func waitFor(t *testing.T, condition func() bool) {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for !condition() {
		if time.Now().After(deadline) {
			t.Fatal("condition never became true")
		}
		time.Sleep(5 * time.Millisecond)
	}
}
