//! Booting machines: every guest family, plus the machinery they share —
//! detaching, wiring the console, waiting for the guest agent, and running a
//! trailing command against a freshly booted guest.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::cli::*;
use crate::krun::Ctx;
use crate::net::{Gvproxy, PortForward};
use crate::{
    agent, ai, console, db, docker, fetch, flavors, host, id, krun, linux, names, nanos, net,
    network, oci, osv, tty, unikraft, watchdog,
};

use super::flavor::{
    flavor_build_key, flavor_build_volume, flavor_provision_argv, resolve_linux_flavor,
    LinuxFlavorSpec,
};
use super::guest::{
    agent_error, agent_target, guest_os_kind, interactive_shell_argv, interactive_shell_env,
};
use super::{
    basename, load_attached_disks, load_env, load_mounts, machine_dir_or_tmp, machine_rootfs_dir,
    save_attached_disks, save_env, save_mounts, volume_dir,
};

/// Attach any `--attach-disk` images after the root disk. Block ids are
/// `data0`, `data1`, … — libkrun only requires them to be unique.
pub(crate) fn attach_extra_disks(ctx: &Ctx, disks: &[DiskSpec]) -> Result<()> {
    for (i, disk) in disks.iter().enumerate() {
        let block_id = format!("data{i}");
        db::record_disk(&disk.path.to_string_lossy());
        ctx.add_disk(&block_id, &disk.path, disk.read_only)
            .with_context(|| format!("attaching disk {}", disk.path.display()))?;
        info!(
            id = %block_id,
            path = %disk.path.display(),
            read_only = disk.read_only,
            "attached disk"
        );
    }
    Ok(())
}

/// Bring up user-mode networking (on by default). Returns the live gvproxy
/// handle, which must outlive the VM (kept in scope until after `start_enter`).
///
/// If gvproxy isn't installed we degrade gracefully — the guest boots without a
/// NIC and we warn — *unless* the user explicitly asked for port forwards, in
/// which case the missing dependency is a hard error.
/// Bring up gvproxy networking. When `agent_dir` is given and networking is
/// available, also allocate a host port forwarded to the guest agent's TCP port
/// and record it under that machine's state dir (so `exec`/`shell` can reach it).
pub(crate) fn setup_networking_with_agent(
    ctx: &Ctx,
    cfg: &NetConfig,
    agent_dir: Option<&std::path::Path>,
) -> Result<Option<Gvproxy>> {
    if cfg.no_net {
        if !cfg.ports.is_empty() {
            anyhow::bail!("--port cannot be combined with --no-net");
        }
        info!("networking disabled (--no-net)");
        return Ok(None);
    }

    // Probe for gvproxy up front so a missing dependency degrades cleanly.
    if let Err(e) = net::locate() {
        if !cfg.ports.is_empty() {
            return Err(e).context("--port requires gvproxy");
        }
        tracing::warn!("networking disabled: {e:#}");
        return Ok(None);
    }

    let mac = match &cfg.mac {
        Some(s) => net::parse_mac(s).context("parsing --mac")?,
        None => net::DEFAULT_MAC,
    };

    // Shared/global network: if `BSDKRUN_NET_CONTROL` points at a running network
    // gvproxy, join THAT switch (via a per-member /connect bridge) instead of
    // spawning an isolated gvproxy — so members share a subnet and reach each
    // other by IP/name. Each member gets its own IP (`BSDKRUN_NET_IP`).
    if let Some(control) = std::env::var("BSDKRUN_NET_CONTROL")
        .ok()
        .filter(|s| !s.is_empty())
    {
        let control = std::path::PathBuf::from(control);
        // A static (Linux) member knows its IP now; a DHCP (BSD) member gets it
        // after boot — its agent/port forwards + DNS are wired then.
        let ip = std::env::var("BSDKRUN_NET_IP")
            .ok()
            .filter(|s| !s.is_empty());
        // A per-member vfkit socket for the bridge (libkrun connects to it).
        let vfkit = match agent_dir {
            Some(dir) => dir.join("net-bridge.sock"),
            None => std::env::temp_dir().join(format!("bsdkrun-netbr-{}.sock", std::process::id())),
        };
        if let Some(ip) = &ip {
            if let Some(dir) = agent_dir {
                let host =
                    net::free_local_port().context("reserving a host port for the exec agent")?;
                net::expose_on_control(
                    &control,
                    std::net::Ipv4Addr::LOCALHOST.into(),
                    host,
                    ip,
                    agent::GUEST_PORT,
                )
                .context("forwarding the agent port on the shared network")?;
                let _ = std::fs::write(agent::port_file(dir), host.to_string());
                info!(agent_port = host, %ip, "exec agent reachable via the shared network");
            }
            for pf in &cfg.ports {
                net::expose_on_control(&control, pf.bind, pf.host, ip, pf.guest)
                    .with_context(|| format!("forwarding host port {}", pf.host))?;
            }
        }
        // CRITICAL: every member on the shared switch needs a DISTINCT MAC (a
        // shared MAC makes gvproxy's CAM table route both members' traffic to one
        // port, breaking connectivity). `network::join` sets `BSDKRUN_NET_MAC`;
        // fall back to deriving it from the IP's last octet.
        let member_mac = if let Some(s) = std::env::var("BSDKRUN_NET_MAC")
            .ok()
            .filter(|s| !s.is_empty())
        {
            net::parse_mac(&s).unwrap_or(mac)
        } else if let Some(ip) = &ip {
            let last = ip
                .rsplit('.')
                .next()
                .and_then(|o| o.parse::<u8>().ok())
                .unwrap_or(2);
            [0x5a, 0x94, 0xef, 0xe4, 0x0c, last]
        } else {
            mac
        };
        net::start_network_bridge(&vfkit, &control).context("bridging into the shared network")?;
        ctx.add_net_gvproxy(&vfkit, member_mac)
            .context("attaching virtio-net to the shared network")?;
        info!("joined shared network");
        return Ok(None); // the network gvproxy is shared — we don't own it
    }

    // Forward a unique host port to the guest agent (for `exec`/`shell`) and
    // persist it, alongside any user-requested `--port` forwards.
    let mut ports = cfg.ports.clone();
    let agent_port = match agent_dir {
        Some(dir) => {
            let host =
                net::free_local_port().context("reserving a host port for the exec agent")?;
            ports.push(PortForward::loopback(host, agent::GUEST_PORT));
            let _ = std::fs::write(agent::port_file(dir), host.to_string());
            Some(host)
        }
        None => None,
    };

    let gvproxy = Gvproxy::spawn(&ports).context("starting gvproxy networking")?;
    ctx.add_net_gvproxy(&gvproxy.vfkit_socket, mac)
        .context("attaching virtio-net device")?;
    // Write the control socket's path into the machine's state dir: adding a
    // forward to a machine that is already running (what `docker` does for
    // every published container port) has to find it from the outside.
    if let Some(dir) = agent_dir {
        net::record_control_socket(dir, &gvproxy.control_socket);
    }
    if let Some(p) = agent_port {
        info!(agent_port = p, "exec agent reachable via forwarded port");
    }
    Ok(Some(gvproxy))
}

pub(crate) fn boot_kernel(args: KernelArgs) -> Result<()> {
    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    // CoW-clone the root disk per machine (unless --persist / --volume) so many
    // machines can boot the same base image concurrently without touching it.
    let volume = args.run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = match &args.disk {
        Some(d) => Some(prepare_bsd_disk(
            d,
            &vdir,
            args.run.persist,
            volume.as_deref(),
            None,
        )?),
        None => None,
    };
    let image = args
        .disk
        .as_deref()
        .map(basename)
        .unwrap_or_else(|| basename(&args.kernel));

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        if let Some(d) = &root_disk {
            if let Some(orig) = &args.disk {
                db::record_disk(&orig.to_string_lossy());
            }
            ctx.add_disk("root", d, false)
                .with_context(|| format!("attaching root disk {}", d.display()))?;
        }
        attach_extra_disks(&ctx, &args.attach_disk)?;
        // Forward a host port to the guest agent so a user-installed bsdkrun-agent
        // in the guest can serve `exec`/`shell` (idle until the guest runs it).
        let gvproxy = setup_networking_with_agent(&ctx, &args.net, Some(&vdir))?;
        ctx.set_kernel(
            &args.kernel,
            args.format.to_krun(),
            args.initramfs.as_deref(),
            &args.cmdline,
        )
        .context("configuring kernel")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.run.volume.as_deref(), &volume) {
        db::record_volume(name, "kernel", &image, &dir.to_string_lossy());
    }
    run_machine(
        &machine_id,
        &vdir,
        "kernel",
        &image,
        "",
        args.vm.cpus,
        args.vm.mem,
        args.run.detach,
        true, // BSD: use the SMP-shutdown watchdog on the foreground path
        args.run.volume.as_deref(),
        &args.net.ports,
        &[],
        false,
        false,
        build,
    )
}

/// Boot a Unikraft unikernel. See [`crate::unikraft`] for why the image needs a
/// host-side shim before libkrun's aarch64 loader will enter it.
pub(crate) fn boot_unikraft(args: UnikraftArgs) -> Result<()> {
    // A reference rather than a path means the unikernel comes from a
    // registry. It is fetched on first use and cached, so booting the same
    // reference again costs nothing — the same shape as `docker run`.
    //
    // The registry also carries the argv, which a kernel does not record:
    // an explicit --cmdline still wins, since someone passing one is
    // overriding on purpose.
    let (kernel, cmdline) = resolve_unikraft_source(&args)?;
    let volumes = args
        .mount
        .iter()
        .map(|m| unikraft::parse_volume(m))
        .collect::<Result<Vec<_>>>()?;
    boot_unikraft_image(
        &kernel,
        &cmdline,
        args.initramfs.as_deref(),
        args.detach,
        args.net,
        args.vm,
        &volumes,
    )
}

/// Resolve what to boot: a local path, or an OCI reference pulled into the
/// cache. Returns the kernel and the command line to boot it with.
fn resolve_unikraft_source(args: &UnikraftArgs) -> Result<(std::path::PathBuf, String)> {
    let spec = args.path.to_string_lossy();

    #[cfg(feature = "pack")]
    if crate::commands::pack::is_reference(&spec) {
        let pulled = crate::commands::pack::pull_unikernel(&spec)?;
        let cmdline = if args.cmdline.is_empty() {
            pulled.cmdline.clone()
        } else {
            args.cmdline.clone()
        };
        return Ok((pulled.kernel, cmdline));
    }

    // Without pack support there is no registry client to fetch with, and
    // failing here with the real reason beats `<ref> does not exist`.
    #[cfg(not(feature = "pack"))]
    if spec.contains(['/', ':', '@']) && !args.path.exists() {
        anyhow::bail!(
            "{spec} is not a path, and this bsdkrun binary was built without pack support, \
             so it cannot pull from a registry"
        );
    }

    Ok((unikraft::resolve(&args.path)?, args.cmdline.clone()))
}

/// Shared by `unikraft` and `start` (which re-boots a machine's saved image).
pub(crate) fn boot_unikraft_image(
    kernel: &std::path::Path,
    cmdline: &str,
    initramfs: Option<&std::path::Path>,
    detach: bool,
    net: NetConfig,
    vm: VmConfig,
    volumes: &[unikraft::Volume],
) -> Result<()> {
    let prepared = unikraft::prepare(kernel)?;
    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(kernel);
    // Save the cmdline as given; the fstab fragment is derived from `volumes`
    // on every boot, so a restart re-generates it instead of stacking a second
    // copy on top of the saved one.
    let spec = unikraft::BootSpec {
        kernel: kernel.to_path_buf(),
        cmdline: cmdline.to_string(),
        initramfs: initramfs.map(|p| p.to_path_buf()),
        volumes: volumes.to_vec(),
    };
    // Volumes are mounted by the guest from its command line, so the mount
    // table has to be prepended before the cmdline is handed to libkrun.
    let cmdline = unikraft::build_cmdline(cmdline, volumes, &image);

    // libkrun appends an `earlycon=` hint to the end of the command line, which
    // is *after* the `--` stop sequence — so it is not a kernel parameter at
    // all, it is the last word of the application's argv. Unikraft would ignore
    // it either way: its console is chosen at build time by kconfig
    // (CONFIG_LIBPL011) and probed from the device tree, never from the cmdline.
    //
    // Applications are less forgiving. node and bun quietly carry the extra
    // argument, but a program that validates its own argv stops dead on a
    // directive nobody wrote:
    //
    //   FATAL:  postgres: invalid command-line argument:
    //           earlycon=pl011,mmio32,0x0a001000
    //
    // `bsdkrun osv` drops it for the same reason (see `boot_osv`). The console
    // is unaffected; only the hint goes away.
    std::env::set_var("KRUN_NO_EARLYCON", "1");

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(vm.cpus, vm.mem)?;
        // Unikraft's arm64 console is a PL011, which is libkrun's explicit
        // serial — not the implicit virtio-console the Linux path uses.
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        // No agent lives in a unikernel; this just gives it a NIC (which it
        // uses only if it was built with a virtio-net driver).
        let gvproxy = setup_networking_with_agent(&ctx, &net, Some(&vdir))?;
        // Each volume is a virtio-fs share; the guest finds it by tag, which
        // is what the vfs.fstab entry above names as the source device.
        for (i, v) in volumes.iter().enumerate() {
            ctx.add_virtiofs(&unikraft::volume_tag(i), &v.host)
                .with_context(|| format!("sharing --mount host dir {}", v.host.display()))?;
        }
        ctx.set_kernel(&prepared.path, prepared.format, initramfs, &cmdline)
            .context("configuring unikernel")?;
        spec.save(&vdir)?;
        Ok((ctx, gvproxy))
    };

    run_machine(
        &machine_id,
        &vdir,
        "unikraft",
        &image,
        "",
        vm.cpus,
        vm.mem,
        detach,
        // A unikernel that returns from main powers the VM off through PSCI,
        // the same path the watchdog covers for a BSD guest.
        true,
        None,
        &net.ports,
        &[],
        false,
        false,
        build,
    )
}

/// Boot an OSv unikernel. See [`crate::osv`] for the image layout, why this
/// can't go through `bsdkrun kernel`, and why the GIC version matters.
pub(crate) fn boot_osv(args: OsvArgs) -> Result<()> {
    let image = osv::resolve_image(&args.image.to_string_lossy())?;
    boot_osv_image(
        &image,
        &args.cmdline,
        args.gic,
        args.disk.as_deref(),
        args.no_disk,
        args.persist,
        args.volume.as_deref(),
        &args.attach_disk,
        args.detach,
        args.net,
        args.vm,
        None,
    )
}

/// Shared by `osv` and `start` (which passes the machine's own cloned disk as
/// `disk_override` so filesystem writes survive the restart).
#[allow(clippy::too_many_arguments)]
pub(crate) fn boot_osv_image(
    image: &std::path::Path,
    cmdline: &str,
    gic: osv::Gic,
    disk: Option<&std::path::Path>,
    no_disk: bool,
    persist: bool,
    volume: Option<&str>,
    attach_disk: &[DiskSpec],
    detach: bool,
    net: NetConfig,
    vm: VmConfig,
    disk_override: Option<PathBuf>,
) -> Result<()> {
    // How the image boots is decided by what it is, not by the host arch — but
    // the two must agree, since a hardware-virtualized guest runs the host's
    // architecture.
    let kind = osv::probe(image)?;
    let host_arch = host::Arch::current()?;
    match (&kind, host_arch) {
        (osv::Image::Arm64 { .. }, host::Arch::Aarch64) => {}
        (osv::Image::ElfPvh, host::Arch::X86_64) => {}
        (osv::Image::Arm64 { .. }, _) => anyhow::bail!(
            "this is an OSv aarch64 image, but the host is x86_64 — a guest runs \
             the host's architecture, so use the x86_64 loader ELF instead"
        ),
        (osv::Image::ElfPvh, _) => anyhow::bail!(
            "this is an OSv x86_64 image, but the host is aarch64 — use the \
             aarch64 loader.img (e.g. osv-loader-microvm.qemu.aarch64) instead"
        ),
    }

    // Everything libkrun is configured through by environment variable is set
    // HERE, before anything below spawns a thread.
    //
    // setenv(3) is not thread-safe: it can reallocate the environment block
    // while another thread is walking it, and POSIX gives no way to make that
    // safe from the caller's side (Rust 2024 marks `set_var` unsafe for exactly
    // this reason). Once `prepare_network` below starts gvproxy's supervision
    // threads, a `set_var` is a data race that segfaults the process before the
    // VM ever boots — intermittently, and never under a debugger.
    //
    //   * KRUN_NO_RNG — OSv's virtio-rng driver is PCI-only (it registers
    //     itself but never fills in the MMIO interrupt factory), so probing the
    //     device throws std::bad_function_call and aborts the guest partway
    //     through driver init.
    //   * KRUN_NO_BALLOON — OSv has no balloon driver at all, and being first
    //     on the bus it owns the first SPI, so every interrupt it raises is
    //     reported on the console as "unhandled InterruptID irq=0x20".
    //   * KRUN_GIC — see the module docs: the released aarch64 kernel is
    //     GICv2-only. Harmless on x86_64, which has no GIC.
    //   * KRUN_PVH — enter an x86_64 loader ELF at its PHYS32_ENTRY note rather
    //     than via e_entry and the Linux zero page, which would triple-fault.
    //
    // Neither rng nor balloon does anything for an OSv guest, so both are left
    // off the bus.
    std::env::set_var("KRUN_NO_RNG", "1");
    std::env::set_var("KRUN_NO_BALLOON", "1");
    // OSv hands everything after the application path to the application as
    // argv, so libkrun's `earlycon=` hint arrives as a stray argument. A
    // program that parses its own arguments then fails on a directive nobody
    // wrote — redis-server, for one, aborts reading its config. The console is
    // unaffected; only the cmdline hint goes away.
    std::env::set_var("KRUN_NO_EARLYCON", "1");
    std::env::set_var("KRUN_GIC", gic.krun_value());
    if matches!(kind, osv::Image::ElfPvh) {
        std::env::set_var("KRUN_PVH", "1");
    }

    // OSv DHCPs its IP like the BSDs do.
    let joined = prepare_network(&net, true)?;
    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image_name = basename(image);
    std::fs::create_dir_all(&vdir)
        .with_context(|| format!("creating machine dir {}", vdir.display()))?;

    // Per-arch: what to hand libkrun as the kernel, in which format, and
    // whether the image doubles as the root disk.
    let (kernel, kernel_format, root_src, cmdline) = match &kind {
        osv::Image::Arm64 { header } => {
            // libkrun reads the whole kernel file into guest RAM, so a composed
            // image has to be split: the leading arm64 Image becomes the
            // kernel, and the image itself is the root disk.
            let kernel = osv::extract_kernel(image, header, &vdir.join("kernel.img"))?;
            let root = if osv::has_filesystem(image, header)? {
                Some(image.to_path_buf())
            } else {
                None
            };
            (
                kernel,
                krun::KRUN_KERNEL_FORMAT_RAW,
                root,
                osv::effective_cmdline(cmdline, Some(header)),
            )
        }
        osv::Image::ElfPvh => {
            (
                image.to_path_buf(),
                krun::KRUN_KERNEL_FORMAT_ELF,
                // The ELF is kernel only: a root filesystem must be attached
                // explicitly with --disk.
                None,
                osv::effective_cmdline(cmdline, None),
            )
        }
    };

    let root_src = disk.map(|d| d.to_path_buf()).or(root_src);
    let attach_root = !no_disk && root_src.is_some();
    if !attach_root && !cmdline.contains("--nomount") {
        warn!(
            image = %image_name,
            "no root filesystem attached, so OSv has nothing to mount — pass \
             --disk, boot an image composed by capstan, or add --nomount to the \
             command line"
        );
    }
    let root_disk = if attach_root {
        let voldir = volume.map(volume_dir).transpose()?;
        let disk_src = disk_override.unwrap_or_else(|| root_src.unwrap());
        Some(prepare_bsd_disk(
            &disk_src,
            &vdir,
            persist,
            voldir.as_deref(),
            None,
        )?)
    } else {
        None
    };

    let spec = osv::BootSpec {
        image: image.to_path_buf(),
        cmdline: cmdline.clone(),
        gic,
    };

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(vm.cpus, vm.mem)?;
        // OSv's aarch64 console is a PL011 — libkrun's explicit serial, not the
        // implicit virtio-console the Linux path uses. Without this the guest
        // boots correctly but writes into the void.
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        if let Some(disk) = &root_disk {
            db::record_disk(&image.to_string_lossy());
            ctx.add_disk("root", disk, false)
                .with_context(|| format!("attaching root disk {}", disk.display()))?;
        }
        attach_extra_disks(&ctx, attach_disk)?;
        // No agent lives in a unikernel; this just gives it a NIC (which it
        // uses only if the kernel was built with a virtio-net driver).
        let gvproxy = setup_networking_with_agent(&ctx, &net, Some(&vdir))?;
        ctx.set_kernel(&kernel, kernel_format, None, &cmdline)
            .context("configuring the OSv kernel")?;
        spec.save(&vdir)?;
        Ok((ctx, gvproxy))
    };

    let result = run_machine(
        &machine_id,
        &vdir,
        "osv",
        &image_name,
        "",
        vm.cpus,
        vm.mem,
        detach,
        // OSv powers the VM off through PSCI when its application returns, the
        // same path the watchdog covers for a BSD guest.
        true,
        volume,
        &net.ports,
        &[],
        false,
        false,
        build,
    );
    drop(joined);
    result
}

/// Boot a Nanos image. See [`crate::nanos`] for the per-host boot method and
/// the current support status of each.
pub(crate) fn boot_nanos(args: NanosArgs) -> Result<()> {
    let image = nanos::resolve_image(&args.image)?;
    boot_nanos_image(
        &image,
        args.kernel.as_deref(),
        &args.cmdline,
        args.firmware.as_deref(),
        args.detach,
        args.persist,
        args.net,
        args.vm,
        None,
    )
}

/// Shared by `nanos` and `start` (which passes the machine's own cloned disk
/// as `disk_override` so runtime state survives the restart).
#[allow(clippy::too_many_arguments)]
pub(crate) fn boot_nanos_image(
    image: &std::path::Path,
    kernel: Option<&std::path::Path>,
    cmdline: &str,
    firmware: Option<&std::path::Path>,
    detach: bool,
    persist: bool,
    net: NetConfig,
    vm: VmConfig,
    disk_override: Option<PathBuf>,
) -> Result<()> {
    let arch = host::Arch::current()?;
    // Nanos DHCPs its IP like the BSDs do.
    let joined = prepare_network(&net, true)?;
    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let display = basename(image);

    // Per-host boot method. Resolve everything fallible before the closure.
    #[cfg(target_os = "macos")]
    let (kernel_path, firmware_path) = {
        if !matches!(arch, host::Arch::Aarch64) {
            anyhow::bail!("nanos on macOS is arm64-only");
        }
        let fw = match firmware {
            Some(f) => f.to_path_buf(),
            None => locate_krun_efi()?,
        };
        // libkrun serves the ACPI tables Nanos requires (fork feature; a
        // libkrun without it simply ignores the variable).
        std::env::set_var("KRUN_ACPI", "1");
        warn!(
            "Nanos/arm64 needs patched artifacts: the stock 0.1.55 kernel and \
             bootloader die before userspace on libkrun (loader cache \
             maintenance, GIC selection and virtio-mmio IRQ routing — all \
             fixed on the nanos fork's fix/aarch64-libkrun-boot branch). If \
             this boot hangs silently, stage the patched kernel.img and \
             bootaa64.efi into ~/.ops/<version>-arm/ and rebuild the image — \
             see examples/nanos-hello/README.md."
        );
        (kernel.map(|k| k.to_path_buf()), fw)
    };
    #[cfg(target_os = "linux")]
    let kernel_path = {
        if !matches!(arch, host::Arch::X86_64) {
            anyhow::bail!(
                "nanos on Linux is x86_64-only: the arm64 Nanos kernel links at \
                 0x40400000, below libkrun's direct-kernel RAM base"
            );
        }
        let _ = firmware; // EFI is the macOS path
        Some(match kernel {
            Some(k) => k.to_path_buf(),
            None => nanos::default_kernel()?,
        })
    };

    let disk_src = disk_override.unwrap_or_else(|| image.to_path_buf());
    let root_disk = prepare_bsd_disk(&disk_src, &vdir, persist, None, None)?;

    let spec = nanos::BootSpec {
        image: image.to_path_buf(),
        #[cfg(target_os = "macos")]
        kernel: kernel_path.clone(),
        #[cfg(target_os = "linux")]
        kernel: kernel_path.clone(),
        cmdline: cmdline.to_string(),
    };

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(vm.cpus, vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        db::record_disk(&image.to_string_lossy());
        ctx.add_disk("root", &root_disk, false)
            .with_context(|| format!("attaching root disk {}", root_disk.display()))?;
        // No agent lives in a unikernel; this just gives it a NIC.
        let gvproxy = setup_networking_with_agent(&ctx, &net, Some(&vdir))?;
        #[cfg(target_os = "macos")]
        ctx.set_firmware(&firmware_path)
            .context("configuring EFI firmware")?;
        #[cfg(target_os = "linux")]
        if let Some(k) = &kernel_path {
            ctx.set_kernel(k, krun::KRUN_KERNEL_FORMAT_ELF, None, cmdline)
                .context("configuring the Nanos kernel")?;
        }
        spec.save(&vdir)?;
        Ok((ctx, gvproxy))
    };

    let result = run_machine(
        &machine_id,
        &vdir,
        "nanos",
        &display,
        "",
        vm.cpus,
        vm.mem,
        detach,
        true,
        None,
        &net.ports,
        &[],
        false,
        false,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &net.ports, &joined);
    result
}

pub(crate) fn boot_firmware(args: FirmwareArgs) -> Result<()> {
    firmware_machine(
        &args.firmware,
        &args.disk,
        &args.attach_disk,
        &args.run,
        &args.net,
        &args.vm,
        &[],
        false,
        false,
        None,
    )
}

/// Boot a machine via UEFI firmware + a root disk (shared by `firmware` and the
/// `freebsd`/`netbsd` shortcuts). The root disk is CoW-cloned per machine.
#[allow(clippy::too_many_arguments)]
pub(crate) fn firmware_machine(
    firmware: &std::path::Path,
    disk: &std::path::Path,
    attach: &[DiskSpec],
    run: &RunConfig,
    net: &NetConfig,
    vm: &VmConfig,
    exec_after: &[String],
    interactive: bool,
    verbose: bool,
    disk_size: Option<&str>,
) -> Result<()> {
    ensure_net_for_exec(net, exec_after)?;
    // BSD guests DHCP their IP (dhcp = true), so join before the ctx build.
    let joined = prepare_network(net, true)?;
    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(disk);
    let volume = run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = prepare_bsd_disk(disk, &vdir, run.persist, volume.as_deref(), disk_size)?;

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(vm.cpus, vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        db::record_disk(&disk.to_string_lossy());
        ctx.add_disk("root", &root_disk, false)
            .with_context(|| format!("attaching root disk {}", root_disk.display()))?;
        attach_extra_disks(&ctx, attach)?;
        // Forward a host port to the guest agent so a user-installed bsdkrun-agent
        // in the guest can serve `exec`/`shell` (idle until the guest runs it).
        let gvproxy = setup_networking_with_agent(&ctx, net, Some(&vdir))?;
        ctx.set_firmware(firmware).context("configuring firmware")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (run.volume.as_deref(), &volume) {
        db::record_volume(name, "firmware", &image, &dir.to_string_lossy());
    }
    save_attached_disks(&vdir, attach);
    let result = run_machine(
        &machine_id,
        &vdir,
        "firmware",
        &image,
        &exec_after.join(" "),
        vm.cpus,
        vm.mem,
        run.detach,
        true,
        run.volume.as_deref(),
        &net.ports,
        exec_after,
        interactive,
        verbose,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &net.ports, &joined);
    result
}

/// A trailing command runs in the guest via its agent, which rides the guest
/// NIC — so it's incompatible with `--no-net`. Fail early with a clear message.
pub(crate) fn ensure_net_for_exec(net: &NetConfig, exec_after: &[String]) -> Result<()> {
    if !exec_after.is_empty() && net.no_net {
        anyhow::bail!(
            "running a command in the guest needs networking (the agent talks over the \
             guest NIC), but --no-net was given"
        );
    }
    Ok(())
}

/// The command a `freebsd`/`netbsd` boot should run in the guest.
///
/// The bundled BSD images are headless (no console getty), so a plain foreground
/// `bsdkrun freebsd` would boot to a console that just sits at the last rc line.
/// Instead, default it to an interactive shell over the agent — you drop into a
/// prompt and the VM powers off when you exit, like foreground `bsdkrun linux`.
/// This needs the agent (networking); `-d`, an explicit command, or `--no-net`
/// all opt out and keep their own behavior (background / that command / classic
/// console boot).
///
/// Returns `(command, interactive)`. `interactive` is true only for the shell we
/// synthesize, so it gets a PTY; an *explicit* command runs non-interactively
/// (like `docker run` without `-t`). That also sidesteps a guest-agent PTY drain
/// race that swallows a fast command's output when a tty is allocated.
pub(crate) fn bsd_exec_after(
    command: &[String],
    detach: bool,
    no_net: bool,
) -> (Vec<String>, bool) {
    if command.is_empty() && !detach && !no_net {
        (vec!["/bin/sh".to_string()], true)
    } else {
        (command.to_vec(), false)
    }
}

/// If `--repo` was given and there's no explicit command, make the post-boot
/// command the repo clone (installs git + records the cwd marker).
pub(crate) fn bsd_inject_repo(args: &mut BsdArgs) {
    if args.command.is_empty() {
        if let Some(argv) = args.repo.as_deref().and_then(repo_clone_argv) {
            args.command = argv;
        }
    }
}

/// `freebsd` / `netbsd`: fetch the image if needed, auto-locate the firmware,
/// then boot it. How it boots depends on the host OS:
///
/// - **macOS** boots through FreeBSD's `loader.efi`, which needs libkrun's EDK2
///   firmware (the `libkrun-efi` flavor, macOS-only) — see [`boot_freebsd_efi`].
/// - **Linux/amd64** direct-boots the GENERIC kernel via **PVH** (no firmware),
///   like `netbsd` — see [`boot_freebsd_pvh`]. Needs the PVH libkrun fork.
pub(crate) fn boot_freebsd(args: BsdArgs) -> Result<()> {
    boot_freebsd_disk(args, None)
}

/// Boot FreeBSD, optionally from a specific root disk (`disk_override`, used to
/// boot a committed snapshot) instead of the fetched/bundled base image.
pub(crate) fn boot_freebsd_disk(mut args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    bsd_inject_repo(&mut args);
    #[cfg(target_os = "macos")]
    {
        boot_freebsd_efi(args, disk_override)
    }
    #[cfg(target_os = "linux")]
    {
        boot_freebsd_pvh(args, disk_override)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (args, disk_override);
        anyhow::bail!("bsdkrun freebsd is only supported on macOS and Linux");
    }
}

/// macOS EFI-firmware boot: FreeBSD's `loader.efi` takes over from the ESP.
#[cfg(target_os = "macos")]
pub(crate) fn boot_freebsd_efi(args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    // No explicit command on a foreground boot → drop into an interactive shell.
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    // A snapshot boots from its saved disk; otherwise, default (no --version) to
    // bsdkrun's bundled arm64 image (agent injected so `exec` works), or fetch the
    // official FreeBSD VM image for an explicit --version / non-arm64 host.
    let disk = match disk_override {
        Some(d) => d,
        None => match (host::Arch::current()?, &args.version) {
            (host::Arch::Aarch64, None) => fetch::fetch_freebsd_arm64_image(args.force)?,
            _ => {
                let cache = fetch::cache_dir()?;
                fetch::fetch(fetch::Os::Freebsd, args.version.clone(), &cache, args.force)?
            }
        },
    };
    let firmware = match args.firmware {
        Some(f) => f,
        None => locate_krun_efi()?,
    };
    firmware_machine(
        &firmware,
        &disk,
        &args.attach_disk,
        &args.run,
        &args.net,
        &args.vm,
        &exec_after,
        interactive,
        args.verbose,
        args.disk_size.as_deref(),
    )
}

/// FreeBSD command line for the Linux/amd64 PVH boot (override with
/// `$BSDKRUN_FREEBSD_CMDLINE`). The bundled image is a bare makefs UFS on a
/// virtio-blk disk (`vtbd0`); `console=comconsole` routes the console to the
/// serial libkrun provides at COM1.
///
/// `hw.uart.console=io:0x3f8` is the key: it points FreeBSD's `uart(4)` low-level
/// console straight at libkrun's 16550 I/O port, so the console comes up at
/// `cninit` — the earliest console init — without needing the `device.hints` the
/// BOOTLOADER normally supplies (a PVH direct boot has no loader). We also pass
/// the equivalent `hint.uart.0.*` so `uart0` fully attaches as a device later.
/// virtio-mmio device hints are appended by libkrun itself (via
/// `KRUN_VIRTIO_MMIO_HINTS=freebsd`).
#[cfg(target_os = "linux")]
pub(crate) fn freebsd_cmdline() -> String {
    if let Ok(s) = std::env::var("BSDKRUN_FREEBSD_CMDLINE") {
        if !s.is_empty() {
            return s;
        }
    }
    // NB: FreeBSD can't derive its TSC frequency under libkrun on its own (no
    // KVM tsc-freq CPUID leaf by default, AMD has no Intel TSC leaf, and PVH's
    // xen_delay calibration path faults on a Xen pvclock KVM never sets up).
    // `machdep.tsc_freq` looks like the answer but is a runtime sysctl, not a
    // boot tunable, so it can't be set from here. The libkrun fork instead
    // synthesizes CPUID leaf 0x40000010 (tsc_freq_cpuid_vm) from KVM_GET_TSC_KHZ,
    // which FreeBSD reads before calibration — see configure_x86_64 in libkrun.
    "vfs.root.mountfrom=ufs:/dev/vtbd0 console=comconsole hw.uart.console=io:0x3f8 \
     hint.uart.0.at=isa hint.uart.0.port=0x3f8 hint.uart.0.flags=0x10 hint.uart.0.irq=4"
        .to_string()
}

/// Linux/amd64 PVH direct boot: enter the bundled FIRECRACKER kernel at its
/// `PHYS32_ENTRY`, no firmware. Needs a PVH-capable libkrun (the
/// tsirysndr/libkrun `feat/pvh-boot` fork) — stock libkrun boots x86_64 kernels
/// with the Linux protocol and would triple-fault immediately.
#[cfg(target_os = "linux")]
pub(crate) fn boot_freebsd_pvh(args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    let arch = host::Arch::current()?;
    if !matches!(arch, host::Arch::X86_64) {
        anyhow::bail!(
            "FreeBSD on Linux is only supported on amd64 (PVH direct boot) for now; \
             this host is {}.",
            arch.slug()
        );
    }

    // Tell libkrun to enter via the kernel's PHYS32_ENTRY note and to advertise
    // its virtio-mmio devices in FreeBSD's numbered-key cmdline form
    // (`virtio_mmio.device=`, `virtio_mmio.device_1=`, ...). FreeBSD can't read
    // the Linux form: Linux repeats the key, but FreeBSD's kernel environment
    // hides duplicate keys past the first.
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    let joined = prepare_network(&args.net, true)?; // FreeBSD DHCPs its IP
    std::env::set_var("KRUN_PVH", "1");
    std::env::set_var("KRUN_VIRTIO_MMIO_HINTS", "freebsd");

    let disk = match disk_override {
        Some(d) => d,
        None => fetch::fetch_freebsd_amd64_image(args.force)?,
    };
    let kernel = fetch::fetch_freebsd_amd64_kernel(args.force)?;

    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(&disk);
    let volume = args.run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = prepare_bsd_disk(
        &disk,
        &vdir,
        args.run.persist,
        volume.as_deref(),
        args.disk_size.as_deref(),
    )?;

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        db::record_disk(&disk.to_string_lossy());
        ctx.add_disk("root", &root_disk, false)
            .with_context(|| format!("attaching root disk {}", root_disk.display()))?;
        attach_extra_disks(&ctx, &args.attach_disk)?;
        let gvproxy = setup_networking_with_agent(&ctx, &args.net, Some(&vdir))?;
        ctx.set_kernel(&kernel, linux::kernel_format(), None, &freebsd_cmdline())
            .context("configuring kernel")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.run.volume.as_deref(), &volume) {
        db::record_volume(name, "freebsd", &image, &dir.to_string_lossy());
    }
    save_attached_disks(&vdir, &args.attach_disk);
    let result = run_machine(
        &machine_id,
        &vdir,
        "freebsd",
        &image,
        &exec_after.join(" "),
        args.vm.cpus,
        args.vm.mem,
        args.run.detach,
        true,
        args.run.volume.as_deref(),
        &args.net.ports,
        &exec_after,
        interactive,
        args.verbose,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &args.net.ports, &joined);
    result
}

/// NetBSD kernel command line (override with `$BSDKRUN_NETBSD_CMDLINE`). The root
/// device is arch-specific: the arm64 image is GPT-partitioned, so its root FFS
/// is the wedge `dk1` (`dk0` is the EFI partition); the amd64 image is a bare
/// makefs FFS on a virtio-blk disk, which NetBSD roots as `ld0a`.
///
/// amd64 also needs `console=com`: in a PVH direct boot there's no NetBSD
/// bootloader to hand over bootinfo, and without it the kernel defaults its
/// console to VGA text ("pc") — which libkrun doesn't have — so all output
/// silently vanishes. `console=com` selects the serial console libkrun provides
/// (same as the upstream NetBSD-on-Firecracker setup).
pub(crate) fn netbsd_cmdline() -> String {
    if let Ok(s) = std::env::var("BSDKRUN_NETBSD_CMDLINE") {
        if !s.is_empty() {
            return s;
        }
    }
    match host::Arch::current() {
        Ok(host::Arch::X86_64) => "root=ld0a console=com".to_string(),
        _ => "root=dk1".to_string(),
    }
}

/// NetBSD boots via **direct kernel** — libkrun jumps straight into the kernel,
/// no EFI firmware needed (unlike FreeBSD). The disk + kernel are arch-specific:
///
/// - **arm64** uses bsdkrun's bundled evbarm image (the `gzimg` with the agent
///   injected) + the evbarm `GENERIC64` kernel from the NetBSD CDN (`root=dk1`).
/// - **amd64** PVH-direct-boots the bundled MICROVM kernel + FFS rootfs. Needs a
///   PVH-capable libkrun (the tsirysndr/libkrun `feat/pvh-boot` fork) — stock
///   libkrun boots x86_64 kernels with the Linux protocol and triple-faults.
///
/// `--version` applies only to the arm64 kernel; the images themselves are pinned
/// bundled assets.
pub(crate) fn boot_netbsd(args: BsdArgs) -> Result<()> {
    boot_netbsd_disk(args, None)
}

/// Boot NetBSD, optionally from a specific root disk (`disk_override`, used to
/// boot a committed snapshot) instead of the fetched bundled image.
pub(crate) fn boot_netbsd_disk(mut args: BsdArgs, disk_override: Option<PathBuf>) -> Result<()> {
    bsd_inject_repo(&mut args);
    let (exec_after, interactive) = bsd_exec_after(&args.command, args.run.detach, args.net.no_net);
    ensure_net_for_exec(&args.net, &exec_after)?;
    let joined = prepare_network(&args.net, true)?; // NetBSD DHCPs its IP
    let arch = host::Arch::current()?;

    // amd64 NetBSD is a PVH kernel (MICROVM). Tell libkrun to enter via the
    // PHYS32_ENTRY note instead of the Linux boot protocol. Harmless on a libkrun
    // without PVH support (the flag is simply ignored).
    if matches!(arch, host::Arch::X86_64) {
        std::env::set_var("KRUN_PVH", "1");
    }

    // The kernel is always the bundled asset; a snapshot overrides the disk only.
    let kernel = match arch {
        host::Arch::X86_64 => fetch::fetch_netbsd_amd64_kernel(args.force)?,
        host::Arch::Aarch64 => fetch::fetch_netbsd_kernel(args.version.clone(), args.force)?,
    };
    let disk = match disk_override {
        Some(d) => d,
        None => match arch {
            host::Arch::X86_64 => fetch::fetch_netbsd_amd64_image(args.force)?,
            host::Arch::Aarch64 => fetch::fetch_netbsd_arm64_image(args.force)?,
        },
    };

    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(&disk);
    let volume = args.run.volume.as_deref().map(volume_dir).transpose()?;
    let root_disk = prepare_bsd_disk(
        &disk,
        &vdir,
        args.run.persist,
        volume.as_deref(),
        args.disk_size.as_deref(),
    )?;

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        ctx.attach_stdio_serial_console()
            .context("wiring guest serial console to stdio")?;
        db::record_disk(&disk.to_string_lossy());
        ctx.add_disk("root", &root_disk, false)
            .with_context(|| format!("attaching root disk {}", root_disk.display()))?;
        attach_extra_disks(&ctx, &args.attach_disk)?;
        let gvproxy = setup_networking_with_agent(&ctx, &args.net, Some(&vdir))?;
        ctx.set_kernel(&kernel, linux::kernel_format(), None, &netbsd_cmdline())
            .context("configuring kernel")?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.run.volume.as_deref(), &volume) {
        db::record_volume(name, "netbsd", &image, &dir.to_string_lossy());
    }
    save_attached_disks(&vdir, &args.attach_disk);
    let result = run_machine(
        &machine_id,
        &vdir,
        "netbsd",
        &image,
        &exec_after.join(" "),
        args.vm.cpus,
        args.vm.mem,
        args.run.detach,
        true,
        args.run.volume.as_deref(),
        &args.net.ports,
        &exec_after,
        interactive,
        args.verbose,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &args.net.ports, &joined);
    result
}

/// Locate libkrun's EDK2 firmware (`KRUN_EFI`), keeping a copy in bsdkrun's own
/// cache dir (not the current directory). Overridden by `$BSDKRUN_FIRMWARE` (and
/// by the `--firmware` flag before this is called). macOS only — the EFI
/// firmware ships with libkrun-efi, which is macOS-only.
#[cfg(target_os = "macos")]
pub(crate) fn locate_krun_efi() -> Result<PathBuf> {
    if let Some(f) = std::env::var_os("BSDKRUN_FIRMWARE") {
        let p = PathBuf::from(f);
        if !p.exists() {
            anyhow::bail!("BSDKRUN_FIRMWARE={} does not exist", p.display());
        }
        return Ok(p);
    }
    // Keep a copy under bsdkrun's cache ($BSDKRUN_CACHE / ~/.cache/bsdkrun) so it
    // works from any directory and survives a krunkit upgrade.
    let cache = fetch::cache_dir()?;
    let cached = cache.join("KRUN_EFI.fd");
    if cached.exists() {
        return Ok(cached);
    }
    let src = find_krunkit_firmware()?;
    std::fs::create_dir_all(&cache).ok();
    // Copy-on-write clone from krunkit's copy (`cp -c` — clonefile on APFS),
    // falling back to a plain copy.
    if fetch::run(
        std::process::Command::new("cp")
            .arg("-c")
            .arg(&src)
            .arg(&cached),
        "cp (clone firmware)",
    )
    .is_err()
    {
        let _ = std::fs::remove_file(&cached);
        std::fs::copy(&src, &cached).with_context(|| {
            format!("copying firmware {} -> {}", src.display(), cached.display())
        })?;
    }
    info!(path = %cached.display(), "cached KRUN_EFI firmware");
    Ok(cached)
}

/// Find krunkit's `KRUN_EFI.silent.fd` in its Homebrew install. macOS only —
/// krunkit (and the EFI firmware) exist only there.
#[cfg(target_os = "macos")]
pub(crate) fn find_krunkit_firmware() -> Result<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    if let Ok(out) = std::process::Command::new("brew")
        .args(["--prefix", "krunkit"])
        .output()
    {
        if out.status.success() {
            let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !p.is_empty() {
                prefixes.push(PathBuf::from(p));
            }
        }
    }
    for p in ["/opt/homebrew/opt/krunkit", "/opt/homebrew", "/usr/local"] {
        prefixes.push(PathBuf::from(p));
    }
    for base in prefixes {
        let fw = base.join("share/krunkit/KRUN_EFI.silent.fd");
        if fw.exists() {
            return Ok(fw);
        }
    }
    anyhow::bail!(
        "could not find the KRUN_EFI firmware. Install it with `brew install krunkit`, \
         or pass --firmware /path/to/KRUN_EFI.fd (or set BSDKRUN_FIRMWARE)."
    )
}

/// Prepare a BSD machine's root disk. With `volume`, the disk lives at a stable
/// path under `<state>/volumes` and is reused across runs (changes persist);
/// with `persist`, the base disk is booted in place; otherwise it's cloned into
/// `vdir` fresh each boot. Clones use an APFS copy-on-write clone (`cp -c` —
/// instant, no extra disk until the guest writes) so the base stays pristine.
pub(crate) fn prepare_bsd_disk(
    disk: &std::path::Path,
    vdir: &std::path::Path,
    persist: bool,
    volume: Option<&std::path::Path>,
    disk_size: Option<&str>,
) -> Result<PathBuf> {
    // Grow a freshly-cloned disk to `disk_size` (only enlarges); the guest
    // expands its root FS on boot. Never applied to a `persist` base (in place)
    // so the pristine base image is left untouched.
    let grow = |dst: &std::path::Path| -> Result<()> {
        if let Some(size) = disk_size {
            fetch::grow(dst, size)
                .with_context(|| format!("growing disk {} to {size}", dst.display()))?;
        }
        Ok(())
    };
    let ext = disk.extension().and_then(|e| e.to_str()).unwrap_or("img");
    if let Some(voldir) = volume {
        std::fs::create_dir_all(voldir)
            .with_context(|| format!("creating volume dir {}", voldir.display()))?;
        let dst = voldir.join(format!("root.{ext}"));
        if dst.exists() {
            info!(path = %dst.display(), "reusing persistent volume disk");
            return Ok(dst);
        }
        info!(path = %dst.display(), "creating persistent volume (CoW clone of base)");
        clone_cow_file(disk, &dst)?;
        grow(&dst)?;
        return Ok(dst);
    }
    if persist {
        return Ok(disk.to_path_buf());
    }
    let dst = vdir.join(format!("root.{ext}"));
    let _ = std::fs::remove_file(&dst);
    clone_cow_file(disk, &dst)?;
    grow(&dst)?;
    Ok(dst)
}

/// Parse a `--mount HOST:GUEST[:ro]` spec into a bind mount. The host directory
/// must exist (it's canonicalized to an absolute path); the guest path must be
/// absolute.
pub(crate) fn parse_mount(spec: &str) -> Result<linux::BindMount> {
    let parts: Vec<&str> = spec.split(':').collect();
    let (host, guest, ro) = match parts.as_slice() {
        [h, g] => (*h, *g, false),
        [h, g, "ro"] => (*h, *g, true),
        [h, g, "rw"] => (*h, *g, false),
        _ => anyhow::bail!("invalid --mount {spec:?} — expected HOST:GUEST[:ro]"),
    };
    if host.is_empty() || guest.is_empty() {
        anyhow::bail!("invalid --mount {spec:?} — empty HOST or GUEST");
    }
    if !guest.starts_with('/') {
        anyhow::bail!("invalid --mount {spec:?} — GUEST path must be absolute");
    }
    let host = std::fs::canonicalize(host)
        .with_context(|| format!("--mount host directory {host:?} does not exist"))?;
    if !host.is_dir() {
        anyhow::bail!("--mount host path {} is not a directory", host.display());
    }
    Ok(linux::BindMount {
        host,
        guest: guest.to_string(),
        ro,
    })
}

/// Copy `src` to `dst` as a copy-on-write clone where the filesystem supports it
/// (APFS on macOS, reflink on Linux), falling back to a plain copy.
pub(crate) fn clone_cow_file(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    host::cow_copy(src, dst, false)
}

/// A friendly machine name: a pending restart override if set, else a fresh
/// unique `adjective_scientist` (falling back to a non-checked random name only
/// if the DB can't be opened).
pub(crate) fn machine_name() -> String {
    names::take_override().unwrap_or_else(|| {
        db::Db::open()
            .map(|d| d.generate_name())
            .unwrap_or_else(|_| names::random_name())
    })
}

/// Run a machine either in the foreground (records + attaches to this terminal)
/// or detached (`detach`). `watchdog` installs the BSD SMP-shutdown watchdog on
/// the foreground path. `build` creates + configures the libkrun context.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_machine(
    machine_id: &str,
    vdir: &std::path::Path,
    kind: &str,
    image: &str,
    command: &str,
    cpus: u8,
    mem: u32,
    detach: bool,
    watchdog: bool,
    volume: Option<&str>,
    ports: &[PortForward],
    exec_after: &[String],
    interactive: bool,
    verbose: bool,
    build: impl FnOnce() -> Result<(Ctx, Option<Gvproxy>)>,
) -> Result<()> {
    // A trailing command runs in the guest via its agent once it's up, which
    // needs the VM in the background — so a one-shot command boots detached too
    // (but doesn't announce the id and powers the VM off when the command ends).
    if detach || !exec_after.is_empty() {
        return run_detached(
            machine_id,
            vdir,
            kind,
            image,
            command,
            cpus,
            mem,
            volume,
            ports,
            exec_after,
            detach,
            interactive,
            verbose,
            build,
        );
    }
    db::record_machine(
        machine_id,
        &machine_name(),
        image,
        kind,
        command,
        "running",
        Some(std::process::id() as i64),
        false,
        cpus as i64,
        mem as i64,
        &vdir.to_string_lossy(),
        volume,
        net::format_ports(ports).as_deref(),
    );
    // Foreground boot: this process becomes the VM, so sync before entering.
    crate::domains::sync_if_enabled();
    tty::install();
    if watchdog {
        watchdog::install();
    }
    let (ctx, gvproxy) = build()?;
    info!(id = %machine_id, kind, image, "booting machine");
    finish_recording(ctx, gvproxy, machine_id.to_string())
}

/// Single-quote a string for safe interpolation into a `/bin/sh` command.
pub(crate) fn shell_squote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A destination directory name for a cloned repo: the URL's basename without a
/// `.git` suffix, restricted to filesystem-safe characters.
pub(crate) fn repo_dir_name(url: &str) -> String {
    let base = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo");
    let base = base.strip_suffix(".git").unwrap_or(base);
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if cleaned.is_empty() {
        "repo".to_string()
    } else {
        cleaned
    }
}

/// The guest command that clones `repo` into `$HOME` after boot and records its
/// path in `/etc/bsdkrun-cwd`, so opening a shell drops you inside it. Best-effort
/// `git` install if the base image lacks it. `None` for an empty URL.
pub(crate) fn repo_clone_argv(repo: &str) -> Option<Vec<String>> {
    let repo = repo.trim();
    if repo.is_empty() {
        return None;
    }
    let dest = repo_dir_name(repo);
    let q = shell_squote(repo);
    let script = format!(
        "set -e; export PATH=\"/usr/local/bin:/usr/local/sbin:/usr/pkg/bin:$PATH\"; \
         H=\"${{HOME:-/root}}\"; case \"$H\" in \"\"|/) H=/root;; esac; \
         mkdir -p \"$H\" 2>/dev/null || true; \
         has() {{ command -v \"$1\" >/dev/null 2>&1; }}; \
         if ! has git; then \
           echo '==> installing git'; \
           (has apt-get && apt-get update && apt-get install -y git) || \
           (has apk && apk add --no-cache git) || \
           (has dnf && dnf install -y git) || \
           (has microdnf && microdnf install -y git) || \
           (has yum && yum install -y git) || \
           (has pacman && pacman -Sy --noconfirm git) || \
           (has zypper && zypper --non-interactive install git) || \
           (has pkg && ASSUME_ALWAYS_YES=yes pkg install -y git) || \
           (has pkgin && pkgin -y install git) || \
           (has pkg_add && {{ A=$(uname -p 2>/dev/null); [ \"$A\" = x86_64 ] && A=amd64; \
            R=$(uname -r 2>/dev/null | cut -d. -f1).0; \
            PKG_PATH=\"https://cdn.NetBSD.org/pub/pkgsrc/packages/NetBSD/$A/$R/All/\" \
              pkg_add git; }}) || \
           (has nix-env && nix-env -iA nixpkgs.git) || \
           (has nix && nix profile install nixpkgs#git) || true; \
         fi; \
         has git || {{ echo 'error: could not install git in the guest'; exit 1; }}; \
         echo '==> cloning {dest}'; \
         git clone --depth=1 {q} \"$H/{dest}\"; \
         printf '%s\\n' \"$H/{dest}\" > /etc/bsdkrun-cwd 2>/dev/null || true; \
         echo '==> cloned into '\"$H/{dest}\"",
    );
    Some(vec!["sh".to_string(), "-lc".to_string(), script])
}

pub(crate) fn boot_linux(args: LinuxArgs) -> Result<()> {
    let repo = args
        .repo
        .as_deref()
        .and_then(repo_clone_argv)
        .unwrap_or_default();
    boot_linux_from(args, None, &repo)
}

/// Boot a Linux/OCI machine.
///
/// `rootfs_override` clones the rootfs from that path instead of the pulled
/// image's cache — used to boot a *snapshot*/built flavor (its saved rootfs)
/// while still reading the base image's config for the entrypoint.
///
/// `provision` is a command run in the guest *after* boot, via its agent (the
/// `exec_after` hook) — used to install a flavor's packages/tools. A non-empty
/// `provision` implies the machine boots in the background so the parent can
/// wait for the agent and run it (see [`run_machine`]).
/// If a machine is joining a `--network` (or was given a `--name`), resolve its
/// name (recorded via the name override so `ps`/DNS agree) and join the network —
/// which sets `BSDKRUN_NET_*` for [`setup_networking_with_agent`]. Returns the
/// (network, member) to record as membership once the machine row exists.
/// `dhcp` = true for BSD guests (they DHCP their IP); false for Linux (static
/// kernel IP). Returns `(network, member, dhcp)` to finalize once booted.
pub(crate) fn prepare_network(
    net: &NetConfig,
    dhcp: bool,
) -> Result<Option<(String, String, bool)>> {
    if net.network.is_none() && net.name.is_none() {
        return Ok(None);
    }
    let member = match &net.name {
        Some(n) => n.clone(),
        None => db::Db::open()
            .map(|d| d.generate_name())
            .unwrap_or_else(|_| names::random_name()),
    };
    names::set_override(&member);
    if let Some(network) = &net.network {
        network::join(network, &member, dhcp)?;
        return Ok(Some((network.clone(), member, dhcp)));
    }
    Ok(None)
}

/// Finalize network membership after boot: a Linux (static) member just records
/// its allocated IP; a BSD (dhcp) member discovers its leased IP and wires up its
/// agent forward + DNS. No-op when not on a network.
pub(crate) fn finalize_network(
    machine_id: &str,
    agent_dir: Option<&std::path::Path>,
    ports: &[PortForward],
    joined: &Option<(String, String, bool)>,
) {
    if let Some((network, member, dhcp)) = joined {
        if *dhcp {
            if let Some(dir) = agent_dir {
                if let Err(e) = network::finalize_dhcp(network, member, machine_id, dir, ports) {
                    tracing::warn!("network finalize failed: {e:#}");
                }
            }
        } else if let Ok(db) = db::Db::open() {
            let ip = std::env::var("BSDKRUN_NET_IP").unwrap_or_default();
            let _ = db.set_machine_network(machine_id, network, &ip);
        }
        // Refresh every BSD member's /etc/hosts with the new membership so peers
        // resolve by name even where the gvproxy DNS trips a strict resolver
        // (NetBSD). Best-effort; the newly-joined member is now in the DB.
        let _ = network::sync_hosts(network);
    }
}

pub(crate) fn boot_linux_from(
    args: LinuxArgs,
    rootfs_override: Option<PathBuf>,
    provision: &[String],
) -> Result<()> {
    #[cfg(target_os = "macos")]
    crate::store::ensure_linux_storage()?;

    // Prepare everything that can fail before we fork / touch the hypervisor.
    let kernel = linux::ensure_kernel(args.kernel.clone(), &args.kernel_version)?;
    let image = oci::pull(&args.image)?;
    let mut ep =
        linux::resolve_entrypoint(&image.config, args.entrypoint.as_deref(), &args.command);
    // `-e K=V` (and flavor defaults) override the image's env in the guest.
    for kv in &args.env {
        let key = kv.split('=').next().unwrap_or("");
        ep.env.retain(|e| e.split('=').next() != Some(key));
        ep.env.push(kv.clone());
    }
    info!(argv = ?ep.argv, "resolved entrypoint");
    // Clone source: a snapshot's saved rootfs, else the pulled image's rootfs.
    let rootfs_src = rootfs_override.as_deref().unwrap_or(&image.rootfs);

    // Persist the image (best-effort; DB problems never block booting).
    db::record_image(
        &args.image,
        &image.digest,
        image.size,
        &image.rootfs.to_string_lossy(),
    );

    // Join a global network (allocate IP + register DNS + set BSDKRUN_NET_*)
    // before we build the ctx, so `setup_networking_with_agent` bridges in.
    // Linux uses a static kernel IP (dhcp = false).
    let joined = prepare_network(&args.net, false)?;

    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let command = ep.argv.join(" ");

    // A named volume makes the rootfs persist across runs. It needs virtio-fs
    // (an initramfs is a RAM disk — nothing to persist).
    if args.volume.is_some() && args.initramfs {
        anyhow::bail!("--volume needs virtio-fs; it can't persist an --initramfs (RAM) rootfs");
    }
    let volume = args.volume.as_deref().map(volume_dir).transpose()?;

    // Host directory bind mounts (`--mount HOST:GUEST[:ro]`), shared via virtio-fs.
    let mounts: Vec<linux::BindMount> = args
        .mounts
        .iter()
        .map(|s| parse_mount(s))
        .collect::<Result<_>>()?;

    // Decide up front whether networking will come up, so the baked-in initramfs
    // `/init` + kernel `ip=` match what `setup_networking` will actually do.
    let net_up = !args.net.no_net && net::locate().is_ok();

    // A detached machine with no explicit command keeps a console shell alive
    // across exits (so `shell` behaves like a persistent VM you attach to);
    // otherwise the workload runs once and the machine powers off when it ends.
    let persistent = args.detach && args.command.is_empty();

    // `--attach-disk PATH:/guest/path` asks the guest to mount that disk; the
    // init formats a blank one and grows a filesystem whose image has been
    // enlarged (see `linux::DiskMount`).
    let disk_mounts: Vec<linux::DiskMount> = args
        .attach_disk
        .iter()
        .enumerate()
        .filter_map(|(index, d)| {
            d.mount.as_ref().map(|guest| linux::DiskMount {
                index,
                guest: guest.clone(),
                ro: d.read_only,
            })
        })
        .collect();

    // Prepare the rootfs before forking so failures surface synchronously.
    // A volume is a persistent, writable virtio-fs root (CoW-cloned on first use);
    // otherwise it's a per-machine virtio-fs clone, or an initramfs (--initramfs).
    let root_mode = match (&volume, args.virtiofs()) {
        (Some(voldir), _) => LinuxRoot::Virtiofs(linux::prepare_volume_root(
            rootfs_src,
            &ep,
            net_up,
            persistent,
            voldir,
            &mounts,
            &disk_mounts,
        )?),
        (None, true) => LinuxRoot::Virtiofs(linux::prepare_virtiofs_root(
            rootfs_src,
            &ep,
            net_up,
            persistent,
            &machine_rootfs_dir(&machine_id, &vdir),
            &mounts,
            &disk_mounts,
        )?),
        (None, false) => {
            linux::warn_initramfs_memory(rootfs_src, args.vm.mem);
            LinuxRoot::Initramfs(linux::build_initramfs(
                rootfs_src,
                &ep,
                net_up,
                persistent,
                &mounts,
                &disk_mounts,
            )?)
        }
    };

    let build = || -> Result<(Ctx, Option<Gvproxy>)> {
        let ctx = Ctx::new()?;
        ctx.set_vm_config(args.vm.cpus, args.vm.mem)?;
        // `hvc*` uses libkrun's implicit virtio-console; `ttyS*` its explicit serial.
        if !args.console.starts_with("hvc") {
            ctx.attach_stdio_serial_console()
                .context("wiring guest serial console to stdio")?;
        }
        let gvproxy =
            configure_linux_ctx(&ctx, &args, &kernel, &root_mode, &mounts, net_up, &vdir)?;
        Ok((ctx, gvproxy))
    };

    if let (Some(name), Some(dir)) = (args.volume.as_deref(), &volume) {
        db::record_volume(name, "linux", &args.image, &dir.to_string_lossy());
    }
    save_attached_disks(&vdir, &args.attach_disk);
    // The guest's env and host shares are configuration a restart must not
    // lose (see `save_env` / `save_mounts`).
    save_env(&vdir, &args.env);
    save_mounts(&vdir, &mounts);
    // Linux never uses the SMP-shutdown watchdog: it redirects fd 2, which
    // libkrun's implicit virtio-console (hvc0) claims for the guest.
    let result = run_machine(
        &machine_id,
        &vdir,
        "linux",
        &args.image,
        &command,
        args.vm.cpus,
        args.vm.mem,
        args.detach,
        false,
        args.volume.as_deref(),
        &args.net.ports,
        provision,
        false,
        false,
        build,
    );
    finalize_network(&machine_id, Some(&vdir), &args.net.ports, &joined);
    result
}

/// How a Linux guest's root filesystem is provided.
pub(crate) enum LinuxRoot {
    /// Whole rootfs loaded into RAM (`--initramfs`).
    Initramfs(PathBuf),
    /// Copy-on-write virtio-fs clone. Ephemeral per-machine by default, or a
    /// persistent named volume (`-v NAME`) whose clone lives in the volume dir.
    Virtiofs(PathBuf),
}

/// Configure networking + rootfs + kernel on `ctx` (shared by the foreground and
/// detached paths; the caller wires the console first). Returns the gvproxy.
#[allow(clippy::too_many_arguments)]
pub(crate) fn configure_linux_ctx(
    ctx: &Ctx,
    args: &LinuxArgs,
    kernel: &std::path::Path,
    root: &LinuxRoot,
    mounts: &[linux::BindMount],
    net_up: bool,
    vdir: &std::path::Path,
) -> Result<Option<Gvproxy>> {
    let gvproxy = setup_networking_with_agent(ctx, &args.net, Some(vdir))?;
    // Share each `--mount` host directory over virtio-fs; the generated init
    // mounts them by matching tag.
    for (i, m) in mounts.iter().enumerate() {
        ctx.add_virtiofs(&linux::mount_tag(i), &m.host)
            .with_context(|| format!("sharing --mount host dir {}", m.host.display()))?;
    }
    // Raw disks (`--attach-disk`) as virtio-blk. With a virtio-fs root there is
    // no root block device, so the first one is the guest's /dev/vda.
    attach_extra_disks(ctx, &args.attach_disk)?;
    // We boot our own init in every virtio-fs mode (not libkrun's init.krun,
    // which is for the bundled libkrunfw kernel).
    match root {
        LinuxRoot::Virtiofs(dir) => {
            ctx.set_root(dir)
                .context("configuring virtio-fs root (needs a CONFIG_VIRTIO_FS=y kernel)")?;
            let cmdline = linux::virtiofs_cmdline(&args.console, net_up);
            ctx.set_kernel(kernel, linux::kernel_format(), None, &cmdline)
                .context("configuring kernel")?;
        }
        LinuxRoot::Initramfs(img) => {
            let cmdline = linux::kernel_cmdline(&args.console, net_up);
            ctx.set_kernel(kernel, linux::kernel_format(), Some(img), &cmdline)
                .context("configuring kernel")?;
        }
    }
    Ok(gvproxy)
}

/// Like `finish`, but records the guest's exit code in the state DB first.
pub(crate) fn finish_recording(
    ctx: Ctx,
    gvproxy: Option<Gvproxy>,
    machine_id: String,
) -> Result<()> {
    let result = ctx.start_enter();
    tty::restore_stdin_termios();
    let code = result.context("starting machine")?;
    info!(code, "guest exited");
    db::update_machine_status(&machine_id, "exited", Some(code as i64));
    drop(gvproxy);
    std::process::exit(code);
}

/// Run a machine in the background (any guest type): fork, record the child in
/// the state DB, print its id, and detach the child — wiring the guest console
/// to a per-machine PTY broker (console.log + console.sock). `build` (run in the
/// child, after the console is wired) creates + configures the libkrun context.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_detached(
    machine_id: &str,
    vdir: &std::path::Path,
    kind: &str,
    image: &str,
    command: &str,
    cpus: u8,
    mem: u32,
    volume: Option<&str>,
    ports: &[PortForward],
    exec_after: &[String],
    keep_running: bool,
    interactive: bool,
    verbose: bool,
    build: impl FnOnce() -> Result<(Ctx, Option<Gvproxy>)>,
) -> Result<()> {
    use std::io::Write;
    // Flush any pending output (e.g. the pull progress) before forking.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        anyhow::bail!("fork failed: {}", std::io::Error::last_os_error());
    }
    if pid > 0 {
        // Parent: record the running machine and print its id, like `docker -d`.
        db::record_machine(
            machine_id,
            &machine_name(),
            image,
            kind,
            command,
            "running",
            Some(pid as i64),
            true,
            cpus as i64,
            mem as i64,
            &vdir.to_string_lossy(),
            volume,
            net::format_ports(ports).as_deref(),
        );
        // Machine domains follow the DB: parent side only — the forked child
        // becomes the VM and must not run host-side sync.
        crate::domains::sync_if_enabled();
        // No trailing command: classic `-d`, just announce the id.
        if exec_after.is_empty() {
            println!("{machine_id}");
            return Ok(());
        }
        // Command mode: the VM boots in the child (above); here in the parent we
        // wait for its agent, run the command against it, then (for a one-shot)
        // power the VM off. `-d` keeps it running and prints the id first.
        if keep_running {
            println!("{machine_id}");
        }
        return run_guest_command(
            machine_id,
            kind,
            exec_after,
            keep_running,
            interactive,
            verbose,
            pid as libc::pid_t,
        );
    }

    // Child: detach from the terminal/session, wire the console, boot.
    unsafe { libc::setsid() };
    // Send bsdkrun's own logs (fd 2) to a file in the machine dir.
    if let Ok(logf) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(vdir.join("bsdkrun.log"))
    {
        use std::os::fd::AsRawFd;
        unsafe { libc::dup2(logf.as_raw_fd(), 2) };
        std::mem::forget(logf); // fd 2 now owns it
    }

    let result = (|| -> Result<i32> {
        // Wire the guest console (fd 0/1) to the broker's PTY; a thread fans it
        // out to console.log + console.sock (see the console module).
        let console_fd = console::setup_detached(vdir)?;
        unsafe {
            libc::dup2(console_fd, 0);
            libc::dup2(console_fd, 1);
        }
        // Signal cleanup so `stop` (SIGTERM) tears down gvproxy.
        tty::install();
        let (ctx, gvproxy) = build()?;
        info!(id = %machine_id, kind, "detached machine booting");
        let code = ctx.start_enter().context("starting machine")?;
        db::update_machine_status(machine_id, "exited", Some(code as i64));
        drop(gvproxy);
        Ok(code)
    })();

    let code = match result {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("detached machine failed: {e:#}");
            db::update_machine_status(machine_id, "exited", Some(1));
            1
        }
    };
    unsafe { libc::_exit(code) };
}

/// How long to wait for a freshly-booted guest's agent before giving up on a
/// trailing command. A BSD guest takes tens of seconds to reach multiuser.
pub(crate) const GUEST_AGENT_BOOT_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(180);

/// Poll a booting machine's guest agent until it answers (or we time out / the
/// machine dies), returning the forwarded host port to `exec` against.
///
/// A BSD guest takes ~15-20s to reach the agent. When `console` is given
/// (`--verbose`), stream the guest's boot console to **stdout** while we wait —
/// so CI (and the curious) see the full boot, and e2e can assert on it.
/// Otherwise show a terse "still booting" counter on a terminal so the wait
/// doesn't look like a hang.
/// Boot milestones timestamped from the guest console by [`wait_for_agent`], each
/// measured from launch. `None` means the anchor line never appeared: NetBSD
/// direct-boots with no firmware/loader, so its `kernel_entry` ≈ `first_output`;
/// a very fast boot may reach the agent before a milestone is scanned.
#[derive(Default)]
pub(crate) struct BootMarks {
    /// First byte on the console — end of the hypervisor/firmware dead time.
    first_output: Option<std::time::Duration>,
    /// Kernel takes over (`---<<BOOT>>---` / NetBSD `booting ...`) — past firmware+loader.
    kernel_entry: Option<std::time::Duration>,
    /// Root filesystem mount / fsck — the kernel→userland handoff, start of rc.
    root_mount: Option<std::time::Duration>,
}

/// Background console watcher that timestamps [`BootMarks`] off the main
/// agent-poll loop (which blocks on `agent::ping` each iteration). Milestone
/// times are milliseconds-from-launch in atomics (0 = not seen yet); dropping the
/// scanner signals its thread to stop.
pub(crate) struct BootScanner {
    first: std::sync::Arc<std::sync::atomic::AtomicU64>,
    kernel: std::sync::Arc<std::sync::atomic::AtomicU64>,
    root: std::sync::Arc<std::sync::atomic::AtomicU64>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl BootScanner {
    fn spawn(log: std::path::PathBuf, start: std::time::Instant) -> Self {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;
        let s = BootScanner {
            first: Arc::new(AtomicU64::new(0)),
            kernel: Arc::new(AtomicU64::new(0)),
            root: Arc::new(AtomicU64::new(0)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let (first, kernel, root, stop) = (
            s.first.clone(),
            s.kernel.clone(),
            s.root.clone(),
            s.stop.clone(),
        );
        std::thread::spawn(move || {
            // `.max(1)` so a milestone at t≈0 never reads back as "not seen".
            let now = || (start.elapsed().as_millis() as u64).max(1);
            while !stop.load(Ordering::Relaxed) {
                if let Ok(bytes) = std::fs::read(&log) {
                    if first.load(Ordering::Relaxed) == 0 && !bytes.is_empty() {
                        first.store(now(), Ordering::Relaxed);
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    // Kernel takes over: FreeBSD `---<<BOOT>>---`, NetBSD `booting ...`.
                    if kernel.load(Ordering::Relaxed) == 0
                        && ["---<<BOOT>>---", "booting ..."]
                            .iter()
                            .any(|p| text.contains(p))
                    {
                        kernel.store(now(), Ordering::Relaxed);
                    }
                    // Root mount / fsck — the kernel→userland (rc) handoff, and the
                    // last milestone, so the thread can stop once it appears.
                    if root.load(Ordering::Relaxed) == 0
                        && ["Trying to mount root", "Starting root file system check"]
                            .iter()
                            .any(|p| text.contains(p))
                    {
                        root.store(now(), Ordering::Relaxed);
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        s
    }

    fn marks(&self) -> BootMarks {
        use std::sync::atomic::Ordering;
        let d = |a: &std::sync::atomic::AtomicU64| {
            let v = a.load(Ordering::Relaxed);
            (v != 0).then(|| std::time::Duration::from_millis(v))
        };
        BootMarks {
            first_output: d(&self.first),
            kernel_entry: d(&self.kernel),
            root_mount: d(&self.root),
        }
    }
}

impl Drop for BootScanner {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Report the boot timing gathered by [`wait_for_agent`]: one structured `info!`
/// line (shown by default) plus, when `BSDKRUN_BOOT_TIMING` is set, a per-phase
/// breakdown to stderr (firmware/loader → kernel probe → userland rc), computed
/// as deltas between whichever milestones were reached.
pub(crate) fn log_boot_timing(
    id: &str,
    kind: &str,
    total: std::time::Duration,
    marks: &BootMarks,
    show: bool,
) {
    let ms = |d: Option<std::time::Duration>| d.map(|x| x.as_millis() as u64);
    tracing::info!(
        total_ms = total.as_millis() as u64,
        to_first_output_ms = ms(marks.first_output),
        to_kernel_entry_ms = ms(marks.kernel_entry),
        to_root_mount_ms = ms(marks.root_mount),
        "boot timing (launch → agent-ready)"
    );
    if !show {
        return;
    }
    // Ordered checkpoints actually reached, ending at agent-ready; print the gap
    // between each consecutive pair (launch is the implicit zero).
    let mut pts: Vec<(&str, std::time::Duration)> = Vec::new();
    if let Some(t) = marks.first_output {
        pts.push(("first console output", t));
    }
    if let Some(t) = marks.kernel_entry {
        pts.push(("kernel entry", t));
    }
    if let Some(t) = marks.root_mount {
        pts.push(("root mount (rc start)", t));
    }
    pts.push(("agent ready", total));

    let s = |d: std::time::Duration| format!("{:>6.2}s", d.as_secs_f64());
    eprintln!("\x1b[36m[boot timing]\x1b[0m {kind} {id}");
    let mut prev_label = "launch";
    let mut prev = std::time::Duration::ZERO;
    for (label, at) in pts {
        let seg = format!("{prev_label} → {label}");
        eprintln!("  {:<45} {}", seg, s(at.saturating_sub(prev)));
        prev_label = label;
        prev = at;
    }
    eprintln!("  {:<45} {}", "total (launch → agent ready)", s(total));
}

pub(crate) fn wait_for_agent(
    id: &str,
    kind: &str,
    console: Option<&std::path::Path>,
) -> Result<u16> {
    use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
    let counter = console.is_none() && std::io::stderr().is_terminal();
    // A little braille spinner for the counter — cycles each poll.
    const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let mut frame = 0usize;
    let start = std::time::Instant::now();
    let deadline = start + GUEST_AGENT_BOOT_TIMEOUT;
    // Boot timing: timestamp boot milestones from the guest console to split the
    // wait into firmware/loader, kernel device probe, and userland rc. `start` is
    // captured just after the fork, so it tracks the real launch → usable-agent
    // wall clock; the console log is resolved independently of `--verbose` so the
    // milestones are caught even on a quiet boot. Report at `info!` (shown by
    // default) plus a phase breakdown to stderr when BSDKRUN_BOOT_TIMING is set.
    let boot_timing = std::env::var_os("BSDKRUN_BOOT_TIMING").is_some();
    let timing_log = db::Db::open()
        .ok()
        .and_then(|db| db.find_machine(id).ok())
        .map(|vm| std::path::PathBuf::from(vm.state_dir).join("console.log"));
    // Scan the console on a background thread, not in this loop: `agent::ping`
    // below blocks up to its read timeout each iteration, which would starve the
    // scan and collapse every milestone onto one late sample. The scanner drops
    // (and stops) when `wait_for_agent` returns on any path.
    let scanner = timing_log
        .as_ref()
        .map(|p| BootScanner::spawn(p.clone(), start));
    // Verbose: tail the console log, remembering how far we've streamed.
    let mut tail: Option<std::fs::File> = None;
    let mut off: u64 = 0;
    let mut drain_console = || {
        let Some(path) = console else { return };
        if tail.is_none() {
            tail = std::fs::File::open(path).ok();
        }
        if let Some(f) = tail.as_mut() {
            if f.seek(SeekFrom::Start(off)).is_ok() {
                let mut buf = Vec::new();
                if let Ok(n) = f.read_to_end(&mut buf) {
                    if n > 0 {
                        let out = std::io::stdout();
                        let mut h = out.lock();
                        let _ = h.write_all(&buf);
                        let _ = h.flush();
                        off += n as u64;
                    }
                }
            }
        }
    };
    // Animate/stream at a smooth ~12fps, but only actually poll the agent (a real
    // round-trip) about twice a second.
    let tick = std::time::Duration::from_millis(if counter { 80 } else { 250 });
    let mut last_poll = start - std::time::Duration::from_secs(1); // poll immediately
    loop {
        let now = std::time::Instant::now();
        if now.duration_since(last_poll) >= std::time::Duration::from_millis(500) {
            last_poll = now;
            // agent_target() also confirms the machine's pid is alive; ping() does
            // a real agent round-trip, so it's only true once the agent is up.
            if let Ok((_vm, port)) = agent_target(id) {
                if agent::ping(port) {
                    drain_console(); // flush any final boot output
                    if counter {
                        eprint!("\r\x1b[K"); // clear the progress line
                        let _ = std::io::stderr().flush();
                    }
                    let marks = scanner.as_ref().map(|s| s.marks()).unwrap_or_default();
                    log_boot_timing(id, kind, start.elapsed(), &marks, boot_timing);
                    return Ok(port);
                }
            }
            // Fail fast if the guest died during boot rather than wait out the timeout.
            if let Ok(db) = db::Db::open() {
                if let Ok(vm) = db.find_machine(id) {
                    if vm.status == "exited" || !vm.pid.map(db::pid_alive).unwrap_or(false) {
                        drain_console();
                        if counter {
                            eprint!("\r\x1b[K");
                        }
                        anyhow::bail!(
                            "machine {id} exited before its agent came up — see `bsdkrun logs {id}`"
                        );
                    }
                }
            }
        }
        if now >= deadline {
            anyhow::bail!(
                "timed out after {}s waiting for the guest agent on machine {id} \
                 (still booting? try `bsdkrun logs {id}`)",
                GUEST_AGENT_BOOT_TIMEOUT.as_secs()
            );
        }
        drain_console();
        if counter {
            eprint!(
                "\r\x1b[K\x1b[36m{}\x1b[0m booting {kind} microVM — waiting for the guest agent ({}s)…",
                SPIN[frame % SPIN.len()],
                start.elapsed().as_secs()
            );
            let _ = std::io::stderr().flush();
            frame += 1;
        }
        std::thread::sleep(tick);
    }
}

/// Parent-side of a BSD boot with a trailing command: wait for the guest agent,
/// run the command against it (streaming stdio), and — unless `keep_running`
/// (`-d`) — power the VM off afterward. Exits with the command's status.
pub(crate) fn run_guest_command(
    id: &str,
    kind: &str,
    argv: &[String],
    keep_running: bool,
    interactive: bool,
    verbose: bool,
    child_pid: libc::pid_t,
) -> Result<()> {
    use std::io::IsTerminal;
    // `--verbose`: stream the guest's boot console (its state_dir/console.log)
    // while we wait for the agent.
    let console = verbose
        .then(|| db::Db::open().ok().and_then(|db| db.find_machine(id).ok()))
        .flatten()
        .map(|vm| std::path::PathBuf::from(vm.state_dir).join("console.log"));
    let port = wait_for_agent(id, kind, console.as_deref())?;
    // Only the synthesized shell gets a PTY, and only when both ends are a real
    // terminal. Explicit commands run non-interactively so their output isn't
    // lost to the guest agent's PTY drain race on a fast exit.
    let tty = interactive && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    // The synthesized interactive shell (`interactive`) prefers bash and sets a
    // sane TERM on BSD; an explicit command runs verbatim with no injected env.
    let (argv, env): (Vec<String>, Vec<String>) = if interactive {
        (interactive_shell_argv(), interactive_shell_env(kind))
    } else {
        (argv.to_vec(), Vec::new())
    };
    let code = agent::exec(port, &argv, &env, tty).map_err(|e| agent_error(kind, e))?;
    if !keep_running {
        // One-shot: tear the VM down (SIGTERM -> the child's cleanup + poweroff).
        unsafe { libc::kill(child_pid, libc::SIGTERM) };
        db::update_machine_status(id, "exited", Some(128 + libc::SIGTERM as i64));
    }
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// restarting a machine
// ---------------------------------------------------------------------------

/// Restart a stopped machine *in place*: re-boot the recorded image / resources
/// / volume under the SAME id (like `docker start`), rather than minting a new
/// machine. Detached. The guest OS is inferred from the recorded image ref.
pub(crate) fn cmd_start(id: &str) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;

    // Already up? Nothing to do.
    if vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false) {
        println!("{}", vm.id);
        return Ok(());
    }

    let cpus = vm.cpus.clamp(1, 255) as u8;
    let mem = vm.mem.max(64) as u32;
    let volume = vm.volume.clone();

    // Resume a BSD machine from ITS OWN root disk, not a re-fetched base. The
    // per-machine working disk (a CoW clone of the original base — a fetched
    // image OR a committed snapshot) lives in the state dir as `root.<ext>`.
    // Re-cloning a base here would silently replace the disk: for a snapshot
    // flavor that means booting the DEFAULT image over the user's snapshot data
    // (data loss). So when that disk exists, boot it IN PLACE (persist) — which
    // also preserves runtime changes across stop/start, like `docker start`.
    // Volume-backed machines already resume their volume disk, so skip those.
    let existing_disk = if vm.volume.is_none() {
        let vdir = machine_dir_or_tmp(&vm.id);
        ["root.raw", "root.img", "root.qcow2"]
            .iter()
            .map(|n| vdir.join(n))
            .find(|p| p.exists())
    } else {
        None
    };

    // The DB row is left in place (the boot re-records it via INSERT OR REPLACE),
    // so it flips exited→running rather than vanishing from `ps`. NB: don't wipe
    // the whole state dir here — that means an `rm -rf` of the old read-only nix
    // rootfs, which is slow enough to make Play "spin forever". The Linux boot
    // path (prepare_linux_root) renames the stale rootfs aside and GC's it in the
    // background, so the restart returns promptly. Old sockets/logs/port files
    // are simply overwritten on the new boot.

    // The next boot picks up this id + name instead of generating fresh ones.
    id::set_override(&vm.id);
    if let Some(name) = &vm.name {
        names::set_override(name);
    }

    // Re-join the recorded global network on restart (its membership is stored
    // in the DB and edited via `network connect/disconnect`). Reuse the member
    // name so the derived MAC — and thus the BSD DHCP lease — stays stable; hint
    // the previously-assigned IP so a plain restart keeps its address.
    if let Some(ip) = vm.net_ip.as_deref().filter(|s| !s.is_empty()) {
        std::env::set_var("BSDKRUN_NET_PREF_IP", ip);
    }
    let net = NetConfig {
        no_net: false,
        // Restore the port forwards recorded at the original `run`/`-d`, so a
        // restarted machine keeps serving on the same host ports.
        ports: vm
            .ports
            .as_deref()
            .map(net::parse_ports)
            .unwrap_or_default(),
        mac: None,
        network: vm.network.clone(),
        name: vm.name.clone(),
    };
    let vmcfg = VmConfig { cpus, mem };

    // Detect the guest from the recorded kind first (the image ref is unreliable
    // for a snapshot machine — its image is `disk.img`/`disk.raw`, not a
    // `netbsd-*`/`freebsd-*` name). FreeBSD records `firmware` (macOS EFI) or
    // `freebsd` (Linux PVH); NetBSD records `netbsd` (or legacy `kernel`).
    let reference = vm.image.to_lowercase();
    let is_freebsd =
        matches!(vm.kind.as_str(), "firmware" | "freebsd") || reference.starts_with("freebsd");
    let is_netbsd =
        matches!(vm.kind.as_str(), "kernel" | "netbsd") || reference.starts_with("netbsd");

    if vm.kind == "linux" {
        let legacy_machine_dir = machine_dir_or_tmp(&vm.id);
        let legacy_rootfs = legacy_machine_dir.join("rootfs");
        let legacy_rootfs_exists = volume.is_none()
            && legacy_rootfs.symlink_metadata().is_ok()
            && std::fs::read_dir(&legacy_rootfs)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);

        #[cfg(target_os = "macos")]
        crate::store::ensure_linux_storage()?;

        let largs = LinuxArgs {
            image: vm.image.clone(),
            kernel: None,
            kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
            detach: true,
            initramfs: false,
            volume: volume.clone(),
            // Re-share the host directories recorded at the original boot; a
            // dropped share comes back as an empty directory in the guest.
            mounts: load_mounts(&legacy_machine_dir),
            // Re-attach the disks recorded at the original boot, so guest data
            // living on a block device survives stop/start.
            attach_disk: load_attached_disks(&legacy_machine_dir),
            entrypoint: None,
            // Re-apply the `-e` env recorded at the original boot: for many
            // images it decides what the entrypoint does at all.
            env: load_env(&legacy_machine_dir),
            console: "hvc0".to_string(),
            net,
            vm: vmcfg,
            repo: None,
            command: vec![], // persistent restart — keep a console shell alive
        };
        // Resume the machine's OWN rootfs (which holds its snapshot + runtime
        // changes) by passing it as the boot source, so restart never re-clones
        // the base OCI image and loses data. Reuse any intact, non-empty rootfs —
        // NOT gated on /bin|/nix, so images without those top-level dirs still
        // resume. Volume machines resume their volume rootfs already; a missing or
        // broken (nested) dir falls back to the base image.
        let own_rootfs = machine_rootfs_dir(&vm.id, &legacy_machine_dir).join("rootfs");
        if legacy_rootfs_exists && own_rootfs != legacy_rootfs {
            anyhow::bail!(
                "Linux machine {} uses a rootfs created before the case-sensitive macOS store; \
                 remove and recreate it with `bsdkrun rm {}` to avoid resuming corrupted storage",
                vm.id,
                vm.id
            );
        }
        let intact = own_rootfs.symlink_metadata().is_ok()
            && !own_rootfs.join("rootfs").exists()
            && std::fs::read_dir(&own_rootfs)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
        if volume.is_none() && intact {
            boot_linux_from(largs, Some(own_rootfs), &[])
        } else {
            boot_linux(largs)
        }
    } else if vm.kind == "nanos" {
        // Re-boot the machine's own cloned disk in place when it survives (so
        // TFS state persists across stop/start, like the BSDs); fall back to
        // a fresh clone of the original image.
        let spec = nanos::BootSpec::load(std::path::Path::new(&vm.state_dir))?;
        let reuse = existing_disk.is_some();
        boot_nanos_image(
            &spec.image,
            spec.kernel.as_deref(),
            &spec.cmdline,
            None,
            true,
            reuse,
            net,
            vmcfg,
            existing_disk,
        )
    } else if vm.kind == "osv" {
        // Re-boot the machine's own cloned disk in place when it survives, so
        // filesystem writes persist across stop/start like the BSDs; fall back
        // to a fresh clone of the original image.
        let spec = osv::BootSpec::load(std::path::Path::new(&vm.state_dir))?;
        let reuse = existing_disk.is_some();
        boot_osv_image(
            &spec.image,
            &spec.cmdline,
            spec.gic,
            None,
            false,
            reuse,
            volume.as_deref(),
            &[],
            true,
            net,
            vmcfg,
            existing_disk,
        )
    } else if vm.kind == "unikraft" {
        // Nothing to resume — a unikernel has no disk — so this just boots the
        // same image again, from the spec saved beside its console log.
        let spec = unikraft::BootSpec::load(std::path::Path::new(&vm.state_dir))?;
        boot_unikraft_image(
            &spec.kernel,
            &spec.cmdline,
            spec.initramfs.as_deref(),
            true,
            net,
            vmcfg,
            &spec.volumes,
        )
    } else if is_freebsd || is_netbsd {
        // Boot the machine's own disk in place when it exists (see above), so a
        // snapshot machine keeps its data; otherwise fall back to the base image.
        let reuse = existing_disk.is_some();
        let args = BsdArgs {
            version: None, // bundled image (as originally booted)
            firmware: None,
            force: false,
            // Re-attach the extra disks recorded at the original boot (e.g. a
            // build disk) — dropping them here silently unmounts guest data.
            attach_disk: load_attached_disks(&machine_dir_or_tmp(&vm.id)),
            disk_size: None,
            run: RunConfig {
                detach: true,
                persist: reuse, // in-place boot of the existing disk (no re-clone)
                volume,
            },
            net,
            vm: vmcfg,
            verbose: false,
            repo: None,
            command: vec![],
        };
        let result = match (is_freebsd, existing_disk) {
            (true, Some(d)) => boot_freebsd_disk(args, Some(d)),
            (true, None) => boot_freebsd(args),
            (false, Some(d)) => boot_netbsd_disk(args, Some(d)),
            (false, None) => boot_netbsd(args),
        };
        // Booting the in-place disk relabels the row's image to `root.<ext>`;
        // restore the original label so `ps` still shows what it was booted from.
        if reuse && result.is_ok() {
            db.set_machine_image(&vm.id, &vm.image).ok();
        }
        result
    } else {
        // Put the id back for any future attempt and report clearly.
        anyhow::bail!(
            "don't know how to restart a {:?} machine ({}); start it with `run` instead",
            vm.kind,
            vm.image
        );
    }
}

// ---------------------------------------------------------------------------
// booting a flavor
// ---------------------------------------------------------------------------
//
// These live here rather than beside the rest of the flavor commands because
// they end in a boot: the listing and editing half of `flavor` has to stay
// available to a build that links no hypervisor.

pub(crate) fn flavor_linux_args(
    image: String,
    detach: bool,
    cpus: u8,
    mem: u32,
    volume: Option<String>,
    ports: Vec<PortForward>,
    env: Vec<String>,
) -> LinuxArgs {
    LinuxArgs {
        image,
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach,
        initramfs: false,
        volume,
        mounts: vec![],
        attach_disk: vec![],
        entrypoint: None,
        env,
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports,
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig { cpus, mem },
        repo: None,
        command: vec![],
    }
}

/// Ensure a flavor's provisioned rootfs is built and cached, returning its path.
/// Cache hit → returns immediately (instant, no re-provisioning). Miss → runs
/// the provisioning build in a child `bsdkrun` process (streaming its progress)
/// and records the result so every later launch just clones it.
pub(crate) fn ensure_flavor_built(
    spec: &LinuxFlavorSpec,
    name: &str,
    cpus: u8,
    mem: u32,
) -> Result<PathBuf> {
    let key = flavor_build_key(&spec.image, &spec.nix, &spec.provision);
    let vol = flavor_build_volume(&key);
    let voldir = volume_dir(&vol)?;
    let rootfs = voldir.join("rootfs");
    let marker = voldir.join(".provisioned");
    if marker.exists() && rootfs.exists() {
        info!(flavor = name, key = %key, "using cached flavor build");
        return Ok(rootfs);
    }

    // Before provisioning a VM for minutes, try the image CI publishes for
    // this flavor: it is the same steps (see `flavors::dockerfile`) already
    // run. A miss — no image, no network, an arch we do not publish — falls
    // through to the local build rather than failing.
    if std::env::var_os("BSDKRUN_NO_PREBUILT").is_none() {
        match try_prebuilt_flavor(name, &voldir, &rootfs, &marker, &key) {
            Ok(true) => return Ok(rootfs),
            Ok(false) => {}
            Err(e) => info!(
                flavor = name,
                "prebuilt image unusable, building locally: {e:#}"
            ),
        }
    }
    // Cache miss: build in a CHILD process. Provisioning ends in `process::exit`
    // (see `run_guest_command`), so it must not run in this process — we need to
    // survive it to clone + boot the real machine afterwards.
    info!(flavor = name, key = %key, "building flavor (first launch)…");
    host::force_remove_dir_all(&voldir); // clear any half-built remnant
    let exe = std::env::current_exe().context("locating bsdkrun for the flavor build")?;
    let status = std::process::Command::new(exe)
        .args([
            "flavor",
            "__build",
            name,
            "--key",
            &key,
            "--cpus",
            &cpus.to_string(),
            "--mem",
            &mem.to_string(),
        ])
        .status()
        .context("spawning the flavor build")?;
    if !status.success() {
        host::force_remove_dir_all(&voldir);
        anyhow::bail!("provisioning {name} failed (see the output above)");
    }
    if !rootfs.exists() {
        host::force_remove_dir_all(&voldir);
        anyhow::bail!("the flavor build produced no rootfs for {name}");
    }
    std::fs::write(&marker, key.as_bytes()).ok();
    Ok(rootfs)
}

/// Pull the published image for a flavor into its build cache, if there is one.
///
/// Returns whether it was used. The pulled rootfs is CoW-cloned into the same
/// build volume a local provisioning would have filled, so everything
/// downstream — the marker, the per-machine clone, `flavor rm` — is unchanged.
fn try_prebuilt_flavor(
    name: &str,
    voldir: &std::path::Path,
    rootfs: &std::path::Path,
    marker: &std::path::Path,
    key: &str,
) -> Result<bool> {
    // Only flavors with a generated Dockerfile are published; anything else
    // (a bare OCI base, a BSD image) has nothing to pull.
    if flavors::dockerfile(name).is_none() {
        return Ok(false);
    }
    let reference = flavors::prebuilt_image(name);
    info!(flavor = name, image = %reference, "looking for a prebuilt flavor image");
    let image = oci::pull(&reference)?;

    host::force_remove_dir_all(voldir);
    std::fs::create_dir_all(voldir).with_context(|| format!("creating {}", voldir.display()))?;
    host::cow_copy(&image.rootfs, rootfs, true)?;
    std::fs::write(marker, key.as_bytes()).ok();
    info!(flavor = name, "using the prebuilt flavor image");
    Ok(true)
}

/// Hidden `bsdkrun flavor __build` — the child that provisions a flavor into its
/// build volume, then powers the builder off. Not for direct use.
pub(crate) fn cmd_flavor_build(name: &str, key: &str, cpus: u8, mem: u32) -> Result<()> {
    let spec = resolve_linux_flavor(name)
        .ok_or_else(|| anyhow::anyhow!("no such Linux flavor to build: {name}"))?;
    let argv = flavor_provision_argv(name, &spec.nix, &spec.provision)
        .ok_or_else(|| anyhow::anyhow!("{name} has nothing to provision"))?;
    let vol = flavor_build_volume(key);

    // Boot a builder whose root is the persistent build volume. A trivial
    // keep-alive is the main process so the VM (and its agent) stay up on any
    // base image while provisioning runs; `run_machine` powers it off when the
    // provisioning command finishes (detach=false ⇒ keep_running=false).
    let largs = LinuxArgs {
        image: spec.image.clone(),
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach: false,
        initramfs: false,
        volume: Some(vol),
        mounts: vec![],
        attach_disk: vec![],
        entrypoint: None,
        env: spec.env.clone(),
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports: vec![],
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig { cpus, mem },
        repo: None,
        command: vec![
            "sh".into(),
            "-c".into(),
            "while :; do sleep 86400; done".into(),
        ],
    };
    boot_linux_from(largs, None, &argv)
}

/// `bsdkrun flavor build <name>` — pre-build a flavor's provisioned rootfs into
/// the cache so a later `run` is instant. Streams provisioning output.
pub(crate) fn cmd_flavor_prebuild(name: &str, cpus: u8, mem: u32, force: bool) -> Result<()> {
    let Some(spec) = resolve_linux_flavor(name) else {
        anyhow::bail!("no such Linux flavor to build: {name} (see `bsdkrun flavors`)");
    };
    if spec.nix.is_empty() && spec.provision.is_empty() {
        println!("{name}: nothing to build (no provisioning steps)");
        return Ok(());
    }
    if force {
        // Drop the cached build so it's rebuilt from scratch.
        let key = flavor_build_key(&spec.image, &spec.nix, &spec.provision);
        if let Ok(dir) = volume_dir(&flavor_build_volume(&key)) {
            host::force_remove_dir_all(&dir);
        }
    }
    let built = ensure_flavor_built(&spec, name, cpus, mem)?;
    info!(flavor = name, rootfs = %built.display(), "flavor built");
    println!("{name}");
    Ok(())
}

// ---------------------------------------------------------------------------
// AI agent sandboxes
// ---------------------------------------------------------------------------

/// `bsdkrun ai start <agent>` / `bsdkrun claude` — boot a sandbox and, unless
/// told otherwise, hand the terminal to the agent's TUI.
///
/// The default is to *reuse* the agent's running sandbox: an agent's login and
/// its skills live on volumes, but re-booting a VM per invocation would still
/// throw away everything it had loaded. `--new` boots a second one against the
/// same home volume, so two sessions can run side by side.
pub(crate) fn cmd_ai_start(args: AiStartArgs) -> Result<()> {
    let agent = ai::require(&args.agent)?;
    // The CLI shares the directory it was run in; the daemon and the UIs pass
    // an explicit path (or none). That is the whole difference between them.
    let workspace = ai::resolve_workspace(args.workspace.as_deref(), args.cwd)?;

    let existing = if args.new {
        None
    } else {
        super::ai::running_machine(agent.id)?
    };

    let vm = match existing {
        Some(vm) => {
            info!(id = %vm.id, agent = agent.id, "reusing the running sandbox");
            vm
        }
        None => {
            super::ai::announce(agent, workspace.as_deref(), true);
            // The boot forks and the parent returns, but the id was minted
            // here, so the row can be read back directly rather than guessed at.
            let id = boot_ai_sandbox(agent, workspace.as_deref(), &args)?;
            db::Db::open()?.find_machine(&id)?
        }
    };

    // Wait for the guest agent before reporting success. A sandbox that has
    // *booted* is not yet a sandbox you can `exec` into: connecting in that
    // window fails with "the guest agent accepted the connection but sent no
    // output", which is what both the CLI attach below and the desktop panel
    // (which opens a terminal on the id this prints) used to race into.
    wait_for_agent(&vm.id, &vm.kind, None)
        .with_context(|| format!("waiting for the {} sandbox to come up", agent.label))?;

    if args.detach {
        println!("{}", vm.id);
        return Ok(());
    }

    // Attach: replace this process with an interactive exec into the guest, so
    // the agent's TUI owns the terminal exactly as if it were running locally.
    let ws = workspace.as_ref().map(|w| w.to_string_lossy().into_owned());
    let argv = super::ai::attach_argv(agent, ws.as_deref());
    super::guest::cmd_exec(&vm.id, &argv, &[], true)
}

/// `bsdkrun ai resume <machine>` — bring one stopped sandbox back and attach.
///
/// Distinct from `ai start`, which reasons about an *agent*: it would happily
/// boot a second sandbox rather than resume the one you asked for, losing the
/// workspace, name and project recorded against it.
///
/// The waiting is the point. `bsdkrun start <id>` returns once the VM is
/// launched, and a terminal opened in that window fails with "the guest agent
/// accepted the connection but sent no output" — the same race `cmd_ai_start`
/// documents, reached from the other direction.
pub(crate) fn cmd_ai_resume(machine: &str, detach: bool) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(machine)?;

    // The agent is recorded in the state dir rather than parsed out of the
    // name, because a user label may contain dashes.
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let agent_id = ai::agent_of(&vdir).ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not an agent sandbox — use `bsdkrun start {}` to resume an \
             ordinary machine",
            vm.name.as_deref().unwrap_or(&vm.id),
            machine
        )
    })?;
    let agent = ai::require(&agent_id)?;

    let running = vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false);
    if !running {
        info!(id = %vm.id, agent = agent.id, "resuming the stopped sandbox");
        // Resumes in place, with the env and mounts recorded at first boot —
        // which is what keeps the workspace share and the home volume.
        cmd_start(&vm.id)?;
    }

    // Re-read: `cmd_start` forks a fresh supervisor, so the pid and status on
    // the row read above are stale the moment it returns.
    let vm = db::Db::open()?.find_machine(&vm.id)?;
    wait_for_agent(&vm.id, &vm.kind, None)
        .with_context(|| format!("waiting for the {} sandbox to come up", agent.label))?;

    if detach {
        println!("{}", vm.id);
        return Ok(());
    }

    let workspace = ai::workspace_of(&vdir);
    let argv = super::ai::attach_argv(agent, workspace.as_deref());
    super::guest::cmd_exec(&vm.id, &argv, &[], true)
}

/// Boot one sandbox VM for an agent.
fn boot_ai_sandbox(
    agent: &ai::Agent,
    workspace: Option<&std::path::Path>,
    args: &AiStartArgs,
) -> Result<String> {
    let spec = resolve_linux_flavor(agent.flavor).ok_or_else(|| {
        anyhow::anyhow!(
            "{} has no flavor named {:?} — this is a bsdkrun bug",
            agent.label,
            agent.flavor
        )
    })?;

    let machine_id = id::short_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    ai::record_workspace(&vdir, workspace);

    // The agent's toolchain is installed once into a cached build volume; the
    // first launch streams that build, later ones clone it and boot instantly.
    let built = ensure_flavor_built(&spec, agent.flavor, args.vm.cpus, args.vm.mem)?;

    id::set_override(&machine_id);
    let name = ai::next_name(agent.id, args.name.as_deref())?;
    names::set_override(&name);
    // Recorded rather than parsed back out of the machine name: a label can
    // contain dashes, and the agent has to stay unambiguous.
    ai::record_agent(&vdir, agent.id);
    ai::record_label(&vdir, args.name.as_deref());
    ai::record_project(
        &vdir,
        ai::resolve_project(args.project.as_deref(), workspace, args.repo.as_deref()).as_deref(),
    );

    info!(agent = agent.id, sandbox = %name, "booting the agent sandbox");
    let largs = LinuxArgs {
        image: spec.image.clone(),
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach: true,
        initramfs: false,
        // NOT a volume: the *rootfs* is per-sandbox and disposable, which is
        // what makes a second session cheap. What persists is the home volume
        // mounted below, holding the agent's login.
        volume: None,
        mounts: ai::mounts(agent, workspace, !args.no_ssh)?,
        attach_disk: vec![],
        entrypoint: None,
        env: vec![format!("HOME={}", ai::GUEST_HOME)],
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports: vec![],
            mac: None,
            network: None,
            name: Some(name),
        },
        vm: VmConfig {
            cpus: args.vm.cpus,
            mem: args.vm.mem,
        },
        repo: args.repo.clone(),
        command: vec![],
    };
    // The clone runs as the post-boot command, not from `largs.repo` — that
    // field only records the request; `boot_linux` is what turns it into argv,
    // and this path calls `boot_linux_from` directly.
    let clone = args
        .repo
        .as_deref()
        .and_then(repo_clone_argv)
        .unwrap_or_default();
    boot_linux_from(largs, Some(built), &clone)?;
    Ok(machine_id)
}

// ---------------------------------------------------------------------------
// the Docker engine VM
// ---------------------------------------------------------------------------
//
// Here rather than beside the rest of `docker` for the usual reason: it ends
// in a boot, and everything else about the Docker integration has to keep
// working in a build that links no hypervisor.

/// `bsdkrun docker start` — bring up (or resume) the Docker engine VM, serve
/// its API on a host socket, and point the local `docker` CLI at it.
pub(crate) fn cmd_docker_start(args: DockerStartArgs) -> Result<()> {
    let bind = docker::PublishBind::parse(&args.publish_bind)?;
    let db = db::Db::open()?;
    let existing = db.find_machine(docker::MACHINE_NAME).ok();
    let running = existing
        .as_ref()
        .map(|vm| vm.status == "running" && vm.pid.map(db::pid_alive).unwrap_or(false))
        .unwrap_or(false);

    match &existing {
        Some(vm) if running => info!(id = %vm.id, "Docker VM already running"),
        // Resume in place: the volume holds every pulled image, and `start`
        // re-applies the recorded API port forward.
        Some(vm) => {
            info!(id = %vm.id, "resuming the Docker VM");
            // Re-record the shares first, so `--mount` on a later start takes
            // effect and a VM created before shares were persisted repairs
            // itself instead of coming back with an empty $HOME.
            let mounts = docker_mounts(&args)?;
            let parsed: Vec<linux::BindMount> = mounts
                .iter()
                .map(|m| parse_mount(m))
                .collect::<Result<_>>()?;
            save_mounts(&machine_dir_or_tmp(&vm.id), &parsed);
            cmd_start(&vm.id)?;
        }
        None => boot_docker_vm(&args)?,
    }

    // Re-read: a fresh boot minted the row, and a resume may have changed the pid.
    let vm = db::Db::open()?.find_machine(docker::MACHINE_NAME)?;
    let port = docker::api_port(&vm).ok_or_else(|| {
        anyhow::anyhow!(
            "the Docker VM has no API port forwarded — remove it with \
             `bsdkrun docker rm -f` and start again"
        )
    })?;

    info!("waiting for dockerd…");
    docker::wait_for_api(port, std::time::Duration::from_secs(args.timeout as u64))?;

    // A proxy from a previous run points at a port that no longer exists.
    docker::stop_proxy()?;
    docker::spawn_proxy(port, &vm.id, bind)?;

    let socket = docker::socket_path()?;
    if !args.no_context {
        docker::setup_context(&socket, !args.no_activate)?;
    }
    if args.system_socket {
        docker::claim_system_socket(&socket)?;
    }

    let status = docker::status()?;
    if args.json {
        println!("{}", serde_json::to_string(&status)?);
        return Ok(());
    }
    let containers = docker::containers(false).unwrap_or_default();
    super::docker::report_started(&status, &containers);
    Ok(())
}

/// Boot the engine VM itself: dind, on a persistent volume, with the host
/// directories a `docker run -v` is likely to name shared in at the same path.
fn boot_docker_vm(args: &DockerStartArgs) -> Result<()> {
    // Mint the id up front (as `start` does) so the VM always has the same
    // name-and-id pair, however it was created.
    let machine_id = id::short_id();
    let mounts = docker_mounts(args)?;

    // The API port is a normal recorded forward, so `bsdkrun start` re-applies
    // it and `ps` shows it — nothing here is special-cased downstream.
    // A dedicated image store, when asked for: an ext4 disk the guest mounts
    // at /var/lib/docker (see `linux::DiskMount`), so the store has a real
    // size that `docker disk --size` can raise.
    let attach_disk = match &args.disk_size {
        Some(size) => {
            let path = docker::ensure_data_disk(size)?;
            info!(disk = %path.display(), size = %size, "using a dedicated Docker image store");
            vec![DiskSpec {
                path,
                read_only: false,
                mount: Some(docker::DATA_MOUNT.to_string()),
            }]
        }
        None => vec![],
    };

    let api_host_port =
        net::free_local_port().context("reserving a host port for the Docker API")?;
    let mut ports = vec![PortForward::loopback(api_host_port, docker::GUEST_API_PORT)];
    ports.extend(args.ports.iter().copied());

    id::set_override(&machine_id);
    names::set_override(docker::MACHINE_NAME);

    info!(
        image = docker::IMAGE,
        api_port = api_host_port,
        "booting the Docker engine VM"
    );
    let largs = LinuxArgs {
        image: docker::IMAGE.to_string(),
        kernel: None,
        kernel_version: linux::DEFAULT_KERNEL_VERSION.to_string(),
        detach: true,
        initramfs: false,
        // The whole rootfs is the volume, which is what makes /var/lib/docker
        // (images, containers, volumes) survive a stop.
        volume: Some(docker::VOLUME.to_string()),
        mounts,
        attach_disk,
        entrypoint: None,
        // An empty TLS dir is what makes dind listen on plaintext 2375 instead
        // of generating certs and listening on 2376 — the whole reason the
        // forwarded port needs no client certificates.
        env: vec!["DOCKER_TLS_CERTDIR=".to_string()],
        console: "hvc0".to_string(),
        net: NetConfig {
            no_net: false,
            ports,
            mac: None,
            network: None,
            name: Some(docker::MACHINE_NAME.to_string()),
        },
        vm: VmConfig {
            cpus: args.vm.cpus,
            mem: args.vm.mem,
        },
        repo: None,
        command: vec![],
    };
    boot_linux_from(largs, None, &[])
}

/// The host directories shared into the engine VM, each at its own path.
///
/// `docker run -v $PWD:/app` resolves `$PWD` *inside the guest*, so a path the
/// VM cannot see silently becomes an empty directory in the container — the
/// single most confusing failure of a VM-backed Docker. `$HOME` covers what
/// Docker Desktop shares by default; `--mount` adds the rest.
fn docker_mounts(args: &DockerStartArgs) -> Result<Vec<String>> {
    let mut specs: Vec<String> = Vec::new();
    if !args.no_home {
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let home = home.to_string_lossy().into_owned();
            specs.push(format!("{home}:{home}"));
        }
    }
    for m in &args.mounts {
        // `--mount /path` is shorthand for the same path in the guest, which is
        // the only form that makes a copy-pasted `docker run -v` work.
        specs.push(if m.contains(':') {
            m.clone()
        } else {
            format!("{m}:{m}")
        });
    }
    // Validated here so a typo fails before a VM boots, not after.
    for spec in &specs {
        parse_mount(spec)?;
    }
    Ok(specs)
}

// ---------------------------------------------------------------------------
// branching a snapshot
// ---------------------------------------------------------------------------

/// `bsdkrun branch <snapshot|machine>` — boot a NEW machine from saved state.
///
/// The snapshot itself is never booted: every guest family clones its state
/// into the new machine first (`prepare_bsd_disk` for a disk, the rootfs clone
/// for Linux, an explicit copy for a unikernel's mounts), so branching twice
/// from one snapshot gives two independent machines and leaves the snapshot
/// pristine. That is what makes "try it and throw it away" cheap.
///
/// Naming a *machine* branches it as it is right now: a snapshot is taken
/// first and the branch boots from that. Nobody wanting a copy of a machine
/// should have to think about snapshots to get one — and the snapshot is
/// worth keeping either way, since it is what the branch diverged from.
pub(crate) fn cmd_branch(args: BranchArgs) -> Result<()> {
    let snap = resolve_branch_source(&args.snapshot)?;
    let payload = crate::commands::snapshot::snapshot_payload(&snap)?;

    let cpus = args.cpus.unwrap_or(snap.cpus.clamp(1, 255) as u8);
    let mem = args.mem.unwrap_or(snap.mem.max(64) as u32);
    let ports = branch_ports(&args, &snap);

    // The new machine records where it came from, so `snapshot` on the branch
    // carries the lineage forward and the UIs can draw the tree.
    db::set_origin_override(&snap.name);
    if let Some(name) = &args.name {
        names::set_override(name);
    }

    let net = NetConfig {
        no_net: false,
        ports,
        mac: None,
        network: None,
        name: args.name.clone(),
    };

    match payload {
        crate::commands::snapshot::Payload::Rootfs(rootfs) => {
            let largs = flavor_linux_args(
                snap.image.clone(),
                args.detach,
                cpus,
                mem,
                None,
                net.ports.clone(),
                vec![],
            );
            boot_linux_from(largs, Some(rootfs), &[])
        }
        crate::commands::snapshot::Payload::Disk(disk) => {
            let bargs = BsdArgs {
                version: None,
                firmware: None,
                force: false,
                attach_disk: vec![],
                disk_size: None,
                run: RunConfig {
                    detach: args.detach,
                    // Never in place: the branch gets a CoW clone of the
                    // snapshot's disk, so the snapshot stays restorable.
                    persist: false,
                    volume: None,
                },
                net,
                vm: VmConfig { cpus, mem },
                verbose: false,
                repo: None,
                command: vec![],
            };
            if snap.kind == "netbsd" {
                boot_netbsd_disk(bargs, Some(disk))
            } else {
                boot_freebsd_disk(bargs, Some(disk))
            }
        }
        crate::commands::snapshot::Payload::Unikernel { kernel, spec } => {
            // A unikernel's mounts are plain host directories shared over
            // virtio-fs — booting the snapshot's own copies would let the
            // branch write into the snapshot. Clone them into the machine dir
            // first, which means minting its id here (as `start` does) so the
            // copies land where the machine will actually look for them.
            let machine_id = id::short_id();
            let vdir = machine_dir_or_tmp(&machine_id);
            let mut volumes = Vec::with_capacity(spec.volumes.len());
            for (i, v) in spec.volumes.iter().enumerate() {
                let dst = vdir.join("volumes").join(i.to_string());
                std::fs::create_dir_all(dst.parent().unwrap())?;
                host::remove_dir_all_detached(&dst);
                host::clone_or_copy_tree(&v.host, &dst)
                    .with_context(|| format!("cloning mounted directory {}", v.host.display()))?;
                volumes.push(unikraft::Volume {
                    host: dst,
                    guest: v.guest.clone(),
                });
            }
            id::set_override(&machine_id);
            boot_unikraft_image(
                &kernel,
                &spec.cmdline,
                spec.initramfs.as_deref(),
                args.detach,
                net,
                VmConfig { cpus, mem },
                &volumes,
            )
        }
    }
}

/// What to branch from: a snapshot by name/id, or a machine — which is
/// snapshotted on the spot so the branch has something immutable to clone.
fn resolve_branch_source(key: &str) -> Result<db::SnapshotRow> {
    if let Some(snap) = db::Db::open()?.find_snapshot(key)? {
        return Ok(snap);
    }
    // Not a snapshot: if it names a machine, branch that machine's state now.
    // A machine lookup that also fails reports the snapshot error, since
    // `branch` is a snapshot verb and that is what the user most likely meant.
    let db = db::Db::open()?;
    let Ok(vm) = db.find_machine(key) else {
        anyhow::bail!("no such snapshot or machine: {key} (see `bsdkrun snapshots`)");
    };
    let label = vm.name.clone().unwrap_or_else(|| vm.id.clone());
    info!(machine = %label, "snapshotting before branching");
    crate::commands::snapshot::create(&vm.id, None, &format!("branch point for {label}"))
}

/// The port forwards a branch boots with: the ones asked for, none, or the
/// snapshot's — with any host port that is already taken remapped to a free
/// one, since the machine the snapshot came from is usually still running on it
/// and gvproxy would otherwise fail to bind (or, worse, the branch would look
/// up and answer on nobody's port).
fn branch_ports(args: &BranchArgs, snap: &db::SnapshotRow) -> Vec<PortForward> {
    if args.no_ports {
        return vec![];
    }
    if !args.ports.is_empty() {
        return args.ports.clone();
    }
    snap.ports
        .as_deref()
        .map(net::parse_ports)
        .unwrap_or_default()
        .into_iter()
        .map(|p| match net::free_host_port(p.bind, p.host) {
            Some(host) if host != p.host => {
                info!(
                    guest = p.guest,
                    was = p.host,
                    now = host,
                    "host port already in use — the branch takes a free one"
                );
                PortForward {
                    bind: p.bind,
                    host,
                    guest: p.guest,
                }
            }
            _ => p,
        })
        .collect()
}

/// `bsdkrun flavor run <name>` — boot a machine from a catalog/user flavor or a
/// saved snapshot. Provisioned flavors are built once (cached) then cloned.
pub(crate) fn cmd_flavor_run(args: FlavorRunArgs) -> Result<()> {
    let db = db::Db::open()?;

    // Optional `--repo` clones a repo into the machine after boot (cd on shell).
    let repo_argv = args
        .repo
        .as_deref()
        .and_then(repo_clone_argv)
        .unwrap_or_default();

    // A saved snapshot (from `commit`) wins over any catalog/user name.
    if let Some(f) = db.find_flavor(&args.name)? {
        // Normalize (old snapshots stored the boot mode: firmware/kernel).
        let osk = guest_os_kind(&f.kind, &f.base);
        if osk == "linux" {
            let rootfs = std::path::PathBuf::from(&f.path).join("rootfs");
            if !rootfs.exists() {
                anyhow::bail!("snapshot {:?} is missing its rootfs data", f.name);
            }
            let largs = flavor_linux_args(
                f.base.clone(),
                args.detach,
                args.vm.cpus,
                args.vm.mem,
                args.volume,
                args.ports,
                vec![],
            );
            return boot_linux_from(largs, Some(rootfs), &repo_argv);
        }

        // BSD snapshot: boot from its saved root disk (`disk.raw` / `disk.img`).
        let disk = ["disk.raw", "disk.img"]
            .iter()
            .map(|n| std::path::PathBuf::from(&f.path).join(n))
            .find(|p| p.exists())
            .ok_or_else(|| anyhow::anyhow!("snapshot {:?} is missing its disk data", f.name))?;
        let bargs = BsdArgs {
            version: None,
            firmware: None,
            force: false,
            attach_disk: vec![],
            disk_size: None,
            run: RunConfig {
                detach: args.detach,
                persist: false,
                volume: args.volume,
            },
            net: NetConfig {
                no_net: false,
                ports: args.ports,
                mac: None,
                network: None,
                name: None,
            },
            vm: VmConfig {
                cpus: args.vm.cpus,
                mem: args.vm.mem,
            },
            verbose: false,
            repo: None,
            command: repo_argv,
        };
        return if osk == "netbsd" {
            boot_netbsd_disk(bargs, Some(disk))
        } else {
            boot_freebsd_disk(bargs, Some(disk))
        };
    }

    // A Linux flavor (catalog or user): build-once-then-clone.
    if let Some(spec) = resolve_linux_flavor(&args.name) {
        let mut ports: Vec<PortForward> = args.ports;
        for p in &spec.ports {
            if let Ok(pf) = p.parse::<PortForward>() {
                ports.push(pf);
            }
        }
        let largs = flavor_linux_args(
            spec.image.clone(),
            args.detach,
            args.vm.cpus,
            args.vm.mem,
            args.volume,
            ports,
            spec.env.clone(),
        );
        // Provisioned flavors boot from a cached, pre-provisioned rootfs; plain
        // ones boot the base image directly.
        let has_provisioning = !spec.nix.is_empty() || !spec.provision.is_empty();
        if has_provisioning {
            let built = ensure_flavor_built(&spec, &args.name, args.vm.cpus, args.vm.mem)?;
            return boot_linux_from(largs, Some(built), &repo_argv);
        }
        return boot_linux_from(largs, None, &repo_argv);
    }

    // A BSD catalog flavor (no provisioning/cache — boots the bundled image).
    let Some(c) = flavors::find(&args.name) else {
        anyhow::bail!("no such flavor: {} (see `bsdkrun flavors`)", args.name);
    };
    let mut ports: Vec<PortForward> = args.ports;
    for p in c.ports {
        if let Ok(pf) = p.parse::<PortForward>() {
            ports.push(pf);
        }
    }
    let bargs = BsdArgs {
        version: None,
        firmware: None,
        force: false,
        attach_disk: vec![],
        disk_size: None,
        run: RunConfig {
            detach: args.detach,
            persist: false,
            volume: args.volume,
        },
        net: NetConfig {
            no_net: false,
            ports,
            mac: None,
            network: None,
            name: None,
        },
        vm: VmConfig {
            cpus: args.vm.cpus,
            mem: args.vm.mem,
        },
        verbose: false,
        repo: None,
        // On BSD the post-boot command IS the repo clone (if any).
        command: repo_argv,
    };
    match c.base {
        flavors::Base::Freebsd => boot_freebsd(bargs),
        flavors::Base::Netbsd => boot_netbsd(bargs),
        flavors::Base::Oci(_) => unreachable!("OCI flavors handled above"),
    }
}
