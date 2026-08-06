//! Interactive PTY sessions (`bsdkrun exec -t …` / `shell`) and live log
//! streaming (`bsdkrun logs -f …`), fanned out to the webview via Tauri events.
//!
//! Terminal events:  `term://data` { session, bytes }  ·  `term://exit` { session, code }
//! Log events:       `log://line` { id, line }          ·  `log://end`  { id }

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::bsdkrun;
use crate::remote;
use crate::target::Target;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_session_id(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// A live terminal, local or remote.
///
/// The remote variant holds only a channel: the pty itself lives on the daemon's
/// host, which is the whole point — the guest shell gets a real terminal there,
/// and this side just moves bytes.
pub enum TermSession {
    Pty {
        master: Mutex<Box<dyn MasterPty + Send>>,
        writer: Mutex<Box<dyn Write + Send>>,
        child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    },
    Remote {
        input: tokio::sync::mpsc::UnboundedSender<remote::Input>,
    },
}

#[derive(Default)]
pub struct Terminals(pub Mutex<HashMap<String, Arc<TermSession>>>);

#[derive(Clone, Serialize)]
struct TermData {
    session: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Serialize)]
struct TermExit {
    session: String,
    code: Option<i32>,
}

/// Inherit the environment plus an augmented PATH + a sane TERM.
fn env_setup(cmd: &mut CommandBuilder) {
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    cmd.env("PATH", bsdkrun::augmented_path());
    cmd.env("TERM", "xterm-256color");
}

/// Open an interactive PTY into the guest via `bsdkrun exec -t <id> <cmd...>`.
/// `command` should already be a shell that exists in the guest (resolved by the
/// caller, e.g. `bsdkrun::resolve_guest_shell`) so nix images without `/bin/sh`
/// don't 127.
pub fn open(
    app: &AppHandle,
    sessions: &Terminals,
    bin: &Target,
    machine_id: &str,
    command: Vec<String>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    // Remote: the daemon allocates the pty on its own host and bridges it, so
    // there is no local process here at all.
    let bin = match bin {
        Target::Local(p) => p,
        Target::Remote { endpoint, token } => {
            return Ok(open_remote(
                app, sessions, endpoint, token, machine_id, command, rows, cols,
            ))
        }
    };
    let mut cmd = CommandBuilder::new(bin);
    cmd.arg("exec");
    cmd.arg("-t");
    cmd.arg(machine_id);
    if command.is_empty() {
        cmd.arg("/bin/sh");
    } else if command.len() == 1 {
        // A bare resolved shell (e.g. `bash`): wrap it so the session starts in a
        // cloned repo when `--repo` recorded one in /etc/bsdkrun-cwd, then hands
        // off to an interactive shell. `cd` to an empty/missing marker is a no-op.
        // Use the resolved shell itself as the wrapper (nix images lack /bin/sh).
        let sh = &command[0];
        cmd.arg(sh);
        cmd.arg("-c");
        // Add the BSD package prefixes to PATH (FreeBSD `pkg` → /usr/local/bin,
        // NetBSD pkgsrc → /usr/pkg/bin); on NetBSD, point `pkg_add` at the pkgsrc
        // CDN for this release so `pkg_add pkgin` works (an unset PKG_PATH is why
        // it fails; a `-current` release uses the matching `<major>.0` branch);
        // then `cd` into a cloned repo and hand off to the shell.
        cmd.arg(format!(
            "export PATH=\"/usr/local/bin:/usr/local/sbin:/usr/pkg/bin:/usr/pkg/sbin:$PATH\"; \
             [ \"$(uname 2>/dev/null)\" = Linux ] || export TERM=xterm; \
             if [ -z \"$PKG_PATH\" ] && [ \"$(uname 2>/dev/null)\" = NetBSD ]; then \
               __a=$(uname -p 2>/dev/null); [ \"$__a\" = x86_64 ] && __a=amd64; \
               export PKG_PATH=\"https://cdn.NetBSD.org/pub/pkgsrc/packages/NetBSD/$__a/$(uname -r 2>/dev/null | cut -d. -f1).0/All/\"; \
             fi; \
             cd \"$(cat /etc/bsdkrun-cwd 2>/dev/null)\" 2>/dev/null; \
             if command -v bash >/dev/null 2>&1; then exec bash; else exec {sh}; fi"
        ));
    } else {
        for a in &command {
            cmd.arg(a);
        }
    }
    env_setup(&mut cmd);
    spawn_pty(app, sessions, cmd, rows, cols)
}

/// Open a PTY running the *host's* login shell (`$SHELL`, else `/bin/zsh`).
pub fn open_host(
    app: &AppHandle,
    sessions: &Terminals,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut cmd = CommandBuilder::new(&shell);
    cmd.arg("-l");
    env_setup(&mut cmd);
    if let Ok(home) = std::env::var("HOME") {
        cmd.cwd(home);
    }
    spawn_pty(app, sessions, cmd, rows, cols)
}

/// Spawn a prepared command under a fresh PTY, register the session, and stream
/// its output to the webview. Shared by guest (`exec`) and host shells.
fn spawn_pty(
    app: &AppHandle,
    sessions: &Terminals,
    cmd: CommandBuilder,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let session_id = next_session_id("term");
    let session = Arc::new(TermSession::Pty {
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        child: Mutex::new(child),
    });
    sessions
        .0
        .lock()
        .unwrap()
        .insert(session_id.clone(), session.clone());

    // Reader task: fan PTY output out to the webview. The read loop is blocking,
    // so it lives on tokio's blocking pool for the session's lifetime.
    let app2 = app.clone();
    let sid = session_id.clone();
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let _ = app2.emit(
                        "term://data",
                        TermData {
                            session: sid.clone(),
                            bytes: buf[..n].to_vec(),
                        },
                    );
                }
            }
        }
        // Reap the child so `wait()` yields a code, then notify the UI.
        let code = match &*session {
            TermSession::Pty { child, .. } => child
                .lock()
                .unwrap()
                .wait()
                .ok()
                .map(|s| s.exit_code() as i32),
            // This reader task only ever runs for a local pty.
            TermSession::Remote { .. } => None,
        };
        let _ = app2.emit(
            "term://exit",
            TermExit {
                session: sid.clone(),
                code,
            },
        );
    });

    Ok(session_id)
}

fn session(sessions: &Terminals, id: &str) -> Result<Arc<TermSession>, String> {
    sessions
        .0
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or_else(|| "no such session".to_string())
}

pub fn write(sessions: &Terminals, session_id: &str, data: &str) -> Result<(), String> {
    match &*session(sessions, session_id)? {
        TermSession::Pty { writer, .. } => {
            let mut w = writer.lock().unwrap();
            w.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
            w.flush().map_err(|e| e.to_string())
        }
        TermSession::Remote { input } => input
            .send(remote::Input::Data(data.as_bytes().to_vec()))
            .map_err(|_| "the remote session has ended".to_string()),
    }
}

pub fn resize(sessions: &Terminals, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
    match &*session(sessions, session_id)? {
        TermSession::Pty { master, .. } => master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string()),
        TermSession::Remote { input } => input
            .send(remote::Input::Resize { rows, cols })
            .map_err(|_| "the remote session has ended".to_string()),
    }
}

pub fn close(sessions: &Terminals, session: &str) -> Result<(), String> {
    if let Some(s) = sessions.0.lock().unwrap().remove(session) {
        match &*s {
            TermSession::Pty { child, .. } => {
                let _ = child.lock().unwrap().kill();
            }
            // Dropping the request stream ends the RPC, and the daemon kills
            // the pty with it.
            TermSession::Remote { input } => {
                let _ = input.send(remote::Input::Close);
            }
        }
    }
    Ok(())
}

// ---- live log streaming ---------------------------------------------------

/// A running log follow, local or remote.
pub enum LogStream {
    Pty(Mutex<Box<dyn portable_pty::Child + Send + Sync>>),
    Remote(tokio::task::AbortHandle),
}

#[derive(Default)]
pub struct LogStreams(pub Mutex<HashMap<String, Arc<LogStream>>>);

#[derive(Clone, Serialize)]
struct LogLine {
    id: String,
    line: String,
}

#[derive(Clone, Serialize)]
struct LogEnd {
    id: String,
}

/// Stream `bsdkrun logs -f <id>` line-by-line to `log://line` events. The stream
/// is keyed by machine id so a later `stop_log_stream` can tear it down. We run
/// it under a PTY so bsdkrun follows in the same way `logs -f` does interactively.
pub fn start_logs(
    app: &AppHandle,
    streams: &LogStreams,
    bin: &Target,
    machine_id: &str,
) -> Result<(), String> {
    let bin = match bin {
        Target::Local(p) => p,
        Target::Remote { endpoint, token } => {
            start_logs_remote(app, streams, endpoint, token, machine_id);
            return Ok(());
        }
    };
    // Replace any existing stream for this id.
    stop_logs(streams, machine_id);

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new(bin);
    cmd.arg("logs");
    cmd.arg("-f");
    cmd.arg(machine_id);
    cmd.env("PATH", bsdkrun::augmented_path());

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    streams.0.lock().unwrap().insert(
        machine_id.to_string(),
        Arc::new(LogStream::Pty(Mutex::new(child))),
    );

    let app2 = app.clone();
    let id = machine_id.to_string();
    tokio::task::spawn_blocking(move || {
        // Keep the master alive for the task's lifetime.
        let _master = pair.master;
        let mut acc: Vec<u8> = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    acc.extend_from_slice(&buf[..n]);
                    // Emit at most ONE event per read, covering all COMPLETE lines
                    // (up to the last newline). A verbose boot can produce thousands
                    // of lines; per-line events would flood + freeze the webview.
                    if let Some(pos) = acc.iter().rposition(|&b| b == b'\n') {
                        let chunk: Vec<u8> = acc.drain(..=pos).collect();
                        let line = String::from_utf8_lossy(&chunk).into_owned();
                        let _ = app2.emit(
                            "log://line",
                            LogLine {
                                id: id.clone(),
                                line,
                            },
                        );
                    }
                }
            }
        }
        if !acc.is_empty() {
            let line = String::from_utf8_lossy(&acc).into_owned();
            let _ = app2.emit(
                "log://line",
                LogLine {
                    id: id.clone(),
                    line,
                },
            );
        }
        let _ = app2.emit("log://end", LogEnd { id: id.clone() });
    });
    Ok(())
}

pub fn stop_logs(streams: &LogStreams, machine_id: &str) {
    if let Some(stream) = streams.0.lock().unwrap().remove(machine_id) {
        match &*stream {
            LogStream::Pty(child) => {
                let _ = child.lock().unwrap().kill();
            }
            LogStream::Remote(handle) => handle.abort(),
        }
    }
}

// ---- remote sessions -------------------------------------------------------

/// A terminal backed by a daemon rather than a local pty.
///
/// It emits the same `term://data` / `term://exit` events, so the webview
/// cannot tell the two apart — which is the point: the UI is identical whether
/// the VMs are on this machine or a server.
#[allow(clippy::too_many_arguments)]
fn open_remote(
    app: &AppHandle,
    sessions: &Terminals,
    endpoint: &str,
    token: &str,
    machine_id: &str,
    command: Vec<String>,
    rows: u16,
    cols: u16,
) -> String {
    let session_id = next_session_id("term");

    let app_data = app.clone();
    let id_data = session_id.clone();
    let app_exit = app.clone();
    let id_exit = session_id.clone();

    let input = remote::exec_session(
        endpoint.to_string(),
        token.to_string(),
        machine_id.to_string(),
        // An empty command means "this machine's shell"; the daemon resolves a
        // shell that exists in the guest, so we do not probe for one from here.
        command,
        rows,
        cols,
        move |bytes| {
            let _ = app_data.emit(
                "term://data",
                TermData {
                    session: id_data.clone(),
                    bytes,
                },
            );
        },
        move |code| {
            let _ = app_exit.emit(
                "term://exit",
                TermExit {
                    session: id_exit,
                    code,
                },
            );
        },
    );

    sessions
        .0
        .lock()
        .unwrap()
        .insert(session_id.clone(), Arc::new(TermSession::Remote { input }));
    session_id
}

/// Follow a remote machine's console, emitting the same log events.
fn start_logs_remote(
    app: &AppHandle,
    streams: &LogStreams,
    endpoint: &str,
    token: &str,
    machine_id: &str,
) {
    let app_line = app.clone();
    let id_line = machine_id.to_string();
    let app_end = app.clone();
    let id_end = machine_id.to_string();

    let handle = remote::logs_session(
        endpoint.to_string(),
        token.to_string(),
        machine_id.to_string(),
        move |line| {
            let _ = app_line.emit(
                "log://line",
                LogLine {
                    id: id_line.clone(),
                    line: crate::sanitize_log(&line),
                },
            );
        },
        move || {
            let _ = app_end.emit("log://end", LogEnd { id: id_end });
        },
    );

    streams
        .0
        .lock()
        .unwrap()
        .insert(machine_id.to_string(), Arc::new(LogStream::Remote(handle)));
}
