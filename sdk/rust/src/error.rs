//! The one error type every fallible call in this SDK returns.
//!
//! Mirrors the Python SDK's exception hierarchy, flattened into an enum:
//! `BsdkrunError` becomes [`Error`] itself, and `AuthError` — a subclass of
//! `GraphQLError` over there — becomes its own variant here, with [`Error::code`]
//! preserving the "an auth failure always carries `UNAUTHENTICATED`" contract.

/// `Result` alias used across the SDK.
pub type Result<T> = std::result::Result<T, Error>;

/// Every error the SDK raises.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The `bsdkrun` binary could not be located on the host.
    #[error(
        "could not find the \"bsdkrun\" binary. Set BSDKRUN_BIN, add it to PATH, \
         or call set_binary_path(). Looked in: {}",
        searched.join(", ")
    )]
    BinaryNotFound {
        /// Every candidate location, in the order it was tried.
        searched: Vec<String>,
    },

    /// A `bsdkrun` invocation (or a guest command run through it) exited
    /// non-zero.
    #[error("{}", command_failed_message(*exit_code, command, stderr))]
    CommandFailed {
        exit_code: i32,
        stdout: String,
        stderr: String,
        /// A short label for what was run, e.g. `"bsdkrun stop"`.
        command: String,
    },

    /// No machine matched the given id / prefix.
    #[error("no sandbox found matching id {id:?}")]
    SandboxNotFound { id: String },

    /// A guest filesystem operation was refused (see [`crate::FileSystem`]).
    #[error("{message}")]
    FileTransfer {
        /// The path that could not be transferred.
        path: String,
        message: String,
    },

    /// A GraphQL- or transport-level failure talking to a remote `bsdkrund`.
    ///
    /// `code` carries the response's `extensions.code` when the daemon set one
    /// (e.g. `"INVALID_ARGUMENT"`, `"FAILED"`); it is `None` for a transport
    /// failure (the daemon was unreachable) or a malformed response.
    #[error("{message}")]
    GraphQL {
        message: String,
        code: Option<String>,
    },

    /// The daemon rejected the bearer token: an HTTP 401, a GraphQL error with
    /// `extensions.code == "UNAUTHENTICATED"`, or the WebSocket closing before
    /// `connection_ack` was ever received.
    #[error("{message}")]
    Auth { message: String },

    /// An option combination the SDK refuses before running anything — a
    /// missing required builder field, a URL configured without a token.
    #[error("{0}")]
    InvalidInput(String),

    /// A host-side I/O failure spawning or driving the `bsdkrun` process.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Output that should have been JSON was not.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// The daemon's `extensions.code`, when there is one. [`Error::Auth`]
    /// always answers `UNAUTHENTICATED`, matching the Python SDK where
    /// `AuthError` is a `GraphQLError` with that code baked in.
    pub fn code(&self) -> Option<&str> {
        match self {
            Error::GraphQL { code, .. } => code.as_deref(),
            Error::Auth { .. } => Some("UNAUTHENTICATED"),
            _ => None,
        }
    }

    /// The default auth failure, shared by the HTTP 401 and closed-before-ack
    /// paths.
    pub(crate) fn auth_default() -> Error {
        Error::Auth {
            message: "the daemon rejected this token".to_string(),
        }
    }
}

fn command_failed_message(exit_code: i32, command: &str, stderr: &str) -> String {
    let mut message = format!("command failed (exit {exit_code}): {command}");
    let trimmed = stderr.trim();
    if !trimmed.is_empty() {
        message.push('\n');
        message.push_str(trimmed);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_failed_appends_stderr_only_when_present() {
        let quiet = Error::CommandFailed {
            exit_code: 2,
            stdout: String::new(),
            stderr: "   ".into(),
            command: "bsdkrun stop".into(),
        };
        assert_eq!(quiet.to_string(), "command failed (exit 2): bsdkrun stop");

        let loud = Error::CommandFailed {
            exit_code: 1,
            stdout: String::new(),
            stderr: "boom\n".into(),
            command: "bsdkrun rm".into(),
        };
        assert_eq!(
            loud.to_string(),
            "command failed (exit 1): bsdkrun rm\nboom"
        );
    }

    #[test]
    fn auth_reports_the_unauthenticated_code() {
        assert_eq!(Error::auth_default().code(), Some("UNAUTHENTICATED"));
        let gql = Error::GraphQL {
            message: "no".into(),
            code: Some("FAILED".into()),
        };
        assert_eq!(gql.code(), Some("FAILED"));
    }
}
