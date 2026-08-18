//! CI workflows defined in code instead of YAML.
//!
//! The builder produces exactly the file `bsdkrun ci` (and tangled's spindle)
//! consumes — [`CiWorkflow::yaml`] is that file, [`CiWorkflow::save`] commits
//! it to `.tangled/workflows/`, and [`CiWorkflow::run`] executes it in a
//! microVM without a file ever touching the repository:
//!
//! ```no_run
//! use bsdkrun_sdk::ci;
//!
//! ci::workflow("test")
//!     .on_push(["main"])
//!     .deps(["rustc", "cargo"])
//!     .env("CARGO_INCREMENTAL", "0")
//!     .step("check", "cargo check")
//!     .step("test", "cargo test")
//!     .run()?;
//! # Ok::<(), bsdkrun_sdk::Error>(())
//! ```
//!
//! Code is the source of truth and YAML the wire format, in that order —
//! which is why `save` writes a generated-file header: a hand-edit there will
//! be overwritten by the next `save`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Start a CI workflow definition.
pub fn workflow(name: impl Into<String>) -> CiWorkflow {
    CiWorkflow {
        name: name.into(),
        engine: "nixery".to_string(),
        when: Vec::new(),
        deps: BTreeMap::new(),
        env: BTreeMap::new(),
        steps: Vec::new(),
        clone_depth: None,
        clone_skip: false,
    }
}

/// A workflow under construction. See the module docs for the shape.
#[derive(Debug, Clone)]
pub struct CiWorkflow {
    name: String,
    engine: String,
    when: Vec<(Vec<String>, Vec<String>)>,
    // BTreeMaps for deterministic output: the emitted YAML is committed and
    // diffed, so its ordering must not depend on hash seeds.
    deps: BTreeMap<String, Vec<String>>,
    env: BTreeMap<String, String>,
    steps: Vec<CiStep>,
    clone_depth: Option<u32>,
    clone_skip: bool,
}

#[derive(Debug, Clone)]
struct CiStep {
    name: String,
    command: String,
    env: BTreeMap<String, String>,
}

impl CiWorkflow {
    /// Override the engine (`nixery` by default).
    pub fn engine(mut self, engine: impl Into<String>) -> Self {
        self.engine = engine.into();
        self
    }

    /// Add a push trigger for the given branches.
    pub fn on_push<I, S>(mut self, branches: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.when.push((
            vec!["push".into()],
            branches.into_iter().map(Into::into).collect(),
        ));
        self
    }

    /// Add a pull_request trigger targeting the given branches.
    pub fn on_pull_request<I, S>(mut self, branches: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.when.push((
            vec!["pull_request".into()],
            branches.into_iter().map(Into::into).collect(),
        ));
        self
    }

    /// Add nixpkgs dependencies — the toolchain the steps run against.
    pub fn deps<I, S>(mut self, packages: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.deps
            .entry("nixpkgs".into())
            .or_default()
            .extend(packages.into_iter().map(Into::into));
        self
    }

    /// Add dependencies from a custom registry (a flake reference).
    pub fn deps_from<I, S>(mut self, registry: impl Into<String>, packages: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.deps
            .entry(registry.into())
            .or_default()
            .extend(packages.into_iter().map(Into::into));
        self
    }

    /// Set a workflow-level environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Append a step. Steps run serially, in one VM, from the workspace root.
    pub fn step(mut self, name: impl Into<String>, command: impl Into<String>) -> Self {
        self.steps.push(CiStep {
            name: name.into(),
            command: command.into(),
            env: BTreeMap::new(),
        });
        self
    }

    /// Append a step with step-scoped environment variables.
    pub fn step_env(
        mut self,
        name: impl Into<String>,
        command: impl Into<String>,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.steps.push(CiStep {
            name: name.into(),
            command: command.into(),
            env: env.into_iter().collect(),
        });
        self
    }

    /// Set the clone depth (default 1).
    pub fn clone_depth(mut self, depth: u32) -> Self {
        self.clone_depth = Some(depth);
        self
    }

    /// Skip the checkout entirely.
    pub fn skip_clone(mut self) -> Self {
        self.clone_skip = true;
        self
    }

    /// The file name [`CiWorkflow::save`] writes: `<name>.yml`.
    pub fn file_name(&self) -> String {
        if self.name.ends_with(".yml") || self.name.ends_with(".yaml") {
            self.name.clone()
        } else {
            format!("{}.yml", self.name)
        }
    }

    /// Render the workflow file.
    ///
    /// Scalars are emitted as JSON strings — valid YAML by construction —
    /// and commands as literal blocks when safe, so the SDK needs no YAML
    /// dependency.
    pub fn yaml(&self) -> String {
        let mut out = String::new();

        if !self.when.is_empty() {
            out.push_str("when:\n");
            for (events, branches) in &self.when {
                let evs: Vec<String> = events.iter().map(|e| json_str(e)).collect();
                let _ = writeln!(out, "  - event: [{}]", evs.join(", "));
                match branches.len() {
                    0 => {}
                    1 => {
                        let _ = writeln!(out, "    branch: {}", json_str(&branches[0]));
                    }
                    _ => {
                        let bs: Vec<String> = branches.iter().map(|b| json_str(b)).collect();
                        let _ = writeln!(out, "    branch: [{}]", bs.join(", "));
                    }
                }
            }
            out.push('\n');
        }

        let _ = writeln!(out, "engine: {}", self.engine);

        if !self.deps.is_empty() {
            out.push_str("\ndependencies:\n");
            for (reg, pkgs) in &self.deps {
                let _ = writeln!(out, "  {}:", json_str(reg));
                for p in pkgs {
                    let _ = writeln!(out, "    - {}", json_str(p));
                }
            }
        }

        if !self.env.is_empty() {
            out.push_str("\nenvironment:\n");
            for (k, v) in &self.env {
                let _ = writeln!(out, "  {k}: {}", json_str(v));
            }
        }

        if self.clone_skip || self.clone_depth.is_some() {
            out.push_str("\nclone:\n");
            if self.clone_skip {
                out.push_str("  skip: true\n");
            }
            if let Some(d) = self.clone_depth {
                let _ = writeln!(out, "  depth: {d}");
            }
        }

        out.push_str("\nsteps:\n");
        for s in &self.steps {
            let _ = writeln!(out, "  - name: {}", json_str(&s.name));
            write_command(&mut out, &s.command);
            if !s.env.is_empty() {
                out.push_str("    environment:\n");
                for (k, v) in &s.env {
                    let _ = writeln!(out, "      {k}: {}", json_str(v));
                }
            }
        }
        out
    }

    /// Write the workflow into `<repo>/.tangled/workflows/`, where spindle
    /// and `bsdkrun ci` both discover it. Returns the path.
    pub fn save(&self, repo: impl AsRef<Path>) -> Result<PathBuf> {
        let dir = repo.as_ref().join(".tangled").join("workflows");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(self.file_name());
        std::fs::write(
            &path,
            format!(
                "# Generated by the bsdkrun SDK — edit the code that save()d it instead.\n{}",
                self.yaml()
            ),
        )?;
        Ok(path)
    }

    /// Execute the workflow in a microVM against the current directory,
    /// streaming output. The YAML never touches the repository — it goes to
    /// a temp file and `bsdkrun ci run -f`.
    pub fn run(&self) -> Result<()> {
        self.run_in::<&str>(None)
    }

    /// [`CiWorkflow::run`] against an explicit repository directory.
    pub fn run_in<P: AsRef<Path>>(&self, dir: Option<P>) -> Result<()> {
        let tmp = std::env::temp_dir().join(format!("bsdkrun-ci-{}", std::process::id()));
        std::fs::create_dir_all(&tmp)?;
        let file = tmp.join(self.file_name());
        std::fs::write(&file, self.yaml())?;

        let mut args: Vec<String> = vec![
            "ci".into(),
            "run".into(),
            "-f".into(),
            file.display().to_string(),
        ];
        if let Some(d) = &dir {
            args.push("-w".into());
            args.push(d.as_ref().display().to_string());
        }
        let code = crate::process::spawn(&args)?;
        let _ = std::fs::remove_dir_all(&tmp);
        if code != 0 {
            return Err(Error::CommandFailed {
                command: format!("bsdkrun ci run ({})", self.name),
                exit_code: code,
                stdout: String::new(),
                stderr: format!("workflow {} failed", self.name),
            });
        }
        Ok(())
    }
}

/// A literal block when it round-trips byte-for-byte; a JSON string when it
/// cannot (trailing spaces, carriage returns) — never a silent alteration.
fn write_command(out: &mut String, cmd: &str) {
    let block_safe =
        !cmd.is_empty() && !cmd.contains('\r') && cmd.lines().all(|l| l == l.trim_end_matches(' '));
    if !block_safe {
        let _ = writeln!(out, "    command: {}", json_str(cmd));
        return;
    }
    out.push_str("    command: |\n");
    for line in cmd.trim_end_matches('\n').lines() {
        let _ = writeln!(out, "      {line}");
    }
}

/// A JSON string literal, which is a valid YAML scalar by construction.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The YAML this builder emits is consumed by tangled's own workflow
    // parser (inside `bsdkrun ci`), so these tests pin the emitted shape —
    // a change here is a change to what spindle would receive.

    #[test]
    fn renders_the_full_workflow_shape() {
        let y = workflow("test")
            .on_push(["main"])
            .on_pull_request(["main", "develop"])
            .deps(["rustc", "cargo"])
            .deps_from("github:nix-community/fenix/abc123", ["stable.default"])
            .env("CARGO_INCREMENTAL", "0")
            .clone_depth(100)
            .step("check", "cargo check")
            .step_env(
                "test",
                "cargo test",
                [("RUST_BACKTRACE".to_string(), "1".to_string())],
            )
            .yaml();

        assert!(y.contains("  - event: [\"push\"]\n    branch: \"main\""));
        assert!(y.contains("branch: [\"main\", \"develop\"]"));
        assert!(y.contains("engine: nixery"));
        assert!(y.contains("\"nixpkgs\":\n    - \"rustc\"\n    - \"cargo\""));
        assert!(y.contains("\"github:nix-community/fenix/abc123\":"));
        assert!(y.contains("CARGO_INCREMENTAL: \"0\""));
        assert!(y.contains("depth: 100"));
        assert!(y.contains("- name: \"check\"\n    command: |\n      cargo check"));
        assert!(y.contains("environment:\n      RUST_BACKTRACE: \"1\""));
    }

    #[test]
    fn block_unsafe_commands_fall_back_to_json() {
        // Trailing spaces do not survive a literal block scalar; the emitter
        // must switch representation rather than silently altering the
        // command.
        let y = workflow("edge").step("tricky", "echo 'a'  \necho b").yaml();
        assert!(y.contains("command: \"echo 'a'  \\necho b\""), "{y}");
    }

    #[test]
    fn file_names_get_the_yml_suffix() {
        assert_eq!(workflow("build").file_name(), "build.yml");
        assert_eq!(workflow("build.yaml").file_name(), "build.yaml");
    }
}
