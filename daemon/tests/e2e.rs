//! End-to-end tests: a real gRPC server over a real socket, driven by the real
//! generated client.
//!
//! The daemon is driven against a **stub** `bsdkrun` rather than the real one,
//! for two reasons. It makes the tests hermetic and fast — no VM boots, no
//! image downloads, nothing that needs a hypervisor, so they run on any CI
//! runner. And it lets each test assert the exact argv the daemon produced,
//! which is the part most likely to break: the whole service is a translation
//! layer from proto messages to command lines, and a wrong flag would otherwise
//! only surface as a mysterious failure against a real machine.
//!
//! What is covered: token authentication, the typed RPCs and their JSON
//! parsing, argv construction, output streaming, exit-code propagation, and
//! interactive sessions over both a pty and plain pipes.

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use bsdkrun_daemon::auth::TokenAuth;
use bsdkrun_daemon::cli::Cli;
use bsdkrun_daemon::client::{self, RemoteConfig};
use bsdkrun_daemon::pb::bsdkrun_server::BsdkrunServer;
use bsdkrun_daemon::pb::*;
use bsdkrun_daemon::service::BsdkrunService;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_stream::StreamExt;
use tonic::transport::Server;

const TOKEN: &str = "test-token-0123456789";

/// A stub standing in for the `bsdkrun` CLI.
///
/// It records every argv it is called with (one argument per line, `---`
/// between invocations) so tests can assert the exact command line, and answers
/// the subcommands under test with fixtures shaped like the real CLI's output.
const STUB: &str = r#"#!/bin/sh
# The log lives beside the stub rather than coming from the environment: each
# test gets its own temp dir, and tests run in parallel in one process, so a
# shared env var would have them all writing to whichever log was set last.
LOG="$0.log"
for a in "$@"; do printf '%s\n' "$a" >> "$LOG"; done
printf -- '---\n' >> "$LOG"

case "$1 $2" in
"--version "*)
  echo "bsdkrun 9.9.9-stub"; exit 0 ;;
esac

case "$1" in
ps)
  echo '[{"id":"abc123","name":"web","image":"alpine","kind":"linux","command":"sh","status":"running","running":true,"exit_code":null,"pid":42,"detached":true,"cpus":2,"mem":1024,"volume":null,"state_dir":"/s","created_at":"1785993650","finished_at":null,"network":"devnet","net_ip":"192.168.127.7"}]'
  exit 0 ;;
images)
  echo '[{"id":"img1","reference":"alpine:3.20","digest":"sha256:deadbeef","size":28886818,"rootfs":"/r","created_at":"1785854268"}]'
  exit 0 ;;
flavors)
  echo '[{"name":"node","source":"catalog","kind":"linux","base":"node:22","category":"language","method":"docker","description":"Node","ports":["3000:3000"],"nix":[],"created_at":null}]'
  exit 0 ;;
volume)
  # `volume ls --json` reports size as human text, and "-" when unknown.
  echo '[{"name":"data","guest":"linux","base":"b.img","path":"/v/data","size":"2.3 GiB","created_at":"1785847926","tracked":true},{"name":"empty","guest":null,"base":null,"path":"/v/empty","size":"-","created_at":null,"tracked":false}]'
  exit 0 ;;
network)
  echo '[{"name":"devnet","subnet":"192.168.127.0/24","gateway":"192.168.127.1","members":6,"running":4,"up":true,"created_at":"1785868258"}]'
  exit 0 ;;
versions)
  printf 'Available builds:\n  14.3\n  15.1  (latest)\n'
  exit 0 ;;
linux|freebsd|netbsd)
  # A boot command prints the new machine id on stdout and exits.
  echo "m-$1-001"; exit 0 ;;
flavor)
  [ "$2" = run ] && { echo "m-flavor-001"; exit 0; }
  exit 0 ;;
logs)
  echo "line one"; echo "line two"; exit 0 ;;
exec)
  # Echo back a marker plus whether a tty was requested, then read stdin so the
  # interactive tests have something to exercise.
  echo "EXEC_OK"
  [ "$2" = "-t" ] && echo "TTY_REQUESTED"
  cat
  exit 0 ;;
stop|start|rm|update|commit)
  echo "$1 ok"; exit 0 ;;
probe)
  echo "probe ok"; exit 0 ;;
fail)
  echo "boom" >&2; exit 3 ;;
esac

echo "unknown subcommand: $1" >&2
exit 2
"#;

struct Harness {
    addr: SocketAddr,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("bsdkrun");
        std::fs::write(&stub, STUB).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = dir.path().join("bsdkrun.log");
        std::fs::write(&log, "").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let cli = Cli::resolve(Some(stub)).unwrap();
        let svc = BsdkrunServer::with_interceptor(
            BsdkrunService::new(cli),
            TokenAuth::new(TOKEN.to_string()),
        );
        tokio::spawn(async move {
            Server::builder()
                .add_service(svc)
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        Self {
            addr,
            log,
            _dir: dir,
        }
    }

    async fn client(&self) -> client::Client {
        self.connect_with(TOKEN).await.expect("connect")
    }

    async fn connect_with(&self, token: &str) -> anyhow::Result<client::Client> {
        client::connect(&RemoteConfig {
            endpoint: format!("http://{}", self.addr),
            token: token.to_string(),
        })
        .await
    }

    /// Every recorded invocation, each as its argv.
    fn invocations(&self) -> Vec<Vec<String>> {
        let raw = std::fs::read_to_string(&self.log).unwrap_or_default();
        let mut all = Vec::new();
        let mut cur = Vec::new();
        for line in raw.lines() {
            if line == "---" {
                all.push(std::mem::take(&mut cur));
            } else {
                cur.push(line.to_string());
            }
        }
        all
    }

    /// The argv of the last invocation that is not the `--version` probe.
    fn last_argv(&self) -> Vec<String> {
        self.invocations()
            .into_iter()
            .rfind(|a| a.first().map(|s| s != "--version").unwrap_or(false))
            .expect("expected at least one recorded invocation")
    }
}

/// Collect a whole output stream into (stdout, stderr, exit code).
async fn drain(mut stream: tonic::Streaming<OutputChunk>) -> (String, String, Option<i32>) {
    let (mut out, mut err, mut code) = (Vec::new(), Vec::new(), None);
    while let Some(chunk) = stream.next().await {
        match chunk.unwrap().payload {
            Some(output_chunk::Payload::Stdout(b)) => out.extend_from_slice(&b),
            Some(output_chunk::Payload::Stderr(b)) => err.extend_from_slice(&b),
            Some(output_chunk::Payload::ExitCode(c)) => code = Some(c),
            None => {}
        }
    }
    (
        String::from_utf8_lossy(&out).into_owned(),
        String::from_utf8_lossy(&err).into_owned(),
        code,
    )
}

// ---------------------------------------------------------------------------
// authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rejects_a_missing_token() {
    let h = Harness::start().await;
    // Bypass the client helper so no authorization header is attached at all.
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{}", h.addr))
        .unwrap()
        .connect()
        .await
        .unwrap();
    let mut raw = bsdkrun_daemon::pb::bsdkrun_client::BsdkrunClient::new(channel);
    let err = raw.info(InfoRequest {}).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert!(err.message().contains("missing token"), "{}", err.message());
}

#[tokio::test]
async fn rejects_a_wrong_token() {
    let h = Harness::start().await;
    let mut c = h.connect_with("not-the-token").await.unwrap();
    let err = c.info(InfoRequest {}).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(err.message(), "invalid token");
}

#[tokio::test]
async fn accepts_the_right_token() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    let info = c.info(InfoRequest {}).await.unwrap().into_inner();
    assert_eq!(info.cli_version, "bsdkrun 9.9.9-stub");
    assert_eq!(info.daemon_version, env!("CARGO_PKG_VERSION"));
    assert!(!info.arch.is_empty());
}

// ---------------------------------------------------------------------------
// typed RPCs and JSON parsing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lists_machines() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    let res = c
        .list_machines(ListMachinesRequest { all: true })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(res.machines.len(), 1);
    let m = &res.machines[0];
    assert_eq!(m.id, "abc123");
    assert_eq!(m.name.as_deref(), Some("web"));
    assert!(m.running);
    assert_eq!(m.cpus, Some(2));
    assert_eq!(m.net_ip.as_deref(), Some("192.168.127.7"));
    assert_eq!(h.last_argv(), ["ps", "-a", "--json"]);
}

#[tokio::test]
async fn list_machines_without_all_omits_the_flag() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    c.list_machines(ListMachinesRequest { all: false })
        .await
        .unwrap();
    assert_eq!(h.last_argv(), ["ps", "--json"]);
}

#[tokio::test]
async fn lists_images_and_flavors_and_networks() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let imgs = c
        .list_images(ListImagesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(imgs.images[0].reference, "alpine:3.20");
    assert_eq!(imgs.images[0].size, 28886818);

    let fl = c
        .list_flavors(ListFlavorsRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(fl.flavors[0].name, "node");
    assert_eq!(fl.flavors[0].ports, ["3000:3000"]);

    let nets = c
        .list_networks(ListNetworksRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(nets.networks[0].subnet, "192.168.127.0/24");
    assert_eq!(nets.networks[0].members, 6);
}

#[tokio::test]
async fn volume_size_is_text_and_unknown_becomes_absent() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    let res = c
        .list_volumes(ListVolumesRequest {})
        .await
        .unwrap()
        .into_inner();

    assert_eq!(res.volumes[0].size.as_deref(), Some("2.3 GiB"));
    // The CLI writes "-" when it cannot measure a volume; that is not a size.
    assert_eq!(res.volumes[1].size, None);
    assert_eq!(h.last_argv(), ["volume", "ls", "--json"]);
}

#[tokio::test]
async fn parses_the_versions_listing() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    let res = c
        .list_versions(ListVersionsRequest {
            os: BsdOs::Freebsd as i32,
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(res.versions.len(), 2);
    assert_eq!(res.versions[0].version, "14.3");
    assert!(!res.versions[0].latest);
    assert_eq!(res.versions[1].version, "15.1");
    assert!(res.versions[1].latest);
    assert_eq!(h.last_argv(), ["versions", "--os", "freebsd"]);
}

#[tokio::test]
async fn rejects_an_unspecified_os() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    let err = c
        .list_versions(ListVersionsRequest {
            os: BsdOs::Unspecified as i32,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// ---------------------------------------------------------------------------
// argv construction for the boot RPCs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_linux_builds_the_expected_command_line() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let res = c
        .run_linux(RunLinuxRequest {
            image: "alpine:3.20".into(),
            vm: Some(VmConfig { cpus: 4, mem: 2048 }),
            net: Some(NetConfig {
                no_net: false,
                ports: vec!["8080:80".into()],
                mac: None,
                network: Some("devnet".into()),
                name: Some("web".into()),
            }),
            volume: Some("data".into()),
            mounts: vec!["/host:/guest:ro".into()],
            env: vec!["A=1".into(), "B=2".into()],
            entrypoint: None,
            initramfs: false,
            kernel: None,
            kernel_version: None,
            console: None,
            repo: None,
            command: vec!["sh".into(), "-c".into(), "echo hi".into()],
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(res.id, "m-linux-001");
    assert_eq!(
        h.last_argv(),
        [
            "linux",
            "-d",
            "--cpus",
            "4",
            "--mem",
            "2048",
            "--port",
            "8080:80",
            "--network",
            "devnet",
            "--name",
            "web",
            "-v",
            "data",
            "--mount",
            "/host:/guest:ro",
            "-e",
            "A=1",
            "-e",
            "B=2",
            "alpine:3.20",
            "--",
            "sh",
            "-c",
            "echo hi",
        ]
    );
}

#[tokio::test]
async fn run_bsd_is_always_detached_and_maps_the_os() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let res = c
        .run_bsd(RunBsdRequest {
            os: BsdOs::Netbsd as i32,
            version: Some("10.1".into()),
            vm: Some(VmConfig { cpus: 0, mem: 512 }),
            net: Some(NetConfig {
                no_net: true,
                ports: vec![],
                mac: None,
                network: None,
                name: None,
            }),
            volume: None,
            persist: true,
            force: false,
            firmware: None,
            attach_disk: vec!["/d.img:ro".into()],
            disk_size: Some("8G".into()),
            repo: None,
            command: vec![],
        })
        .await
        .unwrap()
        .into_inner();

    assert_eq!(res.id, "m-netbsd-001");
    assert_eq!(
        h.last_argv(),
        [
            "netbsd",
            "-d",
            // cpus == 0 means "unset", so no --cpus is emitted.
            "--mem",
            "512",
            "--no-net",
            "--version",
            "10.1",
            "--persist",
            "--attach-disk",
            "/d.img:ro",
            "--disk-size",
            "8G",
        ]
    );
}

#[tokio::test]
async fn empty_repeated_fields_are_rejected_rather_than_run() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let err = c
        .remove_machines(RemoveMachinesRequest {
            ids: vec![],
            force: true,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    // An empty id list would otherwise become a bare `rm -f`, which the CLI
    // could interpret far more broadly than the caller intended.
    assert!(h
        .invocations()
        .iter()
        .all(|a| a.first().map(|s| s != "rm").unwrap_or(true)));
}

// ---------------------------------------------------------------------------
// streaming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logs_stream_and_end_with_an_exit_code() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    let stream = c
        .logs(LogsRequest {
            id: "abc123".into(),
            follow: true,
            boot: false,
        })
        .await
        .unwrap()
        .into_inner();

    let (out, _, code) = drain(stream).await;
    assert!(out.contains("line one"), "{out}");
    assert!(out.contains("line two"), "{out}");
    assert_eq!(code, Some(0));
    assert_eq!(h.last_argv(), ["logs", "-f", "abc123"]);
}

#[tokio::test]
async fn a_nonzero_exit_is_reported_not_raised() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    // Driven through the generic passthrough, which is also the escape hatch
    // for every subcommand without a typed RPC.
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(RunInput {
        payload: Some(run_input::Payload::Start(RunStart {
            args: vec!["fail".into()],
            tty: false,
            size: None,
        })),
    })
    .await
    .unwrap();
    drop(tx);

    let stream = c
        .run(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    let (_, err, code) = drain(stream).await;
    assert!(err.contains("boom"), "{err}");
    assert_eq!(code, Some(3));
}

/// A client with nothing to send half-closes its request stream immediately.
/// That means "no more stdin", never "cancel" — regressing this made every
/// non-interactive call return zero frames.
#[tokio::test]
async fn a_half_closed_request_stream_does_not_cancel_the_command() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let stream = tokio_stream::once(RunInput {
        payload: Some(run_input::Payload::Start(RunStart {
            args: vec!["probe".into()],
            tty: false,
            size: None,
        })),
    });

    let out_stream = c.run(stream).await.unwrap().into_inner();
    let (out, _, code) = drain(out_stream).await;
    assert!(out.contains("probe ok"), "{out}");
    assert_eq!(code, Some(0));
}

// ---------------------------------------------------------------------------
// interactive sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exec_without_a_tty_streams_stdin_through_a_pipe() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(ExecInput {
        payload: Some(exec_input::Payload::Start(ExecStart {
            id: "abc123".into(),
            command: vec!["cat".into()],
            env: vec!["K=V".into()],
            tty: false,
            size: None,
        })),
    })
    .await
    .unwrap();
    tx.send(ExecInput {
        payload: Some(exec_input::Payload::Stdin(b"piped-input\n".to_vec())),
    })
    .await
    .unwrap();
    tx.send(ExecInput {
        payload: Some(exec_input::Payload::StdinEof(true)),
    })
    .await
    .unwrap();
    drop(tx);

    let stream = c
        .exec(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    let (out, _, code) = drain(stream).await;

    assert!(out.contains("EXEC_OK"), "{out}");
    // The stub `cat`s its stdin back, proving the pipe carried the bytes.
    assert!(out.contains("piped-input"), "{out}");
    assert!(!out.contains("TTY_REQUESTED"), "{out}");
    assert_eq!(code, Some(0));
    assert_eq!(h.last_argv(), ["exec", "-e", "K=V", "abc123", "cat"]);
}

#[tokio::test]
async fn exec_with_a_tty_runs_the_cli_under_a_pty() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tx.send(ExecInput {
        payload: Some(exec_input::Payload::Start(ExecStart {
            id: "abc123".into(),
            command: vec!["cat".into()],
            env: vec![],
            tty: true,
            size: Some(Resize {
                rows: 40,
                cols: 120,
            }),
        })),
    })
    .await
    .unwrap();
    tx.send(ExecInput {
        payload: Some(exec_input::Payload::Stdin(b"typed\n".to_vec())),
    })
    .await
    .unwrap();
    tx.send(ExecInput {
        payload: Some(exec_input::Payload::Resize(Resize {
            rows: 50,
            cols: 100,
        })),
    })
    .await
    .unwrap();
    tx.send(ExecInput {
        payload: Some(exec_input::Payload::StdinEof(true)),
    })
    .await
    .unwrap();
    drop(tx);

    let stream = c
        .exec(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
        .unwrap()
        .into_inner();
    let (out, _, code) = drain(stream).await;

    assert!(out.contains("EXEC_OK"), "{out}");
    assert!(out.contains("TTY_REQUESTED"), "{out}");
    assert_eq!(code, Some(0));
    assert_eq!(h.last_argv(), ["exec", "-t", "abc123", "cat"]);
}

/// An empty command means "open this machine's shell", which only makes sense
/// on a terminal — so it maps to `shell` and implies a tty.
#[tokio::test]
async fn exec_with_no_command_opens_a_shell() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let stream = tokio_stream::once(ExecInput {
        payload: Some(exec_input::Payload::Start(ExecStart {
            id: "abc123".into(),
            command: vec![],
            env: vec![],
            tty: false, // deliberately false: a shell implies a tty regardless
            size: None,
        })),
    });

    let out_stream = c.exec(stream).await.unwrap().into_inner();
    let _ = drain(out_stream).await;
    assert_eq!(h.last_argv(), ["shell", "abc123"]);
}

#[tokio::test]
async fn exec_requires_start_as_the_first_message() {
    let h = Harness::start().await;
    let mut c = h.client().await;

    let stream = tokio_stream::once(ExecInput {
        payload: Some(exec_input::Payload::Stdin(b"too soon".to_vec())),
    });
    let err = c.exec(stream).await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

// ---------------------------------------------------------------------------
// the shipped binary
// ---------------------------------------------------------------------------

/// The generated token is the operator's only copy, so it must actually reach
/// stdout rather than only the log.
#[test]
fn the_binary_generates_and_prints_a_token() {
    let exe = daemon_binary();
    let dir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("bsdkrun");
    std::fs::write(&stub, STUB).unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut child = std::process::Command::new(exe)
        // Port 0 lets the OS pick, so concurrent test runs cannot collide.
        .args(["--bind", "127.0.0.1:0", "--bsdkrun"])
        .arg(&stub)
        .env_remove("BSDKRUN_TOKEN")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    // Read just the banner; the daemon then keeps running, so never wait on EOF.
    let mut banner = String::new();
    {
        use std::io::{BufRead, BufReader};
        let out = child.stdout.take().unwrap();
        let mut reader = BufReader::new(out);
        for _ in 0..12 {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            banner.push_str(&line);
            if banner.contains("BSDKRUN_TOKEN=") {
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        banner.contains("access token (generated"),
        "banner was: {banner}"
    );
    // 32 random bytes, hex-encoded.
    let token = banner
        .lines()
        .map(str::trim)
        .find(|l| l.len() == 64 && l.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| panic!("no 64-char hex token in banner: {banner}"));
    assert_eq!(token.len(), 64);
}

/// Locate the `bsdkrund` binary next to the test executable.
fn daemon_binary() -> PathBuf {
    let mut dir = std::env::current_exe().expect("test exe");
    dir.pop(); // deps/
    if dir.ends_with("deps") {
        dir.pop();
    }
    let exe = dir.join("bsdkrund");
    assert!(
        Path::new(&exe).exists(),
        "bsdkrund not built at {}; run `cargo build --bins` first",
        exe.display()
    );
    exe
}
