//// Gleam SDK for [**bsdkrun**](https://github.com/tsirysndr/bsdkrun) — a
//// Firecracker-style microVM launcher for **BSD, Linux, and unikernel** guests on macOS
//// and Linux, built on [libkrun](https://github.com/containers/libkrun).
////
//// The SDK is a thin, stateless wrapper around the `bsdkrun` binary: it builds
//// argv, shells out through an Erlang port, and decodes the JSON output.
////
//// ```gleam
//// import bsdkrun
//// import bsdkrun/args
//// import bsdkrun/types
////
//// pub fn main() {
////   let assert Ok(sbx) = bsdkrun.create(args.linux("alpine"))
////   let assert Ok(res) = bsdkrun.exec(sbx, ["uname", "-a"])
////   echo types.text(res)
////   let assert Ok(sbx) = bsdkrun.stop(sbx)
//// }
//// ```
////
//// This module holds the shortest path to a running machine. Everything else
//// lives in the submodules:
////
//// - `bsdkrun/sandbox` — create, inspect, and drive microVMs.
//// - `bsdkrun/args` — the create options and their builders.
//// - `bsdkrun/images` — list downloaded images.
//// - `bsdkrun/volumes` — list and remove persistent volumes.
//// - `bsdkrun/networks` — global networks; reach machines by name.
//// - `bsdkrun/system` — probe, fetch BSD images, list versions, grow disks.
//// - `bsdkrun/types` — the decoded records and `CommandResult` helpers.
//// - `bsdkrun/binary` — override or inspect binary discovery.
//// - `bsdkrun/error` — the single `Error` type and its renderer.
////
//// The binary is resolved from `bsdkrun/binary.set_binary_path`,
//// `$BSDKRUN_BIN`, `bsdkrun` on `$PATH`, or an in-repo dev build — in that
//// order.

import bsdkrun/args.{type CreateOptions}
import bsdkrun/error.{type Error}
import bsdkrun/sandbox.{type Sandbox}
import bsdkrun/types.{type CommandResult, type SandboxInfo}
import gleam/option.{type Option}

/// Boot a new microVM (detached) and return a handle to it. See
/// `bsdkrun/args` for the option builders.
pub fn create(opts: CreateOptions) -> Result(Sandbox, Error) {
  sandbox.create(opts)
}

/// Reconnect to an existing machine by id — a unique prefix is enough.
pub fn get(id: String) -> Result(Sandbox, Error) {
  sandbox.get(id)
}

/// List running machines.
pub fn list() -> Result(List(SandboxInfo), Error) {
  sandbox.list()
}

/// List machines, including exited ones when `all` is `True`.
pub fn list_all(all: Bool) -> Result(List(SandboxInfo), Error) {
  sandbox.list_all(all)
}

/// Run a command in the guest with the default options. Use
/// `bsdkrun/sandbox.exec` when you need env vars, a TTY, stdin, or a cwd.
pub fn exec(
  sbx: Sandbox,
  command: List(String),
) -> Result(CommandResult, Error) {
  sandbox.exec(sbx, command, sandbox.exec_options())
}

/// Read the machine's console log.
pub fn logs(sbx: Sandbox) -> Result(String, Error) {
  sandbox.logs(sbx)
}

/// Stop the machine. Returns `sbx` back (not `Nil`) — see `bsdkrun/sandbox`.
pub fn stop(sbx: Sandbox) -> Result(Sandbox, Error) {
  sandbox.stop(sbx)
}

/// Restart a stopped machine in place. Returns `sbx` back (not `Nil`) — see
/// `bsdkrun/sandbox`.
pub fn start(sbx: Sandbox) -> Result(Sandbox, Error) {
  sandbox.start(sbx)
}

/// Remove the machine and its state. `force` stops it first if running.
/// Returns `sbx` back (not `Nil`) — see `bsdkrun/sandbox`.
pub fn remove(sbx: Sandbox, force: Bool) -> Result(Sandbox, Error) {
  sandbox.remove(sbx, force)
}

/// This machine's current status row, or `None` if it is gone.
pub fn status(sbx: Sandbox) -> Result(Option(SandboxInfo), Error) {
  sandbox.status(sbx)
}

/// Whether the machine is currently running.
pub fn is_running(sbx: Sandbox) -> Bool {
  sandbox.is_running(sbx)
}
