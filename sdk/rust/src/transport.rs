//! The GraphQL transport for talking to a remote `bsdkrund`.
//!
//! Two halves:
//!
//! * [`http_request`] — a plain blocking `POST` (via `ureq`) for queries and
//!   mutations. Mirrors the Python SDK's `http_request` byte-for-byte:
//!   headers, error mapping, transport-failure wrapping.
//!
//! * [`WsTransport`] — one `graphql-transport-ws` connection (via
//!   `tungstenite`), shared by every subscription a
//!   [`crate::Client`] opens.
//!
//! Threading model for the WS half: a single background thread owns the
//! socket outright — it drains an outbound channel between short
//! timeout-bounded reads, decodes messages, and dispatches
//! `next`/`error`/`complete` to the matching subscription's callbacks. The
//! public methods only ever push onto that channel, so no lock is ever held
//! across a blocking socket call. This keeps the whole SDK synchronous (no
//! async runtime) while still letting `exec()` block a calling thread on a
//! channel fed by the reader, and letting `shell()`'s callbacks fire live.

use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::error::{Error, Result};

/// The environment variable holding the daemon URL.
pub const URL_ENV: &str = "BSDKRUN_URL";
/// The environment variable holding the bearer token.
pub const TOKEN_ENV: &str = "BSDKRUN_TOKEN";

// ---------------------------------------------------------------------------
// URL handling
// ---------------------------------------------------------------------------

/// Turn what a person pastes into a full GraphQL endpoint URL.
///
/// Trim, assume `http://` when no scheme is given (people type
/// `localhost:50052`), strip trailing slashes, and append `/graphql` unless
/// the path already ends with it (case-insensitively).
pub fn normalize_url(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return s;
    }
    let lower = s.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        s = format!("http://{s}");
    }
    let trimmed = s.trim_end_matches('/');
    s = trimmed.to_string();
    if !s.to_ascii_lowercase().ends_with("/graphql") {
        s.push_str("/graphql");
    }
    s
}

/// Derive the subscriptions URL from a normalized HTTP endpoint URL.
///
/// `http://` becomes `ws://`, `https://` becomes `wss://`; trailing slashes
/// on the path are stripped and `/ws` is appended — e.g.
/// `http://host:50052/graphql` -> `ws://host:50052/graphql/ws`.
pub fn ws_url(http_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = http_url.strip_prefix("https://") {
        ("wss://", rest)
    } else if let Some(rest) = http_url.strip_prefix("http://") {
        ("ws://", rest)
    } else {
        ("ws://", http_url)
    };
    format!("{scheme}{}/ws", rest.trim_end_matches('/'))
}

// ---------------------------------------------------------------------------
// HTTP transport (queries + mutations)
// ---------------------------------------------------------------------------

/// Run a query or mutation over HTTP and return `data`.
///
/// Returns [`Error::Auth`] on an HTTP 401 or a GraphQL error whose
/// `extensions.code` is `"UNAUTHENTICATED"`, and [`Error::GraphQL`] for any
/// other GraphQL error or a transport-level failure (the daemon could not be
/// reached at all).
pub fn http_request(url: &str, token: &str, query: &str, variables: &Value) -> Result<Value> {
    let vars = if variables.is_null() {
        json!({})
    } else {
        variables.clone()
    };
    let body = json!({"query": query, "variables": vars}).to_string();
    let result = ureq::post(url)
        .set("content-type", "application/json")
        .set("authorization", &format!("Bearer {token}"))
        .send_string(&body);

    let (status, raw) = match result {
        Ok(response) => {
            let status = response.status();
            let text = response.into_string().map_err(|e| Error::GraphQL {
                message: format!("cannot read the daemon's response — {e}"),
                code: None,
            })?;
            (status, text)
        }
        // A non-2xx response still carries a JSON body worth parsing (the
        // daemon returns structured GraphQL errors even on a 4xx/5xx).
        Err(ureq::Error::Status(code, response)) => {
            (code, response.into_string().unwrap_or_default())
        }
        Err(ureq::Error::Transport(t)) => {
            return Err(Error::GraphQL {
                message: format!("cannot reach the bsdkrun daemon at {url} — {t}"),
                code: None,
            })
        }
    };

    if status == 401 {
        return Err(Error::auth_default());
    }

    let parsed: Value = serde_json::from_str(&raw).map_err(|_| Error::GraphQL {
        message: format!("the daemon returned a non-JSON response ({status})"),
        code: None,
    })?;

    if let Some(first) = parsed
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|errors| errors.first())
    {
        let message = first
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        let code = first
            .pointer("/extensions/code")
            .and_then(Value::as_str)
            .map(str::to_string);
        if code.as_deref() == Some("UNAUTHENTICATED") {
            return Err(Error::Auth { message });
        }
        return Err(Error::GraphQL { message, code });
    }

    Ok(match parsed.get("data") {
        Some(data) if data.is_object() => data.clone(),
        _ => json!({}),
    })
}

// ---------------------------------------------------------------------------
// subscription transport
// ---------------------------------------------------------------------------

/// A subscription's `next` callback: one call per `next` message.
pub type NextFn = Box<dyn FnMut(Value) + Send>;
/// A subscription's terminal error callback.
pub type ErrorFn = Box<dyn FnMut(Error) + Send>;
/// A subscription's clean-completion callback.
pub type CompleteFn = Box<dyn FnMut() + Send>;

struct Sub {
    on_next: NextFn,
    on_error: ErrorFn,
    on_complete: CompleteFn,
}

enum Out {
    Text(String),
    Shutdown,
}

type ClientSocket = WebSocket<MaybeTlsStream<TcpStream>>;

struct WsState {
    tx: Option<Sender<Out>>,
    acked: bool,
    /// `subscribe` messages queued until `connection_ack` arrives — the
    /// protocol forbids subscribing earlier, and the daemon would drop them.
    pending: Vec<String>,
    subs: HashMap<String, Arc<Mutex<Sub>>>,
    next_id: u64,
    /// Bumped per connection so a stale reader thread that dies late can
    /// never clear the state of the connection that replaced it.
    generation: u64,
}

/// One `graphql-transport-ws` connection, shared by every subscription a
/// [`crate::Client`] opens. Reconnects transparently: once the last
/// subscription ends the socket is dropped, and the next `subscribe` opens a
/// fresh one.
pub struct WsTransport {
    url: String,
    token: String,
    state: Arc<Mutex<WsState>>,
}

/// How long a read blocks before the reader loop checks its outbound queue.
/// This bounds write latency (keystrokes, resize) without a second thread
/// contending for the socket.
const READ_TICK: Duration = Duration::from_millis(25);

impl WsTransport {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> WsTransport {
        WsTransport {
            url: url.into(),
            token: token.into(),
            state: Arc::new(Mutex::new(WsState {
                tx: None,
                acked: false,
                pending: Vec::new(),
                subs: HashMap::new(),
                next_id: 1,
                generation: 0,
            })),
        }
    }

    fn ensure_connected(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.tx.is_some() {
            return Ok(());
        }

        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|e| Error::GraphQL {
                message: format!("invalid WebSocket URL {:?} — {e}", self.url),
                code: None,
            })?;
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static("graphql-transport-ws"),
        );
        let (mut socket, _response) =
            tungstenite::connect(request).map_err(|e| Error::GraphQL {
                message: format!("cannot reach the bsdkrun daemon at {} — {e}", self.url),
                code: None,
            })?;
        set_read_timeout(&mut socket);

        // The token travels in connection_init, not a header a real browser
        // could never set on a WS handshake anyway — parity with the other
        // SDKs and the web frontend.
        let init = json!({
            "type": "connection_init",
            "payload": {"authorization": format!("Bearer {}", self.token)},
        })
        .to_string();
        socket
            .send(Message::Text(init))
            .map_err(|e| Error::GraphQL {
                message: format!("the WebSocket connection failed — {e}"),
                code: None,
            })?;

        let (tx, rx) = mpsc::channel();
        state.tx = Some(tx);
        state.acked = false;
        state.pending.clear();
        state.generation += 1;
        let generation = state.generation;
        let shared = Arc::clone(&self.state);
        std::thread::Builder::new()
            .name("bsdkrun-ws-reader".to_string())
            .spawn(move || reader_loop(socket, rx, shared, generation))?;
        Ok(())
    }

    /// Start a subscription. Returns its id (pass to [`WsTransport::unsubscribe`]).
    pub fn subscribe(
        &self,
        query: &str,
        variables: Value,
        on_next: NextFn,
        on_error: ErrorFn,
        on_complete: CompleteFn,
    ) -> Result<String> {
        self.ensure_connected()?;
        let mut state = self.state.lock().unwrap();
        let id = state.next_id.to_string();
        state.next_id += 1;
        state.subs.insert(
            id.clone(),
            Arc::new(Mutex::new(Sub {
                on_next,
                on_error,
                on_complete,
            })),
        );
        let message = json!({
            "id": id,
            "type": "subscribe",
            "payload": {"query": query, "variables": variables},
        })
        .to_string();
        if state.acked {
            if let Some(tx) = &state.tx {
                let _ = tx.send(Out::Text(message));
            }
        } else {
            // Flushed once connection_ack arrives (see dispatch).
            state.pending.push(message);
        }
        Ok(id)
    }

    /// End a subscription; drops the socket once nothing is using it, so a
    /// later `subscribe` reconnects fresh rather than talking to a stale
    /// connection.
    pub fn unsubscribe(&self, id: &str) {
        let (tx, remaining) = {
            let mut state = self.state.lock().unwrap();
            if state.subs.remove(id).is_none() {
                return;
            }
            (state.tx.clone(), state.subs.len())
        };
        if let Some(tx) = tx {
            let _ = tx.send(Out::Text(json!({"id": id, "type": "complete"}).to_string()));
            if remaining == 0 {
                let _ = tx.send(Out::Shutdown);
                self.state.lock().unwrap().tx = None;
            }
        }
    }

    /// Close the socket without notifying subscriptions. Idempotent.
    pub fn close(&self) {
        let tx = {
            let mut state = self.state.lock().unwrap();
            state.subs.clear();
            state.pending.clear();
            state.acked = false;
            state.tx.take()
        };
        if let Some(tx) = tx {
            let _ = tx.send(Out::Shutdown);
        }
    }
}

fn set_read_timeout(socket: &mut ClientSocket) {
    match socket.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(READ_TICK));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.sock.set_read_timeout(Some(READ_TICK));
        }
        _ => {}
    }
}

fn is_timeout(err: &tungstenite::Error) -> bool {
    matches!(
        err,
        tungstenite::Error::Io(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut
    )
}

fn reader_loop(
    mut socket: ClientSocket,
    rx: Receiver<Out>,
    state: Arc<Mutex<WsState>>,
    generation: u64,
) {
    'outer: loop {
        // Flush everything queued for the wire before blocking in read again.
        loop {
            match rx.try_recv() {
                Ok(Out::Text(text)) => {
                    if socket.send(Message::Text(text)).is_err() {
                        break 'outer;
                    }
                }
                Ok(Out::Shutdown) | Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    break 'outer;
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                for reply in dispatch(&state, generation, &text) {
                    if socket.send(Message::Text(reply)).is_err() {
                        break 'outer;
                    }
                }
            }
            Ok(Message::Binary(bytes)) => {
                if let Ok(text) = String::from_utf8(bytes) {
                    for reply in dispatch(&state, generation, &text) {
                        if socket.send(Message::Text(reply)).is_err() {
                            break 'outer;
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => break 'outer,
            // WebSocket-level ping/pong is answered by tungstenite itself on
            // the next read/write; nothing to do here.
            Ok(_) => {}
            Err(ref e) if is_timeout(e) => {}
            Err(_) => break 'outer,
        }
    }
    on_socket_closed(&state, generation);
}

/// Handle one incoming graphql-transport-ws message. Returns any replies to
/// put on the wire (the pending-subscribe flush on ack, a pong for a ping).
fn dispatch(state: &Arc<Mutex<WsState>>, generation: u64, text: &str) -> Vec<String> {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("");

    match msg_type {
        "connection_ack" => {
            let mut st = state.lock().unwrap();
            if st.generation != generation {
                return Vec::new();
            }
            st.acked = true;
            std::mem::take(&mut st.pending)
        }
        "ping" => vec![json!({"type": "pong"}).to_string()],
        "next" | "error" | "complete" => {
            let Some(id) = msg.get("id").and_then(Value::as_str) else {
                return Vec::new();
            };
            // Clone the Arc out and release the state lock before invoking the
            // callback, so a callback that calls back into the transport
            // (unsubscribe, another subscribe) cannot deadlock.
            let sub = {
                let mut st = state.lock().unwrap();
                if st.generation != generation {
                    return Vec::new();
                }
                if msg_type == "next" {
                    st.subs.get(id).cloned()
                } else {
                    st.subs.remove(id)
                }
            };
            if let Some(sub) = sub {
                let mut callbacks = sub.lock().unwrap();
                match msg_type {
                    "next" => {
                        let data = msg.pointer("/payload/data").cloned().unwrap_or(Value::Null);
                        (callbacks.on_next)(data);
                    }
                    "error" => {
                        let detail = match msg.get("payload") {
                            Some(Value::Array(errors)) => errors
                                .iter()
                                .map(|e| {
                                    e.get("message")
                                        .and_then(Value::as_str)
                                        .map(str::to_string)
                                        .unwrap_or_else(|| e.to_string())
                                })
                                .collect::<Vec<_>>()
                                .join("; "),
                            Some(other) => other.to_string(),
                            None => "subscription error".to_string(),
                        };
                        (callbacks.on_error)(Error::GraphQL {
                            message: detail,
                            code: None,
                        });
                    }
                    _ => (callbacks.on_complete)(),
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn on_socket_closed(state: &Arc<Mutex<WsState>>, generation: u64) {
    let (was_acked, subs) = {
        let mut st = state.lock().unwrap();
        if st.generation != generation {
            return;
        }
        let was_acked = st.acked;
        let subs: Vec<_> = st.subs.drain().map(|(_, sub)| sub).collect();
        st.pending.clear();
        st.tx = None;
        st.acked = false;
        (was_acked, subs)
    };

    // An unacked close means the daemon rejected connection_init (a bad
    // token) and hung up before ever acknowledging it; an acked close is just
    // the connection going away later on.
    for sub in subs {
        let err = if was_acked {
            Error::GraphQL {
                message: "the connection to the daemon was closed".to_string(),
                code: None,
            }
        } else {
            Error::auth_default()
        };
        (sub.lock().unwrap().on_error)(err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_scheme_and_suffix() {
        assert_eq!(
            normalize_url("localhost:50052"),
            "http://localhost:50052/graphql"
        );
    }

    #[test]
    fn normalize_strips_trailing_slashes() {
        assert_eq!(
            normalize_url("http://host:50052/"),
            "http://host:50052/graphql"
        );
        assert_eq!(
            normalize_url("http://host:50052///"),
            "http://host:50052/graphql"
        );
    }

    #[test]
    fn normalize_leaves_existing_graphql_suffix() {
        assert_eq!(
            normalize_url("http://host:50052/graphql"),
            "http://host:50052/graphql"
        );
        assert_eq!(
            normalize_url("http://host:50052/graphql/"),
            "http://host:50052/graphql"
        );
    }

    #[test]
    fn normalize_preserves_https() {
        assert_eq!(
            normalize_url("https://host:50052"),
            "https://host:50052/graphql"
        );
    }

    #[test]
    fn normalize_trims_whitespace_and_keeps_empty_empty() {
        assert_eq!(
            normalize_url("  localhost:50052  "),
            "http://localhost:50052/graphql"
        );
        assert_eq!(normalize_url(""), "");
        assert_eq!(normalize_url("   "), "");
    }

    #[test]
    fn normalize_is_case_insensitive_about_scheme_and_suffix() {
        assert_eq!(
            normalize_url("HTTPS://host/GraphQL"),
            "HTTPS://host/GraphQL"
        );
    }

    #[test]
    fn ws_url_derivation() {
        assert_eq!(
            ws_url("http://host:50052/graphql"),
            "ws://host:50052/graphql/ws"
        );
        assert_eq!(
            ws_url("https://host:50052/graphql"),
            "wss://host:50052/graphql/ws"
        );
        assert_eq!(
            ws_url("http://host:50052/graphql/"),
            "ws://host:50052/graphql/ws"
        );
    }
}
