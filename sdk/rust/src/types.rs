//! Typed data structures returned by the SDK.
//!
//! The `*Info` structs mirror the JSON rows emitted by the `bsdkrun` CLI's
//! `--json` output (snake_case field names). The parsers are hand-written over
//! [`serde_json::Value`] rather than derived, because the same struct is fed
//! from two wire shapes: the CLI's rows and the daemon's camelCase GraphQL
//! objects, where timestamps arrive as decimal *strings* (the daemon passes
//! the CLI's own text through unchanged) and numbers may be widened to Float.

use serde_json::Value;

use crate::error::{Error, Result};

// -- lenient Value accessors -------------------------------------------------

fn get_str(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn get_opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn get_bool(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A number, however it arrived: a JSON int, a Float the GraphQL layer widened
/// it to, or a decimal string.
fn get_num(v: &Value, key: &str) -> Option<i64> {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

// -- data types ---------------------------------------------------------------

/// A host to guest TCP port forward.
///
/// `bind` is the host interface the forward is bound to (`127.0.0.1` by
/// default, or `0.0.0.0` for a LAN-reachable forward).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForward {
    pub host: u16,
    pub guest: u16,
    pub bind: String,
}

impl PortForward {
    pub fn from_row(row: &Value) -> PortForward {
        PortForward {
            host: get_num(row, "host").unwrap_or(0) as u16,
            guest: get_num(row, "guest").unwrap_or(0) as u16,
            bind: {
                let bind = get_str(row, "bind");
                if bind.is_empty() {
                    "127.0.0.1".to_string()
                } else {
                    bind
                }
            },
        }
    }
}

/// A machine as reported by `bsdkrun ps --json` (or the daemon's `machines`
/// query).
#[derive(Debug, Clone, PartialEq)]
pub struct SandboxInfo {
    pub id: String,
    pub name: Option<String>,
    pub image: String,
    pub kind: String,
    pub command: String,
    pub status: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub pid: Option<i64>,
    pub detached: bool,
    pub cpus: u32,
    pub mem: u64,
    pub volume: Option<String>,
    pub state_dir: String,
    pub network: Option<String>,
    pub net_ip: Option<String>,
    pub ports: Vec<PortForward>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

impl SandboxInfo {
    /// Build from a CLI `ps --json` row (snake_case).
    pub fn from_row(row: &Value) -> SandboxInfo {
        let running = get_bool(row, "running");
        SandboxInfo {
            id: get_str(row, "id"),
            name: get_opt_str(row, "name"),
            image: get_str(row, "image"),
            kind: get_str(row, "kind"),
            command: get_str(row, "command"),
            status: if running { "running" } else { "exited" }.to_string(),
            running,
            exit_code: get_num(row, "exit_code").map(|v| v as i32),
            pid: get_num(row, "pid"),
            detached: get_bool(row, "detached"),
            cpus: get_num(row, "cpus").unwrap_or(0) as u32,
            mem: get_num(row, "mem").unwrap_or(0) as u64,
            volume: get_opt_str(row, "volume"),
            state_dir: get_str(row, "state_dir"),
            network: get_opt_str(row, "network"),
            net_ip: get_opt_str(row, "net_ip"),
            ports: ports_from(row.get("ports")),
            created_at: get_num(row, "created_at").unwrap_or(0),
            finished_at: get_num(row, "finished_at"),
        }
    }

    /// Build from a GraphQL `Machine` (the `MACHINE_FIELDS` selection).
    pub fn from_graphql(m: &Value) -> SandboxInfo {
        let running = get_bool(m, "running");
        let status = {
            let s = get_str(m, "status");
            if s.is_empty() {
                if running { "running" } else { "exited" }.to_string()
            } else {
                s
            }
        };
        SandboxInfo {
            id: get_str(m, "id"),
            name: get_opt_str(m, "name"),
            image: get_str(m, "image"),
            kind: get_str(m, "kind"),
            command: get_str(m, "command"),
            status,
            running,
            exit_code: get_num(m, "exitCode").map(|v| v as i32),
            pid: get_num(m, "pid"),
            detached: get_bool(m, "detached"),
            cpus: get_num(m, "cpus").unwrap_or(0) as u32,
            mem: get_num(m, "mem").unwrap_or(0) as u64,
            volume: get_opt_str(m, "volume"),
            state_dir: get_str(m, "stateDir"),
            network: get_opt_str(m, "network"),
            net_ip: get_opt_str(m, "netIp"),
            ports: ports_from(m.get("ports")),
            created_at: get_num(m, "createdAt").unwrap_or(0),
            finished_at: get_num(m, "finishedAt"),
        }
    }
}

fn ports_from(v: Option<&Value>) -> Vec<PortForward> {
    v.and_then(Value::as_array)
        .map(|rows| rows.iter().map(PortForward::from_row).collect())
        .unwrap_or_default()
}

/// An image as reported by `bsdkrun images --json`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    pub id: String,
    pub reference: String,
    pub digest: String,
    pub size: u64,
    pub rootfs: String,
    pub created_at: i64,
}

impl ImageInfo {
    pub fn from_row(row: &Value) -> ImageInfo {
        ImageInfo {
            id: get_str(row, "id"),
            reference: get_str(row, "reference"),
            digest: get_str(row, "digest"),
            size: get_num(row, "size").unwrap_or(0) as u64,
            rootfs: get_str(row, "rootfs"),
            created_at: get_num(row, "created_at").unwrap_or(0),
        }
    }
}

/// A volume as reported by `bsdkrun volume ls --json`.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeInfo {
    pub name: String,
    pub guest: Option<String>,
    pub base: Option<String>,
    pub path: String,
    pub size: String,
    pub created_at: Option<i64>,
    pub tracked: bool,
}

impl VolumeInfo {
    pub fn from_row(row: &Value) -> VolumeInfo {
        VolumeInfo {
            name: get_str(row, "name"),
            guest: get_opt_str(row, "guest"),
            base: get_opt_str(row, "base"),
            path: get_str(row, "path"),
            size: get_str(row, "size"),
            created_at: get_num(row, "created_at"),
            tracked: get_bool(row, "tracked"),
        }
    }
}

/// A global network as reported by `bsdkrun network ls --json`.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkInfo {
    pub name: String,
    pub subnet: String,
    pub gateway: String,
    pub members: u32,
    pub running: u32,
    pub up: bool,
    pub created_at: Option<i64>,
}

impl NetworkInfo {
    pub fn from_row(row: &Value) -> NetworkInfo {
        NetworkInfo {
            name: get_str(row, "name"),
            subnet: get_str(row, "subnet"),
            gateway: get_str(row, "gateway"),
            members: get_num(row, "members").unwrap_or(0) as u32,
            running: get_num(row, "running").unwrap_or(0) as u32,
            up: get_bool(row, "up"),
            created_at: get_num(row, "created_at"),
        }
    }
}

/// The outcome of a remote lifecycle mutation (stop/start/remove/update/commit).
///
/// Mirrors the GraphQL `CommandResult` type. A non-zero `exit_code` is
/// reported rather than raised: for some underlying commands (`ssh status`,
/// `tailscale status`) it is a legitimate state to display, not a transport
/// failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandResult {
    pub fn from_graphql(r: &Value) -> CommandResult {
        CommandResult {
            exit_code: get_num(r, "exitCode").unwrap_or(0) as i32,
            stdout: get_str(r, "stdout"),
            stderr: get_str(r, "stderr"),
        }
    }
}

/// A shell session as reported by `openShell` / `shellSessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSessionInfo {
    pub id: String,
    pub machine_id: String,
    pub finished: bool,
    pub truncated: bool,
}

impl ShellSessionInfo {
    pub fn from_graphql(s: &Value) -> ShellSessionInfo {
        ShellSessionInfo {
            id: get_str(s, "id"),
            machine_id: get_str(s, "machineId"),
            finished: get_bool(s, "finished"),
            truncated: get_bool(s, "truncated"),
        }
    }
}

/// The captured result of running a command in a guest through the local CLI.
///
/// Returned by [`crate::Sandbox::exec`] and the command builder's `run()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// A short label for what ran, used in [`Error::CommandFailed`].
    pub command: String,
}

impl ExecResult {
    /// Whether the command succeeded (exit 0).
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }

    /// `stdout` with trailing newlines trimmed — the common case.
    pub fn text(&self) -> &str {
        self.stdout.trim_end_matches('\n')
    }

    /// Parse `stdout` as JSON into any deserializable type.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        Ok(serde_json::from_str(&self.stdout)?)
    }

    /// Non-empty `stdout` lines.
    pub fn lines(&self) -> Vec<String> {
        self.stdout
            .split('\n')
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// A non-zero exit becomes [`Error::CommandFailed`]; exit 0 passes the
    /// result through, so it chains: `sandbox.exec([...])?.ok_or_err()?`.
    pub fn ok_or_err(self) -> Result<ExecResult> {
        if self.exit_code != 0 {
            return Err(Error::CommandFailed {
                exit_code: self.exit_code,
                stdout: self.stdout,
                stderr: self.stderr,
                command: self.command,
            });
        }
        Ok(self)
    }
}

/// The captured result of [`crate::Client::exec`] against a remote daemon.
///
/// Unlike [`ExecResult`] (the local CLI's captured stdout/stderr as text), a
/// remote exec's output is a single interleaved byte stream — the shell
/// agent's `shellOutput` subscription does not distinguish stdout from
/// stderr — so this carries raw bytes instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteExecResult {
    pub exit_code: i32,
    pub output: Vec<u8>,
}

impl RemoteExecResult {
    /// Whether the command succeeded (exit 0).
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }

    /// The output as (lossy) UTF-8 text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sandbox_info_from_row_running() {
        let info = SandboxInfo::from_row(&json!({
            "id": "abc123def456",
            "name": null,
            "image": "alpine",
            "kind": "linux",
            "command": "sleep 300",
            "running": true,
            "exit_code": null,
            "pid": 4242,
            "detached": true,
            "cpus": 2,
            "mem": 1024,
            "volume": null,
            "state_dir": "/var/lib/bsdkrun/abc123",
            "network": "devnet",
            "net_ip": "192.168.127.3",
            "created_at": 1700000000i64,
            "finished_at": null,
        }));
        assert_eq!(info.status, "running");
        assert!(info.running);
        assert_eq!(info.exit_code, None);
        assert_eq!(info.pid, Some(4242));
        assert_eq!(info.network.as_deref(), Some("devnet"));
    }

    #[test]
    fn sandbox_info_from_row_exited_defaults() {
        let info = SandboxInfo::from_row(&json!({
            "id": "abc",
            "image": "alpine",
            "kind": "linux",
            "running": false,
            "exit_code": 0,
            "detached": true,
            "cpus": 1,
            "mem": 512,
            "state_dir": "/s",
            "created_at": 1,
            "finished_at": 2,
        }));
        assert_eq!(info.status, "exited");
        assert_eq!(info.command, "");
        assert_eq!(info.finished_at, Some(2));
    }

    #[test]
    fn sandbox_info_from_graphql_maps_camel_case_and_string_timestamps() {
        let info = SandboxInfo::from_graphql(&json!({
            "id": "abc123def456",
            "name": null,
            "image": "alpine",
            "kind": "linux",
            "command": "sleep 300",
            "status": "running",
            "running": true,
            "exitCode": null,
            "pid": 4242,
            "detached": true,
            "cpus": 2,
            "mem": 1024,
            "volume": null,
            "stateDir": "/var/lib/bsdkrun/abc123",
            "network": "devnet",
            "netIp": "192.168.127.3",
            "ports": [{"bind": "0.0.0.0", "host": 2222, "guest": 22}],
            "createdAt": "1700000000",
            "finishedAt": null,
        }));
        assert_eq!(info.state_dir, "/var/lib/bsdkrun/abc123");
        assert_eq!(info.net_ip.as_deref(), Some("192.168.127.3"));
        assert_eq!(info.created_at, 1700000000);
        assert_eq!(info.finished_at, None);
        assert_eq!(
            info.ports,
            vec![PortForward {
                host: 2222,
                guest: 22,
                bind: "0.0.0.0".into()
            }]
        );
    }

    #[test]
    fn graphql_floats_coerce_back_to_ints() {
        // GraphQL widens pid/mem to Float to dodge 32-bit Int overflow; the
        // mapping must still land on plain integers.
        let info = SandboxInfo::from_graphql(&json!({
            "id": "abc",
            "image": "alpine",
            "kind": "linux",
            "command": "",
            "status": "exited",
            "running": false,
            "exitCode": 0,
            "pid": 4242.0,
            "detached": true,
            "cpus": 1,
            "mem": 512.0,
            "stateDir": "/s",
            "createdAt": "1",
            "finishedAt": "2",
        }));
        assert_eq!(info.pid, Some(4242));
        assert_eq!(info.mem, 512);
        assert_eq!(info.finished_at, Some(2));
    }

    #[test]
    fn volume_and_network_rows() {
        let vol = VolumeInfo::from_row(&json!({
            "name": "web", "path": "/p", "size": "1G", "tracked": true
        }));
        assert_eq!(vol.name, "web");
        assert_eq!(vol.guest, None);
        assert_eq!(vol.created_at, None);

        let net = NetworkInfo::from_row(&json!({
            "name": "devnet",
            "subnet": "192.168.127.0/24",
            "gateway": "192.168.127.1",
            "members": 2,
            "running": 1,
            "up": true,
        }));
        assert_eq!(net.members, 2);
        assert!(net.up);
    }

    #[test]
    fn command_result_defaults_sanely() {
        let full = CommandResult::from_graphql(&json!({
            "exitCode": 1, "stdout": "out", "stderr": "err"
        }));
        assert_eq!((full.exit_code, full.stdout.as_str()), (1, "out"));

        let empty = CommandResult::from_graphql(&json!({}));
        assert_eq!(empty.exit_code, 0);
        assert_eq!(empty.stdout, "");
    }

    #[test]
    fn shell_session_info_from_graphql() {
        let s = ShellSessionInfo::from_graphql(&json!({
            "id": "sess-1", "machineId": "abc123", "finished": false, "truncated": true
        }));
        assert_eq!(s.id, "sess-1");
        assert_eq!(s.machine_id, "abc123");
        assert!(!s.finished);
        assert!(s.truncated);
    }

    #[test]
    fn exec_result_helpers() {
        let ok = ExecResult {
            stdout: "hello\n\n".into(),
            stderr: String::new(),
            exit_code: 0,
            command: "echo".into(),
        };
        assert!(ok.ok());
        assert_eq!(ok.text(), "hello");
        assert_eq!(ok.lines(), vec!["hello".to_string()]);
        assert!(ok.ok_or_err().is_ok());

        let failed = ExecResult {
            stdout: String::new(),
            stderr: "boom".into(),
            exit_code: 1,
            command: "false".into(),
        };
        assert!(!failed.ok());
        assert!(matches!(
            failed.ok_or_err(),
            Err(Error::CommandFailed { exit_code: 1, .. })
        ));
    }

    #[test]
    fn exec_result_json_parses_stdout() {
        let r = ExecResult {
            stdout: "{\"a\": 1}".into(),
            stderr: String::new(),
            exit_code: 0,
            command: "cat".into(),
        };
        let v: serde_json::Value = r.json().unwrap();
        assert_eq!(v["a"], 1);
    }
}
