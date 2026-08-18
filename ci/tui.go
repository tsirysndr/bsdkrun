package main

// The interactive renderer: a Bubble Tea program — every step a live row
// with a realtime duration (seconds with milliseconds),
// the running step showing a dimmed tail of its output, failures keeping
// their last lines on screen so the answer is in front of you when it exits.
//
// It is deliberately NOT a second output path through the runner. The runner
// speaks exactly one structured format — spindle's LogLine records — and this
// TUI is just another consumer of that stream, the same way the desktop and
// web CI screens are. The runner runs in JSON mode into a parsing sink; what
// the TUI knows is what any spindle log consumer would know.

import (
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
)

// tailLines is how much of the running (and failed) step's output stays
// visible. Enough to read a failure; small enough to keep the frame calm.
const tailLines = 8

var (
	styleWorkflow = lipgloss.NewStyle().Bold(true)
	styleImage    = lipgloss.NewStyle().Faint(true)
	styleOK       = lipgloss.NewStyle().Foreground(lipgloss.Color("42"))  // green
	styleFail     = lipgloss.NewStyle().Foreground(lipgloss.Color("204")) // pink/red
	styleRun      = lipgloss.NewStyle().Foreground(lipgloss.Color("51"))  // cyan
	stylePending  = lipgloss.NewStyle().Faint(true)
	styleDur      = lipgloss.NewStyle().Faint(true)
	styleTail     = lipgloss.NewStyle().Faint(true)
	styleSystem   = lipgloss.NewStyle().Faint(true)
)

var spinnerFrames = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}

// --- messages ---------------------------------------------------------------

// tuiWorkflowStart / tuiWorkflowDone bracket one workflow; the LogLine stream
// itself has no workflow boundary, so the orchestrator sends these directly.
type tuiWorkflowStart struct {
	Name     string
	Image    string
	Platform string
}

type tuiWorkflowDone struct {
	Name string
	Err  error
}

// tuiLog is one parsed LogLine record.
type tuiLog struct {
	Kind       string `json:"kind"`
	Content    string `json:"content"`
	StepID     int    `json:"step_id"`
	StepStatus string `json:"step_status"`
	StepKind   int    `json:"step_kind"`
}

type tuiAllDone struct{ failed, total int }

type tuiTick time.Time

// --- model ------------------------------------------------------------------

type tuiStep struct {
	name    string
	system  bool
	status  string // running | ok | failed
	started time.Time
	dur     time.Duration
	tail    []string
}

type tuiWorkflow struct {
	name     string
	image    string
	platform string
	status   string // running | ok | failed
	started  time.Time
	dur      time.Duration
	steps    []*tuiStep
	err      error
}

type tuiModel struct {
	workflows []*tuiWorkflow
	frame     int
	done      bool
	failed    int
	total     int
	quitting  bool
}

func (m *tuiModel) current() *tuiWorkflow {
	if len(m.workflows) == 0 {
		return nil
	}
	return m.workflows[len(m.workflows)-1]
}

func (m *tuiModel) Init() tea.Cmd { return tick() }

func tick() tea.Cmd {
	// ~15 fps: smooth enough for a millisecond readout, cheap enough to leave
	// running for a full compile.
	return tea.Tick(66*time.Millisecond, func(t time.Time) tea.Msg { return tuiTick(t) })
}

func (m *tuiModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		if msg.String() == "ctrl+c" {
			// Quit the frame; the orchestrator turns this into exit 130. The
			// in-flight guest command is not signalled — same trade-off every
			// other cancel path here makes.
			m.quitting = true
			return m, tea.Quit
		}
	case tuiTick:
		m.frame++
		if m.done {
			return m, nil
		}
		return m, tick()
	case tuiWorkflowStart:
		m.workflows = append(m.workflows, &tuiWorkflow{
			name:     msg.Name,
			image:    msg.Image,
			platform: msg.Platform,
			status:   "running",
			started:  time.Now(),
		})
	case tuiWorkflowDone:
		if w := m.current(); w != nil {
			w.dur = time.Since(w.started)
			w.err = msg.Err
			if msg.Err != nil {
				w.status = "failed"
			} else {
				w.status = "ok"
			}
			// A workflow can fail between steps (boot error path): close any
			// step left open so the frame never shows a spinner on a corpse.
			for _, s := range w.steps {
				if s.status == "running" {
					s.dur = time.Since(s.started)
					if msg.Err != nil {
						s.status = "failed"
					} else {
						s.status = "ok"
					}
				}
			}
		}
	case tuiLog:
		w := m.current()
		if w == nil {
			break
		}
		switch msg.Kind {
		case "control":
			switch msg.StepStatus {
			case "start":
				w.steps = append(w.steps, &tuiStep{
					name:    msg.Content,
					system:  msg.StepKind == 0,
					status:  "running",
					started: time.Now(),
				})
			default: // end
				if n := len(w.steps); n > 0 && w.steps[n-1].status == "running" {
					s := w.steps[n-1]
					s.dur = time.Since(s.started)
					s.status = "ok" // a failed end is corrected by tuiWorkflowDone
				}
			}
		case "data":
			if n := len(w.steps); n > 0 {
				s := w.steps[n-1]
				s.tail = append(s.tail, msg.Content)
				if len(s.tail) > tailLines {
					s.tail = s.tail[len(s.tail)-tailLines:]
				}
			}
		}
	case tuiAllDone:
		m.done = true
		m.failed = msg.failed
		m.total = msg.total
		return m, tea.Quit
	}
	return m, nil
}

// fmtDur renders a duration as seconds with milliseconds — `12.345s` — the
// number a build log wants; minutes appear once seconds stop being readable.
func fmtDur(d time.Duration) string {
	if d < 0 {
		d = 0
	}
	if d >= 5*time.Minute {
		m := int(d.Minutes())
		return fmt.Sprintf("%dm%06.3fs", m, d.Seconds()-float64(m)*60)
	}
	return fmt.Sprintf("%.3fs", d.Seconds())
}

func (m *tuiModel) View() string {
	var b strings.Builder
	for _, w := range m.workflows {
		glyph, style := m.glyph(w.status)
		dur := w.dur
		if w.status == "running" {
			dur = time.Since(w.started)
		}
		label := w.name
		if w.platform != "" && w.platform != "tangled" {
			label += " " + styleImage.Render("["+w.platform+"]")
		}
		b.WriteString(fmt.Sprintf("%s %s %s %s\n",
			style.Render(glyph),
			styleWorkflow.Render(label),
			styleImage.Render(w.image),
			styleDur.Render(fmtDur(dur)),
		))
		for _, s := range w.steps {
			g, st := m.glyph(s.status)
			d := s.dur
			if s.status == "running" {
				d = time.Since(s.started)
			}
			name := s.name
			if s.system {
				name = styleSystem.Render(name)
			}
			b.WriteString(fmt.Sprintf("  %s %-38s %s\n",
				st.Render(g), name, styleDur.Render(fmtDur(d))))
			// The running step's tail keeps you company; a failed step's tail
			// is the answer. Everything else collapses.
			if s.status == "running" || s.status == "failed" {
				for _, line := range s.tail {
					b.WriteString(styleTail.Render("      │ "+truncTo(line, 100)) + "\n")
				}
			}
		}
		if w.err != nil {
			b.WriteString(styleFail.Render("  ✗ "+w.err.Error()) + "\n")
		}
	}
	if m.done {
		if m.failed > 0 {
			b.WriteString(styleFail.Render(
				fmt.Sprintf("%d of %d workflow(s) failed", m.failed, m.total)) + "\n")
		} else {
			b.WriteString(styleOK.Render(
				fmt.Sprintf("✓ %d workflow(s) passed", m.total)) + "\n")
		}
	}
	if m.quitting && !m.done {
		b.WriteString(styleFail.Render("interrupted") + "\n")
	}
	return b.String()
}

func (m *tuiModel) glyph(status string) (string, lipgloss.Style) {
	switch status {
	case "ok":
		return "✔", styleOK
	case "failed":
		return "✘", styleFail
	case "running":
		return spinnerFrames[m.frame%len(spinnerFrames)], styleRun
	default:
		return "·", stylePending
	}
}

func truncTo(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

// --- the sink ---------------------------------------------------------------

// tuiSink is the runner's opts.Out in TUI mode: it receives the JSON LogLine
// stream, parses each line, and forwards it to the program. Concurrent-safe —
// stdout and stderr emitters write from separate goroutines.
type tuiSink struct {
	p  *tea.Program
	mu sync.Mutex
	// partial buffers an incomplete trailing line between writes.
	partial string
}

func (t *tuiSink) Write(b []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.partial += string(b)
	for {
		i := strings.IndexByte(t.partial, '\n')
		if i < 0 {
			break
		}
		line := strings.TrimSpace(t.partial[:i])
		t.partial = t.partial[i+1:]
		if line == "" {
			continue
		}
		var rec tuiLog
		if err := json.Unmarshal([]byte(line), &rec); err == nil && rec.Kind != "" {
			t.p.Send(rec)
		} else {
			// Runner chatter that is not a LogLine still belongs on screen,
			// attached to whatever step is open — same tolerance every other
			// consumer of this stream applies.
			t.p.Send(tuiLog{Kind: "data", Content: line})
		}
	}
	return len(b), nil
}

// --- orchestration ----------------------------------------------------------

// runPlansTUI executes the plans sequentially under the TUI. Returns the
// number that failed (mirroring the plain path's accounting) and any terminal
// error from the program itself.
func runPlansTUI(plans []*Plan, opts runOpts) (int, error) {
	model := &tuiModel{}
	p := tea.NewProgram(model)

	failed := 0
	go func() {
		for _, plan := range plans {
			p.Send(tuiWorkflowStart{Name: plan.Name, Image: plan.Image, Platform: plan.Platform})
			runOpts := opts
			runOpts.JSON = true
			runOpts.Out = &tuiSink{p: p}
			_, err := runPlan(plan, runOpts)
			if err != nil {
				failed++
			}
			p.Send(tuiWorkflowDone{Name: plan.Name, Err: err})
		}
		p.Send(tuiAllDone{failed: failed, total: len(plans)})
	}()

	final, err := p.Run()
	if err != nil {
		return failed, err
	}
	if m, ok := final.(*tuiModel); ok && m.quitting && !m.done {
		return failed, fmt.Errorf("interrupted")
	}
	if failed > 0 {
		return failed, fmt.Errorf("%d of %d workflow(s) failed", failed, len(plans))
	}
	return 0, nil
}
