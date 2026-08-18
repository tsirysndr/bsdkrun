//! The embedded terminal: an interactive `bsdkrun shell` rendered *inside*
//! the dashboard, instead of suspending the TUI and handing the real terminal
//! away.
//!
//! Three pieces, none of them exotic:
//!
//!  * a pty (portable-pty — the same crate the daemon's shell sessions use)
//!    running `bsdkrun shell <id>` as a child of this process;
//!  * a vt100 parser fed by a reader thread, turning the child's output into
//!    a screen grid;
//!  * a renderer that copies that grid into ratatui's buffer each frame,
//!    colors, attributes, cursor and all.
//!
//! Every key goes to the child except **Ctrl-\\**, the detach chord — chosen
//! because a shell almost never wants SIGQUIT from a dashboard, and every
//! more obvious key (Esc, q, Ctrl-C) belongs to programs people actually run
//! in shells. Detaching kills the session: the pty's whole life is this pane,
//! and a shell nobody can see is a leak, not a feature.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

/// One live terminal session in the TUI.
pub struct TermPane {
    pub title: String,
    parser: Arc<Mutex<vt100::Parser>>,
    /// Set by the reader thread whenever new output arrived; the draw loop
    /// clears it. Cheaper than a channel for "did anything change".
    dirty: Arc<AtomicBool>,
    /// Set once the child exits (reader saw EOF).
    dead: Arc<AtomicBool>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    size: (u16, u16),
}

impl TermPane {
    /// Spawn `bsdkrun shell <id>` under a pty of `rows` x `cols`.
    pub fn open(id: &str, title: String, rows: u16, cols: u16) -> Result<Self, String> {
        let rows = rows.max(4);
        let cols = cols.max(20);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("allocating a pty: {e}"))?;

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = CommandBuilder::new(exe);
        cmd.arg("shell");
        cmd.arg(id);
        // The child renders into our vt100 parser, which speaks xterm.
        cmd.env("TERM", "xterm-256color");

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("spawning the shell: {e}"))?;
        // Drop the slave so the reader sees EOF when the child exits.
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("cloning the pty reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("taking the pty writer: {e}"))?;

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2000)));
        let dirty = Arc::new(AtomicBool::new(true));
        let dead = Arc::new(AtomicBool::new(false));

        {
            let parser = parser.clone();
            let dirty = dirty.clone();
            let dead = dead.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            parser.lock().unwrap().process(&buf[..n]);
                            dirty.store(true, Ordering::Relaxed);
                        }
                    }
                }
                // Reap, so a closed session leaves no zombie behind.
                let _ = child.wait();
                dead.store(true, Ordering::Relaxed);
                dirty.store(true, Ordering::Relaxed);
            });
        }

        Ok(Self {
            title,
            parser,
            dirty,
            dead,
            writer,
            master: pair.master,
            size: (rows, cols),
        })
    }

    /// Whether output arrived since the last draw (clears the flag).
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    /// Grow/shrink both the pty and the parser to the pane. The child learns
    /// via SIGWINCH exactly as it would on a real terminal.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(4);
        let cols = cols.max(20);
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        self.parser.lock().unwrap().set_size(rows, cols);
    }

    /// Forward one key press as the byte sequence a terminal would send.
    /// Returns false for the detach chord (Ctrl-\\), which is ours, not the
    /// child's.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('\\')) {
            return false;
        }
        let bytes: Vec<u8> = match key.code {
            KeyCode::Char(c) if ctrl => {
                // ^A..^Z and friends: the C0 control for the letter.
                let c = c.to_ascii_uppercase();
                if c.is_ascii_uppercase() {
                    vec![(c as u8) - b'A' + 1]
                } else {
                    match c {
                        '[' => vec![0x1b],
                        ']' => vec![0x1d],
                        '^' => vec![0x1e],
                        '_' => vec![0x1f],
                        ' ' => vec![0x00],
                        _ => return true,
                    }
                }
            }
            KeyCode::Char(c) => {
                let mut b = [0u8; 4];
                let s = c.encode_utf8(&mut b);
                if key.modifiers.contains(KeyModifiers::ALT) {
                    let mut v = vec![0x1b];
                    v.extend_from_slice(s.as_bytes());
                    v
                } else {
                    s.as_bytes().to_vec()
                }
            }
            KeyCode::Enter => vec![b'\r'],
            KeyCode::Backspace => vec![0x7f],
            KeyCode::Tab => vec![b'\t'],
            KeyCode::BackTab => b"\x1b[Z".to_vec(),
            KeyCode::Esc => vec![0x1b],
            KeyCode::Up => b"\x1b[A".to_vec(),
            KeyCode::Down => b"\x1b[B".to_vec(),
            KeyCode::Right => b"\x1b[C".to_vec(),
            KeyCode::Left => b"\x1b[D".to_vec(),
            KeyCode::Home => b"\x1b[H".to_vec(),
            KeyCode::End => b"\x1b[F".to_vec(),
            KeyCode::PageUp => b"\x1b[5~".to_vec(),
            KeyCode::PageDown => b"\x1b[6~".to_vec(),
            KeyCode::Delete => b"\x1b[3~".to_vec(),
            KeyCode::Insert => b"\x1b[2~".to_vec(),
            _ => return true,
        };
        let _ = self.writer.write_all(&bytes);
        let _ = self.writer.flush();
        true
    }

    /// Copy the vt100 screen into ratatui's buffer for `area`.
    pub fn render(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        use ratatui::style::{Color, Modifier, Style};

        let parser = self.parser.lock().unwrap();
        let screen = parser.screen();
        let buf = f.buffer_mut();
        let (rows, cols) = screen.size();

        for row in 0..rows.min(area.height) {
            for col in 0..cols.min(area.width) {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let x = area.x + col;
                let y = area.y + row;
                let target = &mut buf[(x, y)];
                let contents = cell.contents();
                if contents.is_empty() {
                    target.set_symbol(" ");
                } else {
                    target.set_symbol(&contents);
                }
                let mut style = Style::default()
                    .fg(vt_color(cell.fgcolor(), Color::Reset))
                    .bg(vt_color(cell.bgcolor(), Color::Reset));
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                target.set_style(style);
            }
        }

        // The cursor, as the terminal's own — not ratatui's, which the
        // dashboard hides in the alternate screen.
        if !screen.hide_cursor() {
            let (crow, ccol) = screen.cursor_position();
            if crow < area.height && ccol < area.width {
                let target = &mut buf[(area.x + ccol, area.y + crow)];
                target.set_style(target.style().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

fn vt_color(c: vt100::Color, default: ratatui::style::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
