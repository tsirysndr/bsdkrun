// Package procfile reads a Heroku-style Procfile, which is how a project
// states what to actually run — the one thing a provider cannot reliably
// infer from the source tree.
//
// A unikernel runs exactly one program, so only one process type can be
// honoured: `web` if present, otherwise the first declared. The rest are
// reported so a caller can say plainly that they were ignored, rather than
// silently dropping half the file.
package procfile

import (
	"os"
	"path/filepath"
	"strings"
)

// Procfile is the parsed file: process type -> command, plus the order they
// were declared in (a map alone would make "the first one" meaningless).
type Procfile struct {
	Commands map[string]string
	Order    []string
}

// Web returns the command a unikernel should run: `web`, else `worker`,
// else the first declared. Reports false when there is nothing to run.
//
// The worker step matters for a Procfile that declares only background
// processes — falling straight to "first declared" would pick whichever
// happened to be written first, which for `release: migrate` and
// `worker: consume` is a migration that exits immediately.
func (p *Procfile) Web() (string, bool) {
	if p == nil || len(p.Order) == 0 {
		return "", false
	}
	if cmd := p.Commands[chosen(p)]; cmd != "" {
		return cmd, true
	}
	return "", false
}

// chosen is the process type that will run.
func chosen(p *Procfile) string {
	for _, preferred := range []string{"web", "worker"} {
		if cmd, ok := p.Commands[preferred]; ok && cmd != "" {
			return preferred
		}
	}
	return p.Order[0]
}

// Ignored lists the process types that will not run, since only one can.
func (p *Procfile) Ignored() []string {
	if p == nil {
		return nil
	}
	if len(p.Order) == 0 {
		return nil
	}
	running := chosen(p)
	var rest []string
	for _, name := range p.Order {
		if name != running {
			rest = append(rest, name)
		}
	}
	return rest
}

// Read parses dir/Procfile. Returns nil (no error) when there is none —
// most projects have no Procfile, which is not a problem.
func Read(dir string) *Procfile {
	data, err := os.ReadFile(filepath.Join(dir, "Procfile"))
	if err != nil {
		return nil
	}
	p := &Procfile{Commands: map[string]string{}}
	for _, line := range strings.Split(string(data), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		name, cmd, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		name = strings.TrimSpace(name)
		cmd = strings.TrimSpace(cmd)
		if name == "" || cmd == "" {
			continue
		}
		if _, seen := p.Commands[name]; !seen {
			p.Order = append(p.Order, name)
		}
		p.Commands[name] = cmd
	}
	if len(p.Order) == 0 {
		return nil
	}
	return p
}
