//! Persistent state for images, machines, and disks, in a SQLite database via
//! sqlx. Docker-style: every image / VM / disk gets a short id, and `ps` /
//! `images` read back from here.
//!
//! sqlx is async; the rest of bsdkrun is synchronous, so we own a small Tokio
//! runtime and `block_on` each query — the DB is local SQLite, so this is cheap.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::Row;
use tokio::runtime::Runtime;

// (Runtime is created via Builder::new_current_thread — see Db::open.)

/// One network member as `sync_hosts` needs it: (display name, ip, state_dir,
/// kind, pid).
pub type NetworkMemberHost = (String, String, String, String, Option<i64>);

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

/// Directory holding saved snapshot flavors (`<state>/flavors`).
pub fn flavors_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join("flavors"))
}

/// The historical location for named volumes, `<state>/volumes`. Prefer
/// [`volumes_dir`] — on macOS the volumes may have been relocated onto the
/// case-sensitive store.
pub fn volumes_dir_default() -> Result<PathBuf> {
    Ok(state_dir()?.join("volumes"))
}

/// Directory holding named persistent volumes. Unlike a machine's runtime dir,
/// these survive across runs so guest changes persist. On macOS they live on
/// the case-sensitive store when one is set up (see [`crate::store`]), which
/// also keeps `--volume`'s CoW clone of a base rootfs on the same volume.
pub fn volumes_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        crate::store::volumes_dir()
    }
    #[cfg(not(target_os = "macos"))]
    {
        volumes_dir_default()
    }
}

/// Directory holding global-network runtime state (`<state>/networks/<name>`):
/// the shared gvproxy's control socket + log.
pub fn networks_dir() -> Result<PathBuf> {
    Ok(state_dir()?.join("networks"))
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
    pub name: Option<String>,
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
    /// Global network this machine joins on (re)start, if any.
    pub network: Option<String>,
    /// Its assigned IP on that network (cleared when membership is edited; a
    /// fresh IP is allocated / DHCP'd on the next start).
    pub net_ip: Option<String>,
    /// Host↔guest TCP port forwards, comma-joined `HOST:GUEST` pairs (see
    /// [`crate::net::format_ports`] / `parse_ports`), or `None` if there are none.
    pub ports: Option<String>,
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

/// A saved snapshot flavor (a CoW clone of a machine's rootfs/disk).
#[derive(Debug, Clone)]
pub struct FlavorRow {
    pub name: String,
    pub kind: String, // linux / freebsd / netbsd
    pub base: String, // the base image/ref the snapshot came from
    pub path: String, // <state>/flavors/<name>
    pub description: String,
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
        // The desktop app spawns MANY concurrent `bsdkrun` processes (ps/images/
        // volumes/flavors/probe pollers + a boot). With the default rollback
        // journal a single writer exclusively locks the whole DB, so those
        // processes' connections time out ("pool timed out") and the UI hangs.
        // WAL lets readers run concurrently with one writer, and busy_timeout
        // makes a contended op wait-and-retry instead of failing immediately.
        let opts = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(15));
        let pool = rt
            .block_on(
                SqlitePoolOptions::new()
                    .max_connections(4)
                    .acquire_timeout(Duration::from_secs(20))
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
            let _ = sqlx::query("ALTER TABLE machines ADD COLUMN name TEXT")
                .execute(&self.pool)
                .await;
            // Global-network membership: which network a machine joined and the
            // IP it was assigned on that network's shared subnet.
            let _ = sqlx::query("ALTER TABLE machines ADD COLUMN network TEXT")
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("ALTER TABLE machines ADD COLUMN net_ip TEXT")
                .execute(&self.pool)
                .await;
            // Host↔guest port forwards, comma-joined `HOST:GUEST` pairs.
            let _ = sqlx::query("ALTER TABLE machines ADD COLUMN ports TEXT")
                .execute(&self.pool)
                .await;
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS networks (
                    name TEXT PRIMARY KEY,
                    subnet TEXT NOT NULL,
                    gateway TEXT NOT NULL,
                    control_socket TEXT NOT NULL,
                    dir TEXT NOT NULL,
                    pid INTEGER,
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
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS flavors (
                    name TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    base TEXT NOT NULL,
                    path TEXT NOT NULL,
                    description TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await?;
            // Host-level key/value settings (domains feature state, daemon pids).
            // Runtime pids live here rather than a config file so the lazy
            // respawn pattern (`pid_alive` check, then respawn) works exactly as
            // it does for networks.
            sqlx::query(
                "CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                )",
            )
            .execute(&self.pool)
            .await?;
            Ok::<_, sqlx::Error>(())
        })?;
        Ok(())
    }

    // ---- settings -------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.rt
            .block_on(async {
                let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
                    .bind(key)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(row.map(|r| r.get("value")))
            })
            .map_err(Into::into)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT INTO settings (key, value) VALUES (?, ?)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                )
                .bind(key)
                .bind(value)
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn remove_setting(&self, key: &str) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("DELETE FROM settings WHERE key = ?")
                    .bind(key)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
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
        name: &str,
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
        ports: Option<&str>,
    ) -> Result<()> {
        self.rt
            .block_on(async {
                // REPLACE so an in-place restart (`start <id>`) re-records the
                // same id without a delete step — the row updates from exited to
                // running rather than briefly vanishing from `ps`.
                sqlx::query(
                    "INSERT OR REPLACE INTO machines
                     (id, name, image, kind, command, status, pid, detached, cpus, mem, state_dir, created_at, volume, ports)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(id)
                .bind(name)
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
                .bind(ports)
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    /// Is a machine name already taken? Used to keep generated names unique.
    pub fn name_exists(&self, name: &str) -> bool {
        self.rt
            .block_on(async {
                sqlx::query("SELECT 1 FROM machines WHERE name = ? LIMIT 1")
                    .bind(name)
                    .fetch_optional(&self.pool)
                    .await
            })
            .map(|r| r.is_some())
            .unwrap_or(false)
    }

    /// A unique friendly name (`adjective_scientist`), checked against the DB.
    pub fn generate_name(&self) -> String {
        crate::names::unique_name(|n| self.name_exists(n))
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

    /// Update a machine's recorded vCPU / memory. libkrun fixes VM resources at
    /// boot, so this takes effect on the next `start` (an in-place restart).
    /// Overwrite a machine's recorded image label (e.g. to keep a snapshot
    /// machine's original label after a restart that boots its in-place disk).
    pub fn set_machine_image(&self, id: &str, image: &str) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("UPDATE machines SET image = ? WHERE id = ?")
                    .bind(image)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn set_machine_resources(&self, id: &str, cpus: i64, mem: i64) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("UPDATE machines SET cpus = ?, mem = ? WHERE id = ?")
                    .bind(cpus)
                    .bind(mem)
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
                    "SELECT id, name, image, kind, command, status, exit_code, pid, detached,
                            cpus, mem, state_dir, created_at, finished_at, volume, network, net_ip, ports
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
        // An exact name match wins (unambiguous), like `docker <name>`.
        if let Some(row) = self.rt.block_on(async {
            sqlx::query(
                "SELECT id, name, image, kind, command, status, exit_code, pid, detached,
                        cpus, mem, state_dir, created_at, finished_at, volume, network, net_ip, ports
                 FROM machines WHERE name = ? LIMIT 1",
            )
            .bind(prefix)
            .fetch_optional(&self.pool)
            .await
        })? {
            return Ok(row_to_machine(row));
        }
        // Otherwise resolve as an id prefix (Docker-style short ids).
        let matches: Vec<MachineRow> = self.rt.block_on(async {
            let rows = sqlx::query(
                "SELECT id, name, image, kind, command, status, exit_code, pid, detached,
                        cpus, mem, state_dir, created_at, finished_at, volume, network, net_ip, ports
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

    // ---- flavors (snapshots) --------------------------------------------

    /// Record (or replace) a snapshot flavor.
    pub fn upsert_flavor(
        &self,
        name: &str,
        kind: &str,
        base: &str,
        path: &str,
        description: &str,
    ) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT OR REPLACE INTO flavors (name, kind, base, path, description, created_at)
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(name)
                .bind(kind)
                .bind(base)
                .bind(path)
                .bind(description)
                .bind(now())
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn list_flavors(&self) -> Result<Vec<FlavorRow>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT name, kind, base, path, description, created_at
                     FROM flavors ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(rows.into_iter().map(row_to_flavor).collect())
            })
            .map_err(Into::into)
    }

    pub fn find_flavor(&self, name: &str) -> Result<Option<FlavorRow>> {
        self.rt
            .block_on(async {
                let row = sqlx::query(
                    "SELECT name, kind, base, path, description, created_at
                     FROM flavors WHERE name = ?",
                )
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(row.map(row_to_flavor))
            })
            .map_err(Into::into)
    }

    pub fn remove_flavor(&self, name: &str) -> Result<bool> {
        self.rt
            .block_on(async {
                let r = sqlx::query("DELETE FROM flavors WHERE name = ?")
                    .bind(name)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(r.rows_affected() > 0)
            })
            .map_err(Into::into)
    }

    // ---- global networks --------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn create_network(
        &self,
        name: &str,
        subnet: &str,
        gateway: &str,
        control_socket: &str,
        dir: &str,
        pid: Option<i64>,
    ) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query(
                    "INSERT OR REPLACE INTO networks
                       (name, subnet, gateway, control_socket, dir, pid, created_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(name)
                .bind(subnet)
                .bind(gateway)
                .bind(control_socket)
                .bind(dir)
                .bind(pid)
                .bind(now())
                .execute(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn set_network_pid(&self, name: &str, pid: Option<i64>) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("UPDATE networks SET pid = ? WHERE name = ?")
                    .bind(pid)
                    .bind(name)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    pub fn list_networks(&self) -> Result<Vec<NetworkRow>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT name, subnet, gateway, control_socket, dir, pid, created_at
                     FROM networks ORDER BY created_at DESC",
                )
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(rows.into_iter().map(row_to_network).collect())
            })
            .map_err(Into::into)
    }

    pub fn find_network(&self, name: &str) -> Result<Option<NetworkRow>> {
        self.rt
            .block_on(async {
                let row = sqlx::query(
                    "SELECT name, subnet, gateway, control_socket, dir, pid, created_at
                     FROM networks WHERE name = ?",
                )
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(row.map(row_to_network))
            })
            .map_err(Into::into)
    }

    pub fn remove_network(&self, name: &str) -> Result<bool> {
        self.rt
            .block_on(async {
                let r = sqlx::query("DELETE FROM networks WHERE name = ?")
                    .bind(name)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(r.rows_affected() > 0)
            })
            .map_err(Into::into)
    }

    /// Record a machine's network membership + assigned IP.
    pub fn set_machine_network(&self, id: &str, network: &str, net_ip: &str) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("UPDATE machines SET network = ?, net_ip = ? WHERE id = ?")
                    .bind(network)
                    .bind(net_ip)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    /// Edit a machine's network membership (join/switch/leave), clearing its
    /// assigned IP so the next start allocates/DHCPs a fresh one on the target
    /// network. `network = None` detaches it back to the default isolated stack.
    /// Takes effect on the next `start`.
    pub fn update_machine_network(&self, id: &str, network: Option<&str>) -> Result<()> {
        self.rt
            .block_on(async {
                sqlx::query("UPDATE machines SET network = ?, net_ip = NULL WHERE id = ?")
                    .bind(network)
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok::<_, sqlx::Error>(())
            })
            .map_err(Into::into)
    }

    /// Members of a network: (name-or-id, assigned IP, pid) — for IP allocation
    /// and internal DNS. `name` falls back to the id when the machine is unnamed.
    pub fn network_members(&self, network: &str) -> Result<Vec<(String, String, Option<i64>)>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT id, name, net_ip, pid FROM machines
                     WHERE network = ? AND net_ip IS NOT NULL",
                )
                .bind(network)
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(
                    rows.into_iter()
                        .map(|r| {
                            let id: String = r.get("id");
                            let name: Option<String> = r.get("name");
                            let ip: String = r.get("net_ip");
                            let pid: Option<i64> = r.get("pid");
                            (name.unwrap_or(id), ip, pid)
                        })
                        .collect(),
                )
            })
            .map_err(Into::into)
    }

    /// Members of a network with the extra bits needed to push `/etc/hosts` to
    /// each: (display name, ip, state_dir, kind, pid).
    pub fn network_member_hosts(&self, network: &str) -> Result<Vec<NetworkMemberHost>> {
        self.rt
            .block_on(async {
                let rows = sqlx::query(
                    "SELECT id, name, net_ip, state_dir, kind, pid FROM machines
                     WHERE network = ? AND net_ip IS NOT NULL",
                )
                .bind(network)
                .fetch_all(&self.pool)
                .await?;
                Ok::<_, sqlx::Error>(
                    rows.into_iter()
                        .map(|r| {
                            let id: String = r.get("id");
                            let name: Option<String> = r.get("name");
                            let ip: String = r.get("net_ip");
                            let dir: String = r.get("state_dir");
                            let kind: String = r.get("kind");
                            let pid: Option<i64> = r.get("pid");
                            (name.unwrap_or(id), ip, dir, kind, pid)
                        })
                        .collect(),
                )
            })
            .map_err(Into::into)
    }
}

/// A global network: a shared gvproxy switch machines can join to reach each
/// other by IP + name on one subnet.
#[derive(Debug, Clone)]
pub struct NetworkRow {
    pub name: String,
    pub subnet: String,
    pub gateway: String,
    pub control_socket: String,
    pub dir: String,
    pub pid: Option<i64>,
    pub created_at: String,
}

fn row_to_network(r: sqlx::sqlite::SqliteRow) -> NetworkRow {
    NetworkRow {
        name: r.get("name"),
        subnet: r.get("subnet"),
        gateway: r.get("gateway"),
        control_socket: r.get("control_socket"),
        dir: r.get("dir"),
        pid: r.get("pid"),
        created_at: r.get("created_at"),
    }
}

fn row_to_flavor(r: sqlx::sqlite::SqliteRow) -> FlavorRow {
    FlavorRow {
        name: r.get("name"),
        kind: r.get("kind"),
        base: r.get("base"),
        path: r.get("path"),
        description: r.get("description"),
        created_at: r.get("created_at"),
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
        name: r.get("name"),
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
        network: r.get("network"),
        net_ip: r.get("net_ip"),
        ports: r.get("ports"),
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
    name: &str,
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
    ports: Option<&str>,
) {
    if let Err(e) = Db::open().and_then(|db| {
        db.insert_machine(
            id, name, image, kind, command, status, pid, detached, cpus, mem, state_dir, volume,
            ports,
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
