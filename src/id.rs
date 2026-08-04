//! Docker-style short identifiers.

use std::io::Read;
use std::sync::Mutex;

/// A one-shot override for the next machine id. `start <id>` sets this so an
/// existing machine re-boots under its *own* id (an in-place restart) instead of
/// minting a fresh one. Consumed by [`next_machine_id`].
static ID_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);

/// Force the next `next_machine_id()` to return this id (once).
pub fn set_override(id: &str) {
    *ID_OVERRIDE.lock().unwrap() = Some(id.to_string());
}

/// The id for a machine about to boot: a pending override if one was set (and
/// consume it), else a fresh short id.
pub fn next_machine_id() -> String {
    if let Some(id) = ID_OVERRIDE.lock().unwrap().take() {
        return id;
    }
    short_id()
}

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
