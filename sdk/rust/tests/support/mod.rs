//! Fake servers the integration tests run the SDK against.
//!
//! Two of them, mirroring the Python/Ruby SDK test suites:
//!
//! * [`FakeDaemon`] — a combined HTTP + WebSocket GraphQL server on **one**
//!   port, exactly like the real `bsdkrund` (`POST /graphql` and
//!   `/graphql/ws` share a bind address). Each connection is classified by
//!   the presence of an `Upgrade: websocket` header. It dispatches on
//!   substrings of the query text rather than parsing GraphQL — all a unit
//!   test needs — and its WS side auto-acks `connection_init` and replays a
//!   scripted `shellOutput` / `machineLogs` stream on `subscribe`.
//!
//! * [`RawWsServer`] — a script-nothing WebSocket server whose replies are
//!   driven by the test thread, for exercising the transport's protocol
//!   behavior (queued-until-ack, error routing, close-before-ack) step by
//!   step, like Python's `_TestWSServer`.

// Not every test binary uses every helper; that's fine.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::{json, Value};
use tungstenite::handshake::derive_accept_key;
use tungstenite::protocol::Role;
use tungstenite::{Message, WebSocket};

/// One recorded HTTP GraphQL call.
#[derive(Debug, Clone)]
pub struct HttpCall {
    pub query: String,
    pub variables: Value,
    pub authorization: String,
}

#[derive(Default)]
pub struct DaemonState {
    pub http_calls: Mutex<Vec<HttpCall>>,
    pub ws_received: Mutex<Vec<Value>>,
    pub machines: Mutex<Vec<Value>>,
    /// Replayed by a `shellOutput` subscription, then the exit code.
    pub shell_chunks: Mutex<Vec<Vec<u8>>>,
    pub shell_exit: Mutex<i32>,
    /// Replayed by a `machineLogs` subscription, then exit 0.
    pub log_chunks: Mutex<Vec<Vec<u8>>>,
    /// When set, every HTTP response uses this (status_line, body) instead of
    /// the scripted dispatch — for 401 / GraphQL-error tests.
    pub force_response: Mutex<Option<(String, Value)>>,
}

pub struct FakeDaemon {
    pub port: u16,
    pub state: Arc<DaemonState>,
}

impl FakeDaemon {
    pub fn start() -> FakeDaemon {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(DaemonState::default());
        let accept_state = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let conn_state = Arc::clone(&accept_state);
                std::thread::spawn(move || handle_connection(stream, conn_state));
            }
        });
        FakeDaemon { port, state }
    }

    /// What a user would paste: the client normalizes it to `/graphql` itself.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn set_machines(&self, machines: Vec<Value>) {
        *self.state.machines.lock().unwrap() = machines;
    }

    pub fn set_shell_script(&self, chunks: Vec<&[u8]>, exit_code: i32) {
        *self.state.shell_chunks.lock().unwrap() = chunks.into_iter().map(|c| c.to_vec()).collect();
        *self.state.shell_exit.lock().unwrap() = exit_code;
    }

    pub fn set_log_script(&self, chunks: Vec<&[u8]>) {
        *self.state.log_chunks.lock().unwrap() = chunks.into_iter().map(|c| c.to_vec()).collect();
    }

    pub fn force_response(&self, status_line: &str, body: Value) {
        *self.state.force_response.lock().unwrap() = Some((status_line.to_string(), body));
    }

    pub fn http_queries(&self) -> Vec<String> {
        self.state
            .http_calls
            .lock()
            .unwrap()
            .iter()
            .map(|c| c.query.clone())
            .collect()
    }

    pub fn find_call(&self, needle: &str) -> Option<HttpCall> {
        self.state
            .http_calls
            .lock()
            .unwrap()
            .iter()
            .find(|c| c.query.contains(needle))
            .cloned()
    }

    /// Poll until a WS message matching `pred` was received.
    pub fn wait_for_ws(&self, pred: impl Fn(&Value) -> bool, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(found) = self
                .state
                .ws_received
                .lock()
                .unwrap()
                .iter()
                .find(|m| pred(m))
            {
                return found.clone();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "expected WS message not seen within {timeout:?}: {:?}",
            self.state.ws_received.lock().unwrap()
        );
    }
}

fn handle_connection(stream: TcpStream, state: Arc<DaemonState>) {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
        return;
    }
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    if headers
        .get("upgrade")
        .map(|u| u.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
    {
        handle_ws(reader.into_inner(), &headers, state);
    } else {
        handle_http(reader, &headers, state);
    }
}

fn handle_http(
    mut reader: BufReader<TcpStream>,
    headers: &HashMap<String, String>,
    state: Arc<DaemonState>,
) {
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let payload: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));
    let query = payload["query"].as_str().unwrap_or_default().to_string();
    let variables = payload
        .get("variables")
        .cloned()
        .unwrap_or_else(|| json!({}));

    state.http_calls.lock().unwrap().push(HttpCall {
        query: query.clone(),
        variables: variables.clone(),
        authorization: headers.get("authorization").cloned().unwrap_or_default(),
    });

    let (status_line, response) =
        if let Some((status, body)) = state.force_response.lock().unwrap().clone() {
            (status, body)
        } else {
            (
                "200 OK".to_string(),
                json!({"data": dispatch_http(&query, &variables, &state)}),
            )
        };

    let json = response.to_string();
    let stream = reader.get_mut();
    let _ = write!(
        stream,
        "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
        json.len()
    );
    let _ = stream.flush();
}

/// GraphQL-shaped dispatch on substrings of the query text, like the Python
/// and Ruby fakes. Longest/most-specific substrings first.
fn dispatch_http(query: &str, variables: &Value, state: &DaemonState) -> Value {
    if query.contains("openShell") {
        return json!({"openShell": {
            "id": "sess-1",
            "machineId": variables.get("machineId").cloned().unwrap_or(Value::Null),
            "finished": false,
            "truncated": false,
        }});
    }
    if query.contains("closeShell") {
        return json!({"closeShell": true});
    }
    if query.contains("sendShellInput") {
        return json!({"sendShellInput": true});
    }
    if query.contains("resizeShell") {
        return json!({"resizeShell": true});
    }
    for mutation in [
        "stopMachine",
        "startMachine",
        "removeMachines",
        "updateMachine",
        "commitMachine",
    ] {
        if query.contains(mutation) {
            return json!({mutation: {"exitCode": 0, "stdout": "ok", "stderr": ""}});
        }
    }
    for run in [
        "runLinux",
        "runBsd",
        "runNanos",
        "runUnikraft",
        "runSolo5",
        "runOsv",
        "runFlavor",
    ] {
        if query.contains(run) {
            return json!({run: "abcdef123456"});
        }
    }
    if query.contains("machineLogs") {
        return json!({"machineLogs": "one-shot log text"});
    }
    if query.contains("machine(") {
        let wanted = variables["id"].as_str().unwrap_or_default();
        let machines = state.machines.lock().unwrap();
        let found = machines
            .iter()
            .find(|m| m["id"].as_str() == Some(wanted))
            .cloned()
            .unwrap_or(Value::Null);
        return json!({"machine": found});
    }
    if query.contains("machines(") {
        return json!({"machines": state.machines.lock().unwrap().clone()});
    }
    json!({})
}

fn handle_ws(mut stream: TcpStream, headers: &HashMap<String, String>, state: Arc<DaemonState>) {
    let key = headers
        .get("sec-websocket-key")
        .cloned()
        .unwrap_or_default();
    let accept = derive_accept_key(key.as_bytes());
    if write!(
        stream,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"
    )
    .is_err()
    {
        return;
    }
    let _ = stream.flush();

    let mut ws = WebSocket::from_raw_socket(stream, Role::Server, None);
    loop {
        let msg = match ws.read() {
            Ok(msg) => msg,
            Err(_) => break,
        };
        match msg {
            Message::Text(text) => {
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                state.ws_received.lock().unwrap().push(value.clone());
                react(&mut ws, &value, &state);
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

fn send_json(ws: &mut WebSocket<TcpStream>, value: Value) {
    let _ = ws.send(Message::Text(value.to_string()));
}

fn react(ws: &mut WebSocket<TcpStream>, msg: &Value, state: &DaemonState) {
    match msg["type"].as_str() {
        Some("connection_init") => send_json(ws, json!({"type": "connection_ack"})),
        Some("ping") => send_json(ws, json!({"type": "pong"})),
        Some("subscribe") => {
            let sub_id = msg["id"].clone();
            let query = msg
                .pointer("/payload/query")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if query.contains("shellOutput") {
                let chunks = state.shell_chunks.lock().unwrap().clone();
                let exit = *state.shell_exit.lock().unwrap();
                replay(ws, &sub_id, "shellOutput", &chunks, exit);
            } else if query.contains("machineLogs") {
                let chunks = state.log_chunks.lock().unwrap().clone();
                replay(ws, &sub_id, "machineLogs", &chunks, 0);
            }
        }
        _ => {}
    }
}

/// The scripted stream shape both subscriptions share: data frames, then the
/// exit-code frame, then the protocol-level `complete`.
fn replay(
    ws: &mut WebSocket<TcpStream>,
    sub_id: &Value,
    field: &str,
    chunks: &[Vec<u8>],
    exit: i32,
) {
    for chunk in chunks {
        send_json(
            ws,
            json!({"type": "next", "id": sub_id, "payload": {"data": {
                field: {"dataBase64": B64.encode(chunk), "exitCode": null}
            }}}),
        );
    }
    send_json(
        ws,
        json!({"type": "next", "id": sub_id, "payload": {"data": {
            field: {"dataBase64": null, "exitCode": exit}
        }}}),
    );
    send_json(ws, json!({"type": "complete", "id": sub_id}));
}

// ---------------------------------------------------------------------------
// RawWsServer
// ---------------------------------------------------------------------------

/// Accepts one WebSocket connection, records every text message, and lets the
/// test thread script the replies — nothing is sent automatically.
pub struct RawWsServer {
    pub port: u16,
    pub received: Arc<Mutex<Vec<Value>>>,
    conn: Arc<Mutex<Option<WebSocket<TcpStream>>>>,
}

impl RawWsServer {
    pub fn start() -> RawWsServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let conn: Arc<Mutex<Option<WebSocket<TcpStream>>>> = Arc::new(Mutex::new(None));

        let reader_received = Arc::clone(&received);
        let reader_conn = Arc::clone(&conn);
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // Manual handshake: read the request head, answer 101.
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => buf.push(byte[0]),
                    _ => return,
                }
            }
            let head = String::from_utf8_lossy(&buf);
            let key = head
                .lines()
                .find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("sec-websocket-key")
                        .then(|| v.trim().to_string())
                })
                .unwrap_or_default();
            let accept = derive_accept_key(key.as_bytes());
            if write!(
                stream,
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: graphql-transport-ws\r\n\r\n"
            )
            .is_err()
            {
                return;
            }
            let _ = stream.flush();

            // Short read timeouts so the test thread can grab the connection
            // lock to send scripted replies between our reads.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
            *reader_conn.lock().unwrap() =
                Some(WebSocket::from_raw_socket(stream, Role::Server, None));

            loop {
                let mut guard = reader_conn.lock().unwrap();
                let Some(ws) = guard.as_mut() else { break };
                match ws.read() {
                    Ok(Message::Text(text)) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            reader_received.lock().unwrap().push(value);
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(ref e))
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => break,
                }
                drop(guard);
                std::thread::sleep(Duration::from_millis(2));
            }
        });

        RawWsServer {
            port,
            received,
            conn,
        }
    }

    /// A URL shaped like the daemon's subscriptions endpoint.
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}/graphql/ws", self.port)
    }

    /// Send one scripted graphql-transport-ws message to the client.
    pub fn send(&self, value: Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            {
                let mut guard = self.conn.lock().unwrap();
                if let Some(ws) = guard.as_mut() {
                    ws.send(Message::Text(value.to_string())).unwrap();
                    return;
                }
            }
            assert!(Instant::now() < deadline, "no client ever connected");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Poll until a received message matches `pred`.
    pub fn wait_for(&self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(found) = self.received.lock().unwrap().iter().find(|m| pred(m)) {
                return found.clone();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "expected message not seen within 5s: {:?}",
            self.received.lock().unwrap()
        );
    }

    /// Drop the connection (a server hang-up, as a bad token produces).
    pub fn close_conn(&self) {
        let _ = self.conn.lock().unwrap().take();
    }
}

/// Wait until `pred` turns true, or panic after 5s.
pub fn wait_until(pred: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}
