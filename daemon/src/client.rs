//! Client side: connecting to a remote daemon.
//!
//! Exported so the `bsdkrun` CLI and the desktop app can link this crate and
//! talk to a daemon instead of forking a local CLI. Connecting to a daemon is
//! always opt-in — with no endpoint configured, both keep running the CLI
//! directly on the local machine, which stays the default and is never removed.
//!
//! Configuration follows the `DOCKER_HOST` convention:
//!
//! ```text
//! BSDKRUN_HOST=https://vps.example.com:50051   # or http://127.0.0.1:50051
//! BSDKRUN_TOKEN=<the daemon's token>
//! ```

use anyhow::{bail, Context, Result};
use tonic::metadata::MetadataValue;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Status};

use crate::pb::bsdkrun_client::BsdkrunClient;

pub const HOST_ENV: &str = "BSDKRUN_HOST";
pub const TOKEN_ENV: &str = "BSDKRUN_TOKEN";

/// A ready-to-use client with the bearer token attached to every request.
pub type Client = BsdkrunClient<InterceptedService<Channel, BearerAuth>>;

/// Attaches `authorization: Bearer <token>` to outgoing requests.
#[derive(Clone)]
pub struct BearerAuth {
    header: MetadataValue<tonic::metadata::Ascii>,
}

impl tonic::service::Interceptor for BearerAuth {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        req.metadata_mut()
            .insert("authorization", self.header.clone());
        Ok(req)
    }
}

/// Where to reach a daemon, and with what credential.
#[derive(Clone, Debug)]
pub struct RemoteConfig {
    pub endpoint: String,
    pub token: String,
}

impl RemoteConfig {
    /// Read the endpoint and token from the environment.
    ///
    /// Returns `Ok(None)` when no endpoint is set, which is the signal to run
    /// the CLI locally. A host *without* a token is an error rather than a
    /// silent fallback to local execution: quietly running a command on the
    /// wrong machine is far worse than refusing.
    pub fn from_env() -> Result<Option<Self>> {
        let Some(endpoint) = std::env::var(HOST_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty())
        else {
            return Ok(None);
        };
        let token = std::env::var(TOKEN_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .with_context(|| format!("{HOST_ENV} is set but {TOKEN_ENV} is not"))?;
        Ok(Some(Self {
            endpoint: endpoint.trim().to_string(),
            token: token.trim().to_string(),
        }))
    }
}

/// Connect to a daemon. `https://` endpoints use TLS with the platform's root
/// certificates; `http://` endpoints are plaintext and should only be used over
/// loopback or an already-encrypted tunnel.
pub async fn connect(cfg: &RemoteConfig) -> Result<Client> {
    let mut endpoint = Endpoint::from_shared(cfg.endpoint.clone())
        .with_context(|| format!("invalid endpoint {:?}", cfg.endpoint))?
        // Interactive sessions send one small frame per keystroke; Nagle would
        // add a round trip of latency to every one of them.
        .tcp_nodelay(true)
        .http2_keep_alive_interval(std::time::Duration::from_secs(30))
        .keep_alive_while_idle(true);

    if cfg.endpoint.starts_with("https://") {
        endpoint = endpoint
            .tls_config(ClientTlsConfig::new().with_native_roots())
            .context("configuring TLS")?;
    }

    let channel = endpoint
        .connect()
        .await
        .with_context(|| format!("connecting to bsdkrun daemon at {}", cfg.endpoint))?;

    let header: MetadataValue<_> = format!("Bearer {}", cfg.token)
        .parse()
        .context("token contains characters that cannot go in a header")?;

    Ok(BsdkrunClient::with_interceptor(
        channel,
        BearerAuth { header },
    ))
}

/// Connect using the environment, or fail explaining that none was configured.
pub async fn connect_from_env() -> Result<Client> {
    match RemoteConfig::from_env()? {
        Some(cfg) => connect(&cfg).await,
        None => bail!("{HOST_ENV} is not set; nothing to connect to"),
    }
}
