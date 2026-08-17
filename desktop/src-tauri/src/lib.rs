//! bsdkrun Desktop — Tauri backend.
//!
//! A thin, async bridge over the `bsdkrun` CLI: it resolves the binary, runs
//! the JSON-emitting subcommands, launches machines, streams logs, and hosts
//! interactive PTY sessions. All heavy lifting lives in `bsdkrun` and `term`.

mod bsdkrun;
mod menu;
pub mod remote;
mod system;
pub mod target;
mod term;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};

use bsdkrun::{BkError, Flavor, Image, Machine, Network, Snapshot, VersionEntry, Volume};
use target::Target;
use term::{LogStreams, Terminals};

/// Persisted app settings.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Where bsdkrun lives: a path to the binary on this machine, OR a
    /// `bsdkrund` gRPC URL (`grpc://host:50051`) to drive a remote host.
    /// Empty ⇒ auto-resolve a local binary.
    #[serde(default)]
    pub binary_path: String,
    /// Access token, used only when `binary_path` is a daemon URL. Falls back
    /// to `$BSDKRUN_TOKEN`.
    #[serde(default)]
    pub token: String,
    /// Override for bsdkrun's cache directory (images/kernels/agent/flavor
    /// builds), applied via `$BSDKRUN_CACHE`; empty ⇒ the CLI default.
    #[serde(default)]
    pub cache_path: String,
}

/// The default cache directory the CLI uses when `$BSDKRUN_CACHE` is unset:
/// `$XDG_CACHE_HOME/bsdkrun`, else `$HOME/.cache/bsdkrun`.
fn default_cache_path() -> String {
    if let Ok(x) = std::env::var("XDG_CACHE_HOME") {
        if !x.is_empty() {
            return format!("{x}/bsdkrun");
        }
    }
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => format!("{h}/.cache/bsdkrun"),
        _ => "~/.cache/bsdkrun".to_string(),
    }
}

/// Apply settings that affect child `bsdkrun` processes to this process's env,
/// so every spawned CLI invocation inherits them. Currently the cache dir.
fn apply_env(s: &Settings) {
    if s.cache_path.is_empty() {
        std::env::remove_var("BSDKRUN_CACHE");
    } else {
        std::env::set_var("BSDKRUN_CACHE", &s.cache_path);
    }
}

struct AppState {
    settings: Mutex<Settings>,
    sys: Mutex<sysinfo::System>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            settings: Mutex::new(Settings::default()),
            sys: Mutex::new(sysinfo::System::new()),
        }
    }
}

impl AppState {
    /// The backend to drive: a local binary or a remote daemon.
    ///
    /// Still called `binary()` because every call site treats it as "the thing
    /// I run commands against", which is exactly what it still is.
    fn binary(&self) -> Result<Target, BkError> {
        let (target, token) = {
            let s = self.settings.lock().unwrap();
            (s.binary_path.clone(), s.token.clone())
        };
        target::resolve(&target, &token)
    }
}

fn settings_path(app: &tauri::AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    dir.join("settings.json")
}

fn load_settings(app: &tauri::AppHandle) -> Settings {
    let p = settings_path(app);
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(app: &tauri::AppHandle, s: &Settings) {
    let p = settings_path(app);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(p, json);
    }
}

// ---- settings + probe ------------------------------------------------------

#[derive(Serialize)]
struct ProbeResult {
    ok: bool,
    message: String,
    binary: Option<String>,
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn set_settings(
    app: tauri::AppHandle,
    state: State<AppState>,
    binary_path: String,
    cache_path: String,
    token: String,
) -> Settings {
    let mut s = state.settings.lock().unwrap();
    s.binary_path = binary_path;
    s.cache_path = cache_path;
    s.token = token;
    apply_env(&s);
    save_settings(&app, &s);
    s.clone()
}

/// The cache directory bsdkrun uses right now: the configured override, else the
/// CLI default. Shown as the placeholder/current value in Settings.
#[tauri::command]
fn default_cache() -> String {
    default_cache_path()
}

/// Update the menu-bar tray status line (called by the frontend on probe).
#[tauri::command]
fn set_tray_status(app: tauri::AppHandle, ok: bool, detail: String) {
    tray::set_status(&app, ok, &detail);
}

/// Resolve + `bsdkrun probe` — reports whether libkrun links and the hypervisor
/// is reachable, so the UI can show a connection indicator.
#[tauri::command]
async fn probe(state: State<'_, AppState>) -> Result<ProbeResult, BkError> {
    let bin = match state.binary() {
        Ok(b) => b,
        Err(e) => {
            return Ok(ProbeResult {
                ok: false,
                message: e.to_string(),
                binary: None,
            })
        }
    };
    match bsdkrun::run(&bin, &["probe"]).await {
        Ok(out) => Ok(ProbeResult {
            ok: true,
            message: out.trim().to_string(),
            binary: Some(bin.describe()),
        }),
        Err(e) => Ok(ProbeResult {
            ok: false,
            message: e.to_string(),
            binary: Some(bin.describe()),
        }),
    }
}

// ---- listings --------------------------------------------------------------

#[tauri::command]
async fn list_machines(state: State<'_, AppState>, all: bool) -> Result<Vec<Machine>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_machines(&bin, all).await
}

#[tauri::command]
async fn list_images(state: State<'_, AppState>) -> Result<Vec<Image>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_images(&bin).await
}

#[tauri::command]
async fn list_volumes(state: State<'_, AppState>) -> Result<Vec<Volume>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_volumes(&bin).await
}

#[tauri::command]
async fn list_versions(
    state: State<'_, AppState>,
    os: String,
) -> Result<Vec<VersionEntry>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_versions(&bin, &os).await
}

#[tauri::command]
async fn list_flavors(state: State<'_, AppState>) -> Result<Vec<Flavor>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_flavors(&bin).await
}

// ---- snapshots -------------------------------------------------------------
//
// A snapshot is a copy-on-write clone of a machine's disk state — instant, and
// free until the two sides diverge — so the UI can offer it as casually as it
// offers "stop".

#[tauri::command]
async fn list_snapshots(
    state: State<'_, AppState>,
    machine: Option<String>,
) -> Result<Vec<Snapshot>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_snapshots(&bin, machine.as_deref()).await
}

/// Capture a machine's disk state — `bsdkrun snapshot <id> [name]`. Returns the
/// snapshot's name (generated when none was given).
///
/// A BSD guest is powered off first: a mounted UFS cannot be cloned
/// consistently, so the machine is left stopped and the UI should say so.
#[tauri::command]
async fn snapshot_machine(
    state: State<'_, AppState>,
    id: String,
    name: Option<String>,
    description: String,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let mut args: Vec<String> = vec!["snapshot".into(), id];
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        args.push(n);
    }
    if !description.is_empty() {
        args.push("--description".into());
        args.push(description);
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = bsdkrun::run(&bin, &refs).await?;
    Ok(out.trim().to_string())
}

/// Remove a snapshot and its data — `bsdkrun snapshot rm <name>`.
#[tauri::command]
async fn remove_snapshot(state: State<'_, AppState>, name: String) -> Result<(), BkError> {
    let bin = state.binary()?;
    bsdkrun::run(&bin, &["snapshot", "rm", &name]).await?;
    Ok(())
}

/// Put a machine back to a snapshot — `bsdkrun restore -f <id> <snapshot>`.
/// Always `-f`: the UI asked the user already, and a running machine holds the
/// files being replaced. The machine is left stopped.
#[tauri::command]
async fn restore_machine(
    state: State<'_, AppState>,
    id: String,
    snapshot: String,
    backup: bool,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let mut args: Vec<&str> = vec!["restore", "-f", &id, &snapshot];
    if !backup {
        args.push("--no-backup");
    }
    let out = bsdkrun::run(&bin, &args).await?;
    Ok(out.trim().to_string())
}

/// Restore a machine to its most recent snapshot — `bsdkrun rollback -f <id>`.
#[tauri::command]
async fn rollback_machine(
    state: State<'_, AppState>,
    id: String,
    backup: bool,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let mut args: Vec<&str> = vec!["rollback", "-f", &id];
    if !backup {
        args.push("--no-backup");
    }
    let out = bsdkrun::run(&bin, &args).await?;
    Ok(out.trim().to_string())
}

/// Boot a new machine from a snapshot — `bsdkrun branch -d <snapshot>`.
/// Returns the new machine's id.
#[tauri::command]
async fn branch_snapshot(
    state: State<'_, AppState>,
    snapshot: String,
    name: Option<String>,
    ports: Vec<String>,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let mut args: Vec<String> = vec!["branch".into(), "-d".into(), snapshot];
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        args.push("--name".into());
        args.push(n);
    }
    for p in ports.iter().filter(|p| !p.is_empty()) {
        args.push("--port".into());
        args.push(p.clone());
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    bsdkrun::run_detached(&bin, &refs).await
}

// ---- global networks -------------------------------------------------------

#[tauri::command]
async fn list_networks(state: State<'_, AppState>) -> Result<Vec<Network>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_networks(&bin).await
}

/// Create a global network — `bsdkrun network create <name>`.
#[tauri::command]
async fn create_network(state: State<'_, AppState>, name: String) -> Result<(), BkError> {
    let bin = state.binary()?;
    bsdkrun::run(&bin, &["network", "create", &name]).await?;
    Ok(())
}

/// Remove a global network — `bsdkrun network rm [-f] <name>`.
#[tauri::command]
async fn remove_network(
    state: State<'_, AppState>,
    name: String,
    force: bool,
) -> Result<(), BkError> {
    let bin = state.binary()?;
    let mut args = vec!["network", "rm"];
    if force {
        args.push("-f");
    }
    args.push(&name);
    bsdkrun::run(&bin, &args).await?;
    Ok(())
}

/// Refresh members' /etc/hosts so peers resolve by name (esp. NetBSD) —
/// `bsdkrun network sync <name>`.
#[tauri::command]
async fn sync_network(state: State<'_, AppState>, name: String) -> Result<(), BkError> {
    let bin = state.binary()?;
    bsdkrun::run(&bin, &["network", "sync", &name]).await?;
    Ok(())
}

// ---- flavors ---------------------------------------------------------------

/// Boot a new machine from a flavor (catalog / user / snapshot), detached.
/// Returns the new machine id. Extra `ports`/`volume` layer on the flavor's
/// defaults, mirroring `bsdkrun flavor run -d <name> [--port …] [-v …]`.
#[tauri::command]
async fn run_flavor(
    state: State<'_, AppState>,
    name: String,
    ports: Vec<String>,
    volume: Option<String>,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let mut args: Vec<String> = vec!["flavor".into(), "run".into(), "-d".into(), name];
    for p in &ports {
        if !p.is_empty() {
            args.push("--port".into());
            args.push(p.clone());
        }
    }
    if let Some(v) = volume.as_deref().filter(|v| !v.is_empty()) {
        args.push("-v".into());
        args.push(v.into());
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    bsdkrun::run_detached(&bin, &refs).await
}

/// Snapshot a machine's current state into a named flavor — `bsdkrun commit`.
#[tauri::command]
async fn commit_machine(
    state: State<'_, AppState>,
    id: String,
    name: String,
    description: String,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let out = bsdkrun::run(&bin, &["commit", &id, &name, "--description", &description]).await?;
    Ok(out.trim().to_string())
}

/// Define (or update) a custom user flavor — `bsdkrun flavor add …`. Writes it
/// to the user's `flavors.toml` so it shows up alongside the catalog.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn create_flavor(
    state: State<'_, AppState>,
    name: String,
    base: String,
    category: String,
    description: String,
    ports: Vec<String>,
    env: Vec<String>,
    nix: Vec<String>,
    provision: Vec<String>,
) -> Result<(), BkError> {
    let bin = state.binary()?;
    let mut args: Vec<String> = vec!["flavor".into(), "add".into(), name, "--base".into(), base];
    if !category.is_empty() {
        args.push("--category".into());
        args.push(category);
    }
    if !description.is_empty() {
        args.push("--description".into());
        args.push(description);
    }
    for (flag, list) in [
        ("--port", ports),
        ("--env", env),
        ("--nix", nix),
        ("--provision", provision),
    ] {
        for v in list {
            if !v.trim().is_empty() {
                args.push(flag.into());
                args.push(v);
            }
        }
    }
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    bsdkrun::run(&bin, &refs).await?;
    Ok(())
}

/// Remove a saved snapshot or user flavor — `bsdkrun flavor rm [-f] <name>`.
#[tauri::command]
async fn remove_flavor(
    state: State<'_, AppState>,
    name: String,
    force: bool,
) -> Result<(), BkError> {
    let bin = state.binary()?;
    let mut args = vec!["flavor", "rm"];
    if force {
        args.push("-f");
    }
    args.push(&name);
    bsdkrun::run(&bin, &args).await?;
    Ok(())
}

#[derive(Clone, Serialize)]
struct FlavorLog {
    launch_id: String,
    line: String,
}

#[derive(Clone, Serialize)]
struct FlavorDone {
    launch_id: String,
    id: Option<String>,
    error: Option<String>,
}

/// Strip ANSI escape sequences and trim a trailing curl progress meter so a
/// streamed launch/build line reads cleanly in the UI.
pub(crate) fn sanitize_log(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI sequence: ESC '[' … final-letter — drop it entirely.
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    // curl's progress meter (…#=#=#… / carriage returns) — cut it off.
    if let Some(pos) = out.find(['#', '\r']) {
        out.truncate(pos);
    }
    out.trim_end().to_string()
}

/// Spawn a `bsdkrun` invocation and STREAM its combined output to the webview as
/// `flavor://log` { launch_id, line } lines, ending with `flavor://done`
/// { launch_id, id, error }. `capture_id` true treats a lone 12-hex-char stdout
/// token as the resulting machine id (for `run`); false streams everything (for
/// `build`). Returns immediately; the work runs on a background task.
fn stream_bsdkrun(
    app: tauri::AppHandle,
    bin: Target,
    args: Vec<String>,
    launch_id: String,
    capture_id: bool,
    timeout_msg: &'static str,
) {
    // A remote target streams the same argv through the daemon, which reports
    // stdout, stderr and the exit code exactly as a local child does.
    let bin = match bin {
        Target::Local(p) => p,
        Target::Remote { endpoint, token } => {
            stream_remote(app, endpoint, token, args, launch_id, capture_id);
            return;
        }
    };
    tokio::spawn(async move {
        use std::process::Stdio;
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.env("PATH", bsdkrun::augmented_path());
        cmd.args(&args);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        // No kill_on_drop: a detached VM grandchild must never be signalled.
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = app.emit(
                    "flavor://done",
                    FlavorDone {
                        launch_id,
                        id: None,
                        error: Some(e.to_string()),
                    },
                );
                return;
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        // stderr → progress lines (pull %, provisioning, boot logs).
        let app_err = app.clone();
        let lid_err = launch_id.clone();
        let err_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let s = sanitize_log(&l);
                if !s.is_empty() {
                    let _ = app_err.emit(
                        "flavor://log",
                        FlavorLog {
                            launch_id: lid_err.clone(),
                            line: s,
                        },
                    );
                }
            }
        });

        // stdout → the machine id (for `run`) + any stdout progress.
        let app_out = app.clone();
        let lid_out = launch_id.clone();
        let id_slot = std::sync::Arc::new(std::sync::Mutex::new(Option::<String>::None));
        let id_slot2 = id_slot.clone();
        let out_task = tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let t = l.trim().to_string();
                if t.is_empty() {
                    continue;
                }
                if capture_id && t.len() == 12 && t.chars().all(|c| c.is_ascii_hexdigit()) {
                    *id_slot2.lock().unwrap() = Some(t);
                } else {
                    let _ = app_out.emit(
                        "flavor://log",
                        FlavorLog {
                            launch_id: lid_out.clone(),
                            line: t,
                        },
                    );
                }
            }
        });

        let status = tokio::time::timeout(std::time::Duration::from_secs(1800), child.wait()).await;
        // A VM grandchild holds the pipes open, so the drain tasks never EOF.
        err_task.abort();
        out_task.abort();

        let id = id_slot.lock().unwrap().clone();
        let done = match status {
            Ok(Ok(st)) if st.success() => FlavorDone {
                launch_id,
                id,
                error: None,
            },
            Ok(Ok(st)) => FlavorDone {
                launch_id,
                id,
                error: Some(format!("exited with status {}", st.code().unwrap_or(-1))),
            },
            Ok(Err(e)) => FlavorDone {
                launch_id,
                id,
                error: Some(e.to_string()),
            },
            Err(_) => FlavorDone {
                launch_id,
                id,
                error: Some(timeout_msg.into()),
            },
        };
        let _ = app.emit("flavor://done", done);
    });
}

/// Launch a flavor and STREAM its progress (pull → provisioning build → boot),
/// ending with the new machine id. See [`stream_bsdkrun`].
#[tauri::command]
async fn launch_flavor(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    launch_id: String,
    name: String,
    ports: Vec<String>,
    volume: Option<String>,
    repo: Option<String>,
) -> Result<(), String> {
    let bin = state.binary().map_err(|e| e.to_string())?;
    let mut args: Vec<String> = vec!["flavor".into(), "run".into(), "-d".into(), name];
    for p in ports {
        if !p.is_empty() {
            args.push("--port".into());
            args.push(p);
        }
    }
    if let Some(v) = volume.filter(|v| !v.is_empty()) {
        args.push("-v".into());
        args.push(v);
    }
    if let Some(r) = repo.filter(|r| !r.trim().is_empty()) {
        args.push("--repo".into());
        args.push(r);
    }
    stream_bsdkrun(
        app,
        bin,
        args,
        launch_id,
        true,
        "timed out launching flavor",
    );
    Ok(())
}

/// Pre-build a flavor's provisioned rootfs into the cache and STREAM the build
/// logs — used right after saving a custom flavor. See [`stream_bsdkrun`].
#[tauri::command]
async fn build_flavor(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    launch_id: String,
    name: String,
) -> Result<(), String> {
    let bin = state.binary().map_err(|e| e.to_string())?;
    let args: Vec<String> = vec!["flavor".into(), "build".into(), name];
    stream_bsdkrun(
        app,
        bin,
        args,
        launch_id,
        false,
        "timed out building flavor",
    );
    Ok(())
}

/// Host CPU% + RAM and the real on-disk size of all microVMs (status bar).
#[tauri::command]
async fn system_stats(state: State<'_, AppState>) -> Result<system::SystemStats, String> {
    use sysinfo::{MemoryRefreshKind, RefreshKind};
    let (cpu, mem_used, mem_total) = {
        let mut sys = state.sys.lock().unwrap();
        sys.refresh_cpu_usage();
        sys.refresh_specifics(RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()));
        (
            sys.global_cpu_usage(),
            sys.used_memory(),
            sys.total_memory(),
        )
    };
    let (vm_disk, vm_count) = tokio::task::spawn_blocking(system::vm_disk_usage)
        .await
        .unwrap_or((0, 0));
    Ok(system::SystemStats {
        cpu,
        mem_used,
        mem_total,
        vm_disk,
        vm_count,
    })
}

// ---- lifecycle -------------------------------------------------------------

/// Everything the Run dialog can specify. Mirrors the CLI's flags; the GUI
/// always runs detached (`-d`).
#[derive(Debug, Deserialize)]
pub struct RunSpec {
    pub kind: String, // "linux" | "freebsd" | "netbsd" | "unikraft" | "nanos" | "osv" | "solo5"
    #[serde(default)]
    pub image: Option<String>,
    /// Unikraft only: a `kraft` project directory (the image is found under its
    /// `.unikraft/build/`) or a built unikernel image.
    #[serde(default)]
    pub path: Option<String>,
    /// Unikraft/Nanos/OSv: the kernel command line. For OSv this is the
    /// application to run and its arguments, e.g. "/hello.so".
    #[serde(default)]
    pub cmdline: Option<String>,
    /// Nanos only: kernel override (Linux hosts).
    #[serde(default)]
    pub kernel: Option<String>,
    /// Nanos/OSv: boot the image in place instead of a CoW clone.
    #[serde(default)]
    pub persist: bool,
    /// OSv only: root disk (raw). Required on x86_64, where the loader ELF
    /// carries no filesystem.
    #[serde(default)]
    pub disk: Option<String>,
    /// OSv only: interrupt controller, "v2" (default) or "v3". aarch64 only.
    #[serde(default)]
    pub gic: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub cpus: Option<u32>,
    #[serde(default)]
    pub mem: Option<u32>,
    #[serde(default)]
    pub volume: Option<String>,
    #[serde(default)]
    pub no_net: bool,
    #[serde(default)]
    pub initramfs: bool,
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub mounts: Vec<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    /// Extra virtio-blk disks to attach (Linux and BSD), `PATH[:ro]`.
    #[serde(default)]
    pub attach_disks: Vec<String>,
    /// Grow the guest root disk to this size before boot (BSD only), e.g. `8G`.
    #[serde(default)]
    pub disk_size: Option<String>,
    /// Clone a git repo into the guest after boot and `cd` into it on shell open
    /// (Linux guests).
    #[serde(default)]
    pub repo: Option<String>,
    /// Join a global network (shared subnet + internal DNS).
    #[serde(default)]
    pub network: Option<String>,
    /// Friendly name for the machine (its DNS name on a network).
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    /// Solo5 only: backing files for declared block devices, `NAME=FILE`.
    #[serde(default)]
    pub blocks: Vec<String>,
    /// Solo5 only: arguments passed to the unikernel itself. Kept separate
    /// from `command` — that means "run via the guest agent" elsewhere, and a
    /// Solo5 guest has no agent.
    #[serde(default)]
    pub args: Vec<String>,
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty())
}

fn build_run_args(spec: &RunSpec) -> Result<Vec<String>, BkError> {
    let mut a: Vec<String> = vec![spec.kind.clone(), "-d".into()];
    let bsd = spec.kind == "freebsd" || spec.kind == "netbsd";
    // A unikernel is the application linked into the kernel: no disk to
    // persist and no agent to run a repo clone or a command through.
    let unikraft = spec.kind == "unikraft";
    let nanos = spec.kind == "nanos";
    let osv = spec.kind == "osv";
    // Solo5 runs under its own tender rather than libkrun, but takes the same
    // shape of argv — and it is one more agent-less unikernel to the UI.
    let solo5 = spec.kind == "solo5";
    // Every unikernel kind: no volume/repo/command, since none has an agent.
    let unikernel = unikraft || nanos || osv || solo5;

    if bsd {
        if let Some(v) = nonempty(&spec.version) {
            a.push("--version".into());
            a.push(v.into());
        }
    }
    if let Some(c) = spec.cpus {
        a.push("--cpus".into());
        a.push(c.to_string());
    }
    if let Some(m) = spec.mem {
        a.push("--mem".into());
        a.push(m.to_string());
    }
    if !unikernel {
        if let Some(v) = nonempty(&spec.volume) {
            a.push("-v".into());
            a.push(v.into());
        }
    }
    if spec.no_net {
        a.push("--no-net".into());
    }
    for p in &spec.ports {
        if !p.is_empty() {
            a.push("--port".into());
            a.push(p.clone());
        }
    }
    // `--repo` clones a git repo after boot (both Linux and BSD support it).
    if !unikernel {
        if let Some(r) = nonempty(&spec.repo) {
            a.push("--repo".into());
            a.push(r.into());
        }
    }
    // Global network membership + friendly/DNS name.
    if let Some(n) = nonempty(&spec.network) {
        a.push("--network".into());
        a.push(n.into());
    }
    if let Some(n) = nonempty(&spec.name) {
        a.push("--name".into());
        a.push(n.into());
    }
    if spec.kind == "linux" {
        if spec.initramfs {
            a.push("--initramfs".into());
        }
        if let Some(e) = nonempty(&spec.entrypoint) {
            a.push("--entrypoint".into());
            a.push(e.into());
        }
        for m in &spec.mounts {
            if !m.is_empty() {
                a.push("--mount".into());
                a.push(m.clone());
            }
        }
        for d in &spec.attach_disks {
            if !d.is_empty() {
                a.push("--attach-disk".into());
                a.push(d.clone());
            }
        }
        let image = nonempty(&spec.image)
            .ok_or_else(|| BkError::Parse("an image reference is required for linux".into()))?;
        a.push(image.into());
    } else if unikraft {
        if let Some(c) = nonempty(&spec.cmdline) {
            a.push("--cmdline".into());
            a.push(c.into());
        }
        // Volumes are the one disk-shaped option a unikernel takes: virtio-fs
        // shares, which need neither a disk nor an agent.
        for m in &spec.mounts {
            if !m.is_empty() {
                a.push("--mount".into());
                a.push(m.clone());
            }
        }
        // The path is positional and last; default to "." like the CLI.
        a.push(nonempty(&spec.path).unwrap_or(".").into());
    } else if nanos {
        if let Some(k) = nonempty(&spec.kernel) {
            a.push("--kernel".into());
            a.push(k.into());
        }
        if let Some(c) = nonempty(&spec.cmdline) {
            a.push("--cmdline".into());
            a.push(c.into());
        }
        if spec.persist {
            a.push("--persist".into());
        }
        // The image is positional and last.
        let image = nonempty(&spec.image)
            .ok_or_else(|| BkError::Parse("an image is required for nanos".into()))?;
        a.push(image.into());
    } else if osv {
        if let Some(c) = nonempty(&spec.cmdline) {
            a.push("--cmdline".into());
            a.push(c.into());
        }
        // OSv has a root filesystem, unlike the other unikernels: an aarch64
        // loader.img carries it, an x86_64 loader ELF needs one attached.
        if let Some(d) = nonempty(&spec.disk) {
            a.push("--disk".into());
            a.push(d.into());
        }
        if let Some(g) = nonempty(&spec.gic) {
            a.push("--gic".into());
            a.push(g.into());
        }
        if spec.persist {
            a.push("--persist".into());
        }
        // The image is positional and last.
        let image = nonempty(&spec.image)
            .ok_or_else(|| BkError::Parse("an image is required for osv".into()))?;
        a.push(image.into());
    } else if solo5 {
        // The unikernel declares its own devices (MFT1 note); the host only
        // supplies what backs a declared block device.
        for b in &spec.blocks {
            if !b.is_empty() {
                a.push("--block".into());
                a.push(b.clone());
            }
        }
        // The path is positional; default to "." like the CLI.
        a.push(nonempty(&spec.path).unwrap_or(".").into());
        // Guest arguments go last, behind a literal "--": MirageOS options
        // look exactly like bsdkrun's own flags (e.g. --ipv4=…), so without
        // the separator the CLI would eat them. Only emit it when needed.
        if !spec.args.is_empty() {
            a.push("--".into());
            a.extend(spec.args.iter().cloned());
        }
    } else {
        // BSD: grow the root disk + attach extra virtio-blk disks.
        if let Some(sz) = nonempty(&spec.disk_size) {
            a.push("--disk-size".into());
            a.push(sz.into());
        }
        for d in &spec.attach_disks {
            if !d.is_empty() {
                a.push("--attach-disk".into());
                a.push(d.clone());
            }
        }
    }
    if !unikernel && !spec.command.is_empty() {
        a.push("--".into());
        a.extend(spec.command.iter().cloned());
    }
    Ok(a)
}

/// Launch a machine detached; returns its new id.
#[tauri::command]
async fn run_machine(state: State<'_, AppState>, spec: RunSpec) -> Result<String, BkError> {
    let bin = state.binary()?;
    let args = build_run_args(&spec)?;
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    bsdkrun::run_detached(&bin, &refs).await
}

/// Launch a machine and STREAM its progress (OCI pull / BSD image + kernel
/// download / boot), ending with the new machine id. Same event channel as
/// `launch_flavor`, so the launch-progress modal shows live download logs
/// instead of a silent spinner. See [`stream_bsdkrun`].
#[tauri::command]
async fn launch_machine(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    launch_id: String,
    spec: RunSpec,
) -> Result<(), String> {
    let bin = state.binary().map_err(|e| e.to_string())?;
    let args = build_run_args(&spec).map_err(|e| e.to_string())?;
    stream_bsdkrun(
        app,
        bin,
        args,
        launch_id,
        true,
        "timed out launching machine",
    );
    Ok(())
}

/// Update a machine's recorded vCPU / memory — `bsdkrun update <id> --cpus --mem`.
/// Applies on the next start (libkrun fixes VM resources at boot).
#[tauri::command]
async fn update_machine(
    state: State<'_, AppState>,
    id: String,
    cpus: u32,
    mem: u32,
) -> Result<(), BkError> {
    let bin = state.binary()?;
    let (c, m) = (cpus.to_string(), mem.to_string());
    bsdkrun::run(&bin, &["update", &id, "--cpus", &c, "--mem", &m]).await?;
    Ok(())
}

/// Edit a machine's global-network membership (join/switch/leave). `network:
/// None` detaches it back to the isolated default. Applies on next start.
#[tauri::command]
async fn update_machine_network(
    state: State<'_, AppState>,
    id: String,
    network: Option<String>,
) -> Result<(), BkError> {
    let bin = state.binary()?;
    match network {
        Some(net) if !net.is_empty() => {
            bsdkrun::run(&bin, &["network", "connect", &id, &net]).await?;
        }
        _ => {
            bsdkrun::run(&bin, &["network", "disconnect", &id]).await?;
        }
    }
    Ok(())
}

#[tauri::command]
async fn stop_machine(state: State<'_, AppState>, id: String) -> Result<(), BkError> {
    let bin = state.binary()?;
    bsdkrun::run(&bin, &["stop", &id]).await?;
    Ok(())
}

/// Restart a stopped machine in place (same id) — `bsdkrun start <id>`.
#[tauri::command]
async fn restart_machine(state: State<'_, AppState>, id: String) -> Result<String, BkError> {
    let bin = state.binary()?;
    bsdkrun::run_detached(&bin, &["start", &id]).await
}

#[tauri::command]
async fn remove_machine(
    state: State<'_, AppState>,
    id: String,
    force: bool,
) -> Result<(), BkError> {
    let bin = state.binary()?;
    let mut args = vec!["rm"];
    if force {
        args.push("-f");
    }
    args.push(&id);
    bsdkrun::run(&bin, &args).await?;
    Ok(())
}

#[tauri::command]
async fn remove_volume(
    state: State<'_, AppState>,
    name: String,
    force: bool,
) -> Result<(), BkError> {
    let bin = state.binary()?;
    let mut args = vec!["volume", "rm"];
    if force {
        args.push("-f");
    }
    args.push(&name);
    bsdkrun::run(&bin, &args).await?;
    Ok(())
}

/// Run an SSH management action inside a guest: `bsdkrun ssh <id> <args…>`
/// (e.g. `setup`, `status`, `add-key --key …`). Returns the command output.
#[tauri::command]
async fn ssh_action(
    state: State<'_, AppState>,
    id: String,
    args: Vec<String>,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let mut a = vec!["ssh".to_string(), id];
    a.extend(args);
    let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
    bsdkrun::run_output(&bin, &refs).await
}

/// Update the in-guest agent to the current release: `bsdkrun agent update <id>`.
/// Fixes BSD images whose baked-in agent predates ssh/tailscale.
#[tauri::command]
async fn update_agent(state: State<'_, AppState>, id: String) -> Result<String, BkError> {
    let bin = state.binary()?;
    bsdkrun::run_output(&bin, &["agent", "update", &id]).await
}

/// Run a Tailscale action inside a guest: `bsdkrun tailscale <id> <args…>`
/// (e.g. `setup --authkey …`, `status`, `install`, `start`).
#[tauri::command]
async fn tailscale_action(
    state: State<'_, AppState>,
    id: String,
    args: Vec<String>,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    let mut a = vec!["tailscale".to_string(), id];
    a.extend(args);
    let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
    bsdkrun::run_output(&bin, &refs).await
}

/// One-shot console log (not `-f`). `boot` shows bsdkrun's own boot log instead.
#[tauri::command]
async fn machine_logs(
    state: State<'_, AppState>,
    id: String,
    boot: bool,
) -> Result<String, BkError> {
    let bin = state.binary()?;
    if boot {
        bsdkrun::run(&bin, &["logs", "--boot", &id]).await
    } else {
        bsdkrun::run(&bin, &["logs", &id]).await
    }
}

// ---- live logs -------------------------------------------------------------

#[tauri::command]
async fn start_log_stream(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    streams: State<'_, LogStreams>,
    id: String,
) -> Result<(), String> {
    let bin = state.binary().map_err(|e| e.to_string())?;
    term::start_logs(&app, &streams, &bin, &id)
}

#[tauri::command]
async fn stop_log_stream(streams: State<'_, LogStreams>, id: String) -> Result<(), String> {
    term::stop_logs(&streams, &id);
    Ok(())
}

// ---- interactive terminal --------------------------------------------------

#[tauri::command]
async fn term_open(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    terminals: State<'_, Terminals>,
    id: String,
    command: Vec<String>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    let bin = state.binary().map_err(|e| e.to_string())?;
    // No explicit command: probe the guest for a shell that actually exists
    // (nix images often lack /bin/sh), then open a PTY on it.
    let command = if command.is_empty() {
        bsdkrun::resolve_guest_shell(&bin, &id).await
    } else {
        command
    };
    term::open(
        &app,
        &terminals,
        &bin,
        &id,
        command,
        rows.max(1),
        cols.max(1),
    )
}

/// Open an interactive terminal on the HOST (the machine running this app),
/// not a guest. Used by the bottom panel when no machine terminal is open.
#[tauri::command]
async fn term_open_host(
    app: tauri::AppHandle,
    terminals: State<'_, Terminals>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    term::open_host(&app, &terminals, rows.max(1), cols.max(1))
}

#[tauri::command]
async fn term_write(
    terminals: State<'_, Terminals>,
    session: String,
    data: String,
) -> Result<(), String> {
    term::write(&terminals, &session, &data)
}

#[tauri::command]
async fn term_resize(
    terminals: State<'_, Terminals>,
    session: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    term::resize(&terminals, &session, rows.max(1), cols.max(1))
}

#[tauri::command]
async fn term_close(terminals: State<'_, Terminals>, session: String) -> Result<(), String> {
    term::close(&terminals, &session)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .manage(Terminals::default())
        .manage(LogStreams::default())
        .manage(tray::TrayStatus::default())
        .setup(|app| {
            // Load persisted settings into managed state + apply env (cache dir).
            let loaded = load_settings(app.handle());
            apply_env(&loaded);
            *app.state::<AppState>().settings.lock().unwrap() = loaded;
            // Native application menu (macOS top bar + Windows/Linux window menu).
            menu::install(app.handle())?;
            // macOS-style system tray (Docker-Desktop-like).
            tray::install(app.handle())?;
            // Close the window to the tray instead of quitting (app stays alive
            // in the menu bar; use the tray/Cmd-Q to actually quit).
            if let Some(win) = app.get_webview_window("main") {
                let w = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            default_cache,
            set_tray_status,
            probe,
            list_machines,
            list_images,
            list_volumes,
            list_versions,
            list_flavors,
            list_snapshots,
            snapshot_machine,
            remove_snapshot,
            restore_machine,
            rollback_machine,
            branch_snapshot,
            list_networks,
            create_network,
            remove_network,
            sync_network,
            run_flavor,
            launch_flavor,
            build_flavor,
            create_flavor,
            commit_machine,
            remove_flavor,
            system_stats,
            run_machine,
            launch_machine,
            update_machine,
            update_machine_network,
            stop_machine,
            restart_machine,
            remove_machine,
            remove_volume,
            machine_logs,
            ssh_action,
            tailscale_action,
            update_agent,
            start_log_stream,
            stop_log_stream,
            term_open,
            term_open_host,
            term_write,
            term_resize,
            term_close,
        ])
        .run(tauri::generate_context!())
        .expect("error while running bsdkrun desktop");
}

/// The remote half of [`stream_bsdkrun`]: run an argv on the daemon and emit
/// the same `flavor://log` / `flavor://done` events the local path does, so the
/// launch UI cannot tell which backend it is watching.
fn stream_remote(
    app: tauri::AppHandle,
    endpoint: String,
    token: String,
    args: Vec<String>,
    launch_id: String,
    capture_id: bool,
) {
    tokio::spawn(async move {
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        match remote::run(&endpoint, &token, &argv).await {
            Ok(out) => {
                let mut id = None;
                for line in out.stdout.lines().chain(out.stderr.lines()) {
                    let t = line.trim();
                    if t.is_empty() {
                        continue;
                    }
                    // A bare 12-hex-digit line is the machine id, not progress.
                    if capture_id
                        && id.is_none()
                        && t.len() == 12
                        && t.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        id = Some(t.to_string());
                        continue;
                    }
                    let _ = app.emit(
                        "flavor://log",
                        FlavorLog {
                            launch_id: launch_id.clone(),
                            line: sanitize_log(t),
                        },
                    );
                }
                let error =
                    (out.code != 0).then(|| format!("bsdkrun exited with status {}", out.code));
                let _ = app.emit(
                    "flavor://done",
                    FlavorDone {
                        launch_id,
                        id,
                        error,
                    },
                );
            }
            Err(e) => {
                let _ = app.emit(
                    "flavor://done",
                    FlavorDone {
                        launch_id,
                        id: None,
                        error: Some(e.to_string()),
                    },
                );
            }
        }
    });
}
