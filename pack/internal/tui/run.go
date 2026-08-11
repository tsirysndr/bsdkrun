package tui

import (
	"strings"

	tea "github.com/charmbracelet/bubbletea"
	bkclient "github.com/moby/buildkit/client"

	"github.com/tsirysndr/bsdkrun/pack/internal/report"
)

// reporter implements report.Reporter by forwarding every event into the
// running bubbletea program — tea.Program.Send is safe to call from any
// goroutine, which is what lets the pipeline run on its own goroutine while
// this renders concurrently.
type reporter struct{ p *tea.Program }

func (r *reporter) PhaseStart(phase string)        { r.p.Send(phaseStartMsg{phase}) }
func (r *reporter) PhaseDone(phase, detail string) { r.p.Send(phaseDoneMsg{phase, detail}) }
func (r *reporter) PhaseError(phase string, err error) {
	r.p.Send(phaseErrorMsg{phase, err})
}
func (r *reporter) Log(phase, line string) { r.p.Send(logMsg{phase, line}) }
func (r *reporter) BuildKitStatus(phase string, s *bkclient.SolveStatus) {
	r.p.Send(buildkitMsg{phase, s})
	// Surface the build step's own output too, so a failing script says why
	// rather than just dying with an exit code.
	for _, l := range s.Logs {
		for _, line := range strings.Split(strings.TrimRight(string(l.Data), "\n"), "\n") {
			if line != "" {
				r.p.Send(logMsg{phase, line})
			}
		}
	}
}

// Pipeline is what Run drives: the actual pack pipeline, reporting its
// progress through the given Reporter and returning either a final message
// to display (e.g. the `bsdkrun unikraft` boot hint) or an error.
type Pipeline func(r report.Reporter) (final string, err error)

// Run starts the animated TUI and runs pipeline against it on a separate
// goroutine, returning once the pipeline finishes and the final frame has
// rendered.
func Run(pipeline Pipeline) error {
	p := tea.NewProgram(newModel())
	rep := &reporter{p: p}

	go func() {
		final, err := pipeline(rep)
		p.Send(doneMsg{err: err, final: final})
	}()

	finalModel, err := p.Run()
	if err != nil {
		return err
	}
	if m, ok := finalModel.(model); ok {
		return m.err
	}
	return nil
}
