//// Create options and the argv builder behind `sandbox.create`.
////
//// Ported from the TypeScript SDK's `args.ts`; every path ends with `-d` so
//// `create` yields a detached machine to hold a handle to.
////
//// Options are a `CreateOptions` record built from one of the per-guest
//// constructors (`linux`, `freebsd`, `netbsd`, `firmware`, `kernel`) and
//// refined with the `with_*` helpers:
////
//// ```gleam
//// args.linux("alpine")
//// |> args.with_name("web")
//// |> args.with_cpus(2)
//// |> args.with_ports([args.Port(8080, 80)])
//// ```

import bsdkrun/error.{type Error, InvalidOptions}
import gleam/int
import gleam/list
import gleam/option.{type Option, None, Some}

/// A host-to-guest TCP port forward.
pub type Port {
  Port(host: Int, guest: Int)
}

/// Guest-kind-specific options. One of these sits inside every
/// `CreateOptions`.
pub type Guest {
  /// Run an OCI image as a microVM.
  Linux(
    image: String,
    kernel: Option(String),
    kernel_version: Option(String),
    initramfs: Bool,
    entrypoint: Option(String),
    console: Option(String),
    mounts: List(String),
    command: List(String),
  )
  /// Boot FreeBSD — via EFI on macOS, via PVH direct kernel on Linux/amd64.
  Freebsd(version: Option(String), firmware: Option(String), force: Bool)
  /// Boot NetBSD via direct kernel.
  Netbsd(version: Option(String), force: Bool)
  /// Boot an arbitrary disk through a UEFI firmware image.
  Firmware(firmware: String, disk: String)
  /// Boot an arbitrary kernel directly, with no bootloader.
  Kernel(
    kernel: String,
    format: Option(String),
    initramfs: Option(String),
    cmdline: Option(String),
    disk: Option(String),
  )
  /// Boot a Nanos (NanoVMs) unikernel image. No agent (no exec/shell), but
  /// it does have a root disk, so the shared `persist` option is honored.
  Nanos(
    /// A path, or a bare name in `~/.ops/images` (what `ops build -i` makes).
    image: String,
    /// Nanos kernel override (Linux hosts).
    kernel: Option(String),
    cmdline: Option(String),
  )
  /// Boot a Unikraft unikernel — the application linked into the kernel.
  ///
  /// There is no disk and no userland, so the persist / volume / attach-disk
  /// options are ignored, and the resulting sandbox supports neither `exec`
  /// nor `shell` nor snapshots. Read its output with `logs`.
  Unikraft(
    /// A `kraft` project directory (the image is found under its
    /// `.unikraft/build/`) or a built unikernel image.
    path: String,
    cmdline: Option(String),
    initramfs: Option(String),
    /// Persistent volumes over virtio-fs, each `"HOST:GUEST"` with an
    /// absolute guest path. The one disk-shaped option a unikernel takes: a
    /// share needs neither a disk nor an agent.
    mounts: List(String),
  )

  /// Boot a Solo5 unikernel — MirageOS and anything else built for the `hvt`
  /// target — under the `solo5-hvt` tender rather than libkrun.
  ///
  /// The unikernel declares its own network and block devices in its `MFT1`
  /// manifest note, so only the host-side halves are taken here: the backing
  /// file behind each declared block device, and the arguments handed to the
  /// unikernel itself. Always a single vCPU — a `cpus` above 1 is warned
  /// about and ignored. Like Unikraft there is no disk and no agent: neither
  /// `exec` nor `shell` nor snapshots apply, and `logs` reads its output.
  Solo5(
    /// A `.hvt` binary, or a project directory whose `dist/` holds one
    /// (where `mirage build` leaves it). `"."` for the current directory.
    path: String,
    /// Backing files for declared block devices, each `"NAME=FILE"`. The
    /// `NAME=` may be omitted when the unikernel declares exactly one.
    block: List(String),
    /// Arguments for the unikernel itself, e.g. `"--ipv4=10.0.0.2/24"`.
    args: List(String),
  )

  /// Boot an OSv unikernel image.
  ///
  /// Like the other unikernels there is no agent, so neither `exec` nor
  /// `shell` nor snapshots apply — read its output with `logs`. OSv does have
  /// a root filesystem, so the shared `persist` option is honored.
  Osv(
    /// An aarch64 `loader.img` (a capstan-composed image is both kernel and
    /// filesystem), or on x86_64 the loader ELF, which is kernel only and
    /// needs `disk`.
    image: String,
    /// The application to run and its arguments, e.g. `"/hello.so"`.
    cmdline: Option(String),
    /// Root disk (raw). Required on x86_64.
    disk: Option(String),
    /// Interrupt controller, `"v2"` (default, what OSv v0.57.0 needs) or
    /// `"v3"`. aarch64 only.
    gic: Option(String),
  )
}

/// Everything `sandbox.create` needs: the guest, plus the options every guest
/// kind shares.
pub type CreateOptions {
  CreateOptions(
    guest: Guest,
    name: Option(String),
    cpus: Option(Int),
    mem: Option(Int),
    volume: Option(String),
    persist: Bool,
    attach_disk: List(String),
    no_net: Bool,
    ports: List(Port),
    mac: Option(String),
    network: Option(String),
    log_level: Int,
  )
}

/// Wrap a `Guest` in `CreateOptions` with every shared option at its default.
pub fn new(guest: Guest) -> CreateOptions {
  CreateOptions(
    guest: guest,
    name: None,
    cpus: None,
    mem: None,
    volume: None,
    persist: False,
    attach_disk: [],
    no_net: False,
    ports: [],
    mac: None,
    network: None,
    log_level: 1,
  )
}

// --- per-guest constructors -------------------------------------------------

/// Run the OCI image `image` as a microVM.
pub fn linux(image: String) -> CreateOptions {
  new(
    Linux(
      image: image,
      kernel: None,
      kernel_version: None,
      initramfs: False,
      entrypoint: None,
      console: None,
      mounts: [],
      command: [],
    ),
  )
}

/// Boot a FreeBSD guest.
pub fn freebsd() -> CreateOptions {
  new(Freebsd(version: None, firmware: None, force: False))
}

/// Boot a NetBSD guest.
pub fn netbsd() -> CreateOptions {
  new(Netbsd(version: None, force: False))
}

/// Boot `disk` through the UEFI firmware image `firmware`.
pub fn firmware(firmware: String, disk: String) -> CreateOptions {
  new(Firmware(firmware: firmware, disk: disk))
}

/// Boot `kernel` directly, with no bootloader.
pub fn kernel(kernel: String) -> CreateOptions {
  new(Kernel(
    kernel: kernel,
    format: None,
    initramfs: None,
    cmdline: None,
    disk: None,
  ))
}

/// Boot the Nanos image `image` — a path or a `~/.ops/images` name.
pub fn nanos(image: String) -> CreateOptions {
  new(Nanos(image: image, kernel: None, cmdline: None))
}

/// Boot the Unikraft unikernel at `path` — a `kraft` project directory or a
/// built image. Pass `"."` for the current directory.
pub fn unikraft(path: String) -> CreateOptions {
  new(Unikraft(path: path, cmdline: None, initramfs: None, mounts: []))
}

/// Boot the Solo5 unikernel at `path` — a `.hvt` binary or a project
/// directory whose `dist/` holds one. Pass `"."` for the current directory.
pub fn solo5(path: String) -> CreateOptions {
  new(Solo5(path: path, block: [], args: []))
}

/// Boot the OSv image `image` — a `loader.img`, or on x86_64 a loader ELF
/// paired with `with_disk`.
pub fn osv(image: String) -> CreateOptions {
  new(Osv(image: image, cmdline: None, disk: None, gic: None))
}

// --- shared option setters --------------------------------------------------

/// Name the machine, so it can be addressed as `--name` instead of by id.
pub fn with_name(opts: CreateOptions, name: String) -> CreateOptions {
  CreateOptions(..opts, name: Some(name))
}

/// Set the vCPU count.
pub fn with_cpus(opts: CreateOptions, cpus: Int) -> CreateOptions {
  CreateOptions(..opts, cpus: Some(cpus))
}

/// Set the RAM in MiB.
pub fn with_mem(opts: CreateOptions, mem: Int) -> CreateOptions {
  CreateOptions(..opts, mem: Some(mem))
}

/// Back the machine with a named persistent volume.
pub fn with_volume(opts: CreateOptions, volume: String) -> CreateOptions {
  CreateOptions(..opts, volume: Some(volume))
}

/// Persist the machine's disk across restarts.
pub fn with_persist(opts: CreateOptions, persist: Bool) -> CreateOptions {
  CreateOptions(..opts, persist: persist)
}

/// Attach extra raw disk images.
pub fn with_attach_disk(
  opts: CreateOptions,
  disks: List(String),
) -> CreateOptions {
  CreateOptions(..opts, attach_disk: list.append(opts.attach_disk, disks))
}

/// Boot with networking disabled.
pub fn with_no_net(opts: CreateOptions, no_net: Bool) -> CreateOptions {
  CreateOptions(..opts, no_net: no_net)
}

/// Forward host TCP ports into the guest.
pub fn with_ports(opts: CreateOptions, ports: List(Port)) -> CreateOptions {
  CreateOptions(..opts, ports: list.append(opts.ports, ports))
}

/// Pin the guest's MAC address.
pub fn with_mac(opts: CreateOptions, mac: String) -> CreateOptions {
  CreateOptions(..opts, mac: Some(mac))
}

/// Join a global network at boot, so peers can reach this machine by name.
pub fn with_network(opts: CreateOptions, network: String) -> CreateOptions {
  CreateOptions(..opts, network: Some(network))
}

/// Set the `--log-level` used for the `create` invocation itself.
pub fn with_log_level(opts: CreateOptions, level: Int) -> CreateOptions {
  CreateOptions(..opts, log_level: level)
}

// --- Linux-only setters -----------------------------------------------------

/// Set the command to run in the guest (Linux guests only; ignored elsewhere).
pub fn with_command(
  opts: CreateOptions,
  command: List(String),
) -> CreateOptions {
  case opts.guest {
    Linux(..) as g -> CreateOptions(..opts, guest: Linux(..g, command: command))
    _ -> opts
  }
}

/// Bind-mount host paths into the guest as `HOST:GUEST` (Linux guests only).
pub fn with_mounts(opts: CreateOptions, mounts: List(String)) -> CreateOptions {
  case opts.guest {
    Linux(..) as g ->
      CreateOptions(
        ..opts,
        guest: Linux(..g, mounts: list.append(g.mounts, mounts)),
      )
    _ -> opts
  }
}

/// Override the image's entrypoint (Linux guests only).
pub fn with_entrypoint(
  opts: CreateOptions,
  entrypoint: String,
) -> CreateOptions {
  case opts.guest {
    Linux(..) as g ->
      CreateOptions(..opts, guest: Linux(..g, entrypoint: Some(entrypoint)))
    _ -> opts
  }
}

// --- BSD-only setters -------------------------------------------------------

/// Pin the release to fetch (FreeBSD / NetBSD guests only).
pub fn with_version(opts: CreateOptions, version: String) -> CreateOptions {
  case opts.guest {
    Freebsd(..) as g ->
      CreateOptions(..opts, guest: Freebsd(..g, version: Some(version)))
    Netbsd(..) as g ->
      CreateOptions(..opts, guest: Netbsd(..g, version: Some(version)))
    _ -> opts
  }
}

/// Re-download the image even if it is already cached (FreeBSD / NetBSD only).
pub fn with_force(opts: CreateOptions, force: Bool) -> CreateOptions {
  case opts.guest {
    Freebsd(..) as g -> CreateOptions(..opts, guest: Freebsd(..g, force: force))
    Netbsd(..) as g -> CreateOptions(..opts, guest: Netbsd(..g, force: force))
    _ -> opts
  }
}

// --- the builder ------------------------------------------------------------

/// Build the `create` argv — everything after the binary and its global flags.
pub fn build_create(opts: CreateOptions) -> Result(List(String), Error) {
  use head <- try_guest(opts)
  Ok(
    list.flatten([
      head,
      disk_args(opts),
      net_args(opts),
      name_args(opts),
      vm_args(opts),
      command_args(opts),
    ]),
  )
}

/// The leading, guest-specific portion of the argv.
fn try_guest(
  opts: CreateOptions,
  next: fn(List(String)) -> Result(List(String), Error),
) -> Result(List(String), Error) {
  case opts.guest {
    Linux(image: "", ..) ->
      Error(InvalidOptions("linux guests need a non-empty image"))
    Linux(
      image:,
      kernel:,
      kernel_version:,
      initramfs:,
      entrypoint:,
      console:,
      mounts:,
      command: _,
    ) ->
      next(
        list.flatten([
          ["linux", image, "-d"],
          opt("--kernel", kernel),
          opt("--kernel-version", kernel_version),
          flag("--initramfs", initramfs),
          multi("--mount", mounts),
          opt("--entrypoint", entrypoint),
          opt("--console", console),
        ]),
      )

    Freebsd(version:, firmware:, force:) ->
      next(
        list.flatten([
          ["freebsd", "-d"],
          opt("--version", version),
          opt("--firmware", firmware),
          flag("--force", force),
        ]),
      )

    Netbsd(version:, force:) ->
      next(
        list.flatten([
          ["netbsd", "-d"],
          opt("--version", version),
          flag("--force", force),
        ]),
      )

    Firmware(firmware: "", ..) ->
      Error(InvalidOptions("firmware guests need a non-empty firmware path"))
    Firmware(disk: "", ..) ->
      Error(InvalidOptions("firmware guests need a non-empty disk path"))
    Firmware(firmware:, disk:) ->
      next(["firmware", "--firmware", firmware, "--disk", disk, "-d"])

    Kernel(kernel: "", ..) ->
      Error(InvalidOptions("kernel guests need a non-empty kernel path"))
    Kernel(kernel:, format:, initramfs:, cmdline:, disk:) ->
      next(
        list.flatten([
          ["kernel", "--kernel", kernel, "-d"],
          opt("--format", format),
          opt("--initramfs", initramfs),
          opt("--cmdline", cmdline),
          opt("--disk", disk),
        ]),
      )

    Nanos(image: "", ..) ->
      Error(InvalidOptions("nanos guests need a non-empty image"))
    Nanos(image:, kernel:, cmdline:) ->
      next(
        list.flatten([
          ["nanos", "-d"],
          opt("--kernel", kernel),
          opt("--cmdline", cmdline),
          flag("--persist", opts.persist),
          [image],
        ]),
      )

    Unikraft(path: "", ..) ->
      Error(InvalidOptions(
        "unikraft guests need a non-empty path (\".\" for the current directory)",
      ))
    Unikraft(path:, cmdline:, initramfs:, mounts:) ->
      next(
        list.flatten([
          ["unikraft", "-d"],
          opt("--cmdline", cmdline),
          opt("--initramfs", initramfs),
          multi("--mount", mounts),
          [path],
        ]),
      )

    Solo5(path: "", ..) ->
      Error(InvalidOptions(
        "solo5 guests need a non-empty path (\".\" for the current directory)",
      ))
    Solo5(path:, block:, args: _) ->
      next(
        list.flatten([
          ["solo5", "-d"],
          multi("--block", block),
          [path],
        ]),
      )

    Osv(image: "", ..) ->
      Error(InvalidOptions("osv guests need a non-empty image"))
    Osv(image:, cmdline:, disk:, gic:) ->
      next(
        list.flatten([
          ["osv", "-d"],
          opt("--cmdline", cmdline),
          opt("--disk", disk),
          opt("--gic", gic),
          flag("--persist", opts.persist),
          [image],
        ]),
      )
  }
}

/// Linux takes `-v` inline with the guest args; the disk-booting kinds take
/// the full persist / volume / attach-disk set.
fn disk_args(opts: CreateOptions) -> List(String) {
  case opts.guest {
    Linux(..) ->
      list.flatten([
        opt("-v", opts.volume),
        multi("--attach-disk", opts.attach_disk),
      ])
    // A unikernel has no disk, so there is nothing to persist or attach.
    Unikraft(..) -> []
    // Solo5 block devices are declared by the unikernel and backed via
    // `--block` in the guest head; the shared disk options do not apply.
    Solo5(..) -> []
    // Nanos: persist is emitted with the guest args; no volume/attach-disk.
    Nanos(..) -> []
    // OSv: same as Nanos — persist rides with the guest args.
    Osv(..) -> []
    _ ->
      list.flatten([
        flag("--persist", opts.persist),
        opt("-v", opts.volume),
        multi("--attach-disk", opts.attach_disk),
      ])
  }
}

fn net_args(opts: CreateOptions) -> List(String) {
  list.flatten([
    flag("--no-net", opts.no_net),
    list.flat_map(opts.ports, fn(p) {
      ["--port", int.to_string(p.host) <> ":" <> int.to_string(p.guest)]
    }),
    opt("--mac", opts.mac),
    opt("--network", opts.network),
  ])
}

fn name_args(opts: CreateOptions) -> List(String) {
  opt("--name", opts.name)
}

fn vm_args(opts: CreateOptions) -> List(String) {
  list.flatten([
    opt("--cpus", option.map(opts.cpus, int.to_string)),
    opt("--mem", option.map(opts.mem, int.to_string)),
  ])
}

/// A trailing `-- cmd args…` — Linux's guest command, or Solo5's unikernel
/// arguments. Solo5 needs the separator most: MirageOS options look exactly
/// like bsdkrun's own flags (e.g. `--ipv4=…`), and only `--` stops the CLI
/// from eating them.
fn command_args(opts: CreateOptions) -> List(String) {
  case opts.guest {
    Linux(command: [], ..) -> []
    Linux(command: cmd, ..) -> ["--", ..cmd]
    Solo5(args: [], ..) -> []
    Solo5(args: args, ..) -> ["--", ..args]
    _ -> []
  }
}

// --- fragment helpers -------------------------------------------------------

fn opt(flag: String, value: Option(String)) -> List(String) {
  case value {
    Some(v) -> [flag, v]
    None -> []
  }
}

fn flag(name: String, enabled: Bool) -> List(String) {
  case enabled {
    True -> [name]
    False -> []
  }
}

fn multi(flag: String, values: List(String)) -> List(String) {
  list.flat_map(values, fn(v) { [flag, v] })
}
