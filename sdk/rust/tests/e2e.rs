//! End-to-end smoke tests against a **real** `bsdkrun` binary.
//!
//! Skipped by default: `cargo test` must never require the binary, libkrun,
//! or a bootable guest. Opt in with:
//!
//! ```sh
//! BSDKRUN_SDK_E2E=1 cargo test --test e2e -- --nocapture
//! ```

use bsdkrun::Sandbox;

fn gated() -> bool {
    if std::env::var("BSDKRUN_SDK_E2E").is_err() {
        eprintln!("skipping: set BSDKRUN_SDK_E2E=1 to run against a real bsdkrun binary");
        return false;
    }
    true
}

#[test]
fn list_machines_against_the_real_binary() {
    if !gated() {
        return;
    }
    // Just proves discovery + JSON parsing against the real CLI; boots nothing.
    let rows = Sandbox::list(true).expect("bsdkrun ps --json should parse");
    eprintln!("{} machine(s) known to this host", rows.len());
}
