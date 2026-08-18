//! `bsdkrun tui` — terminal ownership for the dashboard: raw mode + alternate
//! screen setup, and the three exit paths that must all restore the terminal
//! (normal quit, panic, signal). The dashboard itself lives in [`crate::tui`].

use std::io::Write;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossterm::event::{self, Event};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::cli::TuiArgs;
use crate::tty;
use crate::tui::{self, ui, App, Outcome};

/// Put the terminal back: leave the alternate screen, show the cursor, cook
/// the tty. Idempotent, so every exit path can call it.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let mut out = std::io::stdout();
    let _ = crossterm::execute!(out, LeaveAlternateScreen, crossterm::cursor::Show);
    let _ = out.flush();
}

/// Signal handler: leave the alternate screen (raw escape — async-signal-safe
/// `write`), restore termios, exit 128+sig. Unlike the boot handler in
/// `tty.rs`, there is no guest or gvproxy to kill here — but there IS an
/// alternate screen, which that handler knows nothing about.
extern "C" fn on_signal(sig: i32) {
    // "\x1b[?1049l" leaves the alternate screen, "\x1b[?25h" shows the cursor.
    const RESET: &[u8] = b"\x1b[?1049l\x1b[?25h";
    unsafe { libc::write(1, RESET.as_ptr().cast(), RESET.len()) };
    tty::restore_stdin_termios();
    unsafe { libc::_exit(128 + sig) };
}

pub(crate) fn cmd_tui(_args: TuiArgs) -> Result<()> {
    if unsafe { libc::isatty(0) } != 1 || unsafe { libc::isatty(1) } != 1 {
        bail!("the TUI needs a terminal (stdin/stdout is not a tty)");
    }

    // Termios snapshot first, then our signal handlers over it. SIGINT mostly
    // arrives as a key event under raw mode; the handler covers `kill -INT`.
    tty::save_stdin_termios();
    let handler = on_signal as extern "C" fn(i32) as libc::sighandler_t;
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        unsafe { libc::signal(sig, handler) };
    }

    // A panic must restore the terminal *before* the default hook prints, or
    // the report lands invisibly in the alternate screen the shell discards.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    enable_raw_mode().context("enabling raw mode")?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)
        .context("entering the alternate screen")?;

    let result = run(&mut App::new());
    restore_terminal();
    result
}

fn run(app: &mut App) -> Result<()> {
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;
    loop {
        terminal.draw(|f| ui::render(f, app))?;
        app.frame = app.frame.wrapping_add(1);

        // Keep an open shell's pty in step with the modal it renders in —
        // before polling, so the child never draws against a stale size.
        if app.term.is_some() {
            let size = terminal.size()?;
            let area = ui::term_inner_area(
                ratatui::layout::Rect::new(0, 0, size.width, size.height),
                app.term_fullscreen,
            );
            if let Some(term) = &mut app.term {
                term.resize(area.height, area.width);
            }
        }

        if event::poll(Duration::from_millis(50))? {
            // A resize (or any non-key event) just redraws on the next turn.
            if let Event::Key(key) = event::read()? {
                let Outcome::Handled = tui::handle_key(app, key);
            }
        }
        app.drain();
        if app.should_quit {
            return Ok(());
        }
    }
}
