//! Flavors — preconfigured dev environments and snapshots.
//!
//! A *flavor* is a named, bootable base state. Three sources:
//!   * **Catalog** — a built-in, curated environment (below): a base image plus
//!     default ports/env and an optional provisioning script (and, on OCI bases,
//!     optional Nix packages installed via the Determinate Systems installer).
//!   * **User** — the same shape, loaded at runtime from a static `flavors.toml`
//!     (see [`user_flavors`]). Lets people define their own stacks declaratively.
//!   * **Snapshot** — a copy-on-write clone of a machine's current rootfs/disk,
//!     captured with `bsdkrun commit` and recorded in the state DB.
//!
//! Booting a flavor clones its rootfs/disk into a fresh machine (fast, CoW).
//!
//! A flavor's *build method* (for the UI) is one of:
//!   * `docker`  — a plain OCI image, no extra provisioning;
//!   * `nix`     — Nix packages installed via the Determinate Systems installer;
//!   * `system`  — direct system packages / shell provisioning in the guest.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ---- provisioning DSL macros ----------------------------------------------
//
// These make a flavor's `provision` list read like a recipe instead of a wall
// of shell. Everything expands to `&'static str` / `&'static [&'static str]` at
// compile time (via `concat!`), so there's zero runtime cost.
//
//     provision![
//         apt!["git", "ripgrep"],                    // apt-get install …
//         npm!["@anthropic-ai/claude-code"],         // npm install -g …
//         "curl -fsSL https://example.com | sh",     // raw command, verbatim
//     ]

/// Join string literals with single spaces at compile time.
macro_rules! join_space {
    ($a:literal) => { $a };
    ($a:literal, $($rest:literal),+ $(,)?) => {
        concat!($a, " ", join_space!($($rest),+))
    };
}

/// `apt!["git", "curl"]` → update the index then install the packages.
macro_rules! apt {
    ($($pkg:literal),+ $(,)?) => {
        concat!("apt-get update && apt-get install -y ", join_space!($($pkg),+))
    };
}

/// `npm!["typescript", "@anthropic-ai/claude-code"]` → global npm install.
macro_rules! npm {
    ($($pkg:literal),+ $(,)?) => {
        concat!("npm install -g ", join_space!($($pkg),+))
    };
}

/// Docker inside a guest: the daemon, the CLI, buildx and compose.
///
/// Installed *in* the sandbox rather than by sharing the host's docker socket,
/// which would hand an AI agent full control of the host's Docker — and
/// through it, the host. Containers an agent starts live and die with its
/// sandbox.
///
/// The official convenience script, because Debian's own `docker.io` lags and
/// the apt-repo dance is four steps that each fail differently. `|| true`
/// keeps the sandbox usable on a release the script does not cover — the
/// agent itself is still installed, just without Docker.
macro_rules! docker_engine {
    () => {
        "command -v dockerd >/dev/null 2>&1 || \
         (curl -fsSL https://get.docker.com | sh) || true"
    };
}

/// `pip!["uv"]` → cache-less pip install.
macro_rules! pip {
    ($($pkg:literal),+ $(,)?) => {
        concat!("pip install --no-cache-dir ", join_space!($($pkg),+))
    };
}

/// `provision![cmd, cmd, …]` → an ordered command list (`&[&str]`). Each entry
/// is one shell command line, run in order in the guest after boot.
macro_rules! provision {
    () => { &[] as &[&str] };
    ($($cmd:expr),+ $(,)?) => { &[$($cmd),+] as &[&str] };
}

/// The base a catalog flavor builds on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// An OCI image reference, e.g. `node:22`.
    Oci(&'static str),
    /// bsdkrun's bundled FreeBSD guest.
    Freebsd,
    /// bsdkrun's bundled NetBSD guest.
    Netbsd,
}

/// A built-in, curated environment.
#[derive(Clone, Debug)]
pub struct CatalogFlavor {
    pub name: &'static str,
    /// Grouping for the UI: `language` / `runtime` / `service` / `web` / `ai` / `os`.
    pub category: &'static str,
    pub description: &'static str,
    pub base: Base,
    /// Default host↔guest port forwards (`HOST:GUEST`).
    pub ports: &'static [&'static str],
    /// Default environment (`K=V`).
    pub env: &'static [&'static str],
    /// Nix packages to install (via the Determinate Systems installer on an OCI
    /// base; ignored on a BSD base, which has no Nix).
    pub nix: &'static [&'static str],
    /// Shell commands run in the guest after boot (via the agent), in order.
    /// Each entry is one command line. Empty = no provisioning.
    pub provision: &'static [&'static str],
}

impl CatalogFlavor {
    /// The guest kind the launcher should use for this flavor's base.
    pub fn kind(&self) -> &'static str {
        match self.base {
            Base::Oci(_) => "linux",
            Base::Freebsd => "freebsd",
            Base::Netbsd => "netbsd",
        }
    }

    /// The image/base reference to boot (an OCI ref, or the BSD slug).
    pub fn image(&self) -> &'static str {
        match self.base {
            Base::Oci(r) => r,
            Base::Freebsd => "freebsd",
            Base::Netbsd => "netbsd",
        }
    }

    /// How this flavor is built, for the UI: `nix`, `system`, or `docker`.
    pub fn method(&self) -> &'static str {
        method_for(
            matches!(self.base, Base::Oci(_)),
            self.nix.is_empty(),
            self.provision.is_empty(),
        )
    }
}

/// Shared build-method rule for catalog and user flavors.
fn method_for(is_oci: bool, nix_empty: bool, provision_empty: bool) -> &'static str {
    if !nix_empty {
        "nix"
    } else if !provision_empty {
        "system"
    } else if is_oci {
        "docker"
    } else {
        "system"
    }
}

/// The built-in catalog. Kept small and dependency-light: each entry maps to a
/// ready base plus sensible defaults; heavier stacks add provisioning commands.
pub fn catalog() -> &'static [CatalogFlavor] {
    const NONE: &[&str] = &[];
    &[
        // ---- languages / runtimes --------------------------------------
        CatalogFlavor {
            name: "node",
            category: "language",
            description: "Node.js 22 runtime (npm, corepack).",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "python",
            category: "language",
            description: "Python 3.12 with the uv package manager.",
            base: Base::Oci("python:3.12-slim"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![concat!(
                pip!["uv"],
                " 2>/dev/null || (",
                apt!["curl"],
                " && curl -LsSf https://astral.sh/uv/install.sh | sh)"
            ),],
        },
        CatalogFlavor {
            name: "php",
            category: "language",
            description: "PHP 8.3 CLI with Composer.",
            base: Base::Oci("php:8.3-cli"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                "command -v composer >/dev/null || \
                 (curl -sS https://getcomposer.org/installer | php -- \
                  --install-dir=/usr/local/bin --filename=composer)",
            ],
        },
        CatalogFlavor {
            name: "laravel",
            category: "language",
            description: "PHP 8.3 + Composer + Node for Laravel apps.",
            base: Base::Oci("php:8.3-cli"),
            ports: &["8000:8000"],
            env: NONE,
            nix: NONE,
            provision: provision![
                apt!["git", "unzip", "curl", "libzip-dev", "nodejs", "npm"],
                "docker-php-ext-install zip pdo pdo_mysql >/dev/null 2>&1 || true",
                "curl -sS https://getcomposer.org/installer | php -- \
                 --install-dir=/usr/local/bin --filename=composer",
            ],
        },
        CatalogFlavor {
            name: "symfony",
            category: "language",
            description: "PHP 8.3 + Composer + Symfony CLI.",
            base: Base::Oci("php:8.3-cli"),
            ports: &["8000:8000"],
            env: NONE,
            nix: NONE,
            provision: provision![
                apt!["git", "unzip", "curl"],
                "curl -sS https://getcomposer.org/installer | php -- \
                 --install-dir=/usr/local/bin --filename=composer",
                "curl -1sLf https://get.symfony.com/cli/installer | bash || true",
            ],
        },
        CatalogFlavor {
            name: "elixir",
            category: "language",
            description: "Elixir/Erlang (mix, hex).",
            base: Base::Oci("elixir:1.17"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                "mix local.hex --force >/dev/null 2>&1 || true",
                "mix local.rebar --force >/dev/null 2>&1 || true",
            ],
        },
        CatalogFlavor {
            name: "phoenix",
            category: "language",
            description: "Elixir + Phoenix web framework (mix phx.new).",
            base: Base::Oci("elixir:1.17"),
            ports: &["4000:4000"],
            env: NONE,
            nix: NONE,
            provision: provision![
                "mix local.hex --force >/dev/null 2>&1 || true",
                "mix local.rebar --force >/dev/null 2>&1 || true",
                "mix archive.install hex phx_new --force >/dev/null 2>&1 || true",
            ],
        },
        CatalogFlavor {
            name: "gleam",
            category: "language",
            description: "Gleam on the BEAM (installed via Nix).",
            base: Base::Oci("debian:12-slim"),
            ports: NONE,
            env: NONE,
            nix: &["gleam", "erlang", "rebar3"],
            provision: NONE,
        },
        CatalogFlavor {
            name: "clojure",
            category: "language",
            description: "Clojure with the official CLI tools.",
            base: Base::Oci("clojure:temurin-21-tools-deps"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        // ---- AI coding agents ------------------------------------------
        // Each is a Node base with the agent CLI installed globally. Auth is
        // interactive on first run (API key / login), so no secrets are baked in.
        CatalogFlavor {
            name: "claude-code",
            category: "ai",
            description: "Claude Code — Anthropic's agentic coding CLI.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                apt!["git", "ripgrep"],
                npm!["@anthropic-ai/claude-code"],
                docker_engine!(),
            ],
        },
        CatalogFlavor {
            name: "codex",
            category: "ai",
            description: "OpenAI Codex CLI coding agent.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![apt!["git"], npm!["@openai/codex"], docker_engine!(),],
        },
        CatalogFlavor {
            name: "gemini",
            category: "ai",
            description: "Gemini CLI — Google's terminal coding agent.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                apt!["git", "ripgrep"],
                npm!["@google/gemini-cli"],
                docker_engine!(),
            ],
        },
        CatalogFlavor {
            name: "kilo",
            category: "ai",
            description: "Kilo Code — terminal AI coding agent.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![apt!["git"], npm!["@kilocode/cli"], docker_engine!(),],
        },
        CatalogFlavor {
            name: "qwen",
            category: "ai",
            description: "Qwen Code — Alibaba's terminal coding agent.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![apt!["git"], npm!["@qwen-code/qwen-code"], docker_engine!(),],
        },
        CatalogFlavor {
            name: "opencode",
            category: "ai",
            description: "OpenCode — open-source terminal AI coding agent.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                apt!["git", "curl"],
                concat!(
                    npm!["opencode-ai"],
                    " || curl -fsSL https://opencode.ai/install | bash"
                ),
                docker_engine!(),
            ],
        },
        CatalogFlavor {
            name: "crush",
            category: "ai",
            description: "Crush — Charm's glamourous terminal AI coding agent.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                apt!["git", "curl"],
                npm!["@charmland/crush"],
                docker_engine!(),
            ],
        },
        CatalogFlavor {
            name: "copilot",
            category: "ai",
            description: "GitHub Copilot CLI coding agent.",
            base: Base::Oci("node:22"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![apt!["git"], npm!["@github/copilot"], docker_engine!(),],
        },
        // These two are *assistants*, not TUI coding agents: they run as
        // services and talk over messaging channels, so they are flavors to
        // boot rather than entries in `bsdkrun ai`'s dropdown.
        CatalogFlavor {
            name: "openclaw",
            category: "ai",
            description: "OpenClaw — personal AI assistant / multi-channel gateway.",
            base: Base::Oci("node:24"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![apt!["git", "ripgrep"], npm!["openclaw"],],
        },
        CatalogFlavor {
            name: "nanoclaw",
            category: "ai",
            // Docker-in-Docker on purpose: nanoclaw sandboxes each of its
            // agents in a container, so the guest needs a working dockerd —
            // which is exactly what the `docker` flavor's base provides.
            description: "nanoclaw — containerised AI assistant (needs Docker; run ./nanoclaw.sh).",
            base: Base::Oci("docker:dind"),
            ports: NONE,
            // dind serves TLS on 2376 unless this is empty; nanoclaw's own
            // `docker` client talks to the local socket either way, but an
            // empty cert dir keeps the daemon from generating certs it will
            // never use.
            env: &["DOCKER_TLS_CERTDIR="],
            nix: NONE,
            // The upstream installer (`nanoclaw.sh`) is interactive by design —
            // it pairs a messaging channel and prompts for credentials — so
            // provisioning stops at the prerequisites and the checkout, and
            // leaves the last step to the user in the guest's shell.
            provision: provision![
                "apk add --no-cache git nodejs npm curl bash >/dev/null 2>&1 || \
                 (apt-get update && apt-get install -y git nodejs npm curl bash)",
                "npm install -g pnpm",
                "git clone --depth 1 https://github.com/nanocoai/nanoclaw.git /opt/nanoclaw",
                "echo 'cd /opt/nanoclaw && ./nanoclaw.sh' > /etc/bsdkrun-motd",
            ],
        },
        // ---- services --------------------------------------------------
        CatalogFlavor {
            name: "postgres",
            category: "service",
            description: "PostgreSQL 16 (user postgres / password secret).",
            base: Base::Oci("postgres:16"),
            ports: &["5432:5432"],
            env: &["POSTGRES_PASSWORD=secret", "POSTGRES_USER=postgres"],
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "mariadb",
            category: "service",
            description: "MariaDB 11 (root password secret).",
            base: Base::Oci("mariadb:11"),
            ports: &["3306:3306"],
            env: &["MARIADB_ROOT_PASSWORD=secret"],
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "mysql",
            category: "service",
            description: "MySQL 8 (root password secret).",
            base: Base::Oci("mysql:8"),
            ports: &["3306:3306"],
            env: &["MYSQL_ROOT_PASSWORD=secret"],
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "redis",
            category: "service",
            description: "Redis 7 in-memory data store.",
            base: Base::Oci("redis:7"),
            ports: &["6379:6379"],
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        // ---- web servers -----------------------------------------------
        CatalogFlavor {
            name: "nginx",
            category: "web",
            description: "nginx web server.",
            base: Base::Oci("nginx:stable-alpine"),
            ports: &["8080:80"],
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "apache",
            category: "web",
            description: "Apache httpd web server.",
            base: Base::Oci("httpd:2.4-alpine"),
            ports: &["8080:80"],
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "frankenphp",
            category: "web",
            // The official image is a Caddy build with PHP embedded, so there
            // is nothing to provision: it serves /app/public out of the box.
            description: "FrankenPHP — PHP app server built on Caddy (HTTP/2, worker mode).",
            base: Base::Oci("dunglas/frankenphp"),
            ports: &["8080:80", "8443:443"],
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "caddy",
            category: "web",
            description: "Caddy web server (automatic HTTPS).",
            base: Base::Oci("caddy:2-alpine"),
            ports: &["8080:80", "8443:443"],
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        // ---- os / runtime ----------------------------------------------
        CatalogFlavor {
            name: "nix",
            category: "runtime",
            description: "Nix package manager (nixos/nix) — build anything.",
            base: Base::Oci("nixos/nix"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "docker",
            category: "runtime",
            description: "Docker-in-Docker daemon.",
            base: Base::Oci("docker:dind"),
            ports: NONE,
            env: &["DOCKER_TLS_CERTDIR="],
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "freebsd",
            category: "os",
            description: "FreeBSD userland (bundled image).",
            base: Base::Freebsd,
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "netbsd",
            category: "os",
            description: "NetBSD (current) userland (bundled image).",
            base: Base::Netbsd,
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
    ]
}

/// Look up a catalog flavor by exact name.
pub fn find(name: &str) -> Option<&'static CatalogFlavor> {
    catalog().iter().find(|f| f.name == name)
}

// ---- user-defined flavors (static TOML) -----------------------------------

/// A user-defined flavor, loaded from a `flavors.toml`. Same shape as a catalog
/// entry but owned (runtime data), with the base written as a plain string:
/// an OCI ref (`node:22`), or `freebsd` / `netbsd` for a BSD base.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UserFlavor {
    pub name: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// OCI image ref, or `freebsd` / `netbsd`.
    pub base: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nix: Vec<String>,
    /// Shell commands run in the guest after boot, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provision: Vec<String>,
}

fn default_category() -> String {
    "custom".to_string()
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FlavorsFile {
    #[serde(default, alias = "flavors", skip_serializing_if = "Vec::is_empty")]
    flavor: Vec<UserFlavor>,
}

impl UserFlavor {
    /// The guest kind the launcher should use (`linux` / `freebsd` / `netbsd`).
    pub fn kind(&self) -> &'static str {
        match self.base.as_str() {
            "freebsd" => "freebsd",
            "netbsd" => "netbsd",
            _ => "linux",
        }
    }

    /// How this flavor is built, for the UI: `nix`, `system`, or `docker`.
    pub fn method(&self) -> &'static str {
        method_for(
            self.kind() == "linux",
            self.nix.is_empty(),
            self.provision.is_empty(),
        )
    }
}

/// Search paths for a user `flavors.toml`, most specific first:
///   1. `$BSDKRUN_FLAVORS_FILE` (explicit override),
///   2. `./bsdkrun.flavors.toml` (project-local),
///   3. `$XDG_CONFIG_HOME/bsdkrun/flavors.toml` (else `~/.config/bsdkrun/…`).
fn user_flavor_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(explicit) = std::env::var("BSDKRUN_FLAVORS_FILE") {
        if !explicit.is_empty() {
            paths.push(PathBuf::from(explicit));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join("bsdkrun.flavors.toml"));
    }
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        });
    if let Some(cfg) = cfg {
        paths.push(cfg.join("bsdkrun").join("flavors.toml"));
    }
    paths
}

/// Load user-defined flavors from the first `flavors.toml` found. Entries whose
/// name collides with a built-in catalog flavor are dropped (the catalog wins).
/// Returns an empty vec if no file exists; a malformed file logs and yields none.
pub fn user_flavors() -> Vec<UserFlavor> {
    for path in user_flavor_paths() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match toml::from_str::<FlavorsFile>(&text) {
            Ok(f) => {
                return f
                    .flavor
                    .into_iter()
                    .filter(|uf| !uf.name.is_empty() && find(&uf.name).is_none())
                    .collect();
            }
            Err(e) => {
                tracing::warn!("ignoring {}: {e}", path.display());
                return Vec::new();
            }
        }
    }
    Vec::new()
}

/// Look up a user-defined flavor by exact name.
pub fn find_user(name: &str) -> Option<UserFlavor> {
    user_flavors().into_iter().find(|f| f.name == name)
}

/// The `flavors.toml` the CLI *writes* user flavors to: `$BSDKRUN_FLAVORS_FILE`
/// if set, else `$XDG_CONFIG_HOME/bsdkrun/flavors.toml` (else `~/.config/…`).
/// (Note: a project-local `bsdkrun.flavors.toml` is read but never written.)
pub fn user_flavor_file_writable() -> Result<PathBuf> {
    if let Ok(explicit) = std::env::var("BSDKRUN_FLAVORS_FILE") {
        if !explicit.is_empty() {
            return Ok(PathBuf::from(explicit));
        }
    }
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
        .context("no HOME/XDG_CONFIG_HOME to place flavors.toml")?;
    Ok(cfg.join("bsdkrun").join("flavors.toml"))
}

/// Load the writable `flavors.toml` (empty if missing/unreadable).
fn load_writable() -> Result<(PathBuf, FlavorsFile)> {
    let path = user_flavor_file_writable()?;
    let file = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| toml::from_str::<FlavorsFile>(&t).ok())
        .unwrap_or_default();
    Ok((path, file))
}

/// Add or replace a user flavor in the writable `flavors.toml`. Returns the path.
/// Rejects names that collide with a built-in catalog flavor.
pub fn upsert_user_flavor(flavor: UserFlavor) -> Result<PathBuf> {
    if flavor.name.is_empty() {
        anyhow::bail!("a flavor name is required");
    }
    if find(&flavor.name).is_some() {
        anyhow::bail!(
            "{:?} is a built-in catalog flavor name — pick another",
            flavor.name
        );
    }
    let (path, mut file) = load_writable()?;
    file.flavor.retain(|f| f.name != flavor.name);
    file.flavor.push(flavor);
    file.flavor.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = toml::to_string_pretty(&file).context("serializing flavors.toml")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Remove a user flavor from the writable `flavors.toml`. Returns whether one
/// was removed.
pub fn remove_user_flavor(name: &str) -> Result<bool> {
    let (path, mut file) = load_writable()?;
    let before = file.flavor.len();
    file.flavor.retain(|f| f.name != name);
    if file.flavor.len() == before {
        return Ok(false);
    }
    let text = toml::to_string_pretty(&file).context("serializing flavors.toml")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}
