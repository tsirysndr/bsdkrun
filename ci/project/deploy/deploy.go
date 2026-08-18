// Package deploy detects where a project ships from the *names* of its
// secrets — an operator who injects RAILWAY_TOKEN has named the deploy
// target as surely as a marker file names the language — and renders the
// deploy step a generated workflow gains. Only generated workflows: a
// committed CI config already says what it deploys, and appending steps to
// someone else's pipeline uninvited would be wrong.
//
// One target per file, mirroring the provider layout. All() lists them in
// priority order: when several tokens are present the first match wins and
// the announcement names the runners-up. Dry-run renders the step as an
// announcement of the exact command instead of running it — the right mode
// while wiring a new project, and the only mode this feature's own tests
// use (a real deploy is not something a test suite should trigger).
package deploy

import (
	"fmt"
	"sort"
	"strings"

	"github.com/tsirysndr/bsdkrun/ci/platforms"
)

// Target is one deploy destination.
type Target struct {
	// Platform names the target, e.g. "railway".
	Platform string
	// Secret is the name (never the value) that gives the target away.
	Secret string
	// Command is the real deploy command the step runs.
	Command string
}

// All returns every target in priority order.
func All() []Target {
	return []Target{
		Railway(),
		Fly(),
		Cloudflare(),
		Vercel(),
		Netlify(),
		DenoDeploy(),
		Koyeb(),
		Heroku(),
	}
}

// Detect picks the target the secret names imply, and lists any additional
// targets whose tokens are also present.
func Detect(secretNames []string) (target *Target, also []string) {
	names := map[string]bool{}
	for _, n := range secretNames {
		names[n] = true
	}
	for _, t := range All() {
		t := t
		if !names[t.Secret] {
			continue
		}
		if target == nil {
			target = &t
		} else {
			also = append(also, t.Platform)
		}
	}
	sort.Strings(also)
	return target, also
}

// Step renders the target as a workflow step.
func (t *Target) Step(dryRun bool) platforms.Step {
	name := fmt.Sprintf("deploy (%s)", t.Platform)
	if dryRun {
		return platforms.Step{
			Name: name + " [dry-run]",
			Command: fmt.Sprintf(
				`echo "[dry-run] would deploy to %s (%s detected): %s"`,
				t.Platform, t.Secret, strings.ReplaceAll(t.Command, "\"", "\\\"")),
		}
	}
	return platforms.Step{Name: name, Command: t.Command}
}
