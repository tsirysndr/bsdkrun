// Package report decouples the pack pipeline (internal/buildkit,
// internal/kraft, and main's own orchestration) from how its progress is
// displayed, so the exact same pipeline drives either the plain sequential
// printer (main.go, used whenever stdout isn't a terminal) or the animated
// bubbletea TUI (internal/tui).
package report

import bkclient "github.com/moby/buildkit/client"

// Phase names, shared by both renderers and by main.go's pipeline — the
// single source of truth for what "the 6 steps" are.
const (
	PhaseDetect     = "detect"
	PhasePlan       = "plan"
	PhaseRootfs     = "build rootfs"
	PhaseKraftfile  = "generate Kraftfile"
	PhaseFetch      = "fetch + patch Unikraft"
	PhaseKraftBuild = "kraft build"
)

// Reporter receives progress events as the pipeline runs.
type Reporter interface {
	PhaseStart(phase string)
	PhaseDone(phase, detail string)
	PhaseError(phase string, err error)
	Log(phase, line string)
	BuildKitStatus(phase string, s *bkclient.SolveStatus)
}
