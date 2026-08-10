//! The WASI guest, compiled to wasm32-wasip1 and run by ../app's host binary
//! with wasmer on every incoming HTTP request. It does nothing but write a
//! line to stdout and exit — the host captures that output over a wasmer-wasix
//! `Pipe` and serves it back as the HTTP response body.

fn main() {
    println!("Hello from Wasmer on Unikraft!");
}
