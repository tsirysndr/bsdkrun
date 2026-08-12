//! End-to-end tests for `WsTransport` against a script-driven WebSocket
//! server — a port of the Python SDK's `test_transport_ws_e2e.py`. The server
//! sends nothing on its own, so each test drives the
//! connection_init -> connection_ack -> subscribe -> next -> complete cycle
//! by hand and asserts the transport's half of it.

mod support;

use std::sync::{Arc, Mutex};

use bsdkrun_sdk::transport::WsTransport;
use bsdkrun_sdk::Error;
use serde_json::json;
use support::{wait_until, RawWsServer};

#[test]
fn full_subscribe_next_complete_cycle_queues_until_ack() {
    let server = RawWsServer::start();
    let transport = WsTransport::new(server.url(), "tok");

    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let next_events = Arc::clone(&events);
    let complete_events = Arc::clone(&events);

    let sub_id = transport
        .subscribe(
            "subscription{x}",
            json!({}),
            Box::new(move |data| next_events.lock().unwrap().push(format!("next:{data}"))),
            Box::new(|_| {}),
            Box::new(move || complete_events.lock().unwrap().push("complete".into())),
        )
        .unwrap();

    let init = server.wait_for(|m| m["type"] == "connection_init");
    assert_eq!(init["payload"]["authorization"], "Bearer tok");

    // Nothing else may reach the wire before the ack — the subscribe above
    // was queued, not sent. (The server records in arrival order, so if the
    // subscribe had been sent early it would already be here.)
    assert_eq!(server.received.lock().unwrap().len(), 1);

    server.send(json!({"type": "connection_ack"}));
    let sub = server.wait_for(|m| m["type"] == "subscribe" && m["id"] == sub_id.as_str());
    assert_eq!(sub["payload"]["query"], "subscription{x}");

    server.send(json!({"type": "next", "id": sub_id, "payload": {"data": {"x": 1}}}));
    server.send(json!({"type": "complete", "id": sub_id}));

    wait_until(
        || events.lock().unwrap().iter().any(|e| e == "complete"),
        "on_complete to fire",
    );
    let seen = events.lock().unwrap().clone();
    assert!(seen.contains(&"next:{\"x\":1}".to_string()), "{seen:?}");
}

#[test]
fn error_message_routes_to_on_error() {
    let server = RawWsServer::start();
    let transport = WsTransport::new(server.url(), "tok");

    let caught = Arc::new(Mutex::new(Vec::<Error>::new()));
    let sink = Arc::clone(&caught);
    let sub_id = transport
        .subscribe(
            "subscription{x}",
            json!({}),
            Box::new(|_| {}),
            Box::new(move |err| sink.lock().unwrap().push(err)),
            Box::new(|| {}),
        )
        .unwrap();

    server.wait_for(|m| m["type"] == "connection_init");
    server.send(json!({"type": "connection_ack"}));
    server.wait_for(|m| m["type"] == "subscribe");
    server.send(json!({"type": "error", "id": sub_id, "payload": [{"message": "boom"}]}));

    wait_until(|| !caught.lock().unwrap().is_empty(), "on_error to fire");
    let errors = caught.lock().unwrap();
    assert!(
        matches!(&errors[0], Error::GraphQL { .. }),
        "{:?}",
        errors[0]
    );
    assert!(errors[0].to_string().contains("boom"));
}

#[test]
fn graphql_level_ping_gets_a_pong_reply() {
    let server = RawWsServer::start();
    let transport = WsTransport::new(server.url(), "tok");

    transport
        .subscribe(
            "subscription{x}",
            json!({}),
            Box::new(|_| {}),
            Box::new(|_| {}),
            Box::new(|| {}),
        )
        .unwrap();
    server.wait_for(|m| m["type"] == "connection_init");
    server.send(json!({"type": "connection_ack"}));
    server.wait_for(|m| m["type"] == "subscribe");

    server.send(json!({"type": "ping"}));
    server.wait_for(|m| m["type"] == "pong");
}

#[test]
fn close_before_ack_is_an_auth_error() {
    let server = RawWsServer::start();
    let transport = WsTransport::new(server.url(), "tok");

    let caught = Arc::new(Mutex::new(Vec::<Error>::new()));
    let sink = Arc::clone(&caught);
    transport
        .subscribe(
            "subscription{x}",
            json!({}),
            Box::new(|_| {}),
            Box::new(move |err| sink.lock().unwrap().push(err)),
            Box::new(|| {}),
        )
        .unwrap();
    server.wait_for(|m| m["type"] == "connection_init");
    server.close_conn(); // hang up without ever acking

    wait_until(|| !caught.lock().unwrap().is_empty(), "on_error to fire");
    let errors = caught.lock().unwrap();
    assert!(matches!(&errors[0], Error::Auth { .. }), "{:?}", errors[0]);
}

#[test]
fn close_after_ack_is_a_generic_graphql_error() {
    let server = RawWsServer::start();
    let transport = WsTransport::new(server.url(), "tok");

    let caught = Arc::new(Mutex::new(Vec::<Error>::new()));
    let sink = Arc::clone(&caught);
    transport
        .subscribe(
            "subscription{x}",
            json!({}),
            Box::new(|_| {}),
            Box::new(move |err| sink.lock().unwrap().push(err)),
            Box::new(|| {}),
        )
        .unwrap();
    server.wait_for(|m| m["type"] == "connection_init");
    server.send(json!({"type": "connection_ack"}));
    server.wait_for(|m| m["type"] == "subscribe");
    server.close_conn();

    wait_until(|| !caught.lock().unwrap().is_empty(), "on_error to fire");
    let errors = caught.lock().unwrap();
    assert!(
        matches!(&errors[0], Error::GraphQL { .. }),
        "an acked close must not read as an auth failure: {:?}",
        errors[0]
    );
}

#[test]
fn unsubscribe_sends_complete() {
    let server = RawWsServer::start();
    let transport = WsTransport::new(server.url(), "tok");

    let sub_id = transport
        .subscribe(
            "subscription{x}",
            json!({}),
            Box::new(|_| {}),
            Box::new(|_| {}),
            Box::new(|| {}),
        )
        .unwrap();
    server.wait_for(|m| m["type"] == "connection_init");
    server.send(json!({"type": "connection_ack"}));
    server.wait_for(|m| m["type"] == "subscribe");

    transport.unsubscribe(&sub_id);
    server.wait_for(|m| m["type"] == "complete" && m["id"] == sub_id.as_str());
}
