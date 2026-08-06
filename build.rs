use std::process::Command;

/// Locate libkrun and tell cargo how to link against it. libkrun exposes a C ABI
/// (`libkrun.dylib`/`libkrun.so`) backed by Hypervisor.framework on macOS and KVM
/// on Linux.
fn main() {
    ensure_web_assets();

    println!("cargo:rustc-link-lib=dylib=krun");
    println!("cargo:rerun-if-env-changed=LIBKRUN_PREFIX");
    println!("cargo:rerun-if-changed=build.rs");

    // Explicit override wins on any OS.
    if let Ok(prefix) = std::env::var("LIBKRUN_PREFIX") {
        if !prefix.is_empty() {
            link_dir(&format!("{prefix}/lib"));
            return;
        }
    }

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        macos_link();
    } else {
        linux_link();
    }
}

/// macOS: ask Homebrew where libkrun lives (from the `libkrun/krun` tap).
fn macos_link() {
    let prefix = Command::new("brew")
        .args(["--prefix", "libkrun"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|p| !p.is_empty());
    match prefix {
        Some(p) => link_dir(&format!("{p}/lib")),
        None => link_dir("/opt/homebrew/lib"),
    }
}

/// Linux: prefer pkg-config, else fall back to the usual system lib dirs.
fn linux_link() {
    if let Some(dir) = Command::new("pkg-config")
        .args(["--variable=libdir", "libkrun"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|p| !p.is_empty())
    {
        link_dir(&dir);
        return;
    }
    for dir in [
        "/usr/local/lib64",
        "/usr/local/lib",
        "/usr/lib64",
        "/usr/lib",
    ] {
        let p = std::path::Path::new(dir);
        if p.join("libkrun.so").exists() {
            link_dir(dir);
            return;
        }
    }
    // Last resort: let the linker's default search path find it.
    link_dir("/usr/local/lib");
}

fn link_dir(dir: &str) {
    println!("cargo:rustc-link-search=native={dir}");
    // Embed an rpath so the versioned shared library resolves at runtime.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
}

/// Tell cargo to rerun when any file under `dir` changes.
fn watch_tree(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            watch_tree(&path);
        }
    }
}

/// Make sure `web/dist` exists so `rust_embed` can compile.
///
/// The web UI is built with node (`make web`), which not every build of this
/// crate will have run — a plain `cargo build` from a fresh checkout must still
/// work. When the real bundle is missing we write a placeholder page saying so,
/// rather than failing the build or silently shipping an empty UI.
fn ensure_web_assets() {
    use std::path::Path;

    let dist = Path::new("web/dist");

    // Watch the whole bundle, not just index.html. rust_embed inlines every
    // file at compile time, so a rebuilt UI whose index.html happens to be
    // unchanged — or merely older, as after restoring a directory — would
    // otherwise leave the previous assets baked into the binary.
    println!("cargo:rerun-if-changed=web/dist");
    watch_tree(dist);

    if dist.join("index.html").exists() {
        return;
    }
    if std::fs::create_dir_all(dist).is_err() {
        return;
    }
    let _ = std::fs::write(
        dist.join("index.html"),
        r#"<!doctype html>
<html><head><meta charset="utf-8"><title>bsdkrun UI not built</title>
<style>body{font-family:ui-sans-serif,system-ui,sans-serif;background:#0b0b0f;color:#e5e7eb;
display:flex;align-items:center;justify-content:center;height:100vh;margin:0}
div{max-width:34rem;padding:2rem}code{background:#1f2937;padding:.15rem .4rem;border-radius:.25rem}
</style></head><body><div>
<h1>The web UI was not bundled</h1>
<p>This <code>bsdkrun</code> binary was built without the web interface.</p>
<p>Build it and rebuild:</p>
<pre><code>make web
cargo build --release</code></pre>
</div></body></html>
"#,
    );
}
