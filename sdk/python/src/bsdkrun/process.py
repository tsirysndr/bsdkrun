"""Run the ``bsdkrun`` CLI and capture its output.

Every invocation is prepended with the global ``--log-level`` flag (default 0)
so the SDK's captured output stays clean. Raise it for boot diagnostics.
"""

from __future__ import annotations

import os
import subprocess
import threading
from collections.abc import Callable, Mapping
from dataclasses import dataclass

from .binary import resolve_binary
from .errors import CommandFailed

__all__ = ["RawResult", "run", "run_checked", "spawn"]


@dataclass(frozen=True)
class RawResult:
    """The buffered result of a ``bsdkrun`` invocation."""

    stdout: str
    stderr: str
    exit_code: int


def _with_globals(args: list[str], log_level: int | None) -> list[str]:
    """Prepend the global ``--log-level`` flag."""
    return ["--log-level", str(log_level if log_level is not None else 0), *args]


def run(
    args: list[str],
    *,
    env: Mapping[str, str] | None = None,
    stdin: str | bytes | None = None,
    log_level: int | None = None,
    on_stdout: Callable[[bytes], None] | None = None,
    on_stderr: Callable[[bytes], None] | None = None,
) -> RawResult:
    """Run ``bsdkrun <args>`` to completion, buffering stdout/stderr.

    ``env`` is merged onto the current process environment. ``stdin``, if given,
    is piped to the child. ``log_level`` sets bsdkrun's global ``--log-level``
    (defaults to 0 — quiet).
    """
    binary = resolve_binary()
    child_env = dict(os.environ)
    if env:
        child_env.update(env)

    input_bytes: bytes | None
    if stdin is None:
        input_bytes = None
    elif isinstance(stdin, bytes):
        input_bytes = stdin
    else:
        input_bytes = stdin.encode("utf-8")

    child = subprocess.Popen(
        [binary, *_with_globals(args, log_level)],
        env=child_env,
        stdin=subprocess.PIPE if input_bytes is not None else None,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    stdout = bytearray()
    stderr = bytearray()

    def drain(pipe: object, captured: bytearray, callback: Callable[[bytes], None] | None) -> None:
        while chunk := pipe.read(8192):  # type: ignore[attr-defined]
            captured.extend(chunk)
            if callback:
                callback(chunk)

    out_thread = threading.Thread(target=drain, args=(child.stdout, stdout, on_stdout))
    err_thread = threading.Thread(target=drain, args=(child.stderr, stderr, on_stderr))
    out_thread.start()
    err_thread.start()
    if input_bytes is not None and child.stdin is not None:
        child.stdin.write(input_bytes)
        child.stdin.close()
    exit_code = child.wait()
    out_thread.join()
    err_thread.join()
    return RawResult(
        stdout=bytes(stdout).decode("utf-8", "replace"),
        stderr=bytes(stderr).decode("utf-8", "replace"),
        exit_code=exit_code,
    )


def run_checked(
    args: list[str],
    label: str,
    *,
    env: Mapping[str, str] | None = None,
    stdin: str | bytes | None = None,
    log_level: int | None = None,
    on_stdout: Callable[[bytes], None] | None = None,
    on_stderr: Callable[[bytes], None] | None = None,
) -> RawResult:
    """Like :func:`run`, but raise :class:`CommandFailed` on a non-zero exit."""
    result = run(
        args, env=env, stdin=stdin, log_level=log_level, on_stdout=on_stdout, on_stderr=on_stderr
    )
    if result.exit_code != 0:
        raise CommandFailed(result.exit_code, result.stdout, result.stderr, label)
    return result


def spawn(
    args: list[str],
    *,
    env: Mapping[str, str] | None = None,
    log_level: int | None = None,
) -> int:
    """Run ``bsdkrun <args>`` inheriting the parent's stdio (interactive).

    Blocks until the child exits and returns its exit code. Used by
    :meth:`~bsdkrun.sandbox.Sandbox.shell`.
    """
    binary = resolve_binary()
    child_env = dict(os.environ)
    if env:
        child_env.update(env)
    completed = subprocess.run(
        [binary, *_with_globals(args, log_level)],
        env=child_env,
        check=False,
    )
    return completed.returncode
