//! The `bsdkrun tui` dashboard: machines, images, volumes, networks, disk
//! snapshots and AI sandboxes as live panels, with the machine lifecycle
//! (start/stop/shell/new/settings/logs) driven from the keyboard.
//!
//! Threading: the UI thread only draws and routes keys. A single worker thread
//! owns every blocking call — `api::list_*` snapshots on a tick, and actions
//! like `api::stop`, which can legitimately block for tens of seconds on a
//! graceful BSD poweroff. Each `api::*` call opens its own `Db` (with its own
//! current-thread runtime), so nothing is shared across threads; WAL +
//! busy_timeout already arbitrate concurrent access, exactly as they do for
//! the desktop app's process swarm.
//!
//! Booting is different: it forks and the child *becomes* the machine, so the
//! TUI never boots in-process — it spawns `current_exe() start <id>` (or a
//! full `linux …` argv from the New-machine wizard) and lets the next snapshot
//! pick up the result.

pub mod ci;
pub mod logs;
pub mod search;
pub mod term;
pub mod ui;

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::{api, db, domains};

/// One consistent view of the world, replaced atomically by the worker.
#[derive(Default)]
pub struct Snapshot {
    pub machines: Vec<api::Machine>,
    pub images: Vec<api::Image>,
    pub volumes: Vec<api::Volume>,
    pub networks: Vec<api::Network>,
    /// Disk snapshots (`bsdkrun snapshots`) — not to be confused with this
    /// struct, which is a snapshot of the *dashboard's* data.
    pub disk_snapshots: Vec<api::Snapshot>,
    /// AI agent sandboxes, grouped nowhere: the TUI is a flat list.
    pub ai: Vec<crate::ai::Session>,
    pub domains: Option<DomainsInfo>,
}

/// What the status-line chip needs to know about machine domains.
#[derive(Clone)]
pub struct DomainsInfo {
    pub settings: domains::Settings,
    pub caddy_running: bool,
}

/// UI → worker.
pub enum Action {
    Refresh,
    Stop(String),
    Remove(String),
    Update {
        id: String,
        cpus: Option<u8>,
        mem: Option<u32>,
    },
}

/// Worker → UI.
pub enum Msg {
    Snapshot(Box<Snapshot>),
    /// An action finished; show its outcome on the status line.
    Done(Result<String, String>),
}

/// Top-level tabs. The dashboard is the original panel grid; CI/CD gets a
/// screen of its own — workflows plus a live step timeline want more room
/// than a seventh grid cell could give them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Ci,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Machines,
    Images,
    Volumes,
    Networks,
    Snapshots,
    Ai,
}

impl Panel {
    pub const ALL: [Panel; 6] = [
        Panel::Machines,
        Panel::Images,
        Panel::Volumes,
        Panel::Networks,
        Panel::Snapshots,
        Panel::Ai,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Panel::Machines => "Machines",
            Panel::Images => "Images",
            Panel::Volumes => "Volumes",
            Panel::Networks => "Networks",
            Panel::Snapshots => "Snapshots",
            Panel::Ai => "AI Sandboxes",
        }
    }

    fn index(self) -> usize {
        Panel::ALL.iter().position(|p| *p == self).unwrap()
    }

    fn next(self) -> Panel {
        Panel::ALL[(self.index() + 1) % Panel::ALL.len()]
    }

    fn prev(self) -> Panel {
        Panel::ALL[(self.index() + Panel::ALL.len() - 1) % Panel::ALL.len()]
    }
}

/// Yes/no confirmation modal (currently only machine removal).
pub struct Confirm {
    pub prompt: String,
    pub id: String,
}

/// The New-machine wizard's fields, edited in order.
pub struct Wizard {
    pub image: String,
    pub name: String,
    pub port: String,
    pub cpus: String,
    pub mem: String,
    /// Focused field, 0..5 in the order above.
    pub field: usize,
    pub error: Option<String>,
}

impl Wizard {
    pub const FIELDS: usize = 5;

    fn new() -> Self {
        Wizard {
            image: String::new(),
            name: String::new(),
            port: String::new(),
            cpus: "1".into(),
            mem: "512".into(),
            field: 0,
            error: None,
        }
    }

    pub fn field_mut(&mut self) -> &mut String {
        match self.field {
            0 => &mut self.image,
            1 => &mut self.name,
            2 => &mut self.port,
            3 => &mut self.cpus,
            _ => &mut self.mem,
        }
    }
}

/// The machine-settings modal: vCPU / memory, applied on next start.
pub struct SettingsModal {
    pub id: String,
    pub machine: String,
    pub cpus: String,
    pub mem: String,
    /// 0 = cpus, 1 = mem.
    pub field: usize,
    pub error: Option<String>,
}

pub struct App {
    pub snap: Snapshot,
    pub focus: Panel,
    pub sel: [usize; 6],
    pub tab: Tab,
    pub ci: ci::CiTab,
    /// The embedded shell, when one is open. Rendered as a modal over
    /// everything; `term_fullscreen` trades the chrome for the whole body.
    pub term: Option<term::TermPane>,
    pub term_fullscreen: bool,
    /// Status line: outcome of the last action.
    pub message: String,
    /// A worker action in flight, shown as a spinner label.
    pub busy: Option<String>,
    /// Frame counter, drives the spinner.
    pub frame: usize,
    pub search: Option<search::SearchModal>,
    pub help: bool,
    pub confirm: Option<Confirm>,
    pub wizard: Option<Wizard>,
    pub settings: Option<SettingsModal>,
    pub log: Option<logs::LogModal>,
    pub should_quit: bool,
    action_tx: mpsc::Sender<Action>,
    rx: mpsc::Receiver<Msg>,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let (action_tx, action_rx) = mpsc::channel::<Action>();
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::spawn(move || worker(action_rx, tx));
        // First paint happens before the first snapshot lands; ask for it now.
        let _ = action_tx.send(Action::Refresh);
        App {
            snap: Snapshot::default(),
            focus: Panel::Machines,
            sel: [0; 6],
            tab: Tab::Dashboard,
            ci: ci::CiTab::new(),
            term: None,
            term_fullscreen: false,
            message: String::new(),
            busy: None,
            frame: 0,
            search: None,
            help: false,
            confirm: None,
            wizard: None,
            settings: None,
            log: None,
            should_quit: false,
            action_tx,
            rx,
        }
    }

    pub fn panel_len(&self, panel: Panel) -> usize {
        match panel {
            Panel::Machines => self.snap.machines.len(),
            Panel::Images => self.snap.images.len(),
            Panel::Volumes => self.snap.volumes.len(),
            Panel::Networks => self.snap.networks.len(),
            Panel::Snapshots => self.snap.disk_snapshots.len(),
            Panel::Ai => self.snap.ai.len(),
        }
    }

    pub fn selection(&self) -> usize {
        self.sel[self.focus.index()].min(self.panel_len(self.focus).saturating_sub(1))
    }

    pub fn set_selection(&mut self, panel: Panel, index: usize) {
        self.sel[panel.index()] = index;
    }

    pub fn selected_machine(&self) -> Option<&api::Machine> {
        if self.focus != Panel::Machines {
            return None;
        }
        self.snap.machines.get(self.selection())
    }

    fn move_sel(&mut self, delta: isize) {
        let len = self.panel_len(self.focus);
        if len == 0 {
            return;
        }
        let cur = self.selection() as isize;
        let next = (cur + delta).rem_euclid(len as isize) as usize;
        self.sel[self.focus.index()] = next;
    }

    fn send(&mut self, action: Action, busy: &str) {
        self.busy = Some(busy.to_string());
        let _ = self.action_tx.send(action);
    }

    /// Drain worker messages; returns true if anything changed (needs redraw).
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Snapshot(s) => {
                    self.snap = *s;
                    // Clamp every selection against the fresh lists.
                    for (i, p) in Panel::ALL.iter().enumerate() {
                        self.sel[i] = self.sel[i].min(self.panel_len(*p).saturating_sub(1));
                    }
                }
                Msg::Done(result) => {
                    self.busy = None;
                    self.message = match result {
                        Ok(m) => m,
                        Err(e) => format!("error: {e}"),
                    };
                }
            }
            changed = true;
        }
        if let Some(log) = &mut self.log {
            changed |= log.drain();
        }
        changed |= self.ci.drain();
        if let Some(term) = &self.term {
            changed |= term.take_dirty();
            if term.is_dead() {
                self.term = None;
                self.term_fullscreen = false;
                self.message = "shell session ended".into();
                changed = true;
            }
        }
        changed
    }

    /// The machine domains URL for a machine, when the feature is on.
    pub fn machine_url(&self, m: &api::Machine) -> Option<String> {
        let info = self.snap.domains.as_ref()?;
        domains::machine_url(m, &info.settings)
    }
}

/// The worker: refresh on a tick, service actions, snapshot after each.
fn worker(rx: mpsc::Receiver<Action>, tx: mpsc::Sender<Msg>) {
    const TICK: Duration = Duration::from_millis(1500);
    let mut next_refresh = Instant::now();
    loop {
        let timeout = next_refresh.saturating_duration_since(Instant::now());
        let action = match rx.recv_timeout(timeout) {
            Ok(a) => Some(a),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        match action {
            Some(Action::Refresh) | None => {}
            Some(Action::Stop(id)) => {
                let r = api::stop(&id).map_err(|e| e.to_string());
                if tx
                    .send(Msg::Done(r.map(|id| format!("stopped {id}"))))
                    .is_err()
                {
                    return;
                }
            }
            Some(Action::Remove(id)) => {
                let r = api::remove_machine(&id, true).map_err(|e| e.to_string());
                if tx
                    .send(Msg::Done(r.map(|id| format!("removed {id}"))))
                    .is_err()
                {
                    return;
                }
            }
            Some(Action::Update { id, cpus, mem }) => {
                let r = api::update(&id, cpus, mem).map_err(|e| e.to_string());
                if tx
                    .send(Msg::Done(
                        r.map(|id| format!("updated {id} (applies on next start)")),
                    ))
                    .is_err()
                {
                    return;
                }
            }
        }
        if tx.send(Msg::Snapshot(Box::new(snapshot()))).is_err() {
            return;
        }
        next_refresh = Instant::now() + TICK;
    }
}

/// Gather one snapshot. Any failing list becomes empty rather than tearing the
/// dashboard down — a transient DB lock shouldn't blank the screen.
fn snapshot() -> Snapshot {
    let domains = (|| -> Result<Option<DomainsInfo>> {
        let db = db::Db::open()?;
        let settings = domains::Settings::load(&db)?;
        if !settings.enabled {
            return Ok(None);
        }
        let caddy_running = domains::caddy::running(&db)?;
        Ok(Some(DomainsInfo {
            settings,
            caddy_running,
        }))
    })()
    .unwrap_or(None);
    Snapshot {
        machines: api::list_machines(true).unwrap_or_default(),
        disk_snapshots: api::list_snapshots(None).unwrap_or_default(),
        ai: crate::ai::sessions().unwrap_or_default(),
        images: api::list_images().unwrap_or_default(),
        volumes: api::list_volumes().unwrap_or_default(),
        networks: api::list_networks().unwrap_or_default(),
        domains,
    }
}

/// What a key press asked the event loop to do beyond mutating `App` — the
/// variants that need the terminal itself (suspend for a subprocess) are
/// returned to `cmd_tui`, which owns it.
pub enum Outcome {
    /// Nothing further; redraw.
    Handled,
}

/// Route one key press.
pub fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) -> Outcome {
    // An open shell owns the keyboard outright — everything is forwarded to
    // the guest except the chords that are ours: Ctrl-\ detaches (and ends
    // the session; a shell nobody can see is a leak), Alt-Enter and F11
    // toggle fullscreen.
    if app.term.is_some() {
        use crossterm::event::{KeyCode, KeyModifiers};
        let alt_enter = key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::ALT);
        if alt_enter || key.code == KeyCode::F(11) {
            app.term_fullscreen = !app.term_fullscreen;
            return Outcome::Handled;
        }
        if let Some(term) = &mut app.term {
            if !term.handle_key(key) {
                app.term = None;
                app.term_fullscreen = false;
                app.message = "shell detached".into();
            }
        }
        return Outcome::Handled;
    }
    use crossterm::event::{KeyCode, KeyModifiers};

    if key.kind != crossterm::event::KeyEventKind::Press {
        return Outcome::Handled;
    }
    // Ctrl-C quits from anywhere (raw mode delivers it as a key).
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return Outcome::Handled;
    }

    // Modal-first routing; one modal at a time.
    if app.search.is_some() {
        search::handle_key(app, key);
        return Outcome::Handled;
    }
    if app.help {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
        ) {
            app.help = false;
        }
        return Outcome::Handled;
    }
    if let Some(confirm) = &app.confirm {
        match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                let id = confirm.id.clone();
                app.confirm = None;
                app.send(Action::Remove(id.clone()), &format!("removing {id}…"));
            }
            KeyCode::Char('n') | KeyCode::Esc => app.confirm = None,
            _ => {}
        }
        return Outcome::Handled;
    }
    if app.wizard.is_some() {
        handle_wizard_key(app, key);
        return Outcome::Handled;
    }
    if app.settings.is_some() {
        handle_settings_key(app, key);
        return Outcome::Handled;
    }
    if app.log.is_some() {
        logs::handle_key(app, key);
        return Outcome::Handled;
    }

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        // Tabs before anything panel-shaped: 1/2 are unclaimed at top level,
        // and every other binding below is meaningless on the CI tab.
        KeyCode::Char('1') => app.tab = Tab::Dashboard,
        KeyCode::Char('2') => app.tab = Tab::Ci,
        _ if app.tab == Tab::Ci => {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    app.ci.sel = (app.ci.sel + 1).min(app.ci.workflows.len().saturating_sub(1));
                }
                KeyCode::Up | KeyCode::Char('k') => app.ci.sel = app.ci.sel.saturating_sub(1),
                KeyCode::Enter => match app.ci.start_selected() {
                    Ok(()) => {
                        app.message = format!(
                            "running {}…",
                            app.ci
                                .workflows
                                .get(app.ci.sel)
                                .map(|w| w.name.as_str())
                                .unwrap_or("workflow")
                        )
                    }
                    Err(e) => app.message = e,
                },
                KeyCode::Char('x') => {
                    app.message = if app.ci.cancel() {
                        "run cancelled".into()
                    } else {
                        "no live run to cancel".into()
                    };
                }
                KeyCode::Char('?') => app.help = true,
                _ => {}
            }
            return Outcome::Handled;
        }
        KeyCode::Tab => app.focus = app.focus.next(),
        KeyCode::BackTab => app.focus = app.focus.prev(),
        KeyCode::Down | KeyCode::Char('j') => app.move_sel(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_sel(-1),
        KeyCode::Char('g') | KeyCode::Home => app.sel[app.focus.index()] = 0,
        KeyCode::Char('G') | KeyCode::End => {
            app.sel[app.focus.index()] = app.panel_len(app.focus).saturating_sub(1)
        }
        KeyCode::Char('/') => app.search = Some(search::SearchModal::open(app)),
        KeyCode::Char('?') => app.help = true,
        KeyCode::Char('r') => {
            let _ = app.action_tx.send(Action::Refresh);
        }
        KeyCode::Char('n') => app.wizard = Some(Wizard::new()),
        KeyCode::Char('s') => {
            if let Some(m) = app.selected_machine() {
                if m.running {
                    app.message = format!("{} is already running", display_name(m));
                } else {
                    let id = m.id.clone();
                    match start_machine(&id) {
                        Ok(()) => app.message = format!("starting {id}…"),
                        Err(e) => app.message = format!("error: {e}"),
                    }
                }
            }
        }
        KeyCode::Char('x') => {
            if let Some(m) = app.selected_machine() {
                if m.running {
                    let id = m.id.clone();
                    let label = format!("stopping {}…", display_name(m));
                    app.send(Action::Stop(id), &label);
                } else {
                    app.message = format!("{} is not running", display_name(m));
                }
            }
        }
        KeyCode::Char('d') => {
            if let Some(m) = app.selected_machine() {
                app.confirm = Some(Confirm {
                    prompt: format!(
                        "Remove {}{}? This deletes its state.",
                        display_name(m),
                        if m.running {
                            " (running — will be killed)"
                        } else {
                            ""
                        }
                    ),
                    id: m.id.clone(),
                });
            }
        }
        KeyCode::Char('e') => {
            if let Some(m) = app.selected_machine() {
                if m.running {
                    let title = format!(" shell · {} ", display_name(m));
                    // Sized provisionally; the draw loop resizes to the modal
                    // before the first frame the child can have drawn into.
                    match term::TermPane::open(&m.id, title, 24, 80) {
                        Ok(pane) => app.term = Some(pane),
                        Err(e) => app.message = format!("error: {e}"),
                    }
                } else {
                    app.message = format!("{} is not running", display_name(m));
                }
            }
        }
        KeyCode::Char('l') => {
            if let Some(m) = app.selected_machine() {
                app.log = Some(logs::LogModal::open(m));
            }
        }
        KeyCode::Char('i') | KeyCode::Enter => {
            if let Some(m) = app.selected_machine() {
                app.settings = Some(SettingsModal {
                    id: m.id.clone(),
                    machine: display_name(m),
                    cpus: m.cpus.unwrap_or(1).to_string(),
                    mem: m.mem.unwrap_or(512).to_string(),
                    field: 0,
                    error: None,
                });
            }
        }
        KeyCode::Char('o') => {
            if let Some(m) = app.selected_machine() {
                match app.machine_url(m) {
                    Some(url) => {
                        open_url(&url);
                        app.message = format!("opened {url}");
                    }
                    None => {
                        app.message = if app.snap.domains.is_none() {
                            "domains are off — `bsdkrun domains enable` first".into()
                        } else {
                            "no URL: machine needs a --port forward".into()
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Outcome::Handled
}

fn handle_wizard_key(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    let Some(w) = &mut app.wizard else { return };
    match key.code {
        KeyCode::Esc => app.wizard = None,
        KeyCode::Tab | KeyCode::Down => w.field = (w.field + 1) % Wizard::FIELDS,
        KeyCode::BackTab | KeyCode::Up => w.field = (w.field + Wizard::FIELDS - 1) % Wizard::FIELDS,
        KeyCode::Backspace => {
            w.field_mut().pop();
        }
        KeyCode::Char(c) => {
            w.field_mut().push(c);
        }
        KeyCode::Enter => {
            if w.image.trim().is_empty() {
                w.error = Some("an OCI image is required (e.g. nginx:alpine)".into());
                return;
            }
            if !w.port.trim().is_empty() && w.port.parse::<crate::net::PortForward>().is_err() {
                w.error = Some("port must be HOST:GUEST (e.g. 18080:80)".into());
                return;
            }
            let w = app.wizard.take().unwrap();
            match launch_new_machine(&w) {
                Ok(()) => app.message = format!("booting {} — it will appear shortly", w.image),
                Err(e) => app.message = format!("error: {e}"),
            }
        }
        _ => {}
    }
}

fn handle_settings_key(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    let Some(s) = &mut app.settings else { return };
    match key.code {
        KeyCode::Esc => app.settings = None,
        KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up => s.field ^= 1,
        KeyCode::Backspace => {
            let f = if s.field == 0 {
                &mut s.cpus
            } else {
                &mut s.mem
            };
            f.pop();
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            let f = if s.field == 0 {
                &mut s.cpus
            } else {
                &mut s.mem
            };
            f.push(c);
        }
        KeyCode::Enter => {
            let (Ok(cpus), Ok(mem)) = (s.cpus.parse::<u8>(), s.mem.parse::<u32>()) else {
                s.error = Some("cpus (1-255) and mem (MiB) must be numbers".into());
                return;
            };
            let s = app.settings.take().unwrap();
            let label = format!("updating {}…", s.machine);
            app.send(
                Action::Update {
                    id: s.id,
                    cpus: Some(cpus),
                    mem: Some(mem),
                },
                &label,
            );
        }
        _ => {}
    }
}

/// A machine's display handle: its name if it has one, else the id.
pub fn display_name(m: &api::Machine) -> String {
    m.name.clone().unwrap_or_else(|| m.id.clone())
}

/// `bsdkrun start <id>` in a fresh process — the boot path forks and becomes
/// the machine, which must never happen inside the TUI's process.
fn start_machine(id: &str) -> Result<()> {
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .args(["start", id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

/// The New-machine wizard's submit: a detached `bsdkrun linux …`.
fn launch_new_machine(w: &Wizard) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.args(["linux", w.image.trim(), "-d"]);
    if !w.name.trim().is_empty() {
        cmd.args(["--name", w.name.trim()]);
    }
    if !w.port.trim().is_empty() {
        cmd.args(["--port", w.port.trim()]);
    }
    if let Ok(c) = w.cpus.trim().parse::<u8>() {
        cmd.args(["--cpus", &c.to_string()]);
    }
    if let Ok(m) = w.mem.trim().parse::<u32>() {
        cmd.args(["--mem", &m.to_string()]);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    let _ = Command::new(opener)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
