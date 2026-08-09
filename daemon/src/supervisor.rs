//! Running a command in a *separate* process — `bsdkrun-supervisor`.
//!
//! Almost everything the daemon does happens in-process, against
//! `bsdkrun-core`. Two things cannot, and both go through here:
//!
//!   * **Booting.** The detached boot `fork()`s and the child *becomes* the
//!     machine. Forking a multithreaded tokio process and then doing libkrun
//!     init, sqlite and tracing in the child risks deadlocking on a lock some
//!     other thread held at fork time — and it would tie every VM's life to the
//!     daemon's. The supervisor has one thread and no server in it, and a
//!     machine it boots survives `systemctl restart bsdkrund`.
//!   * **Long jobs that report progress on stdout** — `fetch`, `flavor build` —
//!     and the passthrough RPC, whose whole point is to run a command line.
//!
//! It is a separate binary rather than this one re-exec'd, because it is where
//! libkrun is linked: keeping it out of `bsdkrund` is what lets the daemon stay
//! a static binary that runs on any distro. It is emphatically *not* the
//! `bsdkrun` CLI — the daemon looks for no such thing, and a host need not have
//! one at all.
//!
//! What crosses the process boundary is a typed
//! [`bsdkrun_core::cli::Command`], JSON-encoded, so there is still no argv
//! between the daemon and the engine to get wrong.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use bsdkrun_core::cli::Command as CoreCommand;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tonic::Status;

use crate::pb::{output_chunk, CommandResult, OutputChunk};

/// The binary this daemon hands boot-shaped work to. Shipped beside it.
pub const SUPERVISOR_BIN: &str = "bsdkrun-supervisor";

/// Its subcommand for a typed [`CoreCommand`], carried as JSON.
pub const RUN_SUBCOMMAND: &str = "run";

/// Its subcommand for a raw command line, used only by the passthrough RPC.
pub const CLI_SUBCOMMAND: &str = "cli";

/// Input from the client for an interactive RPC, shared by the pty and piped
/// session types so [`crate::service`] can drive either through one enum.
pub enum SessionInput {
    Data(Vec<u8>),
    /// End of stdin. A pipe closes; a pty sends EOT instead (see [`crate::pty`]).
    Eof,
}

/// Directories where the supervisor and the runtime tools a machine needs
/// (gvproxy, curl, tar) commonly live. A daemon started by systemd/launchd
/// inherits a minimal PATH, so we append these the same way the desktop app
/// does for GUI launches.
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
/// forever, so reading to EOF would hang. We instead stop when the supervisor
/// process itself exits and drain briefly for whatever it wrote just before.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Cap on a single read from a child pipe, and the streaming channel depth.
const READ_CHUNK: usize = 64 * 1024;
const CHANNEL_DEPTH: usize = 64;

#[derive(Clone, Debug)]
pub struct Supervisor {
    exe: PathBuf,
    path: String,
}

impl Supervisor {
    /// Find `bsdkrun-supervisor`: beside this binary first, then on PATH.
    ///
    /// Resolved at startup rather than per-request, because a daemon that
    /// cannot find it can boot nothing, and saying so once at startup beats
    /// failing on the first boot someone attempts.
    pub fn resolve(override_path: Option<PathBuf>) -> Result<Self> {
        let path = augmented_path();
        if let Some(p) = override_path {
            if !p.exists() {
                anyhow::bail!("no supervisor at {}", p.display());
            }
            return Ok(Self { exe: p, path });
        }
        // Beside the daemon is where a package puts it, and where a `cargo
        // build` leaves it, so it is worth preferring over whatever PATH says.
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.join(SUPERVISOR_BIN)))
            .filter(|p| p.exists());
        let exe = beside
            .or_else(|| {
                path.split(':')
                    .map(|d| std::path::Path::new(d).join(SUPERVISOR_BIN))
                    .find(|p| p.exists())
            })
            .with_context(|| {
                format!(
                    "{SUPERVISOR_BIN} not found beside this binary or on PATH. It ships with \
                     bsdkrund and is what actually boots machines; install it, or point the \
                     daemon at it with --supervisor /path/to/{SUPERVISOR_BIN}"
                )
            })?;
        Ok(Self { exe, path })
    }

    /// Point the supervisor at a specific binary instead of this process.
    ///
    /// For tests, which stand a stub in for the real supervisor so they can
    /// assert what the daemon asked for without booting a machine.
    pub fn with_exe(exe: PathBuf) -> Self {
        Self {
            exe,
            path: augmented_path(),
        }
    }

    pub fn exe(&self) -> &PathBuf {
        &self.exe
    }

    /// The PATH handed to every supervised process.
    pub fn env_path(&self) -> &str {
        &self.path
    }

    /// `bsdkrun-supervisor cli -- <args…>`: a bsdkrun command line, parsed by
    /// the engine's own clap definition. Only the passthrough RPC needs this —
    /// everything else hands over a typed command instead.
    pub fn argv_raw(&self, args: &[String]) -> Vec<String> {
        let mut argv = vec![CLI_SUBCOMMAND.to_string(), "--".to_string()];
        argv.extend(args.iter().cloned());
        argv
    }

    /// `bsdkrun-supervisor run <json>` for a parsed command.
    pub fn argv(&self, cmd: &CoreCommand) -> Result<Vec<String>, Status> {
        let spec = serde_json::to_string(cmd)
            .map_err(|e| Status::internal(format!("encoding the command: {e}")))?;
        Ok(vec![RUN_SUBCOMMAND.to_string(), spec])
    }

    fn command(&self, args: &[String]) -> Command {
        let mut cmd = Command::new(&self.exe);
        cmd.env("PATH", &self.path);
        cmd.args(args);
        cmd.stdin(Stdio::null());
        cmd
    }

    /// Run a command to completion and collect its output. A non-zero exit is
    /// returned in the result rather than raised: for several operations a
    /// non-zero exit is a legitimate state the caller should see, not a
    /// transport error.
    pub async fn output(&self, args: &[String]) -> Result<CommandResult, Status> {
        let mut cmd = self.command(args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(300), cmd.output())
            .await
            .map_err(|_| Status::deadline_exceeded("the supervised command timed out"))?
            .map_err(|e| Status::internal(format!("spawning the supervisor: {e}")))?;
        Ok(CommandResult {
            exit_code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Boot a machine and return its id.
    ///
    /// The supervisor prints the new machine's id on stdout and exits while the
    /// VM — its forked child — keeps running, reparented away from both it and
    /// this daemon.
    pub async fn detached(&self, cmd: &CoreCommand) -> Result<String, Status> {
        let res = self.output(&self.argv(cmd)?).await?;
        if res.exit_code != 0 {
            return Err(Status::internal(format!(
                "booting failed ({}): {}",
                res.exit_code,
                first_error_line(&res.stderr)
            )));
        }
        let id = res
            .stdout
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return Err(Status::internal(
                "the machine booted but reported no id".to_string(),
            ));
        }
        Ok(id)
    }

    /// Run a command and stream its output as it appears, ending with one
    /// `exit_code` frame.
    pub fn stream(&self, args: &[String]) -> mpsc::Receiver<Result<OutputChunk, Status>> {
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let mut cmd = self.command(args);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        tokio::spawn(async move {
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("spawning: {e}"))))
                        .await;
                    return;
                }
            };
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let out_tx = tx.clone();
            let err_tx = tx.clone();
            let out = tokio::spawn(async move { pump(stdout, out_tx, true).await });
            let err = tokio::spawn(async move { pump(stderr, err_tx, false).await });

            let status = child.wait().await;
            // Give the pumps a moment for whatever was written just before exit,
            // then stop: a detached grandchild holds these pipes open forever.
            let _ = tokio::time::timeout(DRAIN_GRACE, async {
                let _ = out.await;
                let _ = err.await;
            })
            .await;

            let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
            let _ = tx
                .send(Ok(OutputChunk {
                    payload: Some(output_chunk::Payload::ExitCode(code)),
                }))
                .await;
        });
        rx
    }

    /// Run a command with stdin attached, for a non-tty interactive session.
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
            .map_err(|e| Status::internal(format!("spawning: {e}")))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| Status::internal("the child has no stdin"))?;
        let (in_tx, mut in_rx) = mpsc::unbounded_channel::<SessionInput>();
        tokio::spawn(async move {
            while let Some(msg) = in_rx.recv().await {
                match msg {
                    SessionInput::Data(b) => {
                        if stdin.write_all(&b).await.is_err() || stdin.flush().await.is_err() {
                            break;
                        }
                    }
                    SessionInput::Eof => break,
                }
            }
            // Dropping stdin closes the pipe, which is the EOF the child sees.
        });

        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out_tx = tx.clone();
        let err_tx = tx.clone();
        tokio::spawn(async move {
            let out = tokio::spawn(async move { pump(stdout, out_tx, true).await });
            let err = tokio::spawn(async move { pump(stderr, err_tx, false).await });
            let status = child.wait().await;
            let _ = tokio::time::timeout(DRAIN_GRACE, async {
                let _ = out.await;
                let _ = err.await;
            })
            .await;
            let code = status.ok().and_then(|s| s.code()).unwrap_or(-1);
            let _ = tx
                .send(Ok(OutputChunk {
                    payload: Some(output_chunk::Payload::ExitCode(code)),
                }))
                .await;
        });

        Ok((PipedSession { input: in_tx }, rx))
    }
}

/// A handle to a running piped session: somewhere to send stdin.
pub struct PipedSession {
    input: mpsc::UnboundedSender<SessionInput>,
}

impl PipedSession {
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.input.send(SessionInput::Data(data));
    }

    /// Close the child's stdin.
    pub fn eof(&self) {
        let _ = self.input.send(SessionInput::Eof);
    }
}

async fn pump<R>(reader: Option<R>, tx: mpsc::Sender<Result<OutputChunk, Status>>, is_stdout: bool)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let Some(reader) = reader else { return };
    let mut reader = BufReader::new(reader);
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let payload = if is_stdout {
                    output_chunk::Payload::Stdout(buf[..n].to_vec())
                } else {
                    output_chunk::Payload::Stderr(buf[..n].to_vec())
                };
                if tx
                    .send(Ok(OutputChunk {
                        payload: Some(payload),
                    }))
                    .await
                    .is_err()
                {
                    break; // the client stopped reading
                }
            }
        }
    }
}

/// The most useful line of a failed boot's stderr.
///
/// The supervisor writes `tracing` output there, so the tail is usually the
/// error and the head is startup noise.
fn first_error_line(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no output")
        .to_string()
}

/// The daemon's own PATH, with the usual tool directories appended.
fn augmented_path() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<&str> = current.split(':').filter(|p| !p.is_empty()).collect();
    for extra in EXTRA_PATHS {
        if !parts.contains(extra) {
            parts.push(extra);
        }
    }
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_argv_is_the_run_subcommand_plus_one_json_argument() {
        let sup = Supervisor {
            exe: PathBuf::from("/usr/bin/bsdkrun-supervisor"),
            path: String::new(),
        };
        let cmd = CoreCommand::Ps(bsdkrun_core::cli::PsArgs {
            all: true,
            json: true,
        });
        let argv = sup.argv(&cmd).unwrap();
        assert_eq!(argv.len(), 2);
        assert_eq!(argv[0], RUN_SUBCOMMAND);
        // Round-trips, so the supervisor gets exactly the command we built.
        let back: CoreCommand = serde_json::from_str(&argv[1]).unwrap();
        assert!(matches!(back, CoreCommand::Ps(a) if a.all && a.json));
    }

    #[test]
    fn the_path_keeps_what_it_was_given_and_appends_the_usual_places() {
        let path = augmented_path();
        for extra in EXTRA_PATHS {
            assert!(path.split(':').any(|p| p == *extra), "missing {extra}");
        }
    }
}
