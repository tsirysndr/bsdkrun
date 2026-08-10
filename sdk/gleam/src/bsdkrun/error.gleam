//// The single error type returned by every fallible function in the SDK.

import gleam/int
import gleam/option.{type Option, None, Some}
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
/// - `GraphqlError` — talking to a remote `bsdkrund` through `bsdkrun/client`
///   failed: a resolver returned a GraphQL error, the response could not be
///   parsed, or the daemon could not be reached at all. `code` carries the
///   GraphQL `extensions.code` the daemon sent, when there was one.
/// - `AuthError` — the daemon rejected the bearer token: an HTTP 401, a
///   GraphQL error whose `extensions.code` is `"UNAUTHENTICATED"`, or a
///   subscription socket that closed before `connection_ack` ever arrived.
///
/// The last two are only ever produced by `bsdkrun/client` (the remote
/// GraphQL client added alongside the local, CLI-shelling API this type was
/// originally written for) — every other function in this SDK only produces
/// the first five.
pub type Error {
  BinaryNotFound(searched: List(String))
  CommandFailed(exit_code: Int, stdout: String, stderr: String, label: String)
  SandboxNotFound(id: String)
  DecodeFailed(label: String, raw: String)
  InvalidOptions(message: String)
  GraphqlError(message: String, code: Option(String))
  AuthError(message: String)
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

    GraphqlError(message, code) ->
      case code {
        Some(c) -> message <> " (" <> c <> ")"
        None -> message
      }

    AuthError(message) -> message
  }
}
