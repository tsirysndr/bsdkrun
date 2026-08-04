//// Host-level operations on persistent volumes.

import bsdkrun/cli
import bsdkrun/error.{type Error}
import bsdkrun/types.{type VolumeInfo}
import gleam/list
import gleam/result

/// List persistent volumes.
pub fn list() -> Result(List(VolumeInfo), Error) {
  use out <- result.try(cli.checked(
    ["volume", "ls", "--json"],
    "bsdkrun volume ls",
    cli.options(),
  ))

  types.decode_rows(
    out.stdout,
    "bsdkrun volume ls",
    types.volume_info_decoder(),
  )
}

/// Remove one or more volumes, and the data they hold.
pub fn remove(names: List(String), force: Bool) -> Result(Nil, Error) {
  let argv =
    list.flatten([
      ["volume", "rm"],
      case force {
        True -> ["--force"]
        False -> []
      },
      names,
    ])

  cli.checked_unit(argv, "bsdkrun volume rm", cli.options())
}
