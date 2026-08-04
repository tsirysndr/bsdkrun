//// A handle to a running (or stopped) `bsdkrun` microVM.
////
//// ```gleam
//// import bsdkrun/args
//// import bsdkrun/sandbox
////
//// let assert Ok(box) = sandbox.create(args.linux("alpine"))
//// let assert Ok(res) = sandbox.exec(box, ["uname", "-a"], sandbox.exec_options())
//// let assert Ok(Nil) = sandbox.stop(box)
//// ```
////
//// Functions that act on a machine take a `Sandbox`; use `from_id` to build
//// one from a bare id you already hold.

import bsdkrun/args.{type CreateOptions}
import bsdkrun/cli
import bsdkrun/error.{type Error, CommandFailed, SandboxNotFound}
import bsdkrun/types.{type CommandResult, type SandboxInfo, CommandResult}
import gleam/int
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result
import gleam/string

/// A handle to a machine. `ssh_port` is populated on `create` when bsdkrun
/// reports a forwarded SSH port.
pub type Sandbox {
  Sandbox(id: String, ssh_port: Option(Int))
}

/// Build a handle from a machine id you already have. No lookup is performed —
/// use `get` if you want the id validated.
pub fn from_id(id: String) -> Sandbox {
  Sandbox(id: id, ssh_port: None)
}

// --- construction / discovery -----------------------------------------------

/// Boot a new microVM (detached) and return a handle to it.
pub fn create(opts: CreateOptions) -> Result(Sandbox, Error) {
  use argv <- result.try(args.build_create(opts))

  let run_opts = cli.options() |> cli.with_log_level(opts.log_level)
  use out <- result.try(cli.run(argv, run_opts))

  case out.exit_code {
    0 ->
      case parse_id(out.stdout) {
        Some(id) -> Ok(Sandbox(id: id, ssh_port: parse_ssh_port(out.stderr)))
        None ->
          Error(CommandFailed(
            out.exit_code,
            out.stdout,
            out.stderr,
            "bsdkrun create (no machine id in output)",
          ))
      }
    code -> Error(CommandFailed(code, out.stdout, out.stderr, "bsdkrun create"))
  }
}

/// Reconnect to an existing machine by id — a unique prefix is enough.
pub fn get(id: String) -> Result(Sandbox, Error) {
  use all <- result.try(list_all(True))

  case list.find(all, fn(m) { m.id == id || string.starts_with(m.id, id) }) {
    Ok(found) -> Ok(Sandbox(id: found.id, ssh_port: None))
    Error(_) -> Error(SandboxNotFound(id))
  }
}

/// List running machines.
pub fn list() -> Result(List(SandboxInfo), Error) {
  list_all(False)
}

/// List machines, including exited ones when `all` is `True`.
pub fn list_all(all: Bool) -> Result(List(SandboxInfo), Error) {
  let argv = case all {
    True -> ["ps", "--json", "--all"]
    False -> ["ps", "--json"]
  }

  use out <- result.try(cli.checked(argv, "bsdkrun ps", cli.options()))
  types.decode_rows(out.stdout, "bsdkrun ps", types.sandbox_info_decoder())
}

// --- exec -------------------------------------------------------------------

/// How to run a command inside a guest. Build one with `exec_options`.
pub type ExecOptions {
  ExecOptions(
    env: List(#(String, String)),
    tty: Bool,
    stdin: Option(String),
    cwd: Option(String),
    fail_on_error: Bool,
    log_level: Int,
  )
}

/// Default exec options: no env, no TTY, no stdin, no cwd, a non-zero exit
/// returned as a `CommandResult` rather than an `Error`.
pub fn exec_options() -> ExecOptions {
  ExecOptions(
    env: [],
    tty: False,
    stdin: None,
    cwd: None,
    fail_on_error: False,
    log_level: 0,
  )
}

/// Set environment variables for the command.
pub fn with_env(
  opts: ExecOptions,
  env: List(#(String, String)),
) -> ExecOptions {
  ExecOptions(..opts, env: list.append(opts.env, env))
}

/// Allocate a pseudo-TTY for the command.
pub fn with_tty(opts: ExecOptions, tty: Bool) -> ExecOptions {
  ExecOptions(..opts, tty: tty)
}

/// Pipe `data` to the command's stdin.
pub fn with_stdin(opts: ExecOptions, data: String) -> ExecOptions {
  ExecOptions(..opts, stdin: Some(data))
}

/// Run the command from `dir` inside the guest.
pub fn with_cwd(opts: ExecOptions, dir: String) -> ExecOptions {
  ExecOptions(..opts, cwd: Some(dir))
}

/// Return `Error(CommandFailed(..))` instead of a `CommandResult` when the
/// command exits non-zero.
pub fn with_fail_on_error(opts: ExecOptions, fail: Bool) -> ExecOptions {
  ExecOptions(..opts, fail_on_error: fail)
}

/// Run a command in the guest through its exec agent.
///
/// The `Error` case covers only failures to *run* the command (no binary, or
/// `fail_on_error` with a non-zero exit); an ordinary non-zero exit is
/// reported in the returned `CommandResult`.
pub fn exec(
  box: Sandbox,
  command: List(String),
  opts: ExecOptions,
) -> Result(CommandResult, Error) {
  let argv = case opts.cwd {
    // bsdkrun's agent has no cwd concept, so emulate it: `sh -c 'cd "$1" &&
    // shift && exec "$@"' sh <dir> <argv…>` keeps the command's own arguments
    // out of the shell's parsing entirely.
    Some(dir) ->
      list.append(
        ["/bin/sh", "-c", "cd \"$1\" && shift && exec \"$@\"", "sh", dir],
        command,
      )
    None -> command
  }

  let full =
    list.flatten([
      ["exec"],
      case opts.tty {
        True -> ["-t"]
        False -> []
      },
      list.flat_map(opts.env, fn(pair) {
        let #(k, v) = pair
        ["-e", k <> "=" <> v]
      }),
      [box.id],
      argv,
    ])

  let run_opts =
    cli.options()
    |> cli.with_log_level(opts.log_level)
    |> fn(o) {
      case opts.stdin {
        Some(data) -> cli.with_stdin(o, data)
        None -> o
      }
    }

  use out <- result.try(cli.run(full, run_opts))

  let label = "exec " <> string.join(argv, " ")
  let res =
    CommandResult(
      stdout: out.stdout,
      stderr: out.stderr,
      exit_code: out.exit_code,
      command: label,
    )

  case opts.fail_on_error && out.exit_code != 0 {
    True -> Error(CommandFailed(out.exit_code, out.stdout, out.stderr, label))
    False -> Ok(res)
  }
}

// --- logs -------------------------------------------------------------------

/// Read the machine's console log.
pub fn logs(box: Sandbox) -> Result(String, Error) {
  read_logs(box, False)
}

/// Read bsdkrun's own boot log for the machine.
pub fn boot_logs(box: Sandbox) -> Result(String, Error) {
  read_logs(box, True)
}

fn read_logs(box: Sandbox, boot: Bool) -> Result(String, Error) {
  let argv = case boot {
    True -> ["logs", "--boot", box.id]
    False -> ["logs", box.id]
  }

  cli.checked(argv, "bsdkrun logs", cli.options())
  |> result.map(fn(out) { out.stdout })
}

// --- lifecycle --------------------------------------------------------------

/// Stop the machine — BSD guests get a clean poweroff, Linux gets SIGTERM.
pub fn stop(box: Sandbox) -> Result(Nil, Error) {
  cli.checked_unit(["stop", box.id], "bsdkrun stop", cli.options())
}

/// Restart a stopped machine in place: same id, disk, and resources.
pub fn start(box: Sandbox) -> Result(Nil, Error) {
  cli.checked_unit(["start", box.id], "bsdkrun start", cli.options())
}

/// Remove the machine and its state. `force` stops it first if it is running.
pub fn remove(box: Sandbox, force: Bool) -> Result(Nil, Error) {
  let argv = case force {
    True -> ["rm", "--force", box.id]
    False -> ["rm", box.id]
  }
  cli.checked_unit(argv, "bsdkrun rm", cli.options())
}

/// Change the recorded vCPU / RAM. Applies on the next `start`.
pub fn update(
  box: Sandbox,
  cpus: Option(Int),
  mem: Option(Int),
) -> Result(Nil, Error) {
  let argv =
    list.flatten([
      ["update", box.id],
      flag_int("--cpus", cpus),
      flag_int("--mem", mem),
    ])

  cli.checked_unit(argv, "bsdkrun update", cli.options())
}

/// Join or switch this machine to a global network. Applies on the next
/// `start`.
pub fn connect_network(box: Sandbox, network: String) -> Result(Nil, Error) {
  cli.checked_unit(
    ["network", "connect", box.id, network],
    "bsdkrun network connect",
    cli.options(),
  )
}

/// Detach this machine from its network. Applies on the next `start`.
pub fn disconnect_network(box: Sandbox) -> Result(Nil, Error) {
  cli.checked_unit(
    ["network", "disconnect", box.id],
    "bsdkrun network disconnect",
    cli.options(),
  )
}

// --- status -----------------------------------------------------------------

/// This machine's current status row, or `None` if it is gone.
pub fn status(box: Sandbox) -> Result(Option(SandboxInfo), Error) {
  use all <- result.try(list_all(True))

  Ok(
    list.find(all, fn(m) { m.id == box.id })
    |> option.from_result,
  )
}

/// Whether the machine is currently running. A machine that cannot be found
/// counts as not running.
pub fn is_running(box: Sandbox) -> Bool {
  case status(box) {
    Ok(Some(info)) -> info.running
    _ -> False
  }
}

// --- agent conveniences -----------------------------------------------------

/// Install SSH keys in the guest via the agent. With no `keys`, the CLI
/// installs your local `~/.ssh/*.pub`.
pub fn ssh_setup(
  box: Sandbox,
  user: Option(String),
  keys: List(String),
) -> Result(CommandResult, Error) {
  let action =
    list.flatten([
      ["setup"],
      opt_flag("--user", user),
      list.flat_map(keys, fn(k) { ["--key", k] }),
    ])

  agent(box, "ssh", action, [])
}

/// Put the guest on your tailnet via the agent. The auth key is passed through
/// the environment as `TS_AUTHKEY`, so it never lands in an argument list.
pub fn tailscale_up(
  box: Sandbox,
  authkey: Option(String),
  hostname: Option(String),
  extra: List(String),
) -> Result(CommandResult, Error) {
  let action =
    list.flatten([["setup"], opt_flag("--hostname", hostname), extra])

  let env = case authkey {
    Some(key) -> [#("TS_AUTHKEY", key)]
    None -> []
  }

  agent(box, "tailscale", action, env)
}

fn agent(
  box: Sandbox,
  family: String,
  action: List(String),
  env: List(#(String, String)),
) -> Result(CommandResult, Error) {
  let run_opts = cli.options() |> cli.with_env(env)
  use out <- result.try(cli.run([family, box.id, ..action], run_opts))

  let label = family <> " " <> string.join(action, " ")

  case out.exit_code {
    0 ->
      Ok(CommandResult(
        stdout: out.stdout,
        stderr: out.stderr,
        exit_code: 0,
        command: label,
      ))
    code -> Error(CommandFailed(code, out.stdout, out.stderr, label))
  }
}

/// Attach an interactive shell to the machine, inheriting this node's stdio.
/// Blocks until the shell exits and returns its exit status.
pub fn shell(box: Sandbox) -> Result(Int, Error) {
  cli.run_inherit(["shell", box.id])
}

// --- internals --------------------------------------------------------------

/// bsdkrun prints the new machine's id on stdout; take the last id-shaped
/// line so any preamble is ignored.
fn parse_id(stdout: String) -> Option(String) {
  stdout
  |> string.split("\n")
  |> list.map(string.trim)
  |> list.filter(is_machine_id)
  |> list.last
  |> option.from_result
}

/// A machine id is six or more lowercase hex digits, and nothing else.
fn is_machine_id(line: String) -> Bool {
  let graphemes = string.to_graphemes(line)

  list.length(graphemes) >= 6
  && list.all(graphemes, fn(c) { string.contains("0123456789abcdef", c) })
}

/// bsdkrun mentions the forwarded SSH port on stderr, as `ssh -p <port>`.
fn parse_ssh_port(stderr: String) -> Option(Int) {
  case string.split_once(stderr, "ssh -p ") {
    Ok(#(_before, after)) ->
      after
      |> string.to_graphemes
      |> list.take_while(fn(c) { string.contains("0123456789", c) })
      |> string.join("")
      |> int.parse
      |> option.from_result
    Error(_) -> None
  }
}

fn flag_int(name: String, value: Option(Int)) -> List(String) {
  case value {
    Some(v) -> [name, int.to_string(v)]
    None -> []
  }
}

fn opt_flag(name: String, value: Option(String)) -> List(String) {
  case value {
    Some(v) -> [name, v]
    None -> []
  }
}
