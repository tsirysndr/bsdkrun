//// Locating the `bsdkrun` binary.
////
//// Resolution order, first match wins:
////
////   1. an explicit override set with `set_binary_path`,
////   2. the `$BSDKRUN_BIN` environment variable,
////   3. `bsdkrun` on `$PATH`,
////   4. an in-repo dev build — `target/release/bsdkrun` then
////      `target/debug/bsdkrun`, searched from the current working directory
////      upwards.
////
//// If nothing matches, `resolve` returns `BinaryNotFound` listing everything
//// it looked at.

import bsdkrun/error.{type Error, BinaryNotFound}
import gleam/list
import gleam/result
import gleam/string

/// Force the SDK to use a specific `bsdkrun` binary, bypassing discovery.
/// Useful in tests, or to run against a locally built binary.
@external(erlang, "bsdkrun_ffi", "set_override")
pub fn set_binary_path(path: String) -> Nil

/// Clear the override set by `set_binary_path`.
@external(erlang, "bsdkrun_ffi", "clear_override")
pub fn reset_binary_path() -> Nil

@external(erlang, "bsdkrun_ffi", "get_override")
fn get_override() -> Result(String, Nil)

@external(erlang, "bsdkrun_ffi", "get_env")
fn get_env(name: String) -> Result(String, Nil)

@external(erlang, "bsdkrun_ffi", "find_executable")
fn find_executable(name: String) -> Result(String, Nil)

@external(erlang, "bsdkrun_ffi", "file_exists")
fn file_exists(path: String) -> Bool

@external(erlang, "bsdkrun_ffi", "cwd")
fn cwd() -> Result(String, Nil)

/// Resolve the path to the `bsdkrun` binary.
pub fn resolve() -> Result(String, Error) {
  let searched = candidates()

  case list.filter_map(searched, usable) {
    [found, ..] -> Ok(found)
    [] -> Error(BinaryNotFound(searched))
  }
}

/// Every location `resolve` will consider, in priority order. Exposed so
/// callers (and error messages) can report exactly what was searched.
pub fn candidates() -> List(String) {
  let dev_builds =
    cwd()
    |> result.map(ancestors)
    |> result.unwrap([])
    |> list.flat_map(fn(dir) {
      [dir <> "/target/release/bsdkrun", dir <> "/target/debug/bsdkrun"]
    })

  [
    get_override() |> result.unwrap(""),
    get_env("BSDKRUN_BIN") |> result.unwrap(""),
    "bsdkrun",
    ..dev_builds
  ]
  |> list.filter(fn(candidate) { candidate != "" })
}

/// A path-like candidate must exist on disk; a bare name must be on `$PATH`.
fn usable(candidate: String) -> Result(String, Nil) {
  case string.contains(candidate, "/") {
    True ->
      case file_exists(candidate) {
        True -> Ok(candidate)
        False -> Error(Nil)
      }
    False -> find_executable(candidate)
  }
}

/// A directory and each of its parents, nearest first: `/a/b/c` yields
/// `["/a/b/c", "/a/b", "/a"]`.
fn ancestors(dir: String) -> List(String) {
  dir
  |> string.split("/")
  |> list.filter(fn(segment) { segment != "" })
  |> list.fold([], fn(acc, segment) {
    let parent = case acc {
      [nearest, ..] -> nearest
      [] -> ""
    }
    [parent <> "/" <> segment, ..acc]
  })
}
