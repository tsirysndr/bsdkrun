//! A tiny HTTP server whose only handler compiles and runs a WASI guest
//! module with wasmer, on every request, and serves back whatever it wrote
//! to stdout.
//!
//! Networking is entirely the *host's* — a plain std::net::TcpListener, no
//! different from ../unikraft-actix or ../unikraft-php. The wasm guest only
//! ever talks to the world through WASI stdout, captured in memory with
//! wasmer-wasix's `Pipe`. This sidesteps WASI sockets (still
//! experimental/unstable in wasmer-wasix) entirely, while still genuinely
//! exercising wasmer's compile-and-execute path on every connection.

use std::io::Write;
use std::net::TcpListener;

use virtual_fs::AsyncReadExt;
use wasmer::Module;
use wasmer_types::ModuleHash;
use wasmer_wasix::{
    runners::wasi::{RuntimeOrEngine, WasiRunner},
    Pipe,
};

const PORT: u16 = 8080;

/// The wasm32-wasip1 guest, built by the Dockerfile (see ../guest) and
/// embedded at compile time so the image ships a single binary with no
/// filesystem path juggling at boot.
static GUEST_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../guest/target/wasm32-wasip1/release/guest.wasm"
));

/// Compile and run the embedded guest module, capturing its WASI stdout.
///
/// A fresh `Engine` and `Module` are built on *every* call — nothing here is
/// cached across requests — so each HTTP request genuinely exercises
/// wasmer's singlepass compiler, not a value computed once at boot and
/// replayed.
fn run_guest() -> String {
    let engine = wasmer::Engine::default();
    let module = Module::new(&engine, GUEST_WASM).expect("compile guest wasm module");

    let (stdout_tx, mut stdout_rx) = Pipe::channel();
    {
        let mut runner = WasiRunner::new();
        runner.with_stdout(Box::new(stdout_tx));
        runner
            .run_wasm(
                RuntimeOrEngine::Engine(engine),
                "guest",
                module,
                ModuleHash::random(),
            )
            .expect("run guest wasm module");
        // `stdout_tx` is dropped here, at the end of the block, which closes
        // the write end of the pipe so the read below sees EOF instead of
        // hanging forever.
    }

    let mut out = String::new();
    virtual_mio::block_on(stdout_rx.read_to_string(&mut out)).expect("read guest stdout");
    out
}

fn main() {
    // argv is intentionally never inspected. libkrun appends its own words
    // (earlycon=..., tsi_hijack, a bare `--`) past the kernel command line's
    // `--` stop sequence, and Unikraft hands them to this binary as argv —
    // see ../unikraft-php/README.md. A fixed port is simpler and just as
    // correct as parsing an optional override out of argv would be.
    //
    // A single multi-threaded tokio runtime is built once and entered for
    // the process's whole lifetime. wasmer-wasix's WasiRunner needs one
    // (`run_wasm` builds and tears down its own otherwise, on every single
    // call) — entering it here up front means every `run_guest()` call below
    // reuses the same runtime instead of spinning one up and dropping it per
    // request.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let _guard = runtime.handle().clone().enter();

    let listener = TcpListener::bind(("0.0.0.0", PORT)).expect("bind TCP listener");
    println!("wasmer host listening on :{PORT}");

    // A sequential accept loop, like ../unikraft-php's server.php: one
    // connection at a time is plenty for an example, and it keeps this
    // binary's threading model as simple as possible on a platform (a
    // from-scratch Unikraft port) where nothing about the threading story
    // has been proven out before this example.
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("accept error: {err}");
                continue;
            }
        };

        let body = run_guest();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );

        if let Err(err) = stream.write_all(response.as_bytes()) {
            eprintln!("write error: {err}");
        }
    }
}
