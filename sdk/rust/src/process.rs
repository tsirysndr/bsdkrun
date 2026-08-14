//! Run the `bsdkrun` CLI and capture its output.
//!
//! Every invocation is prepended with the global `--log-level` flag (default 0)
//! so the SDK's captured output stays clean. Raise it for boot diagnostics.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use crate::binary::resolve_binary;
use crate::error::{Error, Result};

/// The buffered result of a `bsdkrun` invocation.
#[derive(Debug, Clone)]
pub struct RawResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

fn with_globals(args: &[String], log_level: u32) -> Vec<String> {
    let mut out = vec!["--log-level".to_string(), log_level.to_string()];
    out.extend(args.iter().cloned());
    out
}

/// Run `bsdkrun <args>` to completion, buffering stdout/stderr.
///
/// `env` is merged onto the current process environment. `stdin`, if given, is
/// piped to the child (otherwise the child inherits ours, exactly as Python's
/// `subprocess.run(input=None)` does). `log_level` sets bsdkrun's global
/// `--log-level`.
pub(crate) fn run_full(
    args: &[String],
    env: &[(String, String)],
    stdin: Option<&[u8]>,
    log_level: u32,
) -> Result<RawResult> {
    run_full_stream(args, env, stdin, log_level, None, None)
}

pub(crate) fn run_full_stream(
    args: &[String],
    env: &[(String, String)],
    stdin: Option<&[u8]>,
    log_level: u32,
    mut on_stdout: Option<Box<dyn Write + Send>>,
    mut on_stderr: Option<Box<dyn Write + Send>>,
) -> Result<RawResult> {
    let binary = resolve_binary()?;
    let mut cmd = Command::new(binary);
    cmd.args(with_globals(args, log_level));
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }

    let mut child = cmd.spawn()?;

    // Feed stdin and drain stderr on their own threads so no pipe can fill up
    // and deadlock against our sequential reads — the same job Python's
    // `communicate()` does with its worker threads.
    let stdin_thread = stdin.map(|bytes| {
        let mut handle = child.stdin.take().expect("stdin was piped");
        let owned = bytes.to_vec();
        std::thread::spawn(move || {
            let _ = handle.write_all(&owned);
        })
    });
    let stderr_thread = {
        let mut handle = child.stderr.take().expect("stderr was piped");
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 8192];
            loop {
                match handle.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(w) = on_stderr.as_mut() {
                            let _ = w.write_all(&chunk[..n]);
                        }
                    }
                }
            }
            buf
        })
    };

    let mut stdout_buf = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let mut chunk = [0_u8; 8192];
        loop {
            let n = out.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            stdout_buf.extend_from_slice(&chunk[..n]);
            if let Some(w) = on_stdout.as_mut() {
                w.write_all(&chunk[..n])?;
            }
        }
    }
    let stderr_buf = stderr_thread.join().unwrap_or_default();
    if let Some(t) = stdin_thread {
        let _ = t.join();
    }
    let status = child.wait()?;

    Ok(RawResult {
        stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
        exit_code: status.code().unwrap_or(-1),
    })
}

/// Run `bsdkrun <args>` quietly (log level 0) and capture the result.
pub fn run<I, S>(args: I) -> Result<RawResult>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let argv: Vec<String> = args.into_iter().map(Into::into).collect();
    run_full(&argv, &[], None, 0)
}

/// Like [`run`], but a non-zero exit becomes [`Error::CommandFailed`] tagged
/// with `label`.
pub fn run_checked<I, S>(args: I, label: &str) -> Result<RawResult>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let argv: Vec<String> = args.into_iter().map(Into::into).collect();
    checked(&argv, label)
}

pub(crate) fn checked(args: &[String], label: &str) -> Result<RawResult> {
    let result = run_full(args, &[], None, 0)?;
    if result.exit_code != 0 {
        return Err(Error::CommandFailed {
            exit_code: result.exit_code,
            stdout: result.stdout,
            stderr: result.stderr,
            command: label.to_string(),
        });
    }
    Ok(result)
}

/// Run `bsdkrun <args>` inheriting the parent's stdio (interactive).
///
/// Blocks until the child exits and returns its exit code. Used by
/// [`crate::Sandbox::shell`].
pub fn spawn<I, S>(args: I) -> Result<i32>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let binary = resolve_binary()?;
    let argv: Vec<String> = args.into_iter().map(Into::into).collect();
    let status = Command::new(binary).args(with_globals(&argv, 0)).status()?;
    Ok(status.code().unwrap_or(-1))
}
