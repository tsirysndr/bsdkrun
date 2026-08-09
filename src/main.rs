//! `bsdkrun` — the command-line front end.
//!
//! All of the machinery lives in `bsdkrun-core`; this binary parses a command
//! line, sets up logging, and hands the parsed command to the engine. The split
//! exists so `bsdkrund` can call that same engine directly instead of spawning
//! this binary and rebuilding its arguments as strings.

use anyhow::Result;
use bsdkrun_core::cli::Cli;
use bsdkrun_core::krun::Ctx;
use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    bsdkrun_core::init_host();

    let cli = Cli::parse();

    // Our own diagnostics go through `tracing`, written to stderr so they never
    // mingle with the guest console on stdout. `--log-level` sets a sensible
    // default verbosity (matching libkrun's 0..5 scale); `RUST_LOG` overrides.
    let default_filter = match cli.log_level {
        0 => "warn",
        1..=3 => "info",
        4 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter)),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    // libkrun's own internal logging (separate from ours) also honours the flag.
    Ctx::set_log_level(cli.log_level).ok();

    bsdkrun_core::dispatch(cli.cmd)
}
