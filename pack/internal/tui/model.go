// Package tui is the animated bubbletea front end for `bsdkrun pack`. It
// implements report.Reporter (see internal/report) — the pipeline in
// main.go doesn't know or care whether it's driving this or the plain
// printer, it just calls the same Reporter methods either way.
package tui

import (
	"fmt"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/spinner"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"github.com/tsirysndr/bsdkrun/pack/internal/report"
)

// Same accent palette as the Rust CLI's --help styling (core/src/cli.rs) —
// one product, one palette, even though this half is a different language.
var (
	teal   = lipgloss.Color("#00E8C6")
	violet = lipgloss.Color("#8264FF")
	muted  = lipgloss.Color("#C8D2DC")
	red    = lipgloss.Color("#FF6464")

	styleDone    = lipgloss.NewStyle().Foreground(teal)
	styleRunning = lipgloss.NewStyle().Foreground(violet).Bold(true)
	stylePending = lipgloss.NewStyle().Foreground(muted)
	styleError   = lipgloss.NewStyle().Foreground(red).Bold(true)
	styleDetail  = lipgloss.NewStyle().Foreground(muted)
	styleFinal   = lipgloss.NewStyle().Foreground(teal).Bold(true)
)

type status int

const (
	pending status = iota
	running
	done
	failed
)

// steps is the pipeline order every run goes through. Optional phases —
// push, for one — are not here: they appear only if they actually run, and
// showing "push" greyed out on every build would advertise a step most
// builds never take.
var steps = []string{
	report.PhaseDetect,
	report.PhasePlan,
	report.PhaseRootfs,
	report.PhaseKraftfile,
	report.PhaseFetch,
	report.PhaseKraftBuild,
}

type stepState struct {
	status status
	detail string
	err    error

	// started is when the step entered `running`; elapsed is frozen from it
	// the moment the step finishes. While running, the duration shown is
	// recomputed from started on every frame, so it ticks live.
	started time.Time
	elapsed time.Duration
}

// vertex is one BuildKit LLB op, tracked only while report.PhaseRootfs is
// running — reset when it starts, discarded once it's done.
type vertex struct {
	name string
	done bool
}

const maxLogLines = 8

type model struct {
	state map[string]*stepState
	// order is the display order, extended when an optional phase appears.
	order []string
	// current is which step is running now (or most recently was), so
	// incoming logMsg/buildkitMsg — which don't carry the six-step name
	// themselves — know which step's nested detail they belong to.
	current string

	vertexOrder []string
	vertices    map[string]*vertex

	logTail []string

	// width is the terminal width the step durations are right-aligned
	// against; 0 until the first tea.WindowSizeMsg arrives.
	width int

	// started/elapsed time the run as a whole, the same way stepState does
	// one step: elapsed is frozen when doneMsg arrives.
	started time.Time
	elapsed time.Duration

	spinner  spinner.Model
	quitting bool
	err      error
	final    string
}

func newModel() model {
	s := spinner.New()
	s.Spinner = spinner.Dot
	s.Style = styleRunning

	state := make(map[string]*stepState, len(steps))
	for _, s := range steps {
		state[s] = &stepState{status: pending}
	}

	return model{
		order:    append([]string(nil), steps...),
		state:    state,
		vertices: map[string]*vertex{},
		started:  time.Now(),
		spinner:  s,
	}
}

func (m model) Init() tea.Cmd {
	return m.spinner.Tick
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyMsg:
		switch msg.String() {
		case "ctrl+c", "q":
			m.quitting = true
			return m, tea.Quit
		}

	case tea.WindowSizeMsg:
		m.width = msg.Width
		return m, nil

	case spinner.TickMsg:
		var cmd tea.Cmd
		m.spinner, cmd = m.spinner.Update(msg)
		return m, cmd

	case phaseStartMsg:
		s := m.step(msg.phase)
		s.status = running
		s.started = time.Now()
		m.current = msg.phase
		m.logTail = nil
		m.vertexOrder = nil
		m.vertices = map[string]*vertex{}
		return m, nil

	case phaseDoneMsg:
		s := m.step(msg.phase)
		s.status = done
		s.detail = msg.detail
		s.elapsed = time.Since(s.started)
		return m, nil

	case phaseErrorMsg:
		s := m.step(msg.phase)
		s.status = failed
		s.err = msg.err
		s.elapsed = time.Since(s.started)
		return m, nil

	case logMsg:
		m.logTail = append(m.logTail, msg.line)
		if len(m.logTail) > maxLogLines {
			m.logTail = m.logTail[len(m.logTail)-maxLogLines:]
		}
		return m, nil

	case buildkitMsg:
		for _, v := range msg.status.Vertexes {
			key := string(v.Digest)
			vs, ok := m.vertices[key]
			if !ok {
				vs = &vertex{name: v.Name}
				m.vertices[key] = vs
				m.vertexOrder = append(m.vertexOrder, key)
			}
			vs.done = v.Completed != nil
		}
		return m, nil

	case doneMsg:
		m.err = msg.err
		m.final = msg.final
		m.elapsed = time.Since(m.started)
		m.quitting = true
		return m, tea.Quit
	}
	return m, nil
}

// step returns the state for a phase, creating it if this is a phase the
// fixed pipeline does not list. A missing entry used to be a nil map read
// and a panic one frame later, which is a poor way for an optional step to
// announce itself.
func (m *model) step(name string) *stepState {
	if s, ok := m.state[name]; ok {
		return s
	}
	s := &stepState{status: pending}
	m.state[name] = s
	m.order = append(m.order, name)
	return s
}

func (m model) View() string {
	var b strings.Builder
	for _, name := range m.order {
		s := m.state[name]
		switch s.status {
		case done:
			left := styleDone.Render("✓ " + name)
			if s.detail != "" {
				left += styleDetail.Render("  " + s.detail)
			}
			b.WriteString(m.rightAlign(left, report.FormatDuration(s.elapsed), styleDetail))
		case running:
			left := m.spinner.View() + styleRunning.Render(" "+name)
			// Recomputed every frame rather than read from s.elapsed, which
			// is what makes a running step's duration count up live.
			b.WriteString(m.rightAlign(left, report.FormatDuration(time.Since(s.started)), styleRunning))
			b.WriteString(m.renderNested())
		case failed:
			left := styleError.Render("✗ " + name)
			if s.err != nil {
				left += styleDetail.Render("  " + s.err.Error())
			}
			b.WriteString(m.rightAlign(left, report.FormatDuration(s.elapsed), styleDetail))
		default:
			b.WriteString(stylePending.Render("· " + name))
			b.WriteByte('\n')
		}
	}

	if m.quitting {
		b.WriteByte('\n')
		b.WriteString(m.rightAlign(styleDetail.Render("total"), report.FormatDuration(m.elapsed), styleFinal))
		if m.err != nil {
			b.WriteByte('\n')
			b.WriteString(styleError.Render(fmt.Sprintf("error: %v", m.err)))
			b.WriteByte('\n')
		} else if m.final != "" {
			b.WriteByte('\n')
			b.WriteString(styleFinal.Render(m.final))
			b.WriteByte('\n')
		}
	}
	return b.String()
}

// rightAlign renders one step line: left as given, then dur pushed against
// the right edge of the terminal. Widths are measured with lipgloss.Width so
// the ANSI colour escapes in both halves don't count toward the padding.
func (m model) rightAlign(left, dur string, durStyle lipgloss.Style) string {
	width := m.width
	if width <= 0 {
		// Before the first tea.WindowSizeMsg (and in any terminal that never
		// sends one) fall back to the conventional 80 columns.
		width = 80
	}
	pad := width - lipgloss.Width(left) - lipgloss.Width(dur)
	if pad < 1 {
		pad = 1
	}
	return left + strings.Repeat(" ", pad) + durStyle.Render(dur) + "\n"
}

// renderNested draws whichever live detail the currently-running step has:
// BuildKit's vertex tree during report.PhaseRootfs, or a scrolling log tail
// during report.PhaseFetch / report.PhaseKraftBuild (kraft build's own
// stdout/stderr).
func (m model) renderNested() string {
	var b strings.Builder
	if m.current == report.PhaseRootfs {
		for _, key := range m.vertexOrder {
			v := m.vertices[key]
			mark := "⠋"
			style := styleRunning
			if v.done {
				mark = "✓"
				style = styleDone
			}
			b.WriteString(styleDetail.Render("    " + mark + " "))
			b.WriteString(style.Render(v.name))
			b.WriteByte('\n')
		}
		return b.String()
	}
	for _, line := range m.logTail {
		b.WriteString(styleDetail.Render("    " + line))
		b.WriteByte('\n')
	}
	return b.String()
}
