//! Wiring the host resolver to the built-in DNS responder — the one part of
//! machine domains that needs root, kept here so every privileged operation in
//! bsdkrun lives in a single module and runs as a single batched escalation.
//!
//! macOS: `/etc/resolver/<tld>` routes just that TLD to 127.0.0.1:<port>.
//! Linux: a systemd-resolved drop-in (`DNS=127.0.0.1:<port>`, `Domains=~<tld>`).
//!
//! Escalation policy (ported from reeve, which proved it): write directly when
//! permitted; otherwise on macOS use the native admin dialog when a GUI session
//! can host it, else interactive `sudo`; on Linux always interactive `sudo`,
//! printing the exact commands on failure so nothing is a dead end. A password
//! never touches this process — it goes into the OS dialog or straight to sudo.

use std::fs;
use std::process::Command;

use anyhow::{bail, Context, Result};

// ===========================================================================
// macOS: /etc/resolver/<tld> + mDNSResponder cache flush.
// ===========================================================================

/// Where the resolver wiring lives, for status output.
#[cfg(target_os = "macos")]
pub fn location(tld: &str) -> String {
    format!("/etc/resolver/{tld}")
}

/// Can the native macOS admin dialog appear? It needs a logged-in Aqua
/// session; over SSH or headless, osascript fails without ever prompting, so
/// the caller must fall back to terminal `sudo`. `/dev/console` is owned by
/// whoever is logged into the GUI — `root` at the login window, empty headless.
#[cfg(target_os = "macos")]
pub fn gui_escalation_available() -> bool {
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return false;
    }
    match Command::new("stat")
        .args(["-f", "%Su", "/dev/console"])
        .output()
    {
        Ok(o) => {
            let owner = String::from_utf8_lossy(&o.stdout);
            let owner = owner.trim();
            !owner.is_empty() && owner != "root"
        }
        Err(_) => false,
    }
}

/// Point the TLD at our responder: write `/etc/resolver/<tld>` and flush the
/// DNS cache, escalating the whole sequence as one admin action if needed.
#[cfg(target_os = "macos")]
pub fn setup(tld: &str, port: u16) -> Result<()> {
    let contents = format!("nameserver 127.0.0.1\nport {port}\n");

    // Fast path: already root / writable.
    if fs::create_dir_all("/etc/resolver").is_ok()
        && fs::write(format!("/etc/resolver/{tld}"), &contents).is_ok()
    {
        flush_cache();
        return Ok(());
    }

    let script = format!(
        "mkdir -p /etc/resolver && \
         printf 'nameserver 127.0.0.1\\nport {port}\\n' > /etc/resolver/{tld} && \
         dscacheutil -flushcache && killall -HUP mDNSResponder"
    );
    run_as_admin(&script)
        .with_context(|| format!("could not write /etc/resolver/{tld} (admin auth was needed)"))
}

/// Remove the resolver wiring for the TLD.
#[cfg(target_os = "macos")]
pub fn teardown(tld: &str) -> Result<()> {
    let path = format!("/etc/resolver/{tld}");
    if !std::path::Path::new(&path).exists() {
        return Ok(());
    }
    if fs::remove_file(&path).is_ok() {
        flush_cache();
        return Ok(());
    }
    let script = format!("rm -f {path} && dscacheutil -flushcache && killall -HUP mDNSResponder");
    run_as_admin(&script).with_context(|| format!("could not remove {path}"))
}

/// True when the resolver file exists and points at our responder's port —
/// guards against a stale or hand-edited file counting as configured.
#[cfg(target_os = "macos")]
pub fn ok(tld: &str, port: u16) -> bool {
    match fs::read_to_string(format!("/etc/resolver/{tld}")) {
        Ok(c) => c.contains("127.0.0.1") && c.contains(&format!("port {port}")),
        Err(_) => false,
    }
}

#[cfg(target_os = "macos")]
fn flush_cache() {
    let _ = Command::new("dscacheutil").arg("-flushcache").status();
}

/// Run a shell script as root: the native admin dialog when a GUI session can
/// host it, else interactive terminal sudo.
#[cfg(target_os = "macos")]
pub fn run_as_admin(shell_script: &str) -> Result<()> {
    if gui_escalation_available() {
        // Escape for an AppleScript double-quoted string; the shell's literal
        // `\n` must survive as `\\n` so AppleScript hands `\n` back.
        let escaped = shell_script.replace('\\', "\\\\").replace('"', "\\\"");
        let applescript = format!("do shell script \"{escaped}\" with administrator privileges");
        // Capture stdio — osascript echoes the script's output, which must not
        // leak into (and corrupt) the TUI's alternate screen.
        let out = Command::new("osascript")
            .arg("-e")
            .arg(&applescript)
            .output()
            .context("running osascript for admin escalation")?;
        if !out.status.success() {
            bail!("admin authorization was cancelled or failed");
        }
        Ok(())
    } else {
        // stdio inherited so the sudo prompt is visible; the caller must have
        // freed the terminal (the TUI suspends itself first).
        let status = Command::new("sudo")
            .arg("sh")
            .arg("-c")
            .arg(shell_script)
            .status()
            .context("running sudo for admin escalation")?;
        if !status.success() {
            bail!("admin authorization was cancelled or failed");
        }
        Ok(())
    }
}

// ===========================================================================
// Linux: systemd-resolved drop-in + resolvectl cache flush.
// ===========================================================================

/// The drop-in bsdkrun owns. All TLDs share it.
#[cfg(target_os = "linux")]
const RESOLVED_DROPIN: &str = "/etc/systemd/resolved.conf.d/bsdkrun.conf";

#[cfg(target_os = "linux")]
pub fn location(_tld: &str) -> String {
    RESOLVED_DROPIN.to_string()
}

/// Linux has no GUI admin dialog; escalation is always interactive sudo. The
/// TUI uses this to know it must suspend the dashboard first.
#[cfg(target_os = "linux")]
pub fn gui_escalation_available() -> bool {
    false
}

/// `DNS=` with a port needs systemd >= 246 (Ubuntu 24.04 ships 255); `~<tld>`
/// is a routing-only domain, so only that TLD is sent to us.
#[cfg(target_os = "linux")]
fn dropin(tld: &str, port: u16) -> String {
    format!(
        "# Generated by bsdkrun — do not edit by hand.\n\
         [Resolve]\n\
         DNS=127.0.0.1:{port}\n\
         Domains=~{tld}\n"
    )
}

#[cfg(target_os = "linux")]
pub fn setup(tld: &str, port: u16) -> Result<()> {
    if !resolved_available() {
        bail!(
            "systemd-resolved is not available on this host, so bsdkrun can't wire up \
             `.{tld}` resolution automatically. Point your resolver at 127.0.0.1:{port} \
             for that domain manually (or run a local forwarder). Environments like \
             OrbStack manage DNS themselves."
        );
    }

    let contents = dropin(tld, port);
    let dir = std::path::Path::new(RESOLVED_DROPIN).parent().unwrap();

    // Fast path: already root / writable.
    if fs::create_dir_all(dir).is_ok() && fs::write(RESOLVED_DROPIN, &contents).is_ok() {
        let _ = Command::new("systemctl")
            .args(["restart", "systemd-resolved"])
            .status();
        let _ = Command::new("resolvectl").arg("flush-caches").status();
        return Ok(());
    }

    // One sudo action; the restart/flush are best-effort so a transient
    // restart hiccup doesn't report failure when the file did land.
    let printf_body = contents.replace('\\', "\\\\").replace('\'', "'\\''");
    let script = format!(
        "mkdir -p {dir} && printf '%s' '{printf_body}' > {file} && \
         {{ systemctl restart systemd-resolved || true; }} && \
         {{ resolvectl flush-caches || true; }}",
        dir = dir.display(),
        file = RESOLVED_DROPIN,
    );
    run_as_admin(&script).context("could not write the systemd-resolved drop-in (sudo was needed)")
}

#[cfg(target_os = "linux")]
pub fn teardown(_tld: &str) -> Result<()> {
    if !std::path::Path::new(RESOLVED_DROPIN).exists() {
        return Ok(());
    }
    if fs::remove_file(RESOLVED_DROPIN).is_ok() {
        let _ = Command::new("systemctl")
            .args(["restart", "systemd-resolved"])
            .status();
        return Ok(());
    }
    let script =
        format!("rm -f {RESOLVED_DROPIN} && {{ systemctl restart systemd-resolved || true; }}");
    run_as_admin(&script).with_context(|| format!("could not remove {RESOLVED_DROPIN}"))
}

/// True when the drop-in targets our port and routes this TLD.
#[cfg(target_os = "linux")]
pub fn ok(tld: &str, port: u16) -> bool {
    match fs::read_to_string(RESOLVED_DROPIN) {
        Ok(c) => c.contains(&format!("127.0.0.1:{port}")) && c.contains(&format!("~{tld}")),
        Err(_) => false,
    }
}

/// Is systemd-resolved present and not masked? `is-enabled` prints `masked`
/// for a masked unit and errors for an unknown one.
#[cfg(target_os = "linux")]
fn resolved_available() -> bool {
    match Command::new("systemctl")
        .args(["is-enabled", "systemd-resolved"])
        .output()
    {
        Ok(o) => {
            let state = String::from_utf8_lossy(&o.stdout);
            let state = state.trim();
            !state.is_empty() && state != "masked"
        }
        Err(_) => false,
    }
}

/// Run a shell script as root via interactive sudo, printing the manual
/// command on failure so declining is never a dead end.
#[cfg(target_os = "linux")]
pub fn run_as_admin(shell_script: &str) -> Result<()> {
    let status = Command::new("sudo")
        .arg("sh")
        .arg("-c")
        .arg(shell_script)
        .status()
        .context("running sudo for admin escalation")?;
    if !status.success() {
        bail!(
            "admin authorization failed. Run this manually:\n  sudo sh -c '{}'",
            shell_script
        );
    }
    Ok(())
}
