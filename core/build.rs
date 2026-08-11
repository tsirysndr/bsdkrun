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
    if std::env::var_os("CARGO_FEATURE_SOLO5").is_some() {
        ensure_solo5_tender();
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

/// Build `solo5-hvt` (the Solo5 hvt tender) from the pinned `library/solo5`
/// submodule and drop it into `solo5-bin/` so `rust_embed` (see
/// `commands::solo5`) can bake it into this binary.
///
/// The tender is not a libkrun guest: it drives Hypervisor.framework (macOS,
/// via the HVF backend this fork adds) or KVM (Linux) itself, in its own
/// process. Embedding it is what makes `bsdkrun solo5` a single binary with
/// no separate Solo5 install — the same deal `pack` gets, and for the same
/// reason: an end user should never need the upstream toolchain.
///
/// `--disable-toolchain` builds *only* the tender. That matters: it stops
/// short of the cross-compiler and bindings, which would need `ld.lld` and
/// `llvm-objcopy` to emit ELF on a Mach-O host. Building a unikernel needs
/// those; running one does not, and this only ever runs one.
///
/// As with `ensure_pack_binary`, a host that cannot build it must still
/// produce a working `bsdkrun` — so every failure here leaves `solo5-bin/`
/// empty and reports why, rather than failing the build.
fn ensure_solo5_tender() {
    use std::path::Path;

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
    let src = Path::new(&manifest_dir).join("../library/solo5");
    let out_dir = Path::new(&manifest_dir).join("src/solo5-bin");
    let out_bin = out_dir.join(SOLO5_TENDER);

    // rust_embed's derive needs the folder to exist even with nothing in it.
    if std::fs::create_dir_all(&out_dir).is_err() {
        return;
    }

    // Watch the parts of the tree the tender is actually built from. Not the
    // whole submodule: `.git` alone is thousands of files, and cargo stats
    // every path it is handed on every build.
    for f in ["configure.sh", "GNUmakefile", "Makefile.common"] {
        println!("cargo:rerun-if-changed=../library/solo5/{f}");
    }
    for d in ["tenders", "include", "scripts"] {
        watch_tree(&src.join(d));
    }

    if !src.join("configure.sh").exists() {
        println!(
            "cargo:warning=library/solo5 is empty; building bsdkrun without solo5 support. \
             Run `git submodule update --init library/solo5` and rebuild to enable \
             `bsdkrun solo5`."
        );
        return;
    }

    // Only rebuild when something in the source tree is newer than the tender
    // we already have. build.rs re-runs whenever *any* of its watched inputs
    // change — a rebuilt web bundle, a touched pack/ — and a C build per
    // `cargo build` is a cost nobody asked for.
    if let (Some(bin), Some(newest)) = (mtime(&out_bin), newest_mtime(&src)) {
        if bin >= newest {
            return;
        }
    }

    // Build out of tree. In-tree would be simpler and incremental, but the
    // source is read-only under nix (and any vendored/packaged build), and
    // two concurrent `cargo build`s would race in the same directory.
    let build_dir = Path::new(&std::env::var("OUT_DIR").expect("set by cargo")).join("solo5-build");
    let _ = std::fs::remove_dir_all(&build_dir);
    if let Err(e) = copy_tree(&src, &build_dir) {
        println!("cargo:warning=failed to stage library/solo5 for building: {e}");
        return;
    }

    // `scripts/gen_version_h.sh` derives the version from `git describe`, and
    // the copy above deliberately leaves `.git` behind. Its documented
    // fallback for a tree outside git is `version.h.distrib`, which is exactly
    // the release-tarball case — so write one naming the pinned commit.
    let version = Command::new("git")
        .args(["-C", &src.to_string_lossy(), "rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let version_h = build_dir.join("include/version.h.distrib");
    if let Err(e) = std::fs::write(
        &version_h,
        format!(
            "/* Automatically generated by bsdkrun's core/build.rs, do not edit */\n\
             \n#ifndef __VERSION_H__\n#define __VERSION_H__\n\n\
             #define SOLO5_VERSION \"bsdkrun-{}-g{version}\"\n\n#endif\n",
            std::env::var("CARGO_PKG_VERSION").unwrap_or_default(),
        ),
    ) {
        println!("cargo:warning=failed to write {}: {e}", version_h.display());
        return;
    }

    // `--disable-elftool` too: elftool inspects unikernel ELFs, which is a
    // build-time concern. `commands::solo5` reads the manifest note itself.
    if !run_in(
        &build_dir,
        "./configure.sh",
        &["--disable-toolchain", "--disable-elftool"],
    ) {
        return;
    }
    let jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .to_string();
    if !run_in(&build_dir, "make", &["-j", &jobs]) {
        return;
    }

    // On macOS the tender is codesigned at link time with the
    // com.apple.security.hypervisor entitlement, without which
    // Hypervisor.framework refuses to create a VM. The signature lives inside
    // the Mach-O, so copying the bytes here — and again when the embedded copy
    // is extracted at runtime — carries it along intact.
    let built = build_dir.join("tenders/hvt").join(SOLO5_TENDER);
    match std::fs::copy(&built, &out_bin) {
        Ok(_) => {}
        Err(e) => println!(
            "cargo:warning=solo5 tender was not produced at {}: {e}",
            built.display()
        ),
    }
}

/// Run `prog` in `dir`, reporting failure as cargo warnings rather than
/// panicking. Returns whether it succeeded.
///
/// `output()` rather than `status()`, for the reason `ensure_pack_binary`
/// spells out: cargo captures a build script's stdio, so the tool's actual
/// complaint is swallowed and only "exited with 1" survives — useless exactly
/// when it matters. On Linux the usual complaint is a missing
/// `libseccomp-dev`, which the hvt tender needs for its syscall filter.
fn run_in(dir: &std::path::Path, prog: &str, args: &[&str]) -> bool {
    match Command::new(prog).current_dir(dir).args(args).output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            println!("cargo:warning=solo5: `{prog}` exited with {}", o.status);
            for line in String::from_utf8_lossy(&o.stderr)
                .lines()
                .chain(String::from_utf8_lossy(&o.stdout).lines())
                .filter(|l| !l.trim().is_empty())
            {
                println!("cargo:warning=  solo5: {line}");
            }
            false
        }
        Err(e) => {
            println!("cargo:warning=solo5: failed to run `{prog}`: {e}");
            false
        }
    }
}

/// Name of the embedded tender, which is also the file name the extracted copy
/// takes — the tender prints `basename(argv[0])` in its own diagnostics, so
/// keeping it makes those read as Solo5's rather than as bsdkrun's.
const SOLO5_TENDER: &str = "solo5-hvt";

fn mtime(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

/// The newest mtime anywhere under `dir`, ignoring `.git` (whose index is
/// rewritten by unrelated git commands and would force a rebuild each time).
fn newest_mtime(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    let mut newest = mtime(dir);
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        if e.file_name() == ".git" {
            continue;
        }
        let path = e.path();
        let t = if path.is_dir() {
            newest_mtime(&path)
        } else {
            mtime(&path)
        };
        if let Some(t) = t {
            if newest.is_none_or(|n| t > n) {
                newest = Some(t);
            }
        }
    }
    newest
}

/// Recursively copy `src` to `dst`, skipping `.git`. Preserves the executable
/// bit, which `configure.sh` and the scripts it calls need.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
