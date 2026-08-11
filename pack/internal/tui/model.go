// Package tui is the animated bubbletea front end for `bsdkrun pack`. It
// implements report.Reporter (see internal/report) — the pipeline in
// main.go doesn't know or care whether it's driving this or the plain
// printer, it just calls the same Reporter methods either way.
package tui

import (
	"fmt"
	"strings"

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

// steps is the fixed pipeline order — same six names report.Phase* define.
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
	// current is which step is running now (or most recently was), so
	// incoming logMsg/buildkitMsg — which don't carry the six-step name
	// themselves — know which step's nested detail they belong to.
	current string

	vertexOrder []string
	vertices    map[string]*vertex

	logTail []string

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
		state:    state,
		vertices: map[string]*vertex{},
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

	case spinner.TickMsg:
		var cmd tea.Cmd
		m.spinner, cmd = m.spinner.Update(msg)
		return m, cmd

	case phaseStartMsg:
		m.state[msg.phase].status = running
		m.current = msg.phase
		m.logTail = nil
		m.vertexOrder = nil
		m.vertices = map[string]*vertex{}
		return m, nil

	case phaseDoneMsg:
		s := m.state[msg.phase]
		s.status = done
		s.detail = msg.detail
		return m, nil

	case phaseErrorMsg:
		s := m.state[msg.phase]
		s.status = failed
		s.err = msg.err
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
		m.quitting = true
		return m, tea.Quit
	}
	return m, nil
}

func (m model) View() string {
	var b strings.Builder
	for _, name := range steps {
		s := m.state[name]
		switch s.status {
		case done:
			b.WriteString(styleDone.Render("✓ " + name))
			if s.detail != "" {
				b.WriteString(styleDetail.Render("  " + s.detail))
			}
			b.WriteByte('\n')
		case running:
			b.WriteString(m.spinner.View())
			b.WriteString(styleRunning.Render(" " + name))
			b.WriteByte('\n')
			b.WriteString(m.renderNested())
		case failed:
			b.WriteString(styleError.Render("✗ " + name))
			if s.err != nil {
				b.WriteString(styleDetail.Render("  " + s.err.Error()))
			}
			b.WriteByte('\n')
		default:
			b.WriteString(stylePending.Render("· " + name))
			b.WriteByte('\n')
		}
	}

	if m.quitting {
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
