//! Shell sessions for the GraphQL front end.
//!
//! gRPC can carry a shell in a single bidirectional stream. GraphQL cannot:
//! a subscription only ever flows server→client, so there is nowhere to put
//! keystrokes. The shape that does work splits the session across operations —
//! a mutation opens it, a subscription carries output, and further mutations
//! carry input — which means the session has to outlive any one operation and
//! live somewhere both can reach. That is this registry.
//!
//! ```text
//! mutation openShell      -> sessionId          (pty spawned, output buffered)
//! subscription shellOutput(sessionId)           (replay, then live)
//! mutation sendShellInput(sessionId, data)
//! mutation resizeShell(sessionId, rows, cols)
//! mutation closeShell(sessionId)
//! ```
//!
//! The buffer is not an optimisation, it is a correctness requirement. The
//! subscription is necessarily a *separate* operation from the mutation that
//! opened the session, so a shell that wrote its prompt in between would
//! otherwise have lost it before anyone was listening.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio_stream::StreamExt;

use crate::ops::{ExecOpts, OpError, OpResult, Ops};
use crate::pb::output_chunk;
use crate::pty::{PtySession, DEFAULT_COLS, DEFAULT_ROWS};

/// Output held for a session with no live subscriber. Beyond this the oldest
/// bytes are dropped: a terminal's most recent screen is what matters, and an
/// unbounded buffer would let an abandoned session consume the host's memory.
const MAX_BUFFER_BYTES: usize = 1024 * 1024;

/// How long a finished session stays readable after its command exits, so a
/// subscriber that attaches late still receives the tail and the exit code.
const REAP_AFTER: Duration = Duration::from_secs(60);

/// Ceiling on concurrent sessions. Each holds a pty and a CLI process, so this
/// is a real resource bound rather than a formality.
const MAX_SESSIONS: usize = 64;

/// One frame of a shell session, as the subscription yields it.
#[derive(Debug, Clone)]
pub enum ShellEvent {
    Output(Vec<u8>),
    Exit(i32),
}

#[derive(Default)]
struct Buffered {
    events: VecDeque<ShellEvent>,
    bytes: usize,
    /// Set once the command exits; the stream ends after draining.
    ended: bool,
    /// Output was dropped to stay under the cap — worth telling the client,
    /// since its terminal will have a hole in it.
    truncated: bool,
}

impl Buffered {
    fn push(&mut self, event: ShellEvent) {
        if let ShellEvent::Output(ref b) = event {
            self.bytes += b.len();
        }
        self.events.push_back(event);
        while self.bytes > MAX_BUFFER_BYTES {
            match self.events.pop_front() {
                Some(ShellEvent::Output(b)) => {
                    self.bytes -= b.len();
                    self.truncated = true;
                }
                // Never drop the exit code: it is the stream's terminator.
                Some(other) => {
                    self.events.push_front(other);
                    break;
                }
                None => break,
            }
        }
    }
}

pub struct ShellSession {
    pub id: String,
    pub machine_id: String,
    handle: PtySession,
    buffered: Mutex<Buffered>,
    /// Woken on new output, on exit, and on close.
    notify: Notify,
    /// Set by `close` so the pump task stops and drops the output stream,
    /// which is what kills the pty (see [`crate::pty::SessionStream`]).
    closed: Notify,
}

impl ShellSession {
    pub fn write(&self, data: Vec<u8>) {
        self.handle.write(data);
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        self.handle.resize(rows, cols);
    }

    /// True once the underlying command has exited.
    pub fn is_finished(&self) -> bool {
        self.buffered.lock().map(|b| b.ended).unwrap_or(true)
    }
}

#[derive(Default)]
pub struct ShellRegistry {
    sessions: Mutex<HashMap<String, Arc<ShellSession>>>,
}

impl ShellRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a shell (or an arbitrary command) on a machine and start buffering
    /// its output immediately.
    pub fn open(
        self: &Arc<Self>,
        ops: &Ops,
        machine_id: &str,
        command: Vec<String>,
        env: Vec<String>,
        rows: u16,
        cols: u16,
    ) -> OpResult<Arc<ShellSession>> {
        if machine_id.trim().is_empty() {
            return Err(OpError::InvalidArgument(
                "machineId must not be empty".into(),
            ));
        }
        {
            let sessions = self.lock()?;
            if sessions.len() >= MAX_SESSIONS {
                return Err(OpError::Failed(format!(
                    "too many open shell sessions ({MAX_SESSIONS}); close some first"
                )));
            }
        }

        // Always a tty: this exists to back an interactive terminal.
        let (cmd, _) = ExecOpts {
            id: machine_id.to_string(),
            command,
            env,
            tty: true,
        }
        .to_command();
        let sup = ops.supervisor();
        let argv = sup.argv(&cmd).map_err(OpError::from)?;

        let rows = if rows == 0 { DEFAULT_ROWS } else { rows };
        let cols = if cols == 0 { DEFAULT_COLS } else { cols };
        let (handle, mut stream) = PtySession::spawn(sup.exe(), &argv, sup.env_path(), rows, cols)
            .map_err(OpError::from)?;

        let id = crate::auth::random_hex(16)
            .map_err(|e| OpError::Failed(format!("generating a session id: {e}")))?;
        let session = Arc::new(ShellSession {
            id: id.clone(),
            machine_id: machine_id.to_string(),
            handle,
            buffered: Mutex::new(Buffered::default()),
            notify: Notify::new(),
            closed: Notify::new(),
        });

        self.lock()?.insert(id.clone(), session.clone());

        // Pump the pty into the buffer. Owning `stream` here is what keeps the
        // pty alive; returning drops it and kills the CLI process.
        let pumped = session.clone();
        let registry = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    next = stream.next() => {
                        let Some(chunk) = next else { break };
                        let Ok(chunk) = chunk else { break };
                        match chunk.payload {
                            Some(output_chunk::Payload::Stdout(b))
                            | Some(output_chunk::Payload::Stderr(b)) => {
                                pumped.push(ShellEvent::Output(b));
                            }
                            Some(output_chunk::Payload::ExitCode(c)) => {
                                pumped.push(ShellEvent::Exit(c));
                                break;
                            }
                            None => {}
                        }
                    }
                    // closeShell: stop pumping, drop the stream, kill the pty.
                    _ = pumped.closed.notified() => break,
                }
            }
            pumped.finish();

            // Leave the session readable for a while so a late subscriber can
            // still collect the tail and the exit code, then release the slot.
            tokio::time::sleep(REAP_AFTER).await;
            registry.remove(&pumped.id);
        });

        Ok(session)
    }

    pub fn get(&self, id: &str) -> OpResult<Arc<ShellSession>> {
        self.lock()?
            .get(id)
            .cloned()
            .ok_or_else(|| OpError::InvalidArgument(format!("no such shell session: {id}")))
    }

    pub fn list(&self) -> Vec<Arc<ShellSession>> {
        self.lock()
            .map(|s| s.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Close a session, killing its pty. Idempotent: closing an already-closed
    /// or already-finished session is not an error, so a client tearing down a
    /// terminal never has to care which happened first.
    pub fn close(&self, id: &str) -> OpResult<()> {
        if let Some(session) = self.lock()?.remove(id) {
            session.closed.notify_waiters();
            session.notify.notify_waiters();
        }
        Ok(())
    }

    fn remove(&self, id: &str) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.remove(id);
        }
    }

    fn lock(&self) -> OpResult<std::sync::MutexGuard<'_, HashMap<String, Arc<ShellSession>>>> {
        self.sessions
            .lock()
            .map_err(|_| OpError::Failed("shell registry lock poisoned".into()))
    }
}

impl ShellSession {
    fn push(&self, event: ShellEvent) {
        if let Ok(mut b) = self.buffered.lock() {
            b.push(event);
        }
        self.notify.notify_waiters();
    }

    fn finish(&self) {
        if let Ok(mut b) = self.buffered.lock() {
            b.ended = true;
        }
        self.notify.notify_waiters();
    }

    /// Whether output has been dropped to stay under the buffer cap.
    pub fn truncated(&self) -> bool {
        self.buffered.lock().map(|b| b.truncated).unwrap_or(false)
    }

    /// A stream of this session's output: everything buffered so far, then
    /// whatever arrives next, ending after the exit code.
    ///
    /// Only one subscriber is expected — this is a terminal — and events are
    /// taken from the buffer as they are yielded, so a second subscriber would
    /// see a split of the stream rather than a copy of it.
    pub fn subscribe(self: Arc<Self>) -> impl tokio_stream::Stream<Item = ShellEvent> {
        async_stream::stream! {
            loop {
                // Register interest BEFORE inspecting the buffer. The reverse
                // order can miss a wakeup that lands in between and leave the
                // subscriber parked with output already waiting for it.
                let waiter = self.notify.notified();
                tokio::pin!(waiter);
                waiter.as_mut().enable();

                let (drained, ended) = {
                    match self.buffered.lock() {
                        Ok(mut b) => {
                            let events: Vec<_> = b.events.drain(..).collect();
                            b.bytes = 0;
                            (events, b.ended)
                        }
                        Err(_) => (Vec::new(), true),
                    }
                };

                let mut saw_exit = false;
                for event in drained {
                    saw_exit |= matches!(event, ShellEvent::Exit(_));
                    yield event;
                }
                if saw_exit || ended {
                    break;
                }

                waiter.await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(b: &Buffered) -> Vec<ShellEvent> {
        b.events.iter().cloned().collect()
    }

    #[test]
    fn buffer_keeps_the_most_recent_output_when_capped() {
        let mut b = Buffered::default();
        b.push(ShellEvent::Output(vec![b'a'; MAX_BUFFER_BYTES]));
        b.push(ShellEvent::Output(vec![b'b'; 32]));

        assert!(b.truncated, "expected the cap to have dropped something");
        assert!(b.bytes <= MAX_BUFFER_BYTES);
        // The newest bytes survive: a terminal cares about its current screen.
        let last = drain(&b).pop().unwrap();
        assert!(matches!(last, ShellEvent::Output(v) if v == vec![b'b'; 32]));
    }

    /// The exit code terminates the subscription, so dropping it would leave a
    /// client waiting forever for a stream that has already finished.
    #[test]
    fn buffer_never_drops_the_exit_code() {
        let mut b = Buffered::default();
        b.push(ShellEvent::Exit(0));
        b.push(ShellEvent::Output(vec![b'x'; MAX_BUFFER_BYTES * 2]));
        assert!(
            drain(&b).iter().any(|e| matches!(e, ShellEvent::Exit(0))),
            "exit code was dropped"
        );
    }

    #[tokio::test]
    async fn subscribe_replays_output_buffered_before_it_attached() {
        let session = Arc::new(ShellSession {
            id: "s".into(),
            machine_id: "m".into(),
            handle: PtySession::spawn(
                std::path::Path::new("/bin/sh"),
                &["-c".to_string(), "exit 0".to_string()],
                "/usr/bin:/bin",
                24,
                80,
            )
            .unwrap()
            .0,
            buffered: Mutex::new(Buffered::default()),
            notify: Notify::new(),
            closed: Notify::new(),
        });

        // Output produced before anyone subscribed — the race the buffer exists
        // for, since the mutation that opens a shell always precedes the
        // subscription that reads it.
        session.push(ShellEvent::Output(b"prompt$ ".to_vec()));
        session.push(ShellEvent::Exit(0));

        let events: Vec<_> = session.subscribe().collect().await;
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], ShellEvent::Output(b) if b == b"prompt$ "));
        assert!(matches!(events[1], ShellEvent::Exit(0)));
    }

    #[tokio::test]
    async fn subscribe_ends_when_the_session_finishes_with_no_output() {
        let session = Arc::new(ShellSession {
            id: "s".into(),
            machine_id: "m".into(),
            handle: PtySession::spawn(
                std::path::Path::new("/bin/sh"),
                &["-c".to_string(), "exit 0".to_string()],
                "/usr/bin:/bin",
                24,
                80,
            )
            .unwrap()
            .0,
            buffered: Mutex::new(Buffered::default()),
            notify: Notify::new(),
            closed: Notify::new(),
        });
        session.finish();

        let events: Vec<_> = session.subscribe().collect().await;
        assert!(events.is_empty());
    }
}
