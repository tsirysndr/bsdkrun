//! `bsdkrun-supervisor` — runs one bsdkrun command in a process of its own.
//!
//! It exists so `bsdkrund` does not have to link a hypervisor. Two things the
//! daemon cannot do in-process end up here:
//!
//!   * **Booting.** The detached boot `fork()`s and the child *becomes* the
//!     machine. Forking a multithreaded tokio process and then doing libkrun
//!     init, sqlite and tracing in the child risks deadlocking on a lock some
//!     other thread held at fork time — and it would tie every VM's life to the
//!     daemon's. This process has one thread and no server in it, and the
//!     machine it forks outlives both it and the daemon.
//!   * **Long jobs that report progress on stdout** — `fetch`, `flavor build` —
//!     and the daemon's passthrough RPC, whose point is to run a command line.
//!
//! Two shapes, because the daemon has two kinds of caller:
//!
//! ```text
//! bsdkrun-supervisor run '{"Linux":{…}}'   a typed command, as the daemon built it
//! bsdkrun-supervisor cli -- linux -d alpine   a command line, parsed by the engine
//! ```
//!
//! `run` is what almost everything uses: the daemon hands over a
//! [`bsdkrun_core::cli::Command`] it constructed as a *value*, so there is no
//! argv to get wrong in between. `cli` exists only for the passthrough RPC,
//! which is handed a command line by its caller.
//!
//! Not a user-facing tool. It ships beside `bsdkrund`, which finds it there.

use anyhow::{Context, Result};
use bsdkrun_core::cli::{Cli, Command as CoreCommand};
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "bsdkrun-supervisor",
    version,
    about = "Runs one bsdkrun command in its own process (used by bsdkrund; not a user-facing tool)"
)]
struct Args {
    #[command(subcommand)]
    cmd: Mode,
}

#[derive(clap::Subcommand)]
enum Mode {
    /// Run a JSON-encoded `Command`, as the daemon constructed it.
    Run {
        /// The command, as JSON.
        spec: String,
    },
    /// Run a bsdkrun command line, parsed by the engine's own definition.
    Cli {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();

    // stdout belongs to the machine — an id, a console, a command's output — so
    // diagnostics go to stderr, which is the progress half of a launch stream
    // as the daemon reads it back.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    bsdkrun_core::init_host();

    match args.cmd {
        Mode::Run { spec } => {
            let cmd: CoreCommand = serde_json::from_str(&spec).context("decoding the command")?;
            bsdkrun_core::dispatch(cmd)
        }
        Mode::Cli { args } => {
            let cli = Cli::try_parse_from(
                std::iter::once("bsdkrun".to_string()).chain(args.into_iter()),
            )?;
            bsdkrun_core::krun::Ctx::set_log_level(cli.log_level).ok();
            bsdkrun_core::dispatch(cli.cmd)
        }
    }
}
