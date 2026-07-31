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
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DiskRow {
    pub id: String,
    pub path: String,
    pub size: Option<i64>,
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
                    created_at TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await?;
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
    ) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO machines
                     (id, image, kind, command, status, pid, detached, cpus, mem, state_dir, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn set_machine_status(&self, id: &str, status: &str, exit_code: Option<i64>) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("UPDATE machines SET status = ?, exit_code = ? WHERE id = ?")
                    .bind(status)
                    .bind(exit_code)
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
                            cpus, mem, state_dir, created_at
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
                        cpus, mem, state_dir, created_at
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

/// Format a unix-seconds timestamp string as a relative age like "3m ago".
pub fn age(created_at: &str) -> String {
    let then: u64 = created_at.parse().unwrap_or(0);
    let now: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let secs = now.saturating_sub(then);
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

/// Record a machine row.
#[allow(clippy::too_many_arguments)]
pub fn record_machine(
    id: &str,
    image: &str,
    command: &str,
    status: &str,
    pid: Option<i64>,
    detached: bool,
    cpus: i64,
    mem: i64,
    state_dir: &str,
) {
    if let Err(e) = Db::open().and_then(|db| {
        db.insert_machine(
            id, image, "linux", command, status, pid, detached, cpus, mem, state_dir,
        )
    }) {
        tracing::warn!("recording machine in state db: {e:#}");
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
