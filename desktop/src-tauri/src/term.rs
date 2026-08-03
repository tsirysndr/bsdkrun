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

/// Open a PTY running `bsdkrun exec -t <id> <cmd...>` (defaults to `/bin/sh`).
pub fn open(
    app: &AppHandle,
    sessions: &Terminals,
    bin: &PathBuf,
    machine_id: &str,
    command: Vec<String>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(bin);
    cmd.arg("exec");
    cmd.arg("-t");
    cmd.arg(machine_id);
    if command.is_empty() {
        cmd.arg("/bin/sh");
    } else {
        for a in &command {
            cmd.arg(a);
        }
    }
    // Inherit the environment plus an augmented PATH so bsdkrun finds its tools.
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    cmd.env("PATH", bsdkrun::augmented_path());
    cmd.env("TERM", "xterm-256color");

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
