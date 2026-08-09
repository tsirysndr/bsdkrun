//! Talking to a running guest: `logs`, `shell`, `exec`, and the agent-backed
//! `ssh` / `tailscale` / `systemd` helpers.

use anyhow::{Context, Result};

use crate::{agent, console, db, host};

pub(crate) fn cmd_logs(id: &str, follow: bool, boot: bool) -> Result<()> {
    use std::io::Write;
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let console_log = vdir.join("console.log");
    let boot_log = vdir.join("bsdkrun.log");

    // `--boot`: bsdkrun/libkrun's own log (fd 2 of the detached child) — the boot
    // diagnostics and any error that killed the machine before it reached console.
    if boot {
        match std::fs::read(&boot_log) {
            Ok(data) => {
                std::io::stdout().write_all(&data).ok();
                std::io::stdout().flush().ok();
                return Ok(());
            }
            Err(_) => anyhow::bail!(
                "no boot log for {} (only detached machines, run with -d, have one)",
                vm.id
            ),
        }
    }

    let console_data = std::fs::read(&console_log).unwrap_or_default();
    if !console_data.is_empty() {
        std::io::stdout().write_all(&console_data).ok();
        std::io::stdout().flush().ok();
    } else if !follow {
        // No guest console output — the machine may have died during boot. Fall
        // back to the boot log so the failure is actually visible (this is what
        // bit NetBSD-under-libkrun: an empty console but a real error in the log).
        if let Ok(boot_data) = std::fs::read(&boot_log) {
            if !boot_data.is_empty() {
                eprintln!(
                    "── no guest console output; showing boot log ({}) ──",
                    boot_log.display()
                );
                std::io::stdout().write_all(&boot_data).ok();
                std::io::stdout().flush().ok();
                return Ok(());
            }
        }
        anyhow::bail!(
            "no console log for {} (only detached machines, run with -d, have one)",
            vm.id
        );
    }
    if follow {
        console::follow(&vdir)?;
    }
    Ok(())
}

pub(crate) fn cmd_shell(id: &str) -> Result<()> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    reject_unikraft(&vm, "open a shell in")?;
    if !vm.pid.map(db::pid_alive).unwrap_or(false) {
        anyhow::bail!("machine {} is not running", vm.id);
    }
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    // Prefer the guest agent (a fresh interactive shell over TCP). Fall back to
    // the persistent-console attach for machines booted without an agent port.
    if let Some(port) = agent::read_port(&vdir) {
        let argv = interactive_shell_argv();
        let env = interactive_shell_env(&vm.kind);
        let code = agent::exec(port, &argv, &env, true).map_err(|e| agent_error(&vm.kind, e))?;
        std::process::exit(code);
    }
    if !vm.detached {
        anyhow::bail!("`shell` attaches to a detached machine — start it with `-d`");
    }
    console::attach_interactive(&vdir)
}

/// argv that opens an interactive shell inside a guest, preferring `bash` when
/// it's on the guest's PATH and falling back to `/bin/sh` otherwise. The choice
/// is made in the guest (only it knows its PATH) via a `/bin/sh -c` wrapper that
/// `exec`s the winner so it inherits the agent's PTY. `/bin/sh` exists on
/// Alpine/busybox Linux images and on FreeBSD/NetBSD.
/// Shell snippet: on NetBSD, set `PKG_PATH` to the pkgsrc binary-package CDN for
/// this release so `pkg_add` (and thus `pkg_add pkgin`) works — an unset
/// `PKG_PATH` is exactly why `pkg_add pkgin` fails on a fresh guest. A `-current`
/// release (e.g. `11.99.7`) has no packages of its own, so use the matching
/// `<major>.0` stable branch (`11.0`), which pkg_add installs with a harmless
/// "different platform" warning. `x86_64` maps to the pkgsrc port `amd64`.
pub(crate) const NETBSD_PKG_PATH_SETUP: &str = "if [ -z \"$PKG_PATH\" ] && [ \"$(uname 2>/dev/null)\" = NetBSD ]; then __a=$(uname -p 2>/dev/null); [ \"$__a\" = x86_64 ] && __a=amd64; export PKG_PATH=\"https://cdn.NetBSD.org/pub/pkgsrc/packages/NetBSD/$__a/$(uname -r 2>/dev/null | cut -d. -f1).0/All/\"; fi;";

pub(crate) fn interactive_shell_argv() -> Vec<String> {
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        // Add /usr/local/bin (FreeBSD `pkg`) and /usr/pkg/bin (NetBSD pkgsrc) to
        // PATH; on NetBSD, point `pkg_add` at the pkgsrc CDN for this release
        // (`-current` like 11.99.x uses the matching `<major>.0` branch) so
        // `pkg_add pkgin` works; `cd` into a cloned repo; then hand off to a shell.
        format!(
            "export PATH=\"/usr/local/bin:/usr/local/sbin:/usr/pkg/bin:/usr/pkg/sbin:$PATH\"; \
             [ \"$(uname 2>/dev/null)\" = Linux ] || export TERM=xterm; \
             {NETBSD_PKG_PATH_SETUP} \
             cd \"$(cat /etc/bsdkrun-cwd 2>/dev/null)\" 2>/dev/null; \
             if command -v bash >/dev/null 2>&1; then exec bash; else exec /bin/sh; fi"
        ),
    ]
}

/// Extra environment for an interactive shell over the agent. FreeBSD/NetBSD
/// guests boot with no `TERM` set on the agent PTY, which leaves line editing
/// and full-screen tools broken; `xterm` is in both guests' terminfo and gives
/// color plus the usual key sequences. Linux images set their own `TERM`, so
/// leave them alone.
pub(crate) fn interactive_shell_env(kind: &str) -> Vec<String> {
    if is_bsd(kind) {
        vec!["TERM=xterm".to_string()]
    } else {
        Vec::new()
    }
}

/// Whether a machine is a non-Linux guest (where we can't auto-inject the agent
/// — the user installs and starts `bsdkrun-agent` in the guest themselves).
pub fn is_bsd(kind: &str) -> bool {
    kind != "linux"
}

/// `TERM` for an interactive session on a guest of this kind.
///
/// FreeBSD and NetBSD guests boot with no `TERM` on the agent's pty, which
/// leaves the shell in `dumb` mode — no line editing, no colour, no key
/// sequences. `xterm` is in both guests' terminfo. Linux images set their own,
/// so they are left alone.
///
/// `shell` injects this itself; an explicit `exec` runs verbatim with no
/// injected env by design, so anything driving `exec` for an interactive
/// terminal — the daemon, the desktop app — has to supply it.
pub fn interactive_term(kind: &str) -> Option<String> {
    is_bsd(kind).then(|| "TERM=xterm".to_string())
}

/// Normalize a machine's internal boot-mode kind (`firmware`/`kernel`/`linux`) to
/// a guest-OS label (`freebsd`/`netbsd`/`linux`) — for display and for deciding
/// how to boot a committed snapshot.
pub fn guest_os_kind(kind: &str, image: &str) -> &'static str {
    let img = image.to_ascii_lowercase();
    // Unikernel kinds are exact — checked before the image-name heuristics so
    // an image named e.g. `freebsd-tools` can never mislabel one.
    if kind == "unikraft" {
        "unikraft"
    } else if kind == "nanos" {
        "nanos"
    } else if kind == "osv" {
        "osv"
    } else if kind == "freebsd" || kind == "firmware" || img.starts_with("freebsd") {
        "freebsd"
    } else if kind == "netbsd" || kind == "kernel" || img.starts_with("netbsd") {
        "netbsd"
    } else {
        "linux"
    }
}

/// Refuse an operation that a unikernel fundamentally cannot support.
///
/// A Unikraft guest is the application linked into the kernel: there is no
/// disk to snapshot and no userland for the agent to run a shell or command in.
/// Failing here with the reason beats letting the caller hang waiting for an
/// agent that will never answer.
pub(crate) fn reject_unikraft(vm: &db::MachineRow, what: &str) -> Result<()> {
    let flavor = match vm.kind.as_str() {
        "unikraft" => "a Unikraft",
        "nanos" => "a Nanos",
        "osv" => "an OSv",
        _ => return Ok(()),
    };
    anyhow::bail!(
        "cannot {what} {} — it is {flavor} unikernel: the application *is* the kernel, \
         so there is no shell or agent to talk to, and snapshots are unsupported. \
         Use `logs` to see its output.",
        vm.id
    );
}

/// Add a guest-specific hint to an agent connection/exec failure.
pub(crate) fn agent_error(kind: &str, e: anyhow::Error) -> anyhow::Error {
    if is_bsd(kind) {
        // Guest arch == host arch under KVM/HVF.
        let arch = host::Arch::current().unwrap_or(host::Arch::Aarch64);
        anyhow::anyhow!(
            "{e}\n\nBSD guests don't run the exec agent automatically. Download the agent for \
             your guest from the bsdkrun GitHub release:\n  \
             FreeBSD: {}\n  \
             NetBSD:  {}\n\
             then copy it into the running microVM and start it (it listens on TCP port {}): \
             `./bsdkrun-agent &`. bsdkrun forwards a host port to it automatically.",
            agent::asset_url(host::GuestOs::Freebsd, arch),
            agent::asset_url(host::GuestOs::Netbsd, arch),
            agent::GUEST_PORT,
        )
    } else {
        e
    }
}

/// Resolve a running machine and its exec-agent port (shared by exec/tailscale).
pub(crate) fn agent_target(id: &str) -> Result<(db::MachineRow, u16)> {
    let db = db::Db::open()?;
    let vm = db.find_machine(id)?;
    // Covers exec/tailscale/ssh/systemd/agent-update in one place: they all
    // resolve their target through here, and none of them can work.
    reject_unikraft(&vm, "run commands in")?;
    if !vm.pid.map(db::pid_alive).unwrap_or(false) {
        anyhow::bail!("machine {} is not running", vm.id);
    }
    let vdir = std::path::PathBuf::from(&vm.state_dir);
    let port = agent::read_port(&vdir).ok_or_else(|| {
        anyhow::anyhow!(
            "machine {} has no exec agent port — it was booted with networking disabled \
             (--no-net), which the agent needs",
            vm.id
        )
    })?;
    Ok((vm, port))
}

/// Run a command inside a running machine via its guest agent.
pub(crate) fn cmd_exec(id: &str, command: &[String], env: &[String], tty: bool) -> Result<()> {
    let (vm, port) = agent_target(id)?;
    let code = agent::exec(port, command, env, tty).map_err(|e| agent_error(&vm.kind, e))?;
    std::process::exit(code);
}

/// Refresh the in-guest agent binary to the current release. Some bundled BSD
/// images bake in an agent that predates the `ssh`/`tailscale` subcommands, so
/// invoking those makes the old agent try to `listen` again (Address already in
/// use). The running daemon can still `exec`, so we have it download the current
/// binary (via the guest's own fetch tool) and replace the file in place — the
/// next ssh/tailscale/exec then spawns the fresh binary. No restart needed.
pub(crate) fn cmd_agent_update(id: &str) -> Result<()> {
    let (vm, port) = agent_target(id)?;
    // A unikernel is the application linked into the kernel: there is no
    // userland to install an agent into, and no filesystem to write it to.
    // Without this the command picks a guest OS by heuristic and tries to
    // push a binary into a guest that cannot run it.
    reject_unikraft(&vm, "install an agent in")?;
    let arch = host::Arch::current()?;

    // (guest OS, install path, download command incl. its output flag, and the
    // ELF interpreter that proves the binary really is for this guest).
    //
    // Route through `guest_os_kind` rather than re-deriving the OS here: this
    // used to test `kind == "kernel" || image starts with "netbsd"`, which
    // misses a machine recorded as kind `netbsd` whose image is named anything
    // else (`bsdkrun netbsd` on a custom disk, e.g. `disk.img`). Such a guest
    // fell through to the FreeBSD arm and got a FreeBSD binary installed over
    // its agent — see the verification below for why that is so bad.
    //
    // Download tools are base-system only, and differ per BSD: FreeBSD has
    // fetch(1), NetBSD has ftp(1) (which does handle HTTPS and redirects),
    // and neither ships curl.
    let (os, dest, fetch_cmd, interp) = match guest_os_kind(&vm.kind, &vm.image) {
        "linux" => (
            host::GuestOs::Linux,
            "/sbin/bsdkrun-agent",
            "curl -fL -o",
            // Built against musl and statically linked — no interpreter to
            // match on, so the ELF magic check below is all we can assert.
            "",
        ),
        "netbsd" => (
            host::GuestOs::Netbsd,
            "/usr/local/sbin/bsdkrun-agent",
            "ftp -o",
            "/usr/libexec/ld.elf_so",
        ),
        _ => (
            host::GuestOs::Freebsd,
            "/usr/local/sbin/bsdkrun-agent",
            "fetch -o",
            "/libexec/ld-elf.so.1",
        ),
    };
    let url = agent::asset_url(os, arch);

    // Download to a temp file, verify it, chmod, then atomically move over the
    // old binary.
    //
    // Verify BEFORE the move, because the agent we are replacing is the only
    // channel we have into the guest: install a binary the guest cannot
    // execute and `exec` stops working, which also removes the means to put it
    // right. Two cheap checks catch both ways that happens — a wrong-OS build
    // (checked via the ELF interpreter) and a download that saved an error page
    // instead of a binary (checked via the ELF magic, since ftp(1) has no
    // equivalent of curl's --fail).
    let verify = format!(
        "head -c 4 \"$tmp\" | grep -q ELF || {{ \
           echo 'refusing to install: downloaded file is not an ELF binary' >&2; \
           rm -f \"$tmp\"; exit 1; }}; \
         {}",
        if interp.is_empty() {
            String::new()
        } else {
            format!(
                "grep -aq '{interp}' \"$tmp\" || {{ \
                   echo 'refusing to install: not a {} binary (no {interp})' >&2; \
                   rm -f \"$tmp\"; exit 1; }}; ",
                os.slug()
            )
        }
    );
    let script = format!(
        "set -e; tmp=/tmp/bsdkrun-agent.new; {fetch_cmd} \"$tmp\" '{url}'; \
         {verify}\
         chmod +x \"$tmp\"; mv \"$tmp\" '{dest}'; \
         echo \"updated {dest}\"; echo \"from {url}\"",
    );
    eprintln!("updating agent in {} from {url}", vm.id);
    let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script];
    let code = agent::exec(port, &argv, &[], false).map_err(|e| agent_error(&vm.kind, e))?;
    std::process::exit(code);
}

/// Run one of the agent's built-in CLI families (`tailscale`, `ssh`) inside
/// the guest, trying the known agent locations (Linux OCI guests carry it at
/// /sbin, the bundled BSD images at /usr/local/sbin). Exit code 127 means
/// "couldn't spawn" — i.e. wrong path — so try the next candidate.
pub(crate) fn run_agent_cli(id: &str, family: &str, args: &[String], env: &[String]) -> Result<()> {
    let (vm, port) = agent_target(id)?;

    let candidates: &[&str] = if vm.kind == "linux" {
        &["/sbin/bsdkrun-agent", "/usr/local/sbin/bsdkrun-agent"]
    } else {
        &["/usr/local/sbin/bsdkrun-agent", "/sbin/bsdkrun-agent"]
    };
    let mut code = 127;
    for cand in candidates {
        let mut argv: Vec<String> = vec![cand.to_string(), family.to_string()];
        argv.extend(args.iter().cloned());
        code = agent::exec(port, &argv, env, false).map_err(|e| agent_error(&vm.kind, e))?;
        if code != 127 {
            break;
        }
    }
    if code == 127 {
        anyhow::bail!(
            "bsdkrun-agent binary not found inside the guest (tried {}) — the image \
             predates the {family}-capable agent; rebuild/refetch it",
            candidates.join(", ")
        );
    }
    std::process::exit(code);
}

/// `bsdkrun tailscale <id> <action...>` — see the agent's tailscale module.
pub(crate) fn cmd_tailscale(id: &str, args: &[String]) -> Result<()> {
    // Forward the host's TS_AUTHKEY so `bsdkrun tailscale <id> setup` works
    // without pasting the key on the command line (shell history, ps, ...).
    let mut env: Vec<String> = Vec::new();
    if let Ok(k) = std::env::var("TS_AUTHKEY") {
        if !k.is_empty() {
            env.push(format!("TS_AUTHKEY={k}"));
        }
    }
    run_agent_cli(id, "tailscale", args, &env)
}

/// `bsdkrun ssh <id> <action...>` — key-based SSH via the agent's ssh module.
///
/// Host-side sugar on top of the raw agent CLI:
/// - a `--key` value that names a local file is replaced by its contents, so
///   `--key ~/.ssh/id_ed25519.pub` Just Works;
/// - when `setup`/`add-key` is called with no `--key` at all, the local
///   `~/.ssh/id_*.pub` keys are collected and forwarded via $BSDKRUN_SSH_KEYS
///   — the one-liner path: `bsdkrun ssh <id> setup`.
pub(crate) fn cmd_ssh(id: &str, args: &[String]) -> Result<()> {
    let mut out_args: Vec<String> = Vec::with_capacity(args.len());
    let mut have_key = false;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if a == "--key" {
            have_key = true;
            out_args.push(a.clone());
            if let Some(v) = it.next() {
                let p = std::path::Path::new(v);
                if p.is_file() {
                    let k = std::fs::read_to_string(p)
                        .with_context(|| format!("reading key file {v}"))?;
                    out_args.push(k.trim().to_string());
                } else {
                    out_args.push(v.clone());
                }
            }
        } else {
            out_args.push(a.clone());
        }
    }

    let mut env: Vec<String> = Vec::new();
    let action = args.first().map(String::as_str);
    if !have_key && matches!(action, Some("setup") | Some("add-key")) {
        let keys = local_public_keys();
        if !keys.is_empty() {
            println!(
                "using {} local public key(s) from ~/.ssh (pass --key to override)",
                keys.len()
            );
            env.push(format!("BSDKRUN_SSH_KEYS={}", keys.join("\n")));
        }
    }
    run_agent_cli(id, "ssh", &out_args, &env)
}

/// The user's `~/.ssh/id_*.pub` keys, one per line, comments and all.
pub(crate) fn local_public_keys() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let dir = std::path::Path::new(&home).join(".ssh");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with("id_") && name.ends_with(".pub") {
            if let Ok(s) = std::fs::read_to_string(e.path()) {
                let s = s.trim();
                if !s.is_empty() {
                    keys.push(s.to_string());
                }
            }
        }
    }
    keys.sort();
    keys
}
