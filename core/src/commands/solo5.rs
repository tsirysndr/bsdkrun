//! `bsdkrun solo5` — run a Solo5 unikernel (MirageOS and friends).
//!
//! Every other guest in bsdkrun runs *inside* this process, through libkrun.
//! This one does not. A Solo5 unikernel is run by its tender, `solo5-hvt`,
//! which is itself the hypervisor front end — Hypervisor.framework on macOS
//! (via the HVF backend the pinned `library/solo5` fork adds) and KVM on
//! Linux. libkrun is not in the picture at all.
//!
//! So this module is a supervisor rather than a boot path: it extracts the
//! embedded tender, works out the tender's command line from what the
//! unikernel declares (see [`crate::solo5`]), wires networking, spawns the
//! tender and waits on it — recording the machine in the same database as
//! everything else, so `ps`, `logs` and `stop` do not have to care that this
//! guest lives in another process.
//!
//! Three consequences of that split are load-bearing:
//!
//!   * **The tender must stay signed.** On macOS, Hypervisor.framework refuses
//!     to create a VM for a process without the `com.apple.security.hypervisor`
//!     entitlement, and an entitlement only counts as part of a code
//!     signature. The tender is signed when it is built; extracting it here
//!     re-signs it, because a bsdkrun that is itself entitled can only run
//!     children that are validly signed.
//!   * **The tender must be killed with us.** `tty::track_child` hands its pid
//!     to the signal handler; without that, Ctrl-C reaps bsdkrun and leaves a
//!     VM running that nothing knows the id of.
//!   * **Networking is ours to hold.** libkrun opens gvproxy's socket itself
//!     for other guests; here bsdkrun opens it and passes the descriptor
//!     (`--net:NAME=@<fd>`), which is the only way to attach a network on
//!     macOS at all — there is no TAP device to hand over.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use rust_embed::RustEmbed;
use tracing::{info, warn};

use crate::cli::Solo5Args;
use crate::net::Gvproxy;
use crate::{db, fetch, id, net, solo5, tty};

use super::{basename, machine_dir_or_tmp};

/// The tender built from `library/solo5` by `core/build.rs`. Empty when that
/// build did not happen — see `build.rs::ensure_solo5_tender`.
#[derive(RustEmbed)]
#[folder = "src/solo5-bin"]
struct TenderBinary;

const TENDER: &str = "solo5-hvt";

/// The entitlement the tender needs on macOS, restated here because the
/// extracted copy is re-signed rather than trusted to arrive signed.
#[cfg(target_os = "macos")]
const TENDER_ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.hypervisor</key>
    <true/>
</dict>
</plist>
"#;

/// Run a Solo5 unikernel.
pub(crate) fn cmd_solo5(args: Solo5Args) -> Result<()> {
    let kernel = solo5::resolve(&args.path)?;
    let manifest = solo5::read_manifest(&kernel)?;
    let tender = tender_binary(args.tender.as_deref())?;

    // The tender runs one vCPU and has no flag to ask for more. Say so rather
    // than accepting `--cpus 4` and quietly running one.
    if args.vm.cpus > 1 {
        warn!(
            cpus = args.vm.cpus,
            "the Solo5 hvt tender runs a single vCPU; ignoring --cpus"
        );
    }

    let blocks = resolve_blocks(&manifest, &args.block)?;
    if manifest.nets.is_empty() && !args.net.ports.is_empty() {
        bail!(
            "{} declares no network device, so there is nothing to forward a port to. \
             A MirageOS unikernel gets one from a `network` device in its config.ml.",
            basename(&kernel)
        );
    }

    let machine_id = id::next_machine_id();
    let vdir = machine_dir_or_tmp(&machine_id);
    let image = basename(&kernel);
    info!(
        id = %machine_id,
        image = %image,
        nets = ?manifest.nets,
        blocks = ?manifest.blocks,
        "running Solo5 unikernel"
    );

    let run = Run {
        tender,
        kernel,
        manifest,
        blocks,
        args,
        vdir: vdir.clone(),
    };

    if run.args.detach {
        run_detached(&machine_id, &image, run)
    } else {
        run_foreground(&machine_id, &image, run)
    }
}

/// Everything a launch needs, gathered before any process is forked so a bad
/// argument is reported by the command the user typed rather than by a
/// background child writing to a log they have not been told about.
struct Run {
    tender: PathBuf,
    kernel: PathBuf,
    manifest: solo5::Manifest,
    /// Declared block device name -> backing file, in manifest order.
    blocks: Vec<(String, PathBuf)>,
    args: Solo5Args,
    vdir: PathBuf,
}

fn run_foreground(machine_id: &str, image: &str, run: Run) -> Result<()> {
    db::record_machine(
        machine_id,
        &super::boot::machine_name(),
        image,
        "solo5",
        "",
        "running",
        Some(std::process::id() as i64),
        false,
        1,
        run.args.vm.mem as i64,
        &run.vdir.to_string_lossy(),
        None,
        net::format_ports(&run.args.net.ports).as_deref(),
    );
    tty::install();
    let code = launch(&run)?;
    tty::restore_stdin_termios();
    info!(code, "unikernel exited");
    db::update_machine_status(machine_id, "exited", Some(code as i64));
    std::process::exit(code);
}

/// Run in the background and print the machine id, like `docker run -d`.
///
/// Mirrors `boot::run_detached`: fork, record the child, and let the child
/// wire its console to the per-machine PTY broker before starting the guest.
/// The difference is what the child does at the end — it spawns the tender and
/// waits, instead of entering a VM in-process.
fn run_detached(machine_id: &str, image: &str, run: Run) -> Result<()> {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        bail!("fork failed: {}", std::io::Error::last_os_error());
    }
    if pid > 0 {
        db::record_machine(
            machine_id,
            &super::boot::machine_name(),
            image,
            "solo5",
            "",
            "running",
            Some(pid as i64),
            true,
            1,
            run.args.vm.mem as i64,
            &run.vdir.to_string_lossy(),
            None,
            net::format_ports(&run.args.net.ports).as_deref(),
        );
        // Parent side only — the child becomes the tender's supervisor.
        crate::domains::sync_if_enabled();
        println!("{machine_id}");
        return Ok(());
    }

    // Child.
    unsafe { libc::setsid() };
    if let Ok(logf) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(run.vdir.join("bsdkrun.log"))
    {
        unsafe { libc::dup2(logf.as_raw_fd(), 2) };
        std::mem::forget(logf); // fd 2 owns it now
    }

    let result = (|| -> Result<i32> {
        let console_fd = crate::console::setup_detached(&run.vdir)?;
        unsafe {
            libc::dup2(console_fd, 0);
            libc::dup2(console_fd, 1);
        }
        // So a `stop` (SIGTERM) reaches the tender and gvproxy, not just us.
        tty::install();
        let code = launch(&run)?;
        db::update_machine_status(machine_id, "exited", Some(code as i64));
        Ok(code)
    })();

    let code = match result {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("detached unikernel failed: {e:#}");
            db::update_machine_status(machine_id, "exited", Some(1));
            1
        }
    };
    unsafe { libc::_exit(code) };
}

/// Bring up networking, spawn the tender, and wait for it. Returns its exit
/// status.
fn launch(run: &Run) -> Result<i32> {
    // Held for the tender's lifetime: dropping either tears the network down.
    // The sockets are ours rather than the tender's because a unix datagram
    // socket has to be bound on this side to receive anything back.
    let (gvproxy, sockets) = setup_networking(run)?;

    let mut cmd = Command::new(&run.tender);
    cmd.arg(format!("--mem={}", run.args.vm.mem));
    for (i, (name, fd)) in run.manifest.nets.iter().zip(sockets.iter()).enumerate() {
        cmd.arg(format!("--net:{name}=@{}", fd.as_raw_fd()));
        // Fix the MAC of the device gvproxy serves. Left alone, the tender
        // generates a random one per boot, gvproxy's DHCP leases it whatever
        // address is next free — .3, .4, … — and `--port`, which forwards to
        // the fixed guest address, points at nobody. Every other guest here
        // gets its MAC from the same place for the same reason.
        if i == 0 && gvproxy.is_some() {
            cmd.arg(format!("--net-mac:{name}={}", format_mac(guest_mac(run)?)));
        }
    }
    for (name, path) in &run.blocks {
        cmd.arg(format!("--block:{name}={}", path.display()));
    }
    // `--` stops the tender's own option parsing: without it a unikernel
    // argument that starts with a dash (MirageOS takes plenty, e.g.
    // `--ipv4=...`) is read as a tender option and rejected.
    cmd.arg("--").arg(&run.kernel).args(&run.args.args);

    // The sockets were created by this process, so they carry FD_CLOEXEC and
    // would be closed by the exec that follows — leaving the tender pointed at
    // descriptors that are not there. Clearing it in the child, after fork and
    // before exec, is the one place where doing so cannot leak them into any
    // other process this one spawns.
    let fds: Vec<i32> = sockets.iter().map(|s| s.as_raw_fd()).collect();
    unsafe {
        cmd.pre_exec(move || {
            for fd in &fds {
                // F_SETFD with no flags clears FD_CLOEXEC. Async-signal-safe,
                // which anything between fork and exec has to be.
                if libc::fcntl(*fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("running the Solo5 tender {}", run.tender.display()))?;
    tty::track_child(child.id() as i32);

    let status = child.wait().context("waiting for the Solo5 tender")?;
    tty::untrack_child();
    drop(sockets);
    drop(gvproxy);

    // A tender killed by a signal has no exit code of its own; report it the
    // way a shell does, so `ps` shows something other than a bare 0.
    Ok(exit_code(status))
}

/// The MAC to give the guest's gvproxy-facing device: `--mac` if asked for,
/// else the same default every other bsdkrun guest uses.
fn guest_mac(run: &Run) -> Result<[u8; 6]> {
    match &run.args.net.mac {
        Some(s) => net::parse_mac(s).context("parsing --mac"),
        None => Ok(net::DEFAULT_MAC),
    }
}

fn format_mac(mac: [u8; 6]) -> String {
    mac.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Attach one datagram socket per declared network device.
///
/// The first goes to gvproxy, which NATs it out to the host and holds the
/// `--port` forwards. Any others — rare, but a unikernel may declare several —
/// get a socket that is bound and connected to nothing, so the device exists
/// and reads block forever. That is deliberate: the tender refuses to boot
/// unless *every* declared device is attached, so an unattachable second
/// network would otherwise make the unikernel unrunnable rather than merely
/// half-connected.
fn setup_networking(run: &Run) -> Result<(Option<Gvproxy>, Vec<UnixDatagram>)> {
    let cfg = &run.args.net;
    if run.manifest.nets.is_empty() {
        return Ok((None, Vec::new()));
    }
    if cfg.no_net {
        if !cfg.ports.is_empty() {
            bail!("--port cannot be combined with --no-net");
        }
        info!("networking disabled (--no-net); the unikernel's devices are attached to nothing");
        return Ok((None, isolated_sockets(run.manifest.nets.len())?));
    }
    if let Err(e) = net::locate() {
        if !cfg.ports.is_empty() {
            return Err(e).context("--port requires gvproxy");
        }
        warn!("networking disabled: {e:#}");
        return Ok((None, isolated_sockets(run.manifest.nets.len())?));
    }

    // Where this end of each network binds. Inside the machine's own directory
    // so concurrent unikernels never collide, and short enough to stay under
    // the 104-byte limit macOS puts on a unix socket path.
    let local = |i: usize| run.vdir.join(format!("net{i}.sock"));
    std::fs::create_dir_all(&run.vdir)
        .with_context(|| format!("creating {}", run.vdir.display()))?;

    // A shared `--network` switch, joined the same way libkrun guests join it:
    // bridge into the running network's gvproxy rather than starting our own.
    if let Some(control) = std::env::var_os("BSDKRUN_NET_CONTROL").filter(|s| !s.is_empty()) {
        let control = PathBuf::from(control);
        let bridge = run.vdir.join("net-bridge.sock");
        net::start_network_bridge(&bridge, &control).context("bridging into the shared network")?;
        let mut socks = vec![net::vfkit_connect(&bridge, &local(0))?];
        socks.extend(isolated_sockets(run.manifest.nets.len() - 1)?);
        info!("joined shared network");
        return Ok((None, socks));
    }

    let gvproxy = Gvproxy::spawn(&cfg.ports).context("starting gvproxy networking")?;
    let mut socks = vec![net::vfkit_connect(&gvproxy.vfkit_socket, &local(0))?];
    socks.extend(isolated_sockets(run.manifest.nets.len() - 1)?);
    Ok((Some(gvproxy), socks))
}

/// `n` sockets attached to nothing — a NIC the guest can see and send on, with
/// no peer to answer.
fn isolated_sockets(n: usize) -> Result<Vec<UnixDatagram>> {
    (0..n)
        .map(|_| {
            let (ours, theirs) =
                UnixDatagram::pair().context("creating an isolated network device")?;
            // Our end is dropped immediately: what matters is that the tender's
            // end is a valid socket, not that anything is listening.
            drop(ours);
            Ok(theirs)
        })
        .collect()
}

/// Match the `--block NAME=FILE` arguments against the block devices the
/// unikernel declares.
///
/// The tender will not boot with a declared device left unattached, so the
/// useful thing to do with a missing one is name it here — before a VM is
/// started — rather than let the tender fail with its own message after
/// gvproxy is already up.
fn resolve_blocks(manifest: &solo5::Manifest, specs: &[String]) -> Result<Vec<(String, PathBuf)>> {
    let mut given: Vec<(Option<String>, PathBuf)> = Vec::new();
    for spec in specs {
        match spec.split_once('=') {
            Some((name, path)) => given.push((Some(name.to_string()), PathBuf::from(path))),
            // A bare path is allowed only when there is no ambiguity about
            // which device it is for.
            None => given.push((None, PathBuf::from(spec))),
        }
    }
    let unnamed = given.iter().filter(|(n, _)| n.is_none()).count();
    if unnamed > 0 && (manifest.blocks.len() != 1 || given.len() != 1) {
        bail!(
            "--block needs a device name (NAME=FILE) when the unikernel declares {} block \
             devices: {}",
            manifest.blocks.len(),
            manifest.blocks.join(", ")
        );
    }

    // Reject a name the unikernel never declared *before* reporting anything
    // as missing. The two errors describe the same typo — `--block stroage=…`
    // is both an unknown device and a `storage` left unattached — and only
    // this one names the word that is actually wrong.
    for (name, path) in &given {
        let named = name.as_deref().unwrap_or_default();
        if !named.is_empty() && !manifest.blocks.iter().any(|b| b == named) {
            bail!(
                "--block {named}={} names a device this unikernel does not declare (it declares: {})",
                path.display(),
                if manifest.blocks.is_empty() {
                    "none".to_string()
                } else {
                    manifest.blocks.join(", ")
                }
            );
        }
    }

    let mut resolved = Vec::new();
    for name in &manifest.blocks {
        let path = given
            .iter()
            .find(|(n, _)| n.as_deref() == Some(name.as_str()) || n.is_none())
            .map(|(_, p)| p.clone());
        let Some(path) = path else {
            bail!(
                "the unikernel declares a block device named `{name}` — attach one with \
                 `--block {name}=<file>`"
            );
        };
        if !path.exists() {
            bail!("--block {name}={} does not exist", path.display());
        }
        db::record_disk(&path.to_string_lossy());
        resolved.push((name.clone(), path));
    }
    Ok(resolved)
}

/// Extract the embedded tender (or use `--tender`), returning a path to run.
fn tender_binary(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        if !p.exists() {
            bail!("--tender {} does not exist", p.display());
        }
        return Ok(p.to_path_buf());
    }

    let embedded = TenderBinary::get(TENDER).filter(|f| !f.data.is_empty());
    let Some(embedded) = embedded else {
        bail!(
            "this bsdkrun binary was built without solo5 support.\n\
             Run `git submodule update --init library/solo5` and `cargo build --release` \
             again to enable `bsdkrun solo5`, or point at an existing tender with \
             `--tender /path/to/solo5-hvt`."
        );
    };

    let dir = fetch::cache_dir()?.join("solo5").join(crate::VERSION);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let bin = dir.join(TENDER);
    // Always rewrite: the bytes come from this binary, so there is no staleness
    // window worth guarding against and a memcpy is cheaper than the check.
    std::fs::write(&bin, &embedded.data).with_context(|| format!("writing {}", bin.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod +x {}", bin.display()))?;
    }
    #[cfg(target_os = "macos")]
    sign_tender(&bin)?;
    Ok(bin)
}

/// Re-sign the extracted tender with the hypervisor entitlement.
///
/// The signature applied when it was built travels inside the Mach-O and would
/// normally survive the copy — but re-signing costs milliseconds and removes a
/// whole class of failure that reports itself terribly. Without a valid
/// signature the tender is SIGKILLed before `main()` with no crash report
/// naming the cause; without the entitlement it reaches `main()` and fails at
/// `hv_vm_create` instead, which reads like a hypervisor problem rather than a
/// signing one.
#[cfg(target_os = "macos")]
fn sign_tender(bin: &Path) -> Result<()> {
    let plist = bin.with_extension("entitlements");
    std::fs::write(&plist, TENDER_ENTITLEMENTS)
        .with_context(|| format!("writing {}", plist.display()))?;
    // `output()`, not `status()`: codesign says "replacing existing signature"
    // on every run, which would land in the middle of the guest's console.
    let out = Command::new("codesign")
        .args(["--force", "-s", "-", "--entitlements"])
        .arg(&plist)
        .arg(bin)
        .output()
        .with_context(|| format!("running codesign on {}", bin.display()))?;
    if !out.status.success() {
        bail!(
            "codesign {} failed: {}\n{}",
            bin.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(blocks: &[&str]) -> solo5::Manifest {
        solo5::Manifest {
            nets: Vec::new(),
            blocks: blocks.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn a_declared_block_device_must_be_attached() {
        let err = resolve_blocks(&manifest(&["storage"]), &[]).unwrap_err();
        assert!(
            err.to_string().contains("--block storage="),
            "error should name the flag to fix it: {err}"
        );
    }

    #[test]
    fn a_bare_path_is_ambiguous_with_two_devices() {
        let err = resolve_blocks(&manifest(&["a", "b"]), &["disk.img".into()]).unwrap_err();
        assert!(err.to_string().contains("NAME=FILE"), "{err}");
    }

    #[test]
    fn an_undeclared_device_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("disk.img");
        std::fs::write(&img, b"").unwrap();
        let spec = format!("nope={}", img.display());
        let err = resolve_blocks(&manifest(&["storage"]), &[spec]).unwrap_err();
        assert!(err.to_string().contains("does not declare"), "{err}");
    }
}
