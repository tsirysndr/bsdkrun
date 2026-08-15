// Package bsdkrun is a Go SDK for bsdkrun
// (https://github.com/tsirysndr/bsdkrun), a Firecracker-style microVM
// launcher for BSD, Linux, and unikernel guests.
//
// It is a thin, dependency-free wrapper around the bsdkrun CLI: it builds
// argv, shells out, and parses the JSON output. The API is fluent — builders
// chain and end in a terminal call returning (T, error):
//
//	sbx, err := bsdkrun.Linux("alpine").
//		Cpus(2).Mem(1024).
//		Port("8080:80").
//		Command("sleep", "300").
//		Create()
//
//	res, err := sbx.Command("uname").Args("-a").Run()
//	fmt.Println(res.Text())
//	sbx.Stop()
//
// Host-level operations live on the Images, Volumes, Networks, and System
// namespaces. Client is the network sibling: it drives the same operations
// against a remote bsdkrund daemon over its GraphQL API (HTTP for queries
// and mutations, a hand-rolled graphql-transport-ws WebSocket for
// subscriptions), with — like the rest of the SDK — only the standard
// library underneath.
package bsdkrun
