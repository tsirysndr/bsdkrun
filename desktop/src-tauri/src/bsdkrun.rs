//! Thin wrapper around the `bsdkrun` CLI: binary resolution, JSON-emitting
//! subcommands (`ps`/`images`/`volume ls`), lifecycle commands, and helpers to
//! build a `Command`/`CommandBuilder` with a sane PATH so the CLI can find its
//! own runtime tools (gvproxy, curl, tar, libkrun…) when launched from a GUI.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::target::Target;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Directories a GUI-launched process is unlikely to have on PATH but where
/// bsdkrun and its runtime tools commonly live. Prepended to the child PATH.
const EXTRA_PATHS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

#[derive(Debug, thiserror::Error)]
pub enum BkError {
    #[error("bsdkrun binary not found. Set its path in Settings.")]
    NotFound,
    #[error("bsdkrun exited with status {code}: {stderr}")]
    NonZero { code: i32, stderr: String },
    #[error("io error: {0}")]
    Io(String),
    #[error("failed to parse bsdkrun output: {0}")]
    Parse(String),
}

impl serde::Serialize for BkError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl From<std::io::Error> for BkError {
    fn from(e: std::io::Error) -> Self {
        BkError::Io(e.to_string())
    }
}

/// Build a PATH string that includes the usual macOS/Linux tool dirs, keeping
/// any dirs already present in the inherited PATH.
pub fn augmented_path() -> String {
    let mut parts: Vec<String> = EXTRA_PATHS.iter().map(|s| s.to_string()).collect();
    if let Ok(existing) = std::env::var("PATH") {
        for p in existing.split(':') {
            if !p.is_empty() && !parts.iter().any(|x| x == p) {
                parts.push(p.to_string());
            }
        }
    }
    parts.join(":")
}

/// Resolve the bsdkrun binary: an explicit override first, then PATH, then a
/// handful of well-known install locations.
pub fn resolve_binary(override_path: Option<&str>) -> Result<PathBuf, BkError> {
    if let Some(p) = override_path {
        let pb = PathBuf::from(p);
        if !p.is_empty() && pb.exists() {
            return Ok(pb);
        }
    }
    // Search PATH (augmented so a GUI launch still finds Homebrew installs).
    let path = augmented_path();
    if let Ok(found) = which::which_in(
        "bsdkrun",
        Some(path),
        std::env::current_dir().unwrap_or_default(),
    ) {
        return Ok(found);
    }
    for cand in ["/opt/homebrew/bin/bsdkrun", "/usr/local/bin/bsdkrun"] {
        let pb = PathBuf::from(cand);
        if pb.exists() {
            return Ok(pb);
        }
    }
    Err(BkError::NotFound)
}

/// A configured async `Command` for the resolved binary with an augmented PATH.
/// Used for the short, output-capturing subcommands.
pub fn command(bin: &PathBuf) -> Command {
    let mut cmd = Command::new(bin);
    cmd.env("PATH", augmented_path());
    cmd.kill_on_drop(true);
    cmd
}

/// Run a subcommand to completion and return stdout, mapping a non-zero exit to
/// an error carrying stderr. Only for commands that fully exit (ps/images/…).
///
/// Wrapped in a timeout so a misbehaving invocation can never wedge the IPC and
/// freeze the UI: `command()` sets `kill_on_drop`, so a timeout drops (and thus
/// kills) the child, and the caller gets an error instead of hanging forever.
pub async fn run(bin: &Target, args: &[&str]) -> Result<String, BkError> {
    let bin = match bin {
        Target::Local(p) => p,
        Target::Remote { endpoint, token } => {
            let out = crate::remote::run(endpoint, token, args).await?;
            if out.code != 0 {
                return Err(BkError::NonZero {
                    code: out.code,
                    stderr: out.stderr.trim().to_string(),
                });
            }
            return Ok(out.stdout);
        }
    };
    let fut = command(bin).args(args).output();
    let out = tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .map_err(|_| BkError::Io(format!("`bsdkrun {}` timed out", args.join(" "))))??;
    if !out.status.success() {
        return Err(BkError::NonZero {
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a subcommand and return its combined stdout+stderr **regardless of exit
/// code**. Used for diagnostic/agent actions (`ssh`/`tailscale` status/setup)
/// where a non-zero exit is a legitimate state ("not installed", "not running")
/// the user should see, not a hard error. Only spawn/timeout failures error.
pub async fn run_output(bin: &Target, args: &[&str]) -> Result<String, BkError> {
    let bin = match bin {
        Target::Local(p) => p,
        Target::Remote { endpoint, token } => {
            let out = crate::remote::run(endpoint, token, args).await?;
            return Ok(combine(out.code, &out.stdout, &out.stderr));
        }
    };
    let fut = command(bin).args(args).output();
    let out = tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .map_err(|_| BkError::Io(format!("`bsdkrun {}` timed out", args.join(" "))))??;
    Ok(combine(
        out.status.code().unwrap_or(-1),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    ))
}

/// Both streams, with a stand-in when a command said nothing at all — shared by
/// the local and remote paths so they read identically in the UI.
fn combine(code: i32, stdout: &str, stderr: &str) -> String {
    let mut s = stdout.trim_end().to_string();
    let err = stderr.trim();
    if !err.is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(err);
    }
    if s.trim().is_empty() {
        s = format!("(no output — exit {code})");
    }
    s
}

/// Run a subcommand and return only its exit code (no output capture beyond
/// what's needed), never erroring on non-zero. Used for cheap probes.
pub async fn run_code(bin: &Target, args: &[&str]) -> i32 {
    let bin = match bin {
        Target::Local(p) => p,
        Target::Remote { endpoint, token } => {
            return match crate::remote::run(endpoint, token, args).await {
                Ok(out) => out.code,
                Err(_) => -1,
            };
        }
    };
    match tokio::time::timeout(Duration::from_secs(20), command(bin).args(args).output()).await {
        Ok(Ok(out)) => out.status.code().unwrap_or(-1),
        _ => -1,
    }
}

/// Find an interactive shell that actually exists in the guest, trying common
/// candidates via the agent (`exec <id> <sh> -c :`, exit 127 ⇒ not present).
/// Prefers bash, then a PATH `sh`, then absolute paths. Returns the argv to run;
/// falls back to `/bin/sh` if every probe fails (e.g. no agent — the caller's
/// error handling then surfaces the real problem).
pub async fn resolve_guest_shell(bin: &Target, id: &str) -> Vec<String> {
    for sh in [
        "bash",
        "sh",
        "/bin/bash",
        "/bin/sh",
        "/run/current-system/sw/bin/bash",
    ] {
        // 127 = spawn failed (not found). Anything else means the shell exists.
        if run_code(bin, &["exec", id, sh, "-c", ":"]).await != 127 {
            return vec![sh.to_string()];
        }
    }
    vec!["/bin/sh".to_string()]
}

/// Launch a detached (`-d`) machine and return its id.
///
/// CRITICAL: we must NOT use `.output()` here. `bsdkrun -d` forks a long-lived
/// VM process that inherits the stdout/stderr pipes and keeps them open, so
/// reading to EOF would block forever (wedging the IPC worker and freezing the
/// UI). Instead we read exactly the one id line the parent prints, then wait for
/// the short-lived parent to exit — never touching EOF.
pub async fn run_detached(bin: &Target, args: &[&str]) -> Result<String, BkError> {
    let bin = match bin {
        Target::Local(p) => p,
        // The daemon always launches detached and returns the id itself, so
        // none of the pipe-holding-grandchild care below applies.
        Target::Remote { endpoint, token } => {
            return crate::remote::run_detached(endpoint, token, args).await
        }
    };
    let mut cmd = Command::new(bin);
    cmd.env("PATH", augmented_path());
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // No kill_on_drop: the detached VM is a grandchild we must never signal.
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // CRITICAL: continuously drain stderr into a shared bounded buffer. `bsdkrun`
    // logs verbosely to stderr; if we don't read it, the ~64 KiB pipe fills, the
    // CLI *blocks* on its next stderr write, the short-lived parent never exits,
    // and `child.wait()` hangs forever — the real "Play spins forever" bug. This
    // task keeps the pipe empty so the parent can always finish.
    let err_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let err_buf2 = err_buf.clone();
    let err_handle = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let mut b = err_buf2.lock().unwrap();
            if b.len() < 8000 {
                b.push_str(&line);
                b.push('\n');
            }
        }
    });

    // First stdout line = machine id (terminated by '\n' before the parent
    // exits). read_line returns on the newline, so this can't hang on EOF.
    let mut lines = BufReader::new(stdout).lines();
    let id = tokio::time::timeout(Duration::from_secs(300), lines.next_line())
        .await
        .map_err(|_| BkError::Io("timed out waiting for the machine to start".into()))?
        .map_err(|e| BkError::Io(e.to_string()))?;

    // Keep draining stdout too (the VM grandchild inherits it) so it can't fill.
    let out_handle =
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });

    // Wait for the (short-lived) parent to exit to learn its status.
    let status = tokio::time::timeout(Duration::from_secs(300), child.wait())
        .await
        .map_err(|_| BkError::Io("timed out launching machine".into()))?
        .map_err(|e| BkError::Io(e.to_string()))?;

    // The grandchild keeps the pipes open, so the drain tasks never EOF — stop them.
    out_handle.abort();
    err_handle.abort();

    if !status.success() {
        let stderr = err_buf.lock().unwrap().trim().to_string();
        return Err(BkError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr,
        });
    }

    match id {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => Err(BkError::Io("machine started but reported no id".into())),
    }
}

// ---- typed models (mirror the CLI's `--json` shapes) ----------------------

/// A host↔guest TCP port forward (mirrors `bsdkrun ps --json`'s `ports[]`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PortForward {
    pub bind: String,
    pub host: u16,
    pub guest: u16,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Machine {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub image: String,
    pub kind: String,
    pub command: String,
    pub status: String,
    pub running: bool,
    pub exit_code: Option<i64>,
    pub pid: Option<i64>,
    pub detached: bool,
    pub cpus: Option<i64>,
    pub mem: Option<i64>,
    pub volume: Option<String>,
    pub state_dir: Option<String>,
    pub created_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub net_ip: Option<String>,
    #[serde(default)]
    pub ports: Vec<PortForward>,
    /// The snapshot this machine was branched from, if any.
    #[serde(default)]
    pub origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Image {
    pub id: String,
    pub reference: String,
    pub digest: Option<String>,
    pub size: i64,
    pub rootfs: Option<String>,
    pub created_at: Option<String>,
    /// Machines still using it; only a dangling image can be removed.
    #[serde(default)]
    pub used_by: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Volume {
    pub name: String,
    pub guest: Option<String>,
    pub base: Option<String>,
    pub path: Option<String>,
    pub size: Option<i64>,
    pub created_at: Option<String>,
    pub tracked: bool,
}

/// A flavor entry (mirrors `bsdkrun flavors --json`): a catalog environment, a
/// user-defined `flavors.toml` entry, or a saved snapshot.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Flavor {
    pub name: String,
    /// "catalog" | "user" | "snapshot".
    pub source: String,
    /// "linux" | "freebsd" | "netbsd".
    pub kind: String,
    /// Base image ref, or a BSD slug.
    pub base: String,
    pub category: String,
    /// "docker" | "nix" | "system" | "snapshot".
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub nix: Vec<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

pub async fn list_flavors(bin: &Target) -> Result<Vec<Flavor>, BkError> {
    let out = run(bin, &["flavors", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

/// A machine snapshot (mirrors `bsdkrun snapshots --json`): one machine's disk
/// state, captured under a name and bootable as a new machine (`branch`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Snapshot {
    pub id: String,
    pub name: String,
    pub machine_id: String,
    /// The machine's name when it was taken; empty if it had none.
    #[serde(default)]
    pub machine_name: String,
    /// "linux" | "freebsd" | "netbsd" | "unikraft".
    pub kind: String,
    pub image: String,
    pub path: String,
    /// The snapshot the source machine was itself branched from, if any.
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub description: String,
    pub cpus: i64,
    pub mem: i64,
    #[serde(default)]
    pub ports: Vec<PortForward>,
    #[serde(default)]
    pub size: Option<String>,
    pub created_at: String,
}

pub async fn list_snapshots(bin: &Target, machine: Option<&str>) -> Result<Vec<Snapshot>, BkError> {
    let mut args = vec!["snapshots"];
    if let Some(m) = machine.filter(|m| !m.is_empty()) {
        args.push(m);
    }
    args.push("--json");
    let out = run(bin, &args).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

/// A coding agent bsdkrun can sandbox (mirrors `bsdkrun ai agents --json`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiAgent {
    pub id: String,
    pub label: String,
    pub flavor: String,
    #[serde(default)]
    pub description: String,
    /// Its flavor is provisioned, so a sandbox boots in a second. False means
    /// the first launch builds it — minutes, with streamed output.
    pub installed: bool,
    #[serde(default)]
    pub running: i64,
}

/// One agent sandbox (mirrors `bsdkrun ai ls --json`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiSession {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub running: bool,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

pub async fn ai_agents(bin: &Target) -> Result<Vec<AiAgent>, BkError> {
    let out = run(bin, &["ai", "agents", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

pub async fn ai_sessions(bin: &Target) -> Result<Vec<AiSession>, BkError> {
    let out = run(bin, &["ai", "ls", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

/// One CI workflow, as `bsdkrun ci ls --json` reports it.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CiWorkflow {
    pub name: String,
    pub engine: String,
    pub matches: bool,
}

/// The workflows in a repository, and whether each matches `event`.
pub async fn ci_workflows(bin: &Target, dir: &str, event: &str) -> Result<Vec<CiWorkflow>, BkError> {
    let out = run(bin, &["ci", "ls", "-w", dir, "--event", event, "--json"]).await?;
    serde_json::from_str(out.trim()).map_err(|e| BkError::Parse(e.to_string()))
}

/// The Docker engine VM's status (mirrors `bsdkrun docker status --json`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DockerStatus {
    pub running: bool,
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub machine_running: bool,
    pub socket: String,
    #[serde(default)]
    pub socket_ready: bool,
    #[serde(default)]
    pub api_port: Option<u16>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub containers: Option<i64>,
    #[serde(default)]
    pub images: Option<i64>,
    #[serde(default)]
    pub mounts: Vec<String>,
    #[serde(default)]
    pub context: bool,
    #[serde(default)]
    pub context_active: bool,
    #[serde(default)]
    pub system_socket: bool,
    #[serde(default)]
    pub disk: Option<String>,
    #[serde(default)]
    pub disk_size: Option<u64>,
}

/// A container in the Docker engine VM (mirrors `bsdkrun docker ps --json`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub command: String,
    pub state: String,
    pub status: String,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub created: i64,
}

pub async fn docker_status(bin: &Target) -> Result<DockerStatus, BkError> {
    let out = run(bin, &["docker", "status", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

pub async fn docker_containers(bin: &Target, all: bool) -> Result<Vec<DockerContainer>, BkError> {
    let mut args = vec!["docker", "ps"];
    if all {
        args.push("--all");
    }
    args.push("--json");
    let out = run(bin, &args).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

/// A global network (mirrors `bsdkrun network ls --json`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Network {
    pub name: String,
    pub subnet: String,
    pub gateway: String,
    pub members: i64,
    pub running: i64,
    pub up: bool,
    pub created_at: Option<String>,
}

pub async fn list_networks(bin: &Target) -> Result<Vec<Network>, BkError> {
    let out = run(bin, &["network", "ls", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

pub async fn list_machines(bin: &Target, all: bool) -> Result<Vec<Machine>, BkError> {
    let args: &[&str] = if all {
        &["ps", "-a", "--json"]
    } else {
        &["ps", "--json"]
    };
    let out = run(bin, args).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

pub async fn list_images(bin: &Target) -> Result<Vec<Image>, BkError> {
    let out = run(bin, &["images", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

pub async fn list_volumes(bin: &Target) -> Result<Vec<Volume>, BkError> {
    let out = run(bin, &["volume", "ls", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

/// Parse `versions --os <os>` — indented `  <ver>  (latest)` lines.
pub async fn list_versions(bin: &Target, os: &str) -> Result<Vec<VersionEntry>, BkError> {
    let out = run(bin, &["versions", "--os", os]).await?;
    let mut v = Vec::new();
    for line in out.lines() {
        let t = line.trim();
        let first = t.split_whitespace().next().unwrap_or("");
        // Accept version rows (`  15.1  (latest)`) and NetBSD's `current` row
        // (the recommended build that boots to root under libkrun).
        let is_version = first
            .chars()
            .next()
            .map(|c| c.is_ascii_digit())
            .unwrap_or(false);
        let is_current = first == "current";
        if !is_version && !is_current {
            continue;
        }
        let latest = t.contains("(latest)") || is_current;
        v.push(VersionEntry {
            version: first.to_string(),
            latest,
        });
    }
    Ok(v)
}

#[derive(Debug, Serialize)]
pub struct VersionEntry {
    pub version: String,
    pub latest: bool,
}
