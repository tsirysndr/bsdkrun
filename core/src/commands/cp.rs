//! `bsdkrun cp` — copy files and directories between the host and a running
//! machine, in the shape of `docker cp`: exactly one side carries an `ID:`
//! prefix, and `-` means the host's stdin/stdout.
//!
//! The transfer rides the exec agent that is already there, rather than a new
//! agent opcode: `cat` moves a file and `tar` moves a directory, the same way
//! `docker cp` streams a tar over its API. That buys two things worth more than
//! a tidier wire format — it works against machines that are *already running*
//! (including BSD guests whose agent was baked into the image long ago), and it
//! needs no protocol version negotiation across the eight SDKs.
//!
//! What it costs is a dependency on the guest's own tools: `/bin/sh` and `cat`
//! for a file, plus `tar` for `-r`. Every image that boots under bsdkrun already
//! has a shell — the generated init is a `#!/bin/sh` script — so in practice
//! only `-r` on a shell-less image is out of reach.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::agent;

use super::guest::{agent_error, agent_target};

/// One side of a copy. `-` is the host's stdin or stdout depending on which
/// side of the copy it lands on.
#[derive(Debug, PartialEq)]
enum Endpoint {
    Host(PathBuf),
    Stdio,
    Guest { id: String, path: String },
}

/// Split an argument into an endpoint, using docker's rule for the ambiguity
/// between `ID:/path` and a host path that happens to contain a colon: a
/// leading `/`, `./`, `../` or `~` means the host, and otherwise a colon before
/// the first `/` marks the machine reference.
fn parse_endpoint(arg: &str) -> Endpoint {
    if arg == "-" {
        return Endpoint::Stdio;
    }
    let looks_local = arg.starts_with('/')
        || arg.starts_with("./")
        || arg.starts_with("../")
        || arg.starts_with('~')
        || arg == "."
        || arg == "..";
    if !looks_local {
        if let Some((id, path)) = arg.split_once(':') {
            if !id.is_empty() && !id.contains('/') && !path.is_empty() {
                return Endpoint::Guest {
                    id: id.to_string(),
                    path: path.to_string(),
                };
            }
        }
    }
    Endpoint::Host(PathBuf::from(arg))
}

/// Copy between the host and a running machine.
///
/// Directory copies (`-r`) move *contents*: `cp -r ./src web:/app` leaves the
/// guest with `/app` holding what `./src` holds. That is a deliberate departure
/// from `docker cp`, whose answer depends on whether the destination already
/// exists — the rule here reads the same whether or not the target is there,
/// which is what an SDK's `fs.upload(local, remote)` needs to promise.
pub(crate) fn cmd_cp(src: &str, dst: &str, recursive: bool) -> Result<()> {
    match (parse_endpoint(src), parse_endpoint(dst)) {
        (Endpoint::Guest { id, path }, to @ (Endpoint::Host(_) | Endpoint::Stdio)) => {
            download(&id, &path, &to, recursive)
        }
        (from @ (Endpoint::Host(_) | Endpoint::Stdio), Endpoint::Guest { id, path }) => {
            upload(&id, &from, &path, recursive)
        }
        (Endpoint::Guest { .. }, Endpoint::Guest { .. }) => bail!(
            "cannot copy from one machine directly to another — copy to the host first:\n  \
             bsdkrun cp {src} ./tmpfile && bsdkrun cp ./tmpfile {dst}"
        ),
        _ => bail!(
            "one side must name a machine as ID:PATH — neither {src} nor {dst} does.\n  \
             bsdkrun cp ./main.py web:/app/main.py\n  \
             bsdkrun cp web:/var/log/app.log ./app.log"
        ),
    }
}

// --- host -> guest -----------------------------------------------------------

/// Write one file into the guest.
///
/// `$1` is the destination and `$2` the source basename. A destination that
/// ends in `/` or already names a directory takes the file *into* it, as `cp`
/// does; the parent is created either way, so writing to a path in a directory
/// the image doesn't have yet works without a separate mkdir.
const PUT_FILE: &str = r#"
d=$1
case "$d" in */) d=$d$2 ;; esac
[ -d "$d" ] && d=$d/$2
mkdir -p "$(dirname "$d")" || exit 1
exec cat > "$d"
"#;

/// Unpack a tar stream into the guest at `$1`, creating it if needed.
const PUT_DIR: &str = r#"
mkdir -p "$1" || exit 1
exec tar -xf - -C "$1"
"#;

fn upload(id: &str, from: &Endpoint, dst: &str, recursive: bool) -> Result<()> {
    let (vm, port) = agent_target(id)?;

    let (script, args, input): (&str, Vec<String>, Box<dyn std::io::Read + Send>) = match from {
        Endpoint::Stdio => {
            if recursive {
                bail!("-r needs a directory to read; stdin is a single stream");
            }
            (
                PUT_FILE,
                vec![dst.to_string(), "stdin".to_string()],
                Box::new(std::io::stdin()),
            )
        }
        Endpoint::Host(p) if recursive => {
            if !p.is_dir() {
                bail!(
                    "{} is not a directory (drop -r to copy a file)",
                    p.display()
                );
            }
            (PUT_DIR, vec![dst.to_string()], tar_from(p)?)
        }
        Endpoint::Host(p) => {
            if p.is_dir() {
                bail!("{} is a directory (use -r to copy it)", p.display());
            }
            let f = std::fs::File::open(p)
                .with_context(|| format!("opening {} to copy into the guest", p.display()))?;
            (PUT_FILE, vec![dst.to_string(), basename(p)], Box::new(f))
        }
        Endpoint::Guest { .. } => unreachable!("checked by cmd_cp"),
    };

    let (code, err) =
        agent::exec_stream(port, &sh(script, &args), Some(input), &mut std::io::sink())
            .map_err(|e| agent_error(&vm.kind, e))?;
    if code != 0 {
        bail!("{}", guest_failure(&vm.id, dst, code, &err, recursive));
    }
    Ok(())
}

/// A host-side `tar` whose stdout is the stream to feed the guest. Packs the
/// directory's *contents* (`-C dir .`), which is what makes `upload` land them
/// in the destination rather than one level below it.
fn tar_from(dir: &Path) -> Result<Box<dyn std::io::Read + Send>> {
    let mut child = Command::new("tar")
        // macOS `tar` is bsdtar, which stores each file's extended attributes
        // in a sidecar AppleDouble member — so an uploaded directory arrives in
        // the guest with a `._main.py` next to every `main.py`, and a `._.` at
        // its root. COPYFILE_DISABLE is the documented off switch; other tars
        // ignore the variable, so it is safe to set unconditionally.
        .env("COPYFILE_DISABLE", "1")
        .arg("-cf")
        .arg("-")
        .arg("-C")
        .arg(dir)
        .arg(".")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running tar to pack {}", dir.display()))?;
    Ok(Box::new(child.stdout.take().expect("tar stdout is piped")))
}

// --- guest -> host -----------------------------------------------------------

const GET_FILE: &str = r#"
[ -e "$1" ] || { echo "no such file: $1" >&2; exit 1; }
[ -d "$1" ] && { echo "$1 is a directory (use -r)" >&2; exit 1; }
exec cat -- "$1"
"#;

const GET_DIR: &str = r#"
[ -d "$1" ] || { echo "not a directory: $1" >&2; exit 1; }
exec tar -cf - -C "$1" .
"#;

fn download(id: &str, src: &str, to: &Endpoint, recursive: bool) -> Result<()> {
    let (vm, port) = agent_target(id)?;
    let script = if recursive { GET_DIR } else { GET_FILE };
    let argv = sh(script, &[src.to_string()]);

    // Each arm owns its sink for the duration of the transfer, so the file is
    // closed (or the untar reaped) before we look at the guest's exit code.
    let (code, err) = match to {
        Endpoint::Stdio => {
            if recursive {
                bail!("-r produces a directory tree; it cannot be written to stdout");
            }
            let stdout = std::io::stdout();
            let mut h = stdout.lock();
            agent::exec_stream(port, &argv, None, &mut h)
        }
        Endpoint::Host(p) if recursive => {
            std::fs::create_dir_all(p).with_context(|| format!("creating {}", p.display()))?;
            let mut child = Command::new("tar")
                .arg("-xf")
                .arg("-")
                .arg("-C")
                .arg(p)
                .stdin(Stdio::piped())
                .spawn()
                .with_context(|| format!("running tar to unpack into {}", p.display()))?;
            let mut sink = child.stdin.take().expect("tar stdin is piped");
            let res = agent::exec_stream(port, &argv, None, &mut sink);
            drop(sink); // EOF, or tar waits forever for more of the archive
            let status = child.wait().context("waiting for tar")?;
            if res.is_ok() && !status.success() {
                bail!(
                    "tar failed to unpack the copied directory into {}",
                    p.display()
                );
            }
            res
        }
        Endpoint::Host(p) => {
            // A destination directory takes the file into it, like `cp`.
            let target = if p.is_dir() || p.to_string_lossy().ends_with('/') {
                p.join(basename(Path::new(src)))
            } else {
                p.clone()
            };
            if let Some(parent) = target.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            let mut f = std::fs::File::create(&target)
                .with_context(|| format!("creating {}", target.display()))?;
            let res = agent::exec_stream(port, &argv, None, &mut f);
            // Don't leave a truncated (or empty) file standing in for a copy
            // that never happened — a later build step would read it as real.
            if matches!(&res, Ok((c, _)) if *c != 0) || res.is_err() {
                drop(f);
                let _ = std::fs::remove_file(&target);
            }
            res
        }
        Endpoint::Guest { .. } => unreachable!("checked by cmd_cp"),
    }
    .map_err(|e| agent_error(&vm.kind, e))?;

    if code != 0 {
        bail!("{}", guest_failure(&vm.id, src, code, &err, recursive));
    }
    Ok(())
}

// --- shared ------------------------------------------------------------------

/// Wrap a script as `sh -c SCRIPT sh ARGS…`. The paths travel as positional
/// parameters rather than interpolated text, so a name with a space, a quote or
/// a `$` in it is data to the shell instead of syntax.
fn sh(script: &str, args: &[String]) -> Vec<String> {
    let mut argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        script.to_string(),
        "sh".to_string(),
    ];
    argv.extend(args.iter().cloned());
    argv
}

fn basename(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string())
}

/// Turn a guest exit code into something that names the likely cause. 126/127
/// are the shell's own "can't execute"/"not found", which for this command
/// almost always means the image lacks `tar` rather than anything about the
/// path the user asked for.
fn guest_failure(id: &str, path: &str, code: i32, stderr: &str, recursive: bool) -> String {
    if recursive && (code == 126 || code == 127) {
        return format!(
            "machine {id} has no usable `tar`, which -r needs to move a directory.\n\
             Copy the files one at a time, or use an image that ships tar."
        );
    }
    if stderr.is_empty() {
        format!("copying {path} in machine {id} failed (guest exit {code})")
    } else {
        format!("copying {path} in machine {id} failed (guest exit {code}): {stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_machine_reference_needs_a_colon_before_any_slash() {
        assert_eq!(
            parse_endpoint("web:/app/main.py"),
            Endpoint::Guest {
                id: "web".into(),
                path: "/app/main.py".into()
            }
        );
        assert_eq!(
            parse_endpoint("6449536bd129:/etc/hosts"),
            Endpoint::Guest {
                id: "6449536bd129".into(),
                path: "/etc/hosts".into()
            }
        );
    }

    /// The colon is only a separator when the text in front of it could be an
    /// id. Anything that opens like a path is the host's, so a file named
    /// `notes:2026.txt` in the current directory still resolves locally.
    #[test]
    fn host_paths_win_the_colon_ambiguity() {
        for s in [
            "./notes:2026.txt",
            "/tmp/a:b",
            "../x:y",
            "~/notes:1",
            ".",
            "/app",
        ] {
            assert!(
                matches!(parse_endpoint(s), Endpoint::Host(_)),
                "{s:?} should be a host path"
            );
        }
    }

    #[test]
    fn dash_is_stdio() {
        assert_eq!(parse_endpoint("-"), Endpoint::Stdio);
    }

    /// A bare relative name has no leading `./`, so the rule has to fall back on
    /// "is there a colon before the first slash" — `src/main.rs` has none.
    #[test]
    fn bare_relative_paths_are_local() {
        assert!(matches!(parse_endpoint("src/main.rs"), Endpoint::Host(_)));
        assert!(matches!(parse_endpoint("main.py"), Endpoint::Host(_)));
    }

    #[test]
    fn paths_reach_the_shell_as_parameters_not_as_text() {
        let argv = sh(PUT_FILE, &["/tmp/a b'c".to_string(), "x".to_string()]);
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[3], "sh", "argv[3] becomes $0, so paths start at $1");
        assert_eq!(argv[4], "/tmp/a b'c");
        assert!(
            !argv[2].contains("/tmp/a b'c"),
            "the path must not be interpolated into the script"
        );
    }

    #[test]
    fn a_missing_tar_is_reported_as_such_only_for_directory_copies() {
        let msg = guest_failure("web", "/app", 127, "", true);
        assert!(msg.contains("no usable `tar`"), "{msg}");
        // The same code from a file copy means something else entirely.
        let msg = guest_failure("web", "/app/x", 127, "sh: cat: not found", false);
        assert!(msg.contains("cat: not found"), "{msg}");
    }
}
