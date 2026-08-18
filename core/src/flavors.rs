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

/// The GitHub CLI, in every agent sandbox.
///
/// An agent that can read a repository but not its issues, PRs or checks is
/// working with half the context — and `gh` is also how it authenticates git
/// over HTTPS without a key.
///
/// Two routes, because the sandboxes are not one distro: alpine (nanoclaw is
/// `docker:dind`) has it as a package, and debian needs GitHub's own apt repo
/// — Debian's archive does not carry `gh` at all, so `apt-get install gh`
/// alone fails on every one of these images. `|| true` throughout, since a
/// sandbox without `gh` still works; the agent is already installed by here.
macro_rules! gh_cli {
    () => {
        "command -v gh >/dev/null 2>&1 || \
         (command -v apk >/dev/null 2>&1 && apk add --no-cache github-cli) || \
         (command -v apt-get >/dev/null 2>&1 && \
          apt-get update && \
          apt-get install -y curl ca-certificates && \
          mkdir -p -m 755 /etc/apt/keyrings && \
          curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
            -o /etc/apt/keyrings/githubcli-archive-keyring.gpg && \
          chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg && \
          echo \"deb [arch=$(dpkg --print-architecture) \
signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] \
https://cli.github.com/packages stable main\" \
            > /etc/apt/sources.list.d/github-cli.list && \
          apt-get update && apt-get install -y gh) || true"
    };
}

/// Determinate Nix, for an agent that needs a toolchain the sandbox lacks.
///
/// Three flags, each load-bearing:
///
/// - `--init none` — there is no systemd for the daemon to hook into, in a
///   container or in one of these guests. Single-user mode is what works, and
///   what an agent running as root wants anyway.
/// - `--extra-conf "sandbox = false"` — Nix's build sandbox needs mount and
///   user namespaces that `docker build` does not grant, so without this the
///   install dies at `setup_default_profile` while building an empty profile:
///   *"while setting up the build environment"*. This is what the installer's
///   own Docker instructions say to pass. Nothing is lost by it here: the
///   isolation these builds rely on is the container, and later the microVM.
/// - `--no-confirm` — no tty in a build.
///
/// The symlink is the other half. The installer puts `nix` on `PATH` through
/// `/etc/profile.d`, which only a *login* shell reads — and the sandbox's
/// wrapper is not one, so `nix` would be installed and still "not found".
///
/// **No `|| true`.** It used to end with one, and that is precisely how the
/// missing `sandbox = false` went unnoticed: the installer failed, the step
/// still reported success, and the published image simply had no `nix` in it —
/// discovered only when someone ran `nix` in a sandbox. A broken install is
/// now a failed build, which is where the error is still readable.
///
/// The leading `command -v nix` is a skip-if-present guard, not error
/// suppression: it keeps a rebuild over a warm layer cheap.
macro_rules! nix_engine {
    () => {
        "command -v nix >/dev/null 2>&1 || \
         ((curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix \
           | sh -s -- install linux --extra-conf 'sandbox = false' --init none --no-confirm) && \
          ln -sf /nix/var/nix/profiles/default/bin/nix /usr/local/bin/nix)"
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
///
/// **Put the volatile step last.** Each entry becomes its own `RUN` layer in
/// the generated Dockerfile, and a layer invalidates every layer after it. The
/// agent CLIs move constantly while `gh`, Docker and Nix do not, so installing
/// the agent first meant every published image rebuilt those three from
/// scratch on each agent release — minutes of CI, and a cache that never hit.
/// Ordering it last leaves the expensive, stable layers untouched.
///
/// It is also the right order for a live VM provision, where the agent install
/// is the step most likely to fail and the cheapest to retry.
macro_rules! provision {
    () => { &[] as &[&str] };
    ($($cmd:expr),+ $(,)?) => { &[$($cmd),+] as &[&str] };
}

/// The image every node-based agent sandbox is built on.
///
/// One constant rather than nine literals: the shared cache base is only a
/// cache hit if it starts `FROM` the same thing the flavors do, and a flavor
/// left behind on an older node would silently miss every layer.
pub const AGENT_BASE_IMAGE: &str = "node:24";

/// Where prebuilt flavor images are published.
///
/// A provisioned flavor's first launch installs a toolchain in a booted VM,
/// which takes minutes. The same steps as a Dockerfile produce the same
/// rootfs in CI, so a launch pulls that instead and boots immediately —
/// falling back to the local build when the image is missing or unreachable.
pub const PREBUILT_REGISTRY: &str = "ghcr.io/tsirysndr";

/// The published image for a flavor, e.g.
/// `ghcr.io/tsirysndr/bsdkrun-flavor-claude-code:latest`.
pub fn prebuilt_image(flavor: &str) -> String {
    format!("{PREBUILT_REGISTRY}/bsdkrun-flavor-{flavor}:latest")
}

/// The Dockerfile that reproduces a flavor's provisioning.
///
/// Generated from the catalog rather than written by hand, so a change to a
/// flavor's steps cannot silently diverge from the image CI publishes — the
/// `flavors/` tree is regenerated and a CI check fails when it drifts.
///
/// `None` for a flavor with nothing to provision (a bare OCI base is already
/// what a launch would pull) or one that is not OCI-based at all.
pub fn dockerfile(flavor: &str) -> Option<String> {
    let c = find(flavor)?;
    let image = match c.base {
        Base::Oci(image) => image,
        // A BSD flavor boots a disk image, not a rootfs; there is nothing for
        // a Dockerfile to express.
        _ => return None,
    };
    if c.provision.is_empty() && c.nix.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str(&format!(
        "# Generated by `bsdkrun flavor __dockerfiles` — do not edit.\n\
         #\n\
         # Flavor: {name} ({category})\n\
         # {description}\n\
         #\n\
         # Published as {published} and pulled by `bsdkrun flavor run {name}`,\n\
         # so a first launch does not have to provision a VM.\n\
         FROM {image}\n\n\
         # bsdkrun runs a guest as root with no service manager, and provisions\n\
         # in one shell per step — the same shape as these RUN layers.\n\
         SHELL [\"/bin/sh\", \"-c\"]\n",
        name = c.name,
        category = c.category,
        description = c.description,
        published = prebuilt_image(c.name),
        image = image,
    ));
    for env in c.env {
        out.push_str(&format!("ENV {env}\n"));
    }
    if !c.env.is_empty() {
        out.push('\n');
    }
    for step in c.provision {
        // One RUN per step, matching the guest's one-shell-per-step execution
        // so a step that depends on the previous one's side effects behaves
        // the same in both places.
        out.push_str(&format!("RUN {step}\n\n"));
    }
    // `nix` packages are installed by the guest-side provisioner, which needs
    // a booted VM (the installer wants /proc and a writable /nix). A flavor
    // that uses them still builds locally.
    if !c.nix.is_empty() {
        out.push_str(
            "# This flavor also installs Nix packages, which need a booted VM;\n\
             # `bsdkrun` finishes those on first launch.\n",
        );
    }
    Some(out)
}

/// The name of the shared cache-seed image. Underscore-prefixed because it is
/// not a flavor: nothing launches it, and the publish workflow skips it.
pub const BASE_IMAGE_DIR: &str = "_base";

/// The steps every agent sandbox shares, in the order they appear in each
/// agent flavor's Dockerfile.
///
/// Layer identity is (parent, command), so these are only *one* cached layer
/// each across all the agent images if they run before anything that differs —
/// which is why the per-flavor `apt!` comes after them rather than first, even
/// though installing git early would read more naturally.
const SHARED_AGENT_STEPS: &[&str] = &[gh_cli!(), docker_engine!(), nix_engine!()];

/// The base every node-based agent flavor starts with, as a Dockerfile.
///
/// Built once in CI and written to a shared build cache, so nine agent images
/// do not each spend minutes installing the same `gh`, Docker and Nix. It is
/// generated from the same constants as the flavors themselves — a hand-copied
/// base would drift, and a drifted base is worse than none: every layer misses
/// and the build silently does the work twice.
pub fn base_dockerfile() -> String {
    let mut out = String::from(
        "# Generated by `bsdkrun flavor __dockerfiles` — do not edit.\n\
         #\n\
         # Not a flavor: this is the layer prefix every agent sandbox shares,\n\
         # built once in CI so the per-agent images can read it from cache\n\
         # instead of installing gh, Docker and Nix nine times over.\n\
         FROM ",
    );
    out.push_str(AGENT_BASE_IMAGE);
    out.push_str("\n\nSHELL [\"/bin/sh\", \"-c\"]\n\n");
    for step in SHARED_AGENT_STEPS {
        out.push_str(&format!("RUN {step}\n\n"));
    }
    out
}

/// Every flavor that has a Dockerfile, as `(name, contents)`, plus the shared
/// agent base.
pub fn dockerfiles() -> Vec<(&'static str, String)> {
    catalog()
        .iter()
        .filter_map(|c| dockerfile(c.name).map(|d| (c.name, d)))
        .chain(std::iter::once((BASE_IMAGE_DIR, base_dockerfile())))
        .collect()
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
            description: "Gleam 1.18 on the BEAM (official image).",
            // The project's own image rather than debian + Nix. It is
            // `erlang:latest` plus the gleam binary, so erlang and rebar3 —
            // the whole reason the Nix list existed — are already in it, and a
            // launch pulls instead of provisioning. Multi-arch (verified
            // linux/amd64 + linux/arm64), which the Nix path got for free and
            // a single-arch image would have quietly broken on Apple silicon.
            //
            // Pinned to a release tag: `latest` here would change the language
            // version under a project without anything in this repo moving.
            base: Base::Oci("ghcr.io/gleam-lang/gleam:v1.18.1-erlang"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "uv",
            category: "language",
            description: "Python 3.13 with uv 0.12 (Astral's installer/resolver).",
            // Astral's own image, pinned to both versions it carries: the uv
            // release and the Python it is built against. `python3.13-trixie`
            // alone would move either one underneath a project.
            base: Base::Oci("ghcr.io/astral-sh/uv:0.12.5-python3.13-trixie"),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: NONE,
        },
        CatalogFlavor {
            name: "mise",
            category: "language",
            description: "mise — one runtime manager for node, python, go, rust…",
            // The project's own image, pinned. mise releases date-versioned and
            // often, so `latest` here would mean a different tool on a rebuild.
            base: Base::Oci("ghcr.io/jdx/mise:2026.8.8"),
            ports: NONE,
            env: NONE,
            nix: NONE,
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
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git", "ripgrep"],
                npm!["@anthropic-ai/claude-code"],
            ],
        },
        CatalogFlavor {
            name: "codex",
            category: "ai",
            description: "OpenAI Codex CLI coding agent.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git"],
                npm!["@openai/codex"],
            ],
        },
        CatalogFlavor {
            name: "gemini",
            category: "ai",
            description: "Gemini CLI — Google's terminal coding agent.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git", "ripgrep"],
                npm!["@google/gemini-cli"],
            ],
        },
        CatalogFlavor {
            name: "kilo",
            category: "ai",
            description: "Kilo Code — terminal AI coding agent.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git"],
                npm!["@kilocode/cli"],
            ],
        },
        CatalogFlavor {
            name: "qwen",
            category: "ai",
            description: "Qwen Code — Alibaba's terminal coding agent.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git"],
                npm!["@qwen-code/qwen-code"],
            ],
        },
        CatalogFlavor {
            name: "kiro-cli",
            category: "ai",
            description: "Kiro CLI — AWS's agentic coding CLI.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            // The one agent installed by `curl | bash` rather than a package
            // manager. AWS ships Kiro CLI as a zip from its own CDN
            // (`prod.download.cli.kiro.dev/stable/latest/kirocli-<arch>-linux.zip`,
            // verified present for both aarch64 and x86_64); the `kiro-cli` npm
            // package is an unrelated 0.0.1 placeholder installing a `kirox`
            // binary, so npm would produce a flavor whose agent does not exist.
            //
            // `unzip` is what the installer needs and what it fails on — it
            // checks for it up front and exits, which in a build reads as a
            // download problem.
            //
            // `--force` skips its "existing installation found" branch, the
            // only path that reads from /dev/tty. There is no tty in a build,
            // and a rebuild over a cached layer is exactly when it would hit.
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git", "curl", "unzip"],
                "curl -fsSL https://cli.kiro.dev/install | bash -s -- --force",
                // The installer puts it in ~/.local/bin, which is not on PATH
                // in a non-login shell — where the sandbox's wrapper runs it.
                "ln -sf \"$HOME/.local/bin/kiro-cli\" /usr/local/bin/kiro-cli",
            ],
        },
        CatalogFlavor {
            name: "opencode",
            category: "ai",
            description: "OpenCode — open-source terminal AI coding agent.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git", "curl"],
                concat!(
                    npm!["opencode-ai"],
                    " || curl -fsSL https://opencode.ai/install | bash"
                ),
            ],
        },
        CatalogFlavor {
            name: "crush",
            category: "ai",
            description: "Crush — Charm's glamourous terminal AI coding agent.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git", "curl"],
                npm!["@charmland/crush"],
            ],
        },
        CatalogFlavor {
            name: "copilot",
            category: "ai",
            description: "GitHub Copilot CLI coding agent.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git"],
                npm!["@github/copilot"],
            ],
        },
        // These two are *assistants*, not TUI coding agents: they run as
        // services and talk over messaging channels, so they are flavors to
        // boot rather than entries in `bsdkrun ai`'s dropdown.
        CatalogFlavor {
            name: "openclaw",
            category: "ai",
            description: "OpenClaw — personal AI assistant / multi-channel gateway.",
            base: Base::Oci(AGENT_BASE_IMAGE),
            ports: NONE,
            env: NONE,
            nix: NONE,
            provision: provision![
                gh_cli!(),
                docker_engine!(),
                nix_engine!(),
                apt!["git", "ripgrep"],
                npm!["openclaw"],
            ],
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
                gh_cli!(),
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
        CatalogFlavor {
            name: "dragonfly",
            category: "service",
            description: "DragonflyDB 1.40 — Redis-compatible in-memory store.",
            // Pinned, and from ghcr.io rather than Docker Hub: the Hub mirror
            // `dragonflydb/dragonfly` has not moved since v1.27.1 and is
            // amd64-only, so on Apple silicon it would either fail to pull or
            // land an emulated guest. The ghcr image is multi-arch (verified
            // linux/amd64 + linux/arm64 for v1.40.1).
            base: Base::Oci("ghcr.io/dragonflydb/dragonfly:v1.40.1"),
            // The Redis port, because that is the point: existing clients
            // connect without knowing what is answering.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent's own install must be the **last** provisioning step.
    ///
    /// Each step is a `RUN` layer, and a changed layer invalidates every layer
    /// below it. The agent CLIs release constantly; `gh`, Docker and Nix do
    /// not. With the agent first, every published image rebuilt those three on
    /// each agent release — so this ordering is a cache property, not a style
    /// preference, and appending a shared step to one of these lists would
    /// silently undo it.
    #[test]
    fn agent_flavors_install_the_agent_last() {
        const SHARED: &[&str] = &["command -v gh ", "command -v dockerd ", "command -v nix "];
        for c in catalog().iter().filter(|c| c.category == "ai") {
            let Some(last) = c.provision.last() else {
                continue;
            };
            // A flavor that has none of the shared steps has nothing to order.
            if !c
                .provision
                .iter()
                .any(|s| SHARED.iter().any(|m| s.starts_with(m)))
            {
                continue;
            }
            assert!(
                !SHARED.iter().any(|m| last.starts_with(m)),
                "{} ends with a shared step ({last:.40}…) — the agent install has to \
                 come last, or every agent release rebuilds gh/docker/nix",
                c.name
            );
        }
    }

    /// The shared base must be a prefix of every agent Dockerfile's
    /// *instructions*.
    ///
    /// This is the whole mechanism: Docker keys a layer on (parent, command),
    /// so a base whose steps differ caches nothing while still building
    /// successfully — CI stays green and quietly does the work nine times. The
    /// failure is invisible without this test.
    ///
    /// Comments and blank lines are dropped before comparing, because the
    /// parser drops them too: the flavor files carry an explanatory comment
    /// above `SHELL` that the base has no reason to repeat, and it changes
    /// nothing about which layers are reused.
    #[test]
    fn the_shared_base_is_a_prefix_of_every_agent_dockerfile() {
        fn instructions(dockerfile: &str) -> Vec<&str> {
            dockerfile
                .lines()
                .map(str::trim_end)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect()
        }

        let base_src = base_dockerfile();
        let base = instructions(&base_src);
        assert!(base.len() > 1, "the base should carry FROM plus its steps");

        for c in catalog().iter().filter(|c| c.category == "ai") {
            let Base::Oci(image) = c.base else { continue };
            if image != AGENT_BASE_IMAGE {
                continue;
            }
            let df = dockerfile(c.name).expect("an agent flavor provisions something");
            let flavor = instructions(&df);
            for (n, want) in base.iter().enumerate() {
                let got = flavor.get(n).copied().unwrap_or("<missing>");
                assert_eq!(
                    got, *want,
                    "{} diverges from the shared base at instruction {n}, so every \
                     layer from there on misses the cache",
                    c.name
                );
            }
        }
    }

    /// Agent sandboxes track the current Node LTS together; a straggler means
    /// one agent silently runs on an older runtime than the rest.
    #[test]
    fn node_based_agent_flavors_agree_on_one_base() {
        let bases: Vec<&str> = catalog()
            .iter()
            .filter(|c| c.category == "ai")
            .filter_map(|c| match c.base {
                Base::Oci(image) if image.starts_with("node:") => Some(image),
                _ => None,
            })
            .collect();
        assert!(!bases.is_empty(), "the agent flavors are all node-based");
        for base in &bases {
            assert_eq!(
                *base, AGENT_BASE_IMAGE,
                "an agent flavor is still on {base} while the rest moved on"
            );
        }
    }
}
