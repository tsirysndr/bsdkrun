//! The committed `flavors/` tree must match the flavor catalog.
//!
//! A test rather than a CLI invocation, because the CLI links libkrun and CI
//! has no reason to install a hypervisor to compare two strings. `bsdkrun`
//! depends on `bsdkrun-core` with default features on, so
//! `cargo run -p bsdkrun --no-default-features` does *not* turn `boot` off —
//! it fails at `-lkrun`, which is what this replaces. Run it with:
//!
//! ```sh
//! cargo test -p bsdkrun-core --no-default-features --test flavor_dockerfiles
//! ```
//!
//! Regenerate with `bsdkrun flavor __dockerfiles` (needs a built CLI, so it
//! happens on a developer's machine, not in the check).

use std::path::{Path, PathBuf};

fn tree() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("core/ has a parent")
        .join("flavors")
}

#[test]
fn generated_dockerfiles_match_the_catalog() {
    let root = tree();
    let files = bsdkrun_core::flavors::dockerfiles();
    assert!(
        !files.is_empty(),
        "the catalog produced no Dockerfiles at all — has `dockerfile()` stopped matching?"
    );

    let mut stale = Vec::new();
    for (name, contents) in &files {
        let path = root.join(name).join("Dockerfile");
        match std::fs::read_to_string(&path) {
            Ok(on_disk) if on_disk == *contents => {}
            Ok(_) => stale.push(format!("{name} (differs)")),
            Err(_) => stale.push(format!("{name} (missing)")),
        }
    }

    // A removed flavor leaves its directory behind, and the publish workflow
    // builds every directory it finds — so a stale one keeps pushing an image
    // that nothing can launch.
    if let Ok(entries) = std::fs::read_dir(&root) {
        let known: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && !known.contains(&name.as_str()) {
                stale.push(format!("{name} (no such flavor)"));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "the generated Dockerfiles are out of date ({}) — run \
         `bsdkrun flavor __dockerfiles` and commit the result",
        stale.join(", ")
    );
}

/// `flavors/`, not `images/` — the latter is git-ignored (it holds guest disk
/// images), so a tree generated there is silently never committed and the
/// publish workflow fails with `ls: cannot access 'images'`.
#[test]
fn the_tree_is_committed_where_git_can_see_it() {
    let root = tree();
    assert!(
        root.is_dir(),
        "{} does not exist — regenerate with `bsdkrun flavor __dockerfiles`",
        root.display()
    );
    assert!(
        !root.parent().unwrap().join("images/claude-code").exists(),
        "a generated tree is still under images/, which .gitignore excludes"
    );
}
