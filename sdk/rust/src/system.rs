//! Host toolchain / image operations that aren't tied to a single machine.

use crate::error::Result;
use crate::process::{run, run_checked};

/// Sanity-check the toolchain (libkrun links, a context is creatable).
///
/// Does not boot. `Ok(true)` on success; an `Err` only for host-side
/// failures like a missing binary.
pub fn probe() -> Result<bool> {
    Ok(run(["probe"])?.exit_code == 0)
}

/// Download + prepare a BSD image ahead of time.
///
/// ```no_run
/// bsdkrun::system::fetch_image("freebsd").version("14.3").run()?;
/// # Ok::<(), bsdkrun::Error>(())
/// ```
pub fn fetch_image(os: impl Into<String>) -> FetchImageBuilder {
    FetchImageBuilder {
        os: os.into(),
        version: None,
        dir: None,
        force: false,
    }
}

/// A `bsdkrun fetch` invocation being assembled — see [`fetch_image`].
#[derive(Debug, Clone)]
pub struct FetchImageBuilder {
    os: String,
    version: Option<String>,
    dir: Option<String>,
    force: bool,
}

impl FetchImageBuilder {
    /// The release to fetch (`--version`).
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Download directory (`--dir`).
    pub fn dir(mut self, dir: impl Into<String>) -> Self {
        self.dir = Some(dir.into());
        self
    }

    /// Re-download even if cached (`--force`).
    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    /// Fetch the image. Returns the command output.
    pub fn run(self) -> Result<String> {
        let mut args = vec!["fetch".to_string(), "--os".to_string(), self.os];
        if let Some(version) = self.version {
            args.push("--version".to_string());
            args.push(version);
        }
        if let Some(dir) = self.dir {
            args.push("--dir".to_string());
            args.push(dir);
        }
        if self.force {
            args.push("--force".to_string());
        }
        Ok(run_checked(args, "bsdkrun fetch")?.stdout)
    }
}

/// List the arm64 builds available to fetch for a BSD (`"freebsd"`/`"netbsd"`).
///
/// Returns the non-empty output lines.
pub fn versions(os: &str) -> Result<Vec<String>> {
    let out = run_checked(["versions", "--os", os], "bsdkrun versions")?.stdout;
    Ok(out
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

/// Grow a raw disk image (the guest expands its root FS on next boot).
pub fn grow_disk(disk: &str, size: &str) -> Result<()> {
    run_checked(["grow", "--disk", disk, "--size", size], "bsdkrun grow")?;
    Ok(())
}
