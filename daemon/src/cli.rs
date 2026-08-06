//! Driving the `bsdkrun` CLI as a subprocess.
//!
//! The daemon deliberately owns no VM logic of its own: it resolves the CLI
//! installed on the host and runs it, so a daemon always exposes exactly the
//! feature set of the binary next to it and can never drift from it.
//!
//! Three shapes cover every RPC:
//!   * [`Cli::output`] / [`Cli::json`] — short commands that exit on their own.
//!   * [`Cli::stream`] — long commands whose output is streamed as it appears.
//!   * [`Cli::detached`] — boot commands that fork a long-lived VM.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tonic::Status;

use crate::pb::{output_chunk, CommandResult, OutputChunk};

/// Input from the client for an interactive RPC, shared by the pty and piped
/// session types so [`crate::service`] can drive either through one enum.
pub enum SessionInput {
    Data(Vec<u8>),
    /// End of stdin. A pipe closes; a pty sends EOT instead (see [`crate::pty`]).
    Eof,
}

/// Directories where bsdkrun and its runtime tools (gvproxy, curl, tar, libkrun)
/// commonly live. A daemon started by systemd/launchd inherits a minimal PATH,
/// so we prepend these the same way the desktop app does for GUI launches.
const EXTRA_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// How long to keep draining a finished command's pipes.
///
/// A detached VM is a *grandchild* that inherits the pipes and holds them open
/// forever, so reading to EOF would hang. We instead stop when the CLI process
/// itself exits and drain briefly for whatever it wrote just before exiting.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Cap on a single read from a child pipe, and the streaming channel depth.
const READ_CHUNK: usize = 64 * 1024;
const CHANNEL_DEPTH: usize = 64;

#[derive(Clone, Debug)]
pub struct Cli {
    bin: PathBuf,
    path: String,
}

impl Cli {
    /// Resolve the CLI: an explicit override, then PATH, then well-known
    /// install locations. Fails fast at startup rather than per-RPC.
    pub fn resolve(override_path: Option<PathBuf>) -> Result<Self> {
        let path = augmented_path();
        let bin = if let Some(p) = override_path {
            if !p.exists() {
                anyhow::bail!("bsdkrun binary not found at {}", p.display());
            }
            p
        } else if let Ok(found) = which::which_in(
            "bsdkrun",
            Some(&path),
            std::env::current_dir().unwrap_or_default(),
        ) {
            found
        } else {
            let fallbacks = ["/opt/homebrew/bin/bsdkrun", "/usr/local/bin/bsdkrun"];
            fallbacks
                .iter()
                .map(PathBuf::from)
                .find(|p| p.exists())
                .context(
                    "bsdkrun binary not found on PATH. Install it, or point the daemon at it \
                     with --bsdkrun /path/to/bsdkrun",
                )?
        };
        Ok(Self { bin, path })
    }

    pub fn bin(&self) -> &PathBuf {
        &self.bin
    }

    /// The PATH handed to every spawned CLI process.
    pub fn env_path(&self) -> &str {
        &self.path
    }

    fn command(&self, args: &[String]) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.env("PATH", &self.path);
        cmd.args(args);
        cmd.stdin(Stdio::null());
        cmd
    }

    /// Run a command to completion and collect its output. A non-zero exit is
    /// returned in the result rather than raised: for many CLI subcommands
    /// (`ssh status`, `tailscale status`) a non-zero exit is a legitimate state
    /// the caller should see, not a transport error.
    pub async fn output(&self, args: &[String]) -> Result<CommandResult, Status> {
        let mut cmd = self.command(args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(300), cmd.output())
            .await
            .map_err(|_| {
                Status::deadline_exceeded(format!("`bsdkrun {}` timed out", args.join(" ")))
            })?
            .map_err(|e| Status::internal(format!("spawning bsdkrun: {e}")))?;
        Ok(CommandResult {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Run a `--json` subcommand and deserialize stdout. Unlike [`Cli::output`]
    /// a non-zero exit *is* an error here: there is no partial list to return.
    pub async fn json<T: DeserializeOwned>(&self, args: &[String]) -> Result<T, Status> {
        let res = self.output(args).await?;
        if res.exit_code != 0 {
            return Err(Status::internal(format!(
                "`bsdkrun {}` failed ({}): {}",
                args.join(" "),
                res.exit_code,
                res.stderr.trim()
            )));
        }
        serde_json::from_str(&res.stdout).map_err(|e| {
            Status::internal(format!("parsing `bsdkrun {}` output: {e}", args.join(" ")))
        })
    }

    /// Spawn a command and stream stdout/stderr as they arrive, finishing with
    /// a single `exit_code` frame.
    ///
    /// Reads are byte-oriented, not line-oriented, so progress output that
    /// redraws a line with `\r` (image pulls, provisioning) reaches the client
    /// as it happens instead of being buffered until a newline.
    pub fn stream(&self, args: &[String]) -> mpsc::Receiver<Result<OutputChunk, Status>> {
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let mut cmd = self.command(args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        // The CLI process is ours to kill if the client disconnects; a detached
        // VM it may have spawned is a grandchild and is never signalled.
        cmd.kill_on_drop(true);
        let display = args.join(" ");

        tokio::spawn(async move {
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("spawning bsdkrun: {e}"))))
                        .await;
                    return;
                }
            };
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            let out_task = stdout.map(|s| {
                let tx = tx.clone();
                tokio::spawn(async move { pump(s, tx, true).await })
            });
            let err_task = stderr.map(|s| {
                let tx = tx.clone();
                tokio::spawn(async move { pump(s, tx, false).await })
            });

            // Wait on the CLI process, not on pipe EOF — see DRAIN_GRACE.
            // A client that stops reading (disconnect, cancelled RPC) closes
            // the channel; returning then drops `child`, killing the CLI. This
            // matters most for `logs -f`, which otherwise never finishes.
            let status = tokio::select! {
                s = child.wait() => s,
                _ = tx.closed() => return,
            };

            // Let the readers pick up anything written just before exit, then
            // cut them loose so a pipe-holding grandchild can't wedge the RPC.
            for task in [out_task, err_task].into_iter().flatten() {
                if tokio::time::timeout(DRAIN_GRACE, task).await.is_err() {
                    // Task is still blocked on a read the grandchild keeps open.
                }
            }

            let code = match status {
                Ok(s) => s.code().unwrap_or(-1),
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("`bsdkrun {display}`: {e}"))))
                        .await;
                    return;
                }
            };
            let _ = tx
                .send(Ok(OutputChunk {
                    payload: Some(output_chunk::Payload::ExitCode(code)),
                }))
                .await;
        });

        rx
    }

    /// Launch a detached machine and return its id.
    ///
    /// CRITICAL: this must not read to EOF. `bsdkrun -d` forks a long-lived VM
    /// that inherits the pipes and keeps them open, so `.output()` would block
    /// forever. We read exactly the one id line the short-lived parent prints,
    /// while continuously draining stderr — if that pipe filled, the CLI would
    /// block on its next write and never exit.
    pub async fn detached(&self, args: &[String]) -> Result<String, Status> {
        let mut cmd = self.command(args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        // No kill_on_drop: the VM is a grandchild we must never signal.
        let mut child = cmd
            .spawn()
            .map_err(|e| Status::internal(format!("spawning bsdkrun: {e}")))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let err_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let err_buf2 = err_buf.clone();
        let err_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut b = err_buf2.lock().unwrap();
                if b.len() < 8000 {
                    b.push_str(&line);
                    b.push('\n');
                }
            }
        });

        let mut lines = BufReader::new(stdout).lines();
        let id = tokio::time::timeout(Duration::from_secs(600), lines.next_line())
            .await
            .map_err(|_| Status::deadline_exceeded("timed out waiting for the machine to start"))?
            .map_err(|e| Status::internal(e.to_string()))?;

        let out_task =
            tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

        let status = tokio::time::timeout(Duration::from_secs(600), child.wait())
            .await
            .map_err(|_| Status::deadline_exceeded("timed out launching machine"))?
            .map_err(|e| Status::internal(e.to_string()))?;

        out_task.abort();
        err_task.abort();

        if !status.success() {
            let stderr = err_buf.lock().unwrap().trim().to_string();
            return Err(Status::internal(format!(
                "bsdkrun exited with status {}: {stderr}",
                status.code().unwrap_or(-1)
            )));
        }

        match id {
            Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
            _ => Err(Status::internal("machine started but reported no id")),
        }
    }
}

/// A non-tty interactive command: stdout and stderr stay distinguishable (a pty
/// would merge them) and stdin is a real pipe that can be closed, so piping data
/// into a guest command works as it does locally.
///
/// Like [`crate::pty::PtySession`], this handle does not own the child's
/// lifetime — see that type for why. Cleanup is driven by the client dropping
/// the response stream.
pub struct PipedSession {
    input: mpsc::UnboundedSender<SessionInput>,
}

impl PipedSession {
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.input.send(SessionInput::Data(data));
    }

    pub fn eof(&self) {
        let _ = self.input.send(SessionInput::Eof);
    }
}

impl Cli {
    /// Spawn a command with all three streams piped, for interactive use
    /// without a terminal.
    pub fn spawn_piped(
        &self,
        args: &[String],
    ) -> Result<(PipedSession, mpsc::Receiver<Result<OutputChunk, Status>>), Status> {
        let mut cmd = self.command(args);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| Status::internal(format!("spawning bsdkrun: {e}")))?;

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<SessionInput>();

        tokio::spawn(async move {
            while let Some(msg) = input_rx.recv().await {
                match msg {
                    SessionInput::Data(b) => {
                        if stdin.write_all(&b).await.is_err() || stdin.flush().await.is_err() {
                            return;
                        }
                    }
                    // Dropping the handle closes the pipe, giving the guest
                    // command a real EOF on stdin.
                    SessionInput::Eof => return,
                }
            }
        });

        let out_tx = tx.clone();
        let err_tx = tx.clone();
        tokio::spawn(async move {
            let out = tokio::spawn(async move { pump(stdout, out_tx, true).await });
            let err = tokio::spawn(async move { pump(stderr, err_tx, false).await });

            let code = tokio::select! {
                status = child.wait() => status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1),
                // The client dropped the response stream (disconnect or
                // cancellation). Returning drops `child`, and kill_on_drop
                // tears the CLI process down. This is the only cleanup path:
                // the client half-closing its *input* stream just means "no
                // more stdin" and must not end the command.
                _ = tx.closed() => return,
            };

            for task in [out, err] {
                let _ = tokio::time::timeout(DRAIN_GRACE, task).await;
            }
            let _ = tx
                .send(Ok(OutputChunk {
                    payload: Some(output_chunk::Payload::ExitCode(code)),
                }))
                .await;
        });

        Ok((PipedSession { input: input_tx }, rx))
    }
}

/// Forward one pipe into the output channel until EOF, the child dies, or the
/// client hangs up (a send error).
async fn pump<R>(mut src: R, tx: mpsc::Sender<Result<OutputChunk, Status>>, is_stdout: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match src.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                let payload = if is_stdout {
                    output_chunk::Payload::Stdout(bytes)
                } else {
                    output_chunk::Payload::Stderr(bytes)
                };
                if tx
                    .send(Ok(OutputChunk {
                        payload: Some(payload),
                    }))
                    .await
                    .is_err()
                {
                    return; // client gone
                }
            }
        }
    }
}

/// PATH for spawned processes: the usual tool dirs plus whatever we inherited.
pub fn augmented_path() -> String {
    let mut parts: Vec<String> = EXTRA_PATHS.iter().map(|s| s.to_string()).collect();
    if let Ok(existing) = std::env::var("PATH") {
        for p in existing.split(':') {
            if !p.is_empty() && !parts.iter().any(|x| x == p) {
                parts.push(p.to_string());
            }
        }
    }
    parts.join(":")
}
