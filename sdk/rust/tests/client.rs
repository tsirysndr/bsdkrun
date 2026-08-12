//! Tests for `Client`: construction, HTTP-backed methods, error mapping, the
//! run builders' GraphQL inputs, and `exec()`'s open -> subscribe -> wait ->
//! close sequencing — all against the in-process [`support::FakeDaemon`], no
//! real `bsdkrund` needed.

mod support;

use std::time::Duration;

use bsdkrun_sdk::{BsdOs, Client, Error};
use serde_json::json;
use support::FakeDaemon;

fn machine_row() -> serde_json::Value {
    json!({
        "id": "abc123def456",
        "name": "web",
        "image": "alpine",
        "kind": "linux",
        "command": "sleep 1",
        "status": "running",
        "running": true,
        "exitCode": null,
        "pid": 42,
        "detached": true,
        "cpus": 2,
        "mem": 512,
        "volume": null,
        "stateDir": "/s",
        "createdAt": "1700000000",
        "finishedAt": null,
        "network": null,
        "netIp": null,
        "ports": [{"bind": "127.0.0.1", "host": 2222, "guest": 22}],
    })
}

// One test covers every from_env case: they all mutate process-global env
// vars, and tests in one binary run concurrently.
#[test]
fn from_env_requires_url_and_token_then_normalizes() {
    let saved_url = std::env::var("BSDKRUN_URL").ok();
    let saved_token = std::env::var("BSDKRUN_TOKEN").ok();

    std::env::remove_var("BSDKRUN_URL");
    std::env::remove_var("BSDKRUN_TOKEN");
    assert!(matches!(Client::from_env(), Err(Error::InvalidInput(_))));

    std::env::set_var("BSDKRUN_URL", "http://localhost:50052");
    assert!(matches!(Client::from_env(), Err(Error::InvalidInput(_))));

    std::env::set_var("BSDKRUN_TOKEN", "   ");
    assert!(matches!(Client::from_env(), Err(Error::InvalidInput(_))));

    std::env::set_var("BSDKRUN_URL", "localhost:50052");
    std::env::set_var("BSDKRUN_TOKEN", "secret");
    let client = Client::from_env().unwrap();
    assert_eq!(client.url(), "http://localhost:50052/graphql");

    match saved_url {
        Some(v) => std::env::set_var("BSDKRUN_URL", v),
        None => std::env::remove_var("BSDKRUN_URL"),
    }
    match saved_token {
        Some(v) => std::env::set_var("BSDKRUN_TOKEN", v),
        None => std::env::remove_var("BSDKRUN_TOKEN"),
    }
}

#[test]
fn new_rejects_a_url_without_a_token() {
    assert!(matches!(
        Client::new("localhost:50052", ""),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        Client::new("localhost:50052", "   "),
        Err(Error::InvalidInput(_))
    ));
    assert!(Client::new("localhost:50052", "tok").is_ok());
}

#[test]
fn list_and_get_map_graphql_fields() {
    let daemon = FakeDaemon::start();
    daemon.set_machines(vec![machine_row()]);
    let client = Client::new(daemon.url(), "tok").unwrap();

    let machines = client.list(true).unwrap();
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0].id, "abc123def456");
    assert_eq!(machines[0].created_at, 1700000000);
    assert_eq!(machines[0].pid, Some(42));
    assert_eq!(machines[0].ports[0].host, 2222);

    let found = client.get("abc123def456").unwrap().unwrap();
    assert_eq!(found.name.as_deref(), Some("web"));

    assert!(client.get("nope").unwrap().is_none());

    // The bearer token rode along on every call.
    let call = daemon.find_call("machines(").unwrap();
    assert_eq!(call.authorization, "Bearer tok");
}

#[test]
fn lifecycle_mutations_map_command_results() {
    let daemon = FakeDaemon::start();
    let client = Client::new(daemon.url(), "tok").unwrap();

    let stopped = client.stop("abc").unwrap();
    assert_eq!(stopped.exit_code, 0);
    assert_eq!(stopped.stdout, "ok");

    client.start("abc").unwrap();
    client.remove(&["abc", "def"], true).unwrap();
    client.update("abc", Some(4), Some(2048)).unwrap();
    client.commit("abc", "snap", "a snapshot").unwrap();

    let update = daemon.find_call("updateMachine").unwrap();
    assert_eq!(update.variables["cpus"], 4);
    assert_eq!(update.variables["mem"], 2048);

    let remove = daemon.find_call("removeMachines").unwrap();
    assert_eq!(remove.variables["ids"], json!(["abc", "def"]));
    assert_eq!(remove.variables["force"], true);
}

#[test]
fn http_401_maps_to_auth_error() {
    let daemon = FakeDaemon::start();
    daemon.force_response("401 Unauthorized", json!({"errors": [{"message": "no"}]}));
    let client = Client::new(daemon.url(), "bad").unwrap();
    let err = client.list(false).unwrap_err();
    assert!(matches!(err, Error::Auth { .. }), "{err:?}");
    assert_eq!(err.code(), Some("UNAUTHENTICATED"));
}

#[test]
fn graphql_errors_carry_the_extensions_code() {
    let daemon = FakeDaemon::start();
    daemon.force_response(
        "200 OK",
        json!({"errors": [{"message": "boom", "extensions": {"code": "FAILED"}}]}),
    );
    let client = Client::new(daemon.url(), "tok").unwrap();
    let err = client.list(false).unwrap_err();
    match &err {
        Error::GraphQL { message, code } => {
            assert_eq!(message, "boom");
            assert_eq!(code.as_deref(), Some("FAILED"));
        }
        other => panic!("expected GraphQL error, got {other:?}"),
    }
}

#[test]
fn unauthenticated_extension_code_maps_to_auth_error() {
    let daemon = FakeDaemon::start();
    daemon.force_response(
        "200 OK",
        json!({"errors": [{"message": "bad token", "extensions": {"code": "UNAUTHENTICATED"}}]}),
    );
    let client = Client::new(daemon.url(), "tok").unwrap();
    assert!(matches!(client.list(false), Err(Error::Auth { .. })));
}

#[test]
fn unreachable_daemon_is_a_transport_graphql_error() {
    // Port 9 (discard) on localhost is reliably closed.
    let client = Client::new("http://127.0.0.1:9", "tok").unwrap();
    let err = client.list(false).unwrap_err();
    match err {
        Error::GraphQL { code, message } => {
            assert_eq!(code, None);
            assert!(message.contains("cannot reach"), "{message}");
        }
        other => panic!("expected transport GraphQL error, got {other:?}"),
    }
}

#[test]
fn run_linux_builder_sends_the_exact_graphql_input() {
    let daemon = FakeDaemon::start();
    let client = Client::new(daemon.url(), "tok").unwrap();

    let id = client
        .run_linux()
        .image("alpine")
        .cpus(2)
        .mem(1024)
        .port("8080:80")
        .forward(2222, 22)
        .network("devnet")
        .volume("web")
        .mount("~/project:/src")
        .env("X", "hi")
        .command(["sleep", "300"])
        .launch()
        .unwrap();
    assert_eq!(id, "abcdef123456");

    let call = daemon.find_call("runLinux").unwrap();
    let input = &call.variables["input"];
    assert_eq!(input["image"], "alpine");
    assert_eq!(input["cpus"], 2);
    assert_eq!(input["mem"], 1024);
    assert_eq!(input["net"]["ports"], json!(["8080:80", "2222:22"]));
    assert_eq!(input["net"]["network"], "devnet");
    assert_eq!(input["net"]["noNet"], false);
    assert_eq!(input["volume"], "web");
    assert_eq!(input["mounts"], json!(["~/project:/src"]));
    assert_eq!(input["env"], json!(["X=hi"]));
    assert_eq!(input["command"], json!(["sleep", "300"]));
    assert_eq!(input["initramfs"], false);
}

#[test]
fn run_linux_requires_an_image() {
    let daemon = FakeDaemon::start();
    let client = Client::new(daemon.url(), "tok").unwrap();
    assert!(matches!(
        client.run_linux().cpus(2).launch(),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn run_bsd_builder_maps_the_os_enum_and_untouched_net_is_null() {
    let daemon = FakeDaemon::start();
    let client = Client::new(daemon.url(), "tok").unwrap();

    client
        .run_bsd(BsdOs::Freebsd)
        .version("14.3")
        .mem(2048)
        .persist()
        .launch()
        .unwrap();

    let call = daemon.find_call("runBsd").unwrap();
    let input = &call.variables["input"];
    assert_eq!(input["os"], "FREEBSD");
    assert_eq!(input["version"], "14.3");
    assert_eq!(input["persist"], true);
    assert!(
        input["net"].is_null(),
        "untouched net must serialize as null"
    );
}

#[test]
fn remaining_run_builders_hit_their_mutations() {
    let daemon = FakeDaemon::start();
    let client = Client::new(daemon.url(), "tok").unwrap();

    client.run_nanos().image("hello").launch().unwrap();
    client
        .run_unikraft()
        .path(".")
        .cmdline("hi")
        .launch()
        .unwrap();
    client
        .run_solo5()
        .path("dist/hello.hvt")
        .args(["--ipv4=10.0.0.2/24"])
        .launch()
        .unwrap();
    client
        .run_osv()
        .image("loader.img")
        .gic("v2")
        .launch()
        .unwrap();
    client.run_flavor("caddy").port("8080:80").launch().unwrap();

    let solo5 = daemon.find_call("runSolo5").unwrap();
    assert_eq!(
        solo5.variables["input"]["args"],
        json!(["--ipv4=10.0.0.2/24"])
    );
    let flavor = daemon.find_call("runFlavor").unwrap();
    assert_eq!(flavor.variables["input"]["name"], "caddy");
    assert_eq!(flavor.variables["input"]["ports"], json!(["8080:80"]));
}

#[test]
fn exec_opens_subscribes_waits_then_closes() {
    let daemon = FakeDaemon::start();
    daemon.set_shell_script(vec![b"hello ", b"world\n"], 7);
    let client = Client::new(daemon.url(), "tok").unwrap();

    let result = client.exec("machine123", ["echo", "hello world"]).unwrap();
    assert_eq!(result.exit_code, 7);
    assert_eq!(result.output, b"hello world\n");
    assert!(!result.ok());
    assert_eq!(result.text(), "hello world\n");

    let queries = daemon.http_queries();
    let open_idx = queries
        .iter()
        .position(|q| q.contains("openShell"))
        .unwrap();
    let close_idx = queries
        .iter()
        .position(|q| q.contains("closeShell"))
        .unwrap();
    assert!(open_idx < close_idx, "openShell must run before closeShell");

    // The open call carried the command through unmodified.
    let open = daemon.find_call("openShell").unwrap();
    assert_eq!(open.variables["command"], json!(["echo", "hello world"]));
    assert_eq!(open.variables["machineId"], "machine123");

    // Exactly one subscription, against the session openShell returned.
    let sub = daemon.wait_for_ws(|m| m["type"] == "subscribe", Duration::from_secs(5));
    assert_eq!(sub["payload"]["variables"]["sessionId"], "sess-1");

    // The WS connection carried the token in connection_init.
    let init = daemon.wait_for_ws(|m| m["type"] == "connection_init", Duration::from_secs(5));
    assert_eq!(init["payload"]["authorization"], "Bearer tok");
}

#[test]
fn one_shot_logs_query() {
    let daemon = FakeDaemon::start();
    let client = Client::new(daemon.url(), "tok").unwrap();
    assert_eq!(client.logs("abc", false).unwrap(), "one-shot log text");
    let call = daemon.find_call("machineLogs").unwrap();
    assert_eq!(call.variables["boot"], false);
}
