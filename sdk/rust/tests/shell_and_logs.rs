//! The scripted interactive shell session and log streaming, against the
//! in-process fake daemon.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bsdkrun::Client;
use serde_json::json;
use support::{wait_until, FakeDaemon};

#[test]
fn shell_session_buffers_output_until_a_callback_registers() {
    let daemon = FakeDaemon::start();
    daemon.set_shell_script(vec![b"$ ", b"ls\nREADME\n"], 0);
    let client = Client::new(daemon.url(), "tok").unwrap();

    let mut session = client
        .shell("machine123")
        .rows(50)
        .cols(120)
        .open()
        .unwrap();
    assert_eq!(session.id(), "sess-1");

    // Give the scripted stream time to arrive *before* any callback exists —
    // this is exactly the window the buffering must cover.
    let exited = Arc::new(Mutex::new(None::<i32>));
    let exit_sink = Arc::clone(&exited);
    session.on_exit(move |code| *exit_sink.lock().unwrap() = Some(code));
    wait_until(
        || exited.lock().unwrap().is_some(),
        "the session's exit event",
    );

    let collected = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = Arc::clone(&collected);
    session.on_output(move |bytes| sink.lock().unwrap().extend_from_slice(bytes));
    assert_eq!(
        collected.lock().unwrap().as_slice(),
        b"$ ls\nREADME\n",
        "output that arrived before on_output must be flushed to it"
    );
    assert_eq!(*exited.lock().unwrap(), Some(0));

    // The open call carried the terminal geometry.
    let open = daemon.find_call("openShell").unwrap();
    assert_eq!(open.variables["rows"], 50);
    assert_eq!(open.variables["cols"], 120);

    // write/resize/close are plain mutations against the session id.
    session.write("ls -la\n").unwrap();
    let input = daemon.find_call("sendShellInput").unwrap();
    assert_eq!(input.variables["sessionId"], "sess-1");
    assert_eq!(input.variables["dataBase64"], "bHMgLWxhCg=="); // "ls -la\n"

    session.resize(40, 100).unwrap();
    let resize = daemon.find_call("resizeShell").unwrap();
    assert_eq!(resize.variables["rows"], 40);

    session.close();
    session.close(); // idempotent
    let queries = daemon.http_queries();
    assert_eq!(
        queries.iter().filter(|q| q.contains("closeShell")).count(),
        1,
        "close() must be idempotent"
    );
}

#[test]
fn follow_logs_streams_decoded_bytes_then_completes() {
    let daemon = FakeDaemon::start();
    daemon.set_log_script(vec![b"boot line 1\n", b"boot line 2\n"]);
    let client = Client::new(daemon.url(), "tok").unwrap();

    let collected = Arc::new(Mutex::new(Vec::<u8>::new()));
    let done = Arc::new(Mutex::new(false));
    let sink = Arc::clone(&collected);
    let done_flag = Arc::clone(&done);

    let sub = client
        .follow_logs("abc123")
        .boot(false)
        .on_data(move |bytes| sink.lock().unwrap().extend_from_slice(&bytes))
        .on_complete(move || *done_flag.lock().unwrap() = true)
        .start()
        .unwrap();

    wait_until(|| *done.lock().unwrap(), "the log stream to complete");
    assert_eq!(
        collected.lock().unwrap().as_slice(),
        b"boot line 1\nboot line 2\n"
    );

    let subscribe = daemon.wait_for_ws(|m| m["type"] == "subscribe", Duration::from_secs(5));
    assert_eq!(subscribe["payload"]["variables"]["follow"], true);
    assert_eq!(subscribe["payload"]["variables"]["id"], "abc123");

    sub.unsubscribe();
}

#[test]
fn raw_subscribe_escape_hatch_delivers_data() {
    let daemon = FakeDaemon::start();
    daemon.set_log_script(vec![b"x\n"]);
    let client = Client::new(daemon.url(), "tok").unwrap();

    let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let sink = Arc::clone(&events);
    let _sub = client
        .subscribe(
            "subscription($id: String!, $follow: Boolean!, $boot: Boolean!) { machineLogs(id: $id, follow: $follow, boot: $boot) { dataBase64 exitCode } }",
            json!({"id": "abc", "follow": true, "boot": false}),
            move |data| sink.lock().unwrap().push(data),
        )
        .unwrap();

    wait_until(|| events.lock().unwrap().len() >= 2, "two next events");
    let first = events.lock().unwrap()[0].clone();
    assert!(first.pointer("/machineLogs/dataBase64").is_some());
}
