//! Fixtures shared by the gRPC and GraphQL suites.
//!
//! Two things are stubbed so the tests stay hermetic — no VM boots, no image
//! downloads, nothing that needs a hypervisor, so they run on any CI runner:
//!
//!   * **State.** Reads (`ps`, `images`, `volume ls`, …) now run *in* the
//!     daemon against the engine, so the tests point `BSDKRUN_STATE` at a temp
//!     directory and seed a real database with fixtures.
//!   * **The supervisor.** Booting and streaming still run in a separate
//!     process, so a stub stands in for it and records the command it was
//!     handed. Those recordings are JSON-encoded `Command`s rather than argv,
//!     which is what makes them worth asserting: a test pins the *meaning* of a
//!     request rather than the spelling of a flag.

#![allow(dead_code)] // each suite uses a different subset

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Mutex, OnceLock};

pub const STUB: &str = r#"#!/bin/sh
# The log lives beside the stub rather than coming from the environment: each
# test gets its own temp dir, and tests run in parallel in one process, so a
# shared env var would have them all writing to whichever log was set last.
LOG="$0.log"
for a in "$@"; do printf '%s\n' "$a" >> "$LOG"; done
printf -- '---\n' >> "$LOG"

SPEC="$2"
case "$SPEC" in
*'"Linux"'*|*'"Freebsd"'*|*'"Netbsd"'*|*'"Unikraft"'*|*'"Nanos"'*|*'"Osv"'*|*'"Flavor"'*)
  # A boot command prints the new machine id on stdout and exits.
  echo "m-boot-001"; exit 0 ;;
*'"Logs"'*)
  echo "line one"; echo "line two"; exit 0 ;;
*'"Exec"'*|*'"Shell"'*)
  # Echo back a marker plus whether a tty was requested, then read stdin so the
  # interactive tests have something to exercise.
  echo "EXEC_OK"
  case "$SPEC" in *'"tty":true'*) echo "TTY_REQUESTED" ;; esac
  cat
  exit 0 ;;
*'"Ssh"'*|*'"Tailscale"'*|*'"Systemd"'*|*'"Agent"'*|*'"Fetch"'*)
  echo "tool ok"; exit 0 ;;
*'"Probe"'*)
  echo "probe ok"; exit 0 ;;
*BSDKRUN_STUB_FAIL*)
  echo "boom" >&2; exit 3 ;;
esac

# `__cli -- <args…>`: the passthrough RPC.
if [ "$1" = "__cli" ]; then
  shift
  [ "$1" = "--" ] && shift
  case "$1" in
  fail) echo "boom" >&2; exit 3 ;;
  *) echo "ran: $*"; cat; exit 0 ;;
  esac
fi

echo "unknown command: $SPEC" >&2
exit 2
"#;

/// One seeded state directory for the whole test binary.
///
/// `BSDKRUN_STATE` is read from the environment, which is process-wide, so it
/// cannot be per-test. The fixtures below are only *read*, so sharing them
/// across tests running in parallel is safe; a test that mutates state uses
/// names of its own.
pub fn fixture_state() -> &'static Path {
    static STATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    STATE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("BSDKRUN_STATE", dir.path());
            std::env::set_var("BSDKRUN_CACHE", dir.path().join("cache"));
            // On its own thread for the same reason the daemon uses one: the
            // engine's database opens a runtime, which cannot nest inside the
            // test's.
            let path = dir.path().to_path_buf();
            std::thread::spawn(move || seed(&path)).join().unwrap();
            dir
        })
        .path()
}

/// Real processes standing in for running machines.
///
/// A fixture machine's pid has to belong to something actually alive, or the
/// engine reconciles the row to "exited" — and it must not be *this* process,
/// or a test that stops a machine signals the test runner. Each is a `sleep`
/// this module owns and outlives.
static LIVE: Mutex<Vec<Child>> = Mutex::new(Vec::new());

/// Spawn a process to stand in for a running machine, returning its pid.
pub fn live_process() -> i64 {
    let child = std::process::Command::new("sleep")
        .arg("120")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawning a stand-in process");
    let pid = child.id() as i64;
    LIVE.lock().unwrap().push(child);
    pid
}

/// Record a running machine of its own, for a test that mutates one.
///
/// The shared fixtures are read by tests running in parallel, so anything that
/// stops or removes a machine needs its own.
pub fn machine_with_live_process(id: &str) -> String {
    let state = fixture_state();
    let pid = live_process();
    let id = id.to_string();
    let dir = state.join("machines").join(&id).to_string_lossy().into_owned();
    let recorded = id.clone();
    std::thread::spawn(move || {
        bsdkrun_core::db::record_machine(
            &recorded, &recorded, "alpine", "linux", "sh", "running",
            Some(pid), true, 1, 512, &dir, None,
        );
    })
    .join()
    .unwrap();
    id
}

/// Write the machines, images, volumes, networks and flavors the read tests
/// assert on, through the engine's own database layer.
pub fn seed(state: &Path) {
    use bsdkrun_core::db;

    std::fs::create_dir_all(state).unwrap();
    // A running Linux machine on a network, and a running FreeBSD one.
    db::record_machine(
        "abc123", "web", "alpine", "linux", "sh", "running",
        Some(live_process()), true, 2, 1024,
        &state.join("machines/abc123").to_string_lossy(), None,
    );
    db::record_machine(
        "bsd456789abc", "fbsd", "disk.raw", "freebsd", "", "running",
        Some(live_process()), true, 2, 1024,
        &state.join("machines/bsd456789abc").to_string_lossy(), None,
    );
    // One that is not running, so `all` has something to include.
    db::record_machine(
        "dead00000001", "old", "alpine", "linux", "sh", "exited",
        None, true, 1, 512,
        &state.join("machines/dead00000001").to_string_lossy(), None,
    );

    let db = db::Db::open().unwrap();
    // Membership needs an assigned IP: a machine with a network but no address
    // has not joined yet, and is not counted as a member.
    db.set_machine_network("abc123", "devnet", "192.168.127.7")
        .unwrap();

    // An image whose size exceeds a 32-bit int, so the wire type is exercised.
    db::record_image("alpine:3.20", "sha256:deadbeef", 5_000_000_000, "/r");

    db.create_network(
        "devnet", "192.168.127.0/24", "192.168.127.1",
        &state.join("net/control.sock").to_string_lossy(),
        &state.join("net").to_string_lossy(),
        None,
    )
    .ok();

    // A volume whose directory exists (so it can be measured) and one whose
    // directory does not (so its size comes back absent).
    let vdir = db::volumes_dir().unwrap().join("data");
    std::fs::create_dir_all(&vdir).unwrap();
    std::fs::write(vdir.join("blob"), vec![0u8; 4096]).unwrap();
    db::record_volume("data", "linux", "b.img", &vdir.to_string_lossy());
    db::record_volume(
        "gone",
        "linux",
        "b.img",
        &state.join("no-such-dir/gone").to_string_lossy(),
    );
}

/// Write the stub somewhere and make it executable, returning its path and the
/// log it will record invocations to.
pub fn install_stub(dir: &Path) -> (PathBuf, PathBuf) {
    let stub = dir.join("bsdkrund-stub");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let log = dir.join("bsdkrund-stub.log");
    std::fs::write(&log, "").unwrap();
    (stub, log)
}

/// Every recorded invocation, each as its argv.
pub fn invocations(log: &Path) -> Vec<Vec<String>> {
    let raw = std::fs::read_to_string(log).unwrap_or_default();
    let mut all = Vec::new();
    let mut cur = Vec::new();
    for line in raw.lines() {
        if line == "---" {
            all.push(std::mem::take(&mut cur));
        } else {
            cur.push(line.to_string());
        }
    }
    all
}

/// Decode the JSON command out of a `__run` invocation.
pub fn decode(argv: &[String]) -> serde_json::Value {
    assert_eq!(
        argv.first().map(String::as_str),
        Some("__run"),
        "not a __run invocation: {argv:?}"
    );
    serde_json::from_str(&argv[1]).expect("the supervisor was handed valid JSON")
}
