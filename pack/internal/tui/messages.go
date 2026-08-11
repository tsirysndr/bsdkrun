package tui

import bkclient "github.com/moby/buildkit/client"

type phaseStartMsg struct{ phase string }

type phaseDoneMsg struct{ phase, detail string }

type phaseErrorMsg struct {
	phase string
	err   error
}

type logMsg struct{ phase, line string }

type buildkitMsg struct {
	phase  string
	status *bkclient.SolveStatus
}

type doneMsg struct {
	err   error
	final string
}
