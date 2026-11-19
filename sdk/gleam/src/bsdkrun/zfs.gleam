//// ZFS volumes: one zpool per volume on a disk image, held by one machine
//// at a time (Linux, FreeBSD, NetBSD). Branches are datasets cloned
//// copy-on-write inside the pool (`zfs snapshot` + `zfs clone`), so a branch
//// costs nothing until it diverges.

import bsdkrun/cli
import bsdkrun/error.{type Error}
import bsdkrun/types.{type ZfsBranch, type ZfsVolumeInfo}
import gleam/list
import gleam/option.{type Option, None, Some}
import gleam/result
import gleam/string

fn flag(name: String, value: Option(String)) -> List(String) {
  case value {
    Some(v) -> [name, v]
    None -> []
  }
}

fn line(argv: List(String), label: String) -> Result(String, Error) {
  use out <- result.try(cli.checked(argv, label, cli.options()))
  Ok(string.trim(out.stdout))
}

/// List ZFS volumes, with their branches and holder.
pub fn list() -> Result(List(ZfsVolumeInfo), Error) {
  use out <- result.try(cli.checked(
    ["zfs", "ls", "--json"],
    "bsdkrun zfs ls",
    cli.options(),
  ))
  types.decode_rows(out.stdout, "bsdkrun zfs ls", types.zfs_volume_info_decoder())
}

/// The branches of one volume.
pub fn branches(volume: String) -> Result(List(ZfsBranch), Error) {
  use out <- result.try(cli.checked(
    ["zfs", "ls", volume, "--json"],
    "bsdkrun zfs ls",
    cli.options(),
  ))
  types.decode_rows(out.stdout, "bsdkrun zfs ls", types.zfs_branch_decoder())
}

/// Create a volume: a sparse image that becomes a zpool with a `main` branch
/// the first time a guest imports it. `size` like `"10G"` (the default).
pub fn create(name: String, size: Option(String)) -> Result(String, Error) {
  line(
    list.flatten([["zfs", "create", name], flag("--size", size)]),
    "bsdkrun zfs create",
  )
}

/// Fork a branch copy-on-write from `from` (`main` when `None`). Runs in the
/// machine holding the volume or in a short-lived FreeBSD helper, so it can
/// take as long as a BSD boot. Returns the new `VOL@BRANCH`.
pub fn branch(
  volume: String,
  name: String,
  from: Option(String),
) -> Result(String, Error) {
  line(
    list.flatten([["zfs", "branch", volume, name], flag("--from", from)]),
    "bsdkrun zfs branch",
  )
}

/// Attach `VOL` or `VOL@BRANCH` to a machine at `mount` (`/mnt/VOL`, or
/// `/mnt/VOL-BRANCH`, when `None`). A pool is imported by one machine at a
/// time; a stopped machine picks it up on start, a running holder mounts the
/// branch live. Returns the CLI's one-line report.
pub fn attach(
  target: String,
  machine: String,
  mount: Option(String),
) -> Result(String, Error) {
  line(
    list.flatten([["zfs", "attach", target, machine], flag("--mount", mount)]),
    "bsdkrun zfs attach",
  )
}

/// Detach one branch (`VOL@BRANCH`) or the whole volume (`VOL`) from a machine.
pub fn detach(target: String, machine: String) -> Result(String, Error) {
  line(["zfs", "detach", target, machine], "bsdkrun zfs detach")
}

/// Destroy branches (`VOL@BRANCH`) or whole volumes (`VOL`). `force` destroys
/// a volume a stopped machine still has attached.
pub fn destroy(targets: List(String), force: Bool) -> Result(Nil, Error) {
  let argv =
    list.flatten([
      ["zfs", "destroy"],
      case force {
        True -> ["--force"]
        False -> []
      },
      targets,
    ])
  cli.checked_unit(argv, "bsdkrun zfs destroy", cli.options())
}
