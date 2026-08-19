// A tiny dagger module, so `bsdkrun ci` has something real to call.
//
// The point of the example is the plumbing around it: bsdkrun detects the
// module, boots a microVM with a Docker daemon, installs the dagger CLI,
// pulls the engine and calls a function — none of which the module knows.
package main

import (
	"context"
)

type Demo struct{}

// Ci is what `bsdkrun ci run` calls when no function is named: it runs a
// container through the dagger engine and returns what it printed.
func (m *Demo) Ci(ctx context.Context) (string, error) {
	return dag.Container().
		From("alpine:3.20").
		WithExec([]string{"echo", "dagger-example-ok"}).
		Stdout(ctx)
}
