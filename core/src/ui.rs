//! `bsdkrun ui` — serve the bundled web interface.
//!
//! The SPA in `web/` is compiled into this binary, so the UI needs no node, no
//! separate web server and no install step: `bsdkrun ui` and a browser is the
//! whole story.
//!
//! It serves static assets only. The UI talks to a `bsdkrund` GraphQL API,
//! which it asks for on first run — so the page and the daemon are independent,
//! and one served page can drive a daemon on any host you have a token for.

use std::net::SocketAddr;

use actix_web::http::header;
use actix_web::{web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use anyhow::{Context, Result};
use rust_embed::RustEmbed;
use tracing::info;

/// The built SPA.
///
/// `build.rs` guarantees this directory exists — with a placeholder page when
/// the UI has not been built — so `cargo build` never depends on having run
/// node first.
///
/// The bundle lives at the repo root rather than inside this crate: it is a
/// property of the product, and `web/` is built by `make web`, not by cargo.
#[derive(RustEmbed)]
#[folder = "../web/dist"]
struct Assets;

/// Serve an embedded file, or fall back to `index.html`.
///
/// The fallback is what makes a single-page app work on a plain file server: a
/// deep link is a client-side route, not a file, so anything that is not an
/// asset has to return the app shell and let the router sort it out. Real
/// missing assets (a stale hashed bundle) are still a 404, since those live
/// under `/assets/`.
fn serve(path: &str) -> HttpResponse {
    let path = path.trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = Assets::get(candidate) {
        let mime = mime_guess::from_path(candidate).first_or_octet_stream();
        // Hashed filenames are content-addressed and safe to cache forever;
        // index.html must not be, or a rebuild would never reach the browser.
        let cache = if candidate.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return HttpResponse::Ok()
            .content_type(mime.as_ref())
            .insert_header((header::CACHE_CONTROL, cache))
            .body(file.data.into_owned());
    }

    if candidate.starts_with("assets/") {
        return HttpResponse::NotFound().body("not found");
    }

    match Assets::get("index.html") {
        Some(index) => HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .insert_header((header::CACHE_CONTROL, "no-cache"))
            .body(index.data.into_owned()),
        None => HttpResponse::InternalServerError()
            .body("the web UI was not bundled into this binary — build it with `make web`"),
    }
}

async fn index() -> impl Responder {
    serve("index.html")
}

async fn asset(req: HttpRequest) -> impl Responder {
    serve(req.path())
}

/// Run the server until interrupted.
#[actix_web::main]
pub async fn serve_ui(bind: SocketAddr, open: bool) -> Result<()> {
    // A binary built before the UI was compiled would otherwise serve the
    // placeholder and leave the user wondering; say so up front instead.
    if Assets::get("index.html").is_none() {
        anyhow::bail!(
            "this binary has no web UI bundled. Build it with `make web`, then rebuild bsdkrun."
        );
    }

    let url = if bind.ip().is_unspecified() {
        format!("http://localhost:{}", bind.port())
    } else {
        format!("http://{bind}")
    };

    println!("bsdkrun UI: {url}");
    println!();
    println!("  It will ask for a bsdkrund GraphQL URL and access token on first run.");
    println!("  Start one with:  bsdkrund --graphql-bind 127.0.0.1:50052");
    println!();

    if open {
        open_browser(&url);
    }

    info!(%bind, "serving the web UI");
    HttpServer::new(|| {
        App::new()
            .route("/", web::get().to(index))
            .default_service(web::get().to(asset))
    })
    .bind(bind)
    .with_context(|| format!("binding {bind}"))?
    .run()
    .await
    .context("serving the web UI")
}

/// Best-effort browser launch; never fatal, since the URL is printed anyway.
fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(target_os = "macos"))]
    let cmd = "xdg-open";

    let _ = std::process::Command::new(cmd)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
