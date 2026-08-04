//// Global networks — put machines on a shared subnet so they can reach each
//// other by name.
////
//// Names resolve on Linux and FreeBSD via the network's DNS; NetBSD resolves
//// via a synced `/etc/hosts` block. Joins auto-sync, and `sync` refreshes an
//// existing network without restarting its members.

import bsdkrun/cli
import bsdkrun/error.{type Error}
import bsdkrun/sandbox
import bsdkrun/types.{type NetworkInfo, type SandboxInfo}
import gleam/list
import gleam/option.{Some}
import gleam/result

/// List global networks.
pub fn list() -> Result(List(NetworkInfo), Error) {
  use out <- result.try(cli.checked(
    ["network", "ls", "--json"],
    "bsdkrun network ls",
    cli.options(),
  ))

  types.decode_rows(
    out.stdout,
    "bsdkrun network ls",
    types.network_info_decoder(),
  )
}

/// Create a global network.
pub fn create(name: String) -> Result(Nil, Error) {
  cli.checked_unit(
    ["network", "create", name],
    "bsdkrun network create",
    cli.options(),
  )
}

/// Remove one or more networks. `force` removes them even with members
/// attached.
pub fn remove(names: List(String), force: Bool) -> Result(Nil, Error) {
  let argv =
    list.flatten([
      ["network", "rm"],
      case force {
        True -> ["--force"]
        False -> []
      },
      names,
    ])

  cli.checked_unit(argv, "bsdkrun network rm", cli.options())
}

/// Join `machine` to `network`. Applies on the machine's next start.
pub fn connect(machine: String, network: String) -> Result(Nil, Error) {
  cli.checked_unit(
    ["network", "connect", machine, network],
    "bsdkrun network connect",
    cli.options(),
  )
}

/// Detach `machine` from its network. Applies on its next start.
pub fn disconnect(machine: String) -> Result(Nil, Error) {
  cli.checked_unit(
    ["network", "disconnect", machine],
    "bsdkrun network disconnect",
    cli.options(),
  )
}

/// Re-push name resolution to every member of `network`, without restarting
/// them.
pub fn sync(network: String) -> Result(Nil, Error) {
  cli.checked_unit(
    ["network", "sync", network],
    "bsdkrun network sync",
    cli.options(),
  )
}

/// The machines currently attached to `network`, running or stopped.
pub fn members(network: String) -> Result(List(SandboxInfo), Error) {
  use all <- result.try(sandbox.list_all(True))
  Ok(list.filter(all, fn(m) { m.network == Some(network) }))
}
