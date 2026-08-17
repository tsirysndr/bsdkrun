//! Getting local files onto a *remote* engine's sandbox.
//!
//! Everything else in [`crate::ai`] resolves paths on the engine's host, which
//! is right when the engine is your own machine and useless when it is a VPS:
//! your skills, your keys and your project are on the laptop, and the sandbox
//! cannot see any of them.
//!
//! So they are uploaded. The client tars a directory ([`pack`]), the transport
//! carries the bytes, and the engine unpacks it into the place its sandboxes
//! already mount ([`receive`]) — no new mount plumbing, and a local engine can
//! use the same path without noticing.
//!
//! What each kind lands on is the whole design:
//!
//! | Kind | Unpacks to | So that |
//! | ---- | ---------- | ------- |
//! | [`Kind::Skills`] | the engine's shared skills dir | every agent on that host sees them, exactly as on a local engine |
//! | [`Kind::Ssh`] | the agent's home volume | `git push` works, and it survives across that agent's sessions |
//! | [`Kind::Workspace`] | `<state>/ai-workspaces/<name>` | there is a path to hand `--workspace`, which is what a sandbox mounts |

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::{home_dir, host_skills_dir, GUEST_HOME, SKILLS_DIR};
use crate::db;

/// What is being uploaded. Each kind has one destination, decided by the
/// engine rather than the caller — a client that could name an arbitrary path
/// would be a way to write anywhere on the daemon's host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// `~/.agents/skills` — shared by every agent on the engine.
    Skills,
    /// `~/.ssh` — into one agent's home volume, so its sandboxes can push.
    Ssh,
    /// A project directory, into a workspace the sandbox can mount.
    Workspace,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "skills" => Ok(Kind::Skills),
            "ssh" => Ok(Kind::Ssh),
            "workspace" | "project" | "dir" => Ok(Kind::Workspace),
            other => anyhow::bail!("unknown upload kind {other:?} (skills | ssh | workspace)"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Skills => "skills",
            Kind::Ssh => "ssh",
            Kind::Workspace => "workspace",
        }
    }

    /// The local directory this kind comes from, for a client packing it.
    pub fn local_source(self, workspace: Option<&Path>) -> Result<PathBuf> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .context("HOME is not set, so there is nothing to upload from")?;
        match self {
            Kind::Skills => Ok(home.join(SKILLS_DIR)),
            Kind::Ssh => Ok(home.join(".ssh")),
            Kind::Workspace => match workspace {
                Some(w) => Ok(w.to_path_buf()),
                None => std::env::current_dir().context("resolving the directory to upload"),
            },
        }
    }
}

/// Build directories skipped even when the project says nothing about them.
///
/// A backstop, not the mechanism: `.gitignore` and `.dockerignore` are read
/// first and are what actually decides most of a tree. This list only catches
/// the project that has no ignore file at all — a scratch directory, a fresh
/// checkout of something that commits its dependencies — where the difference
/// is a 2 MB upload versus a 2 GB one.
///
/// `.git` is deliberately absent: an agent that cannot see history is much
/// less useful, and it is usually smaller than `node_modules`.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".turbo",
    ".gradle",
    "vendor/bundle",
    ".terraform",
    "_build",
    "deps",
    "zig-cache",
    ".zig-cache",
];

/// A packed upload: the tar, plus what it cost, so a caller can report it.
#[derive(Debug)]
pub struct Packed {
    pub bytes: Vec<u8>,
    pub files: usize,
    /// Uncompressed bytes walked.
    pub size: u64,
    /// Directories left out because they are build output (see [`SKIP_DIRS`]).
    pub skipped: Vec<String>,
}

/// The cap on one upload. A few hundred MB of tar over a unary request is
/// already unreasonable; past that the answer is `--repo` (clone on the
/// engine) or a real sync tool, and saying so beats a request that times out.
pub const MAX_UPLOAD: u64 = 256 * 1024 * 1024;

/// The cap on entry count. Size alone does not catch the case that actually
/// hurts — a hundred thousand tiny files tar quickly and then take minutes to
/// unpack on the far side, with no progress and nothing to blame.
pub const MAX_ENTRIES: usize = 20_000;

/// Directories that are never a project, and whose upload is always a mistake.
///
/// `$HOME` is the one a person reaches for by accident: `bsdkrun claude` in a
/// login shell starts in it, so an `--upload` there would ship a browser
/// profile, a photo library and every key on the machine to a VPS. Refused
/// outright rather than merely capped, because the cap would stop it for the
/// wrong reason and a smaller home directory would sail through.
fn refuse_wholesale(source: &Path) -> Result<()> {
    // Resolved first: `~/..`, a symlink, and `.` from `$HOME` all name the
    // same directory and must all be refused.
    let path = std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());

    if path.parent().is_none() {
        anyhow::bail!("refusing to upload the filesystem root");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let home = std::fs::canonicalize(&home).unwrap_or_else(|_| PathBuf::from(&home));
        if path == home {
            anyhow::bail!(
                "refusing to upload your whole home directory to a remote sandbox — \
                 it holds your keys, your browser profile and everything else on this \
                 machine.\n\nUpload a project directory instead (cd into it first), or \
                 use `--repo <url>` to clone it on the engine.\nKeys and skills have \
                 their own uploads: `--what ssh` and `--what skills`."
            );
        }
        // An ancestor of $HOME is worse still: /Users, /home, /.
        if home.starts_with(&path) {
            anyhow::bail!(
                "refusing to upload {} — it contains your home directory. \
                 Name a project directory instead.",
                path.display()
            );
        }
    }
    // System trees. Uploading one is never the intent, and each is large
    // enough that the size cap would only be reached after a long walk.
    //
    // Both sides are canonicalized: on macOS `/etc`, `/var` and `/tmp` are
    // symlinks into `/private`, so comparing a resolved path against the
    // literal names matches none of them.
    const SYSTEM: &[&str] = &[
        "/usr",
        "/etc",
        "/var",
        "/nix",
        "/System",
        "/Library",
        "/Applications",
        "/Volumes",
        "/private",
        "/bin",
        "/sbin",
        "/opt",
        "/proc",
        "/sys",
        "/dev",
    ];
    let is_system = SYSTEM.iter().any(|s| {
        let sys = Path::new(s);
        path == sys
            || source == sys
            || std::fs::canonicalize(sys).is_ok_and(|resolved| resolved == path)
    });
    if is_system {
        anyhow::bail!(
            "refusing to upload the system directory {} — name a project directory instead",
            path.display()
        );
    }
    Ok(())
}

/// Tar a local directory, gzipped, skipping build output unless `everything`.
pub fn pack(source: &Path, everything: bool) -> Result<Packed> {
    if !source.is_dir() {
        anyhow::bail!("{} is not a directory", source.display());
    }
    refuse_wholesale(source)?;
    let mut skipped = Vec::new();
    let mut files = 0usize;

    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(gz);
    // Symlinks are stored as symlinks rather than followed: `~/.ssh` and a
    // project both routinely contain links, and following them can pull in a
    // parent directory (or loop).
    builder.follow_symlinks(false);

    let survey = walk(source, everything, &mut skipped)?;
    let total = survey.size;

    for entry in survey.entries {
        let rel = entry
            .strip_prefix(source)
            .expect("walk only yields paths under source");
        if rel.as_os_str().is_empty() {
            continue;
        }
        let meta = match std::fs::symlink_metadata(&entry) {
            Ok(m) => m,
            // Vanished between the walk and now — a build running in the
            // directory being uploaded is normal, and losing one temp file is
            // not a reason to fail the upload.
            Err(_) => continue,
        };
        if meta.is_dir() {
            builder.append_dir(rel, &entry)?;
        } else {
            builder
                .append_path_with_name(&entry, rel)
                .with_context(|| format!("packing {}", entry.display()))?;
            files += 1;
        }
    }

    let bytes = builder
        .into_inner()
        .context("finishing the upload archive")?
        .finish()
        .context("compressing the upload archive")?;
    Ok(Packed {
        bytes,
        files,
        size: total,
        skipped,
    })
}

/// What a walk found, and what it cost.
struct Survey {
    entries: Vec<PathBuf>,
    size: u64,
}

/// Walk the tree, honouring the project's own ignore files, and refuse to go
/// on once it is plainly too big to be a project.
///
/// `.gitignore` and `.dockerignore` decide what is sent. A project already
/// states what is derived, secret or huge, and restating it here would be a
/// second answer that drifts from the first — the same reason `pack` reads
/// `.dockerignore` rather than inferring a context.
///
/// Two deviations from a plain `git status`, both deliberate:
///
/// - **Hidden files are included.** `.env.example`, `.github/`, `.cargo/` and
///   `.git` itself are part of the project; an agent that cannot see them is
///   working on a different codebase than you are.
/// - **The machine-wide gitignore is not read.** `~/.config/git/ignore` is a
///   statement about one developer's editor droppings, not about what the
///   project is, and it is invisible to anyone diagnosing a missing file.
///
/// `.dockerignore` is matched with gitignore semantics, which is close but not
/// identical to Docker's (Go `filepath.Match`, patterns anchored at the context
/// root). It is an approximation, and the direction of error is to send a file
/// Docker would have excluded rather than to drop one it would have kept.
///
/// The caps are enforced *during* traversal rather than after it. A directory
/// that should never have been named — an external drive, `~/Movies` — has
/// millions of entries, and collecting them all before checking would hang for
/// minutes before printing an error. The failure has to arrive while it is
/// still cheap.
fn walk(root: &Path, everything: bool, skipped: &mut Vec<String>) -> Result<Survey> {
    let mut entries = Vec::new();
    let mut size = 0u64;
    // Per top-level directory, so the error can say *what* is large. "this is
    // 4.2 GB" sends you hunting; "node_modules is 4.1 GB of it" does not.
    let mut by_top: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_global(false)
        .git_ignore(!everything)
        .git_exclude(!everything)
        .ignore(!everything)
        // Apply ignore files even outside a git repository: a project that has
        // not been `git init`ed still means what its `.gitignore` says.
        .require_git(false)
        .parents(!everything)
        .follow_links(false);
    if !everything {
        builder.add_custom_ignore_filename(".dockerignore");
    }

    // The backstop list, applied as a filter so an ignored directory is never
    // descended into — `node_modules` costs nothing if it is not walked.
    let skip_root = root.to_path_buf();
    let mut skipped_dirs: Vec<String> = Vec::new();
    builder.filter_entry(move |e| {
        if everything {
            return true;
        }
        let is_dir = e.file_type().is_some_and(|t| t.is_dir());
        if !is_dir {
            return true;
        }
        let name = e.file_name().to_string_lossy();
        if SKIP_DIRS.contains(&name.as_ref()) && e.path() != skip_root {
            return false;
        }
        true
    });

    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            // An unreadable directory is skipped rather than fatal: a project
            // tree routinely has one, and failing the whole upload for it
            // would be worse than uploading the rest.
            Err(e) => {
                warn!("skipping an unreadable path: {e}");
                continue;
            }
        };
        let path = entry.path().to_path_buf();
        if path == root {
            continue;
        }

        if let Ok(meta) = entry.metadata() {
            if meta.is_file() {
                size += meta.len();
                if let Some(top) = path
                    .strip_prefix(root)
                    .ok()
                    .and_then(|r| r.components().next())
                {
                    *by_top
                        .entry(top.as_os_str().to_string_lossy().into_owned())
                        .or_default() += meta.len();
                }
            }
        }

        entries.push(path);
        if entries.len() > MAX_ENTRIES {
            anyhow::bail!(too_big(root, MAX_ENTRIES, &by_top, true));
        }
        if size > MAX_UPLOAD {
            anyhow::bail!(too_big(root, MAX_ENTRIES, &by_top, false));
        }
    }

    // Report the backstop skips by name — an upload missing `node_modules`
    // should say so rather than leave it to be discovered in the sandbox.
    if !everything {
        for name in SKIP_DIRS {
            if root.join(name).is_dir() {
                skipped_dirs.push((*name).to_string());
            }
        }
    }
    skipped.extend(skipped_dirs);
    Ok(Survey { entries, size })
}

/// The message for a directory too large to upload — including the three
/// biggest things in it, which is almost always the whole explanation.
fn too_big(
    root: &Path,
    max_entries: usize,
    by_top: &std::collections::HashMap<String, u64>,
    by_count: bool,
) -> String {
    let mut worst: Vec<(&String, &u64)> = by_top.iter().collect();
    worst.sort_by(|a, b| b.1.cmp(a.1));
    let biggest: Vec<String> = worst
        .iter()
        .take(3)
        .map(|(name, size)| format!("{name} ({})", crate::oci::human_size(**size)))
        .collect();

    let limit = if by_count {
        format!("more than {max_entries} files")
    } else {
        format!("more than {}", crate::oci::human_size(MAX_UPLOAD))
    };
    let mut msg = format!(
        "refusing to upload {}: it holds {}, which is too much to send to a \
         remote sandbox.",
        root.display(),
        limit
    );
    if !biggest.is_empty() {
        msg.push_str(&format!("\n\nLargest: {}.", biggest.join(", ")));
    }
    msg.push_str(
        "\n\nUpload a smaller directory, or use `--repo <url>` to clone the project on the \
         engine instead — that transfers nothing.",
    );
    msg
}

/// Where an upload lands on the engine's host.
pub fn destination(kind: Kind, agent: &str, name: Option<&str>) -> Result<PathBuf> {
    match kind {
        Kind::Skills => host_skills_dir()
            .context("this host has no HOME, so there is nowhere to put shared skills"),
        // Into the agent's home volume, which every sandbox of that agent
        // mounts at $HOME — so the keys are there for this session and the next.
        Kind::Ssh => Ok(home_dir(agent)?.join(".ssh")),
        Kind::Workspace => {
            let name = slug(name.unwrap_or("workspace"));
            let dir = db::state_dir()?.join("ai-workspaces").join(name);
            Ok(dir)
        }
    }
}

/// Unpack an upload into its destination, returning where it landed.
///
/// Replaces the destination rather than merging: a merge would leave a file
/// the user deleted locally sitting in the sandbox, which is the kind of
/// difference that wastes an afternoon. The one exception is `Skills`, which
/// is a shared store other agents may have added to.
pub fn receive(kind: Kind, agent: &str, name: Option<&str>, tar_gz: &[u8]) -> Result<PathBuf> {
    let dest = destination(kind, agent, name)?;
    if kind != Kind::Skills {
        crate::host::force_remove_dir_all(&dest);
    }
    std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;

    let gz = flate2::read::GzDecoder::new(tar_gz);
    let mut archive = tar::Archive::new(gz);
    archive.set_overwrite(true);
    // Reject entries that would escape the destination. `tar` already refuses
    // absolute paths and `..` by default, but this is the one place a remote
    // caller's bytes choose filenames on this host.
    archive.set_preserve_permissions(true);
    archive
        .unpack(&dest)
        .with_context(|| format!("unpacking into {}", dest.display()))?;

    // SSH refuses to use a key the world can read, and a tar that crossed
    // machines carries whatever mode it was written with.
    if kind == Kind::Ssh {
        tighten_ssh_permissions(&dest);
    }
    info!(kind = kind.as_str(), dest = %dest.display(), "received an upload");
    Ok(dest)
}

/// `chmod 700` the directory and `600` everything in it, so ssh will use it.
fn tighten_ssh_permissions(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let mode = if e.path().is_dir() { 0o700 } else { 0o600 };
                let _ = std::fs::set_permissions(e.path(), std::fs::Permissions::from_mode(mode));
            }
        }
    }
}

/// The guest path an uploaded workspace is mounted at.
///
/// Uploaded projects do not keep their host path (the laptop's `/Users/...`
/// means nothing on the engine), so they mount under `$HOME` by name —
/// predictable, and short enough for a prompt.
pub fn guest_workspace_path(name: &str) -> String {
    format!("{GUEST_HOME}/{}", slug(name))
}

/// A filesystem-safe directory name from a project's name.
fn slug(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(['-', '.']).to_string();
    if cleaned.is_empty() {
        "workspace".to_string()
    } else {
        cleaned
    }
}

/// Read a whole upload from a reader (the hidden `ai __receive` reads stdin).
pub fn read_all(mut r: impl Read) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf).context("reading the upload")?;
    Ok(buf)
}

/// Write an upload to a writer, for a client piping it to a remote engine.
pub fn write_all(mut w: impl Write, bytes: &[u8]) -> Result<()> {
    w.write_all(bytes).context("sending the upload")?;
    w.flush().context("flushing the upload")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_names() {
        for k in [Kind::Skills, Kind::Ssh, Kind::Workspace] {
            assert_eq!(Kind::parse(k.as_str()).unwrap(), k);
        }
        // The aliases a person would actually type for a project directory.
        assert_eq!(Kind::parse("project").unwrap(), Kind::Workspace);
        assert_eq!(Kind::parse("dir").unwrap(), Kind::Workspace);
        assert!(Kind::parse("everything").is_err());
    }

    #[test]
    fn slug_makes_a_usable_directory_name() {
        assert_eq!(slug("my app"), "my-app");
        assert_eq!(slug("/weird/../name"), "weird-..-name");
        assert_eq!(slug("..."), "workspace");
        assert_eq!(slug(""), "workspace");
    }

    #[test]
    fn pack_skips_build_output_and_round_trips() {
        let tmp = std::env::temp_dir().join(format!("bsdkrun-upload-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::create_dir_all(tmp.join("node_modules/left-pad")).unwrap();
        std::fs::write(tmp.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(tmp.join("node_modules/left-pad/index.js"), b"nope").unwrap();

        let packed = pack(&tmp, false).unwrap();
        assert_eq!(packed.files, 1, "only src/main.rs should be packed");
        assert_eq!(packed.skipped, vec!["node_modules".to_string()]);

        // And it unpacks to the same tree.
        let out = tmp.join("unpacked");
        std::fs::create_dir_all(&out).unwrap();
        let gz = flate2::read::GzDecoder::new(&packed.bytes[..]);
        tar::Archive::new(gz).unpack(&out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("src/main.rs")).unwrap(),
            "fn main() {}"
        );
        assert!(!out.join("node_modules").exists());

        // `--all` includes what was skipped.
        let everything = pack(&tmp, true).unwrap();
        assert!(everything.files > packed.files);
        assert!(everything.skipped.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gitignore_and_dockerignore_are_respected() {
        let tmp = std::env::temp_dir().join(format!("bsdkrun-upload-ign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::create_dir_all(tmp.join("build")).unwrap();
        std::fs::create_dir_all(tmp.join("secrets")).unwrap();

        std::fs::write(tmp.join(".gitignore"), "build/\n*.log\n!keep.log\n").unwrap();
        std::fs::write(tmp.join(".dockerignore"), "secrets/\n").unwrap();
        std::fs::write(tmp.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(tmp.join("build/out.bin"), b"derived").unwrap();
        std::fs::write(tmp.join("debug.log"), b"noise").unwrap();
        std::fs::write(tmp.join("keep.log"), b"wanted").unwrap();
        std::fs::write(tmp.join("secrets/key.pem"), b"private").unwrap();
        // Hidden files are project content, not droppings.
        std::fs::write(tmp.join(".env.example"), b"KEY=").unwrap();

        let out = tmp.join("unpacked");
        let packed = pack(&tmp, false).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        tar::Archive::new(flate2::read::GzDecoder::new(&packed.bytes[..]))
            .unpack(&out)
            .unwrap();

        assert!(out.join("src/main.rs").exists(), "source must be uploaded");
        assert!(out.join(".env.example").exists(), "dotfiles are content");
        assert!(!out.join("build").exists(), ".gitignore: build/");
        assert!(!out.join("debug.log").exists(), ".gitignore: *.log");
        assert!(
            out.join("keep.log").exists(),
            "gitignore negation (!keep.log) must be honoured — a hand-rolled \
             matcher is exactly where this breaks"
        );
        assert!(
            !out.join("secrets").exists(),
            ".dockerignore must be read too"
        );

        // `--all` overrides every ignore file, which is the escape hatch for a
        // project whose .gitignore hides something the agent needs.
        let out_all = tmp.join("unpacked-all");
        let everything = pack(&tmp, true).unwrap();
        std::fs::create_dir_all(&out_all).unwrap();
        tar::Archive::new(flate2::read::GzDecoder::new(&everything.bytes[..]))
            .unpack(&out_all)
            .unwrap();
        assert!(out_all.join("build/out.bin").exists());
        assert!(out_all.join("secrets/key.pem").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_home_directory_is_refused() {
        let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
            return; // Nothing to assert about a host with no HOME.
        };
        let home = PathBuf::from(home);

        let err = refuse_wholesale(&home).unwrap_err().to_string();
        assert!(
            err.contains("whole home directory"),
            "expected a home-directory refusal, got: {err}"
        );

        // And through a path that only resolves to it — this is the one a
        // person actually hits, from a subdirectory.
        let indirect = home.join("..").join(
            home.file_name()
                .map(|n| n.to_owned())
                .unwrap_or_else(|| "x".into()),
        );
        if indirect.exists() {
            assert!(
                refuse_wholesale(&indirect).is_err(),
                "{} resolves to $HOME and must be refused too",
                indirect.display()
            );
        }

        // The parent of $HOME (/Users, /home) is refused as containing it.
        if let Some(parent) = home.parent() {
            let err = refuse_wholesale(parent).unwrap_err().to_string();
            assert!(
                err.contains("contains your home directory") || err.contains("system directory"),
                "expected {} to be refused, got: {err}",
                parent.display()
            );
        }

        // A project directory under $HOME is fine — the guard must not be so
        // broad that it blocks the actual use case.
        let project = std::env::temp_dir();
        assert!(refuse_wholesale(&project).is_ok() || project.starts_with("/private/var"));
    }

    #[test]
    fn the_filesystem_root_and_system_trees_are_refused() {
        assert!(refuse_wholesale(Path::new("/")).is_err());
        for sys in ["/usr", "/etc", "/System"] {
            if Path::new(sys).exists() {
                assert!(
                    refuse_wholesale(Path::new(sys)).is_err(),
                    "{sys} should be refused"
                );
            }
        }
    }

    #[test]
    fn an_oversized_tree_fails_before_it_is_packed() {
        let tmp = std::env::temp_dir().join(format!("bsdkrun-upload-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("many")).unwrap();
        // Cheap to create, and over the entry cap — the count limit is what
        // catches a directory of small files, where size alone would not.
        for i in 0..(MAX_ENTRIES + 10) {
            std::fs::write(tmp.join("many").join(format!("f{i}")), b"x").unwrap();
        }
        let err = pack(&tmp, false).unwrap_err().to_string();
        assert!(
            err.contains("too much to send") && err.contains("many"),
            "the error should name the limit and the offender, got: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
