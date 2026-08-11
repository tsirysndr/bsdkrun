// Package report decouples the pack pipeline (internal/buildkit,
// internal/kraft, and main's own orchestration) from how its progress is
// displayed, so the exact same pipeline drives either the plain sequential
// printer (main.go, used whenever stdout isn't a terminal) or the animated
// bubbletea TUI (internal/tui).
package report

import (
	"fmt"
	"time"

	bkclient "github.com/moby/buildkit/client"
)

// Phase names, shared by both renderers and by main.go's pipeline — the
// single source of truth for what "the 6 steps" are.
const (
	PhaseDetect     = "detect"
	PhasePlan       = "plan"
	PhaseRootfs     = "build rootfs"
	PhaseKraftfile  = "generate Kraftfile"
	PhaseFetch      = "fetch + patch Unikraft"
	PhaseKraftBuild = "kraft build"
	PhasePush       = "push"
)

// FormatDuration renders an elapsed time the way both renderers show it at
// the right of a step: "4.5s", and "2m04.5s" once a step runs past a minute
// (kraft build routinely does).
func FormatDuration(d time.Duration) string {
	if d < 0 {
		d = 0
	}
	secs := d.Seconds()
	if secs < 60 {
		return fmt.Sprintf("%.1fs", secs)
	}
	m := int(secs) / 60
	return fmt.Sprintf("%dm%04.1fs", m, secs-float64(m*60))
}

// Reporter receives progress events as the pipeline runs.
type Reporter interface {
	PhaseStart(phase string)
	PhaseDone(phase, detail string)
	PhaseError(phase string, err error)
	Log(phase, line string)
	BuildKitStatus(phase string, s *bkclient.SolveStatus)
}
