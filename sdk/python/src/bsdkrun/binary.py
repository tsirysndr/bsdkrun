"""Locate the ``bsdkrun`` binary on the host.

Resolution order (first match wins, then cached):

1. an explicit override set via :func:`set_binary_path`,
2. the ``$BSDKRUN_BIN`` environment variable,
3. ``bsdkrun`` on ``$PATH``,
4. in-repo dev builds relative to this file:
   ``<repo_root>/target/release/bsdkrun`` then ``.../target/debug/bsdkrun``.
"""

from __future__ import annotations

import os
from pathlib import Path
from shutil import which

from .errors import BinaryNotFound

__all__ = ["set_binary_path", "resolve_binary", "reset_binary_cache"]

_override: str | None = None
_resolved: str | None = None


def set_binary_path(path: str) -> None:
    """Force the SDK to use a specific ``bsdkrun`` binary, bypassing discovery.

    Handy in tests or when running against a locally built debug binary.
    """
    global _override, _resolved
    _override = path
    _resolved = None


def reset_binary_cache() -> None:
    """Reset cached discovery state and any override (mainly for tests)."""
    global _override, _resolved
    _override = None
    _resolved = None


def _repo_root() -> Path:
    # This file lives at sdk/python/src/bsdkrun/binary.py, so the repo root is
    # four directories up (bsdkrun -> src -> python -> sdk -> repo root).
    return Path(__file__).resolve().parents[4]


def _candidates() -> list[str]:
    """Candidate locations, in priority order."""
    out: list[str] = []
    if _override:
        out.append(_override)

    env = os.environ.get("BSDKRUN_BIN")
    if env:
        out.append(env)

    # A `bsdkrun` already on PATH wins over in-repo builds.
    on_path = which("bsdkrun")
    if on_path:
        out.append(on_path)

    repo_root = _repo_root()
    out.append(str(repo_root / "target" / "release" / "bsdkrun"))
    out.append(str(repo_root / "target" / "debug" / "bsdkrun"))
    return out


def resolve_binary() -> str:
    """Resolve (and cache) the path to the ``bsdkrun`` binary.

    Raises :class:`~bsdkrun.errors.BinaryNotFound` if none of the candidate
    locations exist.
    """
    global _resolved
    if _resolved:
        return _resolved

    searched = _candidates()
    for candidate in searched:
        if os.sep in candidate or "/" in candidate:
            if os.path.exists(candidate):
                _resolved = candidate
                return candidate
        else:
            found = which(candidate)
            if found:
                _resolved = candidate
                return candidate
    raise BinaryNotFound(searched)
