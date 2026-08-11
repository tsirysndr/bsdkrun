package kraft

import (
	"bytes"
	"strings"
)

// phaseMarker prefixes a synthetic line kraftSteps emits at each sub-phase
// boundary (fetch vs. build). \x1e (ASCII Record Separator) is vanishingly
// unlikely to appear in real build output, which is what lets lineWriter
// tell "this is a phase transition" apart from "this is a log line" without
// having to pattern-match kraft's or apt's actual output.
const phaseMarker = "\x1epack-phase\x1e"

// lineWriter is an io.Writer that buffers to newlines and dispatches each
// complete line to onPhase (if it's a phaseMarker line) or onLine.
type lineWriter struct {
	onPhase func(phase string)
	onLine  func(line string)
	buf     []byte
}

func (w *lineWriter) Write(p []byte) (int, error) {
	w.buf = append(w.buf, p...)
	for {
		i := bytes.IndexByte(w.buf, '\n')
		if i < 0 {
			break
		}
		line := strings.TrimRight(string(w.buf[:i]), "\r")
		w.buf = w.buf[i+1:]
		if phase, ok := strings.CutPrefix(line, phaseMarker); ok {
			if w.onPhase != nil {
				w.onPhase(phase)
			}
			continue
		}
		if line != "" && w.onLine != nil {
			w.onLine(line)
		}
	}
	return len(p), nil
}
