//! Files in a running sandbox — [`Sandbox::fs`](crate::Sandbox::fs).
//!
//! Every call goes through the guest's exec agent, so the sandbox has to be
//! running; there is no offline write.

use std::path::Path;

use crate::error::{Error, Result};
use crate::process::{run, run_binary};

/// Read and write files inside a running microVM.
///
/// ```no_run
/// # use bsdkrun_sdk::Sandbox;
/// # fn main() -> bsdkrun_sdk::Result<()> {
/// let sbx = Sandbox::get("web")?;
/// sbx.fs().write_file("/app/main.py", b"print('hi')")?;
/// let out = sbx.fs().read_to_string("/app/out.json")?;
/// sbx.fs().upload("./src", "/app/src")?;
/// sbx.fs().download("/app/dist", "./dist", true)?;
/// # Ok(())
/// # }
/// ```
pub struct FileSystem {
    id: String,
}

impl FileSystem {
    pub(crate) fn new(id: impl Into<String>) -> Self {
        FileSystem { id: id.into() }
    }

    /// Write `data` to `path` in the guest, creating parent directories.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<()> {
        let args = vec![
            "cp".to_string(),
            "-".to_string(),
            format!("{}:{}", self.id, path),
        ];
        let res = run_binary(args, Some(data))?;
        if res.exit_code != 0 {
            return Err(transfer_error(&res.stderr, path));
        }
        Ok(())
    }

    /// Read `path` from the guest as bytes.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let args = vec![
            "cp".to_string(),
            format!("{}:{}", self.id, path),
            "-".to_string(),
        ];
        let res = run_binary(args, None)?;
        if res.exit_code != 0 {
            return Err(transfer_error(&res.stderr, path));
        }
        Ok(res.stdout)
    }

    /// Read `path` from the guest and decode it as UTF-8 (lossily).
    pub fn read_to_string(&self, path: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.read_file(path)?).into_owned())
    }

    /// Copy a host file or directory into the guest. A directory's *contents*
    /// land in `remote_path`, so `upload("./src", "/app/src")` leaves the
    /// guest's `/app/src` holding what `./src` holds.
    ///
    /// Whether it recurses is decided by looking at `local_path`, so callers do
    /// not have to say which kind of thing they are copying.
    pub fn upload(&self, local_path: impl AsRef<Path>, remote_path: &str) -> Result<()> {
        let local = local_path.as_ref();
        let meta = std::fs::metadata(local).map_err(|e| Error::FileTransfer {
            path: local.display().to_string(),
            message: format!("cannot upload {}: {e}", local.display()),
        })?;
        let mut args = vec!["cp".to_string()];
        if meta.is_dir() {
            args.push("-r".to_string());
        }
        args.push(local.display().to_string());
        args.push(format!("{}:{}", self.id, remote_path));
        let res = run(args)?;
        if res.exit_code != 0 {
            return Err(transfer_error(&res.stderr, &local.display().to_string()));
        }
        Ok(())
    }

    /// Copy a file or directory out of the guest onto the host. `recursive`
    /// selects a directory; unlike [`upload`](Self::upload) it cannot be
    /// detected here, because the path lives in the guest and answering would
    /// cost an extra round trip.
    pub fn download(
        &self,
        remote_path: &str,
        local_path: impl AsRef<Path>,
        recursive: bool,
    ) -> Result<()> {
        let mut args = vec!["cp".to_string()];
        if recursive {
            args.push("-r".to_string());
        }
        args.push(format!("{}:{}", self.id, remote_path));
        args.push(local_path.as_ref().display().to_string());
        let res = run(args)?;
        if res.exit_code != 0 {
            return Err(transfer_error(&res.stderr, remote_path));
        }
        Ok(())
    }
}

/// The CLI already explains these well; strip its `Error: ` prefix.
fn transfer_error(stderr: &str, path: &str) -> Error {
    let text = stderr.trim().trim_start_matches("Error: ").trim();
    Error::FileTransfer {
        path: path.to_string(),
        message: if text.is_empty() {
            format!("file transfer failed for {path}")
        } else {
            text.to_string()
        },
    }
}
