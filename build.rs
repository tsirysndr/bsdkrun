/// Re-emit libkrun's rpath for this binary.
///
/// `bsdkrun-core` finds libkrun and publishes its directory as `DEP_KRUN_LIBDIR`
/// (it declares `links = "krun"`). The search path and `-lkrun` propagate from
/// there on their own, but `cargo:rustc-link-arg` does not cross a package
/// boundary — so the rpath, without which the versioned shared library will not
/// resolve at runtime, has to be emitted here where the linking happens.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if let Some(dir) = std::env::var_os("DEP_KRUN_LIBDIR") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.to_string_lossy());
    }
}
