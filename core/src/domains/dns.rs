//! The built-in DNS responder behind machine domains: a UDP listener on the
//! loopback that answers every name under the configured TLD with the loopback
//! address, where Caddy is listening.
//!
//! Hand-rolled rather than dnsmasq (reeve's choice) because the behaviour
//! needed is one rule — `*.<tld>` → 127.0.0.1 — and owning the responder means
//! owning its correctness: gvproxy answers NXDOMAIN (not NODATA) for the AAAA
//! of an A-only name, which broke NetBSD's resolver and cost a `/etc/hosts`
//! sync workaround (see `network::sync_hosts`). This responder never repeats
//! that.

use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::warn;

use crate::db;

/// Loopback UDP port the responder listens on. Unprivileged, and distinct from
/// other local-dev DNS tools (dnsmasq setups commonly sit on 5335) so bsdkrun
/// coexists with them.
pub const DNS_PORT: u16 = 5343;

// DNS wire-format constants (RFC 1035).
const QR: u16 = 0x8000;
const AA: u16 = 0x0400;
const RD: u16 = 0x0100;
const RCODE_REFUSED: u16 = 5;
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const CLASS_IN: u16 = 1;
/// Short TTL: machines come and go, and the answer is always loopback anyway.
const TTL: u32 = 10;

/// Answer one DNS query. Pure, so the wire format is unit-testable.
///
/// * A in zone     → 127.0.0.1
/// * AAAA in zone  → ::1 (Caddy binds the wildcard, so v6 loopback terminates)
/// * other in zone → NODATA (RCODE 0, no answers — never NXDOMAIN)
/// * out of zone   → REFUSED, so a mis-routed system query fails fast to the
///   next resolver instead of hanging on us
/// * not a standard query / malformed → `None` (dropped)
pub fn handle_query(req: &[u8], zone: &str) -> Option<Vec<u8>> {
    if req.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([req[0], req[1]]);
    let flags = u16::from_be_bytes([req[2], req[3]]);
    let qdcount = u16::from_be_bytes([req[4], req[5]]);
    // Only standard queries (QR clear, opcode 0) with exactly one question.
    if flags & QR != 0 || (flags >> 11) & 0xF != 0 || qdcount != 1 {
        return None;
    }

    // Decode the question's QNAME labels. Queries carry no compression
    // pointers, so a length byte with the top bits set is malformed here.
    let mut pos = 12;
    let mut name = String::new();
    loop {
        let len = *req.get(pos)? as usize;
        if len == 0 {
            pos += 1;
            break;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        let label = req.get(pos + 1..pos + 1 + len)?;
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(&String::from_utf8_lossy(label));
        pos += 1 + len;
    }
    let qtype = u16::from_be_bytes([*req.get(pos)?, *req.get(pos + 1)?]);
    let qclass = u16::from_be_bytes([*req.get(pos + 2)?, *req.get(pos + 3)?]);
    let question = &req[12..pos + 4];

    let lower = name.to_ascii_lowercase();
    let zone = zone.trim_end_matches('.').to_ascii_lowercase();
    let in_zone = lower == zone || lower.ends_with(&format!(".{zone}"));

    let mut out = Vec::with_capacity(12 + question.len() + 32);
    let rcode = if in_zone { 0 } else { RCODE_REFUSED };
    let answers: u16 = match (in_zone, qtype, qclass) {
        (true, TYPE_A, CLASS_IN) | (true, TYPE_AAAA, CLASS_IN) => 1,
        _ => 0,
    };
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&(QR | AA | (flags & RD) | rcode).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&answers.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out.extend_from_slice(question);

    if answers == 1 {
        out.extend_from_slice(&[0xC0, 0x0C]); // pointer to the question name
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&TTL.to_be_bytes());
        if qtype == TYPE_A {
            out.extend_from_slice(&4u16.to_be_bytes());
            out.extend_from_slice(&[127, 0, 0, 1]);
        } else {
            out.extend_from_slice(&16u16.to_be_bytes());
            let mut v6 = [0u8; 16];
            v6[15] = 1; // ::1
            out.extend_from_slice(&v6);
        }
    }
    Some(out)
}

/// Run the responder: bind the loopback UDP port and answer forever. This is
/// the body of the detached `domains __serve-dns` process — it never returns
/// except on a bind error.
pub fn serve(port: u16, tld: &str) -> Result<()> {
    let sock = UdpSocket::bind(("127.0.0.1", port))
        .with_context(|| format!("binding the domains DNS responder on 127.0.0.1:{port}"))?;
    let mut buf = [0u8; 512];
    loop {
        let Ok((n, peer)) = sock.recv_from(&mut buf) else {
            continue;
        };
        if let Some(reply) = handle_query(&buf[..n], tld) {
            let _ = sock.send_to(&reply, peer);
        }
    }
}

/// Spawn the responder as a detached daemon (`setsid`, stderr → dns.log, pid
/// recorded in settings) — the same pattern as the network gvproxy
/// (`network::spawn_gvproxy`), so the next CLI invocation can lazily respawn it.
pub fn spawn(port: u16, tld: &str) -> Result<u32> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().context("resolving the bsdkrun binary path")?;
    let log_path = dns_log_path()?;
    let log = std::fs::File::create(&log_path)
        .with_context(|| format!("creating {}", log_path.display()))?;
    let mut cmd = Command::new(exe);
    cmd.arg("domains")
        .arg("__serve-dns")
        .arg("--port")
        .arg(port.to_string())
        .arg("--tld")
        .arg(tld)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // Detach into its own session so it outlives this invocation.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().context("spawning the domains DNS responder")?;
    let pid = child.id();
    std::mem::forget(child); // a daemon tracked by pid — don't reap on drop

    // Wait until it answers, so `enable` reports a live responder or an error.
    for _ in 0..50 {
        if probe(port, tld) {
            return Ok(pid);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    anyhow::bail!(
        "the DNS responder did not come up on 127.0.0.1:{port} (see {})",
        log_path.display()
    )
}

/// Respawn the responder if its recorded pid is dead. Lazy supervision, called
/// from `status`/`sync`/the boot hook — mirrors `network::ensure_running`.
pub fn ensure_running(db: &db::Db, port: u16, tld: &str) -> Result<()> {
    let alive = db
        .get_setting("domains.dns_pid")?
        .and_then(|p| p.parse::<i64>().ok())
        .map(db::pid_alive)
        .unwrap_or(false);
    if alive {
        return Ok(());
    }
    let pid = spawn(port, tld)?;
    db.set_setting("domains.dns_pid", &pid.to_string())?;
    warn!(pid, "restarted the domains DNS responder");
    Ok(())
}

/// One live A query for `probe.<tld>` — true when the responder answers with
/// 127.0.0.1. Backs `domains status` and the post-spawn readiness wait.
pub fn probe(port: u16, tld: &str) -> bool {
    let Ok(sock) = UdpSocket::bind("127.0.0.1:0") else {
        return false;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    let mut q = Vec::new();
    q.extend_from_slice(&0x4242u16.to_be_bytes());
    q.extend_from_slice(&RD.to_be_bytes());
    q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // QD=1
    q.extend_from_slice(&[5]);
    q.extend_from_slice(b"probe");
    for label in tld.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_A.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    if sock.send_to(&q, ("127.0.0.1", port)).is_err() {
        return false;
    }
    let mut buf = [0u8; 512];
    match sock.recv_from(&mut buf) {
        Ok((n, _)) => buf[..n].ends_with(&[127, 0, 0, 1]),
        Err(_) => false,
    }
}

/// The responder's stderr log, beside the rest of the domains state.
pub fn dns_log_path() -> Result<PathBuf> {
    Ok(db::state_dir()?.join("dns.log"))
}

/// SIGTERM the recorded responder pid, if any, and clear it.
pub fn stop(db: &db::Db) -> Result<()> {
    if let Some(pid) = db
        .get_setting("domains.dns_pid")?
        .and_then(|p| p.parse::<i32>().ok())
    {
        if db::pid_alive(pid as i64) {
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }
    db.remove_setting("domains.dns_pid")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a query for `name` with the given qtype.
    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = Vec::new();
        q.extend_from_slice(&0x1234u16.to_be_bytes());
        q.extend_from_slice(&RD.to_be_bytes());
        q.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend_from_slice(label.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&CLASS_IN.to_be_bytes());
        q
    }

    fn rcode(reply: &[u8]) -> u16 {
        u16::from_be_bytes([reply[2], reply[3]]) & 0xF
    }

    fn ancount(reply: &[u8]) -> u16 {
        u16::from_be_bytes([reply[6], reply[7]])
    }

    #[test]
    fn a_query_in_zone_answers_loopback() {
        let reply = handle_query(&query("tidy-turing.bsdk", TYPE_A), "bsdk").unwrap();
        assert_eq!(rcode(&reply), 0);
        assert_eq!(ancount(&reply), 1);
        assert!(reply.ends_with(&[127, 0, 0, 1]));
        // QR + AA + RD echoed.
        let flags = u16::from_be_bytes([reply[2], reply[3]]);
        assert_ne!(flags & QR, 0);
        assert_ne!(flags & AA, 0);
        assert_ne!(flags & RD, 0);
    }

    #[test]
    fn aaaa_query_in_zone_answers_v6_loopback() {
        let reply = handle_query(&query("x.bsdk", TYPE_AAAA), "bsdk").unwrap();
        assert_eq!(ancount(&reply), 1);
        let mut v6 = [0u8; 16];
        v6[15] = 1;
        assert!(reply.ends_with(&v6));
    }

    #[test]
    fn other_qtype_in_zone_is_nodata_not_nxdomain() {
        // The gvproxy regression: an MX/TXT/HTTPS query for an existing name
        // must be NODATA (rcode 0, no answers), never an error rcode.
        for qtype in [15u16, 16, 65] {
            let reply = handle_query(&query("x.bsdk", qtype), "bsdk").unwrap();
            assert_eq!(rcode(&reply), 0, "qtype {qtype}");
            assert_eq!(ancount(&reply), 0, "qtype {qtype}");
        }
    }

    #[test]
    fn out_of_zone_is_refused() {
        let reply = handle_query(&query("example.com", TYPE_A), "bsdk").unwrap();
        assert_eq!(rcode(&reply), RCODE_REFUSED);
        assert_eq!(ancount(&reply), 0);
    }

    #[test]
    fn zone_matching_is_case_insensitive_and_dot_tolerant() {
        assert_eq!(ancount(&handle_query(&query("A.BSDK", TYPE_A), "bsdk.").unwrap()), 1);
        // The bare zone apex resolves too.
        assert_eq!(ancount(&handle_query(&query("bsdk", TYPE_A), "bsdk").unwrap()), 1);
    }

    #[test]
    fn malformed_and_non_queries_are_dropped() {
        assert!(handle_query(&[0, 1, 2], "bsdk").is_none());
        // A response (QR set) must not be answered.
        let mut q = query("x.bsdk", TYPE_A);
        q[2] |= 0x80;
        assert!(handle_query(&q, "bsdk").is_none());
        // A compression pointer in the question is malformed.
        let mut q = query("x.bsdk", TYPE_A);
        q[12] = 0xC0;
        assert!(handle_query(&q, "bsdk").is_none());
    }
}
