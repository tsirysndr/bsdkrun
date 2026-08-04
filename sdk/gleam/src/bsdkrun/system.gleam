//// Host-level toolchain and image operations.

import bsdkrun/cli
import bsdkrun/error.{type Error}
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result
import gleam/string

/// Which BSD an image operation targets.
pub type Bsd {
  Freebsd
  Netbsd
}

fn bsd_name(os: Bsd) -> String {
  case os {
    Freebsd -> "freebsd"
    Netbsd -> "netbsd"
  }
}

/// Sanity-check the toolchain: verify libkrun links and that a context can be
/// created and configured. Does not boot anything.
pub fn probe() -> Bool {
  case cli.run(["probe"], cli.options()) {
    Ok(out) -> out.exit_code == 0
    Error(_) -> False
  }
}

/// Download and prepare a BSD image ahead of time, returning the CLI's output.
pub fn fetch_image(
  os: Bsd,
  version: Option(String),
  dir: Option(String),
  force: Bool,
) -> Result(String, Error) {
  let argv =
    list.flatten([
      ["fetch", "--os", bsd_name(os)],
      opt("--version", version),
      opt("--dir", dir),
      case force {
        True -> ["--force"]
        False -> []
      },
    ])

  cli.checked(argv, "bsdkrun fetch", cli.options())
  |> result.map(fn(out) { out.stdout })
}

/// The builds available to fetch for a BSD — the CLI's non-empty output lines.
pub fn versions(os: Bsd) -> Result(List(String), Error) {
  use out <- result.try(cli.checked(
    ["versions", "--os", bsd_name(os)],
    "bsdkrun versions",
    cli.options(),
  ))

  Ok(
    out.stdout
    |> string.split("\n")
    |> list.filter(fn(line) { line != "" }),
  )
}

/// Grow a raw disk image. The guest expands its root filesystem on next boot.
pub fn grow_disk(disk: String, size: String) -> Result(Nil, Error) {
  cli.checked_unit(
    ["grow", "--disk", disk, "--size", size],
    "bsdkrun grow",
    cli.options(),
  )
}

fn opt(flag: String, value: Option(String)) -> List(String) {
  case value {
    Some(v) -> [flag, v]
    None -> []
  }
}
