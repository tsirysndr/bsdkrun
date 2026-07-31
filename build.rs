use std::process::Command;

/// Locate the Homebrew-installed libkrun and tell cargo how to link against it.
/// libkrun ships a C ABI (libkrun.dylib) backed by Hypervisor.framework on macOS.
fn main() {
    // Allow override, otherwise ask Homebrew where libkrun lives.
    let prefix = std::env::var("LIBKRUN_PREFIX").ok().or_else(|| {
        Command::new("brew")
            .args(["--prefix", "libkrun"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    });

    if let Some(prefix) = prefix {
        println!("cargo:rustc-link-search=native={prefix}/lib");
        // Embed an rpath so the versioned dylib resolves at runtime.
        println!("cargo:rustc-link-arg=-Wl,-rpath,{prefix}/lib");
    } else {
        // Fall back to the standard Homebrew lib dir on Apple Silicon.
        println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/lib");
    }

    println!("cargo:rustc-link-lib=dylib=krun");
    println!("cargo:rerun-if-env-changed=LIBKRUN_PREFIX");
    println!("cargo:rerun-if-changed=build.rs");
}
