//! Local `Sandbox` behavior against a **stub** `bsdkrun` — a shell script
//! that records its argv and prints scripted output. The same hermetic
//! approach the daemon's own test suites use: no hypervisor, no VM boots, and
//! every test can assert the exact argv produced, which is the part most
//! likely to break.
//!
//! Binary resolution is process-global state, so every test here serializes
//! on one lock and resets the cache when done.

#![cfg(unix)]

mod support;

use std::path::PathBuf;
use std::sync::Mutex;

use bsdkrun_sdk::{reset_binary_cache, set_binary_path, Error, Sandbox};

static STUB_LOCK: Mutex<()> = Mutex::new(());

struct Stub {
    dir: PathBuf,
}

impl Stub {
    /// Install a stub whose body is `script` (argv is in `"$@"`; the recorded
    /// argv file path is in `$ARGS_FILE`).
    fn install(name: &str, script: &str) -> Stub {
        let dir =
            std::env::temp_dir().join(format!("bsdkrun-rust-sdk-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let args_file = dir.join("argv.txt");
        let bin = dir.join("bsdkrun");
        let body = format!(
            "#!/bin/sh\nARGS_FILE={}\nprintf '%s\\n' \"$@\" > \"$ARGS_FILE\"\n{script}\n",
            shell_quote(&args_file)
        );
        std::fs::write(&bin, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        set_binary_path(bin.to_string_lossy().into_owned());
        Stub { dir }
    }

    /// The argv the stub was last called with (one entry per line).
    fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("argv.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        reset_binary_cache();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[test]
fn create_parses_the_machine_id_and_ssh_port() {
    let _guard = STUB_LOCK.lock().unwrap();
    let stub = Stub::install(
        "create",
        // Boot noise on stderr, the id on stdout — like a real detached run.
        "echo 'booting...' >&2\n\
         echo '  connect with: ssh -p 2222 root@localhost' >&2\n\
         echo fab8f81e4f91",
    );

    let sandbox = Sandbox::linux("alpine")
        .cpus(2)
        .mem(1024)
        .command(["sleep", "300"])
        .create()
        .unwrap();
    assert_eq!(sandbox.id(), "fab8f81e4f91");
    assert_eq!(sandbox.ssh_port(), Some(2222));

    // Global flags first (create defaults to --log-level 1), then the argv
    // the builder assembled.
    assert_eq!(
        stub.argv(),
        vec![
            "--log-level",
            "1",
            "linux",
            "alpine",
            "-d",
            "--cpus",
            "2",
            "--mem",
            "1024",
            "--",
            "sleep",
            "300",
        ]
    );
}

#[test]
fn create_without_an_id_in_output_is_a_command_failure() {
    let _guard = STUB_LOCK.lock().unwrap();
    let _stub = Stub::install("no-id", "echo 'pulling alpine:3.20'");
    let err = Sandbox::linux("alpine").create().unwrap_err();
    assert!(matches!(err, Error::CommandFailed { .. }), "{err:?}");
    assert!(err.to_string().contains("no machine id"));
}

#[test]
fn exec_builds_the_exec_argv_and_captures_output() {
    let _guard = STUB_LOCK.lock().unwrap();
    let stub = Stub::install("exec", "echo 'Linux vm 6.6'");

    let sandbox = Sandbox::from_id("fab8f81e4f91");
    let result = sandbox
        .command("node")
        .args(["-e", "console.log(1)"])
        .env("X", "hi")
        .cwd("/app")
        .tty(true)
        .run()
        .unwrap();

    assert!(result.ok());
    assert_eq!(result.text(), "Linux vm 6.6");
    assert_eq!(
        stub.argv(),
        vec![
            "--log-level",
            "0",
            "exec",
            "-t",
            "-e",
            "X=hi",
            "fab8f81e4f91",
            "/bin/sh",
            "-c",
            "cd \"$1\" && shift && exec \"$@\"",
            "sh",
            "/app",
            "node",
            "-e",
            "console.log(1)",
        ]
    );
}

#[test]
fn exec_shorthand_reports_nonzero_exit_as_data() {
    let _guard = STUB_LOCK.lock().unwrap();
    let _stub = Stub::install("fail", "echo oops >&2\nexit 3");

    let sandbox = Sandbox::from_id("fab8f81e4f91");
    let result = sandbox.exec(["false"]).unwrap();
    assert!(!result.ok());
    assert_eq!(result.exit_code, 3);
    // ...until the caller opts into raising.
    let err = result.ok_or_err().unwrap_err();
    assert!(matches!(err, Error::CommandFailed { exit_code: 3, .. }));
}

#[test]
fn get_matches_an_id_prefix_and_list_parses_rows() {
    let _guard = STUB_LOCK.lock().unwrap();
    let _stub = Stub::install(
        "ps",
        r#"echo '[{"id":"fab8f81e4f91","image":"alpine","kind":"linux","running":true,"cpus":1,"mem":512,"state_dir":"/s","created_at":1}]'"#,
    );

    let rows = Sandbox::list(true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].running);

    let sandbox = Sandbox::get("fab8f8").unwrap();
    assert_eq!(sandbox.id(), "fab8f81e4f91");

    assert!(matches!(
        Sandbox::get("000000"),
        Err(Error::SandboxNotFound { .. })
    ));
}

#[test]
fn ssh_setup_and_tailscale_build_agent_argv() {
    let _guard = STUB_LOCK.lock().unwrap();
    let stub = Stub::install("ssh", "echo done");

    let sandbox = Sandbox::from_id("fab8f81e4f91");
    sandbox
        .ssh_setup()
        .user("tsiry")
        .key("~/.ssh/work.pub")
        .run()
        .unwrap();
    assert_eq!(
        stub.argv(),
        vec![
            "--log-level",
            "0",
            "ssh",
            "fab8f81e4f91",
            "setup",
            "--user",
            "tsiry",
            "--key",
            "~/.ssh/work.pub",
        ]
    );

    sandbox
        .tailscale_up()
        .authkey("tskey-auth-xyz")
        .hostname("web")
        .run()
        .unwrap();
    assert_eq!(
        stub.argv(),
        vec![
            "--log-level",
            "0",
            "tailscale",
            "fab8f81e4f91",
            "setup",
            "--hostname",
            "web",
        ],
        "the authkey must ride in TS_AUTHKEY, never on the argv"
    );
}

#[test]
fn update_builder_emits_only_the_set_fields() {
    let _guard = STUB_LOCK.lock().unwrap();
    let stub = Stub::install("update", "true");

    Sandbox::from_id("abc123").update().cpus(4).apply().unwrap();
    assert_eq!(
        stub.argv(),
        vec!["--log-level", "0", "update", "abc123", "--cpus", "4"]
    );
}

#[test]
fn stdin_is_piped_to_the_guest_command() {
    let _guard = STUB_LOCK.lock().unwrap();
    let _stub = Stub::install("stdin", "cat"); // echo stdin back
    let sandbox = Sandbox::from_id("abc123");
    let result = sandbox.command("wc").stdin("data on stdin").run().unwrap();
    assert_eq!(result.stdout, "data on stdin");
}
