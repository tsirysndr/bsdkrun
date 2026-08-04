//! bsdkrun Desktop — Tauri backend.
//!
//! A thin, async bridge over the `bsdkrun` CLI: it resolves the binary, runs
//! the JSON-emitting subcommands, launches machines, streams logs, and hosts
//! interactive PTY sessions. All heavy lifting lives in `bsdkrun` and `term`.

mod bsdkrun;
mod menu;
mod system;
mod term;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use bsdkrun::{BkError, Image, Machine, VersionEntry, Volume};
use term::{LogStreams, Terminals};

/// Persisted app settings (currently just an optional binary override).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Explicit path to the `bsdkrun` binary; empty ⇒ auto-resolve.
    #[serde(default)]
    pub binary_path: String,
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
    fn binary(&self) -> Result<PathBuf, BkError> {
        let over = self.settings.lock().unwrap().binary_path.clone();
        let over = if over.is_empty() { None } else { Some(over) };
        bsdkrun::resolve_binary(over.as_deref())
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
fn set_settings(app: tauri::AppHandle, state: State<AppState>, binary_path: String) -> Settings {
    let mut s = state.settings.lock().unwrap();
    s.binary_path = binary_path;
    save_settings(&app, &s);
    s.clone()
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
            binary: Some(bin.to_string_lossy().into_owned()),
        }),
        Err(e) => Ok(ProbeResult {
            ok: false,
            message: e.to_string(),
            binary: Some(bin.to_string_lossy().into_owned()),
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
async fn list_versions(state: State<'_, AppState>, os: String) -> Result<Vec<VersionEntry>, BkError> {
    let bin = state.binary()?;
    bsdkrun::list_versions(&bin, &os).await
}

/// Host CPU% + RAM and the real on-disk size of all microVMs (status bar).
#[tauri::command]
async fn system_stats(state: State<'_, AppState>) -> Result<system::SystemStats, String> {
    use sysinfo::{MemoryRefreshKind, RefreshKind};
    let (cpu, mem_used, mem_total) = {
        let mut sys = state.sys.lock().unwrap();
        sys.refresh_cpu_usage();
        sys.refresh_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
        );
        (sys.global_cpu_usage(), sys.used_memory(), sys.total_memory())
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
    pub kind: String, // "linux" | "freebsd" | "netbsd"
    #[serde(default)]
    pub image: Option<String>,
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
    /// Extra virtio-blk disks to attach (BSD only), `PATH[:ro]`.
    #[serde(default)]
    pub attach_disks: Vec<String>,
    /// Grow the guest root disk to this size before boot (BSD only), e.g. `8G`.
    #[serde(default)]
    pub disk_size: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty())
}

fn build_run_args(spec: &RunSpec) -> Result<Vec<String>, BkError> {
    let mut a: Vec<String> = vec![spec.kind.clone(), "-d".into()];
    let bsd = spec.kind == "freebsd" || spec.kind == "netbsd";

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
    if let Some(v) = nonempty(&spec.volume) {
        a.push("-v".into());
        a.push(v.into());
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
        let image = nonempty(&spec.image)
            .ok_or_else(|| BkError::Parse("an image reference is required for linux".into()))?;
        a.push(image.into());
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
    if !spec.command.is_empty() {
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
async fn remove_volume(state: State<'_, AppState>, name: String, force: bool) -> Result<(), BkError> {
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
async fn machine_logs(state: State<'_, AppState>, id: String, boot: bool) -> Result<String, BkError> {
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
    term::open(&app, &terminals, &bin, &id, command, rows.max(1), cols.max(1))
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
async fn term_write(terminals: State<'_, Terminals>, session: String, data: String) -> Result<(), String> {
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
            // Load persisted settings into managed state.
            let loaded = load_settings(app.handle());
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
            set_tray_status,
            probe,
            list_machines,
            list_images,
            list_volumes,
            list_versions,
            system_stats,
            run_machine,
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
