//! Thin wrapper around the `bsdkrun` CLI: binary resolution, JSON-emitting
//! subcommands (`ps`/`images`/`volume ls`), lifecycle commands, and helpers to
//! build a `Command`/`CommandBuilder` with a sane PATH so the CLI can find its
//! own runtime tools (gvproxy, curl, tar, libkrun…) when launched from a GUI.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
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
    if let Ok(found) = which::which_in("bsdkrun", Some(path), std::env::current_dir().unwrap_or_default())
    {
        return Ok(found);
    }
    for cand in [
        "/opt/homebrew/bin/bsdkrun",
        "/usr/local/bin/bsdkrun",
    ] {
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
pub async fn run(bin: &PathBuf, args: &[&str]) -> Result<String, BkError> {
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

/// Launch a detached (`-d`) machine and return its id.
///
/// CRITICAL: we must NOT use `.output()` here. `bsdkrun -d` forks a long-lived
/// VM process that inherits the stdout/stderr pipes and keeps them open, so
/// reading to EOF would block forever (wedging the IPC worker and freezing the
/// UI). Instead we read exactly the one id line the parent prints, then wait for
/// the short-lived parent to exit — never touching EOF.
pub async fn run_detached(bin: &PathBuf, args: &[&str]) -> Result<String, BkError> {
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

    // First stdout line = machine id (terminated by '\n' before the parent
    // exits). read_line returns on the newline, so this can't hang on EOF.
    let mut lines = BufReader::new(stdout).lines();
    let id = tokio::time::timeout(Duration::from_secs(300), lines.next_line())
        .await
        .map_err(|_| BkError::Io("timed out waiting for the machine to start".into()))?
        .map_err(|e| BkError::Io(e.to_string()))?;

    // Wait for the (short-lived) parent to exit to learn its status.
    let status = tokio::time::timeout(Duration::from_secs(300), child.wait())
        .await
        .map_err(|_| BkError::Io("timed out launching machine".into()))?
        .map_err(|e| BkError::Io(e.to_string()))?;

    if !status.success() {
        // Drain stderr briefly (bounded) for the failure reason.
        let mut errbuf = String::new();
        let mut errlines = BufReader::new(stderr).lines();
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            while let Ok(Some(l)) = errlines.next_line().await {
                errbuf.push_str(&l);
                errbuf.push('\n');
                if errbuf.len() > 4000 {
                    break;
                }
            }
        })
        .await;
        return Err(BkError::NonZero {
            code: status.code().unwrap_or(-1),
            stderr: errbuf.trim().to_string(),
        });
    }

    match id {
        Some(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
        _ => Err(BkError::Io("machine started but reported no id".into())),
    }
}

// ---- typed models (mirror the CLI's `--json` shapes) ----------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Machine {
    pub id: String,
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Image {
    pub id: String,
    pub reference: String,
    pub digest: Option<String>,
    pub size: i64,
    pub rootfs: Option<String>,
    pub created_at: Option<String>,
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

pub async fn list_machines(bin: &PathBuf, all: bool) -> Result<Vec<Machine>, BkError> {
    let args: &[&str] = if all {
        &["ps", "-a", "--json"]
    } else {
        &["ps", "--json"]
    };
    let out = run(bin, args).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

pub async fn list_images(bin: &PathBuf) -> Result<Vec<Image>, BkError> {
    let out = run(bin, &["images", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

pub async fn list_volumes(bin: &PathBuf) -> Result<Vec<Volume>, BkError> {
    let out = run(bin, &["volume", "ls", "--json"]).await?;
    serde_json::from_str(&out).map_err(|e| BkError::Parse(e.to_string()))
}

/// Parse `versions --os <os>` — indented `  <ver>  (latest)` lines.
pub async fn list_versions(bin: &PathBuf, os: &str) -> Result<Vec<VersionEntry>, BkError> {
    let out = run(bin, &["versions", "--os", os]).await?;
    let mut v = Vec::new();
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() || !t.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            continue;
        }
        let latest = t.contains("(latest)");
        let ver = t.split_whitespace().next().unwrap_or("").to_string();
        if !ver.is_empty() {
            v.push(VersionEntry { version: ver, latest });
        }
    }
    Ok(v)
}

#[derive(Debug, Serialize)]
pub struct VersionEntry {
    pub version: String,
    pub latest: bool,
}
