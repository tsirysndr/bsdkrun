//! The CI/CD tab: run the current repository's tangled spindle workflows in
//! microVMs, from inside the dashboard.
//!
//! The heavy lifting belongs to the embedded `bsdkrun-ci` binary — the same
//! one `bsdkrun ci` execs — spawned here as a *child* (the TUI must survive
//! it), with `--json` so the stream is spindle's LogLine records. This module
//! only parses that stream into a step timeline, exactly as the desktop app's
//! CI screen does; the two views render one format.
//!
//! Threading follows the log modal's pattern: a detached reader thread per
//! child forwards lines over an mpsc channel, and the UI thread drains it on
//! tick. Nothing here blocks the draw loop — including workflow discovery,
//! which shells out once at startup on its own thread.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;

/// The embedded runner, when this build carries one — `tui` and `ci` are
/// separate features, and a TUI without the runner should degrade to an
/// explanation, not a compile error.
fn ci_binary() -> Result<std::path::PathBuf, String> {
    #[cfg(feature = "ci")]
    {
        crate::commands::ci::ci_binary().map_err(|e| e.to_string())
    }
    #[cfg(not(feature = "ci"))]
    {
        Err("this build has no ci support (compiled without the `ci` feature)".to_string())
    }
}

/// Per-step scrollback cap. Enough to read a failure; bounded so a chatty
/// build cannot grow the TUI without limit.
const MAX_STEP_LINES: usize = 200;

/// One workflow, as `bsdkrun-ci ls --json` reports it.
#[derive(Debug, Clone)]
pub struct Workflow {
    pub name: String,
    pub engine: String,
    pub matches: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    Ok,
    Failed,
}

pub struct Step {
    pub name: String,
    pub system: bool,
    pub status: StepStatus,
    pub lines: VecDeque<String>,
    pub started: std::time::Instant,
    pub duration: Option<std::time::Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Success,
    Failed,
    Cancelled,
}

/// One run: the child doing the work, and the timeline parsed from it.
pub struct Run {
    pub workflow: String,
    pub status: RunStatus,
    pub steps: Vec<Step>,
    pub error: Option<String>,
    /// The child's pid while it lives — for cancel. The `Child` itself is
    /// owned by the waiter thread, which is the one place `wait()` is safe.
    pid: Option<u32>,
    rx: Option<mpsc::Receiver<Event>>,
}

enum Event {
    Line(String),
    Done(Option<i32>),
}

enum ListEvent {
    Workflows(Vec<Workflow>),
    Error(String),
}

/// The tab's whole state.
pub struct CiTab {
    /// The repository the TUI was launched in, if it is one.
    pub repo: Option<std::path::PathBuf>,
    pub workflows: Vec<Workflow>,
    /// Non-empty while discovery runs or when it failed; shown in the panel.
    pub note: String,
    pub sel: usize,
    /// The latest run is the one on screen; older ones stay for the session.
    pub runs: Vec<Run>,
    list_rx: Option<mpsc::Receiver<ListEvent>>,
}

impl CiTab {
    pub fn new() -> Self {
        let repo = std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim().to_string())
            });

        let (note, list_rx) = match &repo {
            None => (
                "not inside a git repository — `cd` into one and relaunch".to_string(),
                None,
            ),
            Some(root) => {
                let (tx, rx) = mpsc::channel();
                let root = root.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(list_workflows(&root));
                });
                ("loading workflows…".to_string(), Some(rx))
            }
        };

        Self {
            repo,
            workflows: Vec::new(),
            note,
            sel: 0,
            runs: Vec::new(),
            list_rx,
        }
    }

    /// Drain every pending event; returns whether anything changed.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;

        if let Some(rx) = &self.list_rx {
            if let Ok(ev) = rx.try_recv() {
                match ev {
                    ListEvent::Workflows(wfs) => {
                        self.note = if wfs.is_empty() {
                            "no .tangled/workflows in this repository".to_string()
                        } else {
                            String::new()
                        };
                        self.workflows = wfs;
                    }
                    ListEvent::Error(e) => self.note = e,
                }
                self.list_rx = None;
                changed = true;
            }
        }

        for run in &mut self.runs {
            changed |= run.drain();
        }
        changed
    }

    pub fn current_run(&self) -> Option<&Run> {
        self.runs.last()
    }

    /// Start the selected workflow. One run at a time: a second Enter while
    /// one is live is far more often a double-press than a fleet request.
    pub fn start_selected(&mut self) -> Result<(), String> {
        if self
            .runs
            .last()
            .is_some_and(|r| r.status == RunStatus::Running)
        {
            return Err("a run is already live — x cancels it".to_string());
        }
        let Some(repo) = self.repo.clone() else {
            return Err("not inside a git repository".to_string());
        };
        let Some(wf) = self.workflows.get(self.sel) else {
            return Err("no workflow selected".to_string());
        };

        let bin = ci_binary()?;
        let this = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = Command::new(bin)
            .args(["run", "--json", "-w"])
            .arg(&repo)
            .arg(&wf.name)
            .env("BSDKRUN_BIN", this)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawning bsdkrun-ci: {e}"))?;

        let (tx, rx) = mpsc::channel();
        // stdout carries the LogLine stream; stderr carries the runner's own
        // failures ("workflow failed"), which must not be lost.
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if tx.send(Event::Line(line)).is_err() {
                        return;
                    }
                }
            });
        }
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if tx.send(Event::Line(line)).is_err() {
                        return;
                    }
                }
            });
        }
        // A third thread owns the Child and its wait(): the true exit code
        // lands as an event, the UI never blocks on a child, and a killed
        // child still resolves the run. Cancel goes through the pid.
        let pid = child.id();
        {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let code = child.wait().ok().and_then(|st| st.code());
                let _ = tx.send(Event::Done(code));
            });
        }

        self.runs.push(Run {
            workflow: wf.name.clone(),
            status: RunStatus::Running,
            steps: Vec::new(),
            error: None,
            pid: Some(pid),
            rx: Some(rx),
        });
        // One screen of history is plenty for a session.
        if self.runs.len() > 10 {
            self.runs.remove(0);
        }
        Ok(())
    }

    /// Kill the live run, if any.
    pub fn cancel(&mut self) -> bool {
        if let Some(run) = self.runs.last_mut() {
            if run.status == RunStatus::Running {
                if let Some(pid) = run.pid {
                    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                }
                run.status = RunStatus::Cancelled;
                run.close_open_step(StepStatus::Failed);
                return true;
            }
        }
        false
    }
}

impl Drop for CiTab {
    fn drop(&mut self) {
        // Quitting the TUI must not leave a CI child (and its VM boot) running
        // headless — the run was interactive by construction.
        for run in &mut self.runs {
            if run.status == RunStatus::Running {
                if let Some(pid) = run.pid {
                    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                }
            }
        }
    }
}

impl Run {
    fn drain(&mut self) -> bool {
        // Taken out of self so applying a line can borrow self mutably; put
        // back unless the stream ended.
        let Some(rx) = self.rx.take() else {
            return false;
        };
        let mut changed = false;
        let mut keep = true;
        loop {
            match rx.try_recv() {
                Ok(Event::Line(line)) => {
                    self.apply(&line);
                    changed = true;
                }
                Ok(Event::Done(code)) => {
                    if self.status == RunStatus::Running {
                        let failed = code != Some(0);
                        self.status = if failed {
                            RunStatus::Failed
                        } else {
                            RunStatus::Success
                        };
                        self.close_open_step(if failed {
                            StepStatus::Failed
                        } else {
                            StepStatus::Ok
                        });
                    }
                    keep = false;
                    self.pid = None;
                    changed = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    keep = false;
                    break;
                }
            }
        }
        if keep {
            self.rx = Some(rx);
        }
        changed
    }

    /// One stream line: a spindle LogLine when it parses, runner chatter (its
    /// stderr, git noise) attached to the open step when it does not — the
    /// same tolerance the desktop CI screen applies, so nothing is dropped.
    fn apply(&mut self, raw: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
            if !raw.trim().is_empty() {
                self.push_line(raw.to_string());
            }
            return;
        };
        match v["kind"].as_str() {
            Some("control") => {
                let name = v["content"].as_str().unwrap_or("step").to_string();
                match v["step_status"].as_str() {
                    Some("start") => self.steps.push(Step {
                        name,
                        system: v["step_kind"].as_i64() == Some(0),
                        status: StepStatus::Running,
                        lines: VecDeque::new(),
                        started: std::time::Instant::now(),
                        duration: None,
                    }),
                    _ => self.close_open_step(StepStatus::Ok),
                }
            }
            Some("data") => {
                if let Some(content) = v["content"].as_str() {
                    self.push_line(content.to_string());
                }
            }
            _ => self.push_line(raw.to_string()),
        }
    }

    fn push_line(&mut self, line: String) {
        if let Some(step) = self.steps.last_mut() {
            step.lines.push_back(line);
            while step.lines.len() > MAX_STEP_LINES {
                step.lines.pop_front();
            }
        }
    }

    fn close_open_step(&mut self, status: StepStatus) {
        if let Some(step) = self.steps.last_mut() {
            if step.status == StepStatus::Running {
                step.status = status;
                step.duration = Some(step.started.elapsed());
            }
        }
    }
}

fn list_workflows(root: &std::path::Path) -> ListEvent {
    let bin = match ci_binary() {
        Ok(b) => b,
        Err(e) => return ListEvent::Error(e),
    };
    let out = Command::new(bin)
        .args(["ls", "--json", "-w"])
        .arg(root)
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            return ListEvent::Error(
                String::from_utf8_lossy(&o.stderr)
                    .lines()
                    .last()
                    .unwrap_or("listing workflows failed")
                    .to_string(),
            )
        }
        Err(e) => return ListEvent::Error(format!("running bsdkrun-ci: {e}")),
    };
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    ListEvent::Workflows(
        rows.iter()
            .map(|r| Workflow {
                name: r["name"].as_str().unwrap_or("?").to_string(),
                engine: r["engine"].as_str().unwrap_or("?").to_string(),
                matches: r["matches"].as_bool().unwrap_or(false),
            })
            .collect(),
    )
}
