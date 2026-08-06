//! Driving a remote `bsdkrund` over gRPC.
//!
//! The daemon's generic `Run` RPC takes an argv and streams back stdout,
//! stderr and an exit code — the same three things a local subprocess gives
//! us. So a remote target can be made to look exactly like a local one at the
//! level [`crate::bsdkrun`] works at, and the 30-odd commands above it never
//! learn which they are talking to.

use bsdkrun_daemon::client::{connect, Client, RemoteConfig};
use bsdkrun_daemon::pb::{
    exec_input, output_chunk, run_input, ExecInput, ExecStart, LogsRequest, Resize, RunInput,
    RunStart,
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::bsdkrun::BkError;

/// The collected result of a remote command, mirroring `std::process::Output`.
#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

async fn client(endpoint: &str, token: &str) -> Result<Client, BkError> {
    connect(&RemoteConfig {
        endpoint: endpoint.to_string(),
        token: token.to_string(),
    })
    .await
    .map_err(|e| BkError::Io(format!("connecting to {endpoint}: {e}")))
}

/// Run an argv on the daemon and collect everything it produced.
pub async fn run(endpoint: &str, token: &str, args: &[&str]) -> Result<Output, BkError> {
    let mut c = client(endpoint, token).await?;

    // A single `start` message, then half-close: the daemon treats that as
    // "no more stdin", not as a cancellation.
    let input = tokio_stream::once(RunInput {
        payload: Some(run_input::Payload::Start(RunStart {
            args: args.iter().map(|s| s.to_string()).collect(),
            tty: false,
            size: None,
        })),
    });

    let mut stream = c
        .run(input)
        .await
        .map_err(|e| {
            BkError::Io(format!(
                "`bsdkrun {}` on the daemon: {}",
                args.join(" "),
                e.message()
            ))
        })?
        .into_inner();

    let (mut stdout, mut stderr, mut code) = (Vec::new(), Vec::new(), -1);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| BkError::Io(e.message().to_string()))?;
        match chunk.payload {
            Some(output_chunk::Payload::Stdout(b)) => stdout.extend_from_slice(&b),
            Some(output_chunk::Payload::Stderr(b)) => stderr.extend_from_slice(&b),
            Some(output_chunk::Payload::ExitCode(c)) => code = c,
            None => {}
        }
    }

    Ok(Output {
        code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Run a boot command and return the new machine's id.
///
/// The daemon always launches detached, so unlike the local path there is no
/// pipe-holding grandchild to work around — the id comes back on stdout and the
/// stream ends by itself.
pub async fn run_detached(endpoint: &str, token: &str, args: &[&str]) -> Result<String, BkError> {
    let out = run(endpoint, token, args).await?;
    if out.code != 0 {
        return Err(BkError::NonZero {
            code: out.code,
            stderr: out.stderr.trim().to_string(),
        });
    }
    out.stdout
        .lines()
        .map(str::trim)
        .find(|l| l.len() == 12 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_string)
        .ok_or_else(|| BkError::Io("the machine started but reported no id".into()))
}

// ---- interactive sessions --------------------------------------------------

/// What the UI can send into a live remote session.
pub enum Input {
    Data(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Close,
}

/// Open an interactive session on a remote machine.
///
/// The daemon allocates the pty on *its* host and bridges it to this stream, so
/// the guest shell sees a real terminal exactly as it does locally — see the
/// daemon's `pty` module. Here we only move bytes.
///
/// Returns the channel to write into; output and the exit code arrive through
/// the callbacks.
#[allow(clippy::too_many_arguments)]
pub fn exec_session(
    endpoint: String,
    token: String,
    machine_id: String,
    command: Vec<String>,
    rows: u16,
    cols: u16,
    on_output: impl Fn(Vec<u8>) + Send + 'static,
    on_exit: impl FnOnce(Option<i32>) + Send + 'static,
) -> mpsc::UnboundedSender<Input> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Input>();

    tokio::spawn(async move {
        let mut c = match client(&endpoint, &token).await {
            Ok(c) => c,
            Err(_) => {
                on_exit(None);
                return;
            }
        };

        // The request stream: `start`, then whatever the UI sends. It must stay
        // open for the life of the session — half-closing it would only mean
        // "no more stdin", but there is no reason to give up the ability to type.
        let (req_tx, req_rx) = mpsc::unbounded_channel::<ExecInput>();
        let _ = req_tx.send(ExecInput {
            payload: Some(exec_input::Payload::Start(ExecStart {
                id: machine_id,
                command,
                env: vec![],
                tty: true,
                size: Some(Resize {
                    rows: rows as u32,
                    cols: cols as u32,
                }),
            })),
        });

        let forward = req_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let payload = match msg {
                    Input::Data(b) => exec_input::Payload::Stdin(b),
                    Input::Resize { rows, cols } => exec_input::Payload::Resize(Resize {
                        rows: rows as u32,
                        cols: cols as u32,
                    }),
                    // Dropping the request stream ends the RPC, and the daemon
                    // kills the pty when the response stream goes with it.
                    Input::Close => break,
                };
                if forward
                    .send(ExecInput {
                        payload: Some(payload),
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        let stream = match c
            .exec(tokio_stream::wrappers::UnboundedReceiverStream::new(req_rx))
            .await
        {
            Ok(s) => s.into_inner(),
            Err(_) => {
                on_exit(None);
                return;
            }
        };
        let mut stream = stream;

        let mut code = None;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            match chunk.payload {
                Some(output_chunk::Payload::Stdout(b)) | Some(output_chunk::Payload::Stderr(b)) => {
                    on_output(b)
                }
                Some(output_chunk::Payload::ExitCode(c)) => {
                    code = Some(c);
                    break;
                }
                None => {}
            }
        }
        on_exit(code);
    });

    tx
}

/// Follow a remote machine's console log, a line at a time.
///
/// Returns a handle that stops the stream when aborted.
pub fn logs_session(
    endpoint: String,
    token: String,
    machine_id: String,
    on_line: impl Fn(String) + Send + 'static,
    on_end: impl FnOnce() + Send + 'static,
) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let mut c = match client(&endpoint, &token).await {
            Ok(c) => c,
            Err(_) => {
                on_end();
                return;
            }
        };
        let stream = c
            .logs(LogsRequest {
                id: machine_id,
                follow: true,
                boot: false,
            })
            .await;
        let Ok(stream) = stream else {
            on_end();
            return;
        };
        let mut stream = stream.into_inner();

        // The RPC streams bytes; the UI wants lines.
        let mut buf = String::new();
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else { break };
            let bytes = match chunk.payload {
                Some(output_chunk::Payload::Stdout(b)) | Some(output_chunk::Payload::Stderr(b)) => {
                    b
                }
                Some(output_chunk::Payload::ExitCode(_)) => break,
                None => continue,
            };
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(nl) = buf.find('\n') {
                let line: String = buf.drain(..=nl).collect();
                let line = line.trim_end().to_string();
                if !line.is_empty() {
                    on_line(line);
                }
            }
        }
        on_end();
    })
    .abort_handle()
}

#[cfg(test)]
mod tests {
    /// The id is picked out by shape, so provisioning output on stdout cannot
    /// be mistaken for it.
    #[test]
    fn machine_ids_are_recognised_by_shape() {
        let pick = |s: &str| {
            s.lines()
                .map(str::trim)
                .find(|l| l.len() == 12 && l.chars().all(|c| c.is_ascii_hexdigit()))
                .map(str::to_string)
        };
        assert_eq!(
            pick("pulling alpine\nfab8f81e4f91\n"),
            Some("fab8f81e4f91".into())
        );
        assert_eq!(pick("pulling alpine\ndone\n"), None);
        assert_eq!(pick(""), None);
    }
}
