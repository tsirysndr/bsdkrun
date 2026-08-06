//! Interactive sessions: running the CLI under a real pty on the daemon host
//! and bridging that pty to a bidirectional gRPC stream.
//!
//! A remote shell cannot be built out of plain pipes. `bsdkrun shell` and the
//! guest shell behind it both check `isatty`, and without a terminal there is
//! no prompt, no line editing, no job control and no window size to honour. So
//! the daemon allocates a pty on its own host, spawns the CLI as its child —
//! which therefore sees a genuine terminal — and moves bytes between the pty
//! master and the gRPC stream:
//!
//! ```text
//! client stdin    ─► ExecInput::Stdin   ─► pty master write
//! client SIGWINCH ─► ExecInput::Resize  ─► pty master resize (TIOCSWINSZ)
//! pty master read ─► OutputChunk::Stdout ─► client stdout
//! ```
//!
//! With a pty there is a single interleaved stream, so everything the child
//! writes arrives as `Stdout`; `Stderr` frames are only used by the non-pty
//! paths in [`crate::cli`].
//!
//! portable-pty's API is blocking, so each direction gets its own OS thread
//! rather than a tokio task.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;
use tonic::Status;

use crate::cli::SessionInput;
use crate::pb::{output_chunk, OutputChunk};

const READ_CHUNK: usize = 64 * 1024;
const CHANNEL_DEPTH: usize = 64;

/// Fallback size for a client that never sends one. Matches the conventional
/// terminal default, so a guest that queries `stty size` gets something sane.
pub const DEFAULT_ROWS: u16 = 24;
pub const DEFAULT_COLS: u16 = 80;

/// A handle to a live interactive session: somewhere to send input and a way to
/// resize the terminal.
///
/// The handle deliberately does *not* own the child's lifetime. A client that
/// has nothing more to send half-closes its half of the stream, which drops
/// this handle while the session is still running and producing output — so
/// killing the child here would cut every non-interactive call short. Cleanup
/// is driven from the other end instead: [`SessionStream`] kills the CLI when
/// it is dropped, i.e. when the client stops reading the response stream.
pub struct PtySession {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    input: mpsc::UnboundedSender<SessionInput>,
}

impl PtySession {
    /// Spawn `bin args...` under a fresh pty. Returns the session handle and the
    /// stream of output frames, which ends with one `exit_code` frame.
    pub fn spawn(
        bin: &std::path::Path,
        args: &[String],
        env_path: &str,
        rows: u16,
        cols: u16,
    ) -> Result<(Self, SessionStream), Status> {
        let size = PtySize {
            rows: if rows == 0 { DEFAULT_ROWS } else { rows },
            cols: if cols == 0 { DEFAULT_COLS } else { cols },
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| Status::internal(format!("allocating pty: {e}")))?;

        let mut cmd = CommandBuilder::new(bin);
        for a in args {
            cmd.arg(a);
        }
        cmd.env("PATH", env_path);
        // Set unconditionally rather than only when unset. This pty backs a
        // *client's* terminal, so whatever TERM the daemon process happens to
        // have inherited is irrelevant — and frequently wrong: a service
        // manager commonly hands a daemon `TERM=dumb`, which would then be
        // passed straight down and leave the session with no line editing,
        // colour or key sequences.
        cmd.env("TERM", "xterm-256color");
        // Always set this — see session_cwd() for why leaving it unset is not
        // the same as letting portable-pty pick.
        cmd.cwd(session_cwd());

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| Status::internal(format!("spawning bsdkrun under a pty: {e}")))?;
        // An independent killer, so the watchdog never has to contend for the
        // child itself — the reader thread blocks inside `wait()` for the whole
        // life of the session, and a shared lock would make the kill wait for
        // the very process it is trying to stop.
        let killer = child.clone_killer();
        // Drop the slave so the master reader sees EOF once the child exits;
        // holding it open would keep the session alive forever.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| Status::internal(format!("cloning pty reader: {e}")))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|e| Status::internal(format!("taking pty writer: {e}")))?;

        let master = Arc::new(Mutex::new(pair.master));
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let (input_tx, mut input_rx) = mpsc::unbounded_channel::<SessionInput>();

        // stdin: client -> pty master.
        std::thread::spawn(move || {
            while let Some(msg) = input_rx.blocking_recv() {
                let bytes = match msg {
                    SessionInput::Data(b) => b,
                    // A pty has no out-of-band EOF: closing the master would
                    // tear the session down instead of ending one command's
                    // stdin. Send EOT and let the line discipline turn it into
                    // end-of-input, exactly as Ctrl-D does in a local terminal.
                    SessionInput::Eof => vec![0x04],
                };
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    return;
                }
            }
        });

        // stdout: pty master -> client, then reap the child for its exit code.
        // The child is owned here outright: this is the only place that waits.
        std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = OutputChunk {
                            payload: Some(output_chunk::Payload::Stdout(buf[..n].to_vec())),
                        };
                        if tx.blocking_send(Ok(chunk)).is_err() {
                            return; // client hung up; Drop kills the child
                        }
                    }
                }
            }
            let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
            let _ = tx.blocking_send(Ok(OutputChunk {
                payload: Some(output_chunk::Payload::ExitCode(code)),
            }));
        });

        Ok((
            Self {
                master,
                input: input_tx,
            },
            SessionStream {
                inner: tokio_stream::wrappers::ReceiverStream::new(rx),
                killer,
            },
        ))
    }

    /// Feed bytes to the remote program's stdin. A closed channel means the
    /// session is already gone, which is not worth surfacing as an error.
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.input.send(SessionInput::Data(data));
    }

    pub fn eof(&self) {
        let _ = self.input.send(SessionInput::Eof);
    }

    /// Apply a client SIGWINCH. The kernel signals the foreground process group
    /// in the guest, so full-screen programs redraw at the new size.
    pub fn resize(&self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 {
            return;
        }
        if let Ok(master) = self.master.lock() {
            let _ = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }
}

/// The output half of a pty session, which also owns cleanup.
///
/// Tying the kill to this stream's `Drop` is what makes an abandoned remote
/// shell go away: tonic drops the response stream when the client disconnects
/// or cancels, and an idle shell produces no output for a failed send to
/// notice. It must be *this* type and not a background task holding a channel
/// sender — a live sender keeps the channel open, so the stream could never
/// end and every session would hang after its final frame.
pub struct SessionStream {
    inner: tokio_stream::wrappers::ReceiverStream<Result<OutputChunk, Status>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

impl tokio_stream::Stream for SessionStream {
    type Item = Result<OutputChunk, Status>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Drop for SessionStream {
    fn drop(&mut self) {
        // A no-op when the session already ended on its own.
        let _ = self.killer.kill();
    }
}

/// A working directory for the session that is guaranteed to exist.
///
/// Starting in `HOME` matches what an interactive login would do, so relative
/// host paths resolve somewhere predictable rather than wherever the daemon
/// happened to be started.
///
/// This must always return *something*, and it is not enough to simply leave
/// the cwd unset: portable-pty falls back to `$HOME` on its own, and that
/// fallback is unchecked —
///
/// ```text
/// let dir = self.cwd.as_ref().filter(|d| Path::new(d).is_dir())
///               .unwrap_or(home.as_ref());   // no is_dir check
/// ```
///
/// so a `HOME` that does not exist becomes the child's cwd and the spawn fails
/// with ENOENT. Daemons hit this easily: Nix's build sandbox sets
/// `/homeless-shelter`, and service managers often set something equally
/// notional. It surfaced as every interactive session failing with
/// "spawning bsdkrun under a pty: No such file or directory".
fn session_cwd() -> std::path::PathBuf {
    session_cwd_from(std::env::var_os("HOME"), std::env::current_dir().ok())
}

fn session_cwd_from(
    home: Option<std::ffi::OsString>,
    cwd: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    home.map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
        .or_else(|| cwd.filter(|p| p.is_dir()))
        // Root is the last resort because it is the one directory that is
        // always there. A session started there still works; a session that
        // cannot start at all does not.
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_a_real_home() {
        let home = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        assert_eq!(
            session_cwd_from(
                Some(home.path().as_os_str().to_owned()),
                Some(cwd.path().to_path_buf())
            ),
            home.path()
        );
    }

    #[test]
    fn falls_back_to_the_cwd_when_home_does_not_exist() {
        let cwd = tempfile::tempdir().unwrap();
        assert_eq!(
            session_cwd_from(
                Some("/nonexistent-homeless-shelter".into()),
                Some(cwd.path().to_path_buf())
            ),
            cwd.path()
        );
    }

    /// The case that broke the Nix build: a bogus HOME and no usable cwd.
    #[test]
    fn falls_back_to_root_when_nothing_else_exists() {
        assert_eq!(
            session_cwd_from(
                Some("/nonexistent-homeless-shelter".into()),
                Some(std::path::PathBuf::from("/also-not-there"))
            ),
            std::path::PathBuf::from("/")
        );
        assert_eq!(session_cwd_from(None, None), std::path::PathBuf::from("/"));
    }

    /// A regular file is not somewhere a process can start.
    #[test]
    fn rejects_a_home_that_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(
            session_cwd_from(Some(file.into_os_string()), None),
            std::path::PathBuf::from("/")
        );
    }
}
