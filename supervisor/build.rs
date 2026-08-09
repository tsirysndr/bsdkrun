/// Re-emit libkrun's rpath for this binary. See `bsdkrun`'s build.rs — the
/// search path propagates from `bsdkrun-core`, the rpath cannot.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if let Some(dir) = std::env::var_os("DEP_KRUN_LIBDIR") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.to_string_lossy());
    }
}
