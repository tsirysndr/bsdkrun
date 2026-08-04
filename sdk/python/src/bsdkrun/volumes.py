"""Host-level volume operations."""

from __future__ import annotations

import builtins
import json
from collections.abc import Sequence

from .process import run_checked
from .types import VolumeInfo

__all__ = ["list", "remove"]


def list() -> builtins.list[VolumeInfo]:
    """List persistent volumes."""
    res = run_checked(["volume", "ls", "--json"], "bsdkrun volume ls")
    rows = json.loads(res.stdout or "[]")
    return [VolumeInfo.from_row(row) for row in rows]


def remove(names: str | Sequence[str], *, force: bool = False) -> None:
    """Remove one or more volumes (and their data)."""
    name_list = [names] if isinstance(names, str) else [*names]
    args = ["volume", "rm"]
    if force:
        args.append("--force")
    args += name_list
    run_checked(args, "bsdkrun volume rm")
