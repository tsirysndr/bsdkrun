package report

import (
	"fmt"
	"time"

	bkclient "github.com/moby/buildkit/client"
)

// Plain prints each event as a line — pack's original, non-interactive
// output. Used whenever stdout isn't a terminal (piped, redirected, CI),
// where an animated TUI can't render anyway.
type Plain struct {
	// active tracks which BuildKit vertices we've already announced, so a
	// vertex's repeated status updates (bytes transferred, etc.) don't spam
	// a new line each time — only the first "started" per vertex prints.
	active map[string]bool

	// There's no cursor to go back to here, so a step's duration can only be
	// printed once it's over: phaseStart is when the current one began, start
	// when the whole run did.
	phaseStart time.Time
	start      time.Time
}

func NewPlain() *Plain {
	now := time.Now()
	return &Plain{active: map[string]bool{}, phaseStart: now, start: now}
}

func (p *Plain) PhaseStart(phase string) {
	p.phaseStart = time.Now()
	fmt.Printf("  - %s\n", phase)
}

func (p *Plain) PhaseDone(phase, detail string) {
	if detail != "" {
		fmt.Printf("      %s\n", detail)
	}
	fmt.Printf("      done in %s\n", FormatDuration(time.Since(p.phaseStart)))
}

func (p *Plain) PhaseError(phase string, err error) {
	fmt.Printf("      failed after %s: %v\n", FormatDuration(time.Since(p.phaseStart)), err)
}

// Elapsed is how long the whole run has taken, for the total main prints once
// the pipeline returns (the TUI renders its own).
func (p *Plain) Elapsed() time.Duration { return time.Since(p.start) }

func (p *Plain) Log(phase, line string) {
	fmt.Println("      " + line)
}

func (p *Plain) BuildKitStatus(phase string, s *bkclient.SolveStatus) {
	for _, v := range s.Vertexes {
		if v.Started != nil && !p.active[string(v.Digest)] {
			p.active[string(v.Digest)] = true
			fmt.Printf("      [buildkit] %s\n", v.Name)
		}
	}
}
