//! Bearer-token authentication.
//!
//! The daemon can start a VM, open a root shell in it and read any file the
//! host user can, so every RPC is authenticated — there is no anonymous or
//! read-only tier. The token is either supplied by the operator (`--token` /
//! `BSDKRUN_TOKEN`) or generated at startup and printed once.
//!
//! A token is a bearer credential, so on an untrusted network it must be
//! carried over TLS (`--tls-cert`/`--tls-key`) or an encrypted tunnel;
//! otherwise it is readable by anyone on the path. See the crate README.

use std::fs::File;
use std::io::Read;

use anyhow::{Context, Result};
use tonic::service::Interceptor;
use tonic::{Request, Status};

/// 32 bytes of entropy, hex-encoded to 64 characters.
const TOKEN_BYTES: usize = 32;

/// Generate a token from the OS CSPRNG.
///
/// `/dev/urandom` rather than a crate: the daemon only ever runs on Unix hosts
/// (macOS and the BSD/Linux hypervisor hosts bsdkrun targets), and this keeps a
/// security-relevant path free of a dependency's version churn.
pub fn generate_token() -> Result<String> {
    random_hex(TOKEN_BYTES)
}

/// `n` random bytes, hex-encoded. Also used for shell session ids, which are
/// handed out over the API and should not be guessable by a client that holds
/// a session it was not given.
pub fn random_hex(n: usize) -> Result<String> {
    let mut buf = vec![0u8; n];
    File::open("/dev/urandom")
        .context("opening /dev/urandom")?
        .read_exact(&mut buf)
        .context("reading /dev/urandom")?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// Rejects any request that does not carry the expected token.
#[derive(Clone)]
pub struct TokenAuth {
    token: String,
}

impl TokenAuth {
    pub fn new(token: String) -> Self {
        Self { token }
    }

    /// Whether a presented credential is the expected one. `None` (nothing
    /// presented) is never a match.
    ///
    /// Shared by gRPC and HTTP so there is exactly one place that decides who
    /// may drive this host, and only one comparison to get right.
    pub fn matches(&self, presented: Option<&str>) -> bool {
        match presented {
            Some(t) => constant_time_eq(t.as_bytes(), self.token.as_bytes()),
            None => false,
        }
    }
}

impl Interceptor for TokenAuth {
    fn call(&mut self, req: Request<()>) -> Result<Request<()>, Status> {
        let md = req.metadata();
        // `authorization: Bearer <token>` is the standard form; the explicit
        // header is there for clients (and `grpcurl -H`) that would rather not
        // deal with the Bearer prefix.
        let presented = md
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                let (scheme, rest) = v.split_once(' ')?;
                scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
            })
            .or_else(|| md.get("x-bsdkrun-token").and_then(|v| v.to_str().ok()));

        match presented {
            Some(_) if self.matches(presented) => Ok(req),
            Some(_) => Err(Status::unauthenticated("invalid token")),
            None => Err(Status::unauthenticated(
                "missing token: send `authorization: Bearer <token>`",
            )),
        }
    }
}

/// Compare without leaking the position of the first difference through timing.
///
/// The length check is deliberately *not* an early return on content: lengths
/// are not secret (the token length is fixed and public), but the byte
/// comparison must always scan the whole input.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_64_hex_chars_and_distinct() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_eq!(a.len(), TOKEN_BYTES * 2);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
