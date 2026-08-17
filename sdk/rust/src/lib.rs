//! `bsdkrun` — a Rust SDK for [bsdkrun](https://github.com/tsirysndr/bsdkrun),
//! a Firecracker-style microVM launcher for BSD, Linux, and unikernel guests.
//!
//! A thin, blocking wrapper: locally it builds argv, shells out to the
//! `bsdkrun` binary, and parses the JSON output; remotely, [`Client`] drives
//! the same operations against a `bsdkrund` daemon's GraphQL API. No async
//! runtime anywhere.
//!
//! ```no_run
//! use bsdkrun_sdk::Sandbox;
//!
//! let sandbox = Sandbox::linux("alpine")
//!     .cpus(2)
//!     .mem(1024)
//!     .port("8080:80")
//!     .command(["sleep", "300"])
//!     .create()?;
//!
//! println!("{}", sandbox.exec(["uname", "-a"])?.text());
//! sandbox.stop()?;
//! # Ok::<(), bsdkrun_sdk::Error>(())
//! ```
//!
//! Host-level operations live in the [`images`], [`volumes`], [`networks`]
//! and [`system`] modules; the remote client in [`Client`].

mod args;
mod binary;
pub mod cache;
mod client;
mod error;
mod filesystem;
mod process;
mod sandbox;
pub mod transport;
mod types;

pub mod images;
pub mod networks;
pub mod system;
pub mod volumes;

pub use binary::{reset_binary_cache, resolve_binary, set_binary_path};
pub use cache::{Cache, CacheEntry, RestoreResult};
pub use client::{
    BranchBuilder, BsdOs, Client, FollowLogsBuilder, RunBsdBuilder, RunFlavorBuilder,
    RunLinuxBuilder, RunNanosBuilder, RunOsvBuilder, RunSolo5Builder, RunUnikraftBuilder,
    ShellBuilder, ShellSession, Subscription,
};
pub use error::{Error, Result};
pub use filesystem::FileSystem;
pub use process::{run, run_binary, run_checked, spawn, BinaryResult, RawResult};
pub use sandbox::{
    CommandBuilder, FirmwareBuilder, FreebsdBuilder, KernelBuilder, LinuxBuilder, NanosBuilder,
    NetbsdBuilder, OsvBuilder, Sandbox, Solo5Builder, SshSetupBuilder, TailscaleUpBuilder,
    UnikraftBuilder, UpdateBuilder,
};
pub use transport::{normalize_url, ws_url, TOKEN_ENV, URL_ENV};
pub use types::{
    CommandResult, ExecResult, ImageInfo, NetworkInfo, PortForward, RemoteExecResult, SandboxInfo,
    ShellSessionInfo, SnapshotInfo, VolumeInfo,
};
