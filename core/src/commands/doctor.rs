//! `bsdkrun doctor` — check that this host can actually run machines, and say
//! what to do about it when it can't.
//!
//! Everything here is a thing that has, at some point, failed in a way that did
//! not name itself: a missing `curl` surfaces as `No such file or directory`
//! attached to an image pull, an unsigned binary as a bare `EINVAL` from
//! `krun_start_enter`, a case-insensitive store as a nix build that cannot
//! create a directory that already exists. One command, one place to look.

use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
use serde::Serialize;

/// How a single check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum State {
    /// Working.
    Ok,
    /// Works, but something will be missing or slower.
    Warn,
    /// Machines will not run until this is fixed.
    Fail,
}

impl State {
    fn marker(self) -> &'static str {
        match self {
            State::Ok => "ok  ",
            State::Warn => "warn",
            State::Fail => "FAIL",
        }
    }
}

#[derive(Debug, Serialize)]
struct Check {
    name: &'static str,
    state: State,
    detail: String,
    /// What to do about it. Only set when there is something to do.
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<String>,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Check {
        Check {
            name,
            state: State::Ok,
            detail: detail.into(),
            fix: None,
        }
    }

    fn warn(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Check {
        Check {
            name,
            state: State::Warn,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Check {
        Check {
            name,
            state: State::Fail,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

/// Run every check and report. Exits 1 if anything failed, so CI can gate on it.
pub(crate) fn cmd_doctor(json: bool) -> Result<()> {
    let checks = gather();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": !checks.iter().any(|c| c.state == State::Fail),
                "version": crate::VERSION,
                "checks": checks,
            }))?
        );
    } else {
        println!("bsdkrun {} on {}", crate::VERSION, platform());
        println!();
        let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
        for c in &checks {
            println!("[{}] {:<width$}  {}", c.state.marker(), c.name, c.detail);
            if let Some(fix) = &c.fix {
                for line in fix.lines() {
                    println!("       {:<width$}  → {line}", "");
                }
            }
        }
        println!();
        let failed = checks.iter().filter(|c| c.state == State::Fail).count();
        let warned = checks.iter().filter(|c| c.state == State::Warn).count();
        if failed == 0 && warned == 0 {
            println!("all good.");
        } else {
            println!("{failed} failing, {warned} to look at.");
        }
    }

    if checks.iter().any(|c| c.state == State::Fail) {
        std::process::exit(1);
    }
    Ok(())
}

fn gather() -> Vec<Check> {
    let mut checks = vec![
        // The host tools bsdkrun shells out to instead of linking. Missing curl
        // is the one that reads worst: it fails inside an image pull, so it
        // looks like the registry is unreachable.
        tool(
            "curl",
            true,
            "every image pull and download goes through it",
        ),
        tool("tar", true, "unpacking image layers, `cp -r`, and `cache`"),
        tool(
            "gzip",
            false,
            "not required — archives are compressed in-process",
        ),
    ];
    checks.push(hypervisor());
    #[cfg(target_os = "macos")]
    checks.push(signature());
    checks.push(networking());
    checks.push(writable("state directory", crate::db::state_dir()));
    checks.push(writable("cache directory", crate::fetch::cache_dir()));
    #[cfg(target_os = "macos")]
    checks.push(store());
    checks.push(cache_backend());
    checks
}

fn platform() -> String {
    let arch = crate::host::Arch::current()
        .map(|a| a.slug().to_string())
        .unwrap_or_else(|_| "unknown".into());
    format!("{}/{arch}", std::env::consts::OS)
}

/// Is `program` on PATH? `required` decides whether its absence is fatal.
fn tool(program: &'static str, required: bool, why: &str) -> Check {
    match which(program) {
        Some(path) => Check::ok(program, path.display().to_string()),
        None if required => Check::fail(
            program,
            format!("not on PATH — {why}"),
            format!("brew install {program} (macOS), or your distribution's package"),
        ),
        None => Check::warn(program, format!("not on PATH — {why}"), "optional"),
    }
}

fn which(program: &str) -> Option<PathBuf> {
    let out = Command::new("/usr/bin/which").arg(program).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Can we actually create a VM? This is `bsdkrun probe`, folded in — it is the
/// check that subsumes libkrun linking, the hypervisor, and (on macOS) whether
/// the signature took.
fn hypervisor() -> Check {
    #[cfg(feature = "boot")]
    {
        if let Err(e) = crate::host::check_kvm() {
            return Check::fail(
                "hypervisor",
                format!("{e:#}"),
                "on Linux, load kvm and add yourself to the kvm group",
            );
        }
        match crate::krun::Ctx::new().and_then(|ctx| ctx.set_vm_config(1, 256).map(|_| ())) {
            Ok(()) => Check::ok("hypervisor", "libkrun linked; a VM context was created"),
            Err(e) => Check::fail(
                "hypervisor",
                format!("{e:#}"),
                #[cfg(target_os = "macos")]
                "an EINVAL here is almost always a stripped code signature — see the next line",
                #[cfg(not(target_os = "macos"))]
                "check that /dev/kvm exists and is accessible",
            ),
        }
    }
    #[cfg(not(feature = "boot"))]
    Check::warn(
        "hypervisor",
        "this build cannot start machines (compiled without `boot`)",
        "use the bsdkrun CLI binary rather than a daemon-only build",
    )
}

/// macOS refuses `hv_vm_create` to a process without
/// `com.apple.security.hypervisor`, and an entitlement only counts inside a
/// signature — which `cargo build` strips on every rebuild.
#[cfg(target_os = "macos")]
fn signature() -> Check {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            return Check::warn(
                "code signature",
                format!("cannot locate this binary: {e}"),
                "",
            )
        }
    };
    let out = Command::new("codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(&exe)
        .output();
    match out {
        Ok(o)
            if String::from_utf8_lossy(&o.stdout).contains("com.apple.security.hypervisor")
                || String::from_utf8_lossy(&o.stderr).contains("com.apple.security.hypervisor") =>
        {
            Check::ok("code signature", "signed, with the hypervisor entitlement")
        }
        Ok(_) => Check::fail(
            "code signature",
            "signed, but without com.apple.security.hypervisor — krun_start_enter \
             will fail with a bare EINVAL that says nothing about signing",
            "every `cargo build` relinks, and the linker's ad-hoc signature carries no \n\
             entitlements. Re-sign:  make sign-release",
        ),
        Err(e) => Check::warn("code signature", format!("could not run codesign: {e}"), ""),
    }
}

/// gvproxy is what gives a guest a network, port forwards and the exec agent —
/// without it a machine still boots, but `exec`, `cp` and `cache` cannot reach
/// it.
fn networking() -> Check {
    match crate::net::locate() {
        Ok(p) => Check::ok("networking", format!("gvproxy at {}", p.display())),
        Err(e) => Check::warn(
            "networking",
            format!("{e:#}"),
            "brew install gvproxy — without it machines boot with no network, \
             so exec/cp/cache cannot reach them",
        ),
    }
}

fn writable(name: &'static str, dir: Result<PathBuf>) -> Check {
    let dir = match dir {
        Ok(d) => d,
        Err(e) => return Check::fail(name, format!("{e:#}"), "set HOME"),
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Check::fail(
            name,
            format!("{} is not usable: {e}", dir.display()),
            "check the permissions on it",
        );
    }
    // Creating the directory can succeed on a read-only mount; writing proves it.
    let probe = dir.join(".bsdkrun-doctor");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Check::ok(name, dir.display().to_string())
        }
        Err(e) => Check::fail(
            name,
            format!("{} is not writable: {e}", dir.display()),
            "check the permissions on it",
        ),
    }
}

#[cfg(target_os = "macos")]
fn store() -> Check {
    match crate::store::describe() {
        Ok(s) if s.contains("case-INSENSITIVE") => Check::warn(
            "store",
            s,
            "nix guests and Linux kernel sources need case-sensitive storage; \
             bsdkrun creates it automatically on the next Linux run",
        ),
        Ok(s) => Check::ok("store", s),
        Err(e) => Check::warn("store", format!("{e:#}"), ""),
    }
}

/// Where `bsdkrun cache` would put things, and — for S3 — whether it has what
/// it needs to get there. Deliberately does not make a network call: doctor
/// should be fast and safe to run anywhere.
fn cache_backend() -> Check {
    match crate::cache::Store::open() {
        Ok(store @ crate::cache::Store::Disk(_)) => Check::ok("cache", store.describe()),
        Ok(store) => match crate::cache::config::credentials() {
            Ok(_) => Check::ok(
                "cache",
                format!("{} (credentials present)", store.describe()),
            ),
            Err(e) => Check::warn("cache", format!("{}: {e}", store.describe()), ""),
        },
        Err(e) => Check::warn("cache", format!("{e:#}"), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Doctor exists to be read under stress; a check that fails has to carry a
    /// fix, or it is just another error message.
    #[test]
    fn every_failing_or_warning_check_offers_something_to_do() {
        for c in gather() {
            if c.state == State::Ok {
                continue;
            }
            assert!(
                c.fix.is_some(),
                "check {:?} is {:?} but suggests nothing",
                c.name,
                c.state
            );
        }
    }

    /// The tools bsdkrun cannot work without have to be reported as failures,
    /// not warnings — this machine has them, so the shape is checked directly.
    #[test]
    fn a_missing_required_tool_is_fatal_and_an_optional_one_is_not() {
        let missing = tool("definitely-not-a-real-tool", true, "why");
        assert_eq!(missing.state, State::Fail);
        assert!(missing.fix.unwrap().contains("brew install"));

        let optional = tool("definitely-not-a-real-tool", false, "why");
        assert_eq!(optional.state, State::Warn);
    }

    #[test]
    fn a_writable_directory_passes_and_a_bad_one_fails() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(writable("t", Ok(dir.path().to_path_buf())).state, State::Ok);
        assert_eq!(
            writable("t", Ok(PathBuf::from("/dev/null/nope"))).state,
            State::Fail
        );
    }

    /// The probe file must not survive the check that wrote it.
    #[test]
    fn the_writability_probe_cleans_up_after_itself() {
        let dir = tempfile::tempdir().unwrap();
        writable("t", Ok(dir.path().to_path_buf()));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
