//! Host readiness checks: `bsdkrun probe` and `bsdkrun kvm`.

use anyhow::{Context, Result};
use tracing::info;

use crate::host;
use crate::krun::Ctx;

pub(crate) fn probe() -> Result<()> {
    // Checked (and reported) before the context so a KVM problem reads on its
    // own terms rather than as "creating libkrun context: …".
    host::check_kvm()?;
    if let Some(summary) = host::kvm_summary() {
        info!("{summary}");
    }
    let ctx = Ctx::new().context("creating libkrun context")?;
    ctx.set_vm_config(1, 256)
        .context("setting a trivial VM config")?;
    info!("libkrun linked and a context was created + configured (dropped without booting)");
    Ok(())
}

/// `bsdkrun kvm` — report whether this host can run machines, and if not, what
/// to do about it. Exits 1 when KVM is unusable so a script can gate on it
/// (`bsdkrun kvm >/dev/null || skip`); the JSON form still prints its report
/// first, so a caller gets the reason along with the failing status.
#[cfg(target_os = "linux")]
pub(crate) fn cmd_kvm(json: bool) -> Result<()> {
    let status = host::KvmStatus::gather();
    let facts = &status.facts;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": status.is_ok(),
                "device": status.device,
                "status": status.headline(),
                "api_version": status.result.as_ref().ok(),
                "advice": status.advice(),
                "cpu_virt_flag": facts.cpu_virt_flag,
                "module_loaded": facts.module_loaded,
                "in_container": facts.in_container,
                "owner_group": facts.owner_group,
                "mode": facts.mode.map(|m| format!("{m:04o}")),
            }))?
        );
        if !status.is_ok() {
            std::process::exit(1);
        }
        return Ok(());
    }

    println!("{:<14} {}", status.device, status.headline());
    if let Some(mode) = facts.mode {
        let group = facts.owner_group.as_deref().unwrap_or("?");
        println!("{:<14} mode {mode:04o}, group {group}", "node");
    }
    println!(
        "{:<14} {}",
        "kvm module",
        if facts.module_loaded {
            "loaded"
        } else {
            "not loaded"
        }
    );
    println!(
        "{:<14} {}",
        "cpu virt",
        facts.cpu_virt_flag.unwrap_or("not advertised")
    );
    if facts.in_container {
        println!("{:<14} yes", "container");
    }

    // The advice is the whole point when something is wrong — make it the exit
    // status too, not just text the caller has to parse.
    if let Some(advice) = status.advice() {
        println!();
        anyhow::bail!("{advice}");
    }
    Ok(())
}
