//! Interactive PTY sessions (`bsdkrun exec -t …` / `shell`) and live log
//! streaming (`bsdkrun logs -f …`), fanned out to the webview via Tauri events.
//!
//! Terminal events:  `term://data` { session, bytes }  ·  `term://exit` { session, code }
//! Log events:       `log://line` { id, line }          ·  `log://end`  { id }

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::bsdkrun;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_session_id(prefix: &str) -> String {
    format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

pub struct TermSession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
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
    bin: &PathBuf,
    machine_id: &str,
    command: Vec<String>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
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
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let session_id = next_session_id("term");
    let session = Arc::new(TermSession {
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
                        TermData { session: sid.clone(), bytes: buf[..n].to_vec() },
                    );
                }
            }
        }
        // Reap the child so `wait()` yields a code, then notify the UI.
        let code = {
            let mut guard = session.child.lock().unwrap();
            guard.wait().ok().and_then(|s| {
                let c = s.exit_code();
                Some(c as i32)
            })
        };
        let _ = app2.emit("term://exit", TermExit { session: sid.clone(), code });
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
    let s = session(sessions, session_id)?;
    let mut w = s.writer.lock().unwrap();
    w.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())
}

pub fn resize(sessions: &Terminals, session_id: &str, rows: u16, cols: u16) -> Result<(), String> {
    let s = session(sessions, session_id)?;
    let master = s.master.lock().unwrap();
    master
        .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

pub fn close(sessions: &Terminals, session: &str) -> Result<(), String> {
    if let Some(s) = sessions.0.lock().unwrap().remove(session) {
        let _ = s.child.lock().unwrap().kill();
    }
    Ok(())
}

// ---- live log streaming ---------------------------------------------------

#[derive(Default)]
pub struct LogStreams(pub Mutex<HashMap<String, Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>>);

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
    bin: &PathBuf,
    machine_id: &str,
) -> Result<(), String> {
    // Replace any existing stream for this id.
    stop_logs(streams, machine_id);

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;
    let mut cmd = CommandBuilder::new(bin);
    cmd.arg("logs");
    cmd.arg("-f");
    cmd.arg(machine_id);
    cmd.env("PATH", bsdkrun::augmented_path());

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let child = Arc::new(Mutex::new(child));
    streams
        .0
        .lock()
        .unwrap()
        .insert(machine_id.to_string(), child);

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
                        let _ = app2.emit("log://line", LogLine { id: id.clone(), line });
                    }
                }
            }
        }
        if !acc.is_empty() {
            let line = String::from_utf8_lossy(&acc).into_owned();
            let _ = app2.emit("log://line", LogLine { id: id.clone(), line });
        }
        let _ = app2.emit("log://end", LogEnd { id: id.clone() });
    });
    Ok(())
}

pub fn stop_logs(streams: &LogStreams, machine_id: &str) {
    if let Some(child) = streams.0.lock().unwrap().remove(machine_id) {
        let _ = child.lock().unwrap().kill();
    }
}
