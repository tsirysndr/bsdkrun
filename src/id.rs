//! Docker-style short identifiers.

use std::io::Read;

/// A short unique id: 12 lowercase hex characters (48 random bits), like the
/// truncated ids Docker shows. Read from `/dev/urandom`, with a pid+time
/// fallback in the vanishingly unlikely case that read fails.
pub fn short_id() -> String {
    let mut buf = [0u8; 6];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    if !ok {
        // Fallback: mix pid and the current time so we still get a unique-ish id.
        let pid = std::process::id() as u64;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mixed = pid.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(nanos);
        buf.copy_from_slice(&mixed.to_le_bytes()[..6]);
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
