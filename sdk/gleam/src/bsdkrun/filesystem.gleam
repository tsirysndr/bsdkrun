//// Files in a running sandbox.
////
//// Every call goes through the guest's exec agent, so the sandbox has to be
//// running — there is no offline write. Each function takes the machine id, in
//// keeping with the rest of the SDK's shape.
////
//// ```gleam
//// import bsdkrun/filesystem
////
//// let assert Ok(Nil) = filesystem.write_text("web", "/app/main.py", "print(1)")
//// let assert Ok(bytes) = filesystem.read_file("web", "/app/out.json")
//// let assert Ok(Nil) = filesystem.upload("web", "./src", "/app/src")
//// let assert Ok(Nil) = filesystem.download("web", "/app/dist", "./dist", True)
//// ```

import bsdkrun/cli
import bsdkrun/error.{type Error, FileTransferFailed}
import gleam/bit_array
import gleam/list
import gleam/option.{None, Some}
import gleam/result
import gleam/string

/// Write `data` to `path` in the guest, creating parent directories.
pub fn write_file(
  id: String,
  path: String,
  data: BitArray,
) -> Result(Nil, Error) {
  use out <- result.try(cli.run_binary(
    ["cp", "-", id <> ":" <> path],
    Some(data),
  ))
  check(out.exit_code, out.stderr, path)
}

/// Write `text` to `path` in the guest.
pub fn write_text(
  id: String,
  path: String,
  text: String,
) -> Result(Nil, Error) {
  write_file(id, path, bit_array.from_string(text))
}

/// Read `path` from the guest as bytes.
pub fn read_file(id: String, path: String) -> Result(BitArray, Error) {
  use out <- result.try(cli.run_binary(["cp", id <> ":" <> path, "-"], None))
  use _ <- result.try(check(out.exit_code, out.stderr, path))
  Ok(out.stdout)
}

/// Read `path` from the guest and decode it as UTF-8.
pub fn read_text(id: String, path: String) -> Result(String, Error) {
  use bytes <- result.try(read_file(id, path))
  case bit_array.to_string(bytes) {
    Ok(text) -> Ok(text)
    Error(Nil) ->
      Error(FileTransferFailed(path, path <> " is not valid UTF-8 text"))
  }
}

/// Copy a host file or directory into the guest.
///
/// A directory's *contents* land in `remote_path`, so
/// `upload(id, "./src", "/app/src")` leaves the guest's `/app/src` holding what
/// `./src` holds. Pass `recursive` for a directory: unlike the other SDKs this
/// is explicit, because Gleam has no stat in its standard library.
pub fn upload(
  id: String,
  local_path: String,
  remote_path: String,
  recursive: Bool,
) -> Result(Nil, Error) {
  let flags = case recursive {
    True -> ["-r"]
    False -> []
  }
  use out <- result.try(cli.run(
    list.append(["cp", ..flags], [local_path, id <> ":" <> remote_path]),
    cli.options(),
  ))
  check(out.exit_code, out.stderr, local_path)
}

/// Copy a file or directory out of the guest onto the host. `recursive`
/// selects a directory.
pub fn download(
  id: String,
  remote_path: String,
  local_path: String,
  recursive: Bool,
) -> Result(Nil, Error) {
  let flags = case recursive {
    True -> ["-r"]
    False -> []
  }
  use out <- result.try(cli.run(
    list.append(["cp", ..flags], [id <> ":" <> remote_path, local_path]),
    cli.options(),
  ))
  check(out.exit_code, out.stderr, remote_path)
}

/// The CLI already explains these well; strip its `Error: ` prefix.
fn check(exit_code: Int, stderr: String, path: String) -> Result(Nil, Error) {
  case exit_code {
    0 -> Ok(Nil)
    _ -> {
      let text =
        stderr
        |> string.trim
        |> strip_prefix("Error: ")
      let message = case text {
        "" -> "file transfer failed for " <> path
        other -> other
      }
      Error(FileTransferFailed(path, message))
    }
  }
}

fn strip_prefix(text: String, prefix: String) -> String {
  case string.starts_with(text, prefix) {
    True -> string.drop_start(text, string.length(prefix))
    False -> text
  }
}
