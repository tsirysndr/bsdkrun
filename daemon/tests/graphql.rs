//! GraphQL API tests.
//!
//! Driven against the shared fixtures in `common`: a seeded state directory for
//! everything the daemon now reads in-process, and a stub supervisor for the
//! operations that still need their own process. Queries and mutations execute
//! against the real schema; the HTTP layer is exercised through the real router
//! so the auth path is the shipped one.

use std::sync::Arc;

use async_graphql::{Request, Value};
use bsdkrun_daemon::auth::TokenAuth;
use bsdkrun_daemon::graphql::{schema, BsdkrunSchema};
use bsdkrun_daemon::ops::Ops;
use bsdkrun_daemon::shell::ShellRegistry;
use bsdkrun_daemon::supervisor::Supervisor;
use tokio_stream::StreamExt;

mod common;
use common::{decode, fixture_state, install_stub};

const TOKEN: &str = "test-token-graphql";

struct Harness {
    schema: BsdkrunSchema,
    log: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        fixture_state();
        let dir = tempfile::tempdir().unwrap();
        let (stub, log) = install_stub(dir.path());

        let ops = Ops::new(Supervisor::with_exe(stub));
        Self {
            schema: schema(ops, Arc::new(ShellRegistry::new())),
            log,
            _dir: dir,
        }
    }

    async fn query(&self, q: &str) -> Value {
        let res = self.schema.execute(Request::new(q)).await;
        assert!(
            res.errors.is_empty(),
            "unexpected GraphQL errors: {:?}",
            res.errors
        );
        res.data
    }

    async fn errors(&self, q: &str) -> Vec<async_graphql::ServerError> {
        self.schema.execute(Request::new(q)).await.errors
    }

    fn invocations(&self) -> Vec<Vec<String>> {
        common::invocations(&self.log)
    }

    /// The command the daemon handed the supervisor last, decoded.
    fn last_command(&self) -> serde_json::Value {
        decode(
            &self
                .invocations()
                .into_iter()
                .next_back()
                .expect("expected a recorded invocation"),
        )
    }

    /// Wait for the command carrying `variant`, e.g. "Exec" or "Shell".
    ///
    /// Opening a shell spawns the pty asynchronously, so reading the log
    /// immediately would race it.
    async fn wait_command(&self, variant: &str) -> serde_json::Value {
        for _ in 0..100 {
            if let Some(cmd) = self
                .invocations()
                .into_iter()
                .filter(|a| a.first().map(|s| s == "run").unwrap_or(false))
                .map(|a| decode(&a))
                .rfind(|c| c.get(variant).is_some())
            {
                return cmd;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no `{variant}` command was recorded within 5s");
    }
}

/// Serialize to JSON so assertions read like the wire format the frontend sees.
fn json(v: &Value) -> serde_json::Value {
    serde_json::to_value(v).unwrap()
}

// ---------------------------------------------------------------------------
// queries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn info_reports_the_engine_it_links() {
    let h = Harness::new();
    let d = h
        .query("{ info { daemonVersion cliVersion os arch } }")
        .await;
    // The engine is linked in, so its version is reported directly.
    assert_eq!(json(&d)["info"]["cliVersion"], bsdkrun_core::VERSION);
    assert_eq!(json(&d)["info"]["daemonVersion"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn machines_are_camel_cased_for_the_frontend() {
    let h = Harness::new();
    let d = h
        .query("{ machines(all: true) { id name running netIp stateDir createdAt } }")
        .await;
    // Picked out by id, not by position: other tests record machines of their
    // own into the shared state and run alongside this one.
    let all = json(&d);
    let m = all["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "abc123")
        .expect("the seeded machine is listed");
    assert_eq!(m["name"], "web");
    assert_eq!(m["running"], true);
    // snake_case in the CLI's JSON, camelCase in the schema.
    assert_eq!(m["netIp"], "192.168.127.7");
    assert!(m["stateDir"].as_str().unwrap().ends_with("abc123"));
    // Nothing was spawned: the listing came from the engine in-process.
    assert!(h.invocations().is_empty());
}

#[tokio::test]
async fn machines_without_all_lists_only_running_ones() {
    let h = Harness::new();
    let d = h.query("{ machines { id } }").await;
    // The stopped fixture is left out; the running ones are there. Asserted by
    // membership rather than by count: other tests create machines of their own
    // and run alongside this one.
    let ids: Vec<String> = json(&d)["machines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&"abc123".to_string()), "{ids:?}");
    assert!(!ids.contains(&"dead00000001".to_string()), "{ids:?}");
}

#[tokio::test]
async fn machine_lookup_accepts_an_id_prefix_like_the_cli() {
    let h = Harness::new();
    let d = h.query(r#"{ machine(id: "abc") { id name } }"#).await;
    assert_eq!(json(&d)["machine"]["id"], "abc123");

    let d = h.query(r#"{ machine(id: "nope") { id } }"#).await;
    assert!(json(&d)["machine"].is_null());
}

/// An image size in bytes exceeds a signed 32-bit `Int`, so the schema uses
/// `Float`. Returning `Int` would overflow on any image above 2 GiB.
#[tokio::test]
async fn image_size_survives_exceeding_a_32_bit_int() {
    let h = Harness::new();
    let d = h.query("{ images { reference size } }").await;
    // Compared as f64: the schema exposes it as a GraphQL Float.
    let img = json(&d)["images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["reference"] == "alpine:3.20")
        .unwrap()
        .clone();
    assert_eq!(img["size"].as_f64(), Some(5_000_000_000.0));
}

#[tokio::test]
async fn volume_size_is_text_and_unknown_becomes_null() {
    let h = Harness::new();
    let d = h.query("{ volumes { name size tracked } }").await;
    let v = json(&d)["volumes"].as_array().unwrap().to_vec();
    let data = v.iter().find(|v| v["name"] == "data").unwrap();
    let gone = v.iter().find(|v| v["name"] == "gone").unwrap();
    // A measurable volume reports human-readable text…
    let size = data["size"]
        .as_str()
        .expect("a measurable volume has a size");
    assert!(size.ends_with('B'), "unexpected size text: {size:?}");
    // …and one whose directory is gone reports null, not the "-" the CLI's
    // table prints in that cell.
    assert!(gone["size"].is_null());
}

#[tokio::test]
async fn networks_and_flavors_and_versions_resolve() {
    let h = Harness::new();
    let d = h.query("{ networks { name subnet members up } }").await;
    let nets = json(&d)["networks"].as_array().unwrap().to_vec();
    let dev = nets.iter().find(|n| n["name"] == "devnet").unwrap();
    assert_eq!(dev["subnet"], "192.168.127.0/24");
    assert_eq!(dev["members"], 1);

    let d = h.query("{ flavors { name kind source } }").await;
    let flavors = json(&d)["flavors"].as_array().unwrap().to_vec();
    // The catalog is compiled into the engine, so it needs no fixture.
    let node = flavors.iter().find(|f| f["name"] == "node").unwrap();
    assert_eq!(node["kind"], "linux");
    assert_eq!(node["source"], "catalog");

    let d = h
        .query("{ versions(os: FREEBSD) { version latest } }")
        .await;
    // The list comes from the engine (live, or its built-in fallback), so
    // assert its shape rather than fixed releases.
    let versions = json(&d)["versions"].as_array().unwrap().to_vec();
    assert!(!versions.is_empty());
    assert_eq!(versions.iter().filter(|v| v["latest"] == true).count(), 1);
}

// ---------------------------------------------------------------------------
// mutations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_linux_builds_the_expected_command() {
    let h = Harness::new();
    let d = h
        .query(
            r#"mutation { runLinux(input: {
                image: "alpine:3.20",
                cpus: 4, mem: 2048,
                net: { ports: ["8080:80"], network: "devnet", name: "web" },
                volume: "data",
                env: ["A=1"],
                command: ["sh", "-c", "echo hi"]
            }) }"#,
        )
        .await;
    assert_eq!(json(&d)["runLinux"], "m-boot-001");
    let a = &h.last_command()["Linux"];
    assert_eq!(a["image"], "alpine:3.20");
    assert_eq!(a["vm"]["cpus"], 4);
    assert_eq!(a["vm"]["mem"], 2048);
    assert_eq!(a["net"]["ports"][0]["host"], 8080);
    assert_eq!(a["net"]["ports"][0]["guest"], 80);
    assert_eq!(a["net"]["network"], "devnet");
    assert_eq!(a["net"]["name"], "web");
    assert_eq!(a["volume"], "data");
    assert_eq!(a["env"], serde_json::json!(["A=1"]));
    assert_eq!(a["command"], serde_json::json!(["sh", "-c", "echo hi"]));
    assert_eq!(a["detach"], true);
    // Untouched fields come from the engine's defaults, not from blanks.
    assert_eq!(a["console"], "hvc0");
}

#[tokio::test]
async fn run_bsd_maps_the_enum_and_is_always_detached() {
    let h = Harness::new();
    let d = h
        .query(
            r#"mutation { runBsd(input: {
                os: NETBSD, version: "10.1", mem: 512,
                net: { noNet: true }, persist: true, diskSize: "8G"
            }) }"#,
        )
        .await;
    assert_eq!(json(&d)["runBsd"], "m-boot-001");
    let cmd = h.last_command();
    assert!(
        cmd.get("Netbsd").is_some(),
        "expected a netbsd command: {cmd}"
    );
    let a = &cmd["Netbsd"];
    assert_eq!(a["version"], "10.1");
    assert_eq!(a["vm"]["mem"], 512);
    // cpus unset, so the engine's default applies rather than zero.
    assert_eq!(a["vm"]["cpus"], 1);
    assert_eq!(a["net"]["no_net"], true);
    assert_eq!(a["run"]["persist"], true);
    assert_eq!(a["run"]["detach"], true);
    assert_eq!(a["disk_size"], "8G");
}

/// A unikernel has no disk and no agent, so its argv carries none of the
/// volume / persist / repo / command flags — and the path is positional, last.
#[tokio::test]
async fn run_unikraft_puts_the_path_last_and_omits_disk_flags() {
    let h = Harness::new();
    let d = h
        .query(
            r#"mutation { runUnikraft(input: {
                path: "~/hello", cpus: 2, mem: 256,
                net: { noNet: true }, cmdline: "helloworld a b"
            }) }"#,
        )
        .await;
    assert_eq!(json(&d)["runUnikraft"], "m-boot-001");
    let a = &h.last_command()["Unikraft"];
    assert_eq!(a["path"], "~/hello");
    assert_eq!(a["vm"]["cpus"], 2);
    assert_eq!(a["vm"]["mem"], 256);
    assert_eq!(a["net"]["no_net"], true);
    assert_eq!(a["cmdline"], "helloworld a b");
    assert_eq!(a["detach"], true);
}

/// Volumes are the one disk-shaped option a unikernel does take: they are
/// virtio-fs shares, needing neither a disk nor an agent. Each is a separate
/// `--mount`, and they stay before the positional path.
#[tokio::test]
async fn run_unikraft_passes_each_volume_as_its_own_mount() {
    let h = Harness::new();
    h.query(
        r#"mutation { runUnikraft(input: {
            path: "~/hello", mounts: ["~/data:/data", "~/logs:/logs"]
        }) }"#,
    )
    .await;
    let a = &h.last_command()["Unikraft"];
    // Each volume is its own mount rather than one joined string.
    assert_eq!(
        a["mount"],
        serde_json::json!(["~/data:/data", "~/logs:/logs"])
    );
}

/// Nanos: flags stay before the positional image, and persist is the one
/// disk option a unikernel with a real root disk takes.
#[tokio::test]
async fn run_nanos_puts_the_image_last() {
    let h = Harness::new();
    let d = h
        .query(
            r#"mutation { runNanos(input: {
                image: "nanos-hello", mem: 512,
                net: { noNet: true }, cmdline: "x=1", persist: true
            }) }"#,
        )
        .await;
    assert_eq!(json(&d)["runNanos"], "m-boot-001");
    let a = &h.last_command()["Nanos"];
    assert_eq!(a["image"], "nanos-hello");
    assert_eq!(a["vm"]["mem"], 512);
    assert_eq!(a["net"]["no_net"], true);
    assert_eq!(a["cmdline"], "x=1");
    assert_eq!(a["persist"], true);
    assert_eq!(a["detach"], true);
}

#[tokio::test]
async fn run_nanos_requires_an_image() {
    let h = Harness::new();
    let errs = h
        .errors(r#"mutation { runNanos(input: { image: "" }) }"#)
        .await;
    assert!(!errs.is_empty());
}

/// The path defaults to the current directory, exactly as the CLI does.
#[tokio::test]
async fn run_unikraft_defaults_the_path_to_the_current_directory() {
    let h = Harness::new();
    h.query(r#"mutation { runUnikraft(input: {}) }"#).await;
    let a = &h.last_command()["Unikraft"];
    assert_eq!(a["path"], ".");
    assert_eq!(a["detach"], true);
}

#[tokio::test]
async fn lifecycle_mutations_report_a_non_zero_exit_rather_than_failing() {
    let h = Harness::new();
    // Its own machine: the shared fixtures are read by tests running alongside
    // this one, and stopping kills a real process.
    let id = common::machine_with_live_process("stopme000001");
    let d = h
        .query(&format!(
            r#"mutation {{ stopMachine(id: "{id}") {{ exitCode stdout }} }}"#
        ))
        .await;
    // Stopping runs in-process now, so the result carries the engine's own
    // message rather than a subprocess's stdout.
    assert_eq!(json(&d)["stopMachine"]["exitCode"], 0);
    assert!(json(&d)["stopMachine"]["stdout"]
        .as_str()
        .unwrap()
        .contains(&id));
    assert!(h.invocations().is_empty());

    // And a request that cannot be satisfied comes back as a non-zero exit
    // rather than a transport failure.
    let d = h
        .query(r#"mutation { stopMachine(id: "no-such-machine") { exitCode stderr } }"#)
        .await;
    assert_eq!(json(&d)["stopMachine"]["exitCode"], 1);
    assert!(!json(&d)["stopMachine"]["stderr"]
        .as_str()
        .unwrap()
        .is_empty());
}

/// The distinction between "impossible request" and "it ran and failed" is
/// carried in extensions, so a UI can react without parsing messages.
#[tokio::test]
async fn invalid_arguments_are_tagged_in_extensions() {
    let h = Harness::new();
    let errors = h
        .errors("mutation { removeMachines(ids: []) { exitCode } }")
        .await;
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("must not be empty"));
    let ext = errors[0].extensions.as_ref().expect("extensions");
    assert_eq!(
        ext.get("code"),
        Some(&async_graphql::Value::from("INVALID_ARGUMENT"))
    );

    // Nothing was run: a bare `rm -f` could be read far more broadly than the
    // caller intended.
    assert!(h
        .invocations()
        .iter()
        .all(|a| a.first().map(|s| s != "rm").unwrap_or(true)));
}

// ---------------------------------------------------------------------------
// shell sessions
// ---------------------------------------------------------------------------

/// The whole subscription design rests on this: the mutation that opens a shell
/// is necessarily a *separate* operation from the subscription that reads it,
/// so output produced in between has to be buffered or the prompt is lost.
#[tokio::test]
async fn shell_output_produced_before_subscribing_is_not_lost() {
    let h = Harness::new();

    let d = h
        .query(r#"mutation { openShell(machineId: "abc123", command: ["cat"]) { id machineId } }"#)
        .await;
    let session_id = json(&d)["openShell"]["id"].as_str().unwrap().to_string();
    assert_eq!(json(&d)["openShell"]["machineId"], "abc123");
    // openShell always allocates a terminal: it exists to back one.
    let a = h.wait_command("Exec").await["Exec"].clone();
    assert_eq!(a["id"], "abc123");
    assert_eq!(a["command"], serde_json::json!(["cat"]));
    assert_eq!(a["tty"], true);

    // Let the stub write EXEC_OK *before* anyone is listening.
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let mut stream = h.schema.execute_stream(Request::new(format!(
        r#"subscription {{ shellOutput(sessionId: "{session_id}") {{ dataBase64 exitCode }} }}"#
    )));

    // Ctrl-D so the stub's `cat` sees end-of-input and the session finishes.
    h.query(&format!(
        r#"mutation {{ sendShellInput(sessionId: "{session_id}", dataBase64: "BA==") }}"#
    ))
    .await;

    let mut text = String::new();
    let mut exit = None;
    while let Some(res) = stream.next().await {
        let v = json(&res.data);
        let out = &v["shellOutput"];
        if let Some(b64) = out["dataBase64"].as_str() {
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .unwrap();
            text.push_str(&String::from_utf8_lossy(&bytes));
        }
        if let Some(code) = out["exitCode"].as_i64() {
            exit = Some(code);
            break;
        }
    }

    assert!(
        text.contains("EXEC_OK"),
        "output written before subscribing was lost: {text:?}"
    );
    assert_eq!(exit, Some(0), "the stream must end with an exit code");
}

#[tokio::test]
async fn open_shell_with_no_command_opens_the_machine_shell() {
    let h = Harness::new();
    h.query(r#"mutation { openShell(machineId: "abc123") { id } }"#)
        .await;
    assert_eq!(h.wait_command("Shell").await["Shell"]["id"], "abc123");
}

/// A BSD guest boots with no usable TERM, and `exec` gets no env injected by
/// the CLI by design — so without this the shell comes up `dumb`: no line
/// editing, no colour, no arrow keys. Reported against the desktop app driving
/// a remote daemon.
#[tokio::test]
async fn a_bsd_guest_gets_a_usable_term() {
    let h = Harness::new();
    h.query(r#"mutation { openShell(machineId: "bsd456789abc", command: ["cat"]) { id } }"#)
        .await;
    let a = h.wait_command("Exec").await["Exec"].clone();
    assert_eq!(a["id"], "bsd456789abc");
    assert_eq!(a["env"], serde_json::json!(["TERM=xterm"]));
    assert_eq!(a["tty"], true);
}

/// Linux images set their own TERM, so nothing is injected for them.
#[tokio::test]
async fn a_linux_guest_keeps_its_own_term() {
    let h = Harness::new();
    h.query(r#"mutation { openShell(machineId: "abc123", command: ["cat"]) { id } }"#)
        .await;
    let argv = h.wait_command("Exec").await["Exec"]["env"].clone();
    let argv: Vec<String> = serde_json::from_value(argv).unwrap();
    assert!(
        !argv.iter().any(|a| a.starts_with("TERM=")),
        "TERM was injected for a Linux guest: {argv:?}"
    );
}

/// A caller that sets TERM itself is not overridden.
#[tokio::test]
async fn an_explicit_term_wins() {
    let h = Harness::new();
    h.query(
        r#"mutation { openShell(machineId: "bsd456789abc", command: ["cat"], env: ["TERM=screen"]) { id } }"#,
    )
    .await;
    let argv = h.wait_command("Exec").await["Exec"]["env"].clone();
    let argv: Vec<String> = serde_json::from_value(argv).unwrap();
    assert!(argv.contains(&"TERM=screen".to_string()), "{argv:?}");
    assert!(!argv.contains(&"TERM=xterm".to_string()), "{argv:?}");
}

#[tokio::test]
async fn shell_sessions_are_listed_and_can_be_closed() {
    let h = Harness::new();
    let d = h
        .query(r#"mutation { openShell(machineId: "abc123", command: ["cat"]) { id } }"#)
        .await;
    let id = json(&d)["openShell"]["id"].as_str().unwrap().to_string();

    let d = h.query("{ shellSessions { id machineId finished } }").await;
    assert_eq!(json(&d)["shellSessions"].as_array().unwrap().len(), 1);
    assert_eq!(json(&d)["shellSessions"][0]["id"], id.as_str());

    let d = h
        .query(&format!(r#"mutation {{ closeShell(sessionId: "{id}") }}"#))
        .await;
    assert_eq!(json(&d)["closeShell"], true);

    let d = h.query("{ shellSessions { id } }").await;
    assert!(json(&d)["shellSessions"].as_array().unwrap().is_empty());

    // Closing twice is not an error: a client tearing down a terminal should
    // not have to know whether the shell already exited.
    let d = h
        .query(&format!(r#"mutation {{ closeShell(sessionId: "{id}") }}"#))
        .await;
    assert_eq!(json(&d)["closeShell"], true);
}

#[tokio::test]
async fn unknown_sessions_and_bad_base64_are_rejected() {
    let h = Harness::new();
    let errors = h
        .errors(r#"mutation { sendShellInput(sessionId: "nope", dataBase64: "aGk=") }"#)
        .await;
    assert!(errors[0].message.contains("no such shell session"));

    let d = h
        .query(r#"mutation { openShell(machineId: "abc123", command: ["cat"]) { id } }"#)
        .await;
    let id = json(&d)["openShell"]["id"].as_str().unwrap().to_string();
    let errors = h
        .errors(&format!(
            r#"mutation {{ sendShellInput(sessionId: "{id}", dataBase64: "not base64!") }}"#
        ))
        .await;
    assert!(
        errors[0].message.contains("not valid base64"),
        "{:?}",
        errors
    );
}

// ---------------------------------------------------------------------------
// HTTP layer
// ---------------------------------------------------------------------------

mod http {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use tower::ServiceExt;

    fn router(h: &Harness) -> axum::Router {
        bsdkrun_daemon::http::router(
            h.schema.clone(),
            Arc::new(TokenAuth::new(TOKEN.to_string())),
        )
    }

    fn post(token: Option<(&str, &str)>) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder()
            .method("POST")
            .uri("/graphql")
            .header("content-type", "application/json");
        if let Some((name, value)) = token {
            b = b.header(name, value);
        }
        b.body(Body::from(r#"{"query":"{ info { cliVersion } }"}"#))
            .unwrap()
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn rejects_a_request_with_no_token() {
        let h = Harness::new();
        let resp = router(&h).oneshot(post(None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = body_text(resp).await;
        // A GraphQL-shaped error body, so client error handling works normally.
        assert!(body.contains("UNAUTHENTICATED"), "{body}");
    }

    #[tokio::test]
    async fn rejects_a_wrong_token() {
        let h = Harness::new();
        let resp = router(&h)
            .oneshot(post(Some(("authorization", "Bearer wrong"))))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_the_bearer_token() {
        let h = Harness::new();
        let resp = router(&h)
            .oneshot(post(Some(("authorization", &format!("Bearer {TOKEN}")))))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_text(resp).await.contains(bsdkrun_core::VERSION));
    }

    #[tokio::test]
    async fn accepts_the_explicit_header_too() {
        let h = Harness::new();
        let resp = router(&h)
            .oneshot(post(Some(("x-bsdkrun-token", TOKEN))))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// GraphiQL is anonymous, matching gRPC reflection: the schema is public,
    /// the machines are not.
    #[tokio::test]
    async fn graphiql_is_served_without_a_token() {
        let h = Harness::new();
        let resp = router(&h)
            .oneshot(
                HttpRequest::builder()
                    .uri("/graphql")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_text(resp).await.contains("graphiql"));
    }

    /// CORS matters: the frontend dev server is on another origin.
    #[tokio::test]
    async fn cors_allows_a_cross_origin_frontend() {
        let h = Harness::new();
        let resp = router(&h)
            .oneshot(
                HttpRequest::builder()
                    .method("OPTIONS")
                    .uri("/graphql")
                    .header("origin", "http://localhost:5173")
                    .header("access-control-request-method", "POST")
                    .header("access-control-request-headers", "authorization")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.headers().contains_key("access-control-allow-origin"));
    }
}

/// The image is positional and last, and OSv takes the disk options nanos and
/// unikraft do not — it has a root filesystem.
#[tokio::test]
async fn run_osv_puts_the_image_last_and_takes_the_disk_options() {
    let h = Harness::new();
    let d = h
        .query(
            r#"mutation { runOsv(input: {
                image: "loader.elf", mem: 512,
                net: { noNet: true }, cmdline: "/hello.so",
                disk: "d.raw", gic: "v3", persist: true
            }) }"#,
        )
        .await;
    assert_eq!(json(&d)["runOsv"], "m-boot-001");
    let a = &h.last_command()["Osv"];
    assert_eq!(a["image"], "loader.elf");
    assert_eq!(a["vm"]["mem"], 512);
    assert_eq!(a["cmdline"], "/hello.so");
    assert_eq!(a["disk"], "d.raw");
    assert_eq!(a["gic"], "V3");
    assert_eq!(a["persist"], true);
    assert_eq!(a["detach"], true);
}

/// Booting the kernel alone: no disk to attach, so the guest needs --nomount
/// in its command line. The flag has to reach the CLI either way.
#[tokio::test]
async fn run_osv_forwards_no_disk_and_extra_disks() {
    let h = Harness::new();
    h.query(
        r#"mutation { runOsv(input: {
            image: "loader.img", noDisk: true, attachDisk: ["a.raw", "b.raw:ro"]
        }) }"#,
    )
    .await;
    let a = &h.last_command()["Osv"];
    assert_eq!(a["image"], "loader.img");
    assert_eq!(a["no_disk"], true);
    assert_eq!(a["attach_disk"][0]["path"], "a.raw");
    assert_eq!(a["attach_disk"][0]["read_only"], false);
    assert_eq!(a["attach_disk"][1]["path"], "b.raw");
    assert_eq!(a["attach_disk"][1]["read_only"], true);
    // An unset gic keeps the engine's default rather than becoming empty.
    assert_eq!(a["gic"], "V2");
}

#[tokio::test]
async fn run_osv_requires_an_image() {
    let h = Harness::new();
    let errs = h
        .errors(r#"mutation { runOsv(input: { image: "" }) }"#)
        .await;
    assert!(!errs.is_empty());
}
