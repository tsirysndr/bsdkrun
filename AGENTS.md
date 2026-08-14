# Repository guidance

## Scope and architecture

- `src/` is the CLI entry point; most runtime behavior lives in `core/`.
- `agent/` is the in-guest exec agent. Its framed protocol keeps stdin,
  stdout, stderr, terminal resize, and exit status as distinct channels.
- `daemon/` exposes remote operations over GraphQL/gRPC; `supervisor/` owns
  long-running VM processes.
- `sdk/` contains the Clojure, Elixir, Gleam, Go, Python, Ruby, Rust, and
  TypeScript SDKs. Keep equivalent public behavior aligned across all eight.
- `web/` and `desktop/` are user interfaces. `examples/` contains runnable
  guest and unikernel examples.

## Development rules

- Preserve buffered command results when adding streaming behavior. Streaming
  must be opt-in and must not make callers choose between live output and the
  final stdout/stderr values.
- Do not use TTY allocation as a generic streaming switch. A PTY changes
  process semantics and commonly merges stderr into stdout.
- Drain stdout and stderr concurrently whenever both are piped; sequential
  reads can deadlock when either pipe fills.
- Keep argv as structured arguments. Do not introduce shell interpolation for
  user-provided commands, environment values, paths, or input.
- Maintain backward compatibility unless a breaking change is explicitly
  requested. Non-zero guest exit codes are normally returned as result data;
  each SDK's existing opt-in throwing/checking behavior must remain intact.
- Preserve unrelated work in a dirty worktree and use `rg` for repository
  searches.

## Formatting and verification

- Rust/core/CLI: `cargo fmt --all`, then targeted `cargo test` commands.
- Go SDK: `gofmt` changed files and run `go test ./...` from `sdk/go`.
- TypeScript SDK: run `npm run build --prefix sdk/typescript` and its Bun tests
  when local socket creation is available.
- Python SDK: run `python -m unittest discover -s tests -v` from `sdk/python`.
- Ruby SDK: run its tests through `rake test` from `sdk/ruby`.
- Elixir SDK: format only changed `.ex`/`.exs` files with explicit paths, then
  run `mix test` from `sdk/elixir` (there is no repository formatter config).
- Gleam SDK: run `gleam format src test` and `gleam check` from `sdk/gleam`.
- Clojure SDK: run its `:test` alias from `sdk/clojure`; use a workspace-local
  cache if the environment cannot write the default Clojure cache.
- Network-backed SDK tests bind loopback sockets and may require a less
  restrictive sandbox. Report that limitation separately from code failures.

## SDK releases

- Make SDK-facing changes and README examples consistently across all eight
  SDK directories.
- Bump patch versions for backward-compatible features in each package's
  native manifest/version source and update generated lockfile package entries.
- Go module versions are ultimately assigned by Git tags; keep `Version` in
  `sdk/go/version.go` synchronized with the intended release tag.
- Do not create tags or publish packages unless explicitly requested.
