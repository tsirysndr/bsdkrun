use std::process::Command;

/// Locate libkrun and tell cargo how to link against it. libkrun exposes a C ABI
/// (`libkrun.dylib`/`libkrun.so`) backed by Hypervisor.framework on macOS and KVM
/// on Linux.
fn main() {
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
