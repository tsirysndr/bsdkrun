use std::process::Command;

/// Locate libkrun and tell cargo how to link against it. libkrun exposes a C ABI
/// (`libkrun.dylib`/`libkrun.so`) backed by Hypervisor.framework on macOS and KVM
/// on Linux.
///
/// `cargo:rustc-link-lib` and `-link-search` reach the final binary on their own,
/// but `cargo:rustc-link-arg` does *not* propagate out of a dependency's build
/// script — so the `-Wl,-rpath` that makes the versioned shared library resolve
/// at runtime has to be emitted by whichever crate is actually being linked.
/// This package declares `links = "krun"`, which lets it hand the directory to
/// dependents as `DEP_KRUN_LIBDIR`; `bsdkrun/build.rs` and `daemon/build.rs` read
/// it and emit the rpath themselves.
fn main() {
    if std::env::var_os("CARGO_FEATURE_UI").is_some() {
        ensure_web_assets();
    }
    if std::env::var_os("CARGO_FEATURE_PACK").is_some() {
        ensure_pack_binary();
    }

    println!("cargo:rerun-if-env-changed=LIBKRUN_PREFIX");
    println!("cargo:rerun-if-changed=build.rs");

    // Without `boot` there is no FFI compiled, so there is nothing to link and
    // nothing to tell dependents about: the crate builds on a host that has
    // never heard of libkrun.
    if std::env::var_os("CARGO_FEATURE_BOOT").is_none() {
        return;
    }

    println!("cargo:rustc-link-lib=dylib=krun");

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
    // Published to dependents as DEP_KRUN_LIBDIR; they turn it into an rpath.
    println!("cargo:libdir={dir}");
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
///
/// The bundle lives at the repo root, one level up from this crate, because it
/// is a property of the product rather than of this library.
fn ensure_web_assets() {
    use std::path::Path;

    let dist = Path::new("../web/dist");

    // Watch the whole bundle, not just index.html. rust_embed inlines every
    // file at compile time, so a rebuilt UI whose index.html happens to be
    // unchanged — or merely older, as after restoring a directory — would
    // otherwise leave the previous assets baked into the binary.
    println!("cargo:rerun-if-changed=../web/dist");
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

/// Compile `pack/` (the Go half of `bsdkrun pack`) and drop it into
/// `pack-bin/` so `rust_embed` (see `commands::pack`) can bake it into this
/// binary.
///
/// `pack` only ever needs to match the *host* triple already building this
/// crate — unlike the guest agent (`agent.rs`), which is downloaded per guest
/// (os, arch) because it has to match 3 different guest OSes it never runs
/// on. Building and embedding it here means the shipped `bsdkrun` binary
/// needs no Go toolchain of its own at runtime.
///
/// A fresh checkout without `go` on PATH (or a build that fails) must still
/// produce a working `bsdkrun` — same rule as `ensure_web_assets` — so this
/// leaves `pack-bin/` without a binary rather than failing the build;
/// `commands::pack::cmd_pack` reports that plainly instead of crashing.
fn ensure_pack_binary() {
    use std::path::Path;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
    let pack_src = Path::new(&manifest_dir).join("../pack");
    let out_dir = Path::new(&manifest_dir).join("src/pack-bin");
    let out_bin = out_dir.join(pack_binary_name());

    // rust_embed's derive needs the folder to exist even when there is
    // nothing to embed yet.
    if std::fs::create_dir_all(&out_dir).is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=../pack/go.mod");
    println!("cargo:rerun-if-changed=../pack/go.sum");
    watch_tree(&pack_src);

    if Command::new("go").arg("version").output().is_err() {
        println!(
            "cargo:warning=go toolchain not found on PATH; building bsdkrun without pack support \
             (install Go >= 1.22 and rebuild to enable `bsdkrun pack`)"
        );
        return;
    }

    // CGO_ENABLED=0: a fully static binary with no libc dependency, which is
    // what makes it safe to embed and exec on any host of this same triple.
    // `output()` rather than `status()`: cargo captures a build script's
    // stdio, so with `status()` the compiler's actual complaint is swallowed
    // and all that survives is "exited with 1" — which is useless precisely
    // when it matters. Re-emit each line as a cargo warning so a broken
    // `pack/` says why.
    let out = Command::new("go")
        .current_dir(&pack_src)
        .env("CGO_ENABLED", "0")
        .args(["build", "-trimpath", "-ldflags", "-s -w", "-o"])
        .arg(&out_bin)
        .arg(".")
        .output();

    match out {
        Ok(o) if o.status.success() => {}
        // Leave any previously-built binary in place rather than deleting a
        // working one over a transient failure.
        Ok(o) => {
            println!(
                "cargo:warning=`go build` for bsdkrun pack exited with {}; \
                 keeping the previously embedded binary, if any",
                o.status
            );
            for line in String::from_utf8_lossy(&o.stderr).lines() {
                println!("cargo:warning=  go: {line}");
            }
        }
        Err(e) => println!("cargo:warning=failed to run `go build` for bsdkrun pack: {e}"),
    }
}

fn pack_binary_name() -> &'static str {
    "bsdkrun-pack"
}
