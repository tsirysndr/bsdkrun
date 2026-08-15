//// Cached guest directories.
////
//// Entries are keyed, so a rebuild can pick up where the last one left off:
////
//// ```gleam
//// import bsdkrun/cache
////
//// let assert Ok(hit) = cache.restore("web", "deps-abc", None, ["deps-"])
//// case hit.restored {
////   False -> {
////     let assert Ok(_) = sandbox.exec("web", ["npm", "ci"])
////     cache.save("web", "/app/node_modules", "deps-abc", cache.Gzip, False)
////   }
////   True -> Ok(Nil)
//// }
//// ```
////
//// Where entries live — host disk or S3 — is host configuration, not an SDK
//// concern: set `BSDKRUN_CACHE_BACKEND` / `BSDKRUN_CACHE_S3_*`, or write
//// `~/.config/bsdkrun/cache.toml`.

import bsdkrun/cli
import bsdkrun/error.{type Error}
import bsdkrun/types.{type CacheEntry, type RestoreResult}
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result

/// An archive format a cache entry can be stored in.
pub type Compression {
  Gzip
  Zstd
  Estargz
  Uncompressed
}

fn compression_name(c: Compression) -> String {
  case c {
    Gzip -> "gzip"
    Zstd -> "zstd"
    Estargz -> "estargz"
    Uncompressed -> "none"
  }
}

/// Archive the guest directory at `path` under `key`.
pub fn save(
  id: String,
  path: String,
  key: String,
  compression: Compression,
  force: Bool,
) -> Result(CacheEntry, Error) {
  let flags = case compression {
    Gzip -> []
    other -> ["--compression", compression_name(other)]
  }
  let flags = case force {
    True -> list.append(flags, ["--force"])
    False -> flags
  }
  let args =
    list.append(
      ["cache", "save", id <> ":" <> path, "--key", key, "--json"],
      flags,
    )

  use out <- result.try(cli.checked(args, "bsdkrun cache save", cli.options()))
  types.decode_one(
    out.stdout,
    "bsdkrun cache save",
    types.cache_entry_decoder(),
  )
}

/// Restore a stored tree. `path` defaults to where the entry was saved from;
/// `restore_keys` are prefixes tried in order when `key` misses.
pub fn restore(
  id: String,
  key: String,
  path: Option(String),
  restore_keys: List(String),
) -> Result(RestoreResult, Error) {
  let target = case path {
    Some(p) -> id <> ":" <> p
    None -> id
  }
  let args = ["cache", "restore", target, "--key", key, "--json"]
  let args = case restore_keys {
    [] -> args
    keys -> list.append(list.append(args, ["--restore-keys"]), keys)
  }

  use out <- result.try(cli.checked(
    args,
    "bsdkrun cache restore",
    cli.options(),
  ))
  types.decode_one(
    out.stdout,
    "bsdkrun cache restore",
    types.restore_result_decoder(),
  )
}

/// Every stored cache entry, newest first.
pub fn list() -> Result(List(CacheEntry), Error) {
  use out <- result.try(cli.checked(
    ["cache", "ls", "--json"],
    "bsdkrun cache ls",
    cli.options(),
  ))

  types.decode_rows(out.stdout, "bsdkrun cache ls", types.cache_entry_decoder())
}

/// Remove entries by key, or every one of them with `all`.
pub fn remove(keys: List(String), all: Bool) -> Result(Nil, Error) {
  let args = case all {
    True -> ["cache", "rm", "--all"]
    False -> list.append(["cache", "rm"], keys)
  }
  cli.checked_unit(args, "bsdkrun cache rm", cli.options())
}
