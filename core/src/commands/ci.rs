//! `bsdkrun ci` — run tangled spindle workflows in microVMs.
//!
//! The tool itself is a Go binary (`ci/` at the repo root), embedded and
//! exec'd exactly as `pack` is — see that module for the full account of the
//! pattern, including why the extracted binary must be ad-hoc signed on
//! macOS (an entitled parent may only exec a validly signed child; unsigned
//! is SIGKILL before `main()`).
//!
//! Go rather than Rust for one decisive reason: the workflow schema and its
//! `when:` matching are imported from tangled.org/core itself, so a file
//! spindle accepts is a file `bsdkrun ci` accepts. Reimplementing someone
//! else's format here would drift within a month.
//!
//! The Go tool spins VMs through the bsdkrun Go SDK, which resolves its
//! binary from `$BSDKRUN_BIN` — set here to *this very executable*, so the
//! CI runner always drives the bsdkrun that launched it, not whatever an
//! unrelated PATH entry happens to be.

use std::os::unix::process::CommandExt;
use std::process::Command;

use anyhow::{bail, Context, Result};
use rust_embed::RustEmbed;

use crate::fetch::cache_dir;

/// The compiled `bsdkrun-ci` binary, if `core/build.rs` managed to build one.
/// Empty when the machine that ran `cargo build` had no Go toolchain.
#[derive(RustEmbed)]
#[folder = "src/ci-bin"]
struct CiBinary;

const BINARY_NAME: &str = "bsdkrun-ci";

/// Extract the embedded binary (if any), returning the path to it.
pub(crate) fn ci_binary() -> Result<std::path::PathBuf> {
    let embedded = CiBinary::get(BINARY_NAME).filter(|f| !f.data.is_empty());
    let Some(embedded) = embedded else {
        bail!(
            "this bsdkrun binary was built without ci support (no Go toolchain on the \
             machine that ran `cargo build`).\n\
             Install Go >= 1.25 and run `cargo build --release` again to enable `bsdkrun ci`."
        );
    };

    let dir = cache_dir()?.join("ci").join(crate::VERSION);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let bin = dir.join(BINARY_NAME);
    std::fs::write(&bin, &embedded.data).with_context(|| format!("writing {}", bin.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod +x {}", bin.display()))?;
    }
    #[cfg(target_os = "macos")]
    sign_adhoc(&bin)?;

    Ok(bin)
}

/// Extract and exec with `args`, replacing this process. Never returns on
/// success.
pub(crate) fn cmd_ci(args: &[String]) -> Result<()> {
    // Three verbs are Rust's, not the Go tool's — they touch the engine's
    // SQLite, whose schema lives here. Everything else passes through.
    match args.first().map(String::as_str) {
        // The runner pipes its finished trace here: one JSON array of spans
        // on stdin, straight into ci_spans. This is what makes run history
        // queryable with no OpenTelemetry collector anywhere.
        Some("__record-trace") => {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut buf)
                .context("reading spans from stdin")?;
            let spans: Vec<crate::db::CiSpanRow> =
                serde_json::from_str(&buf).context("parsing the span batch")?;
            crate::api::record_ci_spans(&spans)?;
            return Ok(());
        }
        Some("traces") => {
            let traces = crate::api::list_ci_traces(50)?;
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string(&traces)?);
                return Ok(());
            }
            if traces.is_empty() {
                println!("No CI runs recorded yet. `bsdkrun ci run` records one per run.");
                return Ok(());
            }
            #[allow(clippy::print_literal)]
            {
                println!(
                    "{:<34}  {:<22}  {:<8}  {:<9}  {}",
                    "TRACE", "WORKFLOW", "STATUS", "DURATION", "STARTED"
                );
            }
            for t in traces {
                let dur_ms = (t.end_ns - t.start_ns) / 1_000_000;
                println!(
                    "{:<34}  {:<22}  {:<8}  {:>7}ms  {}",
                    t.trace_id,
                    crate::commands::truncate(&t.workflow, 22),
                    if t.ok { "ok" } else { "failed" },
                    dur_ms,
                    chrono_ish(t.start_ns)
                );
            }
            return Ok(());
        }
        Some("spans") => {
            let trace_id = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .context("`ci spans` needs a trace id (from `ci traces`)")?;
            let spans = crate::api::list_ci_spans(trace_id)?;
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string(&spans)?);
                return Ok(());
            }
            for sp in spans {
                let dur_ms = (sp.end_ns - sp.start_ns) / 1_000_000;
                println!(
                    "{:<30}  {:<8}  {:>7}ms{}",
                    crate::commands::truncate(&sp.name, 30),
                    if sp.ok { "ok" } else { "failed" },
                    dur_ms,
                    sp.error
                        .as_deref()
                        .map(|e| format!("  {e}"))
                        .unwrap_or_default()
                );
            }
            return Ok(());
        }
        _ => {}
    }

    let bin = ci_binary()?;
    let this = std::env::current_exe().context("resolving the bsdkrun binary path")?;
    let err = Command::new(&bin)
        .args(args)
        // The SDK inside the Go tool boots VMs by shelling out to bsdkrun;
        // pinning it to this exact binary keeps a dev build driving the dev
        // build and a release driving the release.
        .env("BSDKRUN_BIN", this)
        .exec();
    Err(err).with_context(|| format!("running {}", bin.display()))
}

/// See `pack::sign_adhoc` — the same trap, the same fix.
#[cfg(target_os = "macos")]
fn sign_adhoc(bin: &std::path::Path) -> Result<()> {
    let out = Command::new("codesign")
        .args(["--force", "-s", "-"])
        .arg(bin)
        .output()
        .with_context(|| format!("running codesign on {}", bin.display()))?;
    if !out.status.success() {
        bail!(
            "codesign {} failed: {}",
            bin.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// A human date from unix nanos, without a chrono dependency: seconds
/// precision is plenty for a run listing.
fn chrono_ish(ns: i64) -> String {
    let secs = ns / 1_000_000_000;
    // Days since epoch → civil date (Howard Hinnant's algorithm, integer-only).
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}
