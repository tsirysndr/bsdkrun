//go:build spindle

package main

// `/logs/{knot}/{rkey}/{name}` — spindle's raw log stream.
//
// It predates sh.tangled.ci.subscribePipelineLogs (which spindle's own XRPC
// router serves, CBOR-framed, and which this server therefore gets for free),
// but clients still use it, so a drop-in has to answer it identically: a
// WebSocket carrying one text frame per line of the workflow's JSONL log,
// replayed from the start. A finished workflow's log is replayed and the
// stream closed; a running one is followed until it reaches a finish state.

import (
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"time"

	"github.com/gorilla/websocket"
	"tangled.org/core/spindle/models"
)

var logUpgrader = websocket.Upgrader{
	ReadBufferSize:  1024,
	WriteBufferSize: 1024,
}

const (
	logWriteWait = 10 * time.Second
	logPingEvery = 30 * time.Second
	logPollEvery = 500 * time.Millisecond
)

func (s *spindleServer) logs(w http.ResponseWriter, r *http.Request) {
	knot := r.PathValue("knot")
	rkey := r.PathValue("rkey")
	name := r.PathValue("name")
	if knot == "" || rkey == "" || name == "" {
		http.Error(w, "missing required parameters", http.StatusBadRequest)
		return
	}
	wid := models.WorkflowId{PipelineId: models.PipelineId{Knot: knot, Rkey: rkey}, Name: name}

	conn, err := logUpgrader.Upgrade(w, r, nil)
	if err != nil {
		s.l.Error("upgrading the log stream", "err", err)
		return
	}
	defer conn.Close()

	if err := s.streamLogFile(conn, wid); err != nil && !errors.Is(err, io.EOF) {
		s.l.Error("streaming logs", "workflow", wid.String(), "err", err)
	}
	_ = conn.WriteControl(websocket.CloseMessage,
		websocket.FormatCloseMessage(websocket.CloseNormalClosure, "log stream complete"),
		time.Now().Add(logWriteWait))
}

// streamLogFile replays the workflow's log and, while the workflow is still
// running, keeps following it. Following polls rather than using inotify: the
// log is appended by this process, the file is small, and a poll costs nothing
// next to booting a VM.
func (s *spindleServer) streamLogFile(conn *websocket.Conn, wid models.WorkflowId) error {
	path := models.LogFilePath(s.cfg.Server.LogDir, wid)

	// A workflow that has not started yet has no file; wait briefly for it
	// rather than closing on a client that subscribed a moment too early.
	f, err := openWhenReady(path, 5*time.Second)
	if err != nil {
		return err
	}
	defer f.Close()

	// Client reads are discarded, but they must be drained: without a reader
	// the connection never sees a close frame and the writer blocks forever.
	go func() {
		for {
			if _, _, err := conn.ReadMessage(); err != nil {
				return
			}
		}
	}()

	reader := newLineReader(f)
	lastPing := time.Now()
	for {
		line, err := reader.next()
		if err != nil && !errors.Is(err, io.EOF) {
			return err
		}
		if line != nil {
			if werr := writeText(conn, line); werr != nil {
				return werr
			}
			continue
		}

		// Caught up. Stop once the workflow has reached a finish state and
		// nothing more can be appended.
		if st, serr := s.db.GetStatus(wid); serr == nil && st != nil &&
			models.StatusKind(st.Status).IsFinish() {
			// One more pass: the finishing write may have landed between the
			// read above and this check.
			if line, err := reader.next(); err == nil && line != nil {
				_ = writeText(conn, line)
				continue
			}
			return nil
		}
		if time.Since(lastPing) >= logPingEvery {
			if perr := conn.WriteControl(websocket.PingMessage, nil, time.Now().Add(logWriteWait)); perr != nil {
				return perr
			}
			lastPing = time.Now()
		}
		time.Sleep(logPollEvery)
	}
}

func writeText(conn *websocket.Conn, line []byte) error {
	if err := conn.SetWriteDeadline(time.Now().Add(logWriteWait)); err != nil {
		return err
	}
	return conn.WriteMessage(websocket.TextMessage, line)
}

func openWhenReady(path string, wait time.Duration) (*os.File, error) {
	deadline := time.Now().Add(wait)
	for {
		f, err := os.Open(path)
		if err == nil {
			return f, nil
		}
		if !errors.Is(err, os.ErrNotExist) || time.Now().After(deadline) {
			return nil, fmt.Errorf("opening %s: %w", path, err)
		}
		time.Sleep(logPollEvery)
	}
}

// lineReader hands back whole lines only: a tailed file is routinely read
// mid-write, and half a JSON object is worse than waiting for the rest.
type lineReader struct {
	f   *os.File
	buf []byte
}

func newLineReader(f *os.File) *lineReader { return &lineReader{f: f} }

func (r *lineReader) next() ([]byte, error) {
	for {
		for i, b := range r.buf {
			if b == '\n' {
				line := r.buf[:i]
				r.buf = r.buf[i+1:]
				out := make([]byte, len(line))
				copy(out, line)
				return out, nil
			}
		}
		chunk := make([]byte, 32*1024)
		n, err := r.f.Read(chunk)
		if n > 0 {
			r.buf = append(r.buf, chunk[:n]...)
			continue
		}
		if err != nil {
			return nil, err
		}
		return nil, io.EOF
	}
}
