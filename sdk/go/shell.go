package bsdkrun

import (
	"encoding/base64"
	"sync"
)

// ShellSession is a live interactive session opened by Client.Shell.
//
// Output and exit events arrive on the shared wsTransport's background
// reader goroutine and are handed to whatever callback is registered via
// OnOutput/OnExit at the time they arrive. Anything that arrives *before* a
// callback is registered (a real possibility — the subscription is started
// synchronously in Client.Shell and the daemon can reply before the caller
// gets around to calling OnOutput) is buffered and flushed the moment a
// callback is set, so a caller that does
//
//	s, _ := client.Shell(id, nil)
//	s.OnOutput(cb)
//
// never silently loses a frame.
type ShellSession struct {
	ID string

	client    *Client
	transport *wsTransport
	subID     string
	closed    bool

	cbMu           sync.Mutex
	outputCb       func([]byte)
	exitCb         func(int)
	bufferedOutput [][]byte
	bufferedExit   *int
	exitFired      bool
}

func (s *ShellSession) start() error {
	subID, err := s.transport.subscribe(
		shellOutputSubscription,
		map[string]any{"sessionId": s.ID},
		s.onNext,
		s.onError,
		func() {},
	)
	if err != nil {
		return err
	}
	s.subID = subID
	return nil
}

func (s *ShellSession) onNext(data any) {
	p := asMap(asMap(data)["shellOutput"])
	if p == nil {
		return
	}
	if b64 := asString(p["dataBase64"]); b64 != "" {
		if raw, err := base64.StdEncoding.DecodeString(b64); err == nil {
			s.emitOutput(raw)
		}
	}
	if code, ok := asInt(p["exitCode"]); ok {
		s.emitExit(int(code))
	}
}

func (s *ShellSession) onError(error) {
	// A dropped connection ends the session the same way an exit would, so
	// a caller has one place (OnExit) to notice the session is gone. -1 has
	// no exit-code meaning of its own; it just isn't 0.
	s.emitExit(-1)
}

func (s *ShellSession) emitOutput(data []byte) {
	s.cbMu.Lock()
	cb := s.outputCb
	if cb == nil {
		s.bufferedOutput = append(s.bufferedOutput, data)
	}
	s.cbMu.Unlock()
	if cb != nil {
		cb(data)
	}
}

func (s *ShellSession) emitExit(code int) {
	s.cbMu.Lock()
	if s.exitFired {
		s.cbMu.Unlock()
		return
	}
	s.exitFired = true
	cb := s.exitCb
	if cb == nil {
		s.bufferedExit = &code
	}
	s.cbMu.Unlock()
	if cb != nil {
		cb(code)
	}
}

// OnOutput registers the output callback; anything buffered before the
// registration is flushed to it immediately.
func (s *ShellSession) OnOutput(cb func(data []byte)) {
	s.cbMu.Lock()
	s.outputCb = cb
	buffered := s.bufferedOutput
	s.bufferedOutput = nil
	s.cbMu.Unlock()
	for _, chunk := range buffered {
		cb(chunk)
	}
}

// OnExit registers the exit callback; a buffered exit code is delivered
// immediately.
func (s *ShellSession) OnExit(cb func(code int)) {
	s.cbMu.Lock()
	s.exitCb = cb
	pending := s.bufferedExit
	s.bufferedExit = nil
	s.cbMu.Unlock()
	if pending != nil {
		cb(*pending)
	}
}

// Write sends bytes to the session's stdin.
func (s *ShellSession) Write(data []byte) error {
	_, err := s.client.Request(sendInputMutation, map[string]any{
		"sessionId":  s.ID,
		"dataBase64": base64.StdEncoding.EncodeToString(data),
	})
	return err
}

// WriteString sends a string to the session's stdin.
func (s *ShellSession) WriteString(data string) error {
	return s.Write([]byte(data))
}

// Resize resizes the session's PTY.
func (s *ShellSession) Resize(rows, cols int) error {
	_, err := s.client.Request(resizeMutation, map[string]any{
		"sessionId": s.ID,
		"rows":      rows,
		"cols":      cols,
	})
	return err
}

// Close closes the session and kills its command. Idempotent.
func (s *ShellSession) Close() error {
	if s.closed {
		return nil
	}
	s.closed = true
	if s.subID != "" {
		s.transport.unsubscribe(s.subID)
	}
	// closeShell is idempotent; an already-gone session is not a failure.
	_, _ = s.client.Request(closeMutation, map[string]any{"sessionId": s.ID})
	return nil
}
