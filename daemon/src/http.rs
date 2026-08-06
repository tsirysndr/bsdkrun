//! The HTTP front end: GraphQL over POST, subscriptions over WebSocket, and
//! the GraphiQL IDE.
//!
//! Authentication is the same bearer token gRPC uses — one credential, one
//! decision about who may drive this host. It arrives in one of two places
//! depending on transport:
//!
//!   * HTTP: the `Authorization: Bearer <token>` header.
//!   * WebSocket: the `connection_init` payload, because a browser cannot set
//!     headers on a WebSocket handshake. This is not a weaker path — the token
//!     is checked before any operation runs, and a socket that fails to
//!     authenticate is closed.

use std::net::SocketAddr;
use std::sync::Arc;

use async_graphql::http::GraphiQLSource;
use async_graphql::Data;
use async_graphql_axum::{GraphQLProtocol, GraphQLRequest, GraphQLResponse, GraphQLWebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::serve::ListenerExt;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};

use crate::auth::TokenAuth;
use crate::graphql::BsdkrunSchema;

#[derive(Clone)]
pub struct HttpState {
    pub schema: BsdkrunSchema,
    pub auth: Arc<TokenAuth>,
}

/// Build the router.
///
/// CORS is permissive because the daemon is already gated by a bearer token
/// that a browser will not attach on its own: there is no ambient credential
/// (no cookie, no session) for a hostile page to ride on, so same-origin policy
/// is not what is protecting this API. Being strict here would instead break
/// the ordinary case of a dev server on another port talking to the daemon.
pub fn router(schema: BsdkrunSchema, auth: Arc<TokenAuth>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/graphql", get(graphiql).post(graphql_post))
        // The same path upgraded is the subscription endpoint, which is what
        // graphql-ws clients expect by default.
        .route("/graphql/ws", get(graphql_ws))
        .route("/health", get(|| async { "ok" }))
        .with_state(HttpState { schema, auth })
        .layer(cors)
}

pub async fn serve(
    bind: SocketAddr,
    schema: BsdkrunSchema,
    auth: Arc<TokenAuth>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    // Disable Nagle on every accepted connection: an interactive shell sends
    // one small frame per keystroke, and coalescing those would add a round
    // trip of latency to typing.
    let listener = tokio::net::TcpListener::bind(bind).await?.tap_io(|tcp| {
        let _ = tcp.set_nodelay(true);
    });
    axum::serve(listener, router(schema, auth).into_make_service())
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// The GraphiQL IDE, served unauthenticated.
///
/// It is a static page that carries no data of its own; every query it issues
/// still needs the token. This matches gRPC reflection, which is also
/// anonymous — the schema is public, the machines are not.
async fn graphiql() -> impl IntoResponse {
    Html(
        GraphiQLSource::build()
            .endpoint("/graphql")
            .subscription_endpoint("/graphql/ws")
            .title("bsdkrun")
            .finish(),
    )
}

async fn graphql_post(
    State(state): State<HttpState>,
    headers: HeaderMap,
    req: GraphQLRequest,
) -> Response {
    let presented = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(strip_bearer)
        .or_else(|| {
            headers
                .get("x-bsdkrun-token")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
        });

    if !state.auth.matches(presented) {
        return unauthorized();
    }
    let resp: GraphQLResponse = state.schema.execute(req.into_inner()).await.into();
    resp.into_response()
}

/// The subscription endpoint.
///
/// The token is taken from the `connection_init` payload rather than a header,
/// since the browser WebSocket API cannot set one. The upgrade itself is
/// allowed unconditionally; `on_connection_init` rejects the connection before
/// any operation can run.
async fn graphql_ws(
    State(state): State<HttpState>,
    protocol: GraphQLProtocol,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade
        .protocols(async_graphql::http::ALL_WEBSOCKET_PROTOCOLS)
        .on_upgrade(move |socket| async move {
            let auth = state.auth.clone();
            GraphQLWebSocket::new(socket, state.schema.clone(), protocol)
                .on_connection_init(move |value| async move {
                    let presented = connection_init_token(&value);
                    if auth.matches(presented.as_deref()) {
                        Ok(Data::default())
                    } else {
                        Err(async_graphql::Error::new(
                            "unauthenticated: send {\"authorization\": \"Bearer <token>\"} \
                             in the connection_init payload",
                        ))
                    }
                })
                .serve()
                .await
        })
}

/// Pull a token out of a `connection_init` payload.
///
/// Clients differ on where they put it — `authorization` with a Bearer prefix
/// is the common convention, but a bare `token` key is widespread enough to be
/// worth accepting rather than making people debug an opaque failure.
fn connection_init_token(value: &serde_json::Value) -> Option<String> {
    let obj = value.as_object()?;
    for key in ["authorization", "Authorization"] {
        if let Some(v) = obj.get(key).and_then(|v| v.as_str()) {
            return Some(strip_bearer(v).unwrap_or(v.trim()).to_string());
        }
    }
    obj.get("token")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
}

fn strip_bearer(value: &str) -> Option<&str> {
    let (scheme, rest) = value.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [("www-authenticate", "Bearer")],
        // A GraphQL-shaped body, so a client's error handling works normally
        // instead of tripping over an unexpected content type.
        axum::Json(serde_json::json!({
            "errors": [{
                "message": "unauthenticated: send `Authorization: Bearer <token>`",
                "extensions": { "code": "UNAUTHENTICATED" }
            }]
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_the_bearer_prefix_case_insensitively() {
        assert_eq!(strip_bearer("Bearer abc"), Some("abc"));
        assert_eq!(strip_bearer("bearer abc"), Some("abc"));
        assert_eq!(strip_bearer("BEARER  abc "), Some("abc"));
        assert_eq!(strip_bearer("Basic abc"), None);
        assert_eq!(strip_bearer("abc"), None);
    }

    #[test]
    fn reads_the_token_from_every_shape_clients_send() {
        assert_eq!(
            connection_init_token(&json!({"authorization": "Bearer abc"})).as_deref(),
            Some("abc")
        );
        assert_eq!(
            connection_init_token(&json!({"Authorization": "Bearer abc"})).as_deref(),
            Some("abc")
        );
        // A bare value under `authorization`, without the scheme.
        assert_eq!(
            connection_init_token(&json!({"authorization": "abc"})).as_deref(),
            Some("abc")
        );
        assert_eq!(
            connection_init_token(&json!({"token": "abc"})).as_deref(),
            Some("abc")
        );
        assert_eq!(connection_init_token(&json!({})), None);
        assert_eq!(connection_init_token(&json!(null)), None);
    }
}
