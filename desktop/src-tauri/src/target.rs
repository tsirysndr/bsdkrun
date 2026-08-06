//! Where this app's bsdkrun lives: on this machine, or behind a daemon.
//!
//! The Settings field that used to be "path to the bsdkrun binary" now accepts
//! either form, because the two are the same choice from the user's point of
//! view — *which bsdkrun am I driving* — and asking them to pick a mode first
//! would be a menu in front of a question they have already answered by typing
//! a path or a URL.
//!
//! A local target runs the CLI as a subprocess, exactly as before. A remote
//! target sends the same argv to a `bsdkrund` over gRPC, which runs it on the
//! host that owns the VMs. Everything above this layer is unchanged.

use std::path::PathBuf;

use crate::bsdkrun::BkError;

/// The resolved backend.
#[derive(Clone, Debug)]
pub enum Target {
    /// A `bsdkrun` binary on this machine.
    Local(PathBuf),
    /// A `bsdkrund` reachable over gRPC.
    Remote { endpoint: String, token: String },
}

impl Target {
    /// What to show the user as "the engine we are driving".
    pub fn describe(&self) -> String {
        match self {
            Target::Local(p) => p.display().to_string(),
            Target::Remote { endpoint, .. } => endpoint.clone(),
        }
    }
}

/// Does this look like a daemon URL rather than a filesystem path?
///
/// Only an explicit scheme counts. A bare `host:50051` is ambiguous with a
/// relative path, and silently treating a mistyped path as a network address —
/// then trying to connect to it — is a worse failure than saying the binary was
/// not found.
pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    ["grpc://", "grpcs://", "http://", "https://"]
        .iter()
        .any(|p| s.len() > p.len() && s.to_ascii_lowercase().starts_with(p))
}

/// Normalize a daemon URL to what tonic expects.
///
/// `grpc://` and `grpcs://` are accepted because they are what people write for
/// a gRPC endpoint, but they are not real URI schemes to an HTTP client, so
/// they are mapped onto `http://` and `https://`.
pub fn normalize_endpoint(s: &str) -> String {
    let s = s.trim().trim_end_matches('/');
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("grpcs://") {
        return format!("https://{}", &s[s.len() - rest.len()..]);
    }
    if let Some(rest) = lower.strip_prefix("grpc://") {
        return format!("http://{}", &s[s.len() - rest.len()..]);
    }
    s.to_string()
}

/// Resolve the settings into a target.
///
/// `token` is only meaningful for a remote target; it falls back to
/// `BSDKRUN_TOKEN` so a daemon on this machine works without retyping it.
pub fn resolve(binary_or_url: &str, token: &str) -> Result<Target, BkError> {
    let trimmed = binary_or_url.trim();

    if looks_like_url(trimmed) {
        let token = if token.trim().is_empty() {
            std::env::var("BSDKRUN_TOKEN").unwrap_or_default()
        } else {
            token.trim().to_string()
        };
        if token.is_empty() {
            return Err(BkError::Io(
                "this daemon URL needs an access token — the one bsdkrund printed on startup"
                    .into(),
            ));
        }
        return Ok(Target::Remote {
            endpoint: normalize_endpoint(trimmed),
            token,
        });
    }

    let over = (!trimmed.is_empty()).then_some(trimmed);
    Ok(Target::Local(crate::bsdkrun::resolve_binary(over)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_recognised_by_scheme_only() {
        assert!(looks_like_url("grpc://host:50051"));
        assert!(looks_like_url("grpcs://host:50051"));
        assert!(looks_like_url("http://127.0.0.1:50051"));
        assert!(looks_like_url("HTTPS://Host:50051"));

        // A path is a path, even one that mentions a scheme-like word.
        assert!(!looks_like_url("/usr/local/bin/bsdkrun"));
        assert!(!looks_like_url("bsdkrun"));
        assert!(!looks_like_url(""));
        // Ambiguous with a relative path, so deliberately not a URL.
        assert!(!looks_like_url("host:50051"));
        // A scheme with nothing after it is not an endpoint.
        assert!(!looks_like_url("grpc://"));
    }

    #[test]
    fn grpc_schemes_map_onto_http() {
        assert_eq!(normalize_endpoint("grpc://host:50051"), "http://host:50051");
        assert_eq!(
            normalize_endpoint("grpcs://host:50051"),
            "https://host:50051"
        );
        // Already-HTTP endpoints pass through, trailing slash trimmed.
        assert_eq!(
            normalize_endpoint("http://host:50051/"),
            "http://host:50051"
        );
        assert_eq!(
            normalize_endpoint("https://vps.example.com:50051"),
            "https://vps.example.com:50051"
        );
        // Case in the scheme must not corrupt the authority.
        assert_eq!(normalize_endpoint("GRPC://Host:50051"), "http://Host:50051");
    }

    #[test]
    fn a_remote_target_requires_a_token() {
        std::env::remove_var("BSDKRUN_TOKEN");
        let err = resolve("grpc://host:50051", "").unwrap_err();
        assert!(
            err.to_string().contains("access token"),
            "unhelpful error: {err}"
        );

        let t = resolve("grpc://host:50051", "secret").unwrap();
        match t {
            Target::Remote { endpoint, token } => {
                assert_eq!(endpoint, "http://host:50051");
                assert_eq!(token, "secret");
            }
            other => panic!("expected a remote target, got {other:?}"),
        }
    }

    /// A path is resolved as a binary, never as an endpoint — even a bad one,
    /// and even when a token happens to be set.
    ///
    /// Note this does not assert that a missing path *fails*: `resolve_binary`
    /// deliberately falls back to PATH and the usual install locations when the
    /// configured override is not there, which predates this change.
    #[test]
    fn a_path_is_never_treated_as_an_endpoint() {
        for path in ["/nonexistent/bsdkrun", "bsdkrun", ""] {
            match resolve(path, "irrelevant") {
                Ok(Target::Local(_)) => {}
                Ok(Target::Remote { .. }) => panic!("{path:?} was taken for a URL"),
                // No binary anywhere on this machine is fine; it must simply
                // not be an *endpoint* error.
                Err(e) => assert!(
                    !e.to_string().contains("access token"),
                    "{path:?} failed as a URL: {e}"
                ),
            }
        }
    }
}
