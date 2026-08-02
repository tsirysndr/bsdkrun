//! systemd management for Linux guests (no-op module elsewhere — main.rs only
//! wires it on Linux builds).
//!
//! OCI images boot with bsdkrun's tiny generated /init as PID 1 — fine for
//! `docker run`-style workloads, but no services, no journal, no timers. This
//! turns a guest into a full systemd system:
//!
//!   bsdkrun-agent systemd setup     install systemd if missing, write + enable
//!                                   the agent unit, and mark the rootfs so the
//!                                   next boot execs systemd as PID 1
//!   bsdkrun-agent systemd status    PID 1 / installed / marker state
//!   bsdkrun-agent systemd disable   remove the marker (next boot = plain init)
//!
//! The handoff itself lives in bsdkrun's generated /init: when the marker file
//! exists and a systemd binary is present, it execs systemd instead of running
//! the entrypoint. The agent is then started by the unit written here (the
//! init deliberately does NOT pre-start it in that case — one agent, owned by
//! systemd). Persistence: package installs + the marker live in the rootfs, so
//! boot the machine on a volume (`-v NAME`) to keep systemd across machines.
//!
//! Alpine note: there is no systemd for Alpine (musl/OpenRC world) — setup
//! fails with a clear message instead of half-configuring something.

use std::process::Command;

use crate::util::{find_bin, run_cmd};

/// Marker checked by bsdkrun's generated /init before handing over PID 1.
pub const MARKER: &str = "/etc/bsdkrun-systemd";

const UNIT_PATH: &str = "/etc/systemd/system/bsdkrun-agent.service";
const WANTS_DIR: &str = "/etc/systemd/system/multi-user.target.wants";

/// Locations distros put the real systemd binary.
const SYSTEMD_BINS: &[&str] = &["/lib/systemd/systemd", "/usr/lib/systemd/systemd"];

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("setup") => setup(),
        Some("status") => status(),
        Some("disable") => disable(),
        _ => {
            eprintln!(
                "usage: bsdkrun-agent systemd <setup|status|disable>\n\
                 \n\
                 setup     install systemd if needed + agent unit; next boot runs systemd as PID 1\n\
                 status    show PID 1 / installed / marker state\n\
                 disable   remove the marker; next boot uses the plain bsdkrun init"
            );
            2
        }
    }
}

/// systemd is PID 1 right now iff /run/systemd/system exists (the documented
/// sd_booted() check).
fn booted_with_systemd() -> bool {
    std::path::Path::new("/run/systemd/system").is_dir()
}

fn systemd_binary() -> Option<&'static str> {
    SYSTEMD_BINS
        .iter()
        .copied()
        .find(|p| std::path::Path::new(p).exists())
}

fn setup() -> i32 {
    if booted_with_systemd() {
        println!("systemd is already PID 1 — nothing to do");
        return 0;
    }

    // 1. Install if missing.
    if systemd_binary().is_none() {
        if find_bin("apk").is_some() {
            eprintln!(
                "this guest is Alpine — there is no systemd for Alpine (it uses OpenRC).\n\
                 Use a debian/ubuntu/fedora image for a systemd guest."
            );
            return 1;
        }
        let code = if find_bin("apt-get").is_some() {
            let c = run_cmd(Command::new("apt-get").args(["update", "-qq"]));
            if c != 0 {
                c
            } else {
                // systemd-sysv provides /sbin/init and the expected symlinks;
                // dbus is what makes systemctl usable once booted.
                run_cmd(
                    Command::new("apt-get")
                        .args(["install", "-y", "-qq", "systemd", "systemd-sysv", "dbus"])
                        .env("DEBIAN_FRONTEND", "noninteractive"),
                )
            }
        } else if find_bin("dnf").is_some() {
            run_cmd(Command::new("dnf").args(["install", "-y", "systemd"]))
        } else {
            eprintln!("no known package manager (apt-get/dnf) to install systemd");
            1
        };
        if code != 0 {
            return code;
        }
        if systemd_binary().is_none() {
            eprintln!("install ran but no systemd binary found afterwards");
            return 1;
        }
    }

    // 2. The agent unit — systemd owns the agent after the handoff (the
    //    generated init skips its own agent spawn when the marker is set).
    let unit = "[Unit]\n\
                Description=bsdkrun in-guest exec agent\n\
                After=network.target\n\
                \n\
                [Service]\n\
                ExecStart=/sbin/bsdkrun-agent\n\
                Restart=always\n\
                RestartSec=1\n\
                \n\
                [Install]\n\
                WantedBy=multi-user.target\n";
    if std::fs::write(UNIT_PATH, unit).is_err() {
        eprintln!("cannot write {UNIT_PATH}");
        return 1;
    }
    // Enable by symlink — systemctl isn't usable while systemd isn't PID 1.
    let _ = std::fs::create_dir_all(WANTS_DIR);
    let link = format!("{WANTS_DIR}/bsdkrun-agent.service");
    let _ = std::fs::remove_file(&link);
    if std::os::unix::fs::symlink(UNIT_PATH, &link).is_err() {
        eprintln!("cannot enable the agent unit ({link})");
        return 1;
    }

    // 3. An empty machine-id lets systemd finish first-boot setup itself.
    if !std::path::Path::new("/etc/machine-id").exists() {
        let _ = std::fs::write("/etc/machine-id", "");
    }

    // 4. The marker the generated /init checks.
    if std::fs::write(MARKER, "").is_err() {
        eprintln!("cannot write {MARKER}");
        return 1;
    }

    println!(
        "systemd configured ({}). Restart the machine to boot it as PID 1 —\n\
         boot on a volume (-v NAME) so the rootfs (packages + marker) persists.",
        systemd_binary().unwrap_or("?")
    );
    0
}

fn status() -> i32 {
    println!(
        "PID 1:     {}",
        if booted_with_systemd() {
            "systemd"
        } else {
            "bsdkrun init"
        }
    );
    println!(
        "installed: {}",
        systemd_binary().unwrap_or("no")
    );
    println!(
        "marker:    {}",
        if std::path::Path::new(MARKER).exists() {
            "set (next boot uses systemd)"
        } else {
            "not set"
        }
    );
    0
}

fn disable() -> i32 {
    match std::fs::remove_file(MARKER) {
        Ok(()) => {
            println!("marker removed — next boot uses the plain bsdkrun init");
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("marker was not set");
            0
        }
        Err(e) => {
            eprintln!("cannot remove {MARKER}: {e}");
            1
        }
    }
}
