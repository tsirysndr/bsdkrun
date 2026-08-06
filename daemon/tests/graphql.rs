//! GraphQL API tests.
//!
//! Driven against a stub `bsdkrun` for the same reasons as the gRPC suite: it
//! keeps the tests hermetic and lets each one assert the exact argv produced.
//! Queries and mutations execute against the real schema; the HTTP layer is
//! exercised through the real router so the auth path is the shipped one.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use async_graphql::{Request, Value};
use bsdkrun_daemon::auth::TokenAuth;
use bsdkrun_daemon::cli::Cli;
use bsdkrun_daemon::graphql::{schema, BsdkrunSchema};
use bsdkrun_daemon::ops::Ops;
use bsdkrun_daemon::shell::ShellRegistry;
use tokio_stream::StreamExt;

const TOKEN: &str = "test-token-graphql";

const STUB: &str = r#"#!/bin/sh
LOG="$0.log"
for a in "$@"; do printf '%s\n' "$a" >> "$LOG"; done
printf -- '---\n' >> "$LOG"

case "$1" in
--version) echo "bsdkrun 9.9.9-stub"; exit 0 ;;
ps)
  echo '[{"id":"abc123","name":"web","image":"alpine","kind":"linux","command":"sh","status":"running","running":true,"exit_code":null,"pid":42,"detached":true,"cpus":2,"mem":1024,"volume":null,"state_dir":"/s","created_at":"1785993650","finished_at":null,"network":"devnet","net_ip":"192.168.127.7"},{"id":"bsd456789abc","name":"fbsd","image":"disk.raw","kind":"freebsd","command":"","status":"running","running":true,"exit_code":null,"pid":43,"detached":true,"cpus":2,"mem":1024,"volume":null,"state_dir":"/s2","created_at":"1785993651","finished_at":null,"network":null,"net_ip":null}]'
  exit 0 ;;
images)
  echo '[{"id":"img1","reference":"alpine:3.20","digest":"sha256:dead","size":3221225472,"rootfs":"/r","created_at":"1785854268"}]'
  exit 0 ;;
volume)
  echo '[{"name":"data","guest":"linux","base":"b.img","path":"/v","size":"2.3 GiB","created_at":"1","tracked":true},{"name":"empty","guest":null,"base":null,"path":"/e","size":"-","created_at":null,"tracked":false}]'
  exit 0 ;;
network)
  echo '[{"name":"devnet","subnet":"192.168.127.0/24","gateway":"192.168.127.1","members":6,"running":4,"up":true,"created_at":"1"}]'
  exit 0 ;;
flavors)
  echo '[{"name":"node","source":"catalog","kind":"linux","base":"node:22","category":"language","method":"docker","description":"Node","ports":["3000:3000"],"nix":[],"created_at":null}]'
  exit 0 ;;
versions) printf 'Available builds:\n  14.3\n  15.1  (latest)\n'; exit 0 ;;
linux|freebsd|netbsd) echo "m-$1-001"; exit 0 ;;
exec)
  echo "EXEC_OK"
  cat
  exit 0 ;;
stop|start) echo "$1 ok"; exit 0 ;;
esac
echo "unknown subcommand: $1" >&2
exit 2
"#;

struct Harness {
    schema: BsdkrunSchema,
    log: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("bsdkrun");
        std::fs::write(&stub, STUB).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let log = dir.path().join("bsdkrun.log");
        std::fs::write(&log, "").unwrap();

        let ops = Ops::new(Cli::resolve(Some(stub)).unwrap());
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

    fn last_argv(&self) -> Vec<String> {
        self.invocations()
            .into_iter()
            .rfind(|a| a.first().map(|s| s != "--version").unwrap_or(false))
            .expect("expected a recorded invocation")
    }

    /// Wait for the invocation whose subcommand is `verb`.
    ///
    /// Opening a shell spawns the pty asynchronously *and* runs a `ps` first to
    /// learn the guest kind, so neither "the last invocation" nor "read it
    /// immediately" is right — this waits for the one under test.
    async fn wait_argv(&self, verb: &str) -> Vec<String> {
        for _ in 0..100 {
            if let Some(argv) = self
                .invocations()
                .into_iter()
                .rfind(|a| a.first().map(|s| s == verb).unwrap_or(false))
            {
                return argv;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no `{verb}` invocation was recorded within 5s");
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
async fn info_reports_the_cli_it_drives() {
    let h = Harness::new();
    let d = h
        .query("{ info { daemonVersion cliVersion os arch } }")
        .await;
    assert_eq!(json(&d)["info"]["cliVersion"], "bsdkrun 9.9.9-stub");
    assert_eq!(json(&d)["info"]["daemonVersion"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn machines_are_camel_cased_for_the_frontend() {
    let h = Harness::new();
    let d = h
        .query("{ machines(all: true) { id name running netIp stateDir createdAt } }")
        .await;
    let m = &json(&d)["machines"][0];
    assert_eq!(m["id"], "abc123");
    assert_eq!(m["name"], "web");
    assert_eq!(m["running"], true);
    // snake_case in the CLI's JSON, camelCase in the schema.
    assert_eq!(m["netIp"], "192.168.127.7");
    assert_eq!(m["stateDir"], "/s");
    assert_eq!(h.last_argv(), ["ps", "-a", "--json"]);
}

#[tokio::test]
async fn machines_without_all_omits_the_flag() {
    let h = Harness::new();
    h.query("{ machines { id } }").await;
    assert_eq!(h.last_argv(), ["ps", "--json"]);
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
    assert_eq!(json(&d)["images"][0]["size"].as_f64(), Some(3221225472.0));
}

#[tokio::test]
async fn volume_size_is_text_and_unknown_becomes_null() {
    let h = Harness::new();
    let d = h.query("{ volumes { name size tracked } }").await;
    let v = &json(&d)["volumes"];
    assert_eq!(v[0]["size"], "2.3 GiB");
    // The CLI writes "-" when it cannot measure a volume; that is not a size.
    assert!(v[1]["size"].is_null());
}

#[tokio::test]
async fn networks_and_flavors_and_versions_resolve() {
    let h = Harness::new();
    let d = h.query("{ networks { name subnet members up } }").await;
    assert_eq!(json(&d)["networks"][0]["members"], 6);

    let d = h.query("{ flavors { name kind ports } }").await;
    assert_eq!(json(&d)["flavors"][0]["ports"][0], "3000:3000");

    let d = h
        .query("{ versions(os: FREEBSD) { version latest } }")
        .await;
    assert_eq!(json(&d)["versions"][1]["version"], "15.1");
    assert_eq!(json(&d)["versions"][1]["latest"], true);
    assert_eq!(h.last_argv(), ["versions", "--os", "freebsd"]);
}

// ---------------------------------------------------------------------------
// mutations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_linux_builds_the_expected_command_line() {
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
    assert_eq!(json(&d)["runLinux"], "m-linux-001");
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
            "-e",
            "A=1",
            "alpine:3.20",
            "--",
            "sh",
            "-c",
            "echo hi",
        ]
    );
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
    assert_eq!(json(&d)["runBsd"], "m-netbsd-001");
    assert_eq!(
        h.last_argv(),
        [
            "netbsd",
            "-d",
            // cpus unset, so no --cpus is emitted.
            "--mem",
            "512",
            "--no-net",
            "--version",
            "10.1",
            "--persist",
            "--disk-size",
            "8G",
        ]
    );
}

#[tokio::test]
async fn lifecycle_mutations_report_a_non_zero_exit_rather_than_failing() {
    let h = Harness::new();
    let d = h
        .query(r#"mutation { stopMachine(id: "abc123") { exitCode stdout } }"#)
        .await;
    assert_eq!(json(&d)["stopMachine"]["exitCode"], 0);
    assert_eq!(h.last_argv(), ["stop", "abc123"]);
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
    assert_eq!(h.wait_argv("exec").await, ["exec", "-t", "abc123", "cat"]);

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
    assert_eq!(h.wait_argv("shell").await, ["shell", "abc123"]);
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
    assert_eq!(
        h.wait_argv("exec").await,
        ["exec", "-t", "-e", "TERM=xterm", "bsd456789abc", "cat"]
    );
}

/// Linux images set their own TERM, so nothing is injected for them.
#[tokio::test]
async fn a_linux_guest_keeps_its_own_term() {
    let h = Harness::new();
    h.query(r#"mutation { openShell(machineId: "abc123", command: ["cat"]) { id } }"#)
        .await;
    let argv = h.wait_argv("exec").await;
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
    let argv = h.wait_argv("exec").await;
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
        assert!(body_text(resp).await.contains("9.9.9-stub"));
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
