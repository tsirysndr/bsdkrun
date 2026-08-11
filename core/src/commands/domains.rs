//! `bsdkrun domains` — the printing layer over [`crate::domains`].

use anyhow::Result;

use crate::cli::DomainsEnableArgs;
use crate::{db, domains};

use super::truncate;

pub(crate) fn cmd_enable(args: DomainsEnableArgs) -> Result<()> {
    let s = domains::enable(&args.tld, args.https_port, args.http_port, args.no_trust)?;
    println!("machine domains enabled on *.{}", s.tld);
    println!();
    cmd_ls(false)?;
    println!();
    println!("New machines join automatically; `bsdkrun domains status` checks health.");
    Ok(())
}

pub(crate) fn cmd_disable(purge: bool) -> Result<()> {
    domains::disable(purge)?;
    if purge {
        println!("machine domains disabled; resolver wiring, CA trust and proxy state removed");
    } else {
        println!("machine domains disabled (state kept — `enable` restores it)");
    }
    Ok(())
}

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_status(json: bool) -> Result<()> {
    let db = db::Db::open()?;
    let st = domains::status(&db)?;
    if json {
        println!("{}", serde_json::to_string(&st)?);
        return Ok(());
    }
    let yn = |b: bool| if b { "ok" } else { "NOT OK" };
    println!(
        "machine domains: {}",
        if st.enabled { "enabled" } else { "disabled" }
    );
    println!("{:<12}  {:<8}  {}", "COMPONENT", "STATE", "DETAIL");
    println!(
        "{:<12}  {:<8}  127.0.0.1:{} answering *.{}",
        "dns",
        yn(st.dns_running),
        st.dns_port,
        st.tld
    );
    println!(
        "{:<12}  {:<8}  {}",
        "resolver",
        yn(st.resolver_ok),
        st.resolver_location
    );
    println!(
        "{:<12}  {:<8}  https on :{}",
        "caddy",
        yn(st.caddy_running),
        st.https_port
    );
    println!(
        "{:<12}  {:<8}  {}",
        "ca trust",
        yn(st.ca_trusted),
        "local CA in the system trust store"
    );
    println!("{} machine domain(s) served", st.domains);
    if st.enabled && !(st.dns_running && st.resolver_ok && st.caddy_running && st.ca_trusted) {
        println!("run `bsdkrun domains enable` again to repair");
    }
    Ok(())
}

#[allow(clippy::print_literal)] // padded tabular headers read clearer as args
pub(crate) fn cmd_ls(json: bool) -> Result<()> {
    let db = db::Db::open()?;
    let (_, rows) = domains::list(&db)?;
    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    println!(
        "{:<20}  {:<36}  {:<22}  {}",
        "NAME", "URL", "UPSTREAM", "STATE"
    );
    for r in rows {
        println!(
            "{:<20}  {:<36}  {:<22}  {}",
            truncate(&r.machine, 20),
            truncate(r.url.as_deref().unwrap_or("-"), 36),
            truncate(&r.upstream, 22),
            if r.running { "running" } else { "stopped" }
        );
    }
    Ok(())
}

pub(crate) fn cmd_sync() -> Result<()> {
    let db = db::Db::open()?;
    let s = domains::Settings::load(&db)?;
    if !s.enabled {
        println!("machine domains are disabled — `bsdkrun domains enable` first");
        return Ok(());
    }
    domains::sync(&db)?;
    println!("proxy config regenerated");
    Ok(())
}
