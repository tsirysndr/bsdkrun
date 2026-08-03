//! Persistent state for images, machines, and disks, in a SQLite database via
//! sqlx. Docker-style: every image / VM / disk gets a short id, and `ps` /
//! `images` read back from here.
//!
//! sqlx is async; the rest of bsdkrun is synchronous, so we own a small Tokio
//! runtime and `block_on` each query — the DB is local SQLite, so this is cheap.

use std::path::PathBuf;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use tokio::runtime::Runtime;

// (Runtime is created via Builder::new_current_thread — see Db::open.)

/// State directory: `$BSDKRUN_STATE`, else `$XDG_STATE_HOME/bsdkrun`, else
/// `$HOME/.local/state/bsdkrun`. Holds the database and per-machine runtime dirs.
pub fn state_dir() -> Result<PathBuf> {
    if let Ok(s) = std::env::var("BSDKRUN_STATE") {
        if !s.is_empty() {
            return Ok(PathBuf::from(s));
        }
    }
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x).join("bsdkrun"));
        }
    }
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/bsdkrun"))
}

/// Runtime directory for a machine (`<state>/machines/<id>`): console log + socket, etc.
pub fn machine_dir(id: &str) -> Result<PathBuf> {
    Ok(state_dir()?.join("machines").join(id))
}

/// Directory holding named persistent volumes (`<state>/volumes`). Unlike a
/// machine's runtime dir, these survive across runs so guest changes persist.
pub fn volumes_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join("volumes"))
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // some columns are stored for future use / debugging
pub struct ImageRow {
    pub id: String,
    pub reference: String,
    pub digest: String,
    pub size: i64,
    pub rootfs: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // some columns are stored for future use / debugging
pub struct MachineRow {
    pub id: String,
    pub image: String,
    pub kind: String,
    pub command: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub pid: Option<i64>,
    pub detached: bool,
    pub cpus: i64,
    pub mem: i64,
    pub state_dir: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub volume: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiskRow {
    pub id: String,
    pub path: String,
    pub size: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct VolumeRow {
    pub name: String,
    pub kind: String,
    pub base: String,
    pub path: String,
    pub created_at: String,
}

pub struct Db {
    rt: Runtime,
    pool: SqlitePool,
}

impl Db {
    /// Open (creating if needed) the state database and run migrations.
    pub fn open() -> Result<Self> {
        let dir = state_dir()?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating state dir {}", dir.display()))?;
        let path = dir.join("bsdkrun.db");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .context("creating tokio runtime for sqlite")?;
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = rt
            .block_on(
                SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect_with(opts),
            )
            .with_context(|| format!("opening database {}", path.display()))?;
        let db = Db { rt, pool };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.rt.block_on(async {
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS images (
                    id TEXT PRIMARY KEY,
                    reference TEXT NOT NULL,
                    digest TEXT NOT NULL UNIQUE,
                    size INTEGER NOT NULL,
                    rootfs TEXT NOT NULL,
                    created_at TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS machines (
                    id TEXT PRIMARY KEY,
                    image TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    command TEXT NOT NULL,
                    status TEXT NOT NULL,
                    exit_code INTEGER,
                    pid INTEGER,
                    detached INTEGER NOT NULL,
                    cpus INTEGER NOT NULL,
                    mem INTEGER NOT NULL,
                    state_dir TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    finished_at TEXT
                )",
            )
            .execute(&self.pool)
            .await?;
            // Add columns to databases created before they existed (errors with
            // "duplicate column" on newer ones — ignored).
            let _ = sqlx::query("ALTER TABLE machines ADD COLUMN finished_at TEXT")
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("ALTER TABLE machines ADD COLUMN volume TEXT")
                .execute(&self.pool)
                .await;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS disks (
                    id TEXT PRIMARY KEY,
                    path TEXT NOT NULL UNIQUE,
                    size INTEGER,
                    created_at TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await?;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS volumes (
                    name TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    base TEXT NOT NULL,
                    path TEXT NOT NULL,
                    created_at TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await?;
            Ok::<_, sqlx::Error>(())
        })?;
        Ok(())
    }

    // ---- images ---------------------------------------------------------

    /// Record an image (idempotent by digest). Returns its short id, reusing the
    /// existing one if this digest was seen before.
    pub fn upsert_image(
        &self,
        reference: &str,
        digest: &str,
        size: i64,
        rootfs: &str,
    ) -> Result<String> {
        self.rt
            .block_on(async {
                if let Some(row) = sqlx::query("SELECT id FROM images WHERE digest = ?")
                    .bind(digest)
                    .fetch_optional(&self.pool)
                    .await?
                {
                    let id: String = row.get("id");
                    // Keep the reference/rootfs fresh (tag may have moved).
                    sqlx::query("UPDATE images SET reference = ?, rootfs = ? WHERE id = ?")
                        .bind(reference)
                        .bind(rootfs)
                        .bind(&id)
                        .execute(&self.pool)
                        .await?;
                    return Ok::<_, sqlx::Error>(id);
                }
                let id = crate::id::short_id();
                sqlx::query(
                    "INSERT INTO images (id, reference, digest, size, rootfs, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(reference)
                .bind(digest)
                .bind(size)
                .bind(rootfs)
                .bind(now())
                .execute(&self.pool)
                .await?;
                Ok(id)
            })
            .map_err(Into::into)
    }

    pub fn list_images(&self) -> Result<Vec<ImageRow>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT id, reference, digest, size, rootfs, created_at
                     FROM images ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(
                    rows.into_iter()
                        .map(|r| ImageRow {
                            id: r.get("id"),
                            reference: r.get("reference"),
                            digest: r.get("digest"),
                            size: r.get("size"),
                            rootfs: r.get("rootfs"),
                            created_at: r.get("created_at"),
                        })
                        .collect(),
                )
            })
            .map_err(Into::into)
    }

    // ---- machines ------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn insert_machine(
        &self,
        id: &str,
        image: &str,
        kind: &str,
        command: &str,
        status: &str,
        pid: Option<i64>,
        detached: bool,
        cpus: i64,
        mem: i64,
        state_dir: &str,
        volume: Option<&str>,
    ) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO machines
                     (id, image, kind, command, status, pid, detached, cpus, mem, state_dir, created_at, volume)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(id)
                .bind(image)
                .bind(kind)
                .bind(command)
                .bind(status)
                .bind(pid)
                .bind(detached as i64)
                .bind(cpus)
                .bind(mem)
                .bind(state_dir)
                .bind(now())
                .bind(volume)
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn set_machine_status(&self, id: &str, status: &str, exit_code: Option<i64>) -> Result<()> {
        self.rt
            .block_on(async {
                // Stamp finished_at the first time a machine exits (COALESCE keeps
                // any earlier stamp; NULL while it's still running).
                let finished_at = (status == "exited").then(now);
                sqlx::query(
                    "UPDATE machines
                     SET status = ?, exit_code = ?, finished_at = COALESCE(finished_at, ?)
                     WHERE id = ?",
                )
                .bind(status)
                .bind(exit_code)
                .bind(finished_at)
                .bind(id)
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn list_machines(&self) -> Result<Vec<MachineRow>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT id, image, kind, command, status, exit_code, pid, detached,
                            cpus, mem, state_dir, created_at, finished_at, volume
                     FROM machines ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(rows.into_iter().map(row_to_machine).collect())
            })
            .map_err(Into::into)
    }

    /// Resolve an id prefix to exactly one VM (Docker-style short-id matching).
    pub fn find_machine(&self, prefix: &str) -> Result<MachineRow> {
        let matches: Vec<MachineRow> = self.rt.block_on(async {
            let rows = sqlx::query(
                "SELECT id, image, kind, command, status, exit_code, pid, detached,
                        cpus, mem, state_dir, created_at, finished_at, volume
                 FROM machines WHERE id LIKE ? ORDER BY created_at DESC",
            )
            .bind(format!("{prefix}%"))
            .fetch_all(&self.pool)
            .await?;
            Ok::<_, sqlx::Error>(rows.into_iter().map(row_to_machine).collect())
        })?;
        match matches.len() {
            0 => anyhow::bail!("no such machine: {prefix}"),
            1 => Ok(matches.into_iter().next().unwrap()),
            n => anyhow::bail!("ambiguous id {prefix:?} matches {n} machines"),
        }
    }

    /// Delete a machine's DB row (its state dir is removed separately).
    pub fn delete_machine(&self, id: &str) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("DELETE FROM machines WHERE id = ?")
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    // ---- disks ----------------------------------------------------------

    /// Record a disk by path (idempotent). Returns its short id.
    pub fn upsert_disk(&self, path: &str, size: Option<i64>) -> Result<String> {
        self.rt
            .block_on(async {
                if let Some(row) = sqlx::query("SELECT id FROM disks WHERE path = ?")
                    .bind(path)
                    .fetch_optional(&self.pool)
                    .await?
                {
                    return Ok::<_, sqlx::Error>(row.get("id"));
                }
                let id = crate::id::short_id();
                sqlx::query("INSERT INTO disks (id, path, size, created_at) VALUES (?, ?, ?, ?)")
                    .bind(&id)
                    .bind(path)
                    .bind(size)
                    .bind(now())
                    .execute(&self.pool)
                    .await?;
                Ok(id)
            })
            .map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn list_disks(&self) -> Result<Vec<DiskRow>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT id, path, size, created_at FROM disks ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(
                    rows.into_iter()
                        .map(|r| DiskRow {
                            id: r.get("id"),
                            path: r.get("path"),
                            size: r.get("size"),
                            created_at: r.get("created_at"),
                        })
                        .collect(),
                )
            })
            .map_err(Into::into)
    }

    // ---- volumes --------------------------------------------------------

    /// Record a named volume the first time it's created (idempotent by name, so
    /// reusing a volume keeps its original metadata).
    pub fn upsert_volume(&self, name: &str, kind: &str, base: &str, path: &str) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO volumes (name, kind, base, path, created_at)
                     VALUES (?, ?, ?, ?, ?)
                     ON CONFLICT(name) DO NOTHING",
                )
                .bind(name)
                .bind(kind)
                .bind(base)
                .bind(path)
                .bind(now())
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn list_volumes(&self) -> Result<Vec<VolumeRow>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT name, kind, base, path, created_at FROM volumes
                     ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(rows.into_iter().map(row_to_volume).collect())
            })
            .map_err(Into::into)
    }

    pub fn find_volume(&self, name: &str) -> Result<Option<VolumeRow>> {
        self.rt
            .block_on(async {
                let row = sqlx::query(
                    "SELECT name, kind, base, path, created_at FROM volumes WHERE name = ?",
                )
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(row.map(row_to_volume))
            })
            .map_err(Into::into)
    }

    /// Remove a volume row; returns whether a row was deleted.
    pub fn remove_volume(&self, name: &str) -> Result<bool> {
        self.rt
            .block_on(async {
                let r = sqlx::query("DELETE FROM volumes WHERE name = ?")
                    .bind(name)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(r.rows_affected() > 0)
            })
            .map_err(Into::into)
    }
}

fn row_to_volume(r: sqlx::sqlite::SqliteRow) -> VolumeRow {
    VolumeRow {
        name: r.get("name"),
        kind: r.get("kind"),
        base: r.get("base"),
        path: r.get("path"),
        created_at: r.get("created_at"),
    }
}

fn row_to_machine(r: sqlx::sqlite::SqliteRow) -> MachineRow {
    MachineRow {
        id: r.get("id"),
        image: r.get("image"),
        kind: r.get("kind"),
        command: r.get("command"),
        status: r.get("status"),
        exit_code: r.get("exit_code"),
        pid: r.get("pid"),
        detached: r.get::<i64, _>("detached") != 0,
        cpus: r.get("cpus"),
        mem: r.get("mem"),
        state_dir: r.get("state_dir"),
        created_at: r.get("created_at"),
        finished_at: r.get("finished_at"),
        volume: r.get("volume"),
    }
}

/// UTC-ish timestamp string (seconds since the epoch, plus a readable form).
fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

/// Seconds elapsed since a unix-seconds timestamp string.
fn secs_since(ts: &str) -> u64 {
    let then: u64 = ts.parse().unwrap_or(0);
    let now: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(then)
}

/// Format a unix-seconds timestamp string as a relative age like "3m ago".
pub fn age(created_at: &str) -> String {
    let secs = secs_since(created_at);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// A human duration in Docker's exact phrasing (`docker/go-units` HumanDuration):
/// "5 seconds", "About a minute", "3 hours", "2 days", …
pub fn human_duration(secs: u64) -> String {
    let minutes = secs / 60;
    // Round hours like Docker (+30 min).
    let hours = (secs as f64 / 3600.0 + 0.5) as u64;
    if secs < 1 {
        "Less than a second".to_string()
    } else if secs == 1 {
        "1 second".to_string()
    } else if secs < 60 {
        format!("{secs} seconds")
    } else if minutes == 1 {
        "About a minute".to_string()
    } else if minutes < 60 {
        format!("{minutes} minutes")
    } else if hours == 1 {
        "About an hour".to_string()
    } else if hours < 48 {
        format!("{hours} hours")
    } else if hours < 24 * 7 * 2 {
        format!("{} days", hours / 24)
    } else if hours < 24 * 30 * 2 {
        format!("{} weeks", hours / 24 / 7)
    } else if hours < 24 * 365 * 2 {
        format!("{} months", hours / 24 / 30)
    } else {
        format!("{} years", hours / 24 / 365)
    }
}

/// Docker-style human duration since a unix-seconds timestamp string.
pub fn human_duration_since(ts: &str) -> String {
    human_duration(secs_since(ts))
}

/// Whether a process with `pid` is currently alive.
pub fn pid_alive(pid: i64) -> bool {
    pid > 0 && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0
}

// --- Best-effort recording helpers -------------------------------------------
//
// Each opens the DB, does one write, and closes it. Opening per-call (rather than
// holding a handle) keeps us from carrying an open SQLite connection + Tokio
// runtime across `fork()` in detached mode, which would be unsound. State is
// advisory, so failures are logged, never fatal.

/// Record a pulled image (idempotent by digest).
pub fn record_image(reference: &str, digest: &str, size: i64, rootfs: &str) {
    if let Err(e) = Db::open().and_then(|db| db.upsert_image(reference, digest, size, rootfs)) {
        tracing::warn!("recording image in state db: {e:#}");
    }
}

/// Record a machine row. `kind` is `linux` / `firmware` / `kernel` — the guest
/// type, which `shell`/`logs`/etc. use to apply the right behavior.
#[allow(clippy::too_many_arguments)]
pub fn record_machine(
    id: &str,
    image: &str,
    kind: &str,
    command: &str,
    status: &str,
    pid: Option<i64>,
    detached: bool,
    cpus: i64,
    mem: i64,
    state_dir: &str,
    volume: Option<&str>,
) {
    if let Err(e) = Db::open().and_then(|db| {
        db.insert_machine(
            id, image, kind, command, status, pid, detached, cpus, mem, state_dir, volume,
        )
    }) {
        tracing::warn!("recording machine in state db: {e:#}");
    }
}

/// Record a named volume (best-effort, idempotent by name).
pub fn record_volume(name: &str, kind: &str, base: &str, path: &str) {
    if let Err(e) = Db::open().and_then(|db| db.upsert_volume(name, kind, base, path)) {
        tracing::warn!("recording volume in state db: {e:#}");
    }
}

/// Update a machine's status (best-effort).
pub fn update_machine_status(id: &str, status: &str, exit_code: Option<i64>) {
    if let Err(e) = Db::open().and_then(|db| db.set_machine_status(id, status, exit_code)) {
        tracing::warn!("updating machine status: {e:#}");
    }
}

/// Record a disk by path (best-effort), giving it a short id like Docker.
pub fn record_disk(path: &str) {
    let size = std::fs::metadata(path).ok().map(|m| m.len() as i64);
    if let Err(e) = Db::open().and_then(|db| db.upsert_disk(path, size)) {
        tracing::warn!("recording disk in state db: {e:#}");
    }
}
