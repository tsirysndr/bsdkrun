//// The HTTP half of the GraphQL transport used by `bsdkrun/client`: one
//// `POST` per query or mutation, plus URL normalization. The WebSocket half
//// (subscriptions) is `bsdkrun/ws`.
////
//// Mirrors `web/src/lib/graphql.ts`'s `gql()` and
//// `web/src/lib/connection.ts`'s `normalizeUrl` — same request shape, same
//// status/error interpretation, same URL rules, just over `:httpc`
//// (`bsdkrun_remote_ffi.erl`'s `http_post/3`) instead of `fetch`.
////
//// `parse_response` — the status-code/body interpretation — is factored out
//// from `execute` specifically so it can be unit-tested against literal
//// `(status, body)` pairs with no socket involved (see `test/client_test.gleam`),
//// per this feature's own suggested fallback for testing the HTTP layer.

import bsdkrun/error.{type Error, AuthError, GraphqlError}
import gleam/dynamic.{type Dynamic}
import gleam/dynamic/decode
import gleam/int
import gleam/json.{type Json}
import gleam/option.{type Option, None, Some}
import gleam/string

@external(erlang, "bsdkrun_remote_ffi", "http_post")
fn ffi_http_post(
  url: String,
  token: String,
  body: String,
) -> Result(#(Int, String), String)

/// Run one GraphQL query or mutation against `url` (the full endpoint URL,
/// e.g. `http://host:50052/graphql`) with `token`, and return its `data`
/// field as a `Dynamic` for the caller to decode.
pub fn execute(
  url: String,
  token: String,
  query: String,
  variables: Json,
) -> Result(Dynamic, Error) {
  execute_raw(url, token, query, json.to_string(variables))
}

/// Like `execute`, but takes `variables` as an already-serialized JSON
/// string rather than a `gleam_json`-built `Json` value. Used by
/// `bsdkrun/client`'s `request`/`subscribe` escape hatch, whose caller hands
/// in a `Dynamic` — `bsdkrun_remote_ffi.erl`'s `dynamic_to_json/1` turns
/// that into a JSON string directly, with nowhere for a `Json` value to come
/// from in between.
pub fn execute_raw(
  url: String,
  token: String,
  query: String,
  variables_json: String,
) -> Result(Dynamic, Error) {
  let body =
    "{\"query\":"
    <> json.to_string(json.string(query))
    <> ",\"variables\":"
    <> variables_json
    <> "}"

  case ffi_http_post(url, token, body) {
    // `http_post` only ever returns `Error` for a transport-level failure —
    // the daemon unreachable, refused, timed out, TLS handshake failed —
    // never for a non-2xx HTTP status, which is a normal response `execute`
    // still has to interpret (a 401 in particular).
    Error(reason) ->
      Error(GraphqlError(
        "cannot reach the bsdkrun daemon at " <> url <> " — " <> reason,
        None,
      ))
    Ok(#(status, resp_body)) -> parse_response(status, resp_body)
  }
}

/// Interpret one HTTP response: a `401` is always an `AuthError`; otherwise
/// the body is parsed as JSON and, if `errors` is a non-empty array, its
/// first entry becomes an `AuthError` (when `extensions.code` is
/// `"UNAUTHENTICATED"`) or a `GraphqlError` (any other error). With no
/// errors, the `data` field is returned as-is for the caller to decode.
pub fn parse_response(status: Int, body: String) -> Result(Dynamic, Error) {
  case status {
    401 -> Error(AuthError("the daemon rejected this token"))
    _ ->
      case json.parse(body, decode.dynamic) {
        Error(_) ->
          Error(GraphqlError(
            "the daemon returned a non-JSON response ("
              <> int.to_string(status)
              <> ")",
            None,
          ))
        Ok(dyn) -> interpret_body(dyn)
      }
  }
}

fn interpret_body(dyn: Dynamic) -> Result(Dynamic, Error) {
  case error_list(dyn) {
    [first, ..] -> {
      let message = case error_message(first) {
        "" -> "the daemon returned a GraphQL error"
        m -> m
      }
      case error_code(first) {
        Some("UNAUTHENTICATED") -> Error(AuthError(message))
        code -> Error(GraphqlError(message, code))
      }
    }
    [] -> Ok(data_field(dyn))
  }
}

// Every helper below does its own isolated `decode.run` and defaults on any
// `Error` — deliberately not composed into one big decoder, so a
// differently-shaped `errors[0]` (missing `extensions`, `extensions` not an
// object, `code` the wrong type, ...) degrades that one field to its default
// instead of poisoning the whole response's interpretation and silently
// treating a real GraphQL error as success.

fn error_list(dyn: Dynamic) -> List(Dynamic) {
  case
    decode.run(
      dyn,
      decode.field("errors", decode.list(decode.dynamic), decode.success),
    )
  {
    Ok(errors) -> errors
    Error(_) -> []
  }
}

fn error_message(dyn: Dynamic) -> String {
  case decode.run(dyn, decode.field("message", decode.string, decode.success)) {
    Ok(message) -> message
    Error(_) -> ""
  }
}

fn error_code(dyn: Dynamic) -> Option(String) {
  case decode.run(dyn, decode.at(["extensions", "code"], decode.string)) {
    Ok(code) -> Some(code)
    Error(_) -> None
  }
}

fn data_field(dyn: Dynamic) -> Dynamic {
  case decode.run(dyn, decode.field("data", decode.dynamic, decode.success)) {
    Ok(data) -> data
    Error(_) -> dyn
  }
}

// ---------------------------------------------------------------------------
// URL normalization
// ---------------------------------------------------------------------------

/// Accept what a person actually types or pastes and turn it into the
/// GraphQL endpoint URL: trim, add `http://` when no scheme is given, strip
/// trailing slashes, append `/graphql` unless the path already ends with it.
/// Mirrors `web/src/lib/connection.ts`'s `normalizeUrl` exactly, including
/// leaving an empty/blank input as `""` rather than inventing an endpoint.
pub fn normalize_url(input: String) -> String {
  case string.trim(input) {
    "" -> ""
    trimmed -> {
      let with_scheme = case has_scheme(trimmed) {
        True -> trimmed
        False -> "http://" <> trimmed
      }
      let no_trailing_slash = drop_trailing_slashes(with_scheme)
      case string.lowercase(no_trailing_slash) |> string.ends_with("/graphql") {
        True -> no_trailing_slash
        False -> no_trailing_slash <> "/graphql"
      }
    }
  }
}

fn has_scheme(s: String) -> Bool {
  let lower = string.lowercase(s)
  string.starts_with(lower, "http://") || string.starts_with(lower, "https://")
}

fn drop_trailing_slashes(s: String) -> String {
  case string.ends_with(s, "/") {
    True -> drop_trailing_slashes(string.drop_end(s, 1))
    False -> s
  }
}
