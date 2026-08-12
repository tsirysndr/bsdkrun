//! Locate the `bsdkrun` binary on the host.
//!
//! Resolution order (first match wins, then cached):
//!
//! 1. an explicit override set via [`set_binary_path`],
//! 2. the `BSDKRUN_BIN` environment variable,
//! 3. `bsdkrun` on `PATH`,
//! 4. in-repo dev builds relative to this crate's source:
//!    `<repo_root>/target/release/bsdkrun` then `.../target/debug/bsdkrun`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{Error, Result};

struct BinState {
    override_path: Option<String>,
    resolved: Option<String>,
}

static STATE: Mutex<BinState> = Mutex::new(BinState {
    override_path: None,
    resolved: None,
});

/// Force the SDK to use a specific `bsdkrun` binary, bypassing discovery.
///
/// Handy in tests or when running against a locally built debug binary.
pub fn set_binary_path(path: impl Into<String>) {
    let mut state = STATE.lock().unwrap();
    state.override_path = Some(path.into());
    state.resolved = None;
}

/// Reset cached discovery state and any override (mainly for tests).
pub fn reset_binary_cache() {
    let mut state = STATE.lock().unwrap();
    state.override_path = None;
    state.resolved = None;
}

/// The repo root when this crate is built from a checkout: the manifest lives
/// at `<repo>/sdk/rust`, so two levels up. For a crate pulled from a registry
/// this points into the cargo cache, where the `target/` candidates simply
/// fail their `exists()` check — same effect as Python's `__file__`-relative
/// lookup outside a checkout.
fn repo_root() -> Option<&'static Path> {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)
}

/// A minimal `which`: the first `PATH` entry holding an executable `bsdkrun`.
fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// Candidate locations, in priority order.
fn candidates(override_path: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(explicit) = override_path {
        out.push(explicit.to_string());
    }
    if let Ok(env) = std::env::var("BSDKRUN_BIN") {
        if !env.is_empty() {
            out.push(env);
        }
    }
    // A `bsdkrun` already on PATH wins over in-repo builds.
    if let Some(on_path) = which("bsdkrun") {
        out.push(on_path.to_string_lossy().into_owned());
    }
    if let Some(root) = repo_root() {
        out.push(
            root.join("target/release/bsdkrun")
                .to_string_lossy()
                .into_owned(),
        );
        out.push(
            root.join("target/debug/bsdkrun")
                .to_string_lossy()
                .into_owned(),
        );
    }
    out
}

/// Resolve (and cache) the path to the `bsdkrun` binary.
///
/// Returns [`Error::BinaryNotFound`] if none of the candidate locations exist.
pub fn resolve_binary() -> Result<String> {
    let mut state = STATE.lock().unwrap();
    if let Some(resolved) = &state.resolved {
        return Ok(resolved.clone());
    }

    let searched = candidates(state.override_path.as_deref());
    for candidate in &searched {
        if Path::new(candidate).exists() {
            state.resolved = Some(candidate.clone());
            return Ok(candidate.clone());
        }
    }
    Err(Error::BinaryNotFound { searched })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Discovery reads process-global state (the override cache and env vars),
    // so the tests here serialize on one lock instead of racing each other.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn touch_executable(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("bsdkrun-sdk-test-{name}-{}", std::process::id()));
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn explicit_override_wins() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fake = touch_executable("override");
        set_binary_path(fake.to_string_lossy().into_owned());
        assert_eq!(resolve_binary().unwrap(), fake.to_string_lossy());
        reset_binary_cache();
        std::fs::remove_file(&fake).ok();
    }

    #[test]
    fn env_var_wins_when_no_override_is_set() {
        let _guard = TEST_LOCK.lock().unwrap();
        let fake = touch_executable("env");
        let saved = std::env::var("BSDKRUN_BIN").ok();
        reset_binary_cache();
        std::env::set_var("BSDKRUN_BIN", &fake);
        assert_eq!(resolve_binary().unwrap(), fake.to_string_lossy());
        match saved {
            Some(v) => std::env::set_var("BSDKRUN_BIN", v),
            None => std::env::remove_var("BSDKRUN_BIN"),
        }
        reset_binary_cache();
        std::fs::remove_file(&fake).ok();
    }

    #[test]
    fn missing_override_falls_through_to_later_candidates() {
        let _guard = TEST_LOCK.lock().unwrap();
        set_binary_path("/definitely/not/here/bsdkrun");
        // In this repo a target/release build usually exists, so resolution
        // may still succeed — the assertion is only that the bogus override
        // never wins.
        if let Ok(found) = resolve_binary() {
            assert_ne!(found, "/definitely/not/here/bsdkrun");
        }
        reset_binary_cache();
    }
}
