//// The single error type returned by every fallible function in the SDK.

import gleam/int
import gleam/string

/// Why a call failed.
///
/// - `BinaryNotFound` — the `bsdkrun` binary could not be located; carries
///   every path that was searched.
/// - `CommandFailed` — a `bsdkrun` invocation exited non-zero; carries the
///   exit code, both captured streams, and a label naming the command.
/// - `SandboxNotFound` — no machine matched the given id or prefix.
/// - `DecodeFailed` — `bsdkrun --json` produced output the SDK could not
///   decode; carries the raw text.
/// - `InvalidOptions` — the create options were internally inconsistent, e.g.
///   a required field left empty.
pub type Error {
  BinaryNotFound(searched: List(String))
  CommandFailed(exit_code: Int, stdout: String, stderr: String, label: String)
  SandboxNotFound(id: String)
  DecodeFailed(label: String, raw: String)
  InvalidOptions(message: String)
}

/// A human-readable, single-block rendering of an error — the string you would
/// print or log.
pub fn to_string(error: Error) -> String {
  case error {
    BinaryNotFound(searched) ->
      "could not find the \"bsdkrun\" binary. Set BSDKRUN_BIN, add it to PATH, "
      <> "or call bsdkrun/binary.set_binary_path. Looked in: "
      <> string.join(searched, ", ")

    CommandFailed(exit_code, _stdout, stderr, label) -> {
      let detail = case string.trim(stderr) {
        "" -> ""
        trimmed -> "\n" <> trimmed
      }
      "command failed (exit "
      <> int.to_string(exit_code)
      <> "): "
      <> label
      <> detail
    }

    SandboxNotFound(id) -> "no sandbox found matching id \"" <> id <> "\""

    DecodeFailed(label, raw) ->
      "could not decode JSON from " <> label <> ": " <> string.inspect(raw)

    InvalidOptions(message) -> "invalid options: " <> message
  }
}
