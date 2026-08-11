//! Machine domains: every machine reachable at `https://<name>.<tld>` on this
//! host.
//!
//! Three cooperating pieces, each supervised lazily (pid in the settings
//! table, respawned by the next bsdkrun invocation that notices it dead — the
//! `network::ensure_running` pattern):
//!
//!   * [`dns`] — a built-in UDP responder answering `*.<tld>` with 127.0.0.1.
//!   * [`resolver`] — the host resolver wiring pointing that TLD at it
//!     (`/etc/resolver/<tld>` on macOS, a systemd-resolved drop-in on Linux).
//!   * [`caddy`] — a Caddy reverse proxy terminating TLS with its local CA and
//!     forwarding to each machine's gvproxy port forward. The host cannot
//!     reach guest IPs (gvproxy is userspace NAT), so a machine without a
//!     `--port` forward has nothing to route to and is skipped.

pub mod caddy;
pub mod dns;
pub mod resolver;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{api, db};

/// Default TLD — short, memorable, and never fighting other local-dev tools
/// over `/etc/resolver/test`.
pub const DEFAULT_TLD: &str = "bsdk";

/// The feature's persisted settings, read from the settings table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub enabled: bool,
    pub tld: String,
    pub https_port: u16,
    pub http_port: u16,
    pub dns_port: u16,
}

impl Settings {
    pub fn load(db: &db::Db) -> Result<Self> {
        let get = |k: &str| db.get_setting(k);
        Ok(Settings {
            enabled: get("domains.enabled")?.as_deref() == Some("1"),
            tld: get("domains.tld")?.unwrap_or_else(|| DEFAULT_TLD.to_string()),
            https_port: get("domains.https_port")?
                .and_then(|p| p.parse().ok())
                .unwrap_or(443),
            http_port: get("domains.http_port")?
                .and_then(|p| p.parse().ok())
                .unwrap_or(80),
            dns_port: get("domains.dns_port")?
                .and_then(|p| p.parse().ok())
                .unwrap_or(dns::DNS_PORT),
        })
    }

    fn store(&self, db: &db::Db) -> Result<()> {
        db.set_setting("domains.enabled", if self.enabled { "1" } else { "0" })?;
        db.set_setting("domains.tld", &self.tld)?;
        db.set_setting("domains.https_port", &self.https_port.to_string())?;
        db.set_setting("domains.http_port", &self.http_port.to_string())?;
        db.set_setting("domains.dns_port", &self.dns_port.to_string())
    }
}

/// A machine name as a DNS label: lowercase, `_` → `-` (names.rs generates
/// `adjective_scientist`, and underscore is invalid in hostnames), everything
/// else outside `[a-z0-9-]` dropped, trimmed, capped at 63.
pub fn dns_label(name: &str) -> String {
    let mut label: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == '_' { '-' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    label.truncate(63);
    label.trim_matches('-').to_string()
}

/// Assign each named machine its final label, disambiguating collisions after
/// normalization: the oldest machine keeps the bare label, later ones get a
/// `-<id prefix>` suffix. Deterministic, so the Caddyfile is too.
pub fn dns_labels(machines: &[api::Machine]) -> Vec<(String, &api::Machine)> {
    let mut named: Vec<&api::Machine> = machines.iter().filter(|m| m.name.is_some()).collect();
    // list_machines returns newest first; oldest first keeps labels stable as
    // new machines appear.
    named.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in named {
        let base = dns_label(m.name.as_deref().unwrap_or_default());
        if base.is_empty() {
            continue;
        }
        let label = if seen.insert(base.clone()) {
            base
        } else {
            let id4: String = m.id.chars().take(4).collect();
            let l = format!("{base}-{id4}");
            if !seen.insert(l.clone()) {
                continue; // same id prefix twice cannot happen; be safe anyway
            }
            l
        };
        out.push((label, m));
    }
    out
}

/// The URL a machine serves on, when domains are enabled and it has a forward.
pub fn machine_url(m: &api::Machine, s: &Settings) -> Option<String> {
    if !s.enabled || m.ports.is_empty() {
        return None;
    }
    let label = dns_label(m.name.as_deref()?);
    if label.is_empty() {
        return None;
    }
    let port = if s.https_port == 443 {
        String::new()
    } else {
        format!(":{}", s.https_port)
    };
    Some(format!("https://{label}.{}{port}", s.tld))
}

/// One row of `domains ls`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainRow {
    pub machine: String,
    pub id: String,
    pub url: Option<String>,
    /// `127.0.0.1:<host port>` when routable, or why it isn't.
    pub upstream: String,
    pub running: bool,
}

/// Component health for `domains status` (and the TUI chip).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub enabled: bool,
    pub tld: String,
    pub https_port: u16,
    pub dns_running: bool,
    pub dns_port: u16,
    pub resolver_ok: bool,
    pub resolver_location: String,
    pub caddy_running: bool,
    pub ca_trusted: bool,
    pub domains: usize,
}

/// Everything `ls` needs, from one machine listing.
pub fn list(db: &db::Db) -> Result<(Settings, Vec<DomainRow>)> {
    let s = Settings::load(db)?;
    let machines = api::list_machines(true)?;
    let labeled = dns_labels(&machines);
    let mut rows = Vec::new();
    for (label, m) in labeled {
        let (url, upstream) = match m.ports.first() {
            Some(pf) => {
                let port = if s.https_port == 443 {
                    String::new()
                } else {
                    format!(":{}", s.https_port)
                };
                (
                    Some(format!("https://{label}.{}{port}", s.tld)),
                    format!("127.0.0.1:{}", pf.host),
                )
            }
            None => (None, "no --port forward".to_string()),
        };
        rows.push(DomainRow {
            machine: m.name.clone().unwrap_or_default(),
            id: m.id.clone(),
            url,
            upstream,
            running: m.running,
        });
    }
    Ok((s, rows))
}

/// Component health, with a live DNS probe.
pub fn status(db: &db::Db) -> Result<Status> {
    let s = Settings::load(db)?;
    let dns_running = dns::probe(s.dns_port, &s.tld);
    let caddy_running = caddy::running(db)?;
    let (_, rows) = list(db)?;
    Ok(Status {
        enabled: s.enabled,
        tld: s.tld.clone(),
        https_port: s.https_port,
        dns_running,
        dns_port: s.dns_port,
        resolver_ok: resolver::ok(&s.tld, s.dns_port),
        resolver_location: resolver::location(&s.tld),
        caddy_running,
        ca_trusted: caddy::is_trusted(),
        domains: rows.iter().filter(|r| r.url.is_some()).count(),
    })
}

fn valid_tld(tld: &str) -> bool {
    !tld.is_empty()
        && tld
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.'))
        && !tld.starts_with('.')
        && !tld.ends_with('.')
}

/// `domains enable` — every step idempotent, so re-running repairs.
pub fn enable(tld: &str, https_port: u16, http_port: u16, no_trust: bool) -> Result<Settings> {
    if !valid_tld(tld) {
        bail!("invalid TLD {tld:?}: lowercase letters, digits, `-` and `.` only");
    }
    let db = db::Db::open()?;
    let s = Settings {
        enabled: true,
        tld: tld.to_string(),
        https_port,
        http_port,
        dns_port: dns::DNS_PORT,
    };
    s.store(&db)?;

    // 1. The DNS responder.
    dns::ensure_running(&db, s.dns_port, &s.tld)?;

    // 2. Host resolver wiring — the one privileged step, batched.
    resolver::setup(&s.tld, s.dns_port).context("wiring the system resolver")?;

    // 3. Low ports (Linux needs a one-time sysctl; macOS allows them).
    if !caddy::can_bind(s.https_port) {
        #[cfg(target_os = "linux")]
        {
            println!(
                "Port {} needs a one-time sysctl (net.ipv4.ip_unprivileged_port_start=80).",
                s.https_port
            );
            caddy::allow_low_ports()?;
        }
        if !caddy::can_bind(s.https_port) {
            bail!(
                "cannot bind port {} — another process holds it (`lsof -i :{}` names it), \
                 or re-run with `--https-port 8443 --http-port 8080`",
                s.https_port,
                s.https_port
            );
        }
    }

    // 4. Caddy, from the current machine list.
    let machines = api::list_machines(true)?;
    let conf = caddy::render_caddyfile(&machines, &s)?;
    caddy::write_caddyfile(&conf)?;
    caddy::reload(&db)?;

    // 5. Trust the local CA.
    if !no_trust && !caddy::is_trusted() {
        caddy::trust()?;
    }
    Ok(s)
}

/// `domains disable` — stop the daemons; `purge` also unwires the resolver,
/// untrusts the CA, and deletes the proxy state (including the CA itself).
pub fn disable(purge: bool) -> Result<()> {
    let db = db::Db::open()?;
    let s = Settings::load(&db)?;
    dns::stop(&db)?;
    caddy::stop(&db)?;
    db.set_setting("domains.enabled", "0")?;
    if purge {
        caddy::untrust();
        resolver::teardown(&s.tld)?;
        let dir = caddy::proxy_dir()?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
    }
    Ok(())
}

/// Regenerate the Caddyfile and reload — cheap when nothing changed (byte
/// compare), a reload when it did. Also heals dead daemons.
pub fn sync(db: &db::Db) -> Result<()> {
    let s = Settings::load(db)?;
    if !s.enabled {
        return Ok(());
    }
    dns::ensure_running(db, s.dns_port, &s.tld)?;
    let machines = api::list_machines(true)?;
    let conf = caddy::render_caddyfile(&machines, &s)?;
    if caddy::write_caddyfile(&conf)? {
        caddy::reload(db)?;
    } else {
        caddy::ensure_running(db)?;
    }
    Ok(())
}

/// The hook the machine-lifecycle paths call after a DB write (boot, stop,
/// rm, update, commit). Must never fail a boot: swallow and log.
pub fn sync_if_enabled() {
    let result = (|| -> Result<()> {
        let db = db::Db::open()?;
        if db.get_setting("domains.enabled")?.as_deref() != Some("1") {
            return Ok(());
        }
        sync(&db)
    })();
    if let Err(e) = result {
        warn!(error = %e, "domains sync failed (machine domains may be stale)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_normalize_underscores_and_junk() {
        assert_eq!(dns_label("tidy_turing"), "tidy-turing");
        assert_eq!(dns_label("My App!"), "myapp");
        assert_eq!(dns_label("__x__"), "x");
        assert_eq!(dns_label("---"), "");
        let long = "a".repeat(80);
        assert_eq!(dns_label(&long).len(), 63);
    }

    fn machine(id: &str, name: &str, created: &str) -> api::Machine {
        api::Machine {
            id: id.into(),
            name: Some(name.into()),
            image: String::new(),
            kind: "linux".into(),
            command: String::new(),
            status: "running".into(),
            running: true,
            exit_code: None,
            pid: None,
            detached: true,
            cpus: None,
            mem: None,
            volume: None,
            state_dir: None,
            created_at: Some(created.into()),
            finished_at: None,
            network: None,
            net_ip: None,
            ports: vec![crate::net::PortForward::loopback(18080, 80)],
        }
    }

    #[test]
    fn label_collisions_keep_oldest_bare() {
        let ms = vec![
            machine("bbbb1111", "foo-bar", "2026-01-02"),
            machine("aaaa2222", "foo_bar", "2026-01-01"),
        ];
        let labels = dns_labels(&ms);
        assert_eq!(labels[0].0, "foo-bar");
        assert_eq!(labels[0].1.id, "aaaa2222"); // older keeps the bare label
        assert_eq!(labels[1].0, "foo-bar-bbbb");
    }

    #[test]
    fn urls_only_when_enabled_and_forwarded() {
        let s = Settings {
            enabled: true,
            tld: "bsdk".into(),
            https_port: 443,
            http_port: 80,
            dns_port: 5343,
        };
        let m = machine("abcd", "tidy_turing", "2026-01-01");
        assert_eq!(
            machine_url(&m, &s).as_deref(),
            Some("https://tidy-turing.bsdk")
        );
        let mut no_ports = m.clone();
        no_ports.ports.clear();
        assert_eq!(machine_url(&no_ports, &s), None);
        let alt = Settings {
            https_port: 8443,
            ..s.clone()
        };
        assert_eq!(
            machine_url(&m, &alt).as_deref(),
            Some("https://tidy-turing.bsdk:8443")
        );
        let off = Settings {
            enabled: false,
            ..s
        };
        assert_eq!(machine_url(&m, &off), None);
    }

    #[test]
    fn tld_validation() {
        assert!(valid_tld("bsdk"));
        assert!(valid_tld("bsdk.test"));
        assert!(!valid_tld(""));
        assert!(!valid_tld(".bsdk"));
        assert!(!valid_tld("bsdk."));
        assert!(!valid_tld("BSDK"));
        assert!(!valid_tld("b sdk"));
    }
}
