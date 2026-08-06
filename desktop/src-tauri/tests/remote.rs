//! Drives a real `bsdkrund` through the same code path the app uses for a
//! remote target.
//!
//! Ignored by default because it needs a daemon. Run it against one with:
//!
//! ```console
//! $ bsdkrund --bind 127.0.0.1:50077 &
//! $ BSDKRUN_TEST_DAEMON=http://127.0.0.1:50077 BSDKRUN_TEST_TOKEN=<token> \
//!     cargo test --test remote -- --ignored --nocapture
//! ```
//!
//! The unit tests cover parsing; this covers the part that can only be wrong
//! against a real server — that an argv sent over gRPC comes back as the same
//! stdout, stderr and exit code a local subprocess would have produced.

use bsdkrun_desktop_lib::remote;

fn daemon() -> Option<(String, String)> {
    let endpoint = std::env::var("BSDKRUN_TEST_DAEMON").ok()?;
    let token = std::env::var("BSDKRUN_TEST_TOKEN").ok()?;
    Some((endpoint, token))
}

#[tokio::test]
#[ignore = "needs a running bsdkrund; see the module docs"]
async fn runs_a_command_on_the_daemon() {
    let Some((endpoint, token)) = daemon() else {
        panic!("set BSDKRUN_TEST_DAEMON and BSDKRUN_TEST_TOKEN");
    };

    let out = remote::run(&endpoint, &token, &["--version"])
        .await
        .unwrap();
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout.contains("bsdkrun"),
        "unexpected version output: {:?}",
        out.stdout
    );
    println!("version   : {}", out.stdout.trim());
}

/// The JSON listings the whole UI is built on.
#[tokio::test]
#[ignore = "needs a running bsdkrund; see the module docs"]
async fn lists_machines_as_json() {
    let Some((endpoint, token)) = daemon() else {
        panic!("set BSDKRUN_TEST_DAEMON and BSDKRUN_TEST_TOKEN");
    };

    let out = remote::run(&endpoint, &token, &["ps", "-a", "--json"])
        .await
        .unwrap();
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let parsed: serde_json::Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON: {e}\n{}", out.stdout));
    assert!(parsed.is_array(), "expected an array of machines");
    println!("machines  : {}", parsed.as_array().unwrap().len());
}

/// A non-zero exit must come back as data, not as a transport error — several
/// commands report legitimate states that way.
#[tokio::test]
#[ignore = "needs a running bsdkrund; see the module docs"]
async fn a_failing_command_reports_its_exit_code() {
    let Some((endpoint, token)) = daemon() else {
        panic!("set BSDKRUN_TEST_DAEMON and BSDKRUN_TEST_TOKEN");
    };

    let out = remote::run(&endpoint, &token, &["no-such-subcommand"])
        .await
        .unwrap();
    assert_ne!(out.code, 0);
    assert!(
        out.stderr.contains("unrecognized") || out.stderr.contains("unexpected"),
        "unexpected stderr: {:?}",
        out.stderr
    );
    println!("bad cmd   : exit {}", out.code);
}

/// A wrong token must be refused, not silently accepted.
#[tokio::test]
#[ignore = "needs a running bsdkrund; see the module docs"]
async fn a_bad_token_is_rejected() {
    let Some((endpoint, _)) = daemon() else {
        panic!("set BSDKRUN_TEST_DAEMON and BSDKRUN_TEST_TOKEN");
    };

    let err = remote::run(&endpoint, "not-the-token", &["--version"])
        .await
        .expect_err("a bad token must not be accepted");
    let msg = err.to_string();
    assert!(
        msg.contains("token") || msg.to_lowercase().contains("unauth"),
        "unhelpful rejection: {msg}"
    );
    println!("bad token : {msg}");
}
