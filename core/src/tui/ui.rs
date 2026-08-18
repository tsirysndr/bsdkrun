//! Rendering for the dashboard. Pure: takes `&App`, draws a frame. The accent
//! palette mirrors the CLI's --help styling (teal headers, violet literals) so
//! the TUI reads as the same product.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::api;

use super::{display_name, App, Panel};

// The Night Rider palette (trustfall/vscode-night-rider) at its neon end,
// matching the desktop app's default theme: electric cyan for chrome, the
// theme's signature neon magenta for emphasis (its most-used token color),
// its own muted lavender, and teal/pink for good/bad.
const TEAL: Color = Color::Rgb(0x71, 0xE4, 0xFE);
const VIOLET: Color = Color::Rgb(0xE5, 0x91, 0xFF);
const MUTED: Color = Color::Rgb(0x69, 0x62, 0x92);
const GREEN: Color = Color::Rgb(0x55, 0xF0, 0xD7);
const RED: Color = Color::Rgb(0xFF, 0x70, 0x9D);
const YELLOW: Color = Color::Rgb(240, 200, 80);

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let [header, body, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(f, app, header);
    match app.tab {
        super::Tab::Dashboard => render_panels(f, app, body),
        super::Tab::Ci => render_ci(f, app, body),
    }
    render_status_line(f, app, status);

    // Modals, one at a time, over everything. The shell outranks them all:
    // while it is open it owns the keyboard, so it must own the screen.
    if let Some(term) = &app.term {
        let inner = term_inner_area(area, app.term_fullscreen);
        let outer = Rect {
            x: inner.x.saturating_sub(1),
            y: inner.y.saturating_sub(1),
            width: inner.width + 2,
            height: inner.height + 2,
        };
        f.render_widget(Clear, outer);
        let block = Block::bordered()
            .title(term.title.clone())
            .title_bottom(" C-\\ detach · ⎇⏎ fullscreen ")
            .border_style(Style::default().fg(GREEN));
        f.render_widget(block, outer);
        term.render(f, inner);
    } else if let Some(log) = &app.log {
        render_log(f, app, log, area);
    } else if let Some(w) = &app.wizard {
        render_wizard(f, w, area);
    } else if let Some(s) = &app.settings {
        render_settings(f, s, area);
    } else if let Some(c) = &app.confirm {
        render_confirm(f, c, area);
    } else if let Some(s) = &app.search {
        render_search(f, s, area);
    } else if app.help {
        render_help(f, area);
    }
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let up = app.snap.machines.iter().filter(|m| m.running).count();
    let tab = |label: &str, active: bool| {
        Span::styled(
            format!(" {label} "),
            if active {
                Style::default()
                    .fg(TEAL)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                Style::default().fg(MUTED)
            },
        )
    };
    let line = Line::from(vec![
        Span::styled(
            " bsdkrun ",
            Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
        ),
        tab("1 Dashboard", app.tab == super::Tab::Dashboard),
        tab("2 CI/CD", app.tab == super::Tab::Ci),
        Span::raw("  "),
        Span::styled(
            format!(
                "· {} machines ({} up) · {} images · {} volumes · {} networks",
                app.snap.machines.len(),
                up,
                app.snap.images.len(),
                app.snap.volumes.len(),
                app.snap.networks.len()
            ),
            Style::default().fg(MUTED),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_panels(f: &mut Frame, app: &App, area: Rect) {
    if area.width < 100 {
        // Narrow terminal: a vertical stack.
        let [m, i, v, n, s, a] = Layout::vertical([
            Constraint::Percentage(30),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
            Constraint::Percentage(14),
        ])
        .areas(area);
        render_machines(f, app, m);
        render_images(f, app, i);
        render_volumes(f, app, v);
        render_networks(f, app, n);
        render_snapshots(f, app, s);
        render_ai(f, app, a);
    } else {
        // Machines get the top; the other three share the bottom row.
        let [top, bottom] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
        render_machines(f, app, top);
        let [i, v, n, s, a] = Layout::horizontal([
            Constraint::Percentage(24),
            Constraint::Percentage(19),
            Constraint::Percentage(19),
            Constraint::Percentage(19),
            Constraint::Percentage(19),
        ])
        .areas(bottom);
        render_images(f, app, i);
        render_volumes(f, app, v);
        render_networks(f, app, n);
        render_snapshots(f, app, s);
        render_ai(f, app, a);
    }
}

fn panel_block(app: &App, panel: Panel) -> Block<'static> {
    let focused = app.focus == panel;
    let style = if focused {
        Style::default().fg(TEAL)
    } else {
        Style::default().fg(MUTED)
    };
    let title = format!(" {} ", panel.title());
    Block::default()
        .borders(Borders::ALL)
        .border_style(style)
        .title(Span::styled(
            title,
            if focused {
                Style::default().fg(TEAL).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            },
        ))
}

/// Render a panel's rows with selection + scroll-into-view.
fn render_rows(f: &mut Frame, app: &App, panel: Panel, area: Rect, rows: Vec<Line<'static>>) {
    let block = panel_block(app, panel);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("(none)", Style::default().fg(MUTED))),
            inner,
        );
        return;
    }
    let visible = inner.height as usize;
    let sel = app.sel[Panel::ALL.iter().position(|p| *p == panel).unwrap()]
        .min(rows.len().saturating_sub(1));
    let offset = sel.saturating_sub(visible.saturating_sub(1));
    let focused = app.focus == panel;
    let mut lines = Vec::with_capacity(visible);
    for (i, mut line) in rows.into_iter().enumerate().skip(offset).take(visible) {
        if i == sel && focused {
            line = line.style(Style::default().add_modifier(Modifier::REVERSED));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn machine_row(app: &App, m: &api::Machine) -> Line<'static> {
    let dot = if m.running {
        Span::styled("● ", Style::default().fg(GREEN))
    } else {
        Span::styled("○ ", Style::default().fg(MUTED))
    };
    let ports = if m.ports.is_empty() {
        "-".to_string()
    } else {
        m.ports
            .iter()
            .map(|p| format!("{}:{}", p.host, p.guest))
            .collect::<Vec<_>>()
            .join(",")
    };
    let url = app.machine_url(m).unwrap_or_default();
    Line::from(vec![
        dot,
        Span::styled(
            format!("{:<20}", crate::commands::truncate(&display_name(m), 20)),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {:<8}", if m.running { "running" } else { "stopped" }),
            Style::default().fg(if m.running { GREEN } else { MUTED }),
        ),
        Span::styled(
            format!("  {:<10}", crate::commands::truncate(&m.kind, 10)),
            Style::default().fg(MUTED),
        ),
        Span::raw(format!("  {:<24}", crate::commands::truncate(&m.image, 24))),
        Span::styled(
            format!("  {:<16}", crate::commands::truncate(&ports, 16)),
            Style::default().fg(MUTED),
        ),
        Span::styled(url, Style::default().fg(VIOLET)),
    ])
}

fn render_machines(f: &mut Frame, app: &App, area: Rect) {
    let rows = app
        .snap
        .machines
        .iter()
        .map(|m| machine_row(app, m))
        .collect();
    render_rows(f, app, Panel::Machines, area, rows);
}

fn render_images(f: &mut Frame, app: &App, area: Rect) {
    let rows = app
        .snap
        .images
        .iter()
        .map(|i| {
            Line::from(vec![
                Span::raw(format!(
                    "{:<30}",
                    crate::commands::truncate(&i.reference, 30)
                )),
                Span::styled(
                    format!("  {}", crate::oci::human_size(i.size as u64)),
                    Style::default().fg(MUTED),
                ),
            ])
        })
        .collect();
    render_rows(f, app, Panel::Images, area, rows);
}

fn render_volumes(f: &mut Frame, app: &App, area: Rect) {
    let rows = app
        .snap
        .volumes
        .iter()
        .map(|v| {
            Line::from(vec![
                Span::raw(format!("{:<20}", crate::commands::truncate(&v.name, 20))),
                Span::styled(
                    format!("  {}", v.size.as_deref().unwrap_or("-")),
                    Style::default().fg(MUTED),
                ),
            ])
        })
        .collect();
    render_rows(f, app, Panel::Volumes, area, rows);
}

fn render_networks(f: &mut Frame, app: &App, area: Rect) {
    let rows = app
        .snap
        .networks
        .iter()
        .map(|n| {
            Line::from(vec![
                Span::styled(
                    if n.up { "● " } else { "○ " },
                    Style::default().fg(if n.up { GREEN } else { MUTED }),
                ),
                Span::raw(format!("{:<16}", crate::commands::truncate(&n.name, 16))),
                Span::styled(
                    format!("  {} up/{}", n.running, n.members),
                    Style::default().fg(MUTED),
                ),
            ])
        })
        .collect();
    render_rows(f, app, Panel::Networks, area, rows);
}

fn render_snapshots(f: &mut Frame, app: &App, area: Rect) {
    let rows = app
        .snap
        .disk_snapshots
        .iter()
        .map(|s| {
            Line::from(vec![
                Span::raw(format!("{:<18}", crate::commands::truncate(&s.name, 18))),
                Span::styled(
                    format!(
                        "  {} · {}",
                        crate::commands::truncate(
                            if s.machine_name.is_empty() {
                                &s.machine_id
                            } else {
                                &s.machine_name
                            },
                            14
                        ),
                        s.kind
                    ),
                    Style::default().fg(MUTED),
                ),
            ])
        })
        .collect();
    render_rows(f, app, Panel::Snapshots, area, rows);
}

fn render_ai(f: &mut Frame, app: &App, area: Rect) {
    let rows = app
        .snap
        .ai
        .iter()
        .map(|s| {
            Line::from(vec![
                Span::styled(
                    if s.running { "● " } else { "○ " },
                    Style::default().fg(if s.running { GREEN } else { MUTED }),
                ),
                Span::raw(format!(
                    "{:<14}",
                    crate::commands::truncate(s.label.as_deref().unwrap_or(&s.name), 14)
                )),
                Span::styled(format!("  {}", s.agent), Style::default().fg(MUTED)),
            ])
        })
        .collect();
    render_rows(f, app, Panel::Ai, area, rows);
}

/// The CI/CD tab: workflows on the left, the latest run's step timeline on
/// the right — the same two halves as the desktop app's CI screen, rendered
/// from the same LogLine stream.
fn render_ci(f: &mut Frame, app: &App, area: Rect) {
    use super::ci::{RunStatus, StepStatus};

    let [left, right] =
        Layout::horizontal([Constraint::Length(34), Constraint::Min(20)]).areas(area);

    // Workflows.
    let block = Block::bordered()
        .title(" Workflows (⏎ run · x cancel) ")
        .border_style(Style::default().fg(TEAL));
    let inner = block.inner(left);
    f.render_widget(block, left);
    let mut lines: Vec<Line> = Vec::new();
    if !app.ci.note.is_empty() {
        lines.push(Line::from(Span::styled(
            app.ci.note.clone(),
            Style::default().fg(MUTED),
        )));
    }
    for (i, w) in app.ci.workflows.iter().enumerate() {
        let sel = i == app.ci.sel;
        let mut spans = vec![
            Span::styled(if sel { "▌ " } else { "  " }, Style::default().fg(TEAL)),
            Span::styled(
                if w.matches { "● " } else { "○ " },
                Style::default().fg(if w.matches { GREEN } else { MUTED }),
            ),
            Span::raw(format!("{:<18}", crate::commands::truncate(&w.name, 18))),
            Span::styled(w.engine.clone(), Style::default().fg(MUTED)),
        ];
        if sel {
            spans = spans
                .into_iter()
                .map(|sp| {
                    let style = sp.style.add_modifier(Modifier::BOLD);
                    sp.style(style)
                })
                .collect();
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);

    // The latest run.
    let (title, border) = match app.ci.current_run() {
        None => (" Run ".to_string(), MUTED),
        Some(r) => match r.status {
            RunStatus::Running => (format!(" Run · {} · live ", r.workflow), TEAL),
            RunStatus::Success => (format!(" Run · {} · passed ", r.workflow), GREEN),
            RunStatus::Failed => (format!(" Run · {} · failed ", r.workflow), RED),
            RunStatus::Cancelled => (format!(" Run · {} · cancelled ", r.workflow), MUTED),
        },
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(border));
    let inner = block.inner(right);
    f.render_widget(block, right);

    let Some(run) = app.ci.current_run() else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "select a workflow and press ⏎ — it boots a microVM and streams here",
                Style::default().fg(MUTED),
            ))),
            inner,
        );
        return;
    };

    // Steps, then the open (or last) step's tail in the remaining space.
    let mut lines: Vec<Line> = Vec::new();
    for step in &run.steps {
        let (glyph, color) = match step.status {
            StepStatus::Running => ("… ", TEAL),
            StepStatus::Ok => ("✓ ", GREEN),
            StepStatus::Failed => ("✗ ", RED),
        };
        let dur = step
            .duration
            .map(|d| format!("  {:.1}s", d.as_secs_f64()))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(glyph, Style::default().fg(color)),
            Span::raw(step.name.clone()),
            Span::styled(
                if step.system { "  [setup]" } else { "" }.to_string(),
                Style::default().fg(MUTED),
            ),
            Span::styled(dur, Style::default().fg(MUTED)),
        ]));
    }
    // The tail: whatever the focused step printed last, bounded to the space
    // left after the step list.
    if let Some(step) = run
        .steps
        .iter()
        .rev()
        .find(|s| s.status != StepStatus::Ok)
        .or(run.steps.last())
    {
        let room = (inner.height as usize).saturating_sub(lines.len() + 1);
        if room > 0 && !step.lines.is_empty() {
            lines.push(Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(MUTED),
            )));
            for l in step.lines.iter().rev().take(room).rev() {
                lines.push(Line::from(Span::styled(
                    l.clone(),
                    Style::default().fg(MUTED),
                )));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// The persistent bottom status line: selection context on the left, the last
/// action (or its spinner) in the middle, the domains chip + help hint right.
/// The neovim-style statusline: a mode block on the left, context and the
/// last message as segments, context-sensitive shortcuts on the right, and a
/// position block mirroring the mode color — colored backgrounds throughout,
/// so the modes read at a glance the way `-- INSERT --` never did.
fn render_status_line(f: &mut Frame, app: &App, area: Rect) {
    const INK: Color = Color::Rgb(0x1e, 0x1c, 0x3f);
    const SEG_BG: Color = Color::Rgb(0x2d, 0x2b, 0x55);
    const SEG_FG: Color = Color::Rgb(0xc9, 0xcb, 0xdb);

    // Mode: which state owns the keyboard right now.
    let (mode, mode_bg) = if app.term.is_some() {
        ("TERMINAL", GREEN)
    } else if app.search.is_some() {
        ("SEARCH", VIOLET)
    } else if app.confirm.is_some() {
        ("CONFIRM", RED)
    } else if app.wizard.is_some() || app.settings.is_some() {
        ("EDIT", YELLOW)
    } else if app.tab == super::Tab::Ci {
        ("CI/CD", TEAL)
    } else {
        ("NORMAL", TEAL)
    };

    // Context segment: where you are.
    let context = if app.term.is_some() {
        app.term
            .as_ref()
            .map(|t| t.title.clone())
            .unwrap_or_default()
    } else if app.tab == super::Tab::Ci {
        match app.ci.current_run() {
            Some(r) => format!(" {} ", r.workflow),
            None => " workflows ".into(),
        }
    } else {
        format!(" {} ", app.focus.title().to_lowercase())
    };

    // Shortcuts: what works *here*. The point of putting them in the bar is
    // that nobody has to open `?` to remember the three keys that matter.
    let hints: &str = if app.term.is_some() {
        "C-\\ detach  ⎇⏎ fullscreen"
    } else if app.search.is_some() {
        "⏎ jump  C-u clear  esc close"
    } else if app.confirm.is_some() {
        "y confirm  n cancel"
    } else if app.tab == super::Tab::Ci {
        "⏎ run  x cancel  j/k move  1 dashboard  ? help"
    } else {
        match app.focus {
            Panel::Machines => "⏎ info  e shell  l logs  s start  x stop  d rm  n new  / search",
            Panel::Snapshots => "/ search  tab next  2 ci/cd  ? help",
            Panel::Ai => "/ search  tab next  2 ci/cd  ? help",
            _ => "/ search  tab next  n new  2 ci/cd  ? help",
        }
    };

    // Position block, far right, in the mode color (nvim's ruler).
    let position = if app.term.is_some() || app.tab == super::Tab::Ci {
        String::new()
    } else {
        let len = app.panel_len(app.focus);
        if len == 0 {
            " 0/0 ".into()
        } else {
            format!(" {}/{} ", app.selection() + 1, len)
        }
    };

    // Middle: busy spinner or the last message, on the bar background.
    let middle = match &app.busy {
        Some(label) => format!("{} {label}", SPINNER[app.frame % SPINNER.len()]),
        None => app.message.clone(),
    };
    let domains = match &app.snap.domains {
        Some(d) if d.caddy_running => format!("·{} ", d.settings.tld),
        _ => String::new(),
    };

    let mode_block = format!(" {mode} ");
    let fixed = mode_block.chars().count()
        + context.chars().count()
        + 1
        + hints.chars().count()
        + 2
        + domains.chars().count()
        + position.chars().count();
    let middle_width = (area.width as usize).saturating_sub(fixed);
    let middle = format!(
        " {:<width$}",
        crate::commands::truncate(&middle, middle_width.saturating_sub(1)),
        width = middle_width.saturating_sub(1)
    );

    let line = Line::from(vec![
        Span::styled(
            mode_block,
            Style::default()
                .fg(INK)
                .bg(mode_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(context, Style::default().fg(SEG_FG).bg(SEG_BG)),
        Span::styled(
            middle,
            Style::default().fg(if app.busy.is_some() { YELLOW } else { MUTED }),
        ),
        Span::styled(format!("{hints}  "), Style::default().fg(SEG_FG).bg(SEG_BG)),
        Span::styled(domains, Style::default().fg(INK).bg(mode_bg)),
        Span::styled(
            position,
            Style::default()
                .fg(INK)
                .bg(mode_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// Where the embedded shell's *content* lives, for both the renderer and the
/// run loop (which must size the pty to exactly this before the child draws).
pub fn term_inner_area(frame: Rect, fullscreen: bool) -> Rect {
    let body = Rect {
        x: frame.x,
        y: frame.y + 1,
        width: frame.width,
        height: frame.height.saturating_sub(2),
    };
    let outer = if fullscreen {
        body
    } else {
        centered(
            body,
            body.width.saturating_sub(8).max(40),
            body.height.saturating_sub(4).max(10),
        )
    };
    // Inside the border.
    Rect {
        x: outer.x + 1,
        y: outer.y + 1,
        width: outer.width.saturating_sub(2),
        height: outer.height.saturating_sub(2),
    }
}

/// A centered modal rect, clamped to the frame.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(VIOLET))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(VIOLET).add_modifier(Modifier::BOLD),
        ))
}

fn render_confirm(f: &mut Frame, c: &super::Confirm, area: Rect) {
    let rect = centered(area, 60, 5);
    f.render_widget(Clear, rect);
    let block = modal_block("confirm");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::raw(c.prompt.clone()),
        Line::default(),
        Line::from(vec![
            Span::styled("y", Style::default().fg(TEAL)),
            Span::raw("/Enter remove   "),
            Span::styled("n", Style::default().fg(TEAL)),
            Span::raw("/Esc cancel"),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn field_line(label: &str, value: &str, focused: bool) -> Line<'static> {
    let style = if focused {
        Style::default().fg(TEAL).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let cursor = if focused { "█" } else { "" };
    Line::from(vec![
        Span::styled(format!("{label:<8}"), Style::default().fg(MUTED)),
        Span::styled(format!("{value}{cursor}"), style),
    ])
}

fn render_wizard(f: &mut Frame, w: &super::Wizard, area: Rect) {
    let rect = centered(area, 56, 11);
    f.render_widget(Clear, rect);
    let block = modal_block("new machine");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines = vec![
        field_line("image", &w.image, w.field == 0),
        field_line("name", &w.name, w.field == 1),
        field_line("port", &w.port, w.field == 2),
        field_line("cpus", &w.cpus, w.field == 3),
        field_line("mem", &w.mem, w.field == 4),
        Line::default(),
        Line::from(Span::styled(
            "Tab next · Enter boot detached · Esc cancel",
            Style::default().fg(MUTED),
        )),
    ];
    if let Some(e) = &w.error {
        lines.push(Line::from(Span::styled(
            e.clone(),
            Style::default().fg(RED),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_settings(f: &mut Frame, s: &super::SettingsModal, area: Rect) {
    let rect = centered(area, 52, 8);
    f.render_widget(Clear, rect);
    let block = modal_block(&format!("settings — {}", s.machine));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let mut lines = vec![
        field_line("cpus", &s.cpus, s.field == 0),
        field_line("mem", &s.mem, s.field == 1),
        Line::default(),
        Line::from(Span::styled(
            "applies on next start · Enter save · Esc cancel",
            Style::default().fg(MUTED),
        )),
    ];
    if let Some(e) = &s.error {
        lines.push(Line::from(Span::styled(
            e.clone(),
            Style::default().fg(RED),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_log(f: &mut Frame, _app: &App, log: &super::logs::LogModal, area: Rect) {
    let rect = centered(
        area,
        area.width.saturating_sub(6),
        area.height.saturating_sub(4),
    );
    f.render_widget(Clear, rect);
    let title = format!("{}{}", log.title, if log.live { " (live)" } else { "" });
    let block = modal_block(&title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let visible = inner.height as usize;
    let total = log.lines.len();
    let offset = if log.follow {
        total.saturating_sub(visible)
    } else {
        log.scroll.min(total.saturating_sub(1))
    };
    let lines: Vec<Line> = log
        .lines
        .iter()
        .skip(offset)
        .take(visible)
        .map(|l| Line::raw(l.clone()))
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_search(f: &mut Frame, s: &super::search::SearchModal, area: Rect) {
    let rect = centered(area, 76, area.height.saturating_sub(6).min(24));
    f.render_widget(Clear, rect);
    let block = modal_block("search");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);

    // fzf's chrome, faithfully: the prompt on the left, the live match
    // counter on the right — the counter is what tells you a query went from
    // narrowing to over-narrowing without reading the list.
    let counter = format!("{}/{}", s.hits.len(), s.entries.len());
    let pad =
        (input.width as usize).saturating_sub(2 + s.query.chars().count() + 1 + counter.len() + 1);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/ ", Style::default().fg(TEAL)),
            Span::raw(s.query.clone()),
            Span::styled("█", Style::default().fg(TEAL)),
            Span::raw(" ".repeat(pad)),
            Span::styled(counter, Style::default().fg(MUTED)),
        ])),
        input,
    );

    let visible = list.height as usize;
    let offset = s.sel.saturating_sub(visible.saturating_sub(1));
    let mut lines = Vec::new();
    for (row, hit) in s.hits.iter().enumerate().skip(offset).take(visible) {
        let entry = &s.entries[hit.entry];
        let mut spans = Vec::new();
        // fzf's pointer column: the selected row gets a bar, everything else
        // an aligning space — steadier to scan than a full-line reverse.
        spans.push(if row == s.sel {
            Span::styled("▌ ", Style::default().fg(TEAL))
        } else {
            Span::raw("  ")
        });
        // The leading word is the entry's kind ("machine", "snapshot", …);
        // colored as a tag so mixed results scan by type at a glance.
        let kind_len = entry.display.find("  ").unwrap_or(0);
        for (ci, ch) in entry.display.chars().enumerate() {
            let matched = hit.indices.contains(&(ci as u32));
            spans.push(Span::styled(
                ch.to_string(),
                if matched {
                    Style::default().fg(VIOLET).add_modifier(Modifier::BOLD)
                } else if ci < kind_len {
                    Style::default().fg(TEAL)
                } else {
                    Style::default()
                },
            ));
        }
        let mut line = Line::from(spans);
        if row == s.sel {
            line = line.style(Style::default().add_modifier(Modifier::BOLD));
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "no matches",
            Style::default().fg(MUTED),
        )));
    }
    f.render_widget(Paragraph::new(lines), list);
}

fn render_help(f: &mut Frame, area: Rect) {
    let rect = centered(area, 62, 22);
    f.render_widget(Clear, rect);
    let block = modal_block("keybindings");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let key = |k: &str, what: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<12}"), Style::default().fg(TEAL)),
            Span::raw(what.to_string()),
        ])
    };
    let section = |t: &str| {
        Line::from(Span::styled(
            t.to_string(),
            Style::default().fg(VIOLET).add_modifier(Modifier::BOLD),
        ))
    };
    let lines = vec![
        section("Global"),
        key("1 / 2", "Dashboard / CI-CD tab"),
        key("Tab / S-Tab", "next / previous panel"),
        key("j k ↑ ↓", "move selection (g/G first/last)"),
        key("/", "fuzzy search across everything"),
        section("CI/CD tab"),
        key("⏎", "run the selected workflow in a microVM"),
        key("x", "cancel the live run"),
        key("?", "this help"),
        key("r", "refresh now"),
        key("q / Ctrl-C", "quit"),
        section("Machines"),
        key("n", "new machine (wizard)"),
        key("s", "start selected"),
        key("x", "stop selected"),
        key("e", "shell into selected (suspends the TUI)"),
        key("l", "logs (follows a running machine live)"),
        key("i / Enter", "settings (vCPU / memory)"),
        key("o", "open https://<name>.<tld> in the browser"),
        key("d", "remove (with confirmation)"),
        section("Log viewer"),
        key("j k / PgUp/Dn", "scroll (G re-sticks to the tail)"),
        key("Esc / q", "close"),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}
