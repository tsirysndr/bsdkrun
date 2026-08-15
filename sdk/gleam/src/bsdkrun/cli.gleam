//// Shelling out to the `bsdkrun` binary.
////
//// Every invocation is prefixed with the global `--log-level` flag (default
//// `0`) so the SDK's captured output stays clean. stdout and stderr are
//// buffered separately.

import bsdkrun/binary
import bsdkrun/error.{type Error, CommandFailed}
import gleam/int
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result

/// A completed `bsdkrun` invocation.
pub type Output {
  Output(stdout: String, stderr: String, exit_code: Int)
}

/// A completed invocation whose stdout is kept as bytes.
///
/// A Gleam `String` must be valid UTF-8, which the contents of a file copied
/// out of a guest need not be — so `bsdkrun/filesystem` reads through this
/// instead of `Output`.
pub type BinaryOutput {
  BinaryOutput(stdout: BitArray, stderr: String, exit_code: Int)
}

/// How to run a command. Build one with `options` and adjust with the
/// `with_*` helpers.
pub type Options {
  Options(
    log_level: Int,
    env: List(#(String, String)),
    stdin: Option(String),
    on_stdout: Option(fn(String) -> Nil),
    on_stderr: Option(fn(String) -> Nil),
  )
}

/// Default run options: log level `0`, no extra environment, no stdin.
pub fn options() -> Options {
  Options(log_level: 0, env: [], stdin: None, on_stdout: None, on_stderr: None)
}

/// Set the global `--log-level` for this invocation.
pub fn with_log_level(opts: Options, level: Int) -> Options {
  Options(..opts, log_level: level)
}

/// Merge extra environment variables onto the child's environment.
pub fn with_env(opts: Options, env: List(#(String, String))) -> Options {
  Options(..opts, env: list.append(opts.env, env))
}

/// Pipe `data` to the child's stdin.
pub fn with_stdin(opts: Options, data: String) -> Options {
  Options(..opts, stdin: Some(data))
}

/// Receive stdout chunks while retaining them in the completed output.
pub fn with_stdout(opts: Options, callback: fn(String) -> Nil) -> Options {
  Options(..opts, on_stdout: Some(callback))
}

/// Receive stderr chunks while retaining them in the completed output.
pub fn with_stderr(opts: Options, callback: fn(String) -> Nil) -> Options {
  Options(..opts, on_stderr: Some(callback))
}

@external(erlang, "bsdkrun_ffi", "run")
fn ffi_run(
  bin: String,
  args: List(String),
  env: List(#(String, String)),
  stdin: Option(String),
  on_stdout: Option(fn(String) -> Nil),
  on_stderr: Option(fn(String) -> Nil),
) -> Output

@external(erlang, "bsdkrun_ffi", "run_bits")
fn ffi_run_bits(
  bin: String,
  args: List(String),
  env: List(#(String, String)),
  stdin: Option(BitArray),
) -> BinaryOutput

@external(erlang, "bsdkrun_ffi", "run_inherit")
fn ffi_run_inherit(bin: String, args: List(String)) -> Int

/// Run `bsdkrun <args>` with byte-exact stdin and stdout, whatever the exit
/// code. Used for file transfers; everything else wants `run`.
pub fn run_binary(
  args: List(String),
  stdin: Option(BitArray),
) -> Result(BinaryOutput, Error) {
  use bin <- result.try(binary.resolve())
  Ok(ffi_run_bits(bin, ["--log-level", "0", ..args], [], stdin))
}

/// Run `bsdkrun <args>` to completion and return its captured output,
/// whatever the exit code. Only binary resolution can fail here.
pub fn run(args: List(String), opts: Options) -> Result(Output, Error) {
  use bin <- result.try(binary.resolve())
  let full = ["--log-level", int.to_string(opts.log_level), ..args]
  Ok(ffi_run(bin, full, opts.env, opts.stdin, opts.on_stdout, opts.on_stderr))
}

/// Run `bsdkrun <args>` and fail on a non-zero exit. `label` names the command
/// in the resulting error.
pub fn checked(
  args: List(String),
  label: String,
  opts: Options,
) -> Result(Output, Error) {
  use out <- result.try(run(args, opts))

  case out.exit_code {
    0 -> Ok(out)
    code -> Error(CommandFailed(code, out.stdout, out.stderr, label))
  }
}

/// Like `checked`, but discards the output — for commands run only for their
/// effect (`stop`, `rm`, `update`, …).
pub fn checked_unit(
  args: List(String),
  label: String,
  opts: Options,
) -> Result(Nil, Error) {
  checked(args, label, opts) |> result.replace(Nil)
}

/// Run `bsdkrun <args>` with the child wired to this node's own stdio, for
/// interactive subcommands. Blocks until it exits and returns its exit code.
pub fn run_inherit(args: List(String)) -> Result(Int, Error) {
  use bin <- result.try(binary.resolve())
  Ok(ffi_run_inherit(bin, ["--log-level", "0", ..args]))
}
