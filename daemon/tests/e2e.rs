//! End-to-end tests: a real gRPC server over a real socket, driven by the real
//! generated client.
//!
//! Two things are stubbed so the tests stay hermetic — no VM boots, no image
//! downloads, nothing that needs a hypervisor, so they run on any CI runner:
//!
//!   * **State.** Reads (`ps`, `images`, `volume ls`, …) now run *in* the
//!     daemon against the engine, so the tests point `BSDKRUN_STATE` at a temp
//!     directory and seed a real database with fixtures.
//!   * **The supervisor.** Booting and streaming still run in a separate
//!     process, so a stub stands in for it and records the command it was
//!     handed. Those recordings are JSON-encoded `Command`s rather than argv,
//!     which is what makes them worth asserting: a test now pins the *meaning*
//!     of a request rather than the spelling of a flag.
//!
//! What is covered: token authentication, the typed RPCs and their decoding,
//! command construction, output streaming, exit-code propagation, and
//! interactive sessions over both a pty and plain pipes.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

mod common;
use common::{decode, fixture_state, install_stub};

use bsdkrun_daemon::auth::TokenAuth;
use bsdkrun_daemon::client::{self, RemoteConfig};
use bsdkrun_daemon::pb::bsdkrun_server::BsdkrunServer;
use bsdkrun_daemon::pb::*;
use bsdkrun_daemon::service::BsdkrunService;
use bsdkrun_daemon::supervisor::Supervisor;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_stream::StreamExt;
use tonic::transport::Server;

const TOKEN: &str = "test-token-0123456789";

/// A stub standing in for the supervisor — this daemon's own binary, re-entered
/// as `__run <json>` or `__cli -- <args…>`.
///
/// It records every argv it is called with (one argument per line, `---`
/// between invocations) so a test can decode the command it was handed, and
/// answers the operations under test with fixtures shaped like real output.
/// The `case` patterns match the externally-tagged JSON serde produces for
/// `Command`, e.g. `{"Linux":{…}}`.
struct Harness {
    addr: SocketAddr,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start() -> Self {
        fixture_state();
        let dir = tempfile::tempdir().unwrap();
        let (stub, log) = install_stub(dir.path());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let svc = BsdkrunServer::with_interceptor(
            BsdkrunService::new(Supervisor::with_exe(stub)),
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
        common::invocations(&self.log)
    }

    /// The argv of the last recorded invocation.
    fn last_argv(&self) -> Vec<String> {
        self.invocations()
            .into_iter()
            .next_back()
            .expect("expected at least one recorded invocation")
    }

    /// The command the daemon handed the supervisor last, decoded.
    ///
    /// This is the modern form of "assert the argv": it pins what was asked
    /// for, and it cannot be satisfied by a flag that merely looks right.
    fn last_command(&self) -> serde_json::Value {
        decode(&self.last_argv())
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
    // The engine is linked in, so its version is reported directly rather than
    // read back out of another binary.
    assert_eq!(info.cli_version, bsdkrun_core::VERSION);
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

    // Three seeded machines, one of them stopped.
    assert_eq!(res.machines.len(), 3);
    let m = res.machines.iter().find(|m| m.id == "abc123").unwrap();
    assert_eq!(m.name.as_deref(), Some("web"));
    assert!(m.running);
    assert_eq!(m.cpus, Some(2));
    assert_eq!(m.network.as_deref(), Some("devnet"));
    // No process was involved at all: the listing came from the engine.
    assert!(h.invocations().is_empty(), "a read must not spawn anything");
}

#[tokio::test]
async fn list_machines_without_all_lists_only_running_ones() {
    let h = Harness::start().await;
    let mut c = h.client().await;
    let res = c
        .list_machines(ListMachinesRequest { all: false })
        .await
        .unwrap()
        .into_inner();
    // The stopped fixture is left out; the running ones are there. Asserted by
    // membership rather than by count, since tests run in parallel.
    assert!(res.machines.iter().all(|m| m.running));
    let ids: Vec<&str> = res.machines.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"abc123"), "{ids:?}");
    assert!(!ids.contains(&"dead00000001"), "{ids:?}");
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
    let img = imgs
        .images
        .iter()
        .find(|i| i.reference == "alpine:3.20")
        .expect("the seeded image");
    // Larger than a 32-bit int, so the wire type is exercised for real.
    assert_eq!(img.size, 5_000_000_000);

    let fl = c
        .list_flavors(ListFlavorsRequest {})
        .await
        .unwrap()
        .into_inner();
    // The catalog is compiled into the engine, so it needs no fixture.
    let node = fl
        .flavors
        .iter()
        .find(|f| f.name == "node")
        .expect("the built-in node flavor");
    assert_eq!(node.source, "catalog");
    assert_eq!(node.kind, "linux");

    let nets = c
        .list_networks(ListNetworksRequest {})
        .await
        .unwrap()
        .into_inner();
    let dev = nets
        .networks
        .iter()
        .find(|n| n.name == "devnet")
        .expect("the seeded network");
    assert_eq!(dev.subnet, "192.168.127.0/24");
    // One machine was connected to it, and its pid is alive.
    assert_eq!(dev.members, 1);
    assert_eq!(dev.running, 1);
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

    let data = res.volumes.iter().find(|v| v.name == "data").unwrap();
    let gone = res.volumes.iter().find(|v| v.name == "gone").unwrap();
    // A measurable volume reports human-readable text…
    let size = data
        .size
        .as_deref()
        .expect("a measurable volume has a size");
    assert!(
        size.ends_with("B") && size.chars().next().unwrap().is_ascii_digit(),
        "unexpected size text: {size:?}"
    );
    // …and one whose directory is gone reports no size at all, rather than the
    // "-" placeholder the table prints.
    assert_eq!(gone.size, None);
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

    // The list comes from the engine (live, or its built-in fallback when the
    // mirror is unreachable), so assert its shape rather than fixed releases.
    assert!(!res.versions.is_empty());
    assert_eq!(
        res.versions.iter().filter(|v| v.latest).count(),
        1,
        "exactly one release is the one to pick"
    );
    assert!(res.versions.iter().all(|v| !v.version.is_empty()));
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
async fn run_linux_builds_the_expected_command() {
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
            attach_disk: vec!["/tmp/extra.img:ro".into()],
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

    assert_eq!(res.id, "m-boot-001");

    let a = &h.last_command()["Linux"];
    assert_eq!(a["image"], "alpine:3.20");
    assert_eq!(a["vm"]["cpus"], 4);
    assert_eq!(a["vm"]["mem"], 2048);
    assert_eq!(a["net"]["ports"][0]["host"], 8080);
    assert_eq!(a["net"]["ports"][0]["guest"], 80);
    assert_eq!(a["net"]["network"], "devnet");
    assert_eq!(a["net"]["name"], "web");
    assert_eq!(a["volume"], "data");
    assert_eq!(a["mounts"][0], "/host:/guest:ro");
    assert_eq!(a["attach_disk"][0]["path"], "/tmp/extra.img");
    assert_eq!(a["attach_disk"][0]["read_only"], true);
    assert_eq!(a["env"][0], "A=1");
    assert_eq!(a["env"][1], "B=2");
    assert_eq!(a["command"], serde_json::json!(["sh", "-c", "echo hi"]));
    // Always detached, and the fields the request left alone came from the
    // engine's own defaults rather than being blank.
    assert_eq!(a["detach"], true);
    assert_eq!(a["console"], "hvc0");
    assert!(a["kernel_version"].as_str().is_some_and(|v| !v.is_empty()));
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

    assert_eq!(res.id, "m-boot-001");

    let cmd = h.last_command();
    // The OS chose the variant, so it cannot be mis-spelled as a subcommand.
    assert!(
        cmd.get("Netbsd").is_some(),
        "expected a netbsd command: {cmd}"
    );
    let a = &cmd["Netbsd"];
    assert_eq!(a["version"], "10.1");
    assert_eq!(a["run"]["detach"], true);
    assert_eq!(a["run"]["persist"], true);
    assert_eq!(a["net"]["no_net"], true);
    assert_eq!(a["disk_size"], "8G");
    assert_eq!(a["attach_disk"][0]["read_only"], true);
    // cpus == 0 means "unset", which takes the engine's default rather than
    // booting a machine with no vCPUs.
    assert_eq!(a["vm"]["cpus"], 1);
    assert_eq!(a["vm"]["mem"], 512);
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
    let a = &h.last_command()["Logs"];
    assert_eq!(a["id"], "abc123");
    assert_eq!(a["follow"], true);
    assert_eq!(a["boot"], false);
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
    assert!(out.contains("ran: probe"), "{out}");
    assert_eq!(code, Some(0));
    // The passthrough goes through the supervisor's `cli` entry point, which
    // parses the command line with the engine's own clap definition.
    assert_eq!(h.last_argv(), ["cli", "--", "probe"]);
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
    let a = &h.last_command()["Exec"];
    assert_eq!(a["id"], "abc123");
    assert_eq!(a["command"], serde_json::json!(["cat"]));
    assert_eq!(a["env"], serde_json::json!(["K=V"]));
    assert_eq!(a["tty"], false);
}

#[tokio::test]
async fn exec_with_a_tty_runs_the_session_under_a_pty() {
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
    let a = &h.last_command()["Exec"];
    assert_eq!(a["id"], "abc123");
    assert_eq!(a["tty"], true);
    // A Linux guest sets its own TERM, so none is injected for it.
    assert_eq!(a["env"], serde_json::json!([]));
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
    let a = &h.last_command()["Shell"];
    assert_eq!(a["id"], "abc123");
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

    let mut child = std::process::Command::new(exe)
        // Port 0 lets the OS pick, so concurrent test runs cannot collide.
        .args(["--bind", "127.0.0.1:0"])
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
fn built_binary(name: &str) -> Option<PathBuf> {
    let mut dir = std::env::current_exe().expect("test exe");
    dir.pop(); // deps/
    if dir.ends_with("deps") {
        dir.pop();
    }
    let exe = dir.join(name);
    Path::new(&exe).exists().then_some(exe)
}

fn daemon_binary() -> PathBuf {
    built_binary("bsdkrund")
        .expect("bsdkrund not built; run `cargo build --release -p bsdkrun-daemon` first")
}

/// The real supervisor, if this checkout could build one.
///
/// It links libkrun, so a host without one cannot have built it — and the
/// daemon's own test suite deliberately runs on such hosts. The two tests that
/// need the real binary skip rather than fail there; every other test uses the
/// stub, which is the point of having one.
fn supervisor_binary() -> Option<PathBuf> {
    built_binary("bsdkrun-supervisor")
}

/// The supervisor is real: `bsdkrun-supervisor run <json>` runs the engine with
/// no `bsdkrun` anywhere on PATH.
///
/// This is the property the whole change exists for, so it is asserted against
/// the actual shipped binary rather than a stub.
#[test]
fn the_supervisor_binary_runs_engine_commands() {
    let state = fixture_state().to_path_buf();
    let spec = serde_json::to_string(&serde_json::json!({
        "Ps": { "all": true, "json": true }
    }))
    .unwrap();

    let Some(exe) = supervisor_binary() else {
        eprintln!("skipping: bsdkrun-supervisor is not built (no libkrun on this host)");
        return;
    };
    let out = std::process::Command::new(exe)
        .arg("run")
        .arg(&spec)
        .env("BSDKRUN_STATE", &state)
        // Nothing on PATH at all: there is no CLI to fall back to.
        .env("PATH", "")
        .output()
        .expect("running the supervisor");

    assert!(
        out.status.success(),
        "exit {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let machines: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("`ps --json` printed JSON");
    let ids: Vec<&str> = machines
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"abc123"), "seeded machines missing: {ids:?}");
}

/// An unparseable spec fails loudly rather than booting something unintended.
#[test]
fn the_supervisor_rejects_a_spec_it_cannot_decode() {
    let Some(exe) = supervisor_binary() else {
        eprintln!("skipping: bsdkrun-supervisor is not built (no libkrun on this host)");
        return;
    };
    let out = std::process::Command::new(exe)
        .arg("run")
        .arg("{\"NoSuchCommand\":{}}")
        .output()
        .expect("running the supervisor");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("decoding the command"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
