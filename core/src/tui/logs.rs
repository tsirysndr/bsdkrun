//! The log-viewer modal: instant backfill from `console.log`, then a live
//! follow of `console.sock` for a running machine, streamed by a detached
//! reader thread. The thread self-terminates when the socket closes or the
//! modal is dropped (its receiver hangs up and the next send fails).

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;

use crate::api;

/// How much of console.log to backfill instantly (the follow stream continues
/// from live output).
const BACKFILL_BYTES: u64 = 64 * 1024;
/// Ring-buffer cap: enough scrollback to debug a boot, bounded so a chatty
/// guest can't grow the TUI without limit.
const MAX_LINES: usize = 5000;

pub enum LogEvent {
    Chunk(String),
    Closed,
}

pub struct LogModal {
    pub title: String,
    pub lines: VecDeque<String>,
    /// First visible line; only meaningful when `follow` is off.
    pub scroll: usize,
    /// Stick to the tail as new output arrives.
    pub follow: bool,
    pub live: bool,
    rx: Option<mpsc::Receiver<LogEvent>>,
    /// Trailing bytes of the stream that haven't seen a newline yet.
    partial: String,
}

impl LogModal {
    pub fn open(m: &api::Machine) -> Self {
        let dir = m.state_dir.clone().map(PathBuf::from);
        let mut lines = VecDeque::new();
        if let Some(dir) = &dir {
            // console.log exists for detached machines; bsdkrun.log (boot
            // diagnostics) is the fallback — same order as `bsdkrun logs`.
            let backfill = read_tail(&dir.join("console.log"))
                .filter(|s| !s.trim().is_empty())
                .or_else(|| read_tail(&dir.join("bsdkrun.log")));
            if let Some(text) = backfill {
                for line in text.lines() {
                    push_line(&mut lines, line);
                }
            }
        }
        // Live follow only when there is a console to follow.
        let rx = match (&dir, m.running) {
            (Some(dir), true) => {
                let (tx, rx) = mpsc::channel();
                let dir = dir.clone();
                std::thread::spawn(move || follow_into(&dir, tx));
                Some(rx)
            }
            _ => None,
        };
        LogModal {
            title: format!("logs — {}", super::display_name(m)),
            lines,
            scroll: 0,
            follow: true,
            live: rx.is_some(),
            rx,
            partial: String::new(),
        }
    }

    /// Pull pending stream chunks into the ring buffer. True when new content
    /// arrived (needs a redraw).
    pub fn drain(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(LogEvent::Chunk(text)) => {
                    changed = true;
                    self.partial.push_str(&text);
                    // Split off complete lines; the remainder stays partial.
                    while let Some(nl) = self.partial.find('\n') {
                        let line: String = self.partial.drain(..=nl).collect();
                        push_line(&mut self.lines, line.trim_end_matches(['\n', '\r']));
                    }
                }
                Ok(LogEvent::Closed) => {
                    changed = true;
                    self.live = false;
                    self.rx = None;
                    return changed;
                }
                Err(mpsc::TryRecvError::Empty) => return changed,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.live = false;
                    self.rx = None;
                    return changed;
                }
            }
        }
    }
}

/// Append one cleaned line, evicting from the front past the cap.
fn push_line(lines: &mut VecDeque<String>, raw: &str) {
    if lines.len() == MAX_LINES {
        lines.pop_front();
    }
    lines.push_back(clean_line(raw));
}

/// Strip what a guest console emits that ratatui must not see: ANSI escape
/// sequences (colors, cursor movement) and carriage-return overwrites, where
/// the last CR-separated segment is the one the terminal would have kept.
fn clean_line(raw: &str) -> String {
    let last = raw.rsplit('\r').next().unwrap_or(raw);
    let mut out = String::with_capacity(last.len());
    let mut chars = last.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ params final-byte (@..~). Other escapes: skip one.
            if chars.peek() == Some(&'[') {
                chars.next();
                for f in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&f) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c == '\t' {
            out.push_str("    ");
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// The last `BACKFILL_BYTES` of a file, as text (lossy).
fn read_tail(path: &std::path::Path) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(BACKFILL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    // A mid-line start point yields a torn first line; drop it.
    if start > 0 {
        if let Some(nl) = text.find('\n') {
            text.drain(..=nl);
        }
    }
    Some(text)
}

/// The follower thread: console.sock → LogEvent chunks, until either side
/// closes. A send failing means the modal is gone — exit quietly.
fn follow_into(dir: &std::path::Path, tx: mpsc::Sender<LogEvent>) {
    let Ok(mut stream) = crate::console::open_stream(dir) else {
        let _ = tx.send(LogEvent::Closed);
        return;
    };
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => {
                let _ = tx.send(LogEvent::Closed);
                return;
            }
            Ok(k) => {
                let text = String::from_utf8_lossy(&buf[..k]).into_owned();
                if tx.send(LogEvent::Chunk(text)).is_err() {
                    return;
                }
            }
        }
    }
}

/// Keys inside the log modal.
pub fn handle_key(app: &mut super::App, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    let Some(log) = &mut app.log else { return };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('l') => app.log = None,
        KeyCode::Up | KeyCode::Char('k') => {
            log.follow = false;
            log.scroll = log.scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            log.scroll = log.scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            log.follow = false;
            log.scroll = log.scroll.saturating_sub(20);
        }
        KeyCode::PageDown => {
            log.scroll = log.scroll.saturating_add(20);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            log.follow = false;
            log.scroll = 0;
        }
        KeyCode::Char('G') | KeyCode::End => log.follow = true,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_and_cr_are_stripped() {
        assert_eq!(clean_line("\x1b[32mok\x1b[0m done"), "ok done");
        assert_eq!(clean_line("progress 10%\rprogress 99%"), "progress 99%");
        assert_eq!(clean_line("a\tb"), "a    b");
    }
}
