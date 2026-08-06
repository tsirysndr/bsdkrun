//! `bsdkrund` — the bsdkrun gRPC daemon.
//!
//! Typical use is a VM-capable host (Linux/KVM, bare metal, a VPS) that you
//! drive from a laptop:
//!
//! ```text
//! # on the server
//! bsdkrund --bind 0.0.0.0:50051 --tls-cert cert.pem --tls-key key.pem
//!
//! # on the laptop
//! export BSDKRUN_HOST=https://server:50051 BSDKRUN_TOKEN=<printed token>
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use bsdkrun_daemon::auth::{generate_token, TokenAuth};
use bsdkrun_daemon::cli::Cli as BsdkrunCli;
use bsdkrun_daemon::pb::bsdkrun_server::BsdkrunServer;
use bsdkrun_daemon::service::BsdkrunService;
use clap::Parser;
use tonic::transport::{Identity, Server, ServerTlsConfig};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "bsdkrund",
    version,
    about = "gRPC daemon for driving bsdkrun remotely"
)]
struct Args {
    /// Address to listen on. Defaults to loopback: exposing the daemon to a
    /// network is a deliberate act, since it hands out full VM and shell access.
    #[arg(long, default_value = "127.0.0.1:50051", env = "BSDKRUN_DAEMON_BIND")]
    bind: SocketAddr,

    /// Access token clients must present. Generated and printed when omitted.
    #[arg(long, env = "BSDKRUN_TOKEN", hide_env_values = true)]
    token: Option<String>,

    /// Path to the `bsdkrun` binary to drive (default: found on PATH).
    #[arg(long, env = "BSDKRUN_BIN")]
    bsdkrun: Option<PathBuf>,

    /// PEM certificate chain, to serve over TLS.
    #[arg(long, requires = "tls_key")]
    tls_cert: Option<PathBuf>,

    /// PEM private key matching `--tls-cert`.
    #[arg(long, requires = "tls_cert")]
    tls_key: Option<PathBuf>,

    /// Log verbosity (`RUST_LOG` overrides).
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    // Resolve the CLI up front: a daemon that cannot find `bsdkrun` is broken,
    // and failing at startup beats failing on every RPC.
    let cli = BsdkrunCli::resolve(args.bsdkrun)?;
    info!(bsdkrun = %cli.bin().display(), "driving bsdkrun");

    let (token, generated) = match args.token {
        Some(t) if !t.trim().is_empty() => (t.trim().to_string(), false),
        _ => (generate_token()?, true),
    };

    let tls = match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => {
            let cert =
                std::fs::read(cert).with_context(|| format!("reading {}", cert.display()))?;
            let key = std::fs::read(key).with_context(|| format!("reading {}", key.display()))?;
            Some(ServerTlsConfig::new().identity(Identity::from_pem(cert, key)))
        }
        _ => None,
    };

    // A bearer token on a plaintext socket is readable by anyone on the path.
    // On loopback that is fine; bound to a network it is worth being loud about.
    if tls.is_none() && !args.bind.ip().is_loopback() {
        warn!(
            bind = %args.bind,
            "listening on a non-loopback address without TLS — the access token is sent in \
             the clear. Use --tls-cert/--tls-key, or tunnel over SSH/WireGuard/tailscale."
        );
    }

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<BsdkrunServer<BsdkrunService>>()
        .await;

    // Reflection is served anonymously, so `grpcurl` and Postman can discover
    // the API without a local .proto and without a token. It exposes only the
    // schema — which is public in this repo anyway — never any machine state;
    // every actual RPC still goes through the token interceptor below.
    // The health schema is registered alongside our own so that a health probe
    // is discoverable too, rather than something a client has to know by name.
    let reflection_v1 = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(bsdkrun_daemon::pb::FILE_DESCRIPTOR_SET)
        .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
        .build_v1()
        .context("building the reflection service")?;
    // Older grpcurl builds only speak the v1alpha reflection service.
    let reflection_v1alpha = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(bsdkrun_daemon::pb::FILE_DESCRIPTOR_SET)
        .register_encoded_file_descriptor_set(tonic_health::pb::FILE_DESCRIPTOR_SET)
        .build_v1alpha()
        .context("building the v1alpha reflection service")?;

    let svc =
        BsdkrunServer::with_interceptor(BsdkrunService::new(cli), TokenAuth::new(token.clone()));

    print_banner(&args.bind, &token, generated, tls.is_some());

    let mut server = Server::builder()
        // Interactive sessions send one small frame per keystroke.
        .tcp_nodelay(true)
        .http2_keepalive_interval(Some(std::time::Duration::from_secs(30)));
    if let Some(tls) = tls {
        server = server.tls_config(tls).context("configuring TLS")?;
    }

    server
        .add_service(health_service)
        .add_service(reflection_v1)
        .add_service(reflection_v1alpha)
        .add_service(svc)
        .serve_with_shutdown(args.bind, async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutting down");
        })
        .await
        .context("serving")?;

    Ok(())
}

/// Print the token once on startup. A generated token exists nowhere else, so
/// this is the operator's only chance to copy it — it goes to stdout (not the
/// log) so `bsdkrund > token.txt` and piping both work.
fn print_banner(bind: &SocketAddr, token: &str, generated: bool, tls: bool) {
    let scheme = if tls { "https" } else { "http" };
    let host = if bind.ip().is_unspecified() {
        format!("<this-host>:{}", bind.port())
    } else {
        bind.to_string()
    };

    println!("bsdkrun daemon listening on {scheme}://{bind}");
    if generated {
        println!();
        println!("  access token (generated — shown only once):");
        println!("    {token}");
        println!();
        println!("  set BSDKRUN_TOKEN to reuse it across restarts.");
    } else {
        println!("  using the access token from --token/BSDKRUN_TOKEN");
    }
    println!();
    println!("  connect with:");
    println!("    export BSDKRUN_HOST={scheme}://{host}");
    println!("    export BSDKRUN_TOKEN={token}");
    println!();
}
