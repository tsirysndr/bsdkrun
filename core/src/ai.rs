//! AI coding agents in disposable microVMs.
//!
//! `bsdkrun claude` boots a sandbox from the `claude-code` flavor, shares the
//! directory you ran it in, and drops you straight into the agent's TUI. The
//! agent can run whatever it likes in there — it cannot reach the rest of your
//! machine.
//!
//! Three pieces of state, deliberately separated, because they have different
//! lifetimes and different blast radii:
//!
//! | State | Where | Why |
//! | ----- | ----- | --- |
//! | The agent's login | a per-agent volume, mounted at `$HOME` | logging in once per agent, not once per session, is the difference between usable and not |
//! | Skills | one shared host directory, mounted into *every* sandbox | a skill installed once should be visible to every agent, which is exactly how the host is set up |
//! | Your code | nothing, unless you say so | the sandbox is the product; access is a deliberate act (`--workspace`, or the CLI sharing its cwd) |
//!
//! A "session" is a machine. The default is to reuse the agent's running
//! sandbox; `--new` boots a second one against the same home volume, so two
//! Claude sessions can run side by side without either re-authenticating.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{db, flavors};

/// Machine-name prefix for every agent sandbox: `bsdkrun-ai-claude-<n>`.
///
/// A prefix rather than a fixed name (as the Docker VM uses) because sessions
/// are plural here — but still predictable, so `bsdkrun logs bsdkrun-ai-claude-1`
/// works without looking anything up.
pub const MACHINE_PREFIX: &str = "bsdkrun-ai-";

/// The volume holding one agent's `$HOME` — its login, config and history.
pub fn home_volume(agent: &str) -> String {
    format!("bsdkrun-ai-{agent}")
}

/// Where the guest mounts that volume. Not `/root`: the agents write dotfiles
/// and caches all over it, and a dedicated path keeps the volume's contents
/// recognisable from the host.
pub const GUEST_HOME: &str = "/root";

/// The host's git identity, for the sandbox to commit with.
///
/// Read from the host's git config rather than guessed: an agent that commits
/// as `root@bsdkrun` produces history someone has to rewrite later.
pub fn git_identity() -> (Option<String>, Option<String>) {
    let get = |key: &str| {
        std::process::Command::new("git")
            .args(["config", "--get", key])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|v| !v.is_empty())
    };
    (get("user.name"), get("user.email"))
}

/// The host's `~/.ssh`, when it has one.
///
/// Mounted **read-only** into a sandbox so `git push` over SSH works with the
/// keys you already use. Read-only stops an agent rewriting your config or
/// authorized_keys; it does *not* stop it reading a private key, so this is a
/// deliberate trade — `--no-ssh` opts out, and a sandbox without it can still
/// clone over HTTPS.
pub fn host_ssh_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    let dir = PathBuf::from(home).join(".ssh");
    dir.is_dir().then_some(dir)
}

/// The host directory holding skills shared by every agent, and the guest path
/// it is mounted at.
///
/// `~/.agents/skills` is the cross-agent convention (each agent's own skills
/// directory symlinks into it), so mounting exactly that is what makes a skill
/// installed on the host — or by an agent inside a sandbox — visible to all of
/// them. Shared read-write on purpose: installing a skill from inside a sandbox
/// is the point.
pub const SKILLS_DIR: &str = ".agents/skills";

/// One coding agent: the flavor that installs it, and the command that starts
/// its TUI.
#[derive(Debug, Clone, Copy)]
pub struct Agent {
    /// Stable id — the CLI alias (`bsdkrun claude`), and the key everywhere else.
    pub id: &'static str,
    pub label: &'static str,
    /// The catalog flavor that provisions it.
    pub flavor: &'static str,
    /// argv that starts the agent's TUI inside the guest.
    pub command: &'static [&'static str],
    /// Where this agent looks for skills, relative to `$HOME`. Symlinked at the
    /// shared store so one install serves every agent.
    pub skills_path: &'static str,
    pub description: &'static str,
}

/// Every agent bsdkrun can sandbox.
///
/// Kiro is deliberately absent: it is an IDE, and the `kiro-cli` npm package is
/// a 0.0.1 placeholder — there is no terminal CLI to run. Adding one later is a
/// row here plus a catalog flavor.
pub const AGENTS: &[Agent] = &[
    Agent {
        id: "claude",
        label: "Claude Code",
        flavor: "claude-code",
        command: &["claude"],
        skills_path: ".claude/skills",
        description: "Anthropic's agentic coding CLI.",
    },
    Agent {
        id: "codex",
        label: "Codex",
        flavor: "codex",
        command: &["codex"],
        skills_path: ".codex/skills",
        description: "OpenAI's terminal coding agent.",
    },
    Agent {
        id: "gemini",
        label: "Gemini CLI",
        flavor: "gemini",
        command: &["gemini"],
        skills_path: ".gemini/skills",
        description: "Google's terminal coding agent.",
    },
    Agent {
        id: "opencode",
        label: "OpenCode",
        flavor: "opencode",
        command: &["opencode"],
        skills_path: ".config/opencode/skills",
        description: "Open-source terminal coding agent.",
    },
    Agent {
        id: "crush",
        label: "Crush",
        flavor: "crush",
        command: &["crush"],
        skills_path: ".config/crush/skills",
        description: "Charm's terminal coding agent.",
    },
    Agent {
        id: "copilot",
        label: "GitHub Copilot",
        flavor: "copilot",
        command: &["copilot"],
        skills_path: ".copilot/skills",
        description: "GitHub's terminal coding agent.",
    },
    Agent {
        id: "kilo",
        label: "Kilo Code",
        flavor: "kilo",
        command: &["kilocode"],
        skills_path: ".kilocode/skills",
        description: "Kilo Code's terminal agent.",
    },
    Agent {
        id: "qwen",
        label: "Qwen Code",
        flavor: "qwen",
        command: &["qwen"],
        skills_path: ".qwen/skills",
        description: "Alibaba's terminal coding agent.",
    },
];

/// The default agent when none is named.
pub const DEFAULT_AGENT: &str = "claude";

/// Look one up by id. Also accepts the flavor name (`claude-code`), since that
/// is what `bsdkrun flavors` prints and someone will type it.
pub fn find(id: &str) -> Option<&'static Agent> {
    let id = id.trim();
    AGENTS
        .iter()
        .find(|a| a.id == id)
        .or_else(|| AGENTS.iter().find(|a| a.flavor == id))
}

pub fn require(id: &str) -> Result<&'static Agent> {
    find(id).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown agent {id:?} — try one of: {}",
            AGENTS.iter().map(|a| a.id).collect::<Vec<_>>().join(", ")
        )
    })
}

/// An agent, as the UIs and SDKs list them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    pub flavor: String,
    pub description: String,
    /// The agent's flavor has been provisioned, so a sandbox starts instantly.
    /// False means the first launch builds it (minutes, with streamed output).
    pub installed: bool,
    /// How many sandboxes of this agent are running right now.
    pub running: i64,
}

/// One running (or stopped) sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub agent: String,
    pub running: bool,
    /// The host directory shared into it, if any.
    pub workspace: Option<String>,
    /// A user-given name for this session, when one was set.
    pub label: Option<String>,
    /// The project this session belongs to — several sessions on one codebase
    /// group under it. Defaults to the shared folder's name.
    pub project: Option<String>,
    pub created_at: String,
}

/// Every agent, with whether its flavor is built and how many sandboxes are up.
pub fn agents() -> Result<Vec<AgentInfo>> {
    let sessions = sessions()?;
    Ok(AGENTS
        .iter()
        .map(|a| AgentInfo {
            id: a.id.to_string(),
            label: a.label.to_string(),
            flavor: a.flavor.to_string(),
            description: a.description.to_string(),
            installed: flavor_built(a),
            running: sessions
                .iter()
                .filter(|s| s.agent == a.id && s.running)
                .count() as i64,
        })
        .collect())
}

/// Whether an agent's flavor has already been provisioned into the build cache.
///
/// This is what decides between "the sandbox boots in a second" and "the first
/// launch installs a toolchain", so both the CLI and the UIs check it up front
/// and stream the build when it is false.
pub fn flavor_built(agent: &Agent) -> bool {
    let Some(spec) = crate::commands::flavor::resolve_linux_flavor(agent.flavor) else {
        return false;
    };
    let key = crate::commands::flavor::flavor_build_key(&spec.image, &spec.nix, &spec.provision);
    let vol = crate::commands::flavor::flavor_build_volume(&key);
    crate::commands::volume_dir(&vol)
        .map(|d| d.join(".provisioned").exists() && d.join("rootfs").exists())
        .unwrap_or(false)
}

/// Sandboxes, newest first. A sandbox is just a machine whose name carries the
/// prefix, so `ps`, `logs`, `stop` and the rest work on it unchanged.
pub fn sessions() -> Result<Vec<Session>> {
    let db = db::Db::open()?;
    let mut out = Vec::new();
    for m in db.list_machines()? {
        let Some(name) = m.name.as_deref() else {
            continue;
        };
        if !name.starts_with(MACHINE_PREFIX) {
            continue;
        }
        let vdir = PathBuf::from(&m.state_dir);
        // The agent comes from the state dir, not from the name: a labelled
        // sandbox is `bsdkrun-ai-claude-refactor-auth`, and splitting that on a
        // dash cannot tell the agent from the label.
        let Some(agent) = agent_of(&vdir) else {
            continue;
        };
        out.push(Session {
            running: m.status == "running" && m.pid.map(db::pid_alive).unwrap_or(false),
            workspace: workspace_of(&vdir),
            label: label_of(&vdir),
            project: project_of(&vdir),
            id: m.id,
            name: name.to_string(),
            agent,
            created_at: m.created_at,
        });
    }
    Ok(out)
}

/// One agent's sandboxes.
pub fn sessions_for(agent: &str) -> Result<Vec<Session>> {
    Ok(sessions()?
        .into_iter()
        .filter(|s| s.agent == agent)
        .collect())
}

/// The newest running sandbox for an agent — what a bare `bsdkrun claude`
/// attaches to.
pub fn running_session(agent: &str) -> Result<Option<Session>> {
    Ok(sessions_for(agent)?.into_iter().find(|s| s.running))
}

/// The machine name for a new sandbox: `bsdkrun-ai-<agent>-<label or n>`.
///
/// A label goes in the name too, so `bsdkrun logs bsdkrun-ai-claude-refactor`
/// works — the agent is still read from the state dir, which is what keeps a
/// dashed label unambiguous.
pub fn next_name(agent: &str, label: Option<&str>) -> Result<String> {
    let taken: Vec<String> = sessions()?.into_iter().map(|s| s.name).collect();
    if let Some(label) = label.map(slug).filter(|l| !l.is_empty()) {
        let candidate = format!("{MACHINE_PREFIX}{agent}-{label}");
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
        // A repeated label still has to produce a unique machine name.
        for n in 2.. {
            let candidate = format!("{MACHINE_PREFIX}{agent}-{label}-{n}");
            if !taken.contains(&candidate) {
                return Ok(candidate);
            }
        }
    }
    for n in 1.. {
        let candidate = format!("{MACHINE_PREFIX}{agent}-{n}");
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("the loop returns on the first free name")
}

/// A machine-name-safe form of a user's label.
fn slug(label: &str) -> String {
    label
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Files recording what a sandbox is, in its state dir. The machine row
/// carries none of it, and the *name* is not a safe place to encode the agent:
/// a user-given label like `refactor-auth` contains a dash, so parsing one out
/// of `bsdkrun-ai-claude-refactor-auth` would be ambiguous.
const WORKSPACE_FILE: &str = "ai-workspace";
const AGENT_FILE: &str = "ai-agent";
const LABEL_FILE: &str = "ai-label";
const PROJECT_FILE: &str = "ai-project";

/// Record which agent a sandbox runs.
pub fn record_agent(vdir: &Path, agent: &str) {
    let _ = std::fs::write(vdir.join(AGENT_FILE), agent.as_bytes());
}

pub fn agent_of(vdir: &Path) -> Option<String> {
    read_trimmed(vdir.join(AGENT_FILE))
}

/// Record a user-given name for a sandbox ("refactor-auth"), shown in `ai ls`
/// and in the panel's session switcher.
pub fn record_label(vdir: &Path, label: Option<&str>) {
    let f = vdir.join(LABEL_FILE);
    match label.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => {
            let _ = std::fs::write(&f, l.as_bytes());
        }
        None => {
            let _ = std::fs::remove_file(&f);
        }
    }
}

pub fn label_of(vdir: &Path) -> Option<String> {
    read_trimmed(vdir.join(LABEL_FILE))
}

/// Record which project a sandbox belongs to, so several sessions on one
/// codebase group together in listings and in the panel's switcher.
pub fn record_project(vdir: &Path, project: Option<&str>) {
    let f = vdir.join(PROJECT_FILE);
    match project.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            let _ = std::fs::write(&f, p.as_bytes());
        }
        None => {
            let _ = std::fs::remove_file(&f);
        }
    }
}

pub fn project_of(vdir: &Path) -> Option<String> {
    read_trimmed(vdir.join(PROJECT_FILE))
}

/// The project a session belongs to: what was asked for, else the shared
/// folder's name, else the cloned repository's.
///
/// Defaulting this way is what makes grouping useful without anyone having to
/// think about it — two sessions on `~/code/api`, or two clones of the same
/// repo, are two views of the same work whatever they are called.
pub fn resolve_project(
    explicit: Option<&str>,
    workspace: Option<&Path>,
    repo: Option<&str>,
) -> Option<String> {
    explicit
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| {
            workspace
                .and_then(|w| w.file_name())
                .map(|n| n.to_string_lossy().into_owned())
        })
        .or_else(|| repo.map(repo_project_name).filter(|n| !n.is_empty()))
}

/// `https://github.com/owner/repo.git` → `repo`.
fn repo_project_name(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".git")
        .to_string()
}

fn read_trimmed(path: PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn record_workspace(vdir: &Path, workspace: Option<&Path>) {
    let f = vdir.join(WORKSPACE_FILE);
    match workspace {
        Some(w) => {
            let _ = std::fs::write(&f, w.to_string_lossy().as_bytes());
        }
        None => {
            let _ = std::fs::remove_file(&f);
        }
    }
}

pub fn workspace_of(vdir: &Path) -> Option<String> {
    std::fs::read_to_string(vdir.join(WORKSPACE_FILE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The host's shared skills directory, created if missing.
///
/// Created rather than skipped when absent: a sandbox that mounts it can then
/// install skills into it, and they show up on the host — which is the whole
/// point of one shared store.
pub fn host_skills_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    let dir = PathBuf::from(home).join(SKILLS_DIR);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The host directory backing an agent's `$HOME`, created if missing.
///
/// A plain bsdkrun volume directory, mounted into the guest rather than used as
/// its rootfs: the rootfs stays per-sandbox and disposable (which is what makes
/// a second session cheap), while the login inside `$HOME` is shared by every
/// sandbox of that agent.
pub fn home_dir(agent: &str) -> Result<PathBuf> {
    let dir = crate::db::volumes_dir()?.join(home_volume(agent));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}'s home volume {}", agent, dir.display()))?;
    Ok(dir)
}

/// The `--mount` specs a sandbox boots with: the agent's persistent home, the
/// shared skills store, and the workspace when one was asked for.
///
/// The workspace is shared **at the same path** it has on the host, so a path
/// the agent reads from your terminal — or from a file it is editing — means
/// the same thing inside the guest.
///
/// Order matters: the home mount has to come before the skills mount nested
/// inside it, or virtio-fs mounts the parent over the child and the shared
/// store disappears.
pub fn mounts(agent: &Agent, workspace: Option<&Path>, ssh: bool) -> Result<Vec<String>> {
    let mut specs = vec![format!("{}:{GUEST_HOME}", home_dir(agent.id)?.display())];
    if let Some(skills) = host_skills_dir() {
        specs.push(format!("{}:{GUEST_HOME}/{SKILLS_DIR}", skills.display()));
    }
    if ssh {
        if let Some(ssh_dir) = host_ssh_dir() {
            // Read-only: see `host_ssh_dir` for what that does and does not buy.
            specs.push(format!("{}:{GUEST_HOME}/.ssh:ro", ssh_dir.display()));
        }
    }
    if let Some(w) = workspace {
        // Sharing $HOME itself would defeat the sandbox *and* collide with the
        // home mount above; the agent's own home is already persistent.
        let is_home = std::env::var_os("HOME")
            .map(|h| Path::new(&h) == w)
            .unwrap_or(false);
        if is_home {
            anyhow::bail!(
                "refusing to share your whole home directory with an agent — \
                 name a project directory instead"
            );
        }
        specs.push(format!("{}:{}", w.display(), w.display()));
    }
    Ok(specs)
}

/// The one-shot setup a sandbox runs before the agent starts: point this
/// agent's skills directory at the shared store.
///
/// A symlink rather than a second mount, because the agents disagree about
/// where skills live (`~/.claude/skills`, `~/.codex/skills`, …) and only the
/// shared directory should ever hold the files.
pub fn skills_link_script(agent: &Agent) -> String {
    format!(
        "mkdir -p {home}/{skills} \"$(dirname {home}/{path})\" 2>/dev/null; \
         [ -L {home}/{path} ] || {{ rm -rf {home}/{path} 2>/dev/null; \
         ln -s {home}/{skills} {home}/{path} 2>/dev/null; }}",
        home = GUEST_HOME,
        skills = SKILLS_DIR,
        path = agent.skills_path,
    )
}

/// Start the sandbox's own dockerd, if it has one and it is not already up.
///
/// The guest's init runs the agent, not a service manager, so nothing else
/// would ever start it. Backgrounded and silenced: an agent that wants Docker
/// will find it a second later, and one that does not should never see its log.
///
/// This is the sandbox's *own* daemon. Sharing the host's docker socket would
/// be far easier and is exactly what must not happen — control of the host's
/// Docker is control of the host.
pub fn docker_start_script() -> String {
    "if command -v dockerd >/dev/null 2>&1 && ! docker info >/dev/null 2>&1; then \
     (dockerd >/var/log/dockerd.log 2>&1 &) ; fi"
        .to_string()
}

/// Write the host's git identity into the sandbox, idempotently.
///
/// In the wrapper rather than at provisioning time because the home volume is
/// shared across an agent's sessions and the host's identity can change —
/// re-stating it each start costs nothing and cannot drift.
pub fn git_identity_script() -> String {
    let (name, email) = git_identity();
    let mut parts = Vec::new();
    if let Some(name) = name {
        parts.push(format!(
            "git config --global user.name {} 2>/dev/null || true",
            shell_quote(&name)
        ));
    }
    if let Some(email) = email {
        parts.push(format!(
            "git config --global user.email {} 2>/dev/null || true",
            shell_quote(&email)
        ));
    }
    if parts.is_empty() {
        return "true".to_string();
    }
    // Trust the shared workspace: git refuses to operate in a directory owned
    // by another uid, which is exactly what a host mount looks like in here.
    parts.push("git config --global --add safe.directory '*' 2>/dev/null || true".to_string());
    parts.join("; ")
}

/// The argv that opens an agent's TUI in its sandbox.
///
/// Wrapped in a shell so the session can `cd` into the shared workspace and
/// link the skills store first. `exec` hands the terminal to the agent itself,
/// so quitting the agent ends the session rather than dropping to a shell the
/// user did not ask for.
pub fn tui_argv(agent: &Agent, workspace: Option<&str>) -> Vec<String> {
    // Prefer the shared folder; otherwise fall back to whatever `--repo`
    // cloned, which the boot path records in /etc/bsdkrun-cwd. Without the
    // fallback an agent given a repository would start in `$HOME` and have to
    // be told where its own checkout is.
    let cd = match workspace {
        Some(w) => format!("cd {} 2>/dev/null || true; ", shell_quote(w)),
        None => "cd \"$(cat /etc/bsdkrun-cwd 2>/dev/null)\" 2>/dev/null || true; ".to_string(),
    };
    let cmd = agent
        .command
        .iter()
        .map(|c| shell_quote(c))
        .collect::<Vec<_>>()
        .join(" ");
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "export HOME={home}; export TERM=${{TERM:-xterm-256color}}; \
             export PATH=\"$HOME/.local/bin:/usr/local/bin:$PATH\"; \
             [ -d /nix/var/nix/profiles/default/bin ] && \
             export PATH=\"$HOME/.nix-profile/bin:/nix/var/nix/profiles/default/bin:$PATH\"; \
             {skills}; {git}; {docker}; {cd}\
             if command -v {probe} >/dev/null 2>&1; then exec {cmd}; else \
             echo \"[bsdkrun] {label} is not installed in this sandbox\"; exec bash; fi",
            home = GUEST_HOME,
            skills = skills_link_script(agent),
            git = git_identity_script(),
            docker = docker_start_script(),
            probe = shell_quote(agent.command[0]),
            label = agent.label,
        ),
    ]
}

/// Single-quote for `sh -c`, the way the rest of the guest argv builders do.
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '=' | ':'))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Resolve `--workspace`: an explicit path, the current directory for the CLI,
/// or nothing at all.
///
/// **The path is resolved on the engine's host**, which is this process — so
/// when a client drives a *remote* `bsdkrund`, this runs on the VPS and the
/// path names a directory there. That is the same rule a Docker context over
/// SSH follows, and the error below says so, because the alternative failure
/// (an empty directory appearing inside the sandbox) is silent and baffling.
pub fn resolve_workspace(explicit: Option<&str>, use_cwd: bool) -> Result<Option<PathBuf>> {
    let raw = match (explicit, use_cwd) {
        (Some(p), _) => p.to_string(),
        (None, true) => std::env::current_dir()
            .context("resolving the current directory to share")?
            .to_string_lossy()
            .into_owned(),
        (None, false) => return Ok(None),
    };
    let path = std::fs::canonicalize(&raw).map_err(|e| {
        anyhow::anyhow!(
            "cannot share {raw:?}: {e}\n\nThe path is resolved on the machine running the \
             engine. If you are driving a remote bsdkrund, name a directory on *that* host \
             — or get your code into the sandbox with git, or `bsdkrun cp -r`."
        )
    })?;
    if !path.is_dir() {
        anyhow::bail!("--workspace {} is not a directory", path.display());
    }
    Ok(Some(path))
}

/// Whether the agent's flavor is a catalog flavor at all — a sanity check that
/// keeps a typo in [`AGENTS`] from surfacing as a confusing boot failure.
pub fn flavor_exists(agent: &Agent) -> bool {
    flavors::find(agent.flavor).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_has_a_catalog_flavor() {
        for a in AGENTS {
            assert!(flavor_exists(a), "{} has no catalog flavor", a.id);
        }
    }

    #[test]
    fn agents_are_findable_by_id_and_by_flavor_name() {
        assert_eq!(find("claude").unwrap().id, "claude");
        assert_eq!(find("claude-code").unwrap().id, "claude");
        assert!(find("kiro").is_none());
    }

    #[test]
    fn project_falls_back_to_the_repo_name() {
        assert_eq!(
            resolve_project(None, None, Some("https://github.com/owner/api.git")).as_deref(),
            Some("api")
        );
        assert_eq!(
            resolve_project(None, None, Some("git@github.com:owner/api")).as_deref(),
            Some("api")
        );
        // An explicit project and a shared folder both win over the repo.
        assert_eq!(
            resolve_project(Some("mine"), None, Some("https://x/y.git")).as_deref(),
            Some("mine")
        );
        assert_eq!(
            resolve_project(None, Some(Path::new("/code/web")), Some("https://x/y.git")).as_deref(),
            Some("web")
        );
    }

    #[test]
    fn tui_argv_cds_into_the_workspace_and_links_skills() {
        let claude = find("claude").unwrap();
        let argv = tui_argv(claude, Some("/Users/me/my app"));
        let script = &argv[2];
        // The workspace is quoted (it has a space) and the agent is exec'd.
        assert!(script.contains("cd '/Users/me/my app'"), "{script}");
        assert!(script.contains("exec claude"), "{script}");
        assert!(script.contains(".claude/skills"), "{script}");
        assert!(script.contains(".agents/skills"), "{script}");
        // The sandbox starts its *own* dockerd — never the host's socket.
        assert!(script.contains("dockerd"), "{script}");
        assert!(!script.contains("/var/run/docker.sock"), "{script}");
    }

    #[test]
    fn mounts_share_home_skills_and_the_workspace_in_that_order() {
        let claude = find("claude").unwrap();
        let m = mounts(claude, Some(Path::new("/tmp/project")), false).unwrap();
        // The home mount must precede the skills mount nested inside it.
        let home = m.iter().position(|s| s.ends_with(GUEST_HOME)).unwrap();
        let skills = m.iter().position(|s| s.contains(SKILLS_DIR)).unwrap();
        assert!(home < skills, "{m:?}");
        assert!(m.iter().any(|s| s == "/tmp/project:/tmp/project"), "{m:?}");
    }

    #[test]
    fn sharing_the_whole_home_directory_is_refused() {
        let claude = find("claude").unwrap();
        let home = std::env::var("HOME").unwrap();
        assert!(mounts(claude, Some(Path::new(&home)), false).is_err());
    }

    #[test]
    fn labels_are_slugged_into_machine_names() {
        assert_eq!(slug("Refactor Auth!"), "refactor-auth");
        assert_eq!(slug("  spaced  "), "spaced");
        assert_eq!(slug("a/b"), "a-b");
        // A label that slugs to nothing falls back to the numbered form.
        assert_eq!(slug("!!!"), "");
    }
}
