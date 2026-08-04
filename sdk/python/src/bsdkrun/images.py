"""Host-level image operations."""

from __future__ import annotations

import builtins
import json

from .process import run_checked
from .types import ImageInfo

__all__ = ["list"]


def list() -> builtins.list[ImageInfo]:
    """List downloaded images (pulled OCI images + fetched BSD images)."""
    res = run_checked(["images", "--json"], "bsdkrun images")
    rows = json.loads(res.stdout or "[]")
    return [ImageInfo.from_row(row) for row in rows]
