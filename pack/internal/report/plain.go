package report

import (
	"fmt"

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
}

func NewPlain() *Plain { return &Plain{active: map[string]bool{}} }

func (p *Plain) PhaseStart(phase string) {
	fmt.Printf("  - %s\n", phase)
}

func (p *Plain) PhaseDone(phase, detail string) {
	if detail != "" {
		fmt.Printf("      %s\n", detail)
	}
}

func (p *Plain) PhaseError(phase string, err error) {
	fmt.Printf("      failed: %v\n", err)
}

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
