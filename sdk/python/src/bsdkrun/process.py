"""Run the ``bsdkrun`` CLI and capture its output.

Every invocation is prepended with the global ``--log-level`` flag (default 0)
so the SDK's captured output stays clean. Raise it for boot diagnostics.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Mapping
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

    completed = subprocess.run(
        [binary, *_with_globals(args, log_level)],
        env=child_env,
        input=input_bytes,
        capture_output=True,
        check=False,
    )
    return RawResult(
        stdout=completed.stdout.decode("utf-8", "replace"),
        stderr=completed.stderr.decode("utf-8", "replace"),
        exit_code=completed.returncode,
    )


def run_checked(
    args: list[str],
    label: str,
    *,
    env: Mapping[str, str] | None = None,
    stdin: str | bytes | None = None,
    log_level: int | None = None,
) -> RawResult:
    """Like :func:`run`, but raise :class:`CommandFailed` on a non-zero exit."""
    result = run(args, env=env, stdin=stdin, log_level=log_level)
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
