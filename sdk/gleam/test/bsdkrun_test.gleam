import bsdkrun/args
import bsdkrun/binary
import bsdkrun/cli
import bsdkrun/error
import bsdkrun/types
import gleam/list
import gleam/option.{None, Some}
import gleam/string
import gleeunit
import gleeunit/should

pub fn main() -> Nil {
  gleeunit.main()
}

// --- args: per-guest argv ---------------------------------------------------

fn build(opts: args.CreateOptions) -> List(String) {
  let assert Ok(argv) = args.build_create(opts)
  argv
}

pub fn linux_minimal_test() {
  build(args.linux("alpine"))
  |> should.equal(["linux", "alpine", "-d"])
}

pub fn linux_full_test() {
  args.linux("alpine")
  |> args.with_name("web")
  |> args.with_cpus(2)
  |> args.with_mem(1024)
  |> args.with_mounts(["/host:/guest"])
  |> args.with_entrypoint("/bin/sh")
  |> args.with_ports([args.Port(8080, 80)])
  |> args.with_command(["echo", "hi"])
  |> build
  |> should.equal([
    "linux", "alpine", "-d", "--mount", "/host:/guest", "--entrypoint",
    "/bin/sh", "--port", "8080:80", "--name", "web", "--cpus", "2", "--mem",
    "1024", "--", "echo", "hi",
  ])
}

pub fn freebsd_test() {
  args.freebsd()
  |> args.with_version("15.0")
  |> args.with_persist(True)
  |> args.with_volume("data")
  |> build
  |> should.equal([
    "freebsd", "-d", "--version", "15.0", "--persist", "-v", "data",
  ])
}

pub fn netbsd_test() {
  args.netbsd()
  |> args.with_force(True)
  |> args.with_attach_disk(["/tmp/extra.img"])
  |> build
  |> should.equal(["netbsd", "-d", "--force", "--attach-disk", "/tmp/extra.img"])
}

pub fn firmware_test() {
  args.firmware("/edk2.fd", "/disk.raw")
  |> build
  |> should.equal([
    "firmware", "--firmware", "/edk2.fd", "--disk", "/disk.raw", "-d",
  ])
}

pub fn kernel_test() {
  args.kernel("/vmlinuz")
  |> args.with_no_net(True)
  |> build
  |> should.equal(["kernel", "--kernel", "/vmlinuz", "-d", "--no-net"])
}

pub fn network_and_mac_test() {
  args.linux("alpine")
  |> args.with_network("lab")
  |> args.with_mac("52:54:00:12:34:56")
  |> build
  |> should.equal([
    "linux", "alpine", "-d", "--mac", "52:54:00:12:34:56", "--network", "lab",
  ])
}

pub fn multiple_ports_test() {
  args.linux("alpine")
  |> args.with_ports([args.Port(8080, 80), args.Port(2222, 22)])
  |> build
  |> should.equal([
    "linux", "alpine", "-d", "--port", "8080:80", "--port", "2222:22",
  ])
}

/// Linux takes `-v` but not the disk-boot flags; the BSD kinds take both.
pub fn linux_ignores_disk_flags_test() {
  args.linux("alpine")
  |> args.with_persist(True)
  |> args.with_volume("data")
  |> args.with_attach_disk(["/tmp/x.img"])
  |> build
  |> should.equal(["linux", "alpine", "-d", "-v", "data"])
}

/// Linux-only setters are no-ops on other guest kinds rather than errors.
pub fn command_ignored_for_bsd_test() {
  args.netbsd()
  |> args.with_command(["echo", "hi"])
  |> build
  |> should.equal(["netbsd", "-d"])
}

pub fn empty_image_is_rejected_test() {
  case args.build_create(args.linux("")) {
    Error(error.InvalidOptions(_)) -> Nil
    other -> panic as string.inspect(other)
  }
}

// --- types: decoding --------------------------------------------------------

pub fn decode_sandbox_rows_test() {
  let raw =
    "[{\"id\":\"abc123\",\"name\":\"web\",\"image\":\"alpine\",\"kind\":\"linux\","
    <> "\"command\":\"sh\",\"running\":true,\"cpus\":2,\"mem\":1024,"
    <> "\"state_dir\":\"/tmp/x\",\"created_at\":1700000000,\"network\":\"lab\"}]"

  let assert Ok([row]) =
    types.decode_rows(raw, "ps", types.sandbox_info_decoder())

  row.id |> should.equal("abc123")
  row.name |> should.equal(Some("web"))
  row.running |> should.equal(True)
  row.cpus |> should.equal(2)
  row.network |> should.equal(Some("lab"))
  row.exit_code |> should.equal(None)
  types.status(row) |> should.equal("running")
}

/// Absent optional keys decode as `None` rather than failing the whole row.
pub fn decode_sparse_row_test() {
  let assert Ok([row]) =
    types.decode_rows("[{\"id\":\"abc\"}]", "ps", types.sandbox_info_decoder())

  row.id |> should.equal("abc")
  row.name |> should.equal(None)
  row.running |> should.equal(False)
  row.cpus |> should.equal(0)
  types.status(row) |> should.equal("exited")
}

/// The CLI prints nothing when there is nothing to list.
pub fn decode_blank_output_test() {
  types.decode_rows("", "ps", types.sandbox_info_decoder())
  |> should.equal(Ok([]))

  types.decode_rows("  \n ", "ps", types.sandbox_info_decoder())
  |> should.equal(Ok([]))
}

/// Numeric columns are accepted as strings too.
pub fn decode_stringy_numbers_test() {
  let assert Ok([row]) =
    types.decode_rows(
      "[{\"id\":\"a\",\"cpus\":\"4\",\"created_at\":\"1700000000\"}]",
      "ps",
      types.sandbox_info_decoder(),
    )

  row.cpus |> should.equal(4)
  row.created_at |> should.equal(1_700_000_000)
}

pub fn decode_failure_is_reported_test() {
  case types.decode_rows("not json", "ps", types.sandbox_info_decoder()) {
    Error(error.DecodeFailed("ps", "not json")) -> Nil
    other -> panic as string.inspect(other)
  }
}

pub fn decode_network_rows_test() {
  let raw =
    "[{\"name\":\"lab\",\"subnet\":\"10.0.0.0/24\",\"gateway\":\"10.0.0.1\","
    <> "\"members\":3,\"running\":2,\"up\":true}]"

  let assert Ok([row]) =
    types.decode_rows(raw, "network ls", types.network_info_decoder())

  row.name |> should.equal("lab")
  row.members |> should.equal(3)
  row.up |> should.equal(True)
  row.created_at |> should.equal(None)
}

// --- types: CommandResult helpers -------------------------------------------

pub fn command_result_helpers_test() {
  let res =
    types.CommandResult(
      stdout: "one\ntwo\n\n",
      stderr: "",
      exit_code: 0,
      command: "exec echo",
    )

  types.is_ok(res) |> should.equal(True)
  types.text(res) |> should.equal("one\ntwo")
  types.lines(res) |> should.equal(["one", "two"])

  types.is_ok(types.CommandResult(..res, exit_code: 1)) |> should.equal(False)
}

// --- errors -----------------------------------------------------------------

pub fn command_failed_message_test() {
  error.CommandFailed(2, "", "boom\n", "bsdkrun ps")
  |> error.to_string
  |> should.equal("command failed (exit 2): bsdkrun ps\nboom")
}

pub fn command_failed_without_stderr_test() {
  error.CommandFailed(1, "", "  ", "bsdkrun rm")
  |> error.to_string
  |> should.equal("command failed (exit 1): bsdkrun rm")
}

pub fn sandbox_not_found_message_test() {
  error.SandboxNotFound("abc")
  |> error.to_string
  |> should.equal("no sandbox found matching id \"abc\"")
}

pub fn binary_not_found_lists_paths_test() {
  let message = error.to_string(error.BinaryNotFound(["/a/bsdkrun", "bsdkrun"]))

  string.contains(message, "/a/bsdkrun") |> should.equal(True)
  string.contains(message, "BSDKRUN_BIN") |> should.equal(True)
}

// --- binary discovery -------------------------------------------------------

/// `resolve` never returns a path that isn't there: a bogus override is
/// skipped rather than handed back.
pub fn binary_override_test() {
  binary.set_binary_path("/definitely/not/a/real/bsdkrun")

  case binary.resolve() {
    Ok(found) -> found |> should.not_equal("/definitely/not/a/real/bsdkrun")
    Error(error.BinaryNotFound(searched)) ->
      list.contains(searched, "/definitely/not/a/real/bsdkrun")
      |> should.equal(True)
    other -> panic as string.inspect(other)
  }

  binary.set_binary_path("/bin/sh")
  binary.resolve() |> should.equal(Ok("/bin/sh"))

  binary.reset_binary_path()
}

pub fn binary_candidates_include_dev_builds_test() {
  binary.reset_binary_path()
  let candidates = binary.candidates()

  list.contains(candidates, "bsdkrun") |> should.equal(True)
  list.any(candidates, string.ends_with(_, "/target/release/bsdkrun"))
  |> should.equal(True)
}

// --- cli: the FFI actually runs things --------------------------------------

/// A stand-in for the real binary that tolerates the SDK's global
/// `--log-level` prefix and then behaves like `sh`. Relative to the project
/// root, which is where `gleam test` runs.
const fake_binary = "./test/support/fake-bsdkrun"

/// Point the SDK at `/bin/echo` and check argv reaches the child intact,
/// including the global `--log-level` prefix.
pub fn cli_run_captures_stdout_test() {
  binary.set_binary_path("/bin/echo")

  let assert Ok(out) = cli.run(["hello", "world"], cli.options())

  out.exit_code |> should.equal(0)
  out.stdout |> should.equal("--log-level 0 hello world\n")
  out.stderr |> should.equal("")

  binary.reset_binary_path()
}

/// stderr must come back separately from stdout, not interleaved into it.
pub fn cli_separates_streams_test() {
  binary.set_binary_path(fake_binary)

  let assert Ok(out) =
    cli.run(["-c", "echo out; echo err >&2; exit 3"], cli.options())

  out.exit_code |> should.equal(3)
  string.contains(out.stdout, "out") |> should.equal(True)
  string.contains(out.stdout, "err") |> should.equal(False)
  string.contains(out.stderr, "err") |> should.equal(True)

  binary.reset_binary_path()
}

pub fn cli_pipes_stdin_test() {
  binary.set_binary_path(fake_binary)

  let assert Ok(out) =
    cli.run(["-c", "cat"], cli.options() |> cli.with_stdin("piped\n"))

  out.exit_code |> should.equal(0)
  string.contains(out.stdout, "piped") |> should.equal(True)

  binary.reset_binary_path()
}

pub fn cli_passes_env_test() {
  binary.set_binary_path(fake_binary)

  let assert Ok(out) =
    cli.run(
      ["-c", "printf %s \"$BSDKRUN_TEST_VAR\""],
      cli.options() |> cli.with_env([#("BSDKRUN_TEST_VAR", "set-by-test")]),
    )

  string.contains(out.stdout, "set-by-test") |> should.equal(True)

  binary.reset_binary_path()
}

/// Arguments carrying shell metacharacters must reach the child verbatim —
/// the `sh -c` wrapper used for redirection must never re-parse them.
pub fn cli_does_not_reparse_arguments_test() {
  binary.set_binary_path("/bin/echo")

  let assert Ok(out) =
    cli.run(["a b; rm -rf /", "$HOME", "`id`"], cli.options())

  string.contains(out.stdout, "a b; rm -rf /") |> should.equal(True)
  string.contains(out.stdout, "$HOME") |> should.equal(True)
  string.contains(out.stdout, "`id`") |> should.equal(True)

  binary.reset_binary_path()
}

pub fn cli_checked_reports_failure_test() {
  binary.set_binary_path(fake_binary)

  case cli.checked(["-c", "echo nope >&2; exit 7"], "label", cli.options()) {
    Error(error.CommandFailed(7, _, stderr, "label")) ->
      string.contains(stderr, "nope") |> should.equal(True)
    other -> panic as string.inspect(other)
  }

  binary.reset_binary_path()
}

/// The log level reaches the child as a global flag ahead of the subcommand.
pub fn cli_log_level_prefix_test() {
  binary.set_binary_path("/bin/echo")

  let assert Ok(out) = cli.run(["ps"], cli.options() |> cli.with_log_level(3))

  out.stdout |> should.equal("--log-level 3 ps\n")

  binary.reset_binary_path()
}

// A unikernel takes no disk options — except volumes, which are virtio-fs
// shares and need neither a disk nor an agent. The path stays positional and
// last, as the CLI expects.
pub fn unikraft_with_volumes_test() {
  build(args.new(args.Unikraft(
    path: "~/x",
    cmdline: None,
    initramfs: None,
    mounts: ["~/d:/data", "~/l:/logs"],
  )))
  |> should.equal([
    "unikraft", "-d", "--mount", "~/d:/data", "--mount", "~/l:/logs", "~/x",
  ])
}

pub fn unikraft_without_volumes_test() {
  build(args.unikraft("."))
  |> should.equal(["unikraft", "-d", "."])
}
