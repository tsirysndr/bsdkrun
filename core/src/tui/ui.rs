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

const TEAL: Color = Color::Rgb(0, 232, 198);
const VIOLET: Color = Color::Rgb(130, 100, 255);
const MUTED: Color = Color::Rgb(140, 150, 160);
const GREEN: Color = Color::Rgb(80, 220, 120);
const RED: Color = Color::Rgb(255, 100, 100);
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
    render_panels(f, app, body);
    render_status_line(f, app, status);

    // Modals, one at a time, over everything.
    if let Some(log) = &app.log {
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
    let line = Line::from(vec![
        Span::styled(
            " bsdkrun ",
            Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
        ),
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
        let [m, i, v, n] = Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
        ])
        .areas(area);
        render_machines(f, app, m);
        render_images(f, app, i);
        render_volumes(f, app, v);
        render_networks(f, app, n);
    } else {
        // Machines get the top; the other three share the bottom row.
        let [top, bottom] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
        render_machines(f, app, top);
        let [i, v, n] = Layout::horizontal([
            Constraint::Percentage(40),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
        ])
        .areas(bottom);
        render_images(f, app, i);
        render_volumes(f, app, v);
        render_networks(f, app, n);
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

/// The persistent bottom status line: selection context on the left, the last
/// action (or its spinner) in the middle, the domains chip + help hint right.
fn render_status_line(f: &mut Frame, app: &App, area: Rect) {
    let len = app.panel_len(app.focus);
    let left = if len == 0 {
        format!(" {} — empty", app.focus.title().to_lowercase())
    } else {
        let ctx = match app.focus {
            Panel::Machines => app
                .selected_machine()
                .map(|m| {
                    format!(
                        " · {} · {}",
                        display_name(m),
                        if m.running { "running" } else { "stopped" }
                    )
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        format!(
            " {} {}/{}{}",
            app.focus.title().to_lowercase(),
            app.selection() + 1,
            len,
            ctx
        )
    };

    let middle = match &app.busy {
        Some(label) => format!("{} {label}", SPINNER[app.frame % SPINNER.len()]),
        None => app.message.clone(),
    };

    let (chip, chip_style) = match &app.snap.domains {
        Some(d) if d.caddy_running => (
            format!("https ·{} ✓", d.settings.tld),
            Style::default().fg(GREEN),
        ),
        Some(d) => (
            format!("https ·{} !", d.settings.tld),
            Style::default().fg(YELLOW),
        ),
        None => ("domains off".to_string(), Style::default().fg(MUTED)),
    };

    let right_text = " ? help ";
    let right_width = chip.chars().count() + 3 + right_text.len();
    let middle_width = (area.width as usize)
        .saturating_sub(left.chars().count())
        .saturating_sub(right_width);
    let line = Line::from(vec![
        Span::styled(left, Style::default().fg(TEAL)),
        Span::styled(
            format!(
                "  {:^width$}",
                crate::commands::truncate(&middle, middle_width.saturating_sub(2)),
                width = middle_width.saturating_sub(2)
            ),
            Style::default().fg(if app.busy.is_some() { YELLOW } else { MUTED }),
        ),
        Span::styled(chip, chip_style),
        Span::styled(" · ", Style::default().fg(MUTED)),
        Span::styled(right_text, Style::default().fg(MUTED)),
    ]);
    f.render_widget(Paragraph::new(line), area);
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

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/ ", Style::default().fg(TEAL)),
            Span::raw(s.query.clone()),
            Span::styled("█", Style::default().fg(TEAL)),
        ])),
        input,
    );

    let visible = list.height as usize;
    let offset = s.sel.saturating_sub(visible.saturating_sub(1));
    let mut lines = Vec::new();
    for (row, hit) in s.hits.iter().enumerate().skip(offset).take(visible) {
        let entry = &s.entries[hit.entry];
        let mut spans = Vec::new();
        for (ci, ch) in entry.display.chars().enumerate() {
            let matched = hit.indices.contains(&(ci as u32));
            spans.push(Span::styled(
                ch.to_string(),
                if matched {
                    Style::default().fg(VIOLET).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ));
        }
        let mut line = Line::from(spans);
        if row == s.sel {
            line = line.style(Style::default().add_modifier(Modifier::REVERSED));
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
        key("Tab / S-Tab", "next / previous panel"),
        key("j k ↑ ↓", "move selection (g/G first/last)"),
        key("/", "fuzzy search across everything"),
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
