//! bsdkrun daemon: a token-authenticated gRPC front end over the bsdkrun engine.
//!
//! The daemon exists so a machine that can actually run VMs — a Linux KVM box,
//! a bare-metal server, a VPS — can be driven from somewhere else. It links
//! `bsdkrun-core` directly, so it *is* bsdkrun rather than a wrapper around the
//! CLI: there need not be a `bsdkrun` binary on the host at all, and the daemon
//! and the CLI cannot drift because there is one implementation.
//!
//! Running the CLI directly on the host is unaffected and remains the default
//! everywhere; pointing a client at a daemon is opt-in.
//!
//! * [`ops`] — every operation, against the engine, independent of transport.
//! * [`service`] — every RPC, mapped to an operation.
//! * [`graphql`] — the web frontend's API, over the same operations.
//! * [`supervisor`] — the few operations that need their own process, run as
//!   this daemon's own binary rather than the CLI.
//! * [`pty`] — interactive sessions (remote shells) over a real pty.
//! * [`auth`] — bearer-token authentication.
//! * [`client`] — the client half, for the CLI and desktop app.
//!
//! The server half is behind the default `server` feature. A consumer that only
//! wants to *talk to* a daemon — the desktop app — turns it off and gets just
//! the generated types and the client, rather than a web server, a GraphQL
//! engine and a pty implementation it will never use.

pub mod client;

#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod supervisor;
#[cfg(feature = "server")]
pub mod graphql;
#[cfg(feature = "server")]
pub mod http;
#[cfg(feature = "server")]
pub mod ops;
#[cfg(feature = "server")]
pub mod pty;
#[cfg(feature = "server")]
pub mod service;
#[cfg(feature = "server")]
pub mod shell;
#[cfg(feature = "server")]
pub mod system;

/// Generated protobuf types and the gRPC client/server stubs.
pub mod pb {
    tonic::include_proto!("bsdkrun.v1");

    /// The compiled schema, served over gRPC reflection.
    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("bsdkrun_descriptor");
}
